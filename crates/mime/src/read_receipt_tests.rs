// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Request a Read Receipt (RFC 8098): a message written with it asks its sender, once, and a
//! request in mail that is read is never taken up.

use mailcraft_model::{Address, Message};

use crate::{WriteOptions, parse, write};

fn message(read_receipt: bool) -> Message {
    Message {
        from: Address { name: "Maya Lin".into(), email: "maya@lumen.example".into() },
        to: vec![Address { name: String::new(), email: "ada@example.org".into() }],
        subject: "Notes".into(),
        read_receipt,
        ..Default::default()
    }
}

fn text(m: &Message) -> String {
    String::from_utf8(write(m, &WriteOptions::default())).unwrap()
}

#[test]
fn the_request_names_the_sender_once() {
    let mut m = message(true);
    // Kept from a copy that came down from a server: not written twice.
    m.headers.push(("Disposition-Notification-To".into(), "someone@example.org".into()));
    let out = text(&m);
    assert!(out.contains("Disposition-Notification-To: Maya Lin <maya@lumen.example>\r\n"), "{out}");
    assert_eq!(out.matches("Disposition-Notification-To").count(), 1, "{out}");
    assert!(!out.contains("someone@example.org"), "{out}");
}

#[test]
fn no_request_without_it_or_without_a_sender() {
    assert!(!text(&message(false)).contains("Disposition-Notification-To"));
    let mut m = message(true);
    m.from = Address::default();
    assert!(!text(&m).contains("Disposition-Notification-To"));
    // A sender's name can't add a field to the request.
    let mut m = message(true);
    m.from.name = "Maya\r\nBcc: evil@example.org".into();
    let out = text(&m);
    assert!(!out.contains("\r\nBcc:"), "{out}");
}

#[test]
fn a_request_in_mail_that_is_read_is_only_kept_as_a_header() {
    let m = parse(
        b"From: Bea <bea@example.org>\r\nTo: owner@example.org\r\nSubject: Read me\r\nDisposition-Notification-To: bea@example.org\r\n\r\nHello\r\n",
    );
    assert!(!m.read_receipt);
    // Written out again as it came (a saved copy), never as a request of the owner's.
    let out = text(&m);
    assert_eq!(out.matches("Disposition-Notification-To: bea@example.org").count(), 1, "{out}");
}
