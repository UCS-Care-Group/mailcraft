// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bytes a Digitally signed or Encrypted message came as (#30) are kept in the content
//! folder as an attachment is: named by their hash, not brought in at opening, read back
//! through the same checks, let go of again, written whole into a saved `.eml`, and forgotten
//! with the message. A missing one leaves the message as one read before they were kept.

use mailcraft_model::Original;

use super::*;

/// A synthetic S/MIME-shaped message with a dummy signature: no real cryptography.
fn signed_bytes() -> Vec<u8> {
    let filler = "Signed text that runs on. ".repeat(200);
    format!(
        "From: Ada <ada@example.org>\r\nTo: Bea <bea@example.org>\r\nSubject: Signed\r\nMessage-ID: <orig-1@example.org>\r\n\
         MIME-Version: 1.0\r\nContent-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"b\"\r\n\r\n\
         --b\r\nContent-Type: text/plain\r\n\r\n{filler}\r\n--b\r\nContent-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\nZHVtbXkgc2lnbmF0dXJl\r\n--b--\r\n"
    )
    .into_bytes()
}

/// A mailbox with the signed message in its Inbox, read as a server gives it; its id.
fn with_signed() -> (Mailbox, Id) {
    let mut mb = mailbox(4);
    let inbox = mb.inbox().unwrap();
    let mut m = mailcraft_mime::parse_fetched(&signed_bytes());
    assert!(m.has_original());
    m.folder = inbox;
    let id = mb.add_message(m).unwrap();
    (mb, id)
}

#[test]
fn the_original_is_kept_outside_named_read_back_and_written_whole() {
    let (mb, id) = with_signed();
    let bytes = signed_bytes();
    assert!(bytes.len() > INLINE_MAX);
    let tmp = Temp::new("original-outside");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    // The record has none of it and names the content file, which holds it.
    let record = prepare(id, mb.messages.get(&id).unwrap(), &mut HashMap::new()).unwrap();
    assert!(!String::from_utf8_lossy(&record.json).contains("multipart/signed"));
    assert_eq!(record.parts.original.map(|p| p.r.len), Some(bytes.len() as u64));
    assert!(String::from_utf8_lossy(&record.refs).contains("\"original\""));
    assert_eq!(fs::read(blob_file(&path, &bytes)).unwrap(), bytes);

    // Opened, it is named and not in memory, even with room for every body.
    let mut s = Session::new(open(&path).unwrap().mailbox);
    s.path = Some(path.clone());
    let m = s.mailbox.message(id).unwrap();
    assert!(m.has_original() && m.original_bytes().is_none() && !m.is_loaded());
    assert_eq!(m.original.as_ref().map(Original::size), Some(bytes.len() as u64));
    assert_eq!(s.content(id, crate::Part::Original).unwrap().as_slice(), bytes.as_slice());
    // Written whole (Save Message As), its body is the original's byte for byte, under the
    // original's own Content-Type.
    let saved = s.run("file.exportMessage", &serde_json::json!({"id": id})).unwrap();
    let data = mailcraft_charset::base64::STANDARD.decode(saved["data"].as_str().unwrap()).unwrap();
    let body = |b: &[u8]| b.windows(4).position(|w| w == b"\r\n\r\n").map(|at| b[at + 4..].to_vec()).unwrap();
    assert_eq!(body(&data), body(&bytes));
    let text = String::from_utf8_lossy(&data);
    assert!(
        text.contains("\r\nContent-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"b\"\r\n\r\n"),
        "{text}"
    );
    // A self-contained copy of the mailbox carries it.
    let loaded = s.loaded_mailbox().unwrap();
    assert_eq!(loaded.message(id).unwrap().original_bytes(), Some(bytes.as_slice()));
    assert_eq!(whole(&path).message(id).unwrap().original_bytes(), Some(bytes.as_slice()));

    // Brought in and safely on disk, it can be let go of again, and is named once more.
    let mut m = Message::clone(s.mailbox.message(id).unwrap());
    m.original = Original::new(bytes.clone());
    let lean = evictable(&path, id, &m).expect("an original safely on disk can be let go of");
    assert!(lean.original_bytes().is_none() && lean.has_original());

    // A save of the message changed (a read mark) names what the file named; a sweep keeps it.
    let mut again = Session::new(open(&path).unwrap().mailbox);
    again.mailbox.message_mut(id).unwrap().read = true;
    save(&path, &again.mailbox).unwrap();
    sweep(&path, Duration::ZERO).unwrap();
    clear_cache();
    assert_eq!(whole(&path).message(id).unwrap().original_bytes(), Some(bytes.as_slice()));

    // Back to its headers and preview, the message no longer names it.
    let mut thin = Message::clone(whole(&path).message(id).unwrap());
    let before = content_names(id, &Arc::new(thin.clone()));
    crate::cmd::graph_down::thin_message(&mut thin, "preview".into());
    let after = content_names(id, &Arc::new(thin.clone()));
    let r = ContentRef { sha256: Digest::of(&bytes).0, len: bytes.len() as u64 };
    assert!(before.contains(&r) && !after.contains(&r));
    assert!(thin.original.is_none());
}

/// [`with_signed`], in an IMAP account's Inbox as UID 7.
fn with_signed_on_imap() -> (Mailbox, Id) {
    let (mut mb, id) = with_signed();
    let account = mb.message(id).unwrap().account;
    mb.account_mut(account).unwrap().kind = mailcraft_model::AccountKind::Imap;
    mb.message_mut(id).unwrap().uid = 7;
    (mb, id)
}

/// A build from before #30 swept the original's file: the message opens whole, is written from
/// its parts, and asks its server for the bytes again (Check again's mark), with nothing said to
/// be unavailable.
#[test]
fn a_missing_original_is_asked_for_again() {
    let (mb, id) = with_signed_on_imap();
    let tmp = Temp::new("original-missing");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    fs::remove_file(blob_file(&path, &signed_bytes())).unwrap();
    let opened = open(&path).unwrap();
    assert_eq!(opened.unavailable, 0);
    assert!(opened.notice().is_none(), "{:?}", opened.notice());
    let m = opened.mailbox.message(id).unwrap();
    assert_ne!(m.body.text, UNAVAILABLE);
    assert!(m.original.is_none());
    assert!(m.original_wanted && m.refetch);
    // Written whole meanwhile, it is built from its parts.
    let written = String::from_utf8_lossy(&mailcraft_mime::write(m, &Default::default())).into_owned();
    assert!(written.contains("multipart/") && written.contains("Signed text"), "{written}");
    // What the server gives again is the original once more.
    let mut mb = opened.mailbox;
    crate::cmd::imap_again::take_again(&mut mb, id, mailcraft_mime::parse_fetched(&signed_bytes()));
    assert_eq!(mb.message(id).unwrap().original_bytes(), Some(signed_bytes().as_slice()));
}

#[test]
fn an_original_that_cant_be_read_writes_the_message_from_its_parts_and_is_asked_for_again() {
    let (mb, id) = with_signed_on_imap();
    let tmp = Temp::new("original-unreadable");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = Session::new(with_budget(1, || open(&path).unwrap()).mailbox);
    s.path = Some(path.clone());
    assert!(s.mailbox.message(id).unwrap().has_original());
    // Gone after the opening looked for it.
    fs::remove_file(blob_file(&path, &signed_bytes())).unwrap();
    clear_cache();
    let m = s.loaded_message(id).unwrap();
    assert!(m.original.is_none());
    assert!(!mailcraft_mime::write(&m, &Default::default()).is_empty());
    // At the next command it lets the original go and asks for it again.
    s.ask_for_lost_originals();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.original.is_none() && m.original_wanted && m.refetch);
}

#[test]
fn a_one_file_mailbox_carries_the_original() {
    let (mb, id) = with_signed();
    let tmp = Temp::new("original-format1");
    let path = tmp.file();
    fs::write(&path, crate::io::write_mailbox(&mb).unwrap()).unwrap();
    let opened = whole(&path);
    assert_eq!(opened.message(id).unwrap().original_bytes(), Some(signed_bytes().as_slice()));
}
