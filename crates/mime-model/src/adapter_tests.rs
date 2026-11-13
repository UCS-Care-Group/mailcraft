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
    assert_eq!(mailcraft_mime::MAX_READ_HEADER, mailcraft_model::MAX_READ_HEADER);
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
        read_header: "Subject: Hi\n".into(),
        headers_cut: true,
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

/// Every way in that reads bytes into the mailbox's message (one message, one as its server gave
/// it, signed or not, and an mbox read whole or one at a time) brings the reader's cut mark and
/// the lines read into the message, and a rule with an exception on a field past the cap doesn't
/// act on it (#566).
#[test]
fn every_way_in_carries_the_cut_mark_and_the_lines_read() {
    use mailcraft_model::{Action, Condition, Rule};
    let fields = (0..201).map(|i| format!("X-Trace-{i}: hop\r\n")).collect::<String>();
    let head = format!("From: a@example.org\r\nTo: b@example.org\r\nSubject: offer\r\n{fields}X-Gateway: keep-this\r\n");
    let plain = format!("{head}\r\nbody\r\n");
    let signed = format!(
        "{head}Content-Type: multipart/signed; protocol=\"application/pgp-signature\"; boundary=b\r\n\r\n--b\r\n\r\nHi\r\n--b\r\nContent-Type: application/pgp-signature\r\n\r\nsig\r\n--b--\r\n"
    );
    let mbox = format!("From a@example.org Tue Nov 10 10:00:00 2026\r\n{plain}");
    let rule = Rule {
        name: "r".into(),
        enabled: true,
        conditions: vec![Condition::SubjectContains(vec!["offer".into()])],
        exceptions: vec![Condition::HeaderContains(vec!["keep-this".into()])],
        actions: vec![Action::Delete],
        ..Default::default()
    };
    let read = [
        ("parse", vec![parse(plain.as_bytes())]),
        ("parse_fetched", vec![parse_fetched(plain.as_bytes())]),
        ("parse_fetched, signed", vec![parse_fetched(signed.as_bytes())]),
        ("parse_mbox", crate::parse_mbox(mbox.as_bytes())),
        ("mbox_messages", crate::mbox_messages(mbox.as_bytes()).collect()),
    ];
    for (way, messages) in read {
        assert_eq!(messages.len(), 1, "{way}");
        for m in &messages {
            assert!(m.headers_cut, "{way}: the cut mark is lost");
            assert!(m.read_header.contains("Subject: offer\n"), "{way}: {:?}", m.read_header);
            assert!(!rule.matches(m), "{way}: the rule deletes a message its exception was to leave alone");
        }
    }
    // Within the caps, nothing is said cut, by any way in.
    let whole = "From: a@example.org\r\nSubject: offer\r\nX-Gateway: keep-this\r\n\r\nbody\r\n";
    assert!(!parse(whole.as_bytes()).headers_cut && !parse_fetched(whole.as_bytes()).headers_cut);
    assert!(crate::parse_mbox(format!("From a@example.org Tue Nov 10 10:00:00 2026\r\n{whole}").as_bytes()).iter().all(|m| !m.headers_cut));
    // Neither is written: what comes back of a cut message written out is what the bytes say.
    let cut = parse(plain.as_bytes());
    let back = parse(&crate::write(&cut, &crate::WriteOptions::default()));
    assert!(!back.read_header.contains("keep-this") && back.read_header.contains("Subject: offer\n"));
}
