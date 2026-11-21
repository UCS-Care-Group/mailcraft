// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reader on its own types, reusable outside the client (#1235): bytes in, the crate's own
//! [`mailcraft_mime::Message`] out, written back by the same crate.

use mailcraft_mime::{Address, FlagStatus, Importance, Message, WriteOptions, parse, write};

#[test]
fn a_message_reads_into_the_crates_own_types() {
    let m: Message = parse(
        b"From: Ada <ada@example.org>\r\nTo: bob@example.org\r\nSubject: Hi\r\nImportance: high\r\nX-Status: F\r\n\
Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\n\r\nHello\r\n--b\r\nContent-Type: application/pdf; name=a.pdf\r\n\r\n%PDF\r\n--b--\r\n",
    );
    assert_eq!(m.from, Address { name: "Ada".into(), email: "ada@example.org".into() });
    assert_eq!(m.to, [Address { name: String::new(), email: "bob@example.org".into() }]);
    assert_eq!(m.subject, "Hi");
    assert_eq!(m.body.text, "Hello");
    assert_eq!((m.importance, m.flag), (Importance::High, FlagStatus::Flagged));
    let files: Vec<(&str, &str, &[u8])> = m.attachments.iter().map(|a| (a.name.as_str(), a.mime.as_str(), a.data.as_slice())).collect();
    assert_eq!(files, [("a.pdf", "application/pdf", &b"%PDF"[..])]);
}

#[test]
fn what_is_written_reads_back() {
    let m = Message {
        from: Address { name: "Ada".into(), email: "ada@example.org".into() },
        subject: "Grüße".into(),
        body: mailcraft_mime::Body { text: "Hello\n".into(), html: None },
        ..Message::default()
    };
    let back = parse(&write(&m, &WriteOptions::default()));
    assert_eq!((back.from, back.subject, back.body.text), (m.from, m.subject, "Hello\n".to_string()));
}

/// The crate stands without the client's model, so that other programs can use it.
#[test]
fn the_crate_does_not_depend_on_the_clients_model() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    assert!(!manifest.contains("mailcraft-model"), "{manifest}");
}
