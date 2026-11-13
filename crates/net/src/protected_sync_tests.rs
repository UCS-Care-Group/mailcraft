// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Digitally signed message comes down with the bytes it came as, and goes onto another
//! account's server with its body as exactly those bytes, so that its signature still checks there (#30).
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

/// The body of a message: what follows its first empty line.
fn body(bytes: &[u8]) -> &[u8] {
    bytes.windows(4).position(|w| w == b"\r\n\r\n").and_then(|at| bytes.get(at + 4..)).unwrap_or_default()
}

#[test]
fn a_signed_message_comes_down_whole_and_its_body_goes_up_byte_for_byte() {
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

    // Moved into another account: what is put on its server is its header written from the
    // message (MailCraft's own line first) and its body as it came, which the signature covers.
    let other = two_way(true, true);
    let folders = synced_folders(&other);
    let mut message = signed.clone();
    message.id = 4_242;
    let place = ImapPlace { message, to: "Projects".into(), to_folder: PROJECTS, marker: "m4rk-3r".into(), ..Default::default() };
    let up = sync(&SyncRequest { places: vec![place], ..request(account(Some(&other), None), folders) });
    assert_eq!(up.errors, Vec::<String>::new());
    assert_eq!(up.placed.len(), 1);
    assert!(up.placed[0].original);
    let world = other.world();
    let put = world.mailboxes.iter().find(|m| m.raw == "Projects").unwrap().messages.last().unwrap().body.clone();
    drop(world);
    assert!(put.starts_with(b"X-MailCraft-Place: m4rk-3r\r\n"));
    assert_eq!(body(&put), body(SIGNED));
    // Come down from there, it has the same body again.
    assert_eq!(mailcraft_mime_model::parse_fetched(&put).original_bytes().map(body), Some(body(SIGNED)));
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

/// The last message the fake server has in "Projects".
fn put_in_projects(imap: &FakeImap) -> Vec<u8> {
    let world = imap.world();
    world.mailboxes.iter().find(|m| m.raw == "Projects").unwrap().messages.last().unwrap().body.clone()
}

/// What is put on a server, with a marker or without one, carries no DKIM signature or ARC
/// seal of the sender's: they cover a header written otherwise there, and would fail.
#[test]
fn a_copy_put_on_a_server_carries_no_dkim_or_arc_seal() {
    let sealed = [&b"DKIM-Signature: v=1; d=example.org; b=x\r\nARC-Seal: i=1; b=y\r\nARC-Message-Signature: i=1; b=z\r\n"[..], SIGNED].concat();
    let mut message = mailcraft_mime_model::parse_fetched(&sealed);
    assert!(message.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("dkim-signature")));
    message.id = 4_243;
    for marker in ["m4rk-3r", ""] {
        let other = two_way(true, true);
        let folders = synced_folders(&other);
        let place = ImapPlace { message: message.clone(), to: "Projects".into(), to_folder: PROJECTS, marker: marker.into(), ..Default::default() };
        let up = sync(&SyncRequest { places: vec![place], ..request(account(Some(&other), None), folders) });
        assert_eq!(up.errors, Vec::<String>::new());
        let put = put_in_projects(&other);
        let text = String::from_utf8_lossy(&put).to_ascii_lowercase();
        assert!(!text.contains("dkim-signature") && !text.contains("arc-seal") && !text.contains("arc-message-signature"), "{marker}: {text}");
        assert_eq!(body(&put), body(SIGNED), "{marker}");
    }
}

/// A protected message with a part sent as `binary` goes written from its parts: an APPEND
/// doesn't carry bytes that aren't lines.
#[test]
fn a_binary_part_goes_up_written_from_its_parts() {
    let der: &[u8] = b"\x30\x0a\x06\x09\n\x2a\x86\nraw\r\n\n";
    let head = b"From: ada@example.org\r\nSubject: Sealed\r\nMessage-ID: <bin@example.org>\r\nMIME-Version: 1.0\r\n\
Content-Type: application/pkcs7-mime; smime-type=enveloped-data; name=\"smime.p7m\"\r\nContent-Transfer-Encoding: binary\r\n\r\n";
    let bytes = [&head[..], der].concat();
    let mut message = mailcraft_mime_model::parse_fetched(&bytes);
    assert_eq!(message.original_bytes(), Some(bytes.as_slice()));
    message.id = 4_244;
    let other = two_way(true, true);
    let folders = synced_folders(&other);
    let place = ImapPlace { message, to: "Projects".into(), to_folder: PROJECTS, marker: "m4rk-3r".into(), ..Default::default() };
    let up = sync(&SyncRequest { places: vec![place], ..request(account(Some(&other), None), folders) });
    assert_eq!(up.errors, Vec::<String>::new());
    assert_eq!(up.placed.len(), 1);
    // Not written from the bytes it came as, and said so.
    assert!(!up.placed[0].original);
    let put = put_in_projects(&other);
    assert!(!put.windows(der.len()).any(|w| w == der));
    assert!(!String::from_utf8_lossy(&put).to_ascii_lowercase().contains("content-transfer-encoding: binary"));
}

/// A message asked for again that comes without its bytes (no `BODY[]`, or an empty one) is
/// settled as refused: nothing came, which says nothing of what its copy holds (#30).
#[test]
fn a_message_asked_for_again_that_comes_empty_is_refused() {
    let imap = Fake::imap(imap_world());
    let mut folders = synced_folders_of(&imap);
    imap.world().mailbox("INBOX").messages.iter_mut().find(|m| m.uid == 1).unwrap().body.clear();
    folders.iter_mut().find(|f| f.remote == "INBOX").unwrap().again = vec![1];
    let result = sync(&request(account(Some(&imap), None), folders));
    let inbox = delta(&result, "INBOX");
    assert!(inbox.refetched.is_empty());
    assert_eq!((inbox.refetch_done.as_slice(), inbox.refetch_refused.as_slice()), (&[1][..], &[1][..]));
}
