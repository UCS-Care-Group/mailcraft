// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::Address;

fn a(name: &str, email: &str) -> Address {
    Address { name: name.into(), email: email.into() }
}

/// A message from lines, joined with CRLF.
fn eml(lines: &[&str]) -> Vec<u8> {
    lines.join("\r\n").into_bytes()
}

#[test]
fn a_plain_message() {
    let m = parse(&eml(&[
        "Return-Path: <ada@example.org>",
        "Received: from mail.example.org (mail.example.org [192.0.2.1])",
        "\tby mx.example.org with ESMTP id 4F2A1; Tue, 1 Jul 2003 10:52:40 +0200",
        "Date: Tue, 1 Jul 2003 10:52:37 +0200",
        "From: Ada Lovelace <ada@example.org>",
        "To: Bob <bob@example.org>, carol@example.org",
        "Cc: \"Hopper, Grace\" <grace@example.org>",
        "Reply-To: replies@example.org",
        "Subject: Lunch on Friday?",
        "Message-ID: <abc.123@example.org>",
        "In-Reply-To: <parent.1@example.org>",
        "References: <root.0@example.org>",
        " <parent.1@example.org>",
        "User-Agent: SyntheticMail 1.0",
        "MIME-Version: 1.0",
        "Content-Type: text/plain; charset=us-ascii",
        "",
        "Hello Bob,",
        "",
        "Lunch on Friday?",
        "",
        "Ada",
        "",
    ]));
    assert_eq!(m.from, a("Ada Lovelace", "ada@example.org"));
    assert_eq!(m.to, [a("Bob", "bob@example.org"), a("", "carol@example.org")]);
    assert_eq!(m.cc, [a("Hopper, Grace", "grace@example.org")]);
    assert_eq!(m.reply_to, [a("", "replies@example.org")]);
    assert!(m.bcc.is_empty());
    assert_eq!(m.subject, "Lunch on Friday?");
    assert_eq!(m.date, 1_057_049_557);
    assert_eq!(m.message_id, "abc.123@example.org");
    assert_eq!(m.in_reply_to, "parent.1@example.org");
    assert_eq!(m.references, ["root.0@example.org", "parent.1@example.org"]);
    assert_eq!(m.body.text, "Hello Bob,\n\nLunch on Friday?\n\nAda");
    assert_eq!(m.body.html, None);
    assert!(m.attachments.is_empty());
    let kept: Vec<&str> = m.headers.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(kept, ["Return-Path", "Received", "User-Agent"]);
    assert_eq!(
        m.headers.get(1).map(|(_, v)| v.as_str()),
        Some("from mail.example.org (mail.example.org [192.0.2.1])\tby mx.example.org with ESMTP id 4F2A1; Tue, 1 Jul 2003 10:52:40 +0200")
    );
    assert!(!m.read && !m.answered && !m.draft);
    assert_eq!((m.flag, m.importance), (FlagStatus::None, Importance::Normal));
}

#[test]
fn bare_lf_line_ends() {
    let m = parse(b"From: ada@example.org\nSubject: Unix\n\nLine one\nLine two\n");
    assert_eq!(m.from.email, "ada@example.org");
    assert_eq!(m.subject, "Unix");
    assert_eq!(m.body.text, "Line one\nLine two");
}

#[test]
fn encoded_words_in_subject_and_names() {
    let m = parse(&eml(&[
        "From: =?UTF-8?B?QWTDoSBMw7N2ZWxhY2U=?= <ada@example.org>",
        "To: =?ISO-8859-1?Q?Andr=E9?= <andre@example.org>",
        "Subject: =?UTF-8?Q?Caf=C3=A9_=E2=82=AC5?=",
        " =?UTF-8?Q?_tomorrow?=",
        "",
        "x",
    ]));
    assert_eq!(m.from, a("Adá Lóvelace", "ada@example.org"));
    assert_eq!(m.to, [a("André", "andre@example.org")]);
    assert_eq!(m.subject, "Café €5 tomorrow");
}

#[test]
fn html_and_plain_alternative() {
    let m = parse(&eml(&[
        "From: ada@example.org",
        "Subject: Both",
        "MIME-Version: 1.0",
        "Content-Type: multipart/alternative; boundary=\"alt-1\"",
        "",
        "This is the preamble, for readers that don't know MIME.",
        "--alt-1",
        "Content-Type: text/plain; charset=utf-8",
        "",
        "Hello *world*",
        "--alt-1",
        "Content-Type: text/html; charset=utf-8",
        "",
        "<p>Hello <b>world</b></p>",
        "--alt-1--",
        "The epilogue.",
    ]));
    assert_eq!(m.body.text, "Hello *world*");
    assert_eq!(m.body.html.as_deref(), Some("<p>Hello <b>world</b></p>"));
    assert!(m.attachments.is_empty());
}

#[test]
fn html_only_gets_readable_text() {
    let m = parse(&eml(&[
        "Content-Type: text/html; charset=utf-8",
        "",
        "<html><body><h1>Sale</h1><p>Up to <b>50%</b> off&nbsp;today.</p></body></html>",
    ]));
    assert_eq!(m.body.html.as_deref(), Some("<html><body><h1>Sale</h1><p>Up to <b>50%</b> off&nbsp;today.</p></body></html>"));
    assert_eq!(m.body.text, "Sale\n\nUp to 50% off today.");
    // A blank plain part next to HTML counts as none.
    let m = parse(&eml(&[
        "Content-Type: multipart/alternative; boundary=b",
        "",
        "--b",
        "Content-Type: text/plain",
        "",
        "  ",
        "--b",
        "Content-Type: text/html",
        "",
        "<p>Real text</p>",
        "--b--",
    ]));
    assert_eq!(m.body.text, "Real text");
}

#[test]
fn html_with_an_inline_image() {
    let m = parse(&eml(&[
        "Content-Type: multipart/related; boundary=\"rel\"; type=\"text/html\"",
        "",
        "--rel",
        "Content-Type: text/html; charset=utf-8",
        "",
        "<p>Logo: <img src=\"cid:logo@example.org\" alt=\"Our logo\"></p>",
        "--rel",
        "Content-Type: image/png; name=\"logo.png\"",
        "Content-Transfer-Encoding: base64",
        "Content-ID: <logo@example.org>",
        "",
        "iVBORw0KGgo=",
        "--rel--",
    ]));
    assert_eq!(m.body.text, "Logo: [Our logo]");
    assert_eq!(m.attachments.len(), 1);
    let logo = &m.attachments[0];
    assert_eq!((logo.name.as_str(), logo.mime.as_str(), logo.content_id.as_str(), logo.inline), ("logo.png", "image/png", "logo@example.org", true));
    assert_eq!(logo.data.as_slice(), b"\x89PNG\r\n\x1a\n");
}

#[test]
fn two_attachments_one_with_an_rfc2231_name() {
    let m = parse(&eml(&[
        "Subject: Files",
        "Content-Type: multipart/mixed; boundary=mix",
        "",
        "--mix",
        "Content-Type: text/plain; charset=utf-8",
        "",
        "See attached.",
        "--mix",
        "Content-Type: application/pdf",
        "Content-Disposition: attachment; filename=\"report.pdf\"",
        "Content-Transfer-Encoding: base64",
        "",
        "JVBERi0xLjQK",
        "--mix",
        "Content-Type: text/plain; charset=utf-8",
        "Content-Disposition: attachment;",
        " filename*0*=utf-8''r%C3%A9sum%C3%A9%20;",
        " filename*1*=%E2%82%AC.txt",
        "Content-Transfer-Encoding: quoted-printable",
        "",
        "Prix: 5 =E2=82=AC=",
        "",
        "--mix--",
    ]));
    assert_eq!(m.body.text, "See attached.");
    assert_eq!(m.attachments.len(), 2);
    assert_eq!((m.attachments[0].name.as_str(), m.attachments[0].mime.as_str(), m.attachments[0].inline), ("report.pdf", "application/pdf", false));
    assert_eq!(m.attachments[0].data.as_slice(), b"%PDF-1.4\n");
    assert_eq!((m.attachments[1].name.as_str(), m.attachments[1].mime.as_str()), ("résumé €.txt", "text/plain"));
    assert_eq!(m.attachments[1].data.as_slice(), "Prix: 5 €".as_bytes());
}

#[test]
fn quoted_printable_latin1() {
    let m = parse(&eml(&[
        "Subject: =?iso-8859-1?Q?R=E9union?=",
        "Content-Type: text/plain; charset=ISO-8859-1",
        "Content-Transfer-Encoding: quoted-printable",
        "",
        "La r=E9union est =E0 10h. Co=FBt: 5=80 (=93environ=94).=20",
        "Une ligne tr=E8s longue qui continue =",
        "sur la suivante.",
    ]));
    assert_eq!(m.subject, "Réunion");
    assert_eq!(m.body.text, "La réunion est à 10h. Coût: 5€ (“environ”). \nUne ligne très longue qui continue sur la suivante.");
}

#[test]
fn base64_utf8() {
    let m = parse(&eml(&[
        "Content-Type: text/plain; charset=\"UTF-8\"",
        "Content-Transfer-Encoding: BASE64",
        "",
        "R3LDvMOfZSBhdXMg",
        "TcO8bmNoZW4g8J+Ziw0KWndlaXRlIFplaWxl",
        "",
    ]));
    assert_eq!(m.body.text, "Grüße aus München 🙋\nZweite Zeile");
}

#[test]
fn a_calendar_invitation() {
    let m = parse(&eml(&[
        "Subject: Planning",
        "Content-Type: multipart/alternative; boundary=cal",
        "",
        "--cal",
        "Content-Type: text/plain",
        "",
        "You are invited.",
        "--cal",
        "Content-Type: text/x-amp-html",
        "",
        "<html amp4email></html>",
        "--cal",
        "Content-Type: text/html",
        "",
        "<p>You are invited.</p>",
        "--cal",
        "Content-Type: text/calendar; charset=utf-8; method=REQUEST",
        "",
        "BEGIN:VCALENDAR",
        "METHOD:REQUEST",
        "END:VCALENDAR",
        "--cal--",
    ]));
    assert_eq!(m.body.text, "You are invited.");
    assert_eq!(m.body.html.as_deref(), Some("<p>You are invited.</p>"));
    assert_eq!(m.attachments.len(), 1);
    assert_eq!((m.attachments[0].name.as_str(), m.attachments[0].mime.as_str()), ("invite.ics", "text/calendar"));
    assert_eq!(m.attachments[0].data.as_slice(), b"BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nEND:VCALENDAR");
}

#[test]
fn an_attached_message_is_not_opened() {
    let m = parse(&eml(&[
        "Subject: Fwd",
        "Content-Type: multipart/mixed; boundary=fw",
        "",
        "--fw",
        "",
        "See below.",
        "--fw",
        "Content-Type: message/rfc822",
        "",
        "Subject: Inner",
        "Content-Type: multipart/mixed; boundary=inner",
        "",
        "--inner",
        "",
        "Inner text",
        "--inner--",
        "--fw--",
    ]));
    assert_eq!(m.subject, "Fwd");
    assert_eq!(m.body.text, "See below.");
    assert_eq!(m.attachments.len(), 1);
    assert_eq!((m.attachments[0].name.as_str(), m.attachments[0].mime.as_str()), ("message.eml", "message/rfc822"));
    assert!(m.attachments[0].data.starts_with(b"Subject: Inner\r\n"));
    assert_eq!(parse(&m.attachments[0].data).body.text, "Inner text");
}

#[test]
fn nested_multiparts() {
    let m = parse(&eml(&[
        "Content-Type: multipart/mixed; boundary=outer",
        "",
        "--outer",
        "Content-Type: multipart/alternative; boundary=alt",
        "",
        "--alt",
        "Content-Type: text/plain",
        "",
        "Plain",
        "--alt",
        "Content-Type: multipart/related; boundary=rel",
        "",
        "--rel",
        "Content-Type: text/html",
        "",
        "<p>Rich</p>",
        "--rel",
        "Content-Type: image/gif",
        "Content-ID: <dot@example.org>",
        "Content-Transfer-Encoding: base64",
        "",
        "R0lGODlh",
        "--rel--",
        "--alt--",
        "--outer",
        "Content-Type: application/zip; name=files.zip",
        "Content-Disposition: attachment",
        "Content-Transfer-Encoding: base64",
        "",
        "UEsDBA==",
        "--outer",
        "Content-Type: text/plain",
        "",
        "-- ",
        "Sent via the example list",
        "--outer--",
    ]));
    assert_eq!(m.body.text, "Plain\n\n-- \nSent via the example list");
    assert_eq!(m.body.html.as_deref(), Some("<p>Rich</p><p>--<br>\nSent via the example list</p>\n"));
    let files: Vec<(&str, &str, bool)> = m.attachments.iter().map(|a| (a.name.as_str(), a.mime.as_str(), a.inline)).collect();
    assert_eq!(files, [("image.gif", "image/gif", true), ("files.zip", "application/zip", false)]);
}

#[test]
fn names_from_every_place() {
    let part = |headers: &[&str]| {
        let mut lines = vec!["Content-Type: multipart/mixed; boundary=b", "", "--b"];
        lines.extend_from_slice(headers);
        lines.extend_from_slice(&["", "data", "--b--"]);
        parse(&eml(&lines)).attachments.first().map(|a| (a.name.clone(), a.mime.clone(), a.inline))
    };
    let got = |name: &str, mime: &str, inline: bool| Some((name.to_string(), mime.to_string(), inline));
    assert_eq!(part(&["Content-Type: application/pdf; name=\"from type.pdf\""]), got("from type.pdf", "application/pdf", false));
    assert_eq!(
        part(&["Content-Type: application/pdf; name=a.pdf", "Content-Disposition: inline; filename=b.pdf"]),
        got("b.pdf", "application/pdf", true)
    );
    assert_eq!(
        part(&["Content-Type: application/octet-stream", "Content-Disposition: attachment; filename=\"=?UTF-8?B?csOpc3Vtw6kucGRm?=\""]),
        got("résumé.pdf", "application/octet-stream", false)
    );
    assert_eq!(part(&["Content-Disposition: attachment; filename=notes.txt"]), got("notes.txt", "text/plain", false));
    assert_eq!(part(&["Content-Type: garbage", "Content-Disposition: attachment; filename=scan.png"]), got("scan.png", "image/png", false));
    assert_eq!(part(&["Content-Type: image/jpeg"]), got("image.jpg", "image/jpeg", false));
    assert_eq!(part(&["Content-Type: application/x-thing"]), got("attachment", "application/x-thing", false));
    assert_eq!(part(&["Content-Type: text/plain", "Content-Disposition: attachment"]), got("attachment.txt", "text/plain", false));
    assert_eq!(part(&["Content-Type: text/html; name=page.html"]), got("page.html", "text/html", false));
    assert_eq!(part(&["Content-Type: text/plain"]), None);
    // A path stays as the sender wrote it; whoever saves the file must not trust it.
    assert_eq!(part(&["Content-Type: application/pdf; name=\"../../x.pdf\""]), got("../../x.pdf", "application/pdf", false));
}

#[test]
fn flags_importance_and_categories() {
    let m =
        parse(&eml(&["Status: RO", "X-Status: AF", "Importance: High", "Keywords: Work, =?UTF-8?Q?Caf=C3=A9?= ,, Travel", "Keywords: More", "", ""]));
    assert!(m.read && m.answered);
    assert_eq!(m.flag, FlagStatus::Flagged);
    assert_eq!(m.importance, Importance::High);
    assert_eq!(m.categories, ["Work", "Café", "Travel", "More"]);
    assert!(m.headers.is_empty());

    let m = parse(&eml(&["Status: O", "X-Status: ", "X-Priority: 5 (Lowest)", "", ""]));
    assert!(!m.read && !m.answered);
    assert_eq!((m.flag, m.importance), (FlagStatus::None, Importance::Low));
    assert_eq!(parse(b"X-Priority: 1\n\n").importance, Importance::High);
    assert_eq!(parse(b"X-Priority: 2\n\n").importance, Importance::High);
    assert_eq!(parse(b"X-Priority: 3\n\n").importance, Importance::Normal);
    assert_eq!(parse(b"X-Priority: 4\n\n").importance, Importance::Low);
    assert_eq!(parse(b"X-Priority: 1\nImportance: low\n\n").importance, Importance::Low);
    assert_eq!(parse(b"Importance: whatever\nX-Priority: 2\n\n").importance, Importance::High);
    assert_eq!(parse(b"X-Status: F\nX-MailCraft-Flag: complete\n\n").flag, FlagStatus::Complete);
}

#[test]
fn message_ids_in_odd_forms() {
    assert_eq!(parse(b"Message-ID: bare@example.org\n\n").message_id, "bare@example.org");
    assert_eq!(parse(b"Message-ID:   < spaced@example.org >  \n\n").message_id, "spaced@example.org");
    assert_eq!(parse(b"In-Reply-To: Your message of Tuesday <a@example.org> (see there)\n\n").in_reply_to, "a@example.org");
    assert_eq!(parse(b"In-Reply-To: your message of Tuesday\n\n").in_reply_to, "");
    assert_eq!(
        parse(b"References: <a@example.org><b@example.org>\n\t<c@example.org> <> <unclosed\n\n").references,
        ["a@example.org", "b@example.org", "c@example.org"]
    );
    assert_eq!(parse(b"References: a@example.org, b@example.org\n\n").references, ["a@example.org", "b@example.org"]);
    let many = format!("References: {}\n\n", "<x@example.org> ".repeat(MAX_IDS + 50));
    assert_eq!(parse(many.as_bytes()).references.len(), MAX_IDS);
}

#[test]
fn dates() {
    assert_eq!(parse(b"Date: 1 Jul 2003 08:52:37 GMT\n\n").date, 1_057_049_557);
    assert_eq!(parse(b"Date: next Tuesday\n\n").date, 0);
    assert_eq!(parse(b"Subject: none\n\n").date, 0);
    assert_eq!(parse(b"Date: 99 Abc 99999 99:99\n\n").date, 0);
}

#[test]
fn kept_headers_are_capped() {
    let many = "X-Extra: value\r\n".repeat(MAX_KEPT_HEADERS + 100);
    assert_eq!(parse(many.as_bytes()).headers.len(), MAX_KEPT_HEADERS);
    let big = format!("X-Big: {}\r\n", "v".repeat(1000)).repeat(100);
    let m = parse(big.as_bytes());
    assert!(m.headers.len() < 100);
    assert!(m.headers.iter().map(|(n, v)| n.len() + v.len()).sum::<usize>() <= MAX_KEPT_BYTES);
    let m = parse(b"Comments: =?UTF-8?Q?caf=C3=A9?=\r\nContent-Language: en\r\nMIME-Version: 1.0\r\n\r\n");
    assert_eq!(m.headers, [("Comments".to_string(), "café".to_string())]);
}

#[test]
fn several_recipient_fields_are_merged() {
    let m = parse(b"To: a@example.org\nTo: b@example.org\nFrom: first@example.org, second@example.org\nFrom: third@example.org\n\n");
    assert_eq!(m.to, [a("", "a@example.org"), a("", "b@example.org")]);
    assert_eq!(m.from, a("", "first@example.org"));
}

#[test]
fn a_truncated_multipart_keeps_what_arrived() {
    let m = parse(&eml(&[
        "Content-Type: multipart/mixed; boundary=cut",
        "",
        "--cut",
        "Content-Type: text/plain",
        "",
        "The text arrived.",
        "--cut",
        "Content-Type: application/pdf; name=half.pdf",
        "Content-Transfer-Encoding: base64",
        "",
        "JVBERi0xLjQKJ",
    ]));
    assert_eq!(m.body.text, "The text arrived.");
    assert_eq!(m.attachments.len(), 1);
    assert_eq!(m.attachments[0].data.as_slice(), b"%PDF-1.4\n");
    // Cut inside the headers of a part.
    let m = parse(&eml(&["Content-Type: multipart/mixed; boundary=cut", "", "--cut", "Content-Type: text/pl"]));
    assert_eq!(m.body.text, "");
}

#[test]
fn a_missing_or_wrong_boundary_shows_the_raw_text() {
    let m = parse(&eml(&["Content-Type: multipart/mixed", "", "--who-knows", "", "Hidden text", "--who-knows--"]));
    assert!(m.body.text.contains("Hidden text"));
    let m = parse(&eml(&["Content-Type: multipart/mixed; boundary=\"\"", "", "text"]));
    assert_eq!(m.body.text, "text");
    // The boundary never appears: there are no parts.
    let m = parse(&eml(&["Content-Type: multipart/mixed; boundary=absent", "", "--other", "", "text", "--other--"]));
    assert_eq!(m.body.text, "");
    assert!(m.attachments.is_empty());
}

#[test]
fn boundaries_that_look_alike() {
    let m = parse(&eml(&[
        "Content-Type: multipart/mixed; boundary=b",
        "",
        "--b",
        "",
        "--bb is not a delimiter",
        "--b-- neither is this",
        "--b  ",
        "",
        "second",
        "--b--  ",
        "--b",
        "",
        "after the end",
    ]));
    assert_eq!(m.body.text, "--bb is not a delimiter\n--b-- neither is this\n\nsecond");
}

#[test]
fn deep_nesting_and_many_parts_are_capped() {
    let mut deep = Vec::new();
    for i in 0..200 {
        deep.extend_from_slice(format!("Content-Type: multipart/mixed; boundary=b{i}\r\n\r\n--b{i}\r\n").as_bytes());
    }
    deep.extend_from_slice(b"Content-Type: text/plain\r\n\r\ndeep text\r\n");
    let m = parse(&deep);
    assert_eq!(m.body.text, "");

    let mut wide = b"Content-Type: multipart/mixed; boundary=w\r\n\r\n".to_vec();
    for _ in 0..(MAX_PARTS + 500) {
        wide.extend_from_slice(b"--w\r\nContent-Type: application/x-a\r\n\r\nx\r\n");
    }
    wide.extend_from_slice(b"--w--\r\n");
    let m = parse(&wide);
    assert_eq!(m.attachments.len(), MAX_PARTS - 1);
}

#[test]
fn garbage_gives_a_mostly_empty_message() {
    assert_eq!(parse(b""), Message::default());
    let m = parse(b"\x00\x01\x02\xff\xfe binary \x80 junk");
    assert!(m.subject.is_empty() && m.to.is_empty() && m.attachments.is_empty());
    assert!(m.body.text.contains("binary"));
    let m = parse(b"no headers at all, just text\r\nsecond line\r\n");
    assert_eq!(m.body.text, "no headers at all, just text\nsecond line");
    let m = parse(b"\r\n\r\nstarts with blank lines");
    assert_eq!(m.body.text, "\nstarts with blank lines");
    let _ = parse(&[b'\n'; 10_000]);
    let _ = parse(&[b':'; 10_000]);
    let _ = parse(b"Content-Type: multipart/mixed; boundary=\r\n\r\n--\r\n----\r\n");
    let _ = parse(b"Content-Type: ;;;=;\"\r\nContent-Disposition: \"\r\nContent-Transfer-Encoding: base64\r\n\r\n====");
}

#[test]
fn a_byte_order_mark_and_raw_8bit_text() {
    let m = parse(b"Content-Type: text/plain; charset=utf-8\n\n\xEF\xBB\xBFHello");
    assert_eq!(m.body.text, "Hello");
    // No charset: UTF-8 when it is valid, else Windows-1252.
    assert_eq!(parse("Subject: x\n\ncafé".as_bytes()).body.text, "café");
    assert_eq!(parse(b"Subject: x\n\ncaf\xE9 \x80").body.text, "café €");
    assert_eq!(parse(b"Content-Type: text/plain; charset=x-unknown\n\ncaf\xE9").body.text, "caf\u{fffd}");
}
