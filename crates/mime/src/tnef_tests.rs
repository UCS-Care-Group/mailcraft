// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages with a TNEF stream (`winmail.dat`): its files come out as attachments, before it.

use mailcraft_charset::base64::STANDARD;
use mailcraft_tnef::test_writer::{Prop, Writer};

use crate::parse;

/// A message with a text part and `stream` attached the way Exchange attaches it.
fn message(text: Option<&str>, content_type: &str, name: &str, stream: &[u8]) -> Vec<u8> {
    let mut m = String::from(
        "From: Ada <ada@example.org>\r\nTo: Bob <bob@example.org>\r\nSubject: Figures\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n",
    );
    if let Some(text) = text {
        m.push_str(&format!("--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n"));
    }
    m.push_str(&format!(
        "--b\r\nContent-Type: {content_type}; name=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"{name}\"\r\n\r\n{}\r\n--b--\r\n",
        STANDARD.encode(stream)
    ));
    m.into_bytes()
}

fn names(m: &crate::Message) -> Vec<&str> {
    m.attachments.iter().map(|a| a.name.as_str()).collect()
}

#[test]
fn the_files_inside_come_before_the_stream() {
    let stream = Writer::new()
        .file("Budget 2026.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", b"PK made up")
        .legacy_file(b"NOTES.TXT", b"notes")
        .finish();
    let m = parse(&message(Some("See the figures."), "application/ms-tnef", "winmail.dat", &stream));
    assert_eq!(names(&m), ["Budget 2026.xlsx", "NOTES.TXT", "winmail.dat"]);
    assert_eq!(m.attachments[0].mime, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
    assert_eq!(*m.attachments[0].data, b"PK made up");
    // No type given: by the name.
    assert_eq!(m.attachments[1].mime, "text/plain");
    assert_eq!(*m.attachments[2].data, stream, "the stream itself is kept whole");
    assert_eq!(m.body.text.trim(), "See the figures.");
}

#[test]
fn the_stream_is_known_by_its_name_or_either_type() {
    let stream = Writer::new().file("a.txt", "text/plain", b"a").finish();
    for (content_type, name) in
        [("application/octet-stream", "WINMAIL.DAT"), ("application/vnd.ms-tnef", "att00001.dat"), ("application/ms-tnef", "x")]
    {
        let m = parse(&message(Some("t"), content_type, name, &stream));
        assert_eq!(m.attachments.len(), 2, "{content_type} {name}");
        assert_eq!(m.attachments[0].name, "a.txt");
    }
    // A file of another name and type is never looked into, whatever its bytes.
    let m = parse(&message(Some("t"), "application/octet-stream", "data.bin", &stream));
    assert_eq!(names(&m), ["data.bin"]);
}

#[test]
fn a_winmail_dat_that_is_not_tnef_stays_as_it_is() {
    let m = parse(&message(Some("t"), "application/ms-tnef", "winmail.dat", b"not a TNEF stream at all"));
    assert_eq!(names(&m), ["winmail.dat"]);
}

#[test]
fn its_text_is_the_body_only_when_the_message_has_none() {
    let stream =
        Writer::new().message_props(&[Prop::Unicode(0x1000, "Text from inside\r\nline two".into())]).file("a.txt", "text/plain", b"a").finish();
    let m = parse(&message(None, "application/ms-tnef", "winmail.dat", &stream));
    assert_eq!(m.body.text, "Text from inside\nline two");
    let m = parse(&message(Some("The message's own text."), "application/ms-tnef", "winmail.dat", &stream));
    assert_eq!(m.body.text.trim(), "The message's own text.");
}

#[test]
fn inline_picture_does_not_suppress_tnef_plain_fallback() {
    let stream = Writer::new().message_props(&[Prop::Unicode(0x1000, "Caption".into())]).finish();
    let picture = "--b\r\nContent-Type: image/png\r\nContent-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n";
    for picture_first in [true, false] {
        for source in [None, Some("Source plain"), Some(" \t")] {
            let original = String::from_utf8(message(source, "application/ms-tnef", "winmail.dat", &stream)).unwrap();
            let wire = if picture_first {
                original.replacen("--b\r\n", &format!("{picture}--b\r\n"), 1)
            } else {
                original.replace("--b--", &format!("{picture}--b--"))
            };
            let message = parse(wire.as_bytes());
            let own_text = source.filter(|text| !text.trim().is_empty());
            let expected = own_text.unwrap_or("Caption");
            assert_eq!(message.body.text, expected);
            // Without synthesized HTML, the pre-feature reader displayed the plain body.
            let visible = message.body.html.as_deref().map(mailcraft_html::to_text).unwrap_or_else(|| message.body.text.clone());
            assert!(visible.contains(expected), "{visible}");
            if let Some(html) = &message.body.html {
                assert_eq!(html.matches(expected).count(), 1, "{html}");
                if own_text.is_some() {
                    assert!(!html.contains("Caption"), "{html}");
                }
                assert_eq!(mailcraft_html::images(&mailcraft_html::parse(html)).len(), 1);
            }
            assert!(message.attachments.iter().any(|part| part.data.as_slice() == stream.as_slice()));
        }
    }
    let wire = String::from_utf8(message(None, "application/ms-tnef", "winmail.dat", &stream)).unwrap();
    let source = "--b\r\nContent-Type: text/html\r\n\r\n<p>Source HTML</p>\r\n";
    let wire = wire.replace("--b--", &format!("{picture}{source}--b--"));
    let message = parse(wire.as_bytes());
    assert!(message.body.text.contains("Source HTML"));
    assert!(!message.body.text.contains("Caption"));
    assert!(!message.body.html.as_deref().unwrap().contains("Caption"));
}

#[test]
fn a_streams_file_keeps_no_content_id_and_is_not_inline() {
    let stream = Writer::new()
        .attachment(
            1,
            &[
                Prop::Unicode(0x3707, "image001.png".into()),
                Prop::Unicode(0x3712, "image001.png@01DA".into()),
                Prop::Binary(0x3701, b"\x89PNG".to_vec()),
            ],
        )
        .finish();
    let m = parse(&message(Some("t"), "application/ms-tnef", "winmail.dat", &stream));
    assert_eq!(m.attachments[0].name, "image001.png");
    assert_eq!(m.attachments[0].content_id, "");
    assert_eq!(m.attachments[0].mime, "image/png");
    assert!(!m.attachments[0].inline);
}

#[test]
fn a_streams_content_id_never_takes_the_place_of_the_messages_picture() {
    // The message's own picture comes after the stream with the content id the stream's file
    // also gives: the message's picture is the only attachment with it, so `cid:logo@example.org`
    // can only be the message's.
    let stream = Writer::new()
        .attachment(
            1,
            &[
                Prop::Unicode(0x3707, "other.png".into()),
                Prop::Unicode(0x3712, "logo@example.org".into()),
                Prop::Binary(0x3701, b"stream's".to_vec()),
            ],
        )
        .finish();
    let mut m = message(Some("t"), "application/ms-tnef", "winmail.dat", &stream);
    let end = m.len() - "--b--\r\n".len();
    let picture =
        "--b\r\nContent-Type: image/png; name=\"logo.png\"\r\nContent-ID: <logo@example.org>\r\nContent-Disposition: inline\r\n\r\nmessage's\r\n";
    m.splice(end..end, picture.bytes());
    let m = parse(&m);
    let with: Vec<_> = m.attachments.iter().filter(|a| a.content_id == "logo@example.org").collect();
    assert_eq!(with.len(), 1);
    assert_eq!(with[0].name, "logo.png");
    assert_eq!(*with[0].data, b"message's");
}

#[test]
fn a_hostile_media_type_or_no_name_is_made_safe() {
    let stream = Writer::new()
        .attachment(1, &[Prop::Unicode(0x370E, "multipart/mixed; boundary=x".into()), Prop::Binary(0x3701, b"x".to_vec())])
        .attachment(1, &[Prop::Unicode(0x370E, "image/png".into()), Prop::Binary(0x3701, b"x".to_vec())])
        .finish();
    let m = parse(&message(Some("t"), "application/ms-tnef", "winmail.dat", &stream));
    assert_eq!(m.attachments[0].mime, "application/octet-stream");
    assert_eq!(m.attachments[0].name, "attachment");
    assert_eq!(m.attachments[1].name, "image.png");
}

#[test]
fn only_the_first_stream_is_opened() {
    let first = Writer::new().file("first.txt", "text/plain", b"1").finish();
    let second = Writer::new().file("second.txt", "text/plain", b"2").finish();
    let mut m = message(Some("t"), "application/ms-tnef", "winmail.dat", &first);
    // A second stream before the closing boundary.
    let end = m.len() - "--b--\r\n".len();
    let extra = format!(
        "--b\r\nContent-Type: application/ms-tnef; name=\"winmail.dat\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        STANDARD.encode(&second)
    );
    m.splice(end..end, extra.into_bytes());
    let m = parse(&m);
    assert_eq!(names(&m), ["first.txt", "winmail.dat", "winmail.dat"]);
}

#[test]
fn a_stream_that_cant_be_read_doesnt_keep_the_next_shut() {
    // The right signature, a version a reader may not take: it stays a file, and the good
    // stream after it is still opened.
    let mut broken = Writer::new().file("never.txt", "text/plain", b"x").finish();
    broken[17] = 2;
    broken[19] = 2;
    let good = Writer::new().file("good.txt", "text/plain", b"g").finish();
    let mut m = message(Some("t"), "application/ms-tnef", "winmail.dat", &broken);
    let end = m.len() - "--b--\r\n".len();
    let extra = format!(
        "--b\r\nContent-Type: application/ms-tnef; name=\"winmail.dat\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        STANDARD.encode(&good)
    );
    m.splice(end..end, extra.into_bytes());
    let m = parse(&m);
    assert_eq!(names(&m), ["winmail.dat", "good.txt", "winmail.dat"]);
}

#[test]
fn a_streams_files_dont_use_up_the_messages_own_parts() {
    // The root, 997 small files and a stream are 999 parts; the message's own text is the
    // 1,000th, the last a message may have read. The stream's file doesn't take its place, so
    // the text is the message's own and not the stream's.
    let stream = Writer::new().message_props(&[Prop::Unicode(0x1000, "Text from inside".into())]).file("inside.txt", "text/plain", b"i").finish();
    let mut m =
        String::from("From: Ada <ada@example.org>\r\nSubject: Many\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n");
    for i in 0..997 {
        m.push_str(&format!("--b\r\nContent-Type: application/octet-stream; name=\"f{i}.bin\"\r\n\r\nx\r\n"));
    }
    m.push_str(&format!(
        "--b\r\nContent-Type: application/ms-tnef; name=\"winmail.dat\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        STANDARD.encode(&stream)
    ));
    m.push_str("--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe message's own text.\r\n--b--\r\n");
    let m = parse(m.as_bytes());
    assert_eq!(m.body.text.trim(), "The message's own text.");
    assert!(m.attachments.iter().any(|a| a.name == "inside.txt"));
    assert_eq!(m.attachments.len(), 997 + 2);
}

#[test]
fn an_attached_message_inside_is_not_opened() {
    let inner = Writer::new().file("deep.txt", "text/plain", b"deep").finish();
    let stream =
        Writer::new().attachment(1, &[Prop::Int32(0x3705, 5), Prop::Object(0x3701, mailcraft_tnef::test_writer::IID_MESSAGE, inner)]).finish();
    let m = parse(&message(Some("t"), "application/ms-tnef", "winmail.dat", &stream));
    assert_eq!(names(&m), ["winmail.dat"]);
}
