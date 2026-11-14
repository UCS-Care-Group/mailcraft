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
    let mut m = mailcraft_mime_model::parse_fetched(&signed_bytes());
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
    let record = prepare(id, mb.messages.get(&id).unwrap(), &mut HashMap::new(), None).unwrap();
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

    // Back to its headers and preview, the message still names it: its server's copy may be
    // one put there from its parts, which would bring none back.
    let mut thin = Message::clone(whole(&path).message(id).unwrap());
    crate::cmd::graph_down::thin_message(&mut thin, "preview".into());
    let after = content_names(id, &Arc::new(thin.clone()));
    let r = ContentRef { sha256: Digest::of(&bytes).0, len: bytes.len() as u64 };
    assert!(after.contains(&r) && thin.has_original());
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
    // Still named until a download brings it: a look that wrongly found it gone loses nothing.
    assert!(m.has_original() && m.original_bytes().is_none());
    assert!(m.original_wanted && m.refetch);
    // Written whole meanwhile, it is built from its parts.
    let written = String::from_utf8_lossy(&mailcraft_mime_model::write(m, &Default::default())).into_owned();
    assert!(written.contains("multipart/") && written.contains("Signed text"), "{written}");
    // What the server gives again is the original once more.
    let mut mb = opened.mailbox;
    crate::cmd::imap_again::take_again(&mut mb, id, mailcraft_mime_model::parse_fetched(&signed_bytes()));
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
    assert!(!mailcraft_mime_model::write(&m, &Default::default()).is_empty());
    // Nor can it be put elsewhere whole meanwhile (#451: the server's copy stays).
    assert!(!s.readable_whole(s.mailbox.message(id).unwrap()));
    // At the next command, the file surely gone, it asks for the original again, keeping it
    // named, and that is a change to save and to show.
    let (rev, dirty) = (s.rev(), s.dirty);
    assert!(!dirty);
    s.ask_for_lost_originals();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.has_original() && m.original_wanted && m.refetch);
    assert!(s.rev() > rev && s.dirty);
}

/// A file that doesn't read back (its bytes not what they should be, damaged, or held by
/// another program for now) never lets the original go: it stays named, to be read the next
/// time, and is asked for again from the server meanwhile, which takes nothing away.
#[test]
fn an_original_unreadable_for_now_stays_named() {
    let (mb, id) = with_signed_on_imap();
    let tmp = Temp::new("original-for-now");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let file = blob_file(&path, &signed_bytes());
    fs::remove_file(&file).unwrap();
    fs::write(&file, vec![b'z'; signed_bytes().len()]).unwrap();
    let mut s = Session::new(open(&path).unwrap().mailbox);
    s.path = Some(path.clone());
    assert!(s.mailbox.message(id).unwrap().has_original());
    assert!(s.loaded_message(id).unwrap().original.is_none());
    // Asked about twice, it is looked at once.
    assert!(s.loaded_message(id).unwrap().original.is_none());
    assert_eq!(s.lost_originals.lock().unwrap().len(), 1);
    assert!(!s.readable_whole(s.mailbox.message(id).unwrap()));
    let rev = s.rev();
    s.ask_for_lost_originals();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.has_original() && m.original_wanted && m.refetch);
    assert!(s.rev() > rev);
    // Readable again, it is the original once more.
    fs::remove_file(&file).unwrap();
    fs::write(&file, signed_bytes()).unwrap();
    clear_cache();
    assert_eq!(s.loaded_message(id).unwrap().original_bytes(), Some(signed_bytes().as_slice()));
    assert!(s.readable_whole(s.mailbox.message(id).unwrap()));
}

/// A message no server has (brought in from a file, moved to this computer) keeps its original
/// named when the file is gone: nothing could bring it back, and a file put back is found.
#[test]
fn a_local_messages_missing_original_stays_named() {
    let (mb, id) = with_signed();
    let tmp = Temp::new("original-local");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let file = blob_file(&path, &signed_bytes());
    fs::remove_file(&file).unwrap();
    let mut s = Session::new(open(&path).unwrap().mailbox);
    s.path = Some(path.clone());
    let m = s.mailbox.message(id).unwrap();
    assert!(m.has_original() && !m.original_wanted);
    // Written from its parts meanwhile.
    let copy = s.loaded_message(id).unwrap();
    assert!(copy.original.is_none());
    assert!(String::from_utf8_lossy(&mailcraft_mime_model::write(&copy, &Default::default())).contains("Signed text"));
    s.ask_for_lost_originals();
    assert!(s.mailbox.message(id).unwrap().has_original());
    // A save keeps it named, and the file put back is the original again.
    save(&path, &s.mailbox).unwrap();
    fs::write(&file, signed_bytes()).unwrap();
    clear_cache();
    assert_eq!(whole(&path).message(id).unwrap().original_bytes(), Some(signed_bytes().as_slice()));
}

/// Opened for a window that shows at once (#519), the originals are looked for afterwards, with
/// the rest ([`check_parts`]), not while opening: a missing one is told then, and asks then.
#[test]
fn an_opening_for_a_window_looks_for_originals_afterwards() {
    let (mb, id) = with_signed_on_imap();
    let tmp = Temp::new("original-later");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    fs::remove_file(blob_file(&path, &signed_bytes())).unwrap();
    *lock(&shared(&path).state) = State::default();
    let opened = with_budget(1 << 30, || open_as(&path, &OpenProgress::default(), Launch { checks_later: true, rows: Some(3) }).unwrap());
    // Nothing decided while opening.
    assert!(opened.mailbox.message(id).unwrap().has_original());
    assert!(!opened.mailbox.message(id).unwrap().original_wanted);
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.clone());
    let missing = check_parts(&path);
    assert_eq!(missing.count, 0, "an original is counted nowhere");
    s.mark_unavailable(&missing.refs);
    let m = s.mailbox.message(id).unwrap();
    assert!(m.has_original() && m.original_wanted && m.refetch);
    assert!(s.dirty);
}

/// [`with_signed_on_imap`] saved at `path`, the file that keeps its original, and the folder
/// that holds that file.
fn saved_on_imap(path: &Path) -> (Id, PathBuf, PathBuf) {
    let (mb, id) = with_signed_on_imap();
    save(path, &mb).unwrap();
    clear_cache();
    let file = blob_file(path, &signed_bytes());
    let sub = file.parent().unwrap().to_path_buf();
    (id, file, sub)
}

/// Whether the message opened from `path` asked for its original again; it is named either way.
fn asked_at_opening(path: &Path, id: Id) -> bool {
    *lock(&shared(path).state) = State::default();
    clear_cache();
    let opened = open(path).unwrap();
    let m = opened.mailbox.message(id).unwrap();
    assert!(m.has_original());
    m.original_wanted
}

/// Only a content folder found there and whole says a file is gone: an empty one (a drive not
/// mounted where it was), one without the file's own folder, or a file that can't be looked
/// at says nothing, and nothing is asked for.
#[test]
fn an_original_is_gone_only_from_a_content_folder_that_is_whole() {
    let tmp = Temp::new("original-sure");
    let path = tmp.file();
    let (id, file, sub) = saved_on_imap(&path);
    let dir = sub.parent().unwrap().to_path_buf();

    // An empty content folder, as an empty mount point is.
    fs::rename(&dir, tmp.0.join("away")).unwrap();
    fs::create_dir(&dir).unwrap();
    assert!(!asked_at_opening(&path, id));
    fs::remove_dir(&dir).unwrap();
    fs::rename(tmp.0.join("away"), &dir).unwrap();

    // The file's own folder missing, the rest there.
    fs::rename(&sub, tmp.0.join("sub-away")).unwrap();
    assert!(!asked_at_opening(&path, id));
    fs::rename(tmp.0.join("sub-away"), &sub).unwrap();

    // A cloud drive's stand-in in its place: away, not gone.
    fs::remove_file(&file).unwrap();
    let stand_in = sub.join(format!(".{}.icloud", file.file_name().unwrap().to_str().unwrap()));
    fs::write(&stand_in, b"stand-in").unwrap();
    assert!(!asked_at_opening(&path, id));
    fs::remove_file(&stand_in).unwrap();

    // Missing from a content folder that is whole: gone.
    assert!(asked_at_opening(&path, id));
}

/// A file that can't be looked at (its folder can't be read) is no answer.
#[cfg(unix)]
#[test]
fn an_original_that_cant_be_looked_at_isnt_gone() {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = Temp::new("original-denied");
    let path = tmp.file();
    let (id, file, sub) = saved_on_imap(&path);
    fs::remove_file(&file).unwrap();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();
    // (Not where permissions don't hold, as for root.)
    let denied = fs::symlink_metadata(&file).is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied);
    let asked = asked_at_opening(&path, id);
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    if denied {
        assert!(!asked);
    }
}

/// Content of the same bytes found unreadable after the window showed asks for the original
/// again, which takes nothing away: it stays named.
#[test]
fn an_original_sharing_its_file_found_unreadable_is_asked_for_and_stays_named() {
    let tmp = Temp::new("original-shared");
    let path = tmp.file();
    let (id, file, _) = saved_on_imap(&path);
    // The bytes damaged for now: of their length, but not their content.
    fs::remove_file(&file).unwrap();
    fs::write(&file, vec![b'z'; signed_bytes().len()]).unwrap();
    let mut s = Session::new(open(&path).unwrap().mailbox);
    s.path = Some(path.clone());
    let r = s.mailbox.message(id).unwrap().original.as_ref().and_then(|o| o.kept).unwrap();
    s.mark_unavailable(&[r]);
    let m = s.mailbox.message(id).unwrap();
    assert!(m.has_original() && m.original_wanted && m.refetch);
    assert!(s.dirty);
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
