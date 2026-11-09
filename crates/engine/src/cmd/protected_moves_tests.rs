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
        assert_eq!(crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).get(&inbox).map(Vec::as_slice), Some(&[1u32][..]));
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
/// The message moved through a Microsoft account (made "AAkA2" there, the IMAP copy gone) and
/// on into the second IMAP account, its original's file gone: the session, the second IMAP
/// account, the Microsoft account, the message and the folder to remove after.
fn moved_out_of_microsoft(name: &str) -> (Session, Id, Id, Id, std::path::PathBuf) {
    use mailcraft_model::sync::{GraphMailResult, MessageOutcome, PlaceDone};
    let (mut s, from, to, id) = two_imap();
    let ms = microsoft(&mut s, "owner@example.org");
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap()})).unwrap();
    let asked = crate::cmd::graph_mail::graph_mail_request(&s, ms).unwrap();
    let made = MessageOutcome::Done { categories: None, flag: None, folder: "in".into(), id: "AAkA2".into() };
    let placed = asked.places.iter().map(|p| PlaceDone { change: p.without_data(), outcome: made.clone() }).collect();
    let result = GraphMailResult { account: ms, email: "owner@example.org".into(), placed, ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    let deleted = vec![mailcraft_model::sync::Deleted { from: "INBOX".into(), uid: 1 }];
    apply_sync(&mut s, SyncResult { account: from, deleted, ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap()})).unwrap();
    assert!(s.mailbox.message(id).unwrap().moving_out.is_some());
    let (s, dir) = without_original(&s, id, name);
    (s, to, ms, id, dir)
}

/// Out of a Microsoft account into an IMAP one: the Microsoft copy is fetched for the bytes
/// first (the message staying as it is here, and the owner told), and the move goes made of its
/// parts once it brought none.
#[test]
fn a_move_out_of_microsoft_waits_for_the_original_from_its_copy() {
    let (mut s, to, ms, id, dir) = moved_out_of_microsoft("from-ms");
    assert!(sync_request(&s, to).unwrap().places.is_empty());
    s.ask_for_lost_originals();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.original_wanted && !m.unfetched);
    assert!(crate::cmd::graph_down::fetch_queue(&s.mailbox, ms, 10, m.received).contains(&"AAkA2".to_string()));
    // Waiting, the owner is told, once.
    let said = crate::cmd::graph_place::unreadable_waiting(&s, to);
    assert!(matches!(said.as_slice(), [one] if one.contains("asked for from where it came")), "{said:?}");
    assert!(crate::cmd::graph_place::unreadable_waiting(&s, to).is_empty());
    let mut index = Default::default();
    assert!(crate::cmd::graph_down::take_fetched(&mut s.mailbox, &mut index, ms, "AAkA2", &long_unprotected()));
    assert!(s.mailbox.message(id).unwrap().original_asked);
    let asked = sync_request(&s, to).unwrap();
    assert!(matches!(asked.places.as_slice(), [p] if p.message.original_bytes().is_none()), "{:?}", asked.places);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Microsoft copy gone meanwhile (deleted there by the owner): the ask is let go of, and the
/// move goes made of its parts, there being nothing left to hold the bytes.
#[test]
fn a_move_out_of_microsoft_whose_copy_went_goes_made_of_its_parts() {
    let (mut s, to, _, id, dir) = moved_out_of_microsoft("ms-gone");
    assert!(sync_request(&s, to).unwrap().places.is_empty());
    s.ask_for_lost_originals();
    assert!(s.mailbox.message(id).unwrap().original_wanted);
    if let Some(out) = s.mailbox.message_mut(id).unwrap().moving_out.as_mut() {
        out.left = true;
    }
    crate::cmd::mail::check_again::settle(&mut s.mailbox);
    let m = s.mailbox.message(id).unwrap();
    assert!(!m.original_wanted && !m.refetch && !m.unfetched);
    let asked = sync_request(&s, to).unwrap();
    assert!(matches!(asked.places.as_slice(), [p] if p.message.original_bytes().is_none()), "{:?}", asked.places);
    let _ = std::fs::remove_dir_all(&dir);
}

/// [`two_imap`]'s message moved to this computer with its original's file gone, the copy it came
/// from asked for it: the session, the IMAP account, the message and the folder to remove after.
fn moved_here_asking(name: &str) -> (Session, Id, Id, std::path::PathBuf) {
    let (mut s, from, _, id) = two_imap();
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap()})).unwrap();
    let (mut s, dir) = without_original(&s, id, name);
    s.note_saved();
    assert!(!deletes(&s, from));
    s.ask_for_lost_originals();
    assert!(s.mailbox.message(id).unwrap().refetch);
    (s, from, id, dir)
}

/// What the IMAP server says of UID 1 of its Inbox asked for again and not given: `refused` or
/// too large (`too_large`), or neither, as a version before #30 said both.
fn not_given(s: &mut Session, account: Id, refused: bool, too_large: bool) {
    let inbox = FolderDelta {
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 7,
        uid_seen: 1,
        refetch_done: vec![1],
        refetch_refused: if refused { vec![1] } else { vec![] },
        refetch_too_large: if too_large { vec![1] } else { vec![] },
        ..Default::default()
    };
    apply_sync(s, SyncResult { account, folders: vec![inbox], ..Default::default() }).unwrap();
}

/// A server that refuses the bytes (`NO`, a failed fetch) brings nothing: the copy there stays,
/// they are asked for again, ever less often, and an outage of hours never gives them up; given
/// up on only once refused so often over a day, the move goes on waiting, the owner is told why,
/// and Check again asks once more.
#[test]
fn a_refusal_from_the_copy_it_came_from_never_lets_it_go() {
    use mailcraft_model::{ORIGINAL_PATIENCE, ORIGINAL_TRIES};
    for refused in [true, false] {
        let (mut s, from, id, dir) = moved_here_asking(if refused { "refused" } else { "refused-old" });
        let inbox = s.mailbox.folder_of_kind(from, FolderKind::Inbox).unwrap();
        let start = 1_800_000_000;
        s.clock.now = Some(start);
        for _ in 0..8 {
            not_given(&mut s, from, refused, false);
            s.note_saved();
            let m = s.mailbox.message(id).cloned().unwrap();
            assert!(m.refetch && m.original_wanted && !m.original_asked && m.original_tries < ORIGINAL_TRIES, "{refused}");
            assert!(!deletes(&s, from), "{refused}");
            // Not asked again before its time, and asked then.
            assert!(!crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).contains_key(&inbox));
            s.clock.now = Some(m.original_retry);
            assert!(crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).contains_key(&inbox));
        }
        assert!(s.now() - start < ORIGINAL_PATIENCE);
        // A day on, refused again: given up on.
        s.clock.now = Some(start + ORIGINAL_PATIENCE);
        not_given(&mut s, from, refused, false);
        s.note_saved();
        let m = s.mailbox.message(id).cloned().unwrap();
        assert!(!m.refetch && !m.original_asked && m.original_tries == ORIGINAL_TRIES);
        assert!(!deletes(&s, from));
        // Not asked again, and said, by the IMAP account's run, as the message is on this computer.
        s.want_original(id);
        s.ask_for_lost_originals();
        assert!(!s.mailbox.message(id).unwrap().refetch);
        assert!(s.held_notice(&m).is_some() && !s.first_told_held(id, crate::session::Held::NotGiven));
        // Check again asks the copy once more, and what comes of it is said again.
        assert_eq!(crate::cmd::mail::check_again::why_not(&s.mailbox, &m), None);
        s.run("mail.checkAgain", &json!({"id": id})).unwrap();
        let m = s.mailbox.message(id).unwrap();
        assert!(m.refetch && m.original_wanted && m.original_tries == 0 && m.original_refused == 0);
        assert!(s.first_told_held(id, crate::session::Held::NotGiven));
        assert!(!deletes(&s, from));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Too large to take here: given up on at once, the copy where it came from staying.
#[test]
fn a_copy_too_large_to_take_holds_the_move() {
    let (mut s, from, id, dir) = moved_here_asking("too-large");
    not_given(&mut s, from, false, true);
    s.note_saved();
    let m = s.mailbox.message(id).unwrap();
    assert!(!m.refetch && !m.original_asked && m.original_tries == mailcraft_model::ORIGINAL_TRIES);
    assert!(!deletes(&s, from));
    // The owner was told by the IMAP account's run, as the message is on this computer.
    let m = s.mailbox.message(id).cloned().unwrap();
    assert!(s.held_notice(&m).is_some() && !s.first_told_held(id, crate::session::Held::NotGiven));
    assert!(crate::cmd::graph_place::held_for_this_computer(&s, from).is_empty(), "said once");
    let _ = std::fs::remove_dir_all(&dir);
}

/// No room here for the bytes when they came: not asked for again until there is room for as
/// much as they need, and said; asked again then.
#[test]
fn no_room_for_the_original_holds_the_move_until_there_is() {
    let (mut s, from, id, dir) = moved_here_asking("no-room");
    let free = mailcraft_model::MAX_ORIGINALS.saturating_sub(s.mailbox.messages.originals_size());
    // As if they had come with no room: a byte more than there is room for now.
    let m = s.mailbox.message_mut(id).unwrap();
    (m.refetch, m.original_wanted, m.original_no_room) = (false, false, free + 1);
    s.want_original(id);
    s.ask_for_lost_originals();
    assert!(!s.mailbox.message(id).unwrap().refetch);
    let m = s.mailbox.message(id).cloned().unwrap();
    assert!(s.held_notice(&m).flatten().is_some_and(|said| said.contains("no room") && said.contains("Check again")));
    assert!(!deletes(&s, from));
    // Room for them now.
    s.mailbox.message_mut(id).unwrap().original_no_room = free;
    s.want_original(id);
    s.ask_for_lost_originals();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.refetch && m.original_no_room == 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mailbox numbered anew on the server it came from (its UID there not known for now): the
/// copy there still counts, and the move waits until it is known, then asks.
#[test]
fn a_move_out_of_a_mailbox_numbered_anew_waits_for_its_uid() {
    let (mut s, from, to, id) = two_imap();
    s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap()})).unwrap();
    let (mut s, dir) = without_original(&s, id, "renumbered");
    let set_uid = |s: &mut Session, to: u32| {
        for p in &mut s.mailbox.pending {
            if let mailcraft_model::Pending::Move { uid, .. } = p {
                *uid = to;
            }
        }
    };
    set_uid(&mut s, 0);
    assert!(sync_request(&s, to).unwrap().places.is_empty());
    s.ask_for_lost_originals();
    let inbox = s.mailbox.folder_of_kind(from, FolderKind::Inbox).unwrap();
    assert!(s.mailbox.message(id).unwrap().original_wanted);
    assert!(!crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).contains_key(&inbox));
    assert!(sync_request(&s, to).unwrap().places.is_empty());
    set_uid(&mut s, 1);
    assert_eq!(crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).get(&inbox).map(Vec::as_slice), Some(&[1u32][..]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The copy it came from gone on the server (deleted there by the owner): the ask is let go of
/// with the move, and Check again isn't told it was asked for already.
#[test]
fn an_ask_of_a_copy_that_went_is_let_go_of() {
    let (mut s, from, id, dir) = moved_here_asking("source-gone");
    let deleted = vec![mailcraft_model::sync::Deleted { from: "INBOX".into(), uid: 1 }];
    apply_sync(&mut s, SyncResult { account: from, deleted, ..Default::default() }).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(!m.refetch && !m.original_wanted, "{:?}", s.mailbox.pending);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Another mailbox opened in the session starts with nothing asked or said of the last one's.
#[test]
fn another_mailbox_forgets_what_was_asked_and_said() {
    let (mut s, _, id, dir) = moved_here_asking("replaced");
    s.want_original(id);
    let m = s.mailbox.message(id).cloned().unwrap();
    assert!(s.held_notice(&m).is_some());
    s.replace_mailbox(mailcraft_model::Mailbox::default());
    assert!(s.lost_originals.lock().unwrap().is_empty());
    assert!(s.first_told_held(id, crate::session::Held::Asked));
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

/// A listing afresh of the Microsoft folder the copy is in (its link no longer taken) that hasn't
/// come to the copy yet: it may be there still, so the move goes on waiting for the bytes from
/// it, and nothing is let go of; listed after all, it is asked; ended without it, it went.
#[test]
fn a_listing_afresh_that_has_not_ended_never_lets_the_copy_go() {
    use mailcraft_model::sync::{GraphMessage, MessagePage, ServerMessage};
    for listed in [true, false] {
        let (mut s, to, ms, id, dir) = moved_out_of_microsoft(if listed { "afresh-listed" } else { "afresh-gone" });
        assert!(sync_request(&s, to).unwrap().places.is_empty());
        s.ask_for_lost_originals();
        let page = |messages: Vec<GraphMessage>, afresh: bool, caught_up: bool| MessagePage {
            folder: "in".into(),
            messages,
            link: "next".into(),
            afresh,
            caught_up,
            ..Default::default()
        };
        let mut index = Default::default();
        let mut down = Default::default();
        crate::cmd::graph_down::take_page(&mut s.mailbox, &mut index, ms, &page(vec![], true, false), &mut down);
        crate::cmd::mail::check_again::settle(&mut s.mailbox);
        let m = s.mailbox.message(id).unwrap();
        assert!(m.original_wanted && m.moving_out.as_ref().is_some_and(|out| out.left && out.listing), "{listed}");
        assert!(sync_request(&s, to).unwrap().places.is_empty(), "{listed}");
        let changes = crate::cmd::graph_mail::graph_mail_request(&s, ms).unwrap().messages;
        assert!(!changes.iter().any(|c| matches!(c, mailcraft_model::sync::MessageChange::MovedOut { .. })), "{listed}");
        let there = GraphMessage { server: ServerMessage { id: "AAkA2".into(), ..Default::default() }, ..Default::default() };
        let last = page(if listed { vec![there] } else { vec![] }, false, true);
        crate::cmd::graph_down::take_page(&mut s.mailbox, &mut index, ms, &last, &mut down);
        crate::cmd::mail::check_again::settle(&mut s.mailbox);
        let m = s.mailbox.message(id).unwrap();
        assert!(m.moving_out.as_ref().is_some_and(|out| out.left != listed && !out.listing), "{listed}");
        // Listed: still asked for from there, and waited for. Gone: let go of, made of its parts.
        assert_eq!(m.original_wanted, listed);
        assert_eq!(sync_request(&s, to).unwrap().places.is_empty(), listed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// [`two_imap`]'s message moved into `into` (an account of the second IMAP one's, or a Microsoft
/// one made for it), its original's file gone and the copy where it came from not asked: put
/// there all the same, made of its parts, as a placement gone wrong would (forced here). The
/// session, the first IMAP account, the message and the folder to remove after.
fn placed_wrongly(name: &str, microsoft_too: bool) -> (Session, Id, Id, std::path::PathBuf) {
    let (mut s, from, to, id) = two_imap();
    let into = if microsoft_too { microsoft(&mut s, "owner@example.org") } else { to };
    let inbox = s.mailbox.folder_of_kind(into, FolderKind::Inbox).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": inbox})).unwrap();
    let (mut s, dir) = without_original(&s, id, name);
    if microsoft_too {
        use mailcraft_model::sync::{GraphMailResult, MessageOutcome, PlaceDone, PlaceUp};
        let m = s.mailbox.message(id).cloned().unwrap();
        let marker = s.mailbox.queued.iter().find_map(|q| match q {
            mailcraft_model::Queued::PlaceMessage { message, marker, .. } if *message == id => Some(marker.clone()),
            _ => None,
        });
        let change = PlaceUp::of(&m, "in".into(), None, marker.unwrap(), true, None);
        let made = MessageOutcome::Done { categories: None, flag: None, folder: "in".into(), id: "AAkB3".into() };
        let placed = vec![PlaceDone { change, outcome: made }];
        crate::cmd::graph_mail::apply_graph_mail(
            &mut s,
            GraphMailResult { account: into, email: "owner@example.org".into(), placed, ..Default::default() },
        )
        .unwrap();
        assert!(s.mailbox.message(id).unwrap().server.is_some());
    } else {
        let placed = vec![mailcraft_model::sync::Placed { message: id, to_folder: inbox, uid: 5, uid_validity: 7, ..Default::default() }];
        apply_sync(&mut s, SyncResult { account: into, placed, ..Default::default() }).unwrap();
        assert_eq!(s.mailbox.message(id).unwrap().uid, 5);
    }
    s.note_saved();
    (s, from, id, dir)
}

/// Every release of the copy a message was moved out of goes through one gate (#30): a copy put
/// on an IMAP server or Microsoft's, made of its parts while the bytes it came as were lost here
/// and the copy it came from wasn't asked, never lets that copy go, nor turns its move into a
/// deletion; only once that copy brought none. (One put there with the bytes whole may.)
#[test]
fn a_copy_placed_wrongly_never_releases_the_original() {
    for microsoft_too in [false, true] {
        let (mut s, from, id, dir) = placed_wrongly(if microsoft_too { "wrong-ms" } else { "wrong-imap" }, microsoft_too);
        assert!(!deletes(&s, from), "{microsoft_too}");
        assert!(s.mailbox.pending.iter().any(|p| matches!(p, mailcraft_model::Pending::Move { message, .. } if *message == id)));
        s.mailbox.message_mut(id).unwrap().original_asked = true;
        assert!(deletes(&s, from), "{microsoft_too}");
        let _ = std::fs::remove_dir_all(&dir);
    }
    // Put there with the bytes whole: a release as before.
    let (mut s, from, id, dir) = placed_wrongly("whole-imap", false);
    let to = s.mailbox.message(id).unwrap().account;
    s.mailbox.message_mut(id).unwrap().original_placed = to;
    assert!(deletes(&s, from));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Out of a Microsoft account, placed wrongly alike: no moved-out change goes to the Microsoft
/// server until the copy there brought none.
#[test]
fn a_copy_placed_wrongly_never_removes_the_microsoft_original() {
    use mailcraft_model::sync::MessageChange;
    let (mut s, to, ms, id, dir) = moved_out_of_microsoft("wrong-out-of-ms");
    let inbox = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let placed = vec![mailcraft_model::sync::Placed { message: id, to_folder: inbox, uid: 5, uid_validity: 7, ..Default::default() }];
    apply_sync(&mut s, SyncResult { account: to, placed, ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().uid, 5);
    let moved_out =
        |s: &Session| crate::cmd::graph_mail::graph_mail_request(s, ms).unwrap().messages.iter().any(|c| matches!(c, MessageChange::MovedOut { .. }));
    assert!(!moved_out(&s));
    s.mailbox.message_mut(id).unwrap().original_asked = true;
    assert!(moved_out(&s));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Moved to this computer, with everything that decides a placement made to think nothing can
/// hold the bytes (forced: the copy where it came from passed over): the copy there stays until
/// it brought none.
#[test]
fn a_move_here_forced_past_its_wait_never_releases_the_original() {
    let (mut s, from, id, dir) = moved_here_asking("wrong-local");
    let m = s.mailbox.message_mut(id).unwrap();
    (m.original_wanted, m.refetch, m.draft) = (false, false, true);
    let m = s.mailbox.message(id).cloned().unwrap();
    assert!(!s.held_for_original(&m));
    assert!(!deletes(&s, from));
    s.mailbox.message_mut(id).unwrap().original_asked = true;
    assert!(deletes(&s, from));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The bytes a protected message came as lost here after it went through a second mailbox, and
/// it is moved on from there: the copy in the first stays the one it is moved out of, and isn't
/// deleted (#30); the one in the second, which Graph made of its parts, goes. Of an IMAP first
/// mailbox alike: its move out of there stays, and goes on waiting for the bytes from there.
#[test]
fn review_moving_on_from_a_second_microsoft_account_never_deletes_the_first_original() {
    use mailcraft_model::sync::{GraphMailResult, MessageOutcome, PlaceDone};
    use mailcraft_model::{Pending, Queued};
    for imap_first in [false, true] {
        let (mut s, from, to, id) = two_imap();
        let place_in = |s: &mut Session, account: Id, email: &str, made: &str| {
            s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap()})).unwrap();
            let asked = crate::cmd::graph_mail::graph_mail_request(s, account).unwrap();
            let done = MessageOutcome::Done { categories: None, flag: None, folder: "in".into(), id: made.into() };
            let placed = asked.places.iter().map(|p| PlaceDone { change: p.without_data(), outcome: done.clone() }).collect();
            crate::cmd::graph_mail::apply_graph_mail(s, GraphMailResult { account, email: email.into(), placed, ..Default::default() }).unwrap();
        };
        let a = microsoft(&mut s, "first@example.org");
        if !imap_first {
            // Its first mailbox a Microsoft one: there, the IMAP copy gone.
            place_in(&mut s, a, "first@example.org", "AAkA2");
            let deleted = vec![mailcraft_model::sync::Deleted { from: "INBOX".into(), uid: 1 }];
            apply_sync(&mut s, SyncResult { account: from, deleted, ..Default::default() }).unwrap();
            assert!(s.mailbox.message(id).unwrap().moving_out.is_none() && s.mailbox.pending.is_empty());
        }
        let b = microsoft(&mut s, "second@example.org");
        place_in(&mut s, b, "second@example.org", "AAkB3");
        let (mut s, dir) = without_original(&s, id, if imap_first { "on-from-imap" } else { "on-from-ms" });
        s.run("mail.move", &json!({"ids": [id], "folder": s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap()})).unwrap();
        let m = s.mailbox.message(id).unwrap();
        let deleting = |account: Id| s.mailbox.queued.iter().any(|q| matches!(q, Queued::DeleteMessage { account: at, .. } if *at == account));
        assert!(!deleting(a), "{imap_first}");
        assert!(deleting(b), "{imap_first}");
        if imap_first {
            assert!(m.moving_out.is_none(), "{:?}", m.moving_out);
            assert!(s.mailbox.pending.iter().any(|p| matches!(p, Pending::Move { message, .. } if *message == id) && p.account() == from));
            assert_eq!(crate::cmd::mail::check_again::source_copy(&s.mailbox, id), Some((from, true)));
            assert!(!deletes(&s, from));
        } else {
            assert!(m.moving_out.as_ref().is_some_and(|out| out.account == a && out.id == "AAkA2"), "{:?}", m.moving_out);
            assert_eq!(crate::cmd::mail::check_again::source_copy(&s.mailbox, id), Some((a, false)));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A clock that was ahead when a refusal was counted, put right since: the wait it set is no
/// wait (asked at the next run), the refusals are counted from now, and Check again asks at once
/// a move that waits between refusals.
#[test]
fn a_clock_put_back_never_holds_a_move_for_good() {
    let (mut s, from, id, dir) = moved_here_asking("clock-back");
    let inbox = s.mailbox.folder_of_kind(from, FolderKind::Inbox).unwrap();
    let right = 1_800_000_000;
    s.clock.now = Some(right + 30 * 24 * 60 * 60);
    not_given(&mut s, from, true, false);
    s.clock.now = Some(right);
    assert!(crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).contains_key(&inbox));
    not_given(&mut s, from, true, false);
    let m = s.mailbox.message(id).unwrap();
    assert!(m.original_refused == right && m.original_retry > right && m.original_wanted);
    assert!(!crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).contains_key(&inbox));
    // Check again: asked now.
    let m = s.mailbox.message(id).cloned().unwrap();
    assert_eq!(crate::cmd::mail::check_again::why_not(&s.mailbox, &m), None);
    s.run("mail.checkAgain", &json!({"id": id})).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.original_retry == 0 && m.original_tries == 0 && m.original_wanted);
    assert!(crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).contains_key(&inbox));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Whether the bytes are whole here, for a release, is read from the disk at that time: never
/// from a copy kept in memory of a file that has gone since.
#[test]
fn the_gate_reads_the_original_from_the_disk() {
    let (s, _, _, id) = two_imap();
    let file = std::cell::RefCell::new(None);
    let (s, dir) = with_original_file(&s, id, "fresh", |path| *file.borrow_mut() = Some(path.to_path_buf()));
    let m = s.mailbox.message(id).cloned().unwrap();
    assert!(s.readable_whole(&m) && s.original_readable(&m));
    std::fs::remove_file(file.borrow().as_ref().unwrap()).unwrap();
    assert!(s.readable_whole(&m), "read from what memory kept");
    assert!(!s.original_readable(&m));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Microsoft folder the copy is in goes from the server while it is listed afresh: no page
/// of it will come, so the copy went; the move goes made of its parts, and says so.
#[test]
fn a_folder_gone_while_listed_afresh_lets_the_copy_go_and_says_so() {
    use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, MessagePage};
    let (mut s, to, ms, id, dir) = moved_out_of_microsoft("afresh-folder-gone");
    s.ask_for_lost_originals();
    let page = MessagePage { folder: "in".into(), link: "next".into(), afresh: true, ..Default::default() };
    crate::cmd::graph_down::take_page(&mut s.mailbox, &mut Default::default(), ms, &page, &mut Default::default());
    assert!(s.mailbox.message(id).unwrap().moving_out.as_ref().is_some_and(|out| out.listing));
    let folders = vec![GraphFolder { id: "other".into(), name: "Other".into(), kind: FolderKind::Custom, ..Default::default() }];
    let listing = Some(FolderListing { folders, complete: true });
    let said = crate::cmd::graph_mail::apply_graph_mail(
        &mut s,
        GraphMailResult { account: ms, email: "owner@example.org".into(), listing, ..Default::default() },
    )
    .unwrap();
    assert!(said["errors"].to_string().contains("went with its folder"), "{said}");
    let m = s.mailbox.message(id).unwrap();
    assert!(m.moving_out.as_ref().is_some_and(|out| out.left && !out.listing) && !m.original_wanted);
    assert!(matches!(sync_request(&s, to).unwrap().places.as_slice(), [p] if p.message.original_bytes().is_none()));
    let _ = std::fs::remove_dir_all(&dir);
}
