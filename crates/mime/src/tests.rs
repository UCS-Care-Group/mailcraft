// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whole-message tests: what is written reads back, and nothing that is read can panic.

use std::sync::Arc;

use mailcraft_model::{Address, Attachment, Body, FlagStatus, Importance, Message};
use proptest::prelude::*;

use crate::{WriteOptions, parse, parse_mbox, write, write_mbox, written_message_id};

fn a(name: &str, email: &str) -> Address {
    Address { name: name.into(), email: email.into() }
}

fn file(name: &str, mime: &str, data: &[u8]) -> Attachment {
    Attachment { name: name.into(), mime: mime.into(), content_id: String::new(), inline: false, data: Arc::new(data.to_vec()), kept: None }
}

fn picture(name: &str, mime: &str, content_id: &str, data: &[u8]) -> Attachment {
    Attachment { content_id: content_id.into(), inline: true, ..file(name, mime, data) }
}

/// What `parse(write(m))` must give: everything the format carries, the rest at its default.
///
/// The few things that are not carried byte for byte: line ends in the body become `\n`, a byte
/// order mark at the start of a body goes, a blank text next to HTML is made from the HTML, inline
/// attachments come before the others when there is HTML, and Bcc is only there when asked for.
fn expected(m: &Message, opts: &WriteOptions) -> Message {
    let lines = |s: &str| s.replace("\r\n", "\n").replace('\r', "\n").trim_start_matches('\u{feff}').to_string();
    let html = m.body.html.as_deref().map(lines);
    let text = lines(&m.body.text);
    let text = match &html {
        Some(html) if text.trim().is_empty() => mailcraft_html::to_text(html),
        _ => text,
    };
    let mut attachments = m.attachments.clone();
    if html.is_some() {
        attachments.sort_by_key(|a| !a.inline);
    }
    Message {
        message_id: m.message_id.clone(),
        in_reply_to: m.in_reply_to.clone(),
        references: m.references.clone(),
        from: m.from.clone(),
        reply_to: m.reply_to.clone(),
        to: m.to.clone(),
        cc: m.cc.clone(),
        bcc: if opts.include_bcc { m.bcc.clone() } else { Vec::new() },
        subject: m.subject.clone(),
        date: m.date,
        received: m.date,
        body: Body { text, html },
        attachments,
        read: m.read,
        flag: m.flag,
        importance: m.importance,
        categories: m.categories.clone(),
        answered: m.answered,
        headers: m.headers.clone(),
        ..Message::default()
    }
}

/// Write, read back, and compare with what must come back.
fn round_trip(m: &Message) {
    for opts in [WriteOptions::default(), WriteOptions { include_bcc: true, offset_minutes: -330 }] {
        let bytes = write(m, &opts);
        assert_eq!(parse(&bytes), expected(m, &opts), "{}", String::from_utf8_lossy(&bytes));
        assert_eq!(write(m, &opts), bytes, "writing is deterministic");
        check_wire_format(&bytes);
    }
}

/// What every written message must look like on the wire.
fn check_wire_format(bytes: &[u8]) {
    let mut previous = 0u8;
    for &b in bytes {
        assert!(b != b'\n' || previous == b'\r', "a bare LF");
        assert!(previous != b'\r' || b == b'\n', "a bare CR");
        previous = b;
    }
    assert!(bytes.ends_with(b"\r\n"));
    let head_end = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = std::str::from_utf8(&bytes[..head_end]).unwrap();
    for line in head.split("\r\n") {
        assert!(line.len() <= 998, "a header line of {} bytes", line.len());
        assert!(line.starts_with([' ', '\t']) || line.split_once(':').is_some_and(|(name, _)| !name.is_empty() && !name.contains(' ')), "{line}");
    }
    for line in bytes.split(|b| *b == b'\n') {
        assert!(line.len() <= 1000, "a line of {} bytes", line.len());
    }
}

fn base() -> Message {
    Message {
        message_id: "abc.123@example.org".into(),
        from: a("Ada Lovelace", "ada@example.org"),
        to: vec![a("Bob", "bob@example.org")],
        subject: "Lunch on Friday?".into(),
        date: 1_057_049_557,
        body: Body { text: "Hello Bob,\n\nLunch on Friday?\n\nAda".into(), html: None },
        ..Message::default()
    }
}

#[test]
fn a_plain_message_round_trips() {
    round_trip(&base());
    round_trip(&Message::default());
    let mut m = base();
    m.in_reply_to = "parent.1@example.org".into();
    m.references = vec!["root.0@example.org".into(), "parent.1@example.org".into()];
    m.cc = vec![a("Hopper, Grace", "grace@example.org"), a("", "carol@example.org")];
    m.bcc = vec![a("Secret \"S\" Person", "secret@example.org")];
    m.reply_to = vec![a("Replies", "replies@example.org")];
    m.read = true;
    m.answered = true;
    m.flag = FlagStatus::Flagged;
    m.importance = Importance::High;
    m.categories = vec!["Work".into(), "Blue, dark".into(), "Été".into()];
    m.headers = vec![
        ("X-Mailer".into(), "Synthetic 1.0".into()),
        ("List-Id".into(), "Example list <list.example.org>".into()),
        ("Comments".into(), "ünïcode value".into()),
        ("X-Note".into(), "=?not decoded?=".into()),
    ];
    round_trip(&m);
    m.flag = FlagStatus::Complete;
    m.importance = Importance::Low;
    round_trip(&m);
}

#[test]
fn unicode_everywhere_round_trips() {
    let mut m = base();
    m.from = a("Adá Lóvelace 🚀", "ada@example.org");
    m.to = vec![
        a("Ünïcödé Nämé That Goes On And On And On For Quite A While Indeed", "long@example.org"),
        a("日本語の名前", "jp@example.org"),
        a("=?UTF-8?Q?not_a_word?=", "tricky@example.org"),
    ];
    m.subject = "Café ☕ — “quoted” and a very long subject that needs more than one encoded word to fit, 日本語, עברית".into();
    m.body.text = "Grüße aus München 🙋\n\tTabbed line\nTrailing space \n\nFrom the start of a line\n.\n=3D is not an escape here\n".into();
    m.body.html = Some("<p>Grüße aus <b>München</b> 🙋</p>\n<p>=3D &amp; so on</p>".into());
    m.categories = vec!["Très important".into(), " padded ".into()];
    round_trip(&m);
}

#[test]
fn odd_subjects_round_trip() {
    for subject in [
        " leading",
        "trailing ",
        "  ",
        "two  spaces",
        "=?UTF-8?Q?fake?=",
        "tab\there",
        "a?= =?b",
        "\u{feff}bom",
        "x".repeat(2000).as_str(),
        &"word ".repeat(300),
        "\u{1}\u{7f}\u{85}",
    ] {
        let mut m = base();
        m.subject = subject.to_string();
        round_trip(&m);
    }
}

#[test]
fn html_and_plain_round_trip() {
    let mut m = base();
    m.body.html = Some("<html><body><p>Hello <b>Bob</b>,</p>\n<p>Lunch on Friday?</p></body></html>".into());
    round_trip(&m);
    // HTML only: the text is made from it.
    m.body.text = String::new();
    let back = parse(&write(&m, &WriteOptions::default()));
    assert_eq!(back.body.text, "Hello Bob,\n\nLunch on Friday?");
    round_trip(&m);
    m.body.html = Some(String::new());
    round_trip(&m);
}

#[test]
fn an_inline_image_round_trips() {
    let mut m = base();
    m.body.html = Some("<p>Our logo: <img src=\"cid:logo@example.org\" alt=\"Logo\"></p>".into());
    m.attachments = vec![picture("logo.png", "image/png", "logo@example.org", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")];
    let bytes = write(&m, &WriteOptions::default());
    let back = parse(&bytes);
    assert!(back.attachments[0].inline);
    assert!(!back.has_attachments());
    round_trip(&m);
    // Inline first, whatever the order they were added in.
    m.attachments.insert(0, file("notes.txt", "text/plain", b"plain notes\n"));
    m.attachments.push(picture("second.gif", "image/gif", "second@example.org", b"GIF89a"));
    let back = parse(&write(&m, &WriteOptions::default()));
    let names: Vec<&str> = back.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["logo.png", "second.gif", "notes.txt"]);
    round_trip(&m);
    // Without HTML an inline picture is still marked inline, and the order stays.
    m.body.html = None;
    round_trip(&m);
}

#[test]
fn attachments_round_trip() {
    let mut m = base();
    let big: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 256) as u8).collect();
    m.attachments = vec![
        file("report.pdf", "application/pdf", b"%PDF-1.4\n%synthetic\n"),
        file("résumé — «final» €.pdf", "application/pdf", &big),
        file("a \"quoted\" name; with\\specials.txt", "text/plain", b"From the top\r\n--not-a-boundary\r\n"),
        file("=?UTF-8?Q?looks_encoded?=.bin", "application/octet-stream", &[0, 255, 0, 255]),
        file(&"long-name-".repeat(30), "application/zip", b"PK\x03\x04"),
        file(&"ü".repeat(120), "image/jpeg", b"\xff\xd8\xff"),
        file("empty.dat", "application/x-empty", b""),
        file("invite.ics", "text/calendar", b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n"),
        file("page.html", "text/html", b"<p>an attached page</p>"),
        file(" spaced .txt ", "text/plain", b"x"),
    ];
    round_trip(&m);
    m.body.html = Some("<p>See the files.</p>".into());
    round_trip(&m);
}

#[test]
fn an_attached_message_round_trips() {
    let inner = write(
        &Message { subject: "Inner".into(), body: Body { text: "From inside\nline two".into(), html: Some("<p>inside</p>".into()) }, ..base() },
        &WriteOptions::default(),
    );
    let mut m = base();
    m.subject = "Fwd: Inner".into();
    m.attachments = vec![
        file("inner.eml", "message/rfc822", &inner),
        file("unix.eml", "message/rfc822", b"Subject: bare\n\nlf only\n"),
        file("eight.eml", "message/rfc822", "Subject: é\r\n\r\nü".as_bytes()),
    ];
    let bytes = write(&m, &WriteOptions::default());
    let back = parse(&bytes);
    assert_eq!(back.subject, "Fwd: Inner");
    assert_eq!(back.body.text, m.body.text);
    assert_eq!(parse(&back.attachments[0].data).subject, "Inner");
    round_trip(&m);
    // The same through an mbox, where the inner `From ` line is quoted and unquoted.
    let mbox = write_mbox(&[&m], &WriteOptions::default());
    assert_eq!(parse_mbox(&mbox), [expected(&m, &WriteOptions::default())]);
}

#[test]
fn text_that_looks_like_structure_round_trips() {
    let mut m = base();
    m.body.html = Some("<p>x</p>".into());
    let first = String::from_utf8(write(&m, &WriteOptions::default())).unwrap();
    let boundary = first.split("boundary=\"").nth(1).and_then(|s| s.split('"').next()).unwrap().to_string();
    for text in [
        format!("--{boundary}\n--{boundary}--\ntext after"),
        "--\n----\n-- \n--=_mc_\n".to_string(),
        "Content-Type: text/html\n\n<b>not a header</b>".to_string(),
        "\n\nleading blank lines".to_string(),
        "trailing blank lines\n\n\n".to_string(),
        "line\r\nends\rof\nevery kind\r\n".to_string(),
        "\u{feff}a byte order mark".to_string(),
        "x".repeat(5000),
        "nul \0 and other \u{1} controls \u{7f}".to_string(),
    ] {
        m.body.text = text.clone();
        round_trip(&m);
        m.attachments = vec![file("same.txt", "message/rfc822", text.as_bytes())];
        round_trip(&m);
        m.attachments.clear();
        let plain = Message { body: Body { text, html: None }, ..base() };
        round_trip(&plain);
    }
}

#[test]
fn a_foreign_message_survives_reading_writing_and_reading() {
    let eml = [
        "Received: from mail.example.org by mx.example.org; Tue, 1 Jul 2003 10:52:40 +0200",
        "Date: Tue, 1 Jul 2003 10:52:37 +0200",
        "From: =?ISO-8859-1?Q?Andr=E9_Dupont?= <andre@example.org>",
        "To: Team: ada@example.org, \"Hopper, Grace\" <grace@example.org>;",
        "Subject: =?UTF-8?B?UsOpdW5pb24gZGUgbHVuZGk=?=",
        "Message-ID: <m1@example.org>",
        "X-Priority: 1 (Highest)",
        "Keywords: Projet, Urgent",
        "Status: RO",
        "MIME-Version: 1.0",
        "Content-Type: multipart/mixed; boundary=\"outer\"",
        "",
        "--outer",
        "Content-Type: multipart/alternative; boundary=\"alt\"",
        "",
        "--alt",
        "Content-Type: text/plain; charset=iso-8859-1",
        "Content-Transfer-Encoding: quoted-printable",
        "",
        "La r=E9union est =E0 10h.",
        "--alt",
        "Content-Type: text/html; charset=iso-8859-1",
        "Content-Transfer-Encoding: quoted-printable",
        "",
        "<p>La r=E9union est =E0 <b>10h</b>.</p>",
        "--alt--",
        "--outer",
        "Content-Type: application/pdf",
        "Content-Disposition: attachment; filename*=iso-8859-1''ordre%20du%20jour%20%E9t%E9.pdf",
        "Content-Transfer-Encoding: base64",
        "",
        "JVBERi0xLjQK",
        "--outer--",
        "",
    ]
    .join("\r\n");
    let first = parse(eml.as_bytes());
    assert_eq!(first.from, a("André Dupont", "andre@example.org"));
    assert_eq!(first.to, [a("", "ada@example.org"), a("Hopper, Grace", "grace@example.org")]);
    assert_eq!(first.subject, "Réunion de lundi");
    assert_eq!(first.body.text, "La réunion est à 10h.");
    assert_eq!(first.body.html.as_deref(), Some("<p>La réunion est à <b>10h</b>.</p>"));
    assert_eq!(first.attachments[0].name, "ordre du jour été.pdf");
    assert_eq!((first.importance, first.read, first.categories.len(), first.headers.len()), (Importance::High, true, 2, 1));
    let opts = WriteOptions { include_bcc: true, offset_minutes: 120 };
    let second = parse(&write(&first, &opts));
    assert_eq!(second, first);
}

#[test]
fn central_european_mail_reads_without_boxes() {
    // #178: windows-1250 and iso-8859-2 were read as UTF-8, and every letter past ASCII drew as a box.
    let mut eml: Vec<u8> = [
        "From: =?iso-8859-2?Q?=D0ur=F0ica_=AEu=BEi=E6?= <djurdjica@example.org>",
        "To: =?windows-1250?B?SXZhbiBHdm+e8GFu?= <ivan@example.org>",
        "Subject: =?windows-1250?Q?Introduction_=96_we=92ve_met,_Gvo=9E=F0e?=",
        "Date: Tue, 1 Jul 2025 10:52:37 +0200",
        "Message-ID: <cee@example.org>",
        "MIME-Version: 1.0",
        "Content-Type: multipart/mixed; boundary=\"outer\"",
        "",
        "--outer",
        "Content-Type: multipart/alternative; boundary=\"alt\"",
        "",
        "--alt",
        "Content-Type: text/plain; charset=windows-1250",
        "Content-Transfer-Encoding: 8bit",
        "",
        "",
    ]
    .join("\r\n")
    .into_bytes();
    eml.extend_from_slice(b"we\x92ve sent the Gvo\x9E\xF0e \x96 \x8E\x9A\xE8 notes.\r\n");
    eml.extend_from_slice(
        [
            "--alt",
            "Content-Type: text/html; charset=\"ISO-8859-2\"",
            "Content-Transfer-Encoding: quoted-printable",
            "",
            "<p>Gvo=BE=F0e =B9=E8</p>",
            "--alt--",
            "--outer",
            "Content-Type: application/pdf",
            "Content-Disposition: attachment; filename*=iso-8859-2''Gvo%BE%F0e.pdf",
            "Content-Transfer-Encoding: base64",
            "",
            "JVBERi0xLjQK",
            "--outer--",
            "",
        ]
        .join("\r\n")
        .as_bytes(),
    );
    let m = parse(&eml);
    assert_eq!(m.from, a("Đurđica Žužić", "djurdjica@example.org"));
    assert_eq!(m.to, [a("Ivan Gvožđan", "ivan@example.org")]);
    assert_eq!(m.subject, "Introduction – we’ve met, Gvožđe");
    assert_eq!(m.body.text, "we’ve sent the Gvožđe – Žšč notes.");
    assert_eq!(m.body.html.as_deref(), Some("<p>Gvožđe šč</p>"));
    assert_eq!(m.attachments[0].name, "Gvožđe.pdf");
}

#[test]
fn large_and_hostile_input_is_read_quickly() {
    // Megabytes of lines, of header-like lines, of delimiters and of nesting.
    let _ = parse(&b"just a line of text\r\n".repeat(100_000));
    let _ = parse(&b"X-Header: value\r\n".repeat(100_000));
    let _ = parse(&b" continuation\r\n".repeat(100_000));
    let mut m = b"Content-Type: multipart/mixed; boundary=b\r\n\r\n".to_vec();
    m.extend(b"--b\r\n".repeat(200_000));
    let _ = parse(&m);
    let mut m = b"Content-Type: multipart/mixed; boundary=b\r\n\r\n".to_vec();
    m.extend(b"--b\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n".repeat(20_000));
    let _ = parse(&m);
    let mut m = b"To: ".to_vec();
    m.extend(b"a@example.org,".repeat(50_000));
    assert_eq!(parse(&m).to.len(), crate::MAX_RECIPIENTS);
    let mut m = b"Cc: ".to_vec();
    m.extend(b"\"a\" <a@example.org>, (c) ".repeat(50_000));
    assert!(parse(&m).cc.len() > 1_000);
    let mut m = b"Subject: ".to_vec();
    m.extend(b"=?UTF-8?Q?a?= =?x".repeat(50_000));
    let _ = parse(&m);
    let mut m = b"Content-Type: text/html\r\n\r\n".to_vec();
    m.extend(b"<div><table><tr><td><blockquote>".repeat(50_000));
    let _ = parse(&m);
    let mut m = b"Content-Transfer-Encoding: quoted-printable\r\n\r\n".to_vec();
    m.extend(b"= \t".repeat(200_000));
    let _ = parse(&m);
    let _ = parse_mbox(&b"From \r\n".repeat(100_000));
    let _ = parse_mbox(&b">From x 12:34\r\n\r\nFrom y 12:34\r\n".repeat(30_000));
}

// ---- generators ----

/// Text for a header: anything but the three characters the writer removes.
fn header_text() -> impl Strategy<Value = String> {
    prop_oneof![any::<String>(), "[ -~]{0,40}", "[a-c =?,;:<>\"\\\\()@.]{0,24}", "\\PC{0,30}"].prop_map(|s| s.replace(['\r', '\n', '\0'], ""))
}

fn email() -> impl Strategy<Value = String> {
    "[a-z0-9]{1,8}(\\.[a-z0-9]{1,5})?@[a-z0-9]{1,8}\\.example\\.org"
}

fn address() -> impl Strategy<Value = Address> {
    (header_text(), email()).prop_map(|(name, email)| Address::new(name, email))
}

fn addresses() -> impl Strategy<Value = Vec<Address>> {
    proptest::collection::vec(address(), 0..3)
}

fn id() -> impl Strategy<Value = String> {
    "[a-z0-9.$_-]{1,16}@[a-z]{1,8}\\.example\\.org"
}

fn body_text() -> impl Strategy<Value = String> {
    prop_oneof![any::<String>(), "[ -~\n]{0,200}", "(From |--|\\.|=|>| |\t|\r\n|\n|\r|x|é){0,60}", Just(String::new()), "\\PC{0,80}"]
}

fn attachment() -> impl Strategy<Value = Attachment> {
    let name = prop_oneof![header_text(), "[a-z]{1,8}\\.(pdf|txt|png)", "[a-zé \"\\\\;=?*'%]{1,24}"].prop_filter("a name", |n| !n.is_empty());
    let mime = proptest::sample::select(vec![
        "application/pdf",
        "image/png",
        "text/plain",
        "text/html",
        "text/calendar",
        "text/x-thing",
        "message/rfc822",
        "application/octet-stream",
        "application/x-custom+zip",
    ]);
    let content_id = prop_oneof![Just(String::new()), "[a-z0-9.]{1,10}@example\\.org"];
    let data = prop_oneof![
        proptest::collection::vec(any::<u8>(), 0..200),
        "([ -~é]{0,60}\r\n){0,4}[ -~]{0,20}".prop_map(String::into_bytes),
        "(From |--=_mc_|--|\r\n|\n|x){0,30}".prop_map(String::into_bytes),
    ];
    (name, mime, content_id, any::<bool>(), data).prop_map(|(name, mime, content_id, inline, data)| Attachment {
        name,
        mime: mime.to_string(),
        content_id,
        inline,
        data: Arc::new(data),
        kept: None,
    })
}

fn message() -> impl Strategy<Value = Message> {
    let people = (prop_oneof![address(), Just(Address::default())], addresses(), addresses(), addresses(), addresses());
    let ids = (prop_oneof![id(), Just(String::new())], prop_oneof![id(), Just(String::new())], proptest::collection::vec(id(), 0..4));
    let content = (
        body_text(),
        proptest::option::of(prop_oneof![body_text(), "(<p>|</p>|<b>|<br>|[a-z ]{1,8}|&amp;|\n){0,20}"]),
        proptest::collection::vec(attachment(), 0..4),
    );
    let state = (
        any::<bool>(),
        any::<bool>(),
        proptest::sample::select(vec![FlagStatus::None, FlagStatus::Flagged, FlagStatus::Complete]),
        proptest::sample::select(vec![Importance::Low, Importance::Normal, Importance::High]),
        proptest::collection::vec(header_text().prop_filter("a category", |c| !c.is_empty()), 0..3),
        // Unstructured text comes back whatever it is; another field as it was written, which
        // printable ASCII is (`has_words`).
        proptest::collection::vec(
            prop_oneof![("Comments|Organization|Thread-Topic", header_text()), ("X-Test-[A-Za-z0-9]{1,8}", "([!-~]([ -~]{0,38}[!-~])?)?"),],
            0..3,
        ),
    );
    (people, ids, header_text(), -2_000_000_000i64..4_000_000_000i64, content, state).prop_map(
        |(
            (from, to, cc, bcc, reply_to),
            (message_id, in_reply_to, references),
            subject,
            date,
            (text, html, attachments),
            (read, answered, flag, importance, categories, headers),
        )| Message {
            message_id,
            in_reply_to,
            references,
            from,
            reply_to,
            to,
            cc,
            bcc,
            subject,
            date,
            // Not carried by the format: must not come back.
            received: date.wrapping_add(99),
            body: Body { text, html },
            hidden_text: String::new(),
            attachments,
            read,
            flag,
            importance,
            categories,
            answered,
            headers,
            id: 7,
            account: 3,
            folder: 5,
            uid: 11,
            draft: true,
            pinned: true,
            warnings_dismissed: vec!["external".into()],
            forwarded: true,
            flag_due: Some(1),
            synced: Some(mailcraft_model::sync::RemoteFlags { uid: 11, read: true, ..Default::default() }),
            not_downloaded: 62_914_560,
            server: Some(mailcraft_model::sync::ServerMessage { id: "AAMkAGI2-not-carried".into(), ..Default::default() }),
            unfetched: false,
            attachments_on_server: false,
            moving_out: None,
            returned: 0,
            placed: String::new(),
            incomplete: false,
            server_draft: None,
            held: String::new(),
            answers: None,
            kept: None,
        },
    )
}

fn options() -> impl Strategy<Value = WriteOptions> {
    (any::<bool>(), -720i32..=840).prop_map(|(include_bcc, offset_minutes)| WriteOptions { include_bcc, offset_minutes })
}

/// Damage bytes: overwrite, insert, delete, cut, or repeat a stretch.
fn mutate(mut bytes: Vec<u8>, edits: &[(usize, u8, u8)]) -> Vec<u8> {
    for &(at, kind, value) in edits {
        if bytes.is_empty() {
            break;
        }
        let i = at % bytes.len();
        match kind % 5 {
            0 => bytes[i] = value,
            1 => bytes.insert(i, value),
            2 => {
                bytes.remove(i);
            }
            3 => bytes.truncate(i),
            _ => {
                let chunk = bytes[i..(i + usize::from(value)).min(bytes.len())].to_vec();
                bytes.splice(i..i, chunk);
            }
        }
    }
    bytes
}

proptest! {
    #[test]
    fn any_message_reads_back_as_written(m in message(), opts in options()) {
        let bytes = write(&m, &opts);
        let back = parse(&bytes);
        let want = expected(&m, &opts);
        // The fields one by one first, for a readable failure.
        prop_assert_eq!(&back.from, &want.from);
        prop_assert_eq!(&back.to, &want.to);
        prop_assert_eq!(&back.cc, &want.cc);
        prop_assert_eq!(&back.bcc, &want.bcc);
        prop_assert_eq!(&back.reply_to, &want.reply_to);
        prop_assert_eq!(&back.subject, &want.subject);
        prop_assert_eq!(back.date, want.date);
        prop_assert_eq!(&back.message_id, &want.message_id);
        prop_assert_eq!(&back.in_reply_to, &want.in_reply_to);
        prop_assert_eq!(&back.references, &want.references);
        prop_assert_eq!(&back.body.text, &want.body.text);
        prop_assert_eq!(&back.body.html, &want.body.html);
        prop_assert_eq!(back.attachments.len(), want.attachments.len());
        for (got, want) in back.attachments.iter().zip(&want.attachments) {
            prop_assert_eq!((&got.name, &got.mime, &got.content_id, got.inline), (&want.name, &want.mime, &want.content_id, want.inline));
            prop_assert_eq!(&got.data, &want.data);
        }
        prop_assert_eq!((back.importance, back.read, back.flag, back.answered), (want.importance, want.read, want.flag, want.answered));
        prop_assert_eq!(&back.categories, &want.categories);
        prop_assert_eq!(&back.headers, &want.headers);
        prop_assert_eq!(back, want);
    }

    #[test]
    fn writing_is_deterministic_and_well_formed(m in message(), opts in options()) {
        let bytes = write(&m, &opts);
        prop_assert_eq!(&write(&m, &opts), &bytes);
        check_wire_format(&bytes);
    }

    #[test]
    fn written_boundaries_never_occur_in_a_part(m in message()) {
        let bytes = write(&m, &WriteOptions::default());
        let text = String::from_utf8_lossy(&bytes);
        for boundary in text.split("boundary=\"").skip(1).filter_map(|s| s.split('"').next()).filter(|b| b.starts_with("=_mc_")) {
            // Every occurrence is the parameter itself or a delimiter line.
            for (at, _) in text.match_indices(boundary) {
                let before = text.get(..at).unwrap_or("");
                prop_assert!(before.ends_with("boundary=\"") || before.ends_with("\r\n--"), "{boundary} inside a part");
            }
        }
    }

    #[test]
    fn an_mbox_reads_back_as_written(messages in proptest::collection::vec(message(), 0..4), opts in options()) {
        let refs: Vec<&Message> = messages.iter().collect();
        let mbox = write_mbox(&refs, &opts);
        let want: Vec<Message> = messages.iter().map(|m| expected(m, &opts)).collect();
        prop_assert_eq!(parse_mbox(&mbox), want);
    }

    #[test]
    fn any_bytes_parse(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let m = parse(&data);
        prop_assert_eq!((m.id, m.account, m.folder, m.uid), (0, 0, 0, 0));
        let _ = parse_mbox(&data);
    }

    #[test]
    fn a_damaged_message_parses(m in message(), edits in proptest::collection::vec((any::<usize>(), any::<u8>(), any::<u8>()), 1..8)) {
        let bytes = mutate(write(&m, &WriteOptions { include_bcc: true, offset_minutes: 0 }), &edits);
        let back = parse(&bytes);
        // What comes back can be written and read again.
        let _ = parse(&write(&back, &WriteOptions::default()));
        let _ = parse_mbox(&bytes);
    }

    #[test]
    fn mime_soup_parses(s in "(Content-Type: |multipart/mixed|text/html|text/plain|message/rfc822|; boundary=|; charset=|; name=|b|\"|--b|--b--|Content-Transfer-Encoding: |base64|quoted-printable|Content-Disposition: |attachment|inline|; filename\\*0\\*=|utf-8''|%E9|=E9|=\r\n|\r\n|\n| |\t|:|<|>|@|=\\?UTF-8\\?B\\?|\\?=|x|é){0,80}") {
        let m = parse(s.as_bytes());
        let _ = parse(&write(&m, &WriteOptions::default()));
    }

    #[test]
    fn what_was_read_is_stable_once_written(data in proptest::collection::vec(any::<u8>(), 0..600)) {
        // Reading garbage gives some message; writing that message and reading it gives the same one.
        let first = parse(&data);
        let opts = WriteOptions { include_bcc: true, offset_minutes: 0 };
        let second = parse(&write(&first, &opts));
        let third = parse(&write(&second, &opts));
        prop_assert_eq!(third, second);
    }
}

// ---- caps on lists and header bytes ----

#[test]
fn recipients_and_categories_are_capped_across_the_message() {
    let line = format!("To: {}\r\n", (0..10_000).map(|i| format!("u{i}@example.org")).collect::<Vec<_>>().join(", "));
    let keywords = format!("Keywords: {}\r\n", (0..10_000).map(|i| format!("k{i}")).collect::<Vec<_>>().join(","));
    let mut raw = String::new();
    for _ in 0..10 {
        raw.push_str(&line);
        raw.push_str(&keywords);
    }
    raw.push_str("Cc: c@example.org\r\nReply-To: ");
    raw.push_str(&(0..5000).map(|i| format!("r{i}@example.org")).collect::<Vec<_>>().join(", "));
    raw.push_str("\r\n\r\nbody\r\n");
    let t = std::time::Instant::now();
    let m = parse(raw.as_bytes());
    assert!(t.elapsed().as_secs() < 1, "{:?}", t.elapsed());
    assert_eq!(m.to.len() + m.cc.len() + m.bcc.len(), crate::MAX_RECIPIENTS);
    assert_eq!(m.categories.len(), crate::MAX_CATEGORIES);
    assert_eq!(m.reply_to.len(), crate::MAX_RECIPIENTS);
}

#[test]
fn a_header_block_keeps_a_bounded_number_of_bytes() {
    let mut raw = Vec::new();
    for i in 0..100 {
        raw.extend_from_slice(format!("X-F{i}: ").as_bytes());
        raw.extend_from_slice(&b"a".repeat(100_000));
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"\r\nbody");
    let (headers, body) = crate::header::split(&raw, true);
    assert!(headers.0.iter().map(|f| f.name.len() + f.value.len()).sum::<usize>() <= 4 * 1024 * 1024);
    assert!(!headers.0.is_empty() && headers.0.len() < 100);
    assert_eq!(body, b"body");
}

#[test]
fn an_mbox_is_read_one_message_at_a_time() {
    let mbox = b"From a@example.org Tue Jul  1 08:52:37 2003\nSubject: one\n\nx\n\nFrom b@example.org Tue Jul  1 08:52:37 2003\nSubject: two\n\ny\n"
        .repeat(10);
    let mut it = crate::mbox_messages(&mbox);
    assert_eq!(it.next().map(|m| m.subject), Some("one".into()));
    assert_eq!(crate::mbox_messages(&mbox).take(5).count(), 5);
    assert_eq!(crate::mbox_messages(&mbox).count(), parse_mbox(&mbox).len());
}

proptest! {
    #[test]
    fn list_headers_stay_under_the_caps(lines in proptest::collection::vec(("(To|Cc|Bcc|Reply-To|Keywords)", "[a-z@.,; <>]{0,3000}"), 0..60)) {
        let mut raw = String::new();
        for (name, value) in &lines {
            raw.push_str(&format!("{name}: {value}\r\n"));
        }
        raw.push_str("\r\nbody");
        let m = parse(raw.as_bytes());
        prop_assert!(m.to.len() + m.cc.len() + m.bcc.len() <= crate::MAX_RECIPIENTS);
        prop_assert!(m.reply_to.len() <= crate::MAX_RECIPIENTS);
        prop_assert!(m.categories.len() <= crate::MAX_CATEGORIES);
    }
}

#[test]
fn the_message_id_said_to_be_written_is_the_one_written() {
    for raw in ["abc.123@example.org", "<abc@example.org>", "id@example.org>\r\nX-Evil: 1\r\n<x", "< >", ""] {
        let m = Message { message_id: raw.into(), ..base() };
        let out = String::from_utf8(write(&m, &WriteOptions::default())).unwrap();
        let field = out.lines().find_map(|line| line.strip_prefix("Message-ID: ")).map(str::to_string);
        assert_eq!(written_message_id(raw), field, "{raw:?}");
    }
}
