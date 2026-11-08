// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check again (#30): a Digitally signed or Encrypted message without the bytes it came as is
//! marked to have them downloaded again, by its UID (IMAP) or fetched whole again (Microsoft
//! Graph, Exchange servers, JMAP), and keeps them once they come. Synthetic messages only.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{AccountKind, Id, Mailbox};
use serde_json::json;

use crate::Session;
use crate::cmd::{graph_down, imap_again};

const SIGNED: &[u8] = b"From: Ada <ada@example.org>\r\nTo: avery@example.org\r\nSubject: Signed\r\nMessage-ID: <ca-1@example.org>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nSigned text.\r\n--b\r\nContent-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\n\
Content-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"smime.p7s\"\r\n\r\nZHVtbXk=\r\n--b--\r\n";

const INLINE: &[u8] = b"From: Ada <ada@example.org>\r\nSubject: Inline\r\n\r\n-----BEGIN PGP SIGNED MESSAGE-----\r\nHash: SHA256\r\n\r\nHi.\r\n-----BEGIN PGP SIGNATURE-----\r\nZHVtbXk=\r\n-----END PGP SIGNATURE-----\r\n";

/// A mailbox whose account is of `kind`, with `bytes` read in its Inbox as before #30 (only its
/// parts kept), as UID 7 or the server's message `AAkA1`.
fn read_before(kind: AccountKind, bytes: &[u8]) -> (Session, Id) {
    let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
    mb.accounts[0].kind = kind;
    let inbox = mb.inbox().unwrap();
    let mut m = mailcraft_mime::parse(bytes);
    m.folder = inbox;
    match kind {
        AccountKind::Imap => m.uid = 7,
        AccountKind::Local => {}
        _ => m.server = Some(ServerMessage { id: "AAkA1".into(), folder: "F".into(), size: bytes.len() as u64, ..Default::default() }),
    }
    let id = mb.add_message(m).unwrap();
    let mut s = Session::new(mb);
    s.sel.messages = vec![id];
    (s, id)
}

#[test]
fn an_imap_message_is_asked_for_again_by_its_uid_and_keeps_its_original() {
    let (mut s, id) = read_before(AccountKind::Imap, SIGNED);
    assert!(s.mailbox.message(id).unwrap().looks_protected());
    assert!(s.enabled("mail.checkAgain"));
    assert_eq!(s.run("mail.checkAgain", &json!({"id": id})).unwrap()["downloads"], true);
    let account = s.mailbox.message(id).unwrap().account;
    let folder = s.mailbox.message(id).unwrap().folder;
    assert!(s.mailbox.message(id).unwrap().refetch);
    assert_eq!(imap_again::wanted(&s.mailbox, account).get(&folder).map(Vec::as_slice), Some(&[7u32][..]));
    // What the server gives again is taken in: the bytes it came as, and the mark goes.
    imap_again::take_again(&mut s.mailbox, id, mailcraft_mime::parse_fetched(SIGNED));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.original_bytes(), Some(SIGNED));
    assert!(!m.refetch && m.subject == "Signed");
    // Having them, it isn't checked again.
    assert!(!s.enabled("mail.checkAgain"));
    assert!(s.run("mail.checkAgain", &json!({"id": id})).is_err());
}

#[test]
fn a_microsoft_message_is_fetched_whole_again_keeping_what_it_has_meanwhile() {
    for kind in [AccountKind::Microsoft, AccountKind::Exchange, AccountKind::Jmap] {
        let (mut s, id) = read_before(kind, SIGNED);
        let text = s.mailbox.message(id).unwrap().body.text.clone();
        s.run("mail.checkAgain", &json!({"id": id})).unwrap();
        let m = s.mailbox.message(id).unwrap();
        assert!(m.unfetched && m.has_attachments() && m.body.text == text, "{kind:?}");
        let account = m.account;
        assert!(graph_down::fetch_queue(&s.mailbox, account, 10, m.received).contains(&"AAkA1".to_string()), "{kind:?}");
        let mut index = Default::default();
        assert!(graph_down::take_fetched(&mut s.mailbox, &mut index, account, "AAkA1", SIGNED), "{kind:?}");
        let m = s.mailbox.message(id).unwrap();
        assert_eq!(m.original_bytes(), Some(SIGNED), "{kind:?}");
        assert!(!m.unfetched);
    }
}

#[test]
fn inline_pgp_is_told_by_its_text() {
    let (mut s, id) = read_before(AccountKind::Imap, INLINE);
    assert!(s.mailbox.message(id).unwrap().attachments.is_empty());
    s.run("mail.checkAgain", &json!({"id": id})).unwrap();
    assert!(s.mailbox.message(id).unwrap().refetch);
}

#[test]
fn other_mail_and_mail_no_server_has_are_refused() {
    let plain = b"From: ada@example.org\r\nSubject: plain\r\n\r\nHello.\r\n";
    let (mut s, id) = read_before(AccountKind::Imap, plain);
    assert!(!s.enabled("mail.checkAgain"));
    let before = s.mailbox.message(id).cloned();
    assert!(s.run("mail.checkAgain", &json!({"id": id})).is_err());
    assert_eq!(s.mailbox.message(id).cloned(), before);
    // On this computer alone.
    let (mut s, id) = read_before(AccountKind::Local, SIGNED);
    assert!(!s.enabled("mail.checkAgain"));
    assert!(s.run("mail.checkAgain", &json!({"id": id})).is_err());
    // An IMAP message not on its server (no UID).
    let (mut s, id) = read_before(AccountKind::Imap, SIGNED);
    s.mailbox.message_mut(id).unwrap().uid = 0;
    assert!(s.run("mail.checkAgain", &json!({"id": id})).is_err());
    // Nothing selected, or a message that isn't there.
    s.sel.messages.clear();
    assert!(!s.enabled("mail.checkAgain"));
    assert!(s.run("mail.checkAgain", &json!({})).is_err());
    assert!(s.run("mail.checkAgain", &json!({"id": 999_999})).is_err());
}

#[test]
fn removing_an_attachment_lets_the_original_go() {
    let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
    let mut m = mailcraft_mime::parse_fetched(SIGNED);
    m.folder = mb.inbox().unwrap();
    let id = mb.add_message(m).unwrap();
    let mut s = Session::new(mb);
    assert!(s.mailbox.message(id).unwrap().has_original());
    s.run("mail.removeAttachment", &json!({"id": id, "index": 0})).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.original.is_none() && m.attachments.is_empty());
    // Written whole now, it is built from what is left: the file doesn't come back.
    let written = mailcraft_mime::write(m, &Default::default());
    assert!(!String::from_utf8_lossy(&written).contains("smime.p7s"));
}
