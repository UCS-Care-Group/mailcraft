// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Digitally signed message moved into another IMAP account (#30) goes onto that server with its
//! body as it came and its header written from the message, so that what the mailbox leaves
//! behind of a server's word (#781) stays behind and the signature still checks there. Synthetic
//! messages with dummy signatures only.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::Session;
use crate::cmd::account::{apply_sync, sync_request};

/// Signed by its sender, with stamps forged above the signed part (a sender can write any header).
const STAMPED: &[u8] = b"Authentication-Results: mx.example.org; dkim=pass header.d=example.org\r\n\
ARC-Authentication-Results: i=1; mx.example.org; dkim=pass\r\n\
X-MS-Exchange-Organization-AuthAs: Internal\r\n\
From: Boss <boss@example.org>\r\nTo: avery@mail.example\r\nSubject: Signed\r\nMessage-ID: <pm-1@example.org>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nPay the invoice.\r\n--b\r\nContent-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\n\
Content-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"smime.p7s\"\r\n\r\nZHVtbXk=\r\n--b--\r\n";

/// The body of a message: what follows its first empty line.
fn body(bytes: &[u8]) -> &[u8] {
    bytes.windows(4).position(|w| w == b"\r\n\r\n").and_then(|at| bytes.get(at + 4..)).unwrap_or_default()
}

/// An IMAP account whose Inbox follows "INBOX", with `new` in it.
fn imap_account(s: &mut Session, email: &str, new: Vec<mailcraft_model::Message>) -> Id {
    let servers = json!({"email": email, "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let a = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let uid_seen = new.iter().map(|m| m.uid).max().unwrap_or(0);
    let inbox = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen, new, ..Default::default() };
    apply_sync(s, SyncResult { account: a, folders: vec![inbox], ..Default::default() }).unwrap();
    a
}

/// What goes onto account `to`'s server for message `bytes`, moved there from another IMAP account.
fn moved_up(bytes: &[u8]) -> Vec<u8> {
    let mut s = Session::empty();
    let read = mailcraft_model::Message { uid: 1, ..mailcraft_mime::parse_fetched(bytes) };
    let from = imap_account(&mut s, "avery@mail.example", vec![read]);
    let to = imap_account(&mut s, "work@mail.example", vec![]);
    let id = s.mailbox.messages.values().find(|m| m.account == from).unwrap().id;
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap()})).unwrap();
    let asked = sync_request(&s, to).unwrap();
    let [place] = asked.places.as_slice() else { panic!("{:?}", asked.places) };
    mailcraft_mime::write_placed(&place.message, &mailcraft_mime::WriteOptions::default(), &place.marker)
}

#[test]
fn a_signed_message_moved_to_another_server_leaves_the_stamps_behind() {
    let put = moved_up(STAMPED);
    let text = String::from_utf8_lossy(&put);
    for stamp in ["Authentication-Results", "ARC-Authentication-Results", "X-MS-Exchange-Organization-AuthAs"] {
        assert!(!text.to_ascii_lowercase().contains(&stamp.to_ascii_lowercase()), "{stamp} in\n{text}");
    }
    // The signed body, byte for byte, under its own Content-Type.
    assert_eq!(body(&put), body(STAMPED));
    assert!(text.contains("Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"b\"\r\n\r\n"));
    assert!(text.starts_with("X-MailCraft-Place: "));
}

#[test]
fn a_signed_message_of_lf_lines_goes_up_in_crlf_with_its_body_canonical() {
    // As an mbox or a file of LF lines brings it in.
    let lf: Vec<u8> = String::from_utf8_lossy(STAMPED).replace("\r\n", "\n").into_bytes();
    let read = mailcraft_mime::parse_mbox(&[&b"From boss@example.org Tue Jul  1 08:52:37 2003\n"[..], &lf].concat());
    let original = read.first().and_then(|m| m.original_bytes()).unwrap().to_vec();
    let put = moved_up(&original);
    assert!(put.windows(2).all(|w| w[1] != b'\n' || w[0] == b'\r'), "a bare LF in\n{}", String::from_utf8_lossy(&put));
    assert!(!put.starts_with(b"\n"));
    assert_eq!(body(&put), body(STAMPED));
}
