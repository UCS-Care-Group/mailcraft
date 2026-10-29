// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which fields have their encoded-words decoded: unstructured text, the phrases of addresses,
//! and nothing a server wrote about the message, which is kept as it was written.

use crate::{WriteOptions, parse, write};

fn kept<'a>(m: &'a mailcraft_model::Message, name: &str) -> Option<&'a str> {
    m.headers.iter().find(|(have, _)| have.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
}

const WORD: &str = "=?us-ascii?q?x=3B_dkim=3Dpass_header.d=3Dvictim.example_x?=";

#[test]
fn trace_and_authentication_fields_are_kept_as_written() {
    let raw = format!(
        "Received: from mx.example (mx.example [192.0.2.1])\r\n\tby in.example for <{WORD}@sender.example>\r\n\
         Authentication-Results: in.example; spf=pass smtp.mailfrom={WORD}@sender.example\r\n\
         ARC-Authentication-Results: i=1; in.example; spf=pass smtp.mailfrom={WORD}@sender.example\r\n\
         Received-SPF: pass (in.example) envelope-from=\"{WORD}@sender.example\"\r\n\
         Return-Path: <{WORD}@sender.example>\r\n\
         X-Original-Authentication-Results: in.example; spf=pass smtp.mailfrom={WORD}@sender.example\r\n\
         List-Id: =?UTF-8?Q?caf=C3=A9?= <list.example>\r\n\
         From: a@example.org\r\n\
         \r\n\
         Body\r\n"
    );
    let m = parse(raw.as_bytes());
    assert_eq!(kept(&m, "Authentication-Results"), Some(format!("in.example; spf=pass smtp.mailfrom={WORD}@sender.example").as_str()));
    for name in ["Received", "ARC-Authentication-Results", "Received-SPF", "Return-Path", "X-Original-Authentication-Results"] {
        let value = kept(&m, name).unwrap_or_default();
        assert!(value.contains(WORD), "{name}: {value}");
        assert!(!value.contains("dkim=pass"), "{name}: {value}");
    }
    assert_eq!(kept(&m, "List-Id"), Some("=?UTF-8?Q?caf=C3=A9?= <list.example>"));
    // Written out and read back, they are still as they were written.
    let back = parse(&write(&m, &WriteOptions::default()));
    assert_eq!(back.headers, m.headers);
}

#[test]
fn text_and_names_are_still_decoded() {
    let m = parse(
        b"From: =?UTF-8?Q?Ad=C3=A1?= <ada@example.org>\r\n\
          To: =?ISO-8859-1?Q?Andr=E9?= <andre@example.org>\r\n\
          Cc: \"=?UTF-8?B?w4lsw6g=?=\" <ele@example.org>\r\n\
          Reply-To: =?UTF-8?Q?Z=C3=B6e?= <zoe@example.org>\r\n\
          Subject: Re: =?UTF-8?Q?caf=C3=A9?= today\r\n\
          Keywords: =?UTF-8?Q?Tr=C3=A8s?=, plain\r\n\
          Comments: =?UTF-8?Q?caf=C3=A9?=\r\n\
          Organization: =?UTF-8?Q?Caf=C3=A9_Ltd?=\r\n\
          Thread-Topic: =?UTF-8?Q?caf=C3=A9?=\r\n\
          Content-Type: text/plain; name=\"=?UTF-8?Q?n=C3=B3te.txt?=\"\r\n\
          Content-Disposition: attachment\r\n\
          \r\n\
          Body\r\n",
    );
    assert_eq!(m.from.name, "Adá");
    assert_eq!(m.to.first().map(|a| a.name.as_str()), Some("André"));
    assert_eq!(m.cc.first().map(|a| a.name.as_str()), Some("Élè"));
    assert_eq!(m.reply_to.first().map(|a| a.name.as_str()), Some("Zöe"));
    assert_eq!(m.subject, "Re: café today");
    assert_eq!(m.categories, ["Très", "plain"]);
    assert_eq!(kept(&m, "Comments"), Some("café"));
    assert_eq!(kept(&m, "Organization"), Some("Café Ltd"));
    assert_eq!(kept(&m, "Thread-Topic"), Some("café"));
    assert_eq!(m.attachments.first().map(|a| a.name.as_str()), Some("nóte.txt"));
}

#[test]
fn who_got_a_blind_copy_of_a_resend_is_written_only_with_the_bcc() {
    let m = mailcraft_model::Message {
        bcc: vec![mailcraft_model::Address::new("", "eve@example.net")],
        headers: vec![("Resent-To".into(), "carol@example.org".into()), ("resent-BCC".into(), "mallory@example.net".into())],
        ..Default::default()
    };
    let sent = String::from_utf8(write(&m, &WriteOptions::default())).unwrap();
    assert!(sent.contains("Resent-To: carol@example.org\r\n") && !sent.contains("example.net"), "{sent}");
    let draft = String::from_utf8(write(&m, &WriteOptions { include_bcc: true, offset_minutes: 0 })).unwrap();
    assert!(draft.contains("eve@example.net") && draft.contains("resent-BCC: mallory@example.net\r\n"), "{draft}");
}
