// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A signature picture the HTML draws, sent as an attachment, stays out of the attachment list
//! with its body on disk, after the mailbox is opened again, and in a mailbox opened in place of
//! another (#1970). Invented people only.

use mailcraft_model::search::Query;

use super::*;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const CID: &str = "image001.png@01DB0000.0000AAAA";

/// A message with a long HTML body that draws its one picture, `picture` of type `kind`, sent
/// as an attachment.
fn signed(picture: &[u8], kind: &str) -> Message {
    let html = format!("<p>{}</p><img src=\"cid:{CID}\">", "Regards. ".repeat(400));
    assert!(html.len() > INLINE_MAX);
    let b64 = crate::cmd::base64_encode(picture);
    let raw = format!(
        "From: Sender <sender@a.example>\r\nTo: Avery <avery@example.org>\r\nSubject: signature\r\n\
         Message-ID: <sig@a.example>\r\nDate: Wed, 14 Oct 2026 15:00:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n\
         --B\r\nContent-Type: {kind}; name=\"image001.png\"\r\nContent-Disposition: attachment; filename=\"image001.png\"\r\n\
         Content-ID: <{CID}>\r\nContent-Transfer-Encoding: base64\r\n\r\n{b64}\r\n--B--\r\n"
    );
    mailcraft_mime_model::parse(raw.as_bytes())
}

fn not_listed_anywhere(m: &Message) -> bool {
    m.listed_attachments().next().is_none()
        && !m.has_attachments()
        && !Query::parse("has:attachment").matches(m, 0)
        && !mailcraft_model::Condition::HasAttachment.matches(m)
}

#[test]
fn a_drawn_signature_picture_has_no_paperclip_with_its_body_on_disk() {
    // A small picture, and one large enough to go to disk itself, said to be of no kind.
    let large: Vec<u8> = PNG.iter().copied().chain(std::iter::repeat_n(0u8, 2 * INLINE_MAX)).collect();
    for (picture, kind) in [(PNG.to_vec(), "image/png"), (large, "application/octet-stream")] {
        let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
        let mut m = signed(&picture, kind);
        m.account = mb.accounts[0].id;
        m.folder = mb.inbox().unwrap();
        let id = mb.add_message(m).unwrap();
        let tmp = Temp::new("drawn-outside");
        let path = tmp.file();
        save(&path, &mb).unwrap();
        clear_cache();
        let s = session_without_bodies(&path);
        let m = s.mailbox.message(id).unwrap();
        assert!(m.body_is_kept(), "{kind}");
        assert!(not_listed_anywhere(m), "{kind}");
    }
}

/// A session on a mailbox with one message read in before its pictures were told: its id.
fn unmarked() -> (Session, Id) {
    let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
    let mut m = signed(PNG, "image/png");
    m.attachments[0].inline = false;
    m.account = mb.accounts[0].id;
    m.folder = mb.inbox().unwrap();
    let id = mb.add_message(m).unwrap();
    (Session::new(mb), id)
}

fn read(s: &mut Session, id: Id) {
    s.read_at(id, crate::ReadIn::Pane, mailcraft_html::Env::default(), false);
}

#[test]
fn an_undo_that_takes_the_mark_away_lets_it_be_told_again() {
    let (mut s, id) = unmarked();
    s.run("mail.flag", &serde_json::json!({"id": id})).unwrap();
    read(&mut s, id);
    assert!(not_listed_anywhere(s.mailbox.message(id).unwrap()));
    // The flag undone puts back the message as it was before it, unmarked.
    s.run("edit.undo", &serde_json::json!({})).unwrap();
    read(&mut s, id);
    assert!(not_listed_anywhere(s.mailbox.message(id).unwrap()));
}

#[test]
fn telling_a_messages_pictures_keeps_the_list_as_it_was_built() {
    let (mut s, id) = unmarked();
    let rows = s.rows();
    assert!(rows.iter().any(|r| r.members.contains(&id)));
    read(&mut s, id);
    assert!(not_listed_anywhere(s.mailbox.message(id).unwrap()));
    assert!(s.rows_in_view().is_some_and(|now| Arc::ptr_eq(&now, &rows)));
}

#[test]
fn a_mailbox_opened_in_place_of_another_has_its_pictures_told_when_first_read() {
    let mailbox = |unmarked: bool| {
        let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
        let mut m = signed(PNG, "image/png");
        m.attachments[0].inline = !unmarked;
        m.account = mb.accounts[0].id;
        m.folder = mb.inbox().unwrap();
        let id = mb.add_message(m).unwrap();
        (mb, id)
    };
    let (first, id) = mailbox(false);
    let mut s = Session::new(first);
    s.read_at(id, crate::ReadIn::Pane, mailcraft_html::Env::default(), false);
    // Another mailbox, read in before its pictures were told, with a message of the same id.
    let (second, same) = mailbox(true);
    assert_eq!(same, id);
    s.replace_mailbox(second);
    assert!(!not_listed_anywhere(s.mailbox.message(id).unwrap()));
    s.read_at(id, crate::ReadIn::Pane, mailcraft_html::Env::default(), false);
    assert!(not_listed_anywhere(s.mailbox.message(id).unwrap()));
}
