// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::time::{Duration, Instant, SystemTime};

use mailcraft_model::{Address, Attachment, Mailbox, Message};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::Session;

/// A folder of its own under the system's temporary one, removed when the test is over.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    /// Where the mailbox file of this test goes.
    fn file(&self) -> PathBuf {
        self.0.join("mailbox.mailcraft")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Bytes that are the same for the same seed and look like nothing in particular.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        out.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

fn long_text(seed: usize) -> String {
    (0..90).map(|i| format!("Line {i} of message {seed}: the figures for the quarter are attached.\n")).collect()
}

/// A mailbox with every kind of content: short and long text, HTML, large and tiny attachments,
/// and one file two messages share.
fn mailbox(messages: usize) -> Mailbox {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    let shared = Attachment::new("shared.pdf", "application/pdf", noise(7, 40_000));
    for i in 0..messages {
        let mut m = Message {
            folder: inbox,
            subject: format!("Message {i}"),
            from: Address::new("Ada Aldrin", "ada@example.org"),
            to: vec![Address::new("Avery Quinn", "avery@example.org")],
            date: 1_700_000_000 + i as i64,
            ..Default::default()
        };
        m.body.text = if i % 3 == 0 { format!("Short note {i}.") } else { long_text(i) };
        if i % 2 == 0 {
            m.body.html = Some(format!("<html><body><p>{}</p></body></html>", long_text(i + 10_000).replace('\n', "<br>")));
        }
        if i % 4 == 0 {
            m.attachments.push(Attachment::new(format!("file-{i}.bin"), "application/octet-stream", noise(i as u64 + 100, 5_000 + i * 37)));
        }
        if i % 8 == 0 {
            m.attachments.push(Attachment::new("tiny.txt", "text/plain", b"tiny".to_vec()));
        }
        if i == 1 || i == 5 {
            m.attachments.push(shared.clone());
        }
        mb.add_message(m).unwrap();
    }
    mb
}

/// Every file under `dir`, with its size.
fn files(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                _ => out.push((e.path(), e.metadata().map_or(0, |m| m.len()))),
            }
        }
    }
    out.sort();
    out
}

fn blob_file(path: &Path, bytes: &[u8]) -> PathBuf {
    blob_path(&blob_dir(path), &Digest::of(bytes)).unwrap().1
}

/// Everything of `opened` into memory, as the mailbox was before it was saved.
fn load(path: &Path, opened: &mut Opened) {
    load_all(path, &mut opened.mailbox).unwrap();
}

/// The mailbox at `path` with every part in memory: what was saved there.
fn whole(path: &Path) -> Mailbox {
    let mut opened = open(path).unwrap();
    load(path, &mut opened);
    opened.mailbox
}

/// `mb` with every part it keeps on disk at `path` brought into memory.
fn loaded(path: &Path, mb: &Mailbox) -> Mailbox {
    let mut mb = mb.clone();
    load_all(path, &mut mb).unwrap();
    mb
}

fn first_with(mb: &Mailbox, test: impl Fn(&Message) -> bool) -> Id {
    mb.messages.values().find(|m| test(m)).map(|m| m.id).unwrap()
}

/// Set a file's time to `ago` in the past.
fn age(file: &Path, ago: Duration) {
    fs::OpenOptions::new().write(true).open(file).unwrap().set_modified(SystemTime::now() - ago).unwrap();
}

#[test]
fn names_are_sha256_and_nothing_else() {
    // FIPS 180-2's first test vector.
    let abc = Digest::of(b"abc");
    assert_eq!(abc.hex(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(Digest::parse(&abc.hex()), Some(abc));
    for bad in [
        "",
        "ba7816bf",
        "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015a",
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015add",
        "../../../../../../../../../../../../../../../../../../../../etc/x",
        "..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\..\\x",
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015a/",
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015a\u{0}",
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f2001é",
    ] {
        assert_eq!(Digest::parse(bad), None, "{bad:?}");
    }
    assert_eq!(blob_dir(Path::new("some/where/mailbox.mailcraft")), Path::new("some/where/mailbox.mailcraft.blobs"));
    assert!(is_temp_of("mailbox.mailcraft.412-1f3a-7.tmp", "mailbox.mailcraft"));
    for other in [
        "mailbox.mailcraft",
        "mailbox.mailcraft.tmp",
        "mailbox.mailcraft.412-zz-7.tmp",
        "mailbox.mailcraft.blobs",
        "ui.json",
        "mailbox.mailcraft.1-a-2-3.tmp",
    ] {
        assert!(!is_temp_of(other, "mailbox.mailcraft"), "{other}");
    }
}

#[test]
fn the_split_form_round_trips() {
    let tmp = Temp::new("round-trip");
    let path = tmp.file();
    let mut mb = mailbox(24);
    let saved = save(&path, &mb).unwrap();
    assert!(saved.bytes > 0 && saved.blobs > 0 && !saved.unchanged);

    // The mailbox file is format 3 and holds no content; each content file is named by the
    // SHA-256 of what is in it, in the folder of its first two digits.
    let head = fs::read(&path).unwrap();
    assert_eq!((&head[..16], &head[16..24]), (&b"MailCraft index\n"[..], &[3u8, 0, 0, 0, 0, 0, 0, 0][..]));
    assert!(saved.compacted && saved.bytes == head.len() as u64);
    let content: u64 = mb.messages.values().map(|m| m.size()).sum();
    assert!(saved.bytes < content / 4, "{} of {content}", saved.bytes);
    let blobs = files(&blob_dir(&path));
    assert_eq!(blobs.len(), saved.blobs);
    for (file, len) in &blobs {
        let name = file.file_name().unwrap().to_str().unwrap();
        let bytes = fs::read(file).unwrap();
        assert_eq!((Digest::of(&bytes).hex().as_str(), bytes.len() as u64), (name, *len));
        assert_eq!(file.parent().unwrap().file_name().unwrap().to_str().unwrap(), &name[..2]);
    }
    // What is small stays inside: 8 short texts, and the tiny attachment.
    let text = String::from_utf8_lossy(&head);
    assert!(text.contains("Short note 3.") && text.contains("\"data\":\"dGlueQ==\"") && !text.contains("Line 5 of message 1:"));
    // 16 long texts, 12 HTML bodies, 6 files and the shared one (once).
    assert_eq!(blobs.len(), 16 + 12 + 6 + 1);
    assert!(blob_file(&path, &noise(7, 40_000)).exists());
    // Mail is its owner's alone, where the system has a way of saying so.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |file: &Path| fs::metadata(file).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&path), mode(&blobs[0].0), mode(&blob_dir(&path)), mode(blobs[0].0.parent().unwrap())), (0o600, 0o600, 0o700, 0o700));
    }

    let mut opened = open(&path).unwrap();
    assert!(opened.split && opened.unavailable == 0);
    // Every attachment stays on disk until it is wanted, with its size known; the bodies, which
    // fit the budget, are in memory.
    assert_eq!(opened.deferred, 6 + 2);
    assert!(opened.mailbox.messages.values().all(|m| !m.body_is_kept() && m.attachments.iter().all(|a| a.is_loaded() == (a.size() <= INLINE_MAX))));
    assert_eq!(opened.mailbox.messages.values().map(|m| m.size()).sum::<u64>(), mb.messages.values().map(|m| m.size()).sum::<u64>());
    load(&path, &mut opened);
    assert!(opened.mailbox == mb);
    // The file two messages carry is in memory once.
    let shared: Vec<_> = opened.mailbox.messages.values().flat_map(|m| &m.attachments).filter(|a| a.name == "shared.pdf").collect();
    assert_eq!(shared.len(), 2);
    assert!(Arc::ptr_eq(&shared[0].data, &shared[1].data));

    // Changes of every kind arrive: a flag, a body, the bytes of an attachment, a message gone.
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    mb.message_mut(ids[0]).unwrap().read = true;
    mb.message_mut(ids[1]).unwrap().body.text = long_text(77);
    Arc::make_mut(&mut mb.message_mut(ids[4]).unwrap().attachments[0].data)[0] ^= 0xff;
    mb.messages.remove(&ids[2]);
    let again = save(&path, &mb).unwrap();
    assert_eq!(again.blobs, 2, "{again:?}");
    assert!(whole(&path) == mb);
    // Saving the same state again writes nothing at all.
    assert_eq!(save(&path, &mb).unwrap(), Saved { unchanged: true, ..Saved::default() });
}

/// An account a newer MailCraft added goes into the mailbox file as its file had it, in the first
/// save and in every one after it, and a mailbox opened and saved as it is isn't written at all.
#[test]
fn an_account_a_newer_version_added_is_saved_as_its_file_had_it() {
    let tmp = Temp::new("newer-account");
    let path = tmp.file();
    let mut mb = mailbox(3);
    let newer = mb.add_account(mailcraft_model::Account { email: "ada@jmap.example".into(), ..Default::default() });
    let mut file = serde_json::to_value(&mb).unwrap();
    let written = json!({
        "id": newer,
        "name": "ada@jmap.example",
        "email": "ada@jmap.example",
        "kind": "fromANewerVersion",
        "settings": {"session": "https://jmap.example/.well-known/jmap", "auth": "bearer"},
        "comesLater": [1, 2.5, {"deep": null}],
    });
    *file["accounts"].as_array_mut().unwrap().iter_mut().find(|a| a["id"] == newer).unwrap() = written.clone();
    let mb: Mailbox = serde_json::from_value(file).unwrap();
    let account = |mb: &Mailbox| serde_json::to_value(mb).unwrap()["accounts"].as_array().unwrap().iter().find(|a| a["id"] == newer).cloned();
    save(&path, &mb).unwrap();
    assert!(String::from_utf8_lossy(&fs::read(&path).unwrap()).contains(r#""comesLater":[1,2.5,{"deep":null}]"#));
    let back = whole(&path);
    assert!(back == mb);
    assert_eq!(account(&back), Some(written.clone()));
    let opened = open(&path).unwrap();
    assert!(save(&path, &opened.mailbox).unwrap().unchanged);
    let mut changed = whole(&path);
    let first = *changed.messages.keys().next().unwrap();
    changed.message_mut(first).unwrap().read = true;
    assert!(!save(&path, &changed).unwrap().unchanged);
    assert!(whole(&path) == changed);
    assert_eq!(account(&whole(&path)), Some(written));
}

/// The mailbox the files at `path` hold, read from a copy of them: what is known about `path`
/// itself (which is what the next save goes by) stays as it is.
fn peek(path: &Path) -> Mailbox {
    let aside = path.with_file_name("peek");
    let _ = fs::remove_dir_all(&aside);
    fs::create_dir_all(&aside).unwrap();
    let copy = aside.join("mailbox.mailcraft");
    fs::copy(path, &copy).unwrap();
    for (file, _) in files(&blob_dir(path)) {
        let to = blob_dir(&copy).join(file.strip_prefix(blob_dir(path)).unwrap());
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(&file, &to).unwrap();
    }
    let mut opened = open(&copy).unwrap();
    assert_eq!(opened.unavailable, 0);
    load(&copy, &mut opened);
    opened.mailbox
}

#[test]
fn what_was_worked_out_for_a_message_is_never_used_for_another_state_of_it() {
    // A save remembers what each message came to, by where the message is in memory. Nothing
    // else holds these messages (no undo history, no snapshot), so every change below could be
    // made in place, in the very memory the last save looked at; each must still be seen.
    let tmp = Temp::new("identity");
    let path = tmp.file();
    let mut mb = mailbox(16);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    save(&path, &mb).unwrap();
    assert!(peek(&path) == mb);
    let changes: [&dyn Fn(&mut Mailbox); 7] = [
        &|mb| mb.message_mut(ids[0]).unwrap().read = true,
        &|mb| mb.message_mut(ids[1]).unwrap().body.text.push_str("One more line.\n"),
        &|mb| mb.message_mut(ids[2]).unwrap().body.html = None,
        // The bytes of an attachment, and of one two messages share (only one of them changes).
        &|mb| Arc::make_mut(&mut mb.message_mut(ids[4]).unwrap().attachments[0].data)[7] ^= 0xff,
        &|mb| Arc::make_mut(&mut mb.message_mut(ids[5]).unwrap().attachments[0].data).truncate(30_000),
        // Another message under the same number.
        &|mb| {
            let other = Message { id: ids[8], subject: "Somebody else".into(), ..mb.message(ids[9]).unwrap().clone() };
            *mb.message_mut(ids[8]).unwrap() = other;
        },
        // A message gone and one with its number back.
        &|mb| {
            let mut back = mb.messages.remove(&ids[12]).unwrap();
            Arc::make_mut(&mut back).body.text = long_text(4242);
            mb.messages.insert(ids[12], back);
        },
    ];
    for (n, change) in changes.iter().enumerate() {
        let before = serde_json::to_vec(&mb).unwrap();
        change(&mut mb);
        assert!(serde_json::to_vec(&mb).unwrap() != before, "change {n} changed nothing");
        save(&path, &mb).unwrap();
        assert!(peek(&path) == mb, "change {n}");
    }
    // The file two messages shared: one of them has the shorter one now, the other the whole.
    let shared = |mb: &Mailbox, id: Id| mb.message(id).unwrap().attachments.iter().find(|a| a.name == "shared.pdf").unwrap().data.len();
    assert_eq!((shared(&mb, ids[1]), shared(&mb, ids[5])), (40_000, 30_000));
}

#[test]
fn an_old_single_file_opens_and_becomes_the_split_form_on_the_next_save() {
    let tmp = Temp::new("upgrade");
    let path = tmp.file();
    let mut mb = mailbox(12);
    fs::write(&path, crate::io::write_mailbox(&mb).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();

    assert!(crate::io::open_path(&path).unwrap() == mb);
    let opened = open(&path).unwrap();
    assert!(!opened.split && opened.unavailable == 0 && opened.mailbox == mb);
    // A mailbox nobody changed since it was opened is not rewritten, in whichever form it is
    // (a session's snapshots of it are the same mailbox).
    assert!(save(&path, &opened.mailbox).unwrap().unchanged && save(&path, &opened.mailbox.clone()).unwrap().unchanged);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(!blob_dir(&path).exists());

    // The first change takes it to the new form.
    let id = *mb.messages.keys().next().unwrap();
    mb.message_mut(id).unwrap().read = true;
    let saved = save(&path, &mb).unwrap();
    assert!(saved.blobs > 0 && (saved.bytes as usize) < before.len() / 4);
    let mut reopened = open(&path).unwrap();
    load(&path, &mut reopened);
    assert!(reopened.split && reopened.mailbox == mb);
    // The file as it was is kept beside it, whole.
    assert_eq!(fs::read(tmp.0.join("mailbox.mailcraft.format1")).unwrap(), before);
    // The file alone is no longer the whole mailbox, and says so instead of opening empty.
    let alone = crate::io::read_mailbox(&fs::read(&path).unwrap()).unwrap_err();
    assert!(alone.contains("folder beside it"), "{alone}");
    assert_eq!(crate::io::Format::detect("whatever", &fs::read(&path).unwrap()), Some(crate::io::Format::Mailbox));
    // A format from the future is refused in both.
    fs::write(&path, br#"{"version":3}"#).unwrap();
    assert!(open(&path).unwrap_err().contains("newer MailCraft"));
    assert!(crate::io::read_mailbox(br#"{"version":3}"#).unwrap_err().contains("newer MailCraft"));
}

#[test]
fn one_self_contained_file_still_works_everywhere() {
    // What the browser keeps and what Save Mailbox As writes: one file, attachments inside.
    let mb = mailbox(12);
    let bytes = crate::io::write_mailbox(&mb).unwrap();
    assert!(crate::io::read_mailbox(&bytes).unwrap() == mb);
    let mut s = Session::new(mb.clone());
    // Without a place for files `file.save` answers the bytes; they open again by themselves.
    let saved = s.run("file.save", &json!({})).unwrap();
    assert_eq!((saved["name"].as_str(), saved.get("path")), (Some("mailbox.mailcraft"), None));
    assert!(s.dirty || s.path.is_none());
    let mut other = Session::empty();
    other.run("file.open", &json!({"name": "mailbox.mailcraft", "data": saved["data"]})).unwrap();
    assert!(other.mailbox == mb && other.path.is_none());

    // `file.saveAs` writes that one file; a mailbox with no place yet lives there from then on.
    let tmp = Temp::new("single");
    let copy = tmp.0.join("copy.mailcraft");
    let r = s.run("file.saveAs", &json!({"path": copy})).unwrap();
    assert_eq!((r["copy"].as_bool(), s.path.as_deref(), s.dirty), (Some(false), Some(copy.as_path()), false));
    assert!(crate::io::read_mailbox(&fs::read(&copy).unwrap()).unwrap() == mb);
    assert!(!blob_dir(&copy).exists());
    assert_eq!(files(&tmp.0).len(), 1, "no temporary file stays behind");

    // `file.save` keeps the mailbox in the form with a content folder, here and from now on.
    let home = tmp.file();
    let first = *s.mailbox.messages.keys().next().unwrap();
    s.run("mail.markRead", &json!({"id": first})).unwrap();
    let r = s.run("file.save", &json!({"path": home})).unwrap();
    assert!(r["blobs"].as_u64().unwrap() > 0 && s.path.as_deref() == Some(home.as_path()) && !s.dirty);
    assert!(blob_dir(&home).is_dir());
    // From a mailbox that has its place, Save As is a copy: saving goes on at home.
    let export = tmp.0.join("export.mailcraft");
    let r = s.run("file.saveAs", &json!({"path": export})).unwrap();
    assert_eq!((r["copy"].as_bool(), s.path.as_deref()), (Some(true), Some(home.as_path())));
    assert!(crate::io::read_mailbox(&fs::read(&export).unwrap()).unwrap() == s.mailbox);
    // Both open by path, and say what they hold.
    for file in [&home, &export] {
        let mut t = Session::empty();
        let r = t.run("file.open", &json!({"path": file})).unwrap();
        assert_eq!((r["messages"].as_u64(), r["unavailable"].as_u64()), (Some(12), Some(0)));
        assert!(t.loaded_mailbox().unwrap() == s.mailbox && t.path.as_deref() == Some(file.as_path()));
    }
    assert!(s.run("file.open", &json!({"path": tmp.0.join("nothing.mailcraft")})).is_err());
    assert!(s.run("file.open", &json!({"path": tmp.0.join("message.eml")})).unwrap_err().to_string().contains("File › Import"));
}

#[test]
fn a_save_after_a_read_mark_writes_the_mailbox_file_and_nothing_else() {
    // The point of the whole arrangement: what a save costs is what changed. 400 messages with
    // 48 MB of attachments; were everything written each time, the second save would write
    // some 65 MB.
    let tmp = Temp::new("cost");
    let path = tmp.file();
    let mut mb = mailbox(400);
    let inbox = mb.inbox().unwrap();
    for i in 0..24 {
        let file = Attachment::new(format!("scan-{i}.pdf"), "application/pdf", noise(i + 5_000, 2_000_000));
        mb.add_message(Message { folder: inbox, subject: format!("Scan {i}"), attachments: vec![file], ..Default::default() }).unwrap();
    }
    let content: u64 = mb.messages.values().map(|m| m.size()).sum();
    let t = Instant::now();
    let first = save(&path, &mb).unwrap();
    let first_took = t.elapsed();
    assert!(first.blob_bytes > 48_000_000 && first.blob_bytes <= content);
    let before = files(&blob_dir(&path));
    let stamps = |list: &[(PathBuf, u64)]| list.iter().map(|(f, _)| fs::metadata(f).unwrap().modified().unwrap()).collect::<Vec<_>>();
    let written = stamps(&before);

    // Three messages read one after another, saved each time (the quickest counts: a busy
    // machine can hold any one save up).
    let mut quickest = Duration::MAX;
    for _ in 0..3 {
        let id = first_with(&mb, |m| !m.read && !m.attachments.is_empty());
        mb.message_mut(id).unwrap().read = true;
        let t = Instant::now();
        let again = save(&path, &mb).unwrap();
        quickest = quickest.min(t.elapsed());
        // No content file is written, and the mailbox file is a sliver of the content.
        assert_eq!((again.blobs, again.blob_bytes), (0, 0));
        assert!(again.bytes > 0 && again.bytes < 1_000_000 && again.bytes < content / 50, "{again:?} of {content}");
    }
    // Nor is one touched.
    let after = files(&blob_dir(&path));
    assert_eq!((&after, stamps(&after)), (&before, written));
    // Generous: it is hundreds of times quicker, and a second is far more than it ever takes.
    assert!(quickest < Duration::from_secs(1) && quickest < first_took, "{quickest:?} against {first_took:?}");
    assert!(whole(&path) == mb);

    // New mail costs its own content: one more message, one more content file.
    let file = Attachment::new("new.pdf", "application/pdf", noise(9_999, 300_000));
    mb.add_message(Message { folder: inbox, subject: "New".into(), attachments: vec![file], ..Default::default() }).unwrap();
    let third = save(&path, &mb).unwrap();
    assert_eq!((third.blobs, third.blob_bytes), (1, 300_000));
    // Moving every message to another folder changes them all, and still writes no content.
    let archive = mb.folders.iter().find(|f| f.kind == mailcraft_model::FolderKind::Archive).unwrap().id;
    for id in mb.messages.keys().copied().collect::<Vec<_>>() {
        mb.move_message(id, archive).unwrap();
    }
    assert_eq!(save(&path, &mb).unwrap().blobs, 0);
    assert!(whole(&path) == mb);
}

#[test]
fn a_crash_leaves_the_old_state_or_the_new() {
    let tmp = Temp::new("crash");
    let path = tmp.file();
    let old = mailbox(12);
    save(&path, &old).unwrap();
    let old_file = fs::read(&path).unwrap();

    // Stopped while writing the new mailbox file: the half of it is in a temporary file beside
    // the old one, which nobody reads.
    let half = tmp.0.join(format!("mailbox.mailcraft.{}-1f-0.tmp", std::process::id()));
    fs::write(&half, &old_file[..old_file.len() / 2]).unwrap();
    let stray = blob_dir(&path).join("ab").join(format!("{}.77-2e-1.tmp", "ab".repeat(32)));
    fs::create_dir_all(stray.parent().unwrap()).unwrap();
    fs::write(&stray, b"half a content file").unwrap();
    let mut opened = open(&path).unwrap();
    load(&path, &mut opened);
    assert!(opened.mailbox == old && opened.unavailable == 0);

    // Stopped after the new content was written and before the mailbox file was renamed over:
    // the old file is there, and everything it names still is.
    let mut new = old.clone();
    let inbox = new.inbox().unwrap();
    let extra = noise(31, 70_000);
    new.add_message(Message {
        folder: inbox,
        attachments: vec![Attachment::new("new.bin", "application/octet-stream", extra.clone())],
        ..Default::default()
    })
    .unwrap();
    save(&path, &new).unwrap();
    assert!(whole(&path) == new);
    fs::write(&path, &old_file).unwrap();
    let mut opened = open(&path).unwrap();
    load(&path, &mut opened);
    assert!(opened.mailbox == old && opened.unavailable == 0);
    assert!(blob_file(&path, &extra).exists());

    // What the crash left goes with a sweep once it is old; nothing that is named does.
    assert_eq!(sweep(&path, GRACE).unwrap().removed, 0);
    for file in [&half, &stray, &blob_file(&path, &extra)] {
        age(file, Duration::from_secs(3600));
    }
    let swept = sweep(&path, GRACE).unwrap();
    assert_eq!(swept.removed, 3, "{swept:?}");
    assert!(!half.exists() && !stray.exists() && !blob_file(&path, &extra).exists());
    assert!(whole(&path) == old);

    // A mailbox file cut short (written in place by something else) is refused, not opened as
    // an empty mailbox for the next save to write over.
    // (Its first save is all of it: half of that is no save at all.)
    fs::write(&path, &old_file[..old_file.len() / 2]).unwrap();
    assert!(open(&path).unwrap_err().contains("no save in it was finished"));
    assert!(sweep(&path, Duration::ZERO).is_err());
    assert_eq!(files(&blob_dir(&path)).len(), swept.kept);
    // The same of one self-contained file.
    let single = crate::io::write_mailbox(&old).unwrap();
    fs::write(&path, &single[..single.len() / 2]).unwrap();
    assert!(open(&path).unwrap_err().contains("isn't a MailCraft mailbox"));
    fs::write(&path, &old_file).unwrap();
    assert!(whole(&path) == old);
}

#[test]
fn a_missing_content_folder_never_stops_the_mailbox_from_opening() {
    let tmp = Temp::new("missing");
    let path = tmp.file();
    let mb = mailbox(12);
    save(&path, &mb).unwrap();
    let away = tmp.0.join("elsewhere");
    fs::rename(blob_dir(&path), &away).unwrap();

    let opened = open(&path).unwrap();
    assert_eq!((opened.unavailable, opened.deferred, opened.loaded), (8 + 6 + 3 + 2, 0, 0));
    assert_eq!(opened.mailbox.messages.len(), 12);
    for (id, m) in &mb.messages {
        let got = opened.mailbox.message(*id).unwrap();
        // Everything that isn't content is there; text that was outside says it is unavailable,
        // an attachment keeps its name, type and size with nothing in it.
        assert_eq!((&got.subject, &got.from, got.date, got.attachments.len()), (&m.subject, &m.from, m.date, m.attachments.len()));
        assert_eq!(got.body.text, if m.body.text.len() > INLINE_MAX { UNAVAILABLE } else { m.body.text.as_str() });
        assert_eq!(got.body.html, None);
        assert_eq!(
            got.kept.as_ref().is_some_and(|k| k.failed),
            m.body.text.len() > INLINE_MAX || m.body.html.as_ref().is_some_and(|h| h.len() > INLINE_MAX)
        );
        for (a, b) in got.attachments.iter().zip(&m.attachments) {
            assert_eq!((&a.name, &a.mime, a.size()), (&b.name, &b.mime, b.size()));
            assert_eq!(a.data.as_slice(), if b.data.len() > INLINE_MAX { &[][..] } else { b.data.as_slice() });
        }
    }
    // Asked for, a part that isn't there says so, and the message isn't asked again.
    let mut s = Session::new(opened.mailbox.clone());
    s.path = Some(path.clone());
    let with_file = first_with(&s.mailbox, |m| m.attachments.iter().any(|a| !a.is_loaded()));
    let index = s.mailbox.message(with_file).unwrap().attachments.iter().position(|a| !a.is_loaded()).unwrap();
    assert!(s.content(with_file, crate::Part::Attachment(index)).unwrap_err().to_string().contains("missing"));
    let lost = first_with(&s.mailbox, |m| m.body.text == UNAVAILABLE);
    assert!(!s.needs_load_for_display(lost) && s.load_for_display(lost) == Ok(false));

    // Working on (a message read, a save) keeps what the file said about the content, so the
    // folder turning up again brings everything back.
    let mut edited = opened.mailbox.clone();
    let id = first_with(&edited, |m| m.body.text == UNAVAILABLE);
    edited.message_mut(id).unwrap().read = true;
    // A message written anew meanwhile is new content: the HTML that went with its old text
    // isn't put beside the new one. Its file, which nobody touched, is still its file.
    let rewritten = first_with(&edited, |m| m.subject == "Message 0");
    edited.message_mut(rewritten).unwrap().body.text = "Written anew.".into();
    // A message whose other fields change (its Message-ID, say) is still that message: what it
    // carries about its content goes with it.
    let renamed = first_with(&edited, |m| m.subject == "Message 2");
    edited.message_mut(renamed).unwrap().message_id = "somebody-else@mail.example.org".into();
    let saved = save(&path, &edited).unwrap();
    assert!(saved.bytes > 0 && saved.blobs == 0 && !blob_dir(&path).exists());
    fs::rename(&away, blob_dir(&path)).unwrap();
    let mut whole_again = mb.clone();
    whole_again.message_mut(id).unwrap().read = true;
    whole_again.message_mut(rewritten).unwrap().body = mailcraft_model::Body { text: "Written anew.".into(), html: None };
    whole_again.message_mut(renamed).unwrap().message_id = "somebody-else@mail.example.org".into();
    let mut back = open(&path).unwrap();
    assert_eq!(back.unavailable, 0);
    load(&path, &mut back);
    assert!(back.mailbox == whole_again);
    assert!(!back.mailbox.message(rewritten).unwrap().attachments[0].data.is_empty());
}

/// Make `link` point at `target`, if this system and this user can. A junction stands in for a
/// directory link on Windows, where links proper need a privilege.
fn link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link).is_ok();
    #[cfg(windows)]
    return if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
            || std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().is_ok_and(|o| o.status.success())
    } else {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    };
    #[cfg(not(any(unix, windows)))]
    return false;
}

#[test]
fn a_mailbox_kept_elsewhere_through_a_link_stays_there() {
    let tmp = Temp::new("linked");
    let real = tmp.0.join("elsewhere").join("real.mailcraft");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    let mut mb = mailbox(8);
    save(&real, &mb).unwrap();
    let path = tmp.file();
    if !link(&real, &path) {
        eprintln!("links to files can't be made here");
        return;
    }
    // Opened and saved through the link, the mailbox file and its content are where the link
    // leads, and the link is still a link.
    assert!(whole(&path) == mb);
    let inbox = mb.inbox().unwrap();
    let extra = noise(61, 20_000);
    mb.add_message(Message {
        folder: inbox,
        attachments: vec![Attachment::new("x.bin", "application/octet-stream", extra.clone())],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(save(&path, &mb).unwrap().blobs, 1);
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink() && !blob_dir(&path).exists());
    assert!(blob_file(&real, &extra).is_file());
    assert!(whole(&real) == mb && whole(&path) == mb);
    assert_eq!(sweep(&path, Duration::ZERO).unwrap().removed, 0);
    // One self-contained file written through the link lands there too.
    save_single(&path, &mb).unwrap();
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    assert!(crate::io::read_mailbox(&fs::read(&real).unwrap()).unwrap() == mb);
}

#[test]
fn a_hostile_content_folder_costs_only_the_content_it_spoiled() {
    let tmp = Temp::new("hostile");
    let path = tmp.file();
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    // One message per attack, each with a file of its own; the last is left alone.
    let content: Vec<Vec<u8>> = (0..8).map(|i| noise(i + 40, 9_000 + i as usize)).collect();
    let mut ids = Vec::new();
    for (i, bytes) in content.iter().enumerate() {
        let file = Attachment::new(format!("{i}.bin"), "application/octet-stream", bytes.clone());
        ids.push(mb.add_message(Message { folder: inbox, subject: format!("{i}"), attachments: vec![file], ..Default::default() }).unwrap());
    }
    save(&path, &mb).unwrap();
    let at = |i: usize| blob_file(&path, &content[i]);
    let outside = tmp.0.join("outside");
    fs::create_dir_all(&outside).unwrap();

    // 0: other content under the name, of the same length. 1: of another length.
    fs::write(at(0), noise(999, content[0].len())).unwrap();
    fs::write(at(1), b"short").unwrap();
    // 2: a huge file (never read: its length is wrong before a byte is looked at).
    fs::OpenOptions::new().write(true).open(at(2)).unwrap().set_len(200 << 20).unwrap();
    // 3: a folder where the file should be.
    fs::remove_file(at(3)).unwrap();
    fs::create_dir(at(3)).unwrap();
    // 4: a link to a file elsewhere, even one with the right content.
    fs::write(outside.join("copy"), &content[4]).unwrap();
    fs::remove_file(at(4)).unwrap();
    let linked_file = link(&outside.join("copy"), &at(4));
    // 5: its folder of two digits is a link to a folder elsewhere that has the file.
    let sub = at(5).parent().unwrap().to_path_buf();
    let elsewhere = outside.join(sub.file_name().unwrap());
    fs::rename(&sub, &elsewhere).unwrap();
    let linked_dir = link(&elsewhere, &sub);
    // 6: gone.
    fs::remove_file(at(6)).unwrap();
    // And things that aren't ours at all.
    fs::write(blob_dir(&path).join("README.txt"), b"hello").unwrap();
    fs::create_dir_all(blob_dir(&path).join("zz").join("deeper")).unwrap();
    fs::write(blob_dir(&path).join("ab").join("not-a-name"), b"x").unwrap_or(());

    let t = Instant::now();
    let opened = open(&path).unwrap();
    assert!(t.elapsed() < Duration::from_secs(20));
    // Six are seen to be missing by a look at the file; the one with other content under the
    // right name and length is found out when it is read.
    assert_eq!((opened.unavailable, opened.deferred), (6, 2));
    for (i, id) in ids.iter().enumerate() {
        let a = &opened.mailbox.message(*id).unwrap().attachments[0];
        assert_eq!((a.name.as_str(), a.data.as_slice(), a.size()), (format!("{i}.bin").as_str(), &[][..], content[i].len()));
        let fetched = fetch(&path, &a.kept.unwrap());
        assert_eq!(fetched.ok().as_deref().map(Vec::as_slice), if i == 7 { Some(content[7].as_slice()) } else { None }, "attachment {i}");
    }
    if !linked_file || !linked_dir {
        eprintln!("links can't be made here: file {linked_file}, folder {linked_dir} (those two read as missing instead)");
    }

    // Saving new content never goes through the link: the folder becomes one of ours again, and
    // nothing new appears where the link pointed.
    let elsewhere_before = files(&outside);
    let mut next = opened.mailbox.clone();
    let digits = sub.file_name().unwrap().to_str().unwrap().to_string();
    let fresh = (0u64..).map(|n| noise(n + 70_000, 5_000)).find(|b| Digest::of(b).hex().starts_with(&digits)).unwrap();
    next.add_message(Message {
        folder: inbox,
        attachments: vec![Attachment::new("fresh.bin", "application/octet-stream", fresh.clone())],
        ..Default::default()
    })
    .unwrap();
    save(&path, &next).unwrap();
    assert_eq!(files(&outside), elsewhere_before);
    assert!(fs::symlink_metadata(&sub).unwrap().file_type().is_dir() && blob_file(&path, &fresh).is_file());
    // What was spoiled stays named (it may come back), the rest round-trips, and a sweep takes
    // nothing that is named or isn't ours.
    let again = open(&path).unwrap();
    assert_eq!(again.unavailable, 6);
    let newest = &again.mailbox.messages.values().last().unwrap().attachments[0];
    assert_eq!(fetch(&path, &newest.kept.unwrap()).unwrap().as_slice(), fresh.as_slice());
    sweep(&path, Duration::ZERO).unwrap();
    assert!(blob_dir(&path).join("README.txt").exists() && blob_dir(&path).join("zz").join("deeper").is_dir() && at(7).is_file());
    assert_eq!(fs::read(outside.join("copy")).unwrap(), content[4]);
}

#[test]
fn a_hostile_mailbox_file_names_nothing_outside_its_folder() {
    let tmp = Temp::new("names");
    let path = tmp.file();
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    let secret = noise(1, 4_000);
    fs::write(tmp.0.join("secret"), &secret).unwrap();
    let file = Attachment::new("a.bin", "application/octet-stream", noise(2, 4_000));
    let id = mb
        .add_message(Message {
            folder: inbox,
            attachments: vec![file.clone(), file],
            body: mailcraft_model::Body { text: long_text(1), html: None },
            ..Default::default()
        })
        .unwrap();
    // A mailbox file of format 2, as the program before this one wrote it.
    save_v2(&path, &mb);
    let good: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let real = good["blobs"][id.to_string()]["text"][0].as_str().unwrap().to_string();
    let abs = tmp.0.join("secret").to_string_lossy().to_string();

    // Names that are paths, lengths that are lies, entries for messages that aren't there, more
    // attachments than the message has: each costs that content and nothing else.
    let attacks = [
        json!({"text": ["../secret", 4000], "attachments": [["..\\secret", 4000], [abs, 4000]]}),
        json!({"text": [real.to_uppercase(), 6000], "attachments": [[format!("{real}/../../secret"), 4000], ["", 0]]}),
        json!({"text": [real, 18446744073709551615u64], "attachments": [[real, 0], [real, 1u64 << 40]]}),
        json!({"text": [real, 6000], "html": [real, 6000], "attachments": vec![json!([real, 6000]); 5000]}),
        json!({"attachments": vec![Value::Null; 100_000]}),
    ];
    for (n, attack) in attacks.iter().enumerate() {
        let mut bad = good.clone();
        bad["blobs"][id.to_string()] = attack.clone();
        bad["blobs"]["4242"] = json!({"text": ["../secret", 4000]});
        bad["blobs"]["18446744073709551615"] = attack.clone();
        fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        let opened = open(&path).unwrap_or_else(|e| panic!("attack {n}: {e}"));
        let m = opened.mailbox.message(id).unwrap();
        assert_eq!(m.attachments.len(), 2, "attack {n}");
        for part in [m.body.text.as_bytes(), m.body.html.as_deref().unwrap_or("").as_bytes(), &m.attachments[0].data, &m.attachments[1].data] {
            assert!(part != secret.as_slice() && part.len() <= 6000, "attack {n}");
        }
        // Saving what was opened is safe too, and writes nothing outside the folder but the
        // file it takes the place of, kept.
        let mut changed = opened.mailbox.clone();
        changed.message_mut(id).unwrap().read ^= true;
        save(&path, &changed).unwrap();
        let kept = tmp.0.join("mailbox.mailcraft.format2");
        assert_eq!(serde_json::from_slice::<Value>(&fs::read(&kept).unwrap()).unwrap(), bad, "attack {n}");
        fs::remove_file(&kept).unwrap();
        assert_eq!(files(&tmp.0).iter().filter(|(f, _)| !f.starts_with(blob_dir(&path))).count(), 2, "attack {n}");
        assert!(open(&path).unwrap().mailbox == changed, "attack {n}");
    }
    // `blobs` that isn't what it should be is a damaged file: refused, never guessed at.
    for junk in [json!(5), json!([1, 2]), json!({"1": 5}), json!({"1": {"text": "x"}}), json!({"1": {"attachments": {"a": 1}}}), json!({"x": {}})] {
        let mut bad = good.clone();
        bad["blobs"] = junk.clone();
        fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(open(&path).unwrap_err().contains("damaged"), "{junk}");
        assert!(sweep(&path, Duration::ZERO).is_err(), "{junk}");
    }
    assert_eq!(fs::read(tmp.0.join("secret")).unwrap(), secret);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever a mailbox file and its content folder hold, opening, saving and sweeping end in
    /// a mailbox or an error.
    #[test]
    fn arbitrary_files_never_panic(file in proptest::collection::vec(any::<u8>(), 0..400), blobs in "[\\[\\]{}\",:0-9a-f ntxehml.]{0,120}", content in proptest::collection::vec(any::<u8>(), 0..64)) {
        let tmp = Temp::new("arbitrary");
        let path = tmp.file();
        let dir = blob_dir(&path);
        fs::create_dir_all(dir.join("ab")).unwrap();
        fs::write(dir.join("ab").join(Digest::of(&content).hex()), &content).unwrap();
        for bytes in [file.clone(), format!("{{\"version\":2,\"messages\":{{\"1\":{{\"attachments\":[{{}}]}}}},\"blobs\":{blobs}}}").into_bytes()] {
            fs::write(&path, &bytes).unwrap();
            if let Ok(opened) = open(&path) {
                prop_assert!(opened.mailbox.clone().repair().is_empty());
                let _ = save(&path, &opened.mailbox);
            }
            let _ = sweep(&path, Duration::ZERO);
            let _ = crate::io::read_mailbox(&bytes);
        }
    }
}

#[test]
fn a_sweep_takes_only_what_nothing_names_and_only_when_it_is_old() {
    let tmp = Temp::new("sweep");
    let path = tmp.file();
    let mut mb = mailbox(12);
    let inbox = mb.inbox().unwrap();
    let (kept, dropped) = (noise(11, 30_000), noise(12, 30_000));
    let keeper = Attachment::new("kept.bin", "application/octet-stream", kept.clone());
    mb.add_message(Message { folder: inbox, attachments: vec![keeper], ..Default::default() }).unwrap();
    let goner = Attachment::new("dropped.bin", "application/octet-stream", dropped.clone());
    let going = mb.add_message(Message { folder: inbox, attachments: vec![goner], ..Default::default() }).unwrap();
    save(&path, &mb).unwrap();
    let all = files(&blob_dir(&path)).len();
    // Nothing is unnamed yet, however old it is.
    for (file, _) in files(&blob_dir(&path)) {
        age(&file, Duration::from_secs(86_400));
    }
    assert_eq!(sweep(&path, GRACE).unwrap(), Swept { removed: 0, bytes: 0, kept: all });

    // The message goes; its content is unnamed from the next save on.
    let with_it = mb.clone();
    mb.messages.remove(&going);
    save(&path, &mb).unwrap();
    assert!(blob_file(&path, &dropped).exists());
    // Written a moment ago (a save in flight looks just like this): left alone.
    age(&blob_file(&path, &dropped), Duration::from_secs(30));
    assert_eq!(sweep(&path, GRACE).unwrap().removed, 0);
    // A clock that was wrong: a time in the future is no age.
    fs::OpenOptions::new()
        .write(true)
        .open(blob_file(&path, &dropped))
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(86_400))
        .unwrap();
    assert_eq!(sweep(&path, Duration::ZERO).unwrap().removed, 0);
    // Old enough: it goes, and only it.
    age(&blob_file(&path, &dropped), Duration::from_secs(3600));
    assert_eq!(sweep(&path, GRACE).unwrap(), Swept { removed: 1, bytes: 30_000, kept: all - 1 });
    assert!(!blob_file(&path, &dropped).exists() && blob_file(&path, &kept).exists());
    assert!(whole(&path) == mb);

    // The message comes back (an undo): its content is written again from memory, though this
    // program had written it once already.
    let saved = save(&path, &with_it).unwrap();
    assert_eq!((saved.blobs, saved.blob_bytes), (1, 30_000));
    assert!(whole(&path) == with_it);
    // Content that is there but unnamed and old gets its time renewed when a save names it
    // again, so a sweep that began before the save can't take it from under the new file.
    save(&path, &mb).unwrap();
    age(&blob_file(&path, &dropped), Duration::from_secs(86_400));
    assert_eq!(save(&path, &with_it).unwrap().blobs, 0);
    assert_eq!(sweep(&path, GRACE).unwrap().removed, 0);
    save(&path, &mb).unwrap();
    assert_eq!(sweep(&path, GRACE).unwrap().removed, 0, "renewed a moment ago");

    // One self-contained file beside a content folder (a copy saved over the mailbox): what the
    // folder holds isn't this file's to judge.
    save_single(&path, &mb).unwrap();
    for (file, _) in files(&blob_dir(&path)) {
        age(&file, Duration::from_secs(86_400));
    }
    assert_eq!(sweep(&path, Duration::ZERO).unwrap(), Swept::default());
    assert_eq!(files(&blob_dir(&path)).len(), all);
    // No mailbox file, no sweep.
    fs::remove_file(&path).unwrap();
    assert!(sweep(&path, Duration::ZERO).is_err());
    assert_eq!(files(&blob_dir(&path)).len(), all);
}

#[test]
fn of_two_saves_racing_the_newer_state_wins() {
    let tmp = Temp::new("race");
    let path = tmp.file();
    let old = mailbox(8);
    let mut new = old.clone();
    let inbox = new.inbox().unwrap();
    new.add_message(Message {
        folder: inbox,
        attachments: vec![Attachment::new("n.bin", "application/octet-stream", noise(3, 50_000))],
        ..Default::default()
    })
    .unwrap();
    // Handed over in this order, arriving at the disk in the other.
    let (first, second) = (next_seq(), next_seq());
    assert!(save_as_of(&path, &new, second).unwrap().bytes > 0);
    assert!(save_as_of(&path, &old, first).unwrap().unchanged);
    assert!(whole(&path) == new);

    // Two threads saving two states over and over, a third reading what is on disk the way
    // another program would (no lock): the mailbox file always parses and everything it names
    // is there, whole.
    let stop = AtomicBool::new(false);
    let seen = std::thread::scope(|scope| {
        let writers: Vec<_> = [&old, &new]
            .into_iter()
            .map(|state| {
                scope.spawn(|| {
                    for _ in 0..40 {
                        save(&path, state).unwrap();
                    }
                })
            })
            .collect();
        let reader = scope.spawn(|| {
            let mut seen = 0;
            let t = Instant::now();
            // At least one look, however late the reader gets a turn on a busy machine (#75: the
            // writers could be done before it had looked at all), and never a wait without end.
            while seen == 0 || !stop.load(Ordering::Relaxed) {
                assert!(t.elapsed() < Duration::from_secs(60), "the mailbox file couldn't be read");
                // (Where a file being replaced can't be opened that instant, look again.)
                let Ok(bytes) = fs::read(&path) else { continue };
                // Read as far as its last commit (an append may be under way).
                let replayed = index::replay(&bytes[..], bytes.len() as u64).unwrap();
                assert!(replayed.rest.is_some());
                for item in replayed.items.values().filter(|item| !item.refs().is_empty()) {
                    let wire: WireParts = serde_json::from_slice(item.refs()).unwrap();
                    for r in wire.text.iter().chain(&wire.html).chain(wire.attachments.iter().flatten()).filter_map(parse_ref) {
                        assert!(get(&blob_dir(&path), &r, &Folders::new()).is_some(), "{r:?}");
                    }
                }
                seen += 1;
            }
            seen
        });
        for w in writers {
            w.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap()
    });
    assert!(seen > 0);
    let mut last = open(&path).unwrap();
    load(&path, &mut last);
    assert!(last.unavailable == 0 && (last.mailbox == old || last.mailbox == new));
    // No temporary file is left anywhere.
    assert!(files(&tmp.0).iter().all(|(f, _)| f.extension().is_none_or(|e| e != "tmp")));
}

/// Wait until `test` holds (the saver's thread is at work), or give up.
#[track_caller]
fn until(test: impl Fn() -> bool) {
    let t = Instant::now();
    while !test() {
        assert!(t.elapsed() < Duration::from_secs(30), "the saver never got there");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_saver_writes_in_the_background_coalesces_and_flushes() {
    let tmp = Temp::new("saver");
    let path = tmp.file();
    let woken = Arc::new(AtomicUsize::new(0));
    let count = woken.clone();
    let saver = Saver::spawn(move || {
        count.fetch_add(1, Ordering::Relaxed);
    })
    .unwrap();
    assert!(!followed(&path));
    let mut mb = mailbox(8);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();

    // Whoever hands a state in never waits for the disk: with the file held (as a slow disk
    // would hold it), handing in five states takes no time, the first is being written, and of
    // the four behind it only the newest is kept.
    let held = shared(&path);
    let gate = lock(&held.state);
    let t = Instant::now();
    saver.submit(&path, &mb, 1);
    until(|| lock(&saver.queue.jobs).writing.is_some());
    let mut states = vec![mb.clone()];
    for rev in 2..=5 {
        mb.message_mut(ids[rev as usize]).unwrap().read = true;
        saver.submit(&path, &mb, rev);
        states.push(mb.clone());
    }
    // The same state again (the window asks until it hears back) changes nothing.
    saver.submit(&path, &mb, 5);
    assert!(t.elapsed() < Duration::from_secs(5));
    assert!(followed(&path) && saver.reports().is_empty() && !path.exists());
    {
        let jobs = lock(&saver.queue.jobs);
        assert_eq!((jobs.writing.clone(), jobs.waiting.len(), jobs.waiting[0].rev), (Some((path.clone(), 1)), 1, 5));
    }
    drop(gate);

    // Flushing waits for both; two states were written, the first and the newest.
    saver.flush();
    let reports = saver.reports();
    assert_eq!(reports, [1, 5].map(|rev| SaveReport { path: path.clone(), rev, result: Ok(()) }));
    assert!(whole(&path) == mb);
    assert!(saver.reports().is_empty());

    // The window is woken once for each of the two, and once more when the sweep the first save
    // queued is over (it runs as soon as there is nothing to write, and holds the file's state
    // while it runs). Counted once that sweep is over, so the count doesn't depend on how soon
    // this thread gets to look (#138: waiting for two failed whenever the sweep was quicker).
    until(|| woken.load(Ordering::Relaxed) == 3);
    assert!(lock(&held.state).swept.is_some());

    // A state that gets to the disk after a newer one was written by somebody else (the Save
    // command, quitting) is dropped rather than written over it.
    let mut gate = lock(&held.state);
    saver.submit(&path, &states[0], 6);
    until(|| lock(&saver.queue.jobs).writing.is_some());
    mb.message_mut(ids[0]).unwrap().read = true;
    write_index(&mut gate, &path, &mb, &held.progress).unwrap();
    gate.newest = next_seq();
    drop(gate);
    saver.flush();
    assert_eq!(saver.reports(), [SaveReport { path: path.clone(), rev: 6, result: Ok(()) }]);
    assert!(whole(&path) == mb);
    // Whichever of a save in the background and one in place comes first, the newer state stays.
    for rev in 7..12 {
        saver.submit(&path, &states[(rev % 5) as usize], rev);
        mb.message_mut(ids[6]).unwrap().read ^= true;
        save(&path, &mb).unwrap();
        saver.flush();
        assert!(whole(&path) == mb);
    }

    // A sweep asked for runs when there is nothing to write, and the thread ends with the saver.
    saver.sweep_later(&path);
    until(|| lock(&saver.queue.jobs).sweeps.is_empty());
    let queue = saver.queue.clone();
    drop(saver);
    until(|| Arc::strong_count(&queue) == 1);
}

#[test]
fn a_large_first_save_runs_in_the_background_says_how_far_it_has_got_and_is_whole() {
    let tmp = Temp::new("progress");
    let path = tmp.file();
    let mb = mailbox(240);
    // A slow disk: every step of saving this file takes a few milliseconds more.
    shared(&path).progress.pause_ms.store(5, Ordering::Relaxed);
    let saver = Saver::spawn(|| {}).unwrap();
    assert_eq!(saver.progress(), None);

    // Handing the mailbox in takes no time however long the writing takes, and handing the same
    // state in again (the window asks every few seconds until it hears back) copies nothing.
    let t = Instant::now();
    saver.submit(&path, &mb, 1);
    for _ in 0..50 {
        saver.submit(&path, &mb, 1);
    }
    assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
    assert!(lock(&saver.queue.jobs).waiting.len() <= 1);

    // Meanwhile it says how far it has got: through the messages, then the content files.
    let mut seen = Vec::new();
    let t = Instant::now();
    while let Some(p) = saver.progress() {
        assert!(t.elapsed() < Duration::from_secs(60), "the save never ended");
        assert!(p.done <= p.total);
        seen.push(p);
        std::thread::sleep(Duration::from_millis(5));
    }
    let midway = |stage: SaveStage| seen.iter().any(|p| p.stage == stage && p.done > 0 && p.done < p.total);
    assert!(midway(SaveStage::Messages) && midway(SaveStage::Content), "{seen:?}");
    assert!(seen.iter().filter(|p| p.stage == SaveStage::Messages).all(|p| p.total == 240));
    // The time the save has been at it only grows. A sweep that follows it is timed from its own
    // start (review of R6.3: counting it in failed this under load).
    let saving: Vec<&SaveProgress> = seen.iter().filter(|p| p.stage != SaveStage::Sweeping).collect();
    assert!(saving.windows(2).all(|w| w[1].millis >= w[0].millis || w[1].stage == SaveStage::Waiting), "{seen:?}");
    assert!(seen.iter().skip_while(|p| p.stage != SaveStage::Sweeping).all(|p| p.stage == SaveStage::Sweeping), "{seen:?}");
    // And the file is whole.
    assert_eq!(saver.reports(), [SaveReport { path: path.clone(), rev: 1, result: Ok(()) }]);
    assert!(whole(&path) == mb);
}

#[test]
fn a_sweep_keeps_the_saver_busy_until_it_is_done() {
    // Review of R6.3: a sweep holds the file from start to end, as a save does. Had the saver
    // said nothing was going on meanwhile, the window would have run a File › Open that waited
    // for it into the sweep, given up after two seconds, and the Open would have been lost.
    let tmp = Temp::new("sweep-busy");
    let path = tmp.file();
    save(&path, &mailbox(4)).unwrap();
    let saver = Saver::spawn(|| {}).unwrap();
    assert_eq!(saver.progress(), None);

    // A sweep of a large content folder, held here at its start: busy, and says what it is at.
    let held = shared(&path);
    let gate = lock(&held.state);
    saver.sweep_later(&path);
    let p = saver.progress().unwrap();
    assert_eq!((p.stage, p.done, p.total), (SaveStage::Sweeping, 0, 0));
    let t = Instant::now();
    while lock(&saver.queue.jobs).sweeping.is_none() {
        assert!(t.elapsed() < Duration::from_secs(10), "the sweep never began");
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(saver.progress().map(|p| p.stage), Some(SaveStage::Sweeping));
    assert!(saver.progress().unwrap().millis >= 20);

    // Done: free again, and the file can be written straight away.
    drop(gate);
    let t = Instant::now();
    while saver.progress().is_some() {
        assert!(t.elapsed() < Duration::from_secs(10), "the sweep never ended");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(save_within(&path, &mailbox(5), Duration::ZERO).is_ok());
}

#[test]
fn handing_in_a_state_copies_the_mailbox_without_the_saver_waiting() {
    // Review of R6.3: the copy of what is handed in is made without the queue's lock, and a
    // state already on its way is neither copied nor queued twice.
    let tmp = Temp::new("submit-lock");
    let path = tmp.file();
    let saver = Saver::spawn(|| {}).unwrap();
    // The file is held here, so the first state taken waits to be written and the queue can be
    // looked at meanwhile.
    let held = shared(&path);
    let gate = lock(&held.state);
    let (one, two) = (mailbox(3), mailbox(6));
    saver.submit(&path, &one, 1);
    let t = Instant::now();
    while lock(&saver.queue.jobs).writing.is_none() {
        assert!(t.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(2));
    }
    // The state being written isn't queued again; a newer one waits its turn, and a newer one
    // still takes its place, once.
    saver.submit(&path, &one, 1);
    assert!(lock(&saver.queue.jobs).waiting.is_empty(), "the state being written was queued again");
    saver.submit(&path, &one, 2);
    saver.submit(&path, &two, 3);
    saver.submit(&path, &two, 3);
    {
        let jobs = lock(&saver.queue.jobs);
        assert_eq!(jobs.waiting.iter().map(|j| j.rev).collect::<Vec<_>>(), [3]);
        assert!(jobs.waiting.first().is_some_and(|j| j.mailbox == two));
    }
    drop(gate);
    saver.flush();
    let revs: Vec<u64> = saver.reports().iter().map(|r| r.rev).collect();
    assert_eq!(revs, [1, 3]);
    assert!(whole(&path) == two);
}

#[test]
fn replacing_the_mailbox_does_not_wait_out_a_save_in_the_background() {
    let tmp = Temp::new("busy");
    let path = tmp.file();
    save(&path, &mailbox(4)).unwrap();
    let mut s = Session::new(open(&path).unwrap().mailbox);
    s.path = Some(path.clone());
    let id = first_with(&s.mailbox, |m| !m.read);
    s.run("mail.markRead", &json!({"id": id})).unwrap();
    let changed = s.mailbox.clone();
    let saver = Saver::spawn(|| {}).unwrap();
    saver.follow(&path);

    // The file is busy for a long while (held here, as a long save or a sweep of a large content
    // folder holds it): File › New gives up after a moment, and the mailbox stays open as it is.
    // (The window doesn't even ask while its saver is at it: it runs the command afterwards.)
    let held = shared(&path);
    let gate = lock(&held.state);
    let t = Instant::now();
    let error = s.run("file.new", &json!({})).unwrap_err().to_string();
    assert!(t.elapsed() < Duration::from_secs(10));
    assert!(error.contains("still being saved"), "{error}");
    assert!(s.mailbox == changed && s.path.as_deref() == Some(path.as_path()));
    drop(gate);
    // Once it is done, the same goes through, and writes the last changes first.
    s.run("file.new", &json!({})).unwrap();
    assert!(open(&path).unwrap().mailbox == changed);
}

#[test]
fn a_save_that_fails_is_reported_leaves_the_file_and_works_when_tried_again() {
    let tmp = Temp::new("failure");
    let path = tmp.file();
    let old = mailbox(8);
    save(&path, &old).unwrap();
    let on_disk = fs::read(&path).unwrap();
    // The new state needs a content file that can't be written: something is in its place.
    let extra = noise(21, 20_000);
    let mut new = old.clone();
    let inbox = new.inbox().unwrap();
    new.add_message(Message {
        folder: inbox,
        attachments: vec![Attachment::new("x.bin", "application/octet-stream", extra.clone())],
        ..Default::default()
    })
    .unwrap();
    let blocked = blob_file(&path, &extra);
    fs::create_dir_all(blocked.join("in the way")).unwrap();

    let error = save(&path, &new).unwrap_err();
    assert!(error.contains("content file"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), on_disk);
    assert!(whole(&path) == old);
    // Through the saver: the window hears why, and the file is as it was.
    let saver = Saver::spawn(|| {}).unwrap();
    saver.submit(&path, &new, 1);
    saver.flush();
    let reports = saver.reports();
    assert!(matches!(reports.as_slice(), [SaveReport { rev: 1, result: Err(e), .. }] if e.contains("content file")), "{reports:?}");
    assert_eq!(fs::read(&path).unwrap(), on_disk);
    // Handed in again once the trouble is over (the window does that), it goes through.
    fs::remove_dir_all(&blocked).unwrap();
    saver.submit(&path, &new, 1);
    saver.flush();
    assert_eq!(saver.reports(), [SaveReport { path: path.clone(), rev: 1, result: Ok(()) }]);
    assert!(whole(&path) == new);
    assert!(files(&tmp.0).iter().all(|(f, _)| f.extension().is_none_or(|e| e != "tmp")));

    // Nowhere to write at all: an error, and nothing appears.
    let nowhere = tmp.0.join("no such folder").join("mailbox.mailcraft");
    assert!(save(&nowhere, &new).is_err() && save_single(&nowhere, &new).is_err());
    assert!(!nowhere.parent().unwrap().exists());
}

#[test]
fn replacing_a_mailbox_a_host_keeps_saved_writes_its_last_changes_first() {
    let tmp = Temp::new("replace");
    let (home, other) = (tmp.file(), tmp.0.join("other.mailcraft"));
    save(&home, &mailbox(8)).unwrap();
    save_single(&other, &mailbox(4)).unwrap();
    let change = |s: &mut Session| {
        let id = first_with(&s.mailbox, |m| !m.read);
        s.run("mail.markRead", &json!({"id": id})).unwrap();
        s.mailbox.clone()
    };

    // A program that saves when it is told to (the CLI, a script): opening another mailbox
    // leaves the first as it was on disk, as before.
    let mut s = Session::new(open(&home).unwrap().mailbox);
    s.path = Some(home.clone());
    let changed = change(&mut s);
    s.run("file.open", &json!({"path": other})).unwrap();
    assert!(open(&home).unwrap().mailbox != changed);

    // A host that keeps the file up to date by itself: what it hadn't written yet is written
    // before the mailbox is replaced, by File › Open and by File › New.
    let saver = Saver::spawn(|| {}).unwrap();
    saver.follow(&home);
    for replace in [("file.open", json!({"path": other})), ("file.new", json!({})), ("file.open", json!({"path": home}))] {
        let mut s = Session::new(open(&home).unwrap().mailbox);
        s.path = Some(home.clone());
        let changed = change(&mut s);
        s.run(replace.0, &replace.1).unwrap();
        assert!(open(&home).unwrap().mailbox == changed, "{}", replace.0);
        // Opening the file the mailbox is kept in shows what was just written, not what was
        // there before.
        if replace.1["path"] == json!(home) {
            assert!(s.mailbox == changed);
        }
    }
    // When that can't be done the mailbox stays open, changes and all.
    let mut s = Session::new(open(&home).unwrap().mailbox);
    s.path = Some(home.clone());
    let inbox = s.mailbox.inbox().unwrap();
    let extra = noise(55, 20_000);
    s.mailbox
        .add_message(Message {
            folder: inbox,
            attachments: vec![Attachment::new("x.bin", "application/octet-stream", extra.clone())],
            ..Default::default()
        })
        .unwrap();
    s.mailbox_changed();
    let kept = s.mailbox.clone();
    fs::create_dir_all(blob_file(&home, &extra).join("in the way")).unwrap();
    let error = s.run("file.new", &json!({})).unwrap_err().to_string();
    assert!(error.contains("couldn't be saved first"), "{error}");
    assert!(s.mailbox == kept && s.path.as_deref() == Some(home.as_path()));
}

/// The mailbox file of format 2 and its content files, as the program before format 3 wrote them:
/// the mailbox as JSON, each message's content outside named under `blobs`.
fn save_v2(path: &Path, mb: &Mailbox) {
    let dir = blob_dir(path);
    let mut hashes = HashMap::new();
    let (mut messages, mut blobs) = (serde_json::Map::new(), serde_json::Map::new());
    for (id, m) in &mb.messages {
        let p = prepare(*id, m, &mut hashes).unwrap();
        for (r, bytes) in p.parts.content(m) {
            let _ = make_dir(&dir);
            put(&dir, &r, bytes).unwrap();
        }
        messages.insert(id.to_string(), serde_json::from_slice(&p.json).unwrap());
        if !p.refs.is_empty() {
            blobs.insert(id.to_string(), serde_json::from_slice(&p.refs).unwrap());
        }
    }
    let mut file = serde_json::to_value(mb).unwrap();
    file["version"] = json!(2);
    file["messages"] = Value::Object(messages);
    file["blobs"] = Value::Object(blobs);
    fs::write(path, serde_json::to_vec(&file).unwrap()).unwrap();
}

// ---- content that stays on disk ----

/// The messages of `mb` newest first, as the budget is given out.
fn newest_first(mb: &Mailbox) -> Vec<Id> {
    let mut ids: Vec<(i64, Id)> = mb.messages.values().map(|m| (m.date, m.id)).collect();
    ids.sort_unstable_by(|a, b| b.cmp(a));
    ids.into_iter().map(|(_, id)| id).collect()
}

/// What the bodies of message `m` take outside the mailbox file.
fn body_outside(m: &Message) -> u64 {
    let part = |len: usize| if len > INLINE_MAX { len as u64 } else { 0 };
    part(m.body.text.len()) + m.body.html.as_ref().map_or(0, |h| part(h.len()))
}

/// A budget that holds the bodies of the `n` newest messages of `mb` and no more.
fn budget_for_newest(mb: &Mailbox, n: usize) -> u64 {
    newest_first(mb).iter().take(n).map(|id| body_outside(mb.message(*id).unwrap())).sum()
}

/// Parts of `mb` kept outside the mailbox file: bodies and attachments over [`INLINE_MAX`].
fn parts_outside(mb: &Mailbox) -> usize {
    mb.messages
        .values()
        .map(|m| {
            usize::from(m.body.text.len() > INLINE_MAX)
                + usize::from(m.body.html.as_ref().is_some_and(|h| h.len() > INLINE_MAX))
                + m.attachments.iter().filter(|a| a.data.len() > INLINE_MAX).count()
        })
        .sum()
}

/// A session on the mailbox at `path`, opened with a budget for the bodies of its `newest` messages.
fn session_on(path: &Path, mb: &Mailbox, newest: usize) -> (Session, Opened) {
    let opened = with_budget(budget_for_newest(mb, newest), || open(path).unwrap());
    let mut s = Session::new(opened.mailbox.clone());
    s.path = Some(path.to_path_buf());
    (s, opened)
}

#[test]
fn content_beyond_the_budget_stays_on_disk_and_comes_when_wanted() {
    let tmp = Temp::new("budget");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    let before = files(&blob_dir(&path));
    clear_cache();

    let (mut s, opened) = session_on(&path, &mb, 6);
    let order = newest_first(&mb);
    // Every message is there with everything but its content; the six newest have their bodies,
    // the older ones have their bodies named on them; every attachment waits on disk.
    assert_eq!((opened.unavailable, opened.loaded + opened.deferred), (0, parts_outside(&mb)));
    let body_parts =
        |m: &Message| usize::from(m.body.text.len() > INLINE_MAX) + usize::from(m.body.html.as_ref().is_some_and(|h| h.len() > INLINE_MAX));
    assert_eq!(opened.loaded, order.iter().take(6).map(|id| body_parts(mb.message(*id).unwrap())).sum::<usize>());
    for (rank, id) in order.iter().enumerate() {
        let (was, now) = (mb.message(*id).unwrap(), s.mailbox.message(*id).unwrap());
        assert_eq!((&now.subject, &now.from, now.date, now.attachments.len()), (&was.subject, &was.from, was.date, was.attachments.len()));
        assert_eq!(now.size(), was.size(), "message {id}");
        if rank < 6 || body_outside(was) == 0 {
            assert!(now.body == was.body && !now.body_is_kept(), "message {id} (rank {rank}) has its body");
        } else {
            // (A short text is in the mailbox file itself; its HTML waits.)
            let text = if was.body.text.len() > INLINE_MAX { "" } else { was.body.text.as_str() };
            assert!(now.body_is_kept() && now.body.text == text && now.body.html.is_none(), "message {id} (rank {rank}) waits");
            assert_eq!(now.kept.as_ref().unwrap().len(), body_outside(was));
        }
        for (a, b) in now.attachments.iter().zip(&was.attachments) {
            assert_eq!((a.is_loaded(), a.size()), (b.data.len() <= INLINE_MAX, b.data.len()));
        }
    }

    // Asked for, a part comes as it was, whether a body or a file, and the same file once.
    let oldest = *order.last().unwrap();
    let was = mb.message(oldest).unwrap();
    assert_eq!(s.content(oldest, crate::Part::Text).unwrap().as_slice(), was.body.text.as_bytes());
    assert!(s.needs_load_for_display(oldest));
    assert_eq!(s.load_for_display(oldest), Ok(true));
    assert!(s.mailbox.message(oldest).unwrap().body == was.body && !s.needs_load_for_display(oldest));
    let with_shared: Vec<Id> = mb.messages.values().filter(|m| m.attachments.iter().any(|a| a.name == "shared.pdf")).map(|m| m.id).collect();
    let fetched: Vec<Arc<Vec<u8>>> = with_shared
        .iter()
        .map(|id| {
            let i = s.mailbox.message(*id).unwrap().attachments.iter().position(|a| a.name == "shared.pdf").unwrap();
            s.content(*id, crate::Part::Attachment(i)).unwrap()
        })
        .collect();
    assert!(fetched.iter().all(|b| b.as_slice() == noise(7, 40_000).as_slice()) && Arc::ptr_eq(&fetched[0], &fetched[1]));
    assert!(cached_bytes() >= 40_000);
    // The whole of a message, in the mailbox itself.
    let with_file = first_with(&mb, |m| m.attachments.iter().any(|a| a.data.len() > INLINE_MAX) && !order[..6].contains(&m.id));
    assert_eq!(s.load_message(with_file), Ok(true));
    assert!(s.mailbox.message(with_file).unwrap() == mb.message(with_file).unwrap());
    assert_eq!(s.load_message(with_file), Ok(false));

    // A file whose bytes aren't what its name says is refused, however right its length.
    let victim = first_with(&mb, |m| m.attachments.iter().any(|a| a.name.starts_with("file-")) && m.id != with_file);
    let data = &mb.message(victim).unwrap().attachments[0].data;
    let file = blob_file(&path, data);
    let mut spoiled = data.as_ref().clone();
    spoiled[100] ^= 0x55;
    fs::write(&file, &spoiled).unwrap();
    let refused = s.content(victim, crate::Part::Attachment(0)).unwrap_err().to_string();
    assert!(refused.contains("damaged"), "{refused}");
    fs::write(&file, data.as_slice()).unwrap();

    // Saved after all that, the file names the same content as before, writes none of it again,
    // and a sweep takes nothing.
    s.mailbox.message_mut(oldest).unwrap().read = true;
    let saved = save(&path, &s.mailbox).unwrap();
    assert!(saved.blobs == 0 && saved.bytes > 0, "{saved:?}");
    assert_eq!(files(&blob_dir(&path)), before);
    assert_eq!(sweep(&path, Duration::ZERO).unwrap().removed, 0);
    let mut expect = mb.clone();
    expect.message_mut(oldest).unwrap().read = true;
    assert!(whole(&path) == expect);
    // Opened with room for everything, the bodies are all there at once.
    let roomy = with_budget(1 << 30, || open(&path).unwrap());
    assert!(roomy.mailbox.messages.values().all(|m| !m.body_is_kept()));
}

#[test]
fn what_is_missing_is_counted_and_what_is_merely_on_disk_is_not() {
    let tmp = Temp::new("missing-few");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let order = newest_first(&mb);
    // Two files and the text of an old message go missing.
    let gone_files: Vec<Id> =
        mb.messages.values().filter(|m| m.attachments.iter().any(|a| a.name.starts_with("file-"))).map(|m| m.id).take(2).collect();
    for id in &gone_files {
        fs::remove_file(blob_file(&path, &mb.message(*id).unwrap().attachments[0].data)).unwrap();
    }
    let old_text = *order.iter().rev().find(|id| mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    fs::remove_file(blob_file(&path, mb.message(old_text).unwrap().body.text.as_bytes())).unwrap();

    let (mut s, opened) = session_on(&path, &mb, 3);
    assert_eq!(opened.unavailable, 3);
    assert_eq!(opened.loaded + opened.deferred + opened.unavailable, parts_outside(&mb));
    // The text that is missing says so, and isn't asked for again; its HTML, if any, stays named.
    let m = s.mailbox.message(old_text).unwrap();
    assert_eq!(m.body.text, UNAVAILABLE);
    assert!(m.kept.as_ref().is_some_and(|k| k.failed && k.text.is_some()) && !m.body_is_kept() && !s.needs_load_for_display(old_text));
    // A missing file says so when it is wanted; one that is there comes.
    let err = s.content(gone_files[0], crate::Part::Attachment(0)).unwrap_err().to_string();
    assert!(err.contains("missing"), "{err}");
    let there = first_with(&mb, |m| m.attachments.iter().any(|a| a.name.starts_with("file-")) && !gone_files.contains(&m.id));
    assert_eq!(s.content(there, crate::Part::Attachment(0)).unwrap().as_slice(), mb.message(there).unwrap().attachments[0].data.as_slice());
    // An old body that is there and wanted comes, and says so once it is missing.
    let old_ok = *order.iter().rev().find(|id| **id != old_text && mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    fs::remove_file(blob_file(&path, mb.message(old_ok).unwrap().body.text.as_bytes())).unwrap();
    let err = s.load_for_display(old_ok).unwrap_err().to_string();
    assert!(err.contains("missing"), "{err}");
    let m = s.mailbox.message(old_ok).unwrap();
    assert!(m.body.text == UNAVAILABLE && m.kept.as_ref().is_some_and(|k| k.failed) && !s.needs_load_for_display(old_ok));
    // Saved, everything the file named is named still: content that turns up again is found.
    save(&path, &s.mailbox).unwrap();
    let again = with_budget(budget_for_newest(&mb, 3), || open(&path).unwrap());
    assert_eq!(again.unavailable, 4);
}

#[test]
fn a_search_reads_the_bodies_it_lacks_newest_first_and_says_what_it_left() {
    let tmp = Temp::new("search");
    let path = tmp.file();
    let mut mb = mailbox(24);
    let order = newest_first(&mb);
    let old = *order.iter().rev().find(|id| mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    let text = mb.message(old).unwrap().body.text.clone();
    mb.message_mut(old).unwrap().body.text = format!("{text}The word zymurgy appears here only.\n");
    save(&path, &mb).unwrap();
    clear_cache();

    let (mut s, _) = session_on(&path, &mb, 4);
    let kept_bodies = s.mailbox.messages.values().filter(|m| m.body_is_kept()).count();
    assert!(kept_bodies >= 8);
    // Headers alone need nothing read; a word in the text brings the older bodies in.
    s.view.search = "from:ada".into();
    assert!(s.rows().len() == 24 && s.unsearched == 0);
    assert_eq!(s.mailbox.messages.values().filter(|m| m.body_is_kept()).count(), kept_bodies);
    s.view.search = "zymurgy".into();
    let rows = s.rows();
    assert_eq!((rows.len(), rows[0].id, s.unsearched), (1, old, 0));
    assert!(s.mailbox.messages.values().all(|m| !m.body_is_kept()));

    // Out of time, the search goes with what it has and says how much it didn't get to.
    let (mut hurried, _) = session_on(&path, &mb, 4);
    hurried.search_time = Duration::ZERO;
    hurried.view.search = "zymurgy".into();
    assert_eq!((hurried.rows().len(), hurried.unsearched), (0, kept_bodies));
    // Given time again, it finishes.
    hurried.search_time = Duration::from_secs(30);
    hurried.view.search = "zymurgy ".into();
    assert_eq!((hurried.rows().len(), hurried.unsearched), (1, 0));
}

#[test]
fn forwarding_exporting_and_sending_carry_what_is_on_disk() {
    let tmp = Temp::new("carry");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let (mut s, _) = session_on(&path, &mb, 2);
    let order = newest_first(&mb);
    let id = *order.iter().rev().find(|id| mb.message(**id).unwrap().attachments.iter().any(|a| a.data.len() > INLINE_MAX)).unwrap();
    let was = mb.message(id).unwrap();
    assert!(s.mailbox.message(id).unwrap().body_is_kept() && !s.mailbox.message(id).unwrap().is_loaded());

    // Forwarded: the files go with it, and the quoted text is the text.
    s.run("mail.forward", &json!({"id": id})).unwrap();
    let c = s.compose.clone().unwrap();
    assert_eq!(
        c.attachments.iter().map(|a| a.data.as_slice()).collect::<Vec<_>>(),
        was.attachments.iter().map(|a| a.data.as_slice()).collect::<Vec<_>>()
    );
    // (An original in HTML is carried below the owner's text; one in plain text is in it.)
    let quoted = c.quote.as_ref().map_or_else(|| c.body.plain(), |q| q.text.clone());
    // (An original in HTML is quoted as its HTML reads, line breaks aside, never as its text part.)
    let said = was.body.html.as_deref().map_or_else(|| was.body.text.clone(), mailcraft_html::to_text);
    let squash = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(c.attachments.iter().all(Attachment::is_loaded) && squash(&quoted).contains(&squash(said.trim())), "{quoted}");
    s.run("compose.discard", &json!({})).unwrap_or_default();

    // Exported as .eml: the files are in it.
    let eml = s.run("file.exportMessage", &json!({"id": id})).unwrap();
    let parsed = mailcraft_mime::parse(&crate::cmd::base64_decode(eml["data"].as_str().unwrap()).unwrap());
    assert_eq!(
        parsed.attachments.iter().map(|a| a.data.as_slice()).collect::<Vec<_>>(),
        was.attachments.iter().map(|a| a.data.as_slice()).collect::<Vec<_>>()
    );
    assert_eq!(parsed.body.text.trim(), was.body.text.trim());
    // A folder as .mbox too.
    let mbox = s.run("file.exportFolder", &json!({"id": was.folder})).unwrap();
    let all = mailcraft_mime::parse_mbox(&crate::cmd::base64_decode(mbox["data"].as_str().unwrap()).unwrap());
    assert_eq!(all.len(), 24);
    assert!(all.iter().all(|m| m.attachments.iter().all(|a| !a.data.is_empty())));

    // What goes to a server goes whole; a message whose file can't be read stays here.
    let outbox = vec![Message::clone(s.mailbox.message(id).unwrap())];
    let loaded = s.loaded_outbox(outbox.clone());
    assert!(loaded[0].is_loaded());
    assert_eq!(loaded[0].attachments[0].data, was.attachments[0].data);
    fs::remove_file(blob_file(&path, &was.attachments[0].data)).unwrap();
    clear_cache();
    assert!(s.loaded_outbox(outbox).is_empty());
    fs::write(blob_file(&path, &was.attachments[0].data), was.attachments[0].data.as_slice()).unwrap();
    // A synchronisation's request takes the Outbox through the same door.
    let mut imap = Session::new(s.mailbox.clone());
    imap.path = s.path.clone();
    imap.mailbox.accounts.iter_mut().for_each(|a| a.kind = mailcraft_model::AccountKind::Imap);
    let outbox_folder = imap.mailbox.folder_of_kind(was.account, mailcraft_model::FolderKind::Outbox).unwrap();
    imap.mailbox.move_message(id, outbox_folder).unwrap();
    let request = crate::cmd::account::sync_request(&imap, was.account).unwrap();
    assert!(request.outbox.len() == 1 && request.outbox[0].id == id && request.outbox[0].is_loaded());

    // Saved as one file, or saved to another place, the mailbox takes everything along.
    let single = tmp.0.join("single.mailcraft");
    s.run("file.saveAs", &json!({"path": single})).unwrap();
    assert!(crate::io::read_mailbox(&fs::read(&single).unwrap()).unwrap() == mb);
    let other = tmp.0.join("elsewhere").join("mailbox.mailcraft");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    s.run("file.save", &json!({"path": other})).unwrap();
    assert!(whole(&other) == mb && files(&blob_dir(&other)).len() == files(&blob_dir(&path)).len());
    // The mailbox in memory still waits for most of its content: nothing was read for keeps.
    assert!(s.mailbox.messages.values().any(|m| !m.is_loaded()));
}

#[test]
fn what_a_command_reads_of_a_message_is_fetched_from_disk() {
    let tmp = Temp::new("commands");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    // Nothing of any body in memory (a budget of one byte).
    let mut s = Session::new(with_budget(1, || open(&path).unwrap()).mailbox);
    s.path = Some(path.clone());
    let id = first_with(&mb, |m| m.body.text.len() > INLINE_MAX && m.attachments.iter().any(|a| a.data.len() > INLINE_MAX));
    let was = mb.message(id).unwrap();
    assert!(s.mailbox.message(id).unwrap().body_is_kept());
    // `mail.get` (what an agent reads) has the text, not an empty body.
    let got = s.run("mail.get", &json!({"id": id})).unwrap();
    assert_eq!(got["body"].as_str(), Some(was.body.text.as_str()));
    // Saving an attachment writes its bytes.
    let index = was.attachments.iter().position(|a| a.data.len() > INLINE_MAX).unwrap();
    assert!(!s.mailbox.message(id).unwrap().attachments[index].is_loaded());
    let out = tmp.0.join("saved.bin");
    s.run("mail.saveAttachment", &json!({"id": id, "index": index, "path": out})).unwrap();
    assert_eq!(fs::read(&out).unwrap(), *was.attachments[index].data);
    // A meeting made from an older message carries its text.
    let older = *newest_first(&mb).iter().rev().find(|i| **i != id && mb.message(**i).unwrap().body.text.len() > INLINE_MAX).unwrap();
    assert!(s.mailbox.message(older).unwrap().body_is_kept());
    let made = s.run("calendar.fromMessage", &json!({"id": older})).unwrap();
    let event = made["id"].as_u64().or_else(|| made["event"].as_u64()).unwrap();
    assert_eq!(s.mailbox.event(event).unwrap().description, mb.message(older).unwrap().body.text);
    // A content file that has gone is said, never written as an empty file.
    fs::remove_file(blob_file(&path, &was.attachments[index].data)).unwrap();
    clear_cache();
    let mut fresh = Session::new(with_budget(1, || open(&path).unwrap()).mailbox);
    fresh.path = Some(path.clone());
    let err = fresh.run("mail.saveAttachment", &json!({"id": id, "index": index, "path": tmp.0.join("gone.bin")})).unwrap_err();
    assert!(err.to_string().contains("missing") && !tmp.0.join("gone.bin").exists(), "{err}");
}

#[test]
fn a_picture_that_can_t_be_read_is_asked_for_once_not_every_frame() {
    let tmp = Temp::new("picture");
    let path = tmp.file();
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    let picture = Attachment { content_id: "logo@x.example".into(), inline: true, ..Attachment::new("logo.png", "image/png", noise(5, 9_000)) };
    let id = mb
        .add_message(Message {
            folder: inbox,
            subject: "With a picture".into(),
            body: mailcraft_model::Body { text: "See the logo.".into(), html: Some("<p><img src=\"cid:logo@x.example\"></p>".into()) },
            attachments: vec![picture.clone()],
            ..Default::default()
        })
        .unwrap();
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = Session::new(open(&path).unwrap().mailbox);
    s.path = Some(path.clone());
    fs::remove_file(blob_file(&path, &picture.data)).unwrap();
    // Shown, the picture is wanted; it isn't there, which is said once.
    assert!(s.needs_load_for_display(id));
    assert!(s.load_for_display(id).unwrap_err().to_string().contains("missing"));
    assert!(!s.needs_load_for_display(id));
    // Opened anew (the file back where it belongs), it is asked for again and comes.
    fs::write(blob_file(&path, &picture.data), picture.data.as_slice()).unwrap();
    s.replace_mailbox(open(&path).unwrap().mailbox);
    assert!(s.needs_load_for_display(id) && s.load_for_display(id) == Ok(true));
    assert_eq!(s.mailbox.message(id).unwrap().attachments[0].data, picture.data);
}

#[test]
fn a_mailbox_isn_t_saved_where_its_content_isn_t() {
    let tmp = Temp::new("elsewhere");
    let path = tmp.file();
    let mb = mailbox(12);
    save(&path, &mb).unwrap();
    let opened = with_budget(1, || open(&path).unwrap());
    for (id, was) in &mb.messages {
        let whole_inside = body_outside(was) == 0 && was.attachments.iter().all(|a| a.size() <= INLINE_MAX);
        assert_eq!(opened.mailbox.message(*id).unwrap().is_loaded(), whole_inside, "message {id}");
    }
    assert!(opened.mailbox.messages.values().any(|m| !m.is_loaded()));
    let other = tmp.0.join("other.mailcraft");
    let err = save(&other, &opened.mailbox).unwrap_err();
    assert!(err.contains("opened from another mailbox"), "{err}");
    assert!(!other.exists() && !blob_dir(&other).exists());
    // Where it was opened it saves as it is.
    let mut edited = opened.mailbox.clone();
    let first = *edited.messages.keys().next().unwrap();
    edited.message_mut(first).unwrap().read = true;
    assert_eq!(save(&path, &edited).unwrap().blobs, 0);
}

#[test]
fn every_content_name_in_a_damaged_file_is_found_wherever_it_falls() {
    let names: Vec<Digest> = (0..4u8).map(|i| Digest::of(&[i])).collect();
    let mut bytes = noise(3, (1 << 20) - 30);
    for (i, d) in names.iter().enumerate() {
        bytes.extend_from_slice(format!("x\"{}\"{}", d.hex(), if i == 2 { "\u{0}\u{1}" } else { "" }).as_bytes());
    }
    // Not names: too short, upper case, not closed.
    bytes.extend_from_slice(format!("\"{}\" \"{}\" \"{}", &names[0].hex()[1..], names[1].hex().to_uppercase(), names[2].hex()).as_bytes());
    let mut found = HashSet::new();
    names_in(&bytes[..], &mut found).unwrap();
    assert_eq!(found, names.iter().copied().collect::<HashSet<_>>());
}

#[test]
fn what_a_session_brings_in_stays_within_its_budget_and_comes_back_when_wanted() {
    // Whatever the machine, what a mailbox brings in when it opens is modest.
    assert!(body_budget() <= MAX_CONTENT && MAX_CONTENT <= 256 << 20 && CACHE_MAX <= 128 << 20);
    let tmp = Temp::new("evict");
    let path = tmp.file();
    let mut mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let kept: Vec<Id> = newest_first(&mb).into_iter().filter(|id| s.mailbox.message(*id).unwrap().body_is_kept()).collect();
    assert!(kept.len() > 8);
    // Room for about three bodies.
    let largest = kept.iter().map(|id| body_outside(mb.message(*id).unwrap())).max().unwrap();
    s.loaded_budget = largest * 3;
    for id in &kept {
        s.sel.messages = vec![*id];
        assert_eq!(s.load_for_display(*id), Ok(true));
        assert!(s.mailbox.message(*id).unwrap().body == mb.message(*id).unwrap().body);
        assert!(s.resident_bytes() <= s.loaded_budget + largest * 2, "{} bytes kept", s.resident_bytes());
    }
    // The first ones went back to disk; the last one shown is in memory.
    let in_memory = |s: &Session| kept.iter().filter(|id| !s.mailbox.message(**id).unwrap().body_is_kept()).count();
    assert!(in_memory(&s) < kept.len() / 2, "{} of {} still in memory", in_memory(&s), kept.len());
    assert!(!s.mailbox.message(*kept.last().unwrap()).unwrap().body_is_kept());
    let first = kept[0];
    assert!(s.mailbox.message(first).unwrap().body_is_kept() && s.needs_load_for_display(first));
    // Wanted again, it comes back as it was.
    s.sel.messages = vec![first];
    assert_eq!(s.load_for_display(first), Ok(true));
    assert!(s.mailbox.message(first).unwrap().body == mb.message(first).unwrap().body);

    // A body changed since the last save is never let go of; the rest saves as it was.
    let changed = kept[1];
    s.sel.messages.clear();
    s.load_message(changed).unwrap();
    s.mailbox.message_mut(changed).unwrap().body.text.push_str("A line added here.\n");
    let added = s.mailbox.message(changed).unwrap().body.text.clone();
    for id in &kept[2..] {
        s.load_for_display(*id).unwrap();
    }
    assert_eq!(s.mailbox.message(changed).unwrap().body.text, added);
    let saved = save(&path, &s.mailbox).unwrap();
    assert_eq!(saved.blobs, 1);
    mb.message_mut(changed).unwrap().body.text = added;
    clear_cache();
    let back = whole(&path);
    for id in &kept {
        assert!(back.message(*id).unwrap() == mb.message(*id).unwrap(), "message {id}");
    }
}

/// A session on the mailbox at `path` with none of its bodies in memory.
fn session_without_bodies(path: &Path) -> Session {
    let opened = with_budget(1, || open(path).unwrap());
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.to_path_buf());
    s
}

/// A source of content that can't be read just now (a file another program holds locked, memory
/// that isn't there for a moment).
struct Unreadable;

impl crate::io::ContentSource for Unreadable {
    fn fetch(&self, _: &Path, _: &ContentRef) -> Result<Arc<Vec<u8>>, String> {
        Err("the file is in use by another program".into())
    }
}

#[test]
fn a_body_that_couldn_t_be_read_for_a_moment_is_still_there_after_a_save_and_a_sweep() {
    let tmp = Temp::new("transient");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let long = first_with(&mb, |m| m.body.text.len() > INLINE_MAX && m.body.html.is_some());
    let short = first_with(&mb, |m| m.body.text.len() <= INLINE_MAX && m.body.html.is_some());
    s.content_source = Some(Arc::new(Unreadable));
    for id in [long, short] {
        assert!(s.needs_load_for_display(id));
        assert!(s.load_for_display(id).is_err());
        let m = s.mailbox.message(id).unwrap();
        // The view says so, once; a short text kept in the mailbox file is left as it was.
        let text = if id == long { UNAVAILABLE } else { mb.message(id).unwrap().body.text.as_str() };
        assert!(m.body.text == text && !s.needs_load_for_display(id), "message {id}");
    }
    // Saved and swept: the files are still named, and still there.
    s.mailbox.message_mut(long).unwrap().read = true;
    save(&path, &s.mailbox).unwrap();
    assert_eq!(sweep(&path, Duration::ZERO).unwrap().removed, 0);
    for id in [long, short] {
        assert!(blob_file(&path, mb.message(id).unwrap().body.html.as_ref().unwrap().as_bytes()).exists(), "message {id}");
    }
    assert!(blob_file(&path, mb.message(long).unwrap().body.text.as_bytes()).exists());
    // Opened again with the file readable, every body comes back whole.
    clear_cache();
    let mut expect = mb.clone();
    expect.message_mut(long).unwrap().read = true;
    assert!(whole(&path) == expect);
}

#[test]
fn undo_after_a_sweep_brings_back_a_message_whose_content_is_still_there() {
    let tmp = Temp::new("undo-sweep");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    // A message with a long text, HTML and a file, none of it in memory, deleted for good.
    let victim =
        first_with(&mb, |m| m.body.text.len() > INLINE_MAX && m.body.html.is_some() && m.attachments.iter().any(|a| a.data.len() > INLINE_MAX));
    assert!(!s.mailbox.message(victim).unwrap().is_loaded());
    s.run("mail.deletePermanently", &json!({"ids": [victim]})).unwrap();
    save(&path, &s.mailbox).unwrap();
    // The sweep leaves what the Undo step still names.
    sweep(&path, Duration::ZERO).unwrap();
    let was = mb.message(victim).unwrap();
    let files_of = |m: &Message| {
        let mut out = vec![blob_file(&path, m.body.text.as_bytes()), blob_file(&path, m.body.html.as_ref().unwrap().as_bytes())];
        out.extend(m.attachments.iter().filter(|a| a.data.len() > INLINE_MAX).map(|a| blob_file(&path, &a.data)));
        out
    };
    assert!(files_of(was).iter().all(|f| f.exists()));
    // Undone and saved, the mailbox is whole again.
    s.run("edit.undo", &json!({})).unwrap();
    save(&path, &s.mailbox).unwrap();
    clear_cache();
    assert!(whole(&path) == mb);

    // Once nothing in memory names it, the content of a message that is gone is swept.
    s.run("mail.deletePermanently", &json!({"ids": [victim]})).unwrap();
    save(&path, &s.mailbox).unwrap();
    s.forget_history();
    sweep(&path, Duration::ZERO).unwrap();
    assert!(files_of(was).iter().all(|f| !f.exists()));

    // And a message brought back whose content went anyway (taken by hand, say) never stops the
    // mailbox from being saved: it is saved naming it, and says it is unavailable.
    let mut s = session_without_bodies(&path);
    let other = first_with(&s.mailbox, |m| m.body_is_kept() && m.body.text.is_empty());
    let text = blob_file(&path, mb.message(other).unwrap().body.text.as_bytes());
    s.run("mail.deletePermanently", &json!({"ids": [other]})).unwrap();
    save(&path, &s.mailbox).unwrap();
    fs::remove_file(&text).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    save(&path, &s.mailbox).unwrap();
    s.run("mail.markAllRead", &json!({})).ok();
    save(&path, &s.mailbox).unwrap();
    let back = with_budget(1 << 30, || open(&path).unwrap());
    assert_eq!(back.mailbox.message(other).unwrap().body.text, UNAVAILABLE);
}

/// `MAILCRAFT_BENCH_MESSAGES=50000 cargo test -p mailcraft-engine --release -- --ignored
/// fifty_thousand_messages` (2 GB under the system's temporary folder while it runs).
#[test]
#[ignore = "builds 2 GB of mail; run by hand"]
fn fifty_thousand_messages_with_two_gigabytes_of_content_open_under_a_small_budget() {
    let messages: usize = std::env::var("MAILCRAFT_BENCH_MESSAGES").ok().and_then(|n| n.parse().ok()).unwrap_or(50_000);
    let body_len = (2u64 << 30) / messages as u64;
    let tmp = Temp::new("fifty-thousand");
    let path = tmp.file();
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    let t = Instant::now();
    for i in 0..messages {
        let mut text = format!("Message {i}.\n");
        let line = format!("Line of message {i} with words that differ: {:x}\n", (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        while (text.len() as u64) < body_len {
            text.push_str(&line);
        }
        let m = Message {
            folder: inbox,
            subject: format!("Message {i}"),
            from: Address::new("Ada Aldrin", "ada@example.org"),
            to: vec![Address::new("Avery Quinn", "avery@example.org")],
            date: 1_600_000_000 + i as i64 * 60,
            body: mailcraft_model::Body { text, html: None },
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    eprintln!("built {messages} messages of {body_len} bytes in {:?}", t.elapsed());
    let t = Instant::now();
    let saved = save(&path, &mb).unwrap();
    eprintln!("saved: {saved:?} in {:?}", t.elapsed());
    drop(mb);
    let rss = || fs::read_to_string("/proc/self/statm").ok().and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok()).map(|pages| pages * 4096);
    let before = rss();
    let t = Instant::now();
    let opened = with_budget(256 << 20, || open(&path).unwrap());
    let took = t.elapsed();
    let after = rss();
    let in_memory: u64 = opened.mailbox.messages.values().map(|m| m.body.text.len() as u64).sum();
    eprintln!(
        "opened {} messages in {took:?}: {} bodies in memory ({in_memory} bytes), {} on disk, {} missing; RSS {:?} -> {:?}",
        opened.mailbox.messages.len(),
        opened.loaded,
        opened.deferred,
        opened.unavailable,
        before,
        after
    );
    assert_eq!(opened.mailbox.messages.len(), messages);
    assert!(in_memory <= 256 << 20 && opened.loaded + opened.deferred == messages && opened.unavailable == 0);
    assert!(took < Duration::from_secs(120), "{took:?}");
    if let (Some(before), Some(after)) = (before, after) {
        assert!(after.saturating_sub(before) < 1 << 30, "memory grew by {} bytes", after.saturating_sub(before));
    }
    // The newest have their bodies; the oldest wait.
    let order = newest_first(&opened.mailbox);
    assert!(!opened.mailbox.message(order[0]).unwrap().body_is_kept() && opened.mailbox.message(*order.last().unwrap()).unwrap().body_is_kept());
}

/// Format 3: the log of records.
/// The content files beside the mailbox file at `path`.
fn content_files(path: &Path) -> usize {
    fs::read_dir(blob_dir(path)).into_iter().flatten().flatten().flat_map(|sub| fs::read_dir(sub.path()).into_iter().flatten().flatten()).count()
}

#[test]
fn content_let_go_of_goes_from_disk_once_a_state_that_new_is_saved_in_the_background() {
    // #143: mail outside its window went back to its headers and preview.
    let tmp = Temp::new("forget");
    let path = tmp.file();
    let mut mb = mailbox(3);
    let saver = Saver::spawn(|| {}).unwrap();
    saver.submit(&path, &mb, 1);
    saver.flush();
    let before = content_files(&path);
    assert!(before >= 4, "{before}");
    // Message 0's own text goes; the file it shares with another message stays for that one.
    let id = *mb.messages.keys().next().unwrap();
    let names = content_names(id, &mb.messages[&id]);
    assert!(names.len() >= 2, "{names:?}");
    forget_when_saved(&path, 2, names.clone());
    // A save of an older state removes nothing.
    saver.submit(&path, &mb, 1);
    saver.flush();
    assert_eq!(content_files(&path), before);
    let m = mb.message_mut(id).unwrap();
    (m.body, m.attachments) = (mailcraft_model::Body { text: "The words of it.".into(), html: None }, Vec::new());
    saver.submit(&path, &mb, 2);
    saver.flush();
    let after = content_files(&path);
    assert!(after < before, "{after} of {before}");
    let shared_still = names.iter().filter(|r| mb.messages.values().any(|m| m.attachments.iter().any(|a| a.data.len() as u64 == r.len))).count();
    assert_eq!(before - after, names.len() - shared_still, "the shared file stays");
    assert!(whole(&path) == mb);
    assert!(lock(&shared(&path).forgetting).is_empty());
}

#[path = "store_index_tests.rs"]
mod format3;

#[path = "store_session_tests.rs"]
mod in_session;

/// The folders and files the app keeps beside the mailbox (pictures, logs, its settings) are its
/// owner's alone too: made so, and an open folder from an earlier version closed.
#[cfg(unix)]
#[test]
fn private_folders_and_files_for_the_app() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = Temp::new("private-helpers");
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let deep = temp.0.join("settings").join("logs");
    create_private_dir_all(&deep).unwrap();
    assert_eq!((mode(&temp.0.join("settings")), mode(&deep)), (0o700, 0o700));
    // Already there: no error.
    create_private_dir_all(&deep).unwrap();

    // A file left over is replaced by one of the owner's, not written through.
    let file = deep.join("index.part");
    fs::write(&file, "left over").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    create_private_replacing(&file).unwrap().write_all(b"new").unwrap();
    assert_eq!((mode(&file), fs::read(&file).unwrap()), (0o600, b"new".to_vec()));
    let elsewhere = temp.0.join("elsewhere");
    fs::write(&elsewhere, "untouched").unwrap();
    let link = deep.join("linked.part");
    symlink(&elsewhere, &link).unwrap();
    create_private_replacing(&link).unwrap();
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "untouched");

    // An open folder is closed; nothing there is no error; a link or a file is not a folder.
    let open = temp.0.join("open");
    fs::create_dir(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
    make_private(&open).unwrap();
    assert_eq!(mode(&open), 0o700);
    make_private(&temp.0.join("missing")).unwrap();
    let shared = temp.0.join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    let to_shared = temp.0.join("to-shared");
    symlink(&shared, &to_shared).unwrap();
    assert!(make_private(&to_shared).is_err());
    assert_eq!(mode(&shared), 0o755);
}

#[test]
fn a_json_file_of_empty_messages_is_refused_while_it_is_read() {
    let t = Temp::new("empty-messages");
    let mut json = String::from("{\"messages\":{");
    for i in 1..=600_000 {
        if i > 1 {
            json.push(',');
        }
        json.push_str(&format!("\"{i}\":{{}}"));
    }
    json.push_str("}}");
    fs::write(t.file(), json).unwrap();
    // (A small budget still reads 250,000 items; 600,000 are more.)
    let err = with_budget(64 << 20, || open(&t.file())).err().unwrap();
    assert!(err.contains("more items"), "{err}");
    // A reasonable file opens under the same budget.
    fs::write(t.file(), "{\"messages\":{\"1\":{},\"2\":{}}}").unwrap();
    assert!(with_budget(64 << 20, || open(&t.file())).is_ok());
}

#[path = "store_launch_tests.rs"]
mod launch;
