// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! AU3-05, AU3-06, AU3-07, H2: what parsing a mailbox file builds is charged while it is built,
//! every list element at any depth, against one budget, in every form a mailbox file comes in: one
//! file read whole, the same read as a stream, the form with a log of records, and a mailbox handed
//! over as text (`file.open` with `data`, the browser's stored mailbox). The same small file of
//! empty list elements is refused by each with the error that it needs more memory to open than it
//! may have, and opens when the budget has room for it.

use std::fs;
use std::path::PathBuf;

use mailcraft_engine::Session;
use mailcraft_engine::test_support::{Limits, with_limits};
use mailcraft_model::Mailbox;
use serde_json::json;

/// A directory of the test's own, removed after it.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-element-budget-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A one-file mailbox of one message whose attachments are `n` empty objects: three bytes of the
/// file each, a few hundred in memory.
fn payload(n: usize) -> Vec<u8> {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    mb.add_message(mailcraft_model::Message { folder: inbox, subject: "Many".into(), ..Default::default() }).unwrap();
    let mut value = serde_json::to_value(&mb).unwrap();
    let message = value["messages"].as_object_mut().unwrap().values_mut().next().unwrap();
    message["attachments"] = json!(["ELEMENTS"]);
    let mut list = String::with_capacity(n * 3);
    for i in 0..n {
        if i > 0 {
            list.push(',');
        }
        list.push_str("{}");
    }
    value.to_string().replace(r#"["ELEMENTS"]"#, &format!("[{list}]")).into_bytes()
}

/// Elements in the payload: few enough that the test is quick, many enough to cost far more than
/// [`SMALL`].
const ELEMENTS: usize = 200_000;

/// A budget the payload doesn't fit in (16 MB), and one it does (1 GiB).
const SMALL: u64 = 16 << 20;
const AMPLE: u64 = 1 << 30;

fn limits(element_bytes: u64, stream: bool) -> Limits {
    Limits { element_bytes, stream }
}

fn refused(result: Result<impl Sized, String>, form: &str) {
    match result {
        Ok(_) => panic!("{form}: opened"),
        Err(e) => assert!(e.contains("memory to open"), "{form}: {e}"),
    }
}

/// The payload as a mailbox, read with room for it.
fn whole_mailbox(bytes: &[u8]) -> Mailbox {
    with_limits(limits(AMPLE, false), || mailcraft_engine::io::read_mailbox(bytes)).unwrap()
}

#[test]
fn one_file_read_whole_is_refused() {
    let t = Temp::new("whole");
    let path = t.file("whole.mailcraft");
    fs::write(&path, payload(ELEMENTS)).unwrap();
    refused(with_limits(limits(SMALL, false), || mailcraft_engine::store::open(&path)), "whole");
    let opened = with_limits(limits(AMPLE, false), || mailcraft_engine::store::open(&path)).unwrap();
    assert!(!opened.split, "read whole");
    assert_eq!(opened.mailbox.messages.values().next().unwrap().attachments.len(), ELEMENTS);
}

#[test]
fn one_file_read_as_a_stream_is_refused() {
    let t = Temp::new("stream");
    let path = t.file("stream.mailcraft");
    fs::write(&path, payload(ELEMENTS)).unwrap();
    refused(with_limits(limits(SMALL, true), || mailcraft_engine::store::open(&path)), "stream");
    let opened = with_limits(limits(AMPLE, true), || mailcraft_engine::store::open(&path)).unwrap();
    assert!(opened.split, "read as a stream");
    assert_eq!(opened.mailbox.messages.values().next().unwrap().attachments.len(), ELEMENTS);
}

#[test]
fn the_form_with_a_log_of_records_is_refused() {
    let t = Temp::new("index");
    let path = t.file("index.mailcraft");
    let mailbox = whole_mailbox(&payload(ELEMENTS));
    mailcraft_engine::store::save(&path, &mailbox).unwrap();
    assert!(fs::read(&path).unwrap().starts_with(mailcraft_engine::io::INDEX_MAGIC));
    refused(with_limits(limits(SMALL, false), || mailcraft_engine::store::open(&path)), "index");
    let opened = with_limits(limits(AMPLE, false), || mailcraft_engine::store::open(&path)).unwrap();
    assert_eq!(opened.mailbox.messages.values().next().unwrap().attachments.len(), ELEMENTS);
}

#[test]
fn a_mailbox_handed_over_as_text_is_refused() {
    let bytes = payload(ELEMENTS);
    // As the browser reads its stored mailbox.
    refused(with_limits(limits(SMALL, false), || mailcraft_engine::io::read_mailbox(&bytes)), "data");
    assert_eq!(whole_mailbox(&bytes).messages.values().next().unwrap().attachments.len(), ELEMENTS);
    // As `file.open` reads `data`.
    let data = mailcraft_charset::base64::STANDARD.encode(&bytes);
    let params = json!({"name": "given.mailcraft", "data": data});
    let mut s = Session::new(Mailbox::default());
    let e = with_limits(limits(SMALL, false), || s.run("file.open", &params)).unwrap_err();
    assert!(e.to_string().contains("memory to open"), "file.open: {e}");
    let mut s = Session::new(Mailbox::default());
    with_limits(limits(AMPLE, false), || s.run("file.open", &params)).unwrap();
    assert_eq!(s.mailbox.messages.values().next().unwrap().attachments.len(), ELEMENTS);
}

/// The element of the payload deep in a value of any kind is charged too: a list of lists held
/// where the mailbox keeps whatever JSON the file has (an account of a kind this version doesn't
/// know).
#[test]
fn elements_at_any_depth_are_charged() {
    let mut json = String::from(r#"{"version":1,"accounts":[{"id":1,"kind":"later","extra":[["#);
    for i in 0..ELEMENTS {
        if i > 0 {
            json.push(',');
        }
        json.push_str("[]");
    }
    json.push_str("]]}]}");
    refused(with_limits(limits(SMALL, false), || mailcraft_engine::io::read_mailbox(json.as_bytes())), "deep");
    with_limits(limits(AMPLE, false), || mailcraft_engine::io::read_mailbox(json.as_bytes())).unwrap();
}

/// A mailbox of a million messages, saved in the form with a log of records, opens within the
/// budget of a computer whose memory can't be measured (8 GiB, `store::UNMEASURED_ELEMENTS`).
/// (`store_budget_tests.rs` checks a fiftieth of it at every run.)
#[test]
#[ignore = "saves and opens a million messages (a few GB of memory); run by hand"]
fn a_million_messages_open_where_memory_cant_be_measured() {
    let t = Temp::new("million");
    let path = t.file("million.mailcraft");
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    for _ in 0..1_000_000 {
        mb.add_message(mailcraft_model::Message { folder: inbox, ..Default::default() }).unwrap();
    }
    mailcraft_engine::store::save(&path, &mb).unwrap();
    drop(mb);
    let opened = with_limits(limits(8 << 30, false), || mailcraft_engine::store::open(&path)).unwrap();
    assert_eq!(opened.mailbox.messages.len(), 1_000_000);
}
