// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! AU3-15: a part is "shown in place", and left out of the attachment list, its filters and a
//! reply, only when an `<img>` of the body asks for it by `cid:` and it is a picture by its own
//! bytes. A `cid:` written anywhere else, or a file of another kind, is listed: from a message
//! read off the wire and from a `.pst` file alike. (While the body is kept outside memory:
//! `store_hidden_tests.rs`.)
//!
//! The people, hosts and files are invented.

use mailcraft_engine::{Session, sample};
use mailcraft_model::search::Query;
use mailcraft_model::{Condition, Message};
use mailcraft_pst::test_writer::{self as w, AttachSpec, FileSpec, FolderSpec, Layout, MessageSpec};
use mailcraft_pst::{Encryption, Format};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

/// A PNG's signature and the start of its header: a picture by its bytes. (Base64 for the wire.)
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUg==";

fn raw(html: &str, part_type: &str, b64: &str) -> Vec<u8> {
    format!(
        "From: Sender <sender@a.example>\r\nReply-To: Other <other@b.example>\r\nTo: Avery Quinn <avery@lumen.example>\r\nSubject: cid\r\n\
         Message-ID: <cid@a.example>\r\nDate: Wed, 14 Oct 2026 15:00:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/related; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n\
         --B\r\nContent-Type: {part_type}\r\nContent-Disposition: attachment; filename=\"tool.zip\"\r\nContent-ID: <z9@a.example>\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{b64}\r\n--B--\r\n"
    )
    .into_bytes()
}

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

/// Whether `m` has its file listed: in the list, `has:attachment` and a rule's "has an attachment".
fn listed(m: &Message) -> bool {
    let all = [
        m.listed_attachments().count() == 1,
        m.has_attachments(),
        Query::parse("has:attachment").matches(m, 0),
        Condition::HasAttachment.matches_as(m, false),
    ];
    assert!(all.iter().all(|x| *x == all[0]), "{all:?}");
    all[0]
}

/// The files a reply to `m` carries with its quote, and the ones the sent reply has.
fn reply_carries(s: &mut Session, m: Message) -> (Vec<String>, Vec<String>) {
    let mut m = m;
    m.folder = s.mailbox.inbox().unwrap();
    m.account = s.mailbox.accounts[0].id;
    let id = s.mailbox.add_message(m).unwrap();
    s.run("mail.reply", &json!({"id": id})).unwrap();
    let c = s.compose.clone().unwrap();
    let pictures = c.quote.as_ref().map(|q| q.pictures.iter().map(|a| a.name.clone()).collect()).unwrap_or_default();
    let sent = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let files = s.mailbox.message(sent).unwrap().attachments.iter().map(|a| a.name.clone()).collect();
    (pictures, files)
}

#[test]
fn a_file_that_is_no_picture_is_listed_whatever_the_body_says_of_it() {
    const ZIP: &str = "UEsDBAo=";
    for (label, html) in [
        ("a comment", "<p>Hello</p><!-- cid:z9@a.example -->"),
        ("text", "<p>Hello</p><p style=\"color:#ffffff\">cid:z9@a.example</p>"),
        ("a link", "<p><a href=\"cid:z9@a.example\">Hello</a></p>"),
        ("an img", "<p>Hello <img src=\"cid:z9@a.example\"></p>"),
        ("nothing", "<p>Hello</p>"),
    ] {
        for part_type in ["application/zip", "image/png"] {
            let m = mailcraft_mime::parse(&raw(html, part_type, ZIP));
            assert!(listed(&m), "{label}, {part_type}");
            let (pictures, sent) = reply_carries(&mut session(), m);
            assert!(pictures.is_empty() && sent.is_empty(), "{label}, {part_type}: {pictures:?} {sent:?}");
        }
    }
}

#[test]
fn a_picture_an_img_asks_for_is_shown_in_place_and_goes_with_a_reply() {
    for html in ["<p>Hello <img src=\"cid:z9@a.example\"></p>", "<p><IMG alt=x SRC='CID:z9@a.example'></p>", "<img src=cid:z9%40a.example>"] {
        let m = mailcraft_mime::parse(&raw(html, "image/png", PNG_B64));
        assert!(!listed(&m), "{html}");
        let (pictures, sent) = reply_carries(&mut session(), m);
        assert_eq!((pictures.len(), sent.len()), (1, 1), "{html}");
    }
    // Only mentioned, it is listed like any other file.
    let m = mailcraft_mime::parse(&raw("<p>cid:z9@a.example</p>", "image/png", PNG_B64));
    assert!(listed(&m));
}

fn pst_item(subject: &str, html: &str, name: &str, mime: &str, data: &[u8], hidden: bool) -> MessageSpec {
    let mut props = vec![
        (0x3705, w::int32(1)),
        (0x3707, w::unicode(name)),
        (0x370E, w::unicode(mime)),
        (0x3712, w::unicode("<t9@a.example>")),
        (0x3701, w::binary(data)),
    ];
    if hidden {
        props.push((0x7FFE, w::boolean(true)));
    }
    MessageSpec {
        class: "IPM.Note".into(),
        subject: subject.into(),
        body: "Hi".into(),
        extra: vec![(0x1013, w::binary(html.as_bytes()))],
        attachments: vec![AttachSpec::Props(props)],
        ..Default::default()
    }
}

#[test]
fn a_pst_files_inline_part_is_a_picture_an_img_asks_for() {
    let comment = "<html><body><p>Hi</p><!-- cid:t9@a.example --></body></html>";
    let img = "<html><body><p>Hi <img src=\"cid:t9@a.example\"></p></body></html>";
    let items = vec![
        pst_item("zip mentioned", comment, "tool.zip", "application/zip", b"PK\x03\x04v27", false),
        pst_item("zip asked for", img, "tool.zip", "application/zip", b"PK\x03\x04v27", false),
        pst_item("zip hidden", img, "tool.zip", "image/png", b"PK\x03\x04v27", true),
        pst_item("picture mentioned", comment, "logo.png", "image/png", PNG, false),
        pst_item("picture asked for", img, "logo.png", "image/png", PNG, false),
    ];
    let file = FileSpec {
        layout: Layout::new(Format::Unicode, Encryption::Permute),
        password: None,
        password_ansi: false,
        name: "Personal Folders".into(),
        folders: vec![FolderSpec { name: "Inbox".into(), class: Some("IPF.Note".into()), messages: items, children: Vec::new() }],
        named: Vec::new(),
        named_strings: Vec::new(),
    };
    let mut s = session();
    let data = mailcraft_engine::cmd::base64_encode(&w::build(&file));
    s.run("file.importPst", &json!({"name": "old-mail.pst", "data": data})).unwrap();
    let found = |subject: &str| s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| Message::clone(m)).unwrap();
    // No file of another kind is inline, whatever the body or the file says.
    for subject in ["zip mentioned", "zip asked for", "zip hidden"] {
        let m = found(subject);
        assert!(!m.attachments[0].inline && listed(&m), "{subject}");
    }
    // A picture only mentioned is listed; the one an <img> asks for is shown in place.
    let mentioned = found("picture mentioned");
    assert!(!mentioned.attachments[0].inline && listed(&mentioned));
    let shown = found("picture asked for");
    assert!(shown.attachments[0].inline && !listed(&shown));
}
