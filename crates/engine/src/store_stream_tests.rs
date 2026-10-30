// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #649: a one-file mailbox (format 1) larger than the memory allows is read as a stream: its
//! content goes to the content folder as each message is read, so that the whole file, and all
//! its content, is never in memory. (Small limits under test: `MAX_MAILBOX_JSON` 16 MB, an item
//! 4 MB, 5,000 items.)

use mailcraft_model::{Address, Attachment, Body, Mailbox, Message};

use serde_json::Value;

use super::*;

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-stream-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("mailbox.mailcraft")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Memory reported as scarce for as long as it lives, so that even a small file is streamed.
struct Scarce;

impl Scarce {
    fn new() -> Scarce {
        TEST_AVAILABLE.set(1);
        Scarce
    }
}

impl Drop for Scarce {
    fn drop(&mut self) {
        TEST_AVAILABLE.set(0);
    }
}

/// Bytes that differ from message to message and don't repeat within one.
fn noise(seed: usize, len: usize) -> Vec<u8> {
    let mut x = (seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn text(seed: usize, len: usize) -> String {
    noise(seed, len).iter().map(|b| char::from(b'a' + b % 26)).collect()
}

/// A mailbox of `n` messages, each with a long text, long HTML and an attachment of `attach` bytes
/// (the first of them also with a short one that stays in the message), and a contact.
fn mailbox(n: usize, attach: usize) -> Mailbox {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    for i in 0..n {
        let mut attachments = vec![Attachment::new(format!("scan-{i}.bin"), "application/octet-stream", noise(i, attach))];
        if i == 0 {
            attachments.push(Attachment::new("note.txt", "text/plain", b"short".to_vec()));
        }
        let m = Message {
            folder: inbox,
            subject: format!("Synthetic message {i}"),
            from: Address::new("Blake Rivera", "blake@example.org"),
            to: vec![Address::new("Avery Quinn", "avery@example.org")],
            date: 1_700_000_000 + i as i64,
            body: Body { text: text(i, 5_000), html: Some(format!("<p>{}</p>", text(i + 1_000, 3_000))) },
            attachments,
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    mb.add_contact(mailcraft_model::Contact { display_name: "Ada Aldrin".into(), ..Default::default() });
    mb
}

fn write_one_file(path: &Path, mb: &Mailbox) -> Vec<u8> {
    let bytes = crate::io::write_mailbox(mb).unwrap();
    fs::write(path, &bytes).unwrap();
    bytes
}

#[test]
fn a_one_file_mailbox_over_the_memory_limit_is_streamed_and_loses_nothing() {
    let tmp = Temp::new("big");
    let path = tmp.file();
    let mb = mailbox(40, 600_000);
    let original = write_one_file(&path, &mb);
    assert!(original.len() as u64 > MAX_MAILBOX_JSON, "{} must pass the limit that used to refuse it", original.len());

    let mut opened = open(&path).unwrap();
    assert_eq!(opened.mailbox.messages.len(), 40);
    assert_eq!(opened.mailbox.contacts.len(), 1);
    assert_eq!(opened.unavailable, 0);
    assert_eq!(opened.deferred, 40 * 3);
    // Nothing large is in memory: not an attachment, not a body.
    let in_memory: usize = opened
        .mailbox
        .messages
        .values()
        .map(|m| {
            m.body.text.len()
                + m.body.html.as_ref().map_or(0, String::len)
                + m.hidden_text.len()
                + m.attachments.iter().map(|a| a.data.len()).sum::<usize>()
        })
        .sum();
    assert!(in_memory < 40 * 64, "{in_memory} bytes of content in memory");
    // All of it is on disk, and reads back as it was.
    for (id, m) in &opened.mailbox.messages {
        let was = mb.messages.get(id).unwrap();
        let kept = m.kept.as_deref().unwrap();
        assert_eq!(fetch(&path, &kept.text.unwrap()).unwrap().as_slice(), was.body.text.as_bytes());
        assert_eq!(fetch(&path, &kept.html.unwrap()).unwrap().as_slice(), was.body.html.as_deref().unwrap().as_bytes());
        for (a, b) in m.attachments.iter().zip(&was.attachments) {
            assert_eq!((&a.name, &a.mime), (&b.name, &b.mime));
            match a.kept {
                Some(k) => assert_eq!(fetch(&path, &k).unwrap().as_slice(), b.data.as_slice()),
                None => assert_eq!(a.data, b.data),
            }
        }
    }
    // The file is only read until a save.
    assert_eq!(fs::read(&path).unwrap(), original);

    // The next save (of a change) writes the log form, and keeps the old file beside it.
    let first = *opened.mailbox.messages.keys().next().unwrap();
    opened.mailbox.message_mut(first).unwrap().read = true;
    save(&path, &opened.mailbox).unwrap();
    assert!(is_index(&path).unwrap());
    assert_eq!(fs::read(path.with_file_name("mailbox.mailcraft.format1")).unwrap(), original);

    // And what opens from that has everything.
    let mut again = open(&path).unwrap();
    assert_eq!(again.unavailable, 0);
    assert!(again.mailbox.messages.get(&first).unwrap().read);
    load_all(&path, &mut again.mailbox).unwrap();
    for (id, was) in &mb.messages {
        let m = again.mailbox.messages.get(id).unwrap();
        assert_eq!((&m.body.text, &m.body.html, &m.subject), (&was.body.text, &was.body.html, &was.subject));
        assert_eq!(m.attachments.len(), was.attachments.len());
        for (a, b) in m.attachments.iter().zip(&was.attachments) {
            assert_eq!(a.data, b.data);
        }
    }
}

#[test]
fn a_streamed_mailbox_is_written_as_a_log_even_if_nothing_is_changed() {
    let tmp = Temp::new("unchanged");
    let path = tmp.file();
    write_one_file(&path, &mailbox(3, 5_000));
    let _scarce = Scarce::new();
    let opened = open(&path).unwrap();
    save(&path, &opened.mailbox).unwrap();
    assert!(is_index(&path).unwrap(), "the old file is not read through at every start");
    assert!(path.with_file_name("mailbox.mailcraft.format1").exists());
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 3);
}

#[test]
fn a_file_that_fails_part_way_is_left_untouched() {
    let tmp = Temp::new("part-way");
    let path = tmp.file();
    let whole = write_one_file(&path, &mailbox(6, 5_000));
    let _scarce = Scarce::new();
    // Cut anywhere (and with a byte order mark in front): an error or a mailbox, never a panic,
    // and the file as it was.
    for cut in (0..whole.len()).step_by(whole.len() / 97 + 1) {
        let mut bytes = whole.get(..cut).unwrap().to_vec();
        fs::write(&path, &bytes).unwrap();
        let result = open(&path);
        assert!(result.is_err(), "cut at {cut}");
        assert_eq!(fs::read(&path).unwrap(), bytes);
        bytes.splice(0..0, [0xEF, 0xBB, 0xBF]);
        fs::write(&path, &bytes).unwrap();
        let _ = open(&path);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    // A byte order mark in front of the whole file is skipped.
    let mut with_mark = vec![0xEF, 0xBB, 0xBF];
    with_mark.extend_from_slice(&whole);
    fs::write(&path, &with_mark).unwrap();
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 6);
}

#[test]
fn damaged_bytes_never_panic() {
    let tmp = Temp::new("flips");
    let path = tmp.file();
    let whole = write_one_file(&path, &mailbox(4, 3_000));
    let _scarce = Scarce::new();
    for at in (0..whole.len()).step_by(whole.len() / 211 + 1) {
        for byte in [0u8, b'"', b'{', b'}', b'[', b'9', 0xFF] {
            let mut bytes = whole.clone();
            if let Some(b) = bytes.get_mut(at) {
                *b = byte;
            }
            fs::write(&path, &bytes).unwrap();
            let _ = open(&path);
        }
    }
}

#[test]
fn an_item_over_the_cap_is_refused_and_the_file_left_alone() {
    let tmp = Temp::new("oversize");
    let path = tmp.file();
    // One message with an attachment larger than the cap for an item (4 MB under test).
    let bytes = write_one_file(&path, &mailbox(1, 5 << 20));
    let _scarce = Scarce::new();
    let e = open(&path).err().unwrap();
    assert!(e.contains("too large"), "{e}");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    // A string that never ends is stopped at the cap, not read until the memory is gone.
    let mut endless = br#"{"version":1,"accounts":[],"pad":""#.to_vec();
    endless.resize(20 << 20, b'x');
    fs::write(&path, &endless).unwrap();
    let e = open(&path).err().unwrap();
    assert!(e.contains("too large"), "{e}");
}

#[test]
fn too_many_items_are_refused() {
    let tmp = Temp::new("many");
    let path = tmp.file();
    let mut json = String::from(r#"{"version":1,"messages":{"#);
    for i in 0..(stream::MAX_ITEMS + 10) {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!("\"{}\":{{}}", i + 1));
    }
    json.push_str("}}");
    fs::write(&path, &json).unwrap();
    let _scarce = Scarce::new();
    let e = open(&path).err().unwrap();
    assert!(e.contains("more than"), "{e}");
}

#[test]
fn deep_nesting_and_the_wrong_types_are_errors() {
    let tmp = Temp::new("hostile");
    let path = tmp.file();
    let _scarce = Scarce::new();
    let deep = format!("{{\"version\":1,\"x\":{}{}}}", "[".repeat(100_000), "]".repeat(100_000));
    let cases = [
        deep.as_str(),
        r#"{"version":1,"messages":[1,2,3]}"#,
        r#"{"version":1,"messages":{"1":5}}"#,
        r#"{"version":1,"messages":{"x":{}}}"#,
        r#"{"version":1,"messages":null}"#,
        r#"{"version":"one"}"#,
        r#"{"version":1,"accounts":5}"#,
        r#"{"version":1,"messages":{"1":{"attachments":[{"data":"!!!not base64"}]}}}"#,
        r#"{"version":1,"messages":{"1":{"body":{"text":5}}}}"#,
        "[1,2]",
        "{",
        "{}{}",
        r#"{"version":1} trailing"#,
    ];
    for case in cases {
        fs::write(&path, case).unwrap();
        let e = open(&path);
        assert!(e.is_err(), "{}", &case[..case.len().min(60)]);
        assert_eq!(fs::read(&path).unwrap(), case.as_bytes());
    }
    // A newer format is said to be.
    fs::write(&path, r#"{"version":99,"messages":{}}"#).unwrap();
    assert!(open(&path).err().unwrap().contains("newer MailCraft"));
}

#[test]
fn a_sparse_message_in_a_big_file_keeps_what_it_has() {
    let tmp = Temp::new("sparse");
    let path = tmp.file();
    let (mb, id) = (mailbox(1, 10), 0);
    let inbox = mb.inbox().unwrap();
    let mut json: Value = serde_json::to_value(&mb).unwrap();
    json["messages"] = serde_json::json!({"7": {"folder": inbox, "subject": "Short", "body": {"text": "hi"}}});
    json["unknown"] = serde_json::json!({"a": [1]});
    let _ = id;
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let _scarce = Scarce::new();
    let opened = open(&path).unwrap();
    let m = opened.mailbox.messages.get(&7).unwrap();
    assert_eq!((m.subject.as_str(), m.body.text.as_str(), m.kept.is_none()), ("Short", "hi", true));
    assert_eq!(opened.deferred, 0);
}
