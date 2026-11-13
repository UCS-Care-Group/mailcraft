// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversion between `mailcraft-mime`'s messages and the mailbox's (#1235).

use std::sync::Arc;

use mailcraft_model::{Address, Attachment, Body, FlagStatus, Importance, Message, Original};

use crate::{from_model, parse, parse_fetched, to_model};

/// The limits `mailcraft-mime` keeps of its own are the model's: a stamp it keeps apart is one
/// the model reads, and an original it keeps is one the model takes.
#[test]
fn the_readers_limits_are_the_models() {
    use mailcraft_model::auth_results;
    assert_eq!(mailcraft_mime::AUTH_AS, auth_results::AUTH_AS);
    assert_eq!(mailcraft_mime::DIRECTIONALITY, auth_results::DIRECTIONALITY);
    assert_eq!(mailcraft_mime::MAX_AUTH_AS, auth_results::MAX_AUTH_AS);
    assert_eq!(mailcraft_mime::MAX_ORIGINAL, mailcraft_model::MAX_ORIGINAL);
    assert_eq!(mailcraft_mime::MAX_ARMOUR_SCAN, mailcraft_model::protected::MAX_ARMOUR_SCAN);
    for word in ["low", "Normal", " HIGH ", "5", "3", "1", "urgent", "non-urgent", "medium", "whatever", ""] {
        let theirs =
            mailcraft_mime::Importance::parse(word).map(|i| to_model(mailcraft_mime::Message { importance: i, ..Default::default() }).importance);
        assert_eq!(theirs, Importance::parse(word), "{word:?}");
    }
}

/// A message read is new to the mailbox: received when it was sent, and nothing of a server's.
#[test]
fn a_message_read_is_received_when_it_was_sent() {
    let m = parse(b"Date: Tue, 1 Jul 2003 10:52:37 +0200\r\nFrom: ada@example.org\r\n\r\nHi\r\n");
    assert_eq!((m.date, m.received), (1_057_049_557, 1_057_049_557));
    assert_eq!((m.id, m.account, m.folder, m.uid), (0, 0, 0, 0));
    assert!(m.synced.is_none() && m.server.is_none() && !m.pinned && !m.forwarded);
}

/// Everything the format carries goes across and back unchanged.
#[test]
fn what_the_format_carries_goes_both_ways() {
    let m = Message {
        message_id: "a@example.org".into(),
        in_reply_to: "b@example.org".into(),
        references: vec!["c@example.org".into()],
        from: Address::new("Ada", "ada@example.org"),
        reply_to: vec![Address::new("", "r@example.org")],
        to: vec![Address::new("Bob", "bob@example.org")],
        cc: vec![Address::new("", "cc@example.org")],
        bcc: vec![Address::new("", "bcc@example.org")],
        subject: "Hi".into(),
        date: 7,
        received: 7,
        body: Body { text: "t".into(), html: Some("<p>t</p>".into()) },
        hidden_text: "pre".into(),
        attachments: vec![Attachment { content_id: "x@example.org".into(), inline: true, ..Attachment::new("a.png", "image/png", vec![1, 2]) }],
        read: true,
        flag: FlagStatus::Complete,
        importance: Importance::Low,
        read_receipt: true,
        categories: vec!["Work".into()],
        answered: true,
        draft: true,
        headers: vec![("X-A".into(), "b".into())],
        fresh_stamps: true,
        original: Original::new(b"Content-Type: multipart/signed\r\n\r\n".to_vec()),
        ..Message::default()
    };
    assert_eq!(to_model(from_model(&m)), m);
    // The bytes are shared, not copied.
    let there = from_model(&m);
    assert!(Arc::ptr_eq(&there.attachments[0].data, &m.attachments[0].data));
    // An original kept outside memory goes as none.
    let kept = Message { original: Some(Original { data: Arc::new(Vec::new()), kept: None }), ..m.clone() };
    assert!(from_model(&kept).original.is_none());
}

/// A Digitally signed message read as it came keeps its original in the mailbox.
#[test]
fn a_fetched_signed_message_keeps_its_original() {
    let signed = b"Content-Type: multipart/signed; protocol=\"application/pgp-signature\"; boundary=b\r\n\r\n--b\r\n\r\nHi\r\n--b\r\nContent-Type: application/pgp-signature\r\n\r\nsig\r\n--b--\r\n";
    let m = parse_fetched(signed);
    assert_eq!(m.original_bytes(), Some(&signed[..]));
    assert!(m.has_original());
    assert!(parse(signed).original.is_none());
}
