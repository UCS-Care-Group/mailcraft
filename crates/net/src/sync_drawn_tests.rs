// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1970: a picture the body draws is marked the body's when an IMAP message comes down, but
//! not in a draft, whose parts go up again as they are. Invented addresses only.

use super::*;

const MIME: &[u8] = b"From: Ada <ada@example.org>\r\nTo: bob@example.org\r\nSubject: Signed\r\nMessage-ID: <s@example.org>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
<p>Regards <img src=\"cid:logo@example.org\"></p>\r\n--B\r\nContent-Type: image/png\r\nContent-Disposition: attachment; filename=\"logo.png\"\r\n\
Content-ID: <logo@example.org>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgoAAAANSUhEUg==\r\n--B--\r\n";

#[test]
fn a_drawn_picture_is_marked_but_not_in_a_draft() {
    let fetched =
        |flags: &[&str]| Fetched { uid: 7, body: Some(MIME.to_vec()), flags: flags.iter().map(|f| f.to_string()).collect(), ..Default::default() };
    let m = message_from(fetched(&[]));
    assert!(m.attachments[0].inline && m.listed_attachments().next().is_none());
    let draft = message_from(fetched(&["\\Draft"]));
    assert!(draft.draft && !draft.attachments[0].inline);
}
