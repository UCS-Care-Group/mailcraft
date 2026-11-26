// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A signature picture the HTML draws, sent as an attachment, stays out of the attachment list
//! with its body on disk, after the mailbox is opened again (#1970). Invented people only.

use mailcraft_model::search::Query;

use super::*;

const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUg==";

#[test]
fn a_drawn_signature_picture_has_no_paperclip_with_its_body_on_disk() {
    let html = format!("<p>{}</p><img src=\"cid:image001.png@01DB0000.0000AAAA\">", "Regards. ".repeat(400));
    assert!(html.len() > INLINE_MAX);
    let raw = format!(
        "From: Sender <sender@a.example>\r\nTo: Avery <avery@example.org>\r\nSubject: signature\r\n\
         Message-ID: <sig@a.example>\r\nDate: Wed, 14 Oct 2026 15:00:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n\
         --B\r\nContent-Type: image/png; name=\"image001.png\"\r\nContent-Disposition: attachment; filename=\"image001.png\"\r\n\
         Content-ID: <image001.png@01DB0000.0000AAAA>\r\nContent-Transfer-Encoding: base64\r\n\r\n{PNG_B64}\r\n--B--\r\n"
    );
    let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
    let mut m = mailcraft_mime_model::parse(raw.as_bytes());
    m.account = mb.accounts[0].id;
    m.folder = mb.inbox().unwrap();
    let id = mb.add_message(m).unwrap();
    let tmp = Temp::new("drawn-outside");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let s = session_without_bodies(&path);
    let m = s.mailbox.message(id).unwrap();
    assert!(m.body_is_kept());
    assert!(m.listed_attachments().next().is_none() && !m.has_attachments());
    assert!(!Query::parse("has:attachment").matches(m, 0) && !mailcraft_model::Condition::HasAttachment.matches(m));
}
