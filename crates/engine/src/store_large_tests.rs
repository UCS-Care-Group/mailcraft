// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox file that is saved is a mailbox file that opens (#649): the limits on what is read
//! and on what is written are the same, and a file that isn't a mailbox is not called too large.
//! (The limits are small under test: `MAX_MAILBOX_JSON` 16 MB, `MAX_INDEX` 64 MB.)

use mailcraft_model::{Address, Mailbox, Message};

use super::*;

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-large-{}-{name}", std::process::id()));
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

/// A mailbox whose messages carry `bytes` of unmodelled headers each (what an IMAP sync keeps).
fn mailbox(messages: usize, header_bytes: usize) -> Mailbox {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    for i in 0..messages {
        let m = Message {
            folder: inbox,
            subject: format!("Synthetic message {i}"),
            from: Address::new("Blake Rivera", "blake@example.org"),
            to: vec![Address::new("Avery Quinn", "avery@example.org")],
            date: 1_700_000_000 + i as i64,
            headers: vec![("X-Trace".into(), "r".repeat(header_bytes))],
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    mb
}

#[test]
fn a_one_file_mailbox_opens_and_is_written_as_a_log_on_the_next_save() {
    let tmp = Temp::new("json");
    let path = tmp.file();
    // (That the limit is above the old 512 MB one is checked below, at compile time.)
    let mb = mailbox(300, 10_000);
    let bytes = crate::io::write_mailbox(&mb).unwrap();
    assert!(bytes.len() as u64 > 2 << 20 && (bytes.len() as u64) < MAX_MAILBOX_JSON, "{}", bytes.len());
    fs::write(&path, &bytes).unwrap();
    let mut opened = open(&path).unwrap();
    assert_eq!(opened.mailbox.messages.len(), 300);
    // (The file is replaced the first time a changed mailbox is saved.)
    let first = *opened.mailbox.messages.keys().next().unwrap();
    opened.mailbox.message_mut(first).unwrap().read = true;
    save(&path, &opened.mailbox).unwrap();
    assert!(is_index(&path).unwrap(), "the next save writes the form with a log of records");
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 300);
}

#[test]
fn a_one_file_mailbox_over_the_limit_that_is_not_a_mailbox_is_refused_and_left_alone() {
    let tmp = Temp::new("over");
    let path = tmp.file();
    // (Past the limit a file is read as a stream, which says what is wrong with it.)
    let mut bytes = br#"{"version":1,"pad":""#.to_vec();
    bytes.resize(usize::try_from(MAX_MAILBOX_JSON).unwrap() + 10, b'x');
    fs::write(&path, &bytes).unwrap();
    let e = open(&path).unwrap_err();
    assert!(e.contains("isn't a MailCraft mailbox") || e.contains("too large"), "{e}");
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn a_big_file_that_is_not_a_mailbox_is_not_called_too_large() {
    let tmp = Temp::new("zeros");
    let path = tmp.file();
    // What a system leaves when it is cut off in the middle of a write: zeros.
    fs::write(&path, vec![0u8; usize::try_from(MAX_MAILBOX_JSON).unwrap() + 10]).unwrap();
    let e = open(&path).unwrap_err();
    assert!(e.contains("isn't a MailCraft mailbox"), "{e}");
    assert!(!e.contains("larger than"), "{e}");
}

#[test]
fn save_single_refuses_to_write_what_open_would_refuse() {
    let tmp = Temp::new("single");
    let path = tmp.file();
    let e = save_single(&path, &mailbox(2_000, 10_000)).unwrap_err();
    assert!(e.contains("nothing was written"), "{e}");
    assert!(!path.exists());
    // And what it does write opens.
    save_single(&path, &mailbox(20, 100)).unwrap();
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 20);
}

#[test]
fn a_single_file_export_obeys_the_streaming_readers_message_boundary() {
    let tmp = Temp::new("export-item-boundary");
    let path = tmp.file();
    let mut mb = mailbox(2, 100);
    let ids: Vec<_> = mb.messages.keys().copied().collect();
    for id in &ids {
        let message = mb.message_mut(*id).unwrap();
        let overhead = serde_json::to_vec(&message).unwrap().len();
        // The reader's value gate starts just before the colon after the message id.
        message.body.text = "x".repeat(usize::try_from(stream::MAX_ITEM).unwrap() - overhead - 1);
    }
    save_single(&path, &mb).unwrap();
    let before = fs::read(&path).unwrap();
    let opened = stream::open(&mut State::default(), &path, before.len() as u64, &OpenProgress::default()).unwrap().unwrap();
    assert_eq!(opened.mailbox.messages.len(), 2);
    for id in &ids {
        let kept = opened.mailbox.message(*id).unwrap().kept.as_ref().unwrap().text.unwrap();
        assert_eq!(fetch(&path, &kept).unwrap().as_slice(), mb.message(*id).unwrap().body.text.as_bytes());
    }
    let entries = fs::read_dir(&tmp.0).unwrap().count();
    mb.message_mut(ids[0]).unwrap().body.text.push('x');
    let error = save_single(&path, &mb).unwrap_err();
    assert!(error.contains("message") && error.contains("nothing was written"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), entries);
}

#[test]
fn the_message_export_limit_counts_base64_expansion() {
    let tmp = Temp::new("export-item-base64");
    let path = tmp.file();
    let mut mb = mailbox(1, 100);
    let id = *mb.messages.keys().next().unwrap();
    mb.message_mut(id).unwrap().attachments.push(mailcraft_model::Attachment::new(
        "synthetic.bin",
        "application/octet-stream",
        vec![0; usize::try_from(stream::MAX_ITEM * 3 / 4).unwrap()],
    ));
    let error = save_single(&path, &mb).unwrap_err();
    assert!(error.contains("message") && error.contains("nothing was written"), "{error}");
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 0);
}

#[test]
fn a_failed_single_file_export_keeps_the_destination_and_removes_its_temporary_file() {
    let tmp = Temp::new("export-failure");
    let path = tmp.file();
    let before = crate::io::write_mailbox(&mailbox(2, 100)).unwrap();
    fs::write(&path, &before).unwrap();
    let mb = mailbox(3, 100);
    let last = *mb.messages.keys().last().unwrap();
    let error =
        save_single_with(
            &path,
            &mb,
            |id| {
                if id == last { Err("synthetic missing attachment".into()) } else { Ok(mb.messages.get(&id).unwrap().clone()) }
            },
        )
        .unwrap_err();
    assert!(error.contains("synthetic missing attachment"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 1);

    // The size limit is checked while writing, not after allocating the whole JSON file.
    let error = save_single(&path, &mailbox(2_000, 10_000)).unwrap_err();
    assert!(error.contains("nothing was written"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 1);
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 2);
}

#[test]
fn the_log_form_refuses_to_grow_past_what_it_opens_and_keeps_the_file_readable() {
    let tmp = Temp::new("index");
    let path = tmp.file();
    // About 20 MB: fits.
    let mut mb = mailbox(2_000, 10_000);
    save(&path, &mb).unwrap();
    let before = fs::read(&path).unwrap();
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 2_000);
    // Past 64 MB: refused, as a whole file or as an addition, and the file is as it was.
    let inbox = mb.inbox().unwrap();
    for i in 0..10_000 {
        let m = Message { folder: inbox, subject: format!("More {i}"), headers: vec![("X-Trace".into(), "r".repeat(10_000))], ..Default::default() };
        mb.add_message(m).unwrap();
    }
    let e = save(&path, &mb).unwrap_err();
    assert!(e.contains("more than MailCraft opens"), "{e}");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 2_000);
}

/// The JSON limit in a real build is above the 512 MB one that locked #649's owner out.
const _: () = assert!((4u64 << 30) > crate::io::MAX_FILE);

#[test]
fn an_append_that_would_pass_the_limit_compacts_instead_of_refusing() {
    let tmp = Temp::new("compact");
    let path = tmp.file();
    // About 37 MB: more than half the limit, so the file can reach it by appending alone.
    let mut mb = mailbox(3_600, 10_000);
    save(&path, &mb).unwrap();
    let ids: Vec<_> = mb.messages.keys().copied().take(3_000).collect();
    for id in ids {
        mb.message_mut(id).unwrap().read = true;
    }
    let saved = save(&path, &mb).unwrap();
    assert!(saved.compacted, "written whole");
    let len = fs::metadata(&path).unwrap().len();
    assert!(len < MAX_INDEX, "{len}");
    let opened = open(&path).unwrap();
    assert_eq!(opened.mailbox.messages.len(), 3_600);
    assert_eq!(opened.mailbox.messages.values().filter(|m| m.read).count(), 3_000);
    // And the saves after it go on.
    let first = *mb.messages.keys().next().unwrap();
    mb.message_mut(first).unwrap().flag = mailcraft_model::FlagStatus::Flagged;
    save(&path, &mb).unwrap();
}

#[test]
fn a_one_file_mailbox_too_large_for_the_memory_is_read_as_a_stream() {
    assert!(json_needs_too_much_memory(3 << 30, Some(8 << 30)));
    assert!(!json_needs_too_much_memory(1 << 30, Some(8 << 30)));
    assert!(json_needs_too_much_memory(UNMEASURED_JSON_MAX + 1, None));
    assert!(!json_needs_too_much_memory(UNMEASURED_JSON_MAX, None));
    let tmp = Temp::new("memory");
    let path = tmp.file();
    fs::write(&path, crate::io::write_mailbox(&mailbox(20, 100)).unwrap()).unwrap();
    let len = fs::metadata(&path).unwrap().len();
    TEST_AVAILABLE.set(len * 2);
    let opened = open(&path);
    TEST_AVAILABLE.set(0);
    // Not refused (#649): the file is read a piece at a time, whatever the memory.
    assert_eq!(opened.unwrap().mailbox.messages.len(), 20);
}

#[test]
fn where_memory_cannot_be_measured_a_big_file_is_read_as_a_stream() {
    let tmp = Temp::new("unmeasured");
    let path = tmp.file();
    fs::write(&path, crate::io::write_mailbox(&mailbox(20, 100)).unwrap()).unwrap();
    TEST_AVAILABLE.set(u64::MAX);
    let small = open(&path).map(|o| o.mailbox.messages.len());
    fs::write(&path, crate::io::write_mailbox(&mailbox(1_000, 10_000)).unwrap()).unwrap();
    let len = fs::metadata(&path).unwrap().len();
    let big = open(&path).map(|o| o.mailbox.messages.len());
    TEST_AVAILABLE.set(0);
    assert_eq!(small.unwrap(), 20);
    assert!(len > UNMEASURED_JSON_MAX && len < MAX_MAILBOX_JSON, "{len}");
    assert_eq!(big.unwrap(), 1_000);
}
