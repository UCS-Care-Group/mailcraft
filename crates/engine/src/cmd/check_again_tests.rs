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
    let mut m = mailcraft_mime_model::parse(bytes);
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
    assert_eq!(imap_again::wanted(&s.mailbox, account, s.now()).get(&folder).map(Vec::as_slice), Some(&[7u32][..]));
    // What the server gives again is taken in: the bytes it came as, and the mark goes.
    imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(SIGNED));
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
    let mut m = mailcraft_mime_model::parse_fetched(SIGNED);
    m.folder = mb.inbox().unwrap();
    let id = mb.add_message(m).unwrap();
    let mut s = Session::new(mb);
    assert!(s.mailbox.message(id).unwrap().has_original());
    s.run("mail.removeAttachment", &json!({"id": id, "index": 0})).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.original.is_none() && m.attachments.is_empty());
    // Written whole now, it is built from what is left: the file doesn't come back.
    let written = mailcraft_mime_model::write(m, &Default::default());
    assert!(!String::from_utf8_lossy(&written).contains("smime.p7s"));
}

#[test]
fn what_check_again_brings_is_taken_for_the_original_alone() {
    // A server's copy that says other things than what is here (a subject, headers): only the
    // bytes it came as are taken; the message stays as the owner had it.
    let other: Vec<u8> = String::from_utf8_lossy(SIGNED).replace("Subject: Signed", "Subject: Changed\r\nX-Server: 1").into_bytes();
    for kind in [AccountKind::Microsoft, AccountKind::Exchange, AccountKind::Jmap, AccountKind::Imap] {
        let (mut s, id) = read_before(kind, SIGNED);
        s.mailbox.message_mut(id).unwrap().subject = "Renamed here".into();
        let before = s.mailbox.message(id).cloned().unwrap();
        s.run("mail.checkAgain", &json!({"id": id})).unwrap();
        assert!(s.mailbox.message(id).unwrap().original_wanted, "{kind:?}");
        // Asked for once: a second Check again waits for the first.
        assert!(s.run("mail.checkAgain", &json!({"id": id})).is_err(), "{kind:?}");
        if kind == AccountKind::Imap {
            imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(&other));
        } else {
            let mut index = Default::default();
            assert!(graph_down::take_fetched(&mut s.mailbox, &mut index, before.account, "AAkA1", &other), "{kind:?}");
        }
        let m = s.mailbox.message(id).unwrap();
        assert_eq!(m.original_bytes(), Some(other.as_slice()), "{kind:?}");
        assert_eq!(
            (m.subject.as_str(), &m.headers, &m.attachments, &m.body),
            ("Renamed here", &before.headers, &before.attachments, &before.body),
            "{kind:?}"
        );
        assert!(!m.original_wanted && !m.unfetched && !m.refetch, "{kind:?}");
    }
}

#[test]
fn a_message_a_part_was_removed_from_is_never_checked_again() {
    for kind in [AccountKind::Imap, AccountKind::Microsoft] {
        let (mut s, id) = read_before(kind, SIGNED);
        s.run("mail.removeAttachment", &json!({"id": id, "index": 0})).unwrap();
        assert!(s.mailbox.message(id).unwrap().parts_removed);
        assert!(!s.enabled("mail.checkAgain"), "{kind:?}");
        assert!(s.run("mail.checkAgain", &json!({"id": id})).is_err(), "{kind:?}");
        // Nor does a download of it again for another reason (#650) bring the original back.
        imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(SIGNED));
        assert!(s.mailbox.message(id).unwrap().original.is_none(), "{kind:?}");
    }
    // The mark is kept with the mailbox.
    let (mut s, id) = read_before(AccountKind::Imap, SIGNED);
    s.run("mail.removeAttachment", &json!({"id": id, "index": 0})).unwrap();
    let json = serde_json::to_string(s.mailbox.message(id).unwrap()).unwrap();
    let back: mailcraft_model::Message = serde_json::from_str(&json).unwrap();
    assert!(back.parts_removed);
}

#[test]
fn past_the_mailboxs_cap_an_original_isnt_kept_and_can_be_checked_again_later() {
    let (mut s, id) = read_before(AccountKind::Imap, SIGNED);
    // Another message names a whole cap's worth of originals.
    let folder = s.mailbox.message(id).unwrap().folder;
    let full = mailcraft_model::ContentRef { sha256: [7; 32], len: mailcraft_model::MAX_ORIGINALS };
    let original = Some(mailcraft_model::Original { data: std::sync::Arc::default(), kept: Some(full) });
    let other = s.mailbox.add_message(mailcraft_model::Message { folder, original, ..Default::default() }).unwrap();
    assert!(s.mailbox.message(other).unwrap().has_original());
    s.run("mail.checkAgain", &json!({"id": id})).unwrap();
    imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(SIGNED));
    assert!(s.mailbox.message(id).unwrap().original.is_none());
    // Coming in new, too.
    let mut read = mailcraft_mime_model::parse_fetched(SIGNED);
    read.folder = folder;
    let new = s.mailbox.add_message(read).unwrap();
    assert!(s.mailbox.message(new).unwrap().original.is_none());
    // Once there is room, Check again fetches it.
    s.mailbox.message_mut(other).unwrap().original = None;
    assert!(s.enabled("mail.checkAgain"));
    s.run("mail.checkAgain", &json!({"id": id})).unwrap();
    imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(SIGNED));
    assert_eq!(s.mailbox.message(id).unwrap().original_bytes(), Some(SIGNED));
}

#[test]
fn the_size_of_a_message_counts_its_original() {
    let m = mailcraft_mime_model::parse_fetched(SIGNED);
    let without = mailcraft_model::Message { original: None, ..m.clone() };
    assert_eq!(m.size(), without.size() + SIGNED.len() as u64);
}

/// A message that is only its headers and preview (armour in a preview looks protected) isn't
/// checked again: opening it fetches it whole, original and all; what Check again brings would
/// be taken for the original alone and leave the preview as its body.
#[test]
fn a_message_not_downloaded_yet_isnt_checked_again() {
    for kind in [AccountKind::Microsoft, AccountKind::Exchange, AccountKind::Jmap, AccountKind::Imap] {
        let (mut s, id) = read_before(kind, INLINE);
        s.mailbox.message_mut(id).unwrap().unfetched = true;
        assert!(s.mailbox.message(id).unwrap().looks_protected());
        assert!(!s.enabled("mail.checkAgain"), "{kind:?}");
        assert!(s.run("mail.checkAgain", &json!({"id": id})).is_err(), "{kind:?}");
        assert!(!s.mailbox.message(id).unwrap().original_wanted, "{kind:?}");
    }
    // Fetched whole when opened, it takes its body and its original.
    let (mut s, id) = read_before(AccountKind::Microsoft, INLINE);
    s.mailbox.message_mut(id).unwrap().unfetched = true;
    s.mailbox.message_mut(id).unwrap().body.text = "-----BEGIN PGP SIGNED MESSAGE-----".into();
    let account = s.mailbox.message(id).unwrap().account;
    let mut index = Default::default();
    assert!(graph_down::take_fetched(&mut s.mailbox, &mut index, account, "AAkA1", INLINE));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.original_bytes(), Some(INLINE));
    assert!(m.body.text.contains("Hi."));
}

/// The room for an original counts the one it replaces: a mailbox at its cap takes a new copy of
/// an original it holds already.
#[test]
fn an_original_taken_again_counts_only_what_it_adds() {
    let (mut s, id) = read_before(AccountKind::Imap, SIGNED);
    s.mailbox.message_mut(id).unwrap().original = mailcraft_model::Original::new(SIGNED.to_vec());
    let folder = s.mailbox.message(id).unwrap().folder;
    let rest = mailcraft_model::ContentRef { sha256: [7; 32], len: mailcraft_model::MAX_ORIGINALS - SIGNED.len() as u64 };
    let original = Some(mailcraft_model::Original { data: std::sync::Arc::default(), kept: Some(rest) });
    s.mailbox.add_message(mailcraft_model::Message { folder, original, ..Default::default() }).unwrap();
    assert!(!s.mailbox.room_for_original(1));
    // Downloaded again (#650), the same bytes come: the mailbox holds no more than before.
    s.mailbox.message_mut(id).unwrap().refetch = true;
    imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(SIGNED));
    assert_eq!(s.mailbox.message(id).unwrap().original_bytes(), Some(SIGNED));
}

/// After Remove Attachment, a download again by UID (#650) takes the text alone, not the files,
/// so not the original either; a fetch whole (Microsoft Graph) takes the files and the original
/// from the same download, and the message is that download again.
#[test]
fn a_whole_download_again_takes_the_files_and_the_original_together() {
    let (mut s, id) = read_before(AccountKind::Imap, SIGNED);
    s.run("mail.removeAttachment", &json!({"id": id, "index": 0})).unwrap();
    s.mailbox.message_mut(id).unwrap().refetch = true;
    imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(SIGNED));
    let m = s.mailbox.message(id).unwrap();
    assert!(m.original.is_none() && m.attachments.is_empty() && m.parts_removed);

    let (mut s, id) = read_before(AccountKind::Microsoft, SIGNED);
    s.run("mail.removeAttachment", &json!({"id": id, "index": 0})).unwrap();
    let account = s.mailbox.message(id).unwrap().account;
    graph_down::thin_message(s.mailbox.message_mut(id).unwrap(), "preview".into());
    let mut index = Default::default();
    assert!(graph_down::take_fetched(&mut s.mailbox, &mut index, account, "AAkA1", SIGNED));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.original_bytes(), Some(SIGNED));
    assert_eq!(m.attachments.len(), 1);
    assert!(!m.parts_removed);
}

/// The server's copy put there from its parts: not signed, as Microsoft Graph makes a message.
const UNPROTECTED: &[u8] =
    b"From: Ada <ada@example.org>\r\nTo: avery@example.org\r\nSubject: Signed\r\nMessage-ID: <ca-1@example.org>\r\n\r\nSigned text.\r\n";

/// [`read_before`], the message keeping the bytes it came as.
fn kept_whole(kind: AccountKind) -> (Session, Id) {
    let (mut s, id) = read_before(kind, SIGNED);
    s.mailbox.message_mut(id).unwrap().original = mailcraft_model::Original::new(SIGNED.to_vec());
    (s, id)
}

/// A download that brings no original (the server's copy was put there from its parts) leaves
/// the one kept here, whether it was asked for alone or the message was fetched whole.
#[test]
fn a_download_without_an_original_keeps_the_one_here() {
    for wanted in [true, false] {
        let (mut s, id) = kept_whole(AccountKind::Imap);
        let m = s.mailbox.message_mut(id).unwrap();
        (m.refetch, m.original_wanted) = (true, wanted);
        imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(UNPROTECTED));
        assert_eq!(s.mailbox.message(id).unwrap().original_bytes(), Some(SIGNED), "IMAP, wanted {wanted}");

        let (mut s, id) = kept_whole(AccountKind::Microsoft);
        let account = s.mailbox.message(id).unwrap().account;
        let m = s.mailbox.message_mut(id).unwrap();
        graph_down::thin_message(m, "preview".into());
        m.original_wanted = wanted;
        let mut index = Default::default();
        assert!(graph_down::take_fetched(&mut s.mailbox, &mut index, account, "AAkA1", UNPROTECTED));
        assert_eq!(s.mailbox.message(id).unwrap().original_bytes(), Some(SIGNED), "Graph, wanted {wanted}");
    }
}

/// A signed message moved to a Microsoft account goes there made of its parts (Graph makes no
/// message of MIME): fetched from there again, the copy here keeps the bytes it came as, and
/// they are what Save As writes and what goes to an IMAP server next.
#[test]
fn a_message_whose_server_copy_isnt_protected_keeps_its_original() {
    let (mut s, id) = kept_whole(AccountKind::Microsoft);
    let account = s.mailbox.message(id).unwrap().account;
    graph_down::thin_message(s.mailbox.message_mut(id).unwrap(), "preview".into());
    let mut index = Default::default();
    assert!(graph_down::take_fetched(&mut s.mailbox, &mut index, account, "AAkA1", UNPROTECTED));
    let saved = s.run("file.exportMessage", &json!({"id": id})).unwrap();
    let data = mailcraft_charset::base64::STANDARD.decode(saved["data"].as_str().unwrap()).unwrap();
    let body = |b: &[u8]| b.windows(4).position(|w| w == b"\r\n\r\n").map(|at| b[at + 4..].to_vec()).unwrap();
    assert_eq!(body(&data), body(SIGNED));
    let placed = mailcraft_mime_model::write_placed(&s.loaded_message(id).unwrap(), &Default::default(), "m");
    assert_eq!(body(&placed), body(SIGNED));
}

/// On IMAP, Check again on a message whose text was read wrongly (U+FFFD) takes the whole of
/// the download, not the original alone: the text is downloaded again only once.
#[test]
fn check_again_on_text_read_wrongly_takes_the_text_too() {
    let (mut s, id) = read_before(AccountKind::Imap, SIGNED);
    s.mailbox.message_mut(id).unwrap().body.text = "Signed t\u{fffd}xt.".into();
    s.run("mail.checkAgain", &json!({"id": id})).unwrap();
    assert_eq!(imap_again::mark_unreadable(&mut s.mailbox), 0, "marked already");
    imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(SIGNED));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.original_bytes(), Some(SIGNED));
    assert_eq!(m.body.text.trim(), "Signed text.");
    assert!(m.refetched && !m.refetch && !m.original_wanted);
}

/// Its text read wrongly kept on disk, not in memory: Check again reads it for that, and takes
/// the whole download too, even once the text is let go of again.
#[test]
fn check_again_on_text_read_wrongly_on_disk_takes_the_text_too() {
    let (mut s, id) = read_before(AccountKind::Imap, SIGNED);
    s.mailbox.message_mut(id).unwrap().body.text = format!("Signed t\u{fffd}xt. {}", "More. ".repeat(400));
    let dir = std::env::temp_dir().join(format!("mailcraft-check-again-disk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mailbox.mailcraft");
    crate::store::save(&path, &s.mailbox).unwrap();
    crate::store::clear_cache();
    let mut s = Session::new(crate::store::with_budget(1, || crate::store::open(&path).unwrap()).mailbox);
    s.path = Some(path.clone());
    assert!(s.mailbox.message(id).unwrap().body.text.is_empty(), "kept on disk");
    s.run("mail.checkAgain", &json!({"id": id})).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.refetch && !m.original_wanted);
    // Let go of again before the download comes.
    let mut thin = mailcraft_model::Message::clone(m);
    thin.body.text = String::new();
    *s.mailbox.message_mut(id).unwrap() = thin;
    imap_again::take_again(&mut s.mailbox, id, mailcraft_mime_model::parse_fetched(SIGNED));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.original_bytes(), Some(SIGNED));
    assert_eq!(m.body.text.trim(), "Signed text.");
    let _ = std::fs::remove_dir_all(&dir);
}
