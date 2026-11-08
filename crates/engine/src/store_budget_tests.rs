// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What parsing a mailbox file builds is charged while it is built (`load_limit`), under the
//! limits of the crate's tests (`MAX_ITEM` 4 MB, a kept cap of 24 MB, [`TEST_ELEMENTS`]), and a
//! real mailbox costs far less of the budget than any machine gets.

use super::*;
use crate::test_support::{Limits, with_limits};

/// `{"version":1,"messages":{"1":{"folder":<inbox>,"attachments":[{},{},…]}}}`, about `len`
/// bytes long.
fn empty_attachments(len: usize) -> Vec<u8> {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    mb.add_message(Message { folder: inbox, ..Default::default() }).unwrap();
    let head = serde_json::to_string(&mb).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&head).unwrap();
    let message = value["messages"].as_object_mut().unwrap().values_mut().next().unwrap();
    message["attachments"] = serde_json::json!(["ELEMENTS"]);
    let text = value.to_string();
    let n = len.saturating_sub(text.len()) / 3;
    let list = vec!["{}"; n].join(",");
    text.replace(r#"["ELEMENTS"]"#, &format!("[{list}]")).into_bytes()
}

#[test]
fn a_streamed_item_of_empty_attachments_is_refused_while_it_is_read() {
    // AU3-05: under the item gate (4 MB) and the kept cap (24 MB) both, yet a few hundred MB once
    // parsed.
    let t = Temp::new("budget-stream");
    let bytes = empty_attachments(usize::try_from(stream::MAX_ITEM).unwrap() - 4096);
    assert!((bytes.len() as u64) < stream::MAX_ITEM);
    fs::write(t.file(), &bytes).unwrap();
    let e = with_limits(Limits { element_bytes: TEST_ELEMENTS, stream: true }, || open(&t.file())).err().unwrap();
    assert!(e.contains("memory to open"), "{e}");
    // The same file with room for it opens, as a stream.
    let opened = with_limits(Limits { element_bytes: 4 << 30, stream: true }, || open(&t.file())).unwrap();
    assert!(opened.split);
}

#[test]
fn a_whole_file_of_four_million_empty_attachments_is_refused_while_it_is_read() {
    // AU3-06: 12 MB of file, read whole (under `MAX_MAILBOX_JSON` and three times under the
    // memory a test reports), refused long before it costs gigabytes.
    let t = Temp::new("budget-whole");
    let bytes = empty_attachments(12_000_055);
    assert!(bytes.len() as u64 > 11 << 20 && (bytes.len() as u64) < MAX_MAILBOX_JSON);
    fs::write(t.file(), &bytes).unwrap();
    TEST_AVAILABLE.set(16 << 30);
    let e = open(&t.file()).err().unwrap();
    TEST_AVAILABLE.set(0);
    assert!(e.contains("memory to open"), "{e}");
}

#[test]
fn every_item_of_a_file_with_a_log_of_records_is_charged_to_one_budget() {
    // AU3-07: many items, each within any budget for one, together over it, read on several
    // threads.
    let t = Temp::new("budget-index");
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    for i in 0..400 {
        let m = Message { folder: inbox, subject: format!("{i}"), references: vec![String::new(); 2_000], ..Default::default() };
        mb.add_message(m).unwrap();
    }
    save(&t.file(), &mb).unwrap();
    assert!(is_index(&t.file()).unwrap());
    let e = with_limits(Limits { element_bytes: 32 << 20, stream: false }, || open(&t.file())).err().unwrap();
    assert!(e.contains("memory to open"), "{e}");
    let opened = with_limits(Limits { element_bytes: 1 << 30, stream: false }, || open(&t.file())).unwrap();
    assert_eq!(opened.mailbox.messages.len(), 400);
}

#[test]
fn a_real_mailbox_costs_a_few_kilobytes_a_message() {
    // What the budget is set from: the sample mailbox (real addresses, references, headers,
    // categories, bodies) charged as it is read, per message and against its own text.
    let mb = crate::sample::mailbox(1_791_993_600, 0);
    let bytes = crate::io::write_mailbox(&mb).unwrap();
    let budget = load_limit::Budget::new(usize::MAX, u64::MAX);
    let back: Mailbox = load_limit::read_json(&bytes, &budget).unwrap();
    let charged = u64::MAX - budget.bytes_left();
    let items = (back.messages.len() + back.contacts.len() + back.events.len() + back.tasks.len()) as u64;
    let per_item = charged / items.max(1);
    assert!(per_item < 6 << 10, "{per_item} bytes an item ({charged} for {items})");
    // At that, the least budget any machine gets reads a quarter of a million items.
    assert!(per_item * 250_000 < 1 << 30);
}

#[test]
fn the_budget_is_kept_when_threads_share_it() {
    let budget = load_limit::Budget::new(usize::MAX, 8 << 20);
    let item = serde_json::to_vec(&Message { references: vec![String::new(); 1_000], ..Default::default() }).unwrap();
    let read: Vec<Option<bool>> = fan_out(400, |_| load_limit::read_json::<Message>(&item, &budget).is_ok());
    let ok = read.iter().filter(|r| **r == Some(true)).count();
    assert!(ok > 0 && ok < 400, "{ok}");
    assert!(budget.spent().is_some());
}

/// A mailbox in the form with a log of records of `n` messages with nothing in them.
fn empty_messages(t: &Temp, n: usize) -> PathBuf {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    for _ in 0..n {
        mb.add_message(Message { folder: inbox, ..Default::default() }).unwrap();
    }
    save(&t.file(), &mb).unwrap();
    assert!(is_index(&t.file()).unwrap());
    t.file()
}

#[test]
fn many_tiny_records_are_charged_while_they_are_replayed() {
    // Each record held costs its bytes and `RECORD_COST`, however small it is in the file.
    let t = Temp::new("budget-replay");
    let path = empty_messages(&t, 20_000);
    let bytes = fs::read(&path).unwrap();
    let len = bytes.len() as u64;
    let room = load_limit::Budget::new(usize::MAX, u64::MAX);
    let replayed = index::replay_within(&bytes[..], len, index::MAX_RECORDS, Some(&room)).unwrap();
    let charged = u64::MAX - room.bytes_left();
    assert!(charged >= replayed.items.len() as u64 * index::RECORD_COST, "{charged}");
    // A budget of less is refused while the records are read, not after.
    let small = load_limit::Budget::new(usize::MAX, charged / 2);
    let e = index::replay_within(&bytes[..], len, index::MAX_RECORDS, Some(&small)).err().unwrap();
    assert!(e.contains("memory to open"), "{e}");
    // And so is an open whose budget the records alone are more than.
    let e = with_limits(Limits { element_bytes: charged / 2, stream: false }, || open(&path)).err().unwrap();
    assert!(e.contains("memory to open"), "{e}");
    // A file longer than the budget is refused before it is read.
    let e = with_limits(Limits { element_bytes: len - 1, stream: false }, || open(&path)).err().unwrap();
    assert!(e.contains("memory to open"), "{e}");
}

#[test]
fn a_saved_mailbox_opens_under_the_budget_where_memory_cant_be_measured_in_proportion() {
    // A fiftieth of a million messages under a fiftieth of the budget: so a million open under
    // the whole of it (`tests/mailbox_element_budget.rs`, `a_million_messages_open_where_memory_cant_be_measured`).
    let t = Temp::new("budget-proportion");
    let path = empty_messages(&t, 20_000);
    let opened = with_limits(Limits { element_bytes: UNMEASURED_ELEMENTS / 50, stream: false }, || open(&path)).unwrap();
    assert_eq!(opened.mailbox.messages.len(), 20_000);
    // With room to spare: a third of it is enough.
    let opened = with_limits(Limits { element_bytes: UNMEASURED_ELEMENTS / 150, stream: false }, || open(&path)).unwrap();
    assert_eq!(opened.mailbox.messages.len(), 20_000);
}

#[test]
fn a_memory_reading_of_zero_falls_back_to_the_unmeasured_budget() {
    // AU3-R3: Windows reads its memory through a call that says 0 when it fails.
    assert_eq!(memory_reading(0), None);
    assert_eq!(elements_for(memory_reading(0)), UNMEASURED_ELEMENTS);
    assert_eq!(elements_for(memory_reading(16 << 30)), 8 << 30);
}

#[test]
fn the_budget_is_half_the_memory_within_its_bounds() {
    assert_eq!(elements_for(Some(512 << 20)), MIN_ELEMENTS);
    assert_eq!(elements_for(Some(6 << 30)), 3 << 30);
    assert_eq!(elements_for(Some(256 << 30)), MAX_ELEMENTS);
    assert_eq!(elements_for(Some(u64::MAX)), MAX_ELEMENTS);
}
