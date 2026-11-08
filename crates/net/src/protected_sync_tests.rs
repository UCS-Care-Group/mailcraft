// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Digitally signed message comes down with the bytes it came as, and goes onto another
//! account's server as exactly those bytes, so that its signature still checks there (#30).
//! A synthetic message with a dummy signature on the in-process fake server: no real crypto.

use mailcraft_model::sync::ImapPlace;

use super::*;

/// An S/MIME-shaped message with a dummy `smime.p7s`, with its line ends as a server keeps them.
const SIGNED: &[u8] = b"From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: Signed\r\n\
Date: Tue, 01 Jul 2003 10:52:37 +0200\r\nMessage-ID: <signed@example.org>\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"=_b\"\r\n\r\n\
This is a cryptographically signed message in MIME format.\r\n\r\n\
--=_b\r\nContent-Type: text/plain; charset=\"iso-8859-1\"\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
Caf=E9 at  noon,   with odd   spacing kept.  \r\n--=_b\r\n\
Content-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\nContent-Transfer-Encoding: base64\r\n\
Content-Disposition: attachment; filename=\"smime.p7s\"\r\n\r\nZHVtbXkgc2lnbmF0dXJlIGJ5dGVz\r\n--=_b--\r\n";

#[test]
fn a_signed_message_comes_down_whole_and_goes_up_byte_for_byte() {
    let mut world = imap_world();
    world.mailbox("INBOX").messages.push(FakeMessage { body: SIGNED.to_vec(), ..message(7, "", "unused") });
    let imap = Fake::imap(world);
    let result = sync(&request(account(Some(&imap), None), local_folders()));
    assert_eq!(result.errors, Vec::<String>::new());
    let inbox = delta(&result, "INBOX");
    let signed = inbox.new.iter().find(|m| m.uid == 7).unwrap();
    assert_eq!(signed.original_bytes(), Some(SIGNED));
    assert_eq!(signed.subject, "Signed");
    // Mail that isn't signed or encrypted keeps nothing more than before.
    assert!(inbox.new.iter().filter(|m| m.uid != 7).all(|m| m.original.is_none()));
    assert!(!inbox.new.is_empty());

    // Moved into another account: what is put on its server is the message as it came, after
    // the one line of MailCraft's own, which no Digital signature covers.
    let other = two_way(true, true);
    let folders = synced_folders(&other);
    let mut message = signed.clone();
    message.id = 4_242;
    let place = ImapPlace { message, to: "Projects".into(), to_folder: PROJECTS, marker: "m4rk-3r".into(), ..Default::default() };
    let up = sync(&SyncRequest { places: vec![place], ..request(account(Some(&other), None), folders) });
    assert_eq!(up.errors, Vec::<String>::new());
    assert_eq!(up.placed.len(), 1);
    let world = other.world();
    let put = world.mailboxes.iter().find(|m| m.raw == "Projects").unwrap().messages.last().unwrap().body.clone();
    drop(world);
    assert_eq!(put, [&b"X-MailCraft-Place: m4rk-3r\r\n"[..], SIGNED].concat());
    // Come down from there, it is the same original again.
    assert_eq!(mailcraft_mime::parse_fetched(&put).original_bytes(), Some(SIGNED));
}

#[test]
fn a_signed_message_too_large_to_download_keeps_no_original() {
    let mut world = imap_world();
    let large = [SIGNED, &vec![b'x'; 4096]].concat();
    world.mailbox("INBOX").messages.push(FakeMessage { body: large, ..message(8, "", "unused") });
    let imap = Fake::imap(world);
    let limits = Limits { largest: 1024, ..Limits::kept_whole() };
    let result = sync_within(&request(account(Some(&imap), None), local_folders()), limits);
    let header_only = delta(&result, "INBOX").new.iter().find(|m| m.uid == 8).unwrap();
    assert!(header_only.not_downloaded > 0);
    assert!(header_only.original.is_none());
}
