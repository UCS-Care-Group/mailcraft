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

/// [`STAMPED`] long enough that its original and its text are kept outside the mailbox file.
fn long_signed() -> Vec<u8> {
    String::from_utf8_lossy(STAMPED).replace("Pay the invoice.", &"Pay the invoice. ".repeat(100)).into_bytes()
}

/// [`long_signed`] unprotected, as a server's copy made of its parts is.
fn long_unprotected() -> Vec<u8> {
    format!("From: Boss <boss@example.org>\r\nSubject: Signed\r\nMessage-ID: <pm-1@example.org>\r\n\r\n{}\r\n", "Pay the invoice. ".repeat(100))
        .into_bytes()
}

/// A session with two IMAP accounts, the first with [`long_signed`] as UID 1 in its Inbox: the
/// session, the two accounts and the message.
fn two_imap() -> (Session, Id, Id, Id) {
    two_imap_with(&long_signed())
}

/// [`two_imap`] with `bytes` for the message.
fn two_imap_with(bytes: &[u8]) -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let read = mailcraft_model::Message { uid: 1, ..mailcraft_mime::parse_fetched(bytes) };
    let from = imap_account(&mut s, "avery@mail.example", vec![read]);
    let to = imap_account(&mut s, "work@mail.example", vec![]);
    let id = s.mailbox.messages.values().find(|m| m.account == from).unwrap().id;
    (s, from, to, id)
}

/// A Microsoft account whose server lists an Inbox ("in").
fn microsoft(s: &mut Session, email: &str) -> Id {
    use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult};
    let account = s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let listing = GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(s, listing).unwrap();
    account
}

/// `s` saved at a folder of its own (`name`) and opened from there, with the file that kept
/// message `id`'s original gone: the session, and the folder to remove after.
fn without_original(s: &Session, id: Id, name: &str) -> (Session, std::path::PathBuf) {
    with_original_file(s, id, name, |file| std::fs::remove_file(file).unwrap())
}

/// [`without_original`], with `spoil` done to the file that keeps the original instead.
fn with_original_file(s: &Session, id: Id, name: &str, spoil: impl Fn(&std::path::Path)) -> (Session, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("mailcraft-protected-moves-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mailbox.mailcraft");
    crate::store::save(&path, &s.mailbox).unwrap();
    crate::store::clear_cache();
    let mut s = Session::new(crate::store::open(&path).unwrap().mailbox);
    s.path = Some(path.clone());
    let original = s.mailbox.message(id).unwrap().original.as_ref().and_then(|o| o.kept).unwrap();
    let digest: String = original.sha256.iter().map(|b| format!("{b:02x}")).collect();
    spoil(&crate::store::blob_dir(&path).join(&digest[..2]).join(&digest));
    crate::store::clear_cache();
    (s, dir)
}

/// What the IMAP account `account`'s server gives of UID `uid` of its Inbox, downloaded again.
fn refetched(s: &mut Session, account: Id, uid: u32, bytes: &[u8]) {
    let read = mailcraft_model::Message { uid, ..mailcraft_mime::parse_fetched(bytes) };
    let inbox =
        FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: uid, refetched: vec![read], ..Default::default() };
    apply_sync(s, SyncResult { account, folders: vec![inbox], ..Default::default() }).unwrap();
}

/// Whether a request of `account`'s asks its server to delete anything.
fn deletes(s: &Session, account: Id) -> bool {
    sync_request(s, account).unwrap().ops.iter().any(|op| matches!(op, mailcraft_model::sync::RemoteOp::Delete { .. }))
}

/// A signed message moved to another IMAP server whose original can't be read here waits: the
/// copy it came from is asked for the bytes (by its UID there, whatever account the message is
/// in now) and stays. It goes as it came when they come, made of its parts when the server's copy
/// brings none (that copy isn't protected either), and the owner is told once meanwhile.
#[test]
fn a_move_waits_for_the_original_from_the_copy_it_came_from() {
    for protected in [true, false] {
        let (mut s, from, to, id) = two_imap();
        s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap()})).unwrap();
        let (mut s, dir) = without_original(&s, id, "imap");
        assert!(sync_request(&s, to).unwrap().places.is_empty());
        let said = crate::cmd::graph_place::unreadable_waiting(&s, to);
        assert!(matches!(said.as_slice(), [one] if one.contains("waits to go")), "{said:?}");
        assert_eq!(crate::cmd::graph_place::unreadable_waiting(&s, to), Vec::<String>::new(), "told once");
        s.ask_for_lost_originals();
        let m = s.mailbox.message(id).unwrap();
        assert!(m.has_original() && m.original_wanted && m.refetch && m.account == to);
        let inbox = s.mailbox.folder_of_kind(from, FolderKind::Inbox).unwrap();
        assert_eq!(crate::cmd::imap_again::wanted(&s.mailbox, from).get(&inbox).map(Vec::as_slice), Some(&[1u32][..]));
        assert!(!deletes(&s, from));

        let bytes = if protected { long_signed() } else { long_unprotected() };
        refetched(&mut s, from, 1, &bytes);
        let m = s.mailbox.message(id).unwrap();
        assert!(!m.original_wanted && !m.refetch && m.original_asked != protected, "{protected}");
        let asked = sync_request(&s, to).unwrap();
        let [place] = asked.places.as_slice() else { panic!("{:?}", asked.places) };
        if protected {
            assert_eq!(place.message.original_bytes(), Some(long_signed().as_slice()));
        } else {
            // Nothing else holds the bytes: made of its parts, and asked for no more.
            assert!(place.message.original_bytes().is_none());
            s.ask_for_lost_originals();
            assert!(!s.mailbox.message(id).unwrap().refetch);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Into a Microsoft account alike: the IMAP copy it came from is asked first.
#[test]
fn a_move_into_microsoft_waits_for_the_original_from_the_imap_copy() {
    let (mut s, from, _, id) = two_imap();
    let ms = microsoft(&mut s, "owner@example.org");
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap()})).unwrap();
    let (mut s, dir) = without_original(&s, id, "to-ms");
    assert!(crate::cmd::graph_mail::graph_mail_request(&s, ms).unwrap().places.is_empty());
    s.ask_for_lost_originals();
    assert!(s.mailbox.message(id).unwrap().refetch);
    assert!(!deletes(&s, from));
    refetched(&mut s, from, 1, &long_unprotected());
    assert_eq!(crate::cmd::graph_mail::graph_mail_request(&s, ms).unwrap().places.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Out of a Microsoft account into an IMAP one: the Microsoft copy is fetched for it first.
#[test]
fn a_move_out_of_microsoft_waits_for_the_original_from_its_copy() {
    use mailcraft_model::sync::{GraphMailResult, MessageOutcome, PlaceDone};
    let (mut s, from, to, id) = two_imap();
    let ms = microsoft(&mut s, "owner@example.org");
    // Into Microsoft's server first (made "AAkA2"), the IMAP copy gone.
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap()})).unwrap();
    let asked = crate::cmd::graph_mail::graph_mail_request(&s, ms).unwrap();
    let made = MessageOutcome::Done { categories: None, flag: None, folder: "in".into(), id: "AAkA2".into() };
    let placed = asked.places.iter().map(|p| PlaceDone { change: p.without_data(), outcome: made.clone() }).collect();
    crate::cmd::graph_mail::apply_graph_mail(
        &mut s,
        GraphMailResult { account: ms, email: "owner@example.org".into(), placed, ..Default::default() },
    )
    .unwrap();
    let deleted = vec![mailcraft_model::sync::Deleted { from: "INBOX".into(), uid: 1 }];
    apply_sync(&mut s, SyncResult { account: from, deleted, ..Default::default() }).unwrap();
    // Then on into the second IMAP account.
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap()})).unwrap();
    assert!(s.mailbox.message(id).unwrap().moving_out.is_some());
    let (mut s, dir) = without_original(&s, id, "from-ms");
    assert!(sync_request(&s, to).unwrap().places.is_empty());
    s.ask_for_lost_originals();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.original_wanted && m.unfetched);
    assert!(crate::cmd::graph_down::fetch_queue(&s.mailbox, ms, 10, m.received).contains(&"AAkA2".to_string()));
    let mut index = Default::default();
    assert!(crate::cmd::graph_down::take_fetched(&mut s.mailbox, &mut index, ms, "AAkA2", &long_unprotected()));
    assert!(s.mailbox.message(id).unwrap().original_asked);
    let asked = sync_request(&s, to).unwrap();
    assert!(matches!(asked.places.as_slice(), [p] if p.message.original_bytes().is_none()), "{:?}", asked.places);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Moved to this computer: the server's copy goes only once it was asked for the bytes, and
/// brought none (or brought them, and they are here).
#[test]
fn a_move_to_this_computer_releases_the_copy_only_after_asking_it() {
    let (mut s, from, _, id) = two_imap();
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap()})).unwrap();
    let (mut s, dir) = without_original(&s, id, "local");
    s.note_saved();
    assert!(!deletes(&s, from));
    s.ask_for_lost_originals();
    assert!(s.mailbox.message(id).unwrap().refetch);
    refetched(&mut s, from, 1, &long_unprotected());
    s.note_saved();
    assert!(s.mailbox.message(id).unwrap().original_asked);
    assert!(deletes(&s, from));
    let _ = std::fs::remove_dir_all(&dir);
}

/// One read decides: a copy whose original was read goes with it, and one whose read failed
/// goes made of its parts only when nothing else can hold the bytes.
#[test]
fn a_copy_is_made_of_what_one_read_brought() {
    let (mut s, _, to, id) = two_imap();
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap()})).unwrap();
    let (mut s, dir) = without_original(&s, id, "one-read");
    let m = s.mailbox.message(id).cloned().unwrap();
    assert!(s.placeable(&m).unwrap().is_none());
    s.mailbox.message_mut(id).unwrap().original_asked = true;
    let m = s.mailbox.message(id).cloned().unwrap();
    assert!(s.placeable(&m).unwrap().is_some_and(|whole| whole.original.is_none()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file that can't be read for good (damaged, of its length) and an original that was the only
/// content file kept (so no other file says the content folder is whole) alike: the copy it came
/// from is asked, and the owner told once that the move waits.
#[test]
fn a_move_whose_original_is_damaged_or_alone_asks_and_says_so() {
    // Its text kept in the mailbox file: the original is the one content file.
    let alone =
        String::from_utf8_lossy(STAMPED).replacen("Subject: Signed", &format!("Subject: Signed\r\nX-Pad: {}", "p ".repeat(700)), 1).into_bytes();
    for (bytes, damaged) in [(long_signed(), true), (alone, false)] {
        let (mut s, from, to, id) = two_imap_with(&bytes);
        s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap()})).unwrap();
        let (mut s, dir) = if damaged {
            with_original_file(&s, id, "damaged", |file| {
                let len = std::fs::metadata(file).unwrap().len() as usize;
                std::fs::remove_file(file).unwrap();
                std::fs::write(file, vec![b'z'; len]).unwrap();
            })
        } else {
            without_original(&s, id, "alone")
        };
        assert!(sync_request(&s, to).unwrap().places.is_empty(), "{damaged}");
        let said = crate::cmd::graph_place::unreadable_waiting(&s, to);
        assert!(matches!(said.as_slice(), [one] if one.contains("waits to go")), "{damaged}: {said:?}");
        assert!(crate::cmd::graph_place::unreadable_waiting(&s, to).is_empty(), "{damaged}");
        s.ask_for_lost_originals();
        assert!(s.mailbox.message(id).unwrap().refetch, "{damaged}");
        assert!(!deletes(&s, from), "{damaged}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
