// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::Cell;
use std::io::{self, Write};

use super::*;

#[test]
fn streamed_export_keeps_every_mailbox_field_and_message() {
    let mut mailbox = crate::sample::mailbox(1_700_000_000, 0);
    let id = *mailbox.messages.keys().next().unwrap();
    let message = mailbox.message_mut(id).unwrap();
    message.headers = vec![("List-Id".into(), "<synthetic.example.org>".into()), ("X-Synthetic".into(), "kept header".into())];
    message.split_headers();
    assert!(!message.header_block.data.is_empty());
    // Removing a header while its block is outside can leave a tombstone, never exported.
    message.headers.push((String::new(), "removed header".into()));
    let next = *mailbox.messages.keys().nth(1).unwrap();
    let message = mailbox.message_mut(next).unwrap();
    message.header_block = Default::default();
    message.headers.push((String::new(), "removed without a block".into()));
    let mut bytes = Vec::new();
    let written = write(&mut bytes, &mailbox, |id| Ok(mailbox.messages.get(&id).unwrap().clone()), u64::MAX).unwrap();
    assert_eq!(written, bytes.len() as u64);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(), serde_json::to_value(&mailbox).unwrap());
    assert!(!String::from_utf8(bytes).unwrap().contains("headerBlock"));
    assert!(!mailbox.message(id).unwrap().header_block.data.is_empty(), "export must not change the session's headers");
}

#[test]
fn a_limit_stops_before_loading_the_rest_of_the_mailbox() {
    let mailbox = crate::sample::mailbox(1_700_000_000, 0);
    let mut shell = mailbox.clone();
    shell.messages = Default::default();
    let limit = crate::io::write_mailbox(&shell).unwrap().len() as u64 + 8_192;
    let calls = Cell::new(0);
    let mut bytes = Vec::new();
    let error = write(
        &mut bytes,
        &mailbox,
        |id| {
            calls.set(calls.get() + 1);
            let mut message = Message::clone(mailbox.messages.get(&id).unwrap());
            message.body.text = "\"".repeat(10_000); // JSON escaping counts too.
            Ok(Arc::new(message))
        },
        limit,
    )
    .unwrap_err();
    assert!(error.contains("nothing was written"), "{error}");
    assert!(bytes.len() as u64 <= limit);
    assert_eq!(calls.get(), 1);
}

struct Broken;

impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("synthetic disk full"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn write_errors_are_reported_before_fetching_content() {
    let mailbox = crate::sample::mailbox(1_700_000_000, 0);
    let error = write(&mut Broken, &mailbox, |_| panic!("must not fetch after an I/O failure"), u64::MAX).unwrap_err();
    assert!(error.contains("synthetic disk full"), "{error}");
}
