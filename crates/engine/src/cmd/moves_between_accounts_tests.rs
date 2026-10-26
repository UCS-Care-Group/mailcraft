// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail moved between accounts (#451): out of an IMAP account into a Microsoft account or onto
//! this computer, and between two Microsoft accounts. The servers' side of each run is written
//! out by hand, as `mailcraft-net` answers it. (Against the fake IMAP server and the Graph
//! stand-in: `apps/mailcraft-cli/tests/moves_between_accounts.rs`.)

use mailcraft_model::sync::{Deleted, FolderDelta, FolderListing, GraphFolder, GraphMailResult, MessageOutcome, PlaceDone, RemoteOp, SyncResult};
use mailcraft_model::{Address, Body, FolderKind, Id, Message, Pending, Queued};
use serde_json::json;

use crate::Session;
use crate::cmd::account::{apply_sync, sync_request};
use crate::cmd::graph_mail::{apply_graph_mail, graph_mail_request};

/// A message as it comes down from an IMAP server.
fn from_server(uid: u32, subject: &str) -> Message {
    Message {
        uid,
        subject: subject.into(),
        message_id: format!("{}@mail.example", subject.to_lowercase()),
        from: Address::new("", "x@example.org"),
        date: 100 + i64::from(uid),
        body: Body { text: format!("{subject}, the whole of it."), html: None },
        ..Default::default()
    }
}

/// A session with a local account and an IMAP account whose Inbox follows "INBOX", with "One",
/// "Two" and "Three" (UIDs 1 to 3) in it, "Big" (UID 4), of which only the headers are here, and
/// an Archive: the session, the IMAP account and the local account's Inbox.
fn imap() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let a = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let big = Message { not_downloaded: 90_000_000, body: Body::default(), ..from_server(4, "Big") };
    let new = vec![from_server(1, "One"), from_server(2, "Two"), from_server(3, "Three"), big];
    let delta = |remote: &str, kind, uid_validity, new: Vec<Message>| {
        let uid_seen = new.iter().map(|m| m.uid).max().unwrap_or(0);
        FolderDelta { remote: remote.into(), kind, uid_validity, uid_seen, new, ..Default::default() }
    };
    let first = SyncResult {
        account: a,
        folders: vec![delta("INBOX", FolderKind::Inbox, 7, new), delta("Archive", FolderKind::Archive, 8, vec![])],
        ..Default::default()
    };
    apply_sync(&mut s, first).unwrap();
    let kept = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    (s, a, kept)
}

fn titled(s: &Session, subject: &str) -> Id {
    s.mailbox.messages.values().find(|m| m.subject == subject).unwrap().id
}

fn ops(s: &Session, account: Id) -> Vec<RemoteOp> {
    sync_request(s, account).unwrap().ops
}

fn delete(uid: u32) -> RemoteOp {
    RemoteOp::Delete { from: "INBOX".into(), uid }
}

/// The UIDs a request says the mailbox has of the account's Inbox: what doesn't come down again.
fn known(s: &Session, account: Id) -> Vec<u32> {
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let mut uids = sync_request(s, account).unwrap().folders.into_iter().find(|f| f.id == inbox).unwrap().known_uids;
    uids.sort_unstable();
    uids
}

/// The IMAP server deleted these from its Inbox.
fn deleted(s: &mut Session, account: Id, uids: &[u32]) {
    let deleted = uids.iter().map(|uid| Deleted { from: "INBOX".into(), uid: *uid }).collect();
    apply_sync(s, SyncResult { account, deleted, ..Default::default() }).unwrap();
}

/// Where a message is, and what it is to its IMAP server there.
fn at(s: &Session, id: Id) -> (Id, Id, u32) {
    let m = s.mailbox.message(id).unwrap();
    (m.account, m.folder, m.uid)
}

#[test]
fn moved_from_an_imap_account_to_this_computer_it_goes_from_the_server_once_it_is_whole_in_the_file() {
    let (mut s, a, kept) = imap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let local = s.mailbox.folder(kept).unwrap().account;
    let (one, two, three, big) = (titled(&s, "One"), titled(&s, "Two"), titled(&s, "Three"), titled(&s, "Big"));
    s.run("mail.move", &json!({"ids": [two, big], "folder": kept})).unwrap();
    assert_eq!(at(&s, two), (local, kept, 0));
    // Not in the mailbox file yet: the server keeps it, and it doesn't come down again meanwhile.
    assert_eq!(ops(&s, a), []);
    assert_eq!(known(&s, a), [1, 2, 3]);
    let dir = std::env::temp_dir().join(format!("mailcraft-451-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("moved.mailcraft");
    s.run("file.save", &json!({"path": path})).unwrap();
    // Saved whole: the server's goes. Of "Big", only the headers are here: the server's is all
    // there is of it, and it stays.
    assert_eq!(ops(&s, a), [delete(2)]);
    deleted(&mut s, a, &[2]);
    assert_eq!((ops(&s, a), at(&s, two)), (vec![], (local, kept, 0)));
    assert!(s.mailbox.pending.is_empty(), "{:?}", s.mailbox.pending);

    // A host that saves by other means says so: what changes after that doesn't hold it up.
    s.run("mail.move", &json!({"ids": [one], "folder": kept})).unwrap();
    s.note_saved();
    s.run("mail.markRead", &json!({"ids": [three]})).unwrap();
    assert!(s.dirty);
    assert!(ops(&s, a).contains(&delete(1)), "{:?}", ops(&s, a));

    // Moved back before it went: it is the server's message again, and nothing is deleted.
    s.run("mail.move", &json!({"ids": [one], "folder": inbox})).unwrap();
    assert_eq!(at(&s, one), (a, inbox, 1));
    assert!(!ops(&s, a).iter().any(|op| matches!(op, RemoteOp::Delete { .. })), "{:?}", ops(&s, a));
    let _ = std::fs::remove_dir_all(dir);
}

const OWNER: &str = "owner@example.org";

/// A Microsoft account whose server lists an Inbox ("in") and an Archive ("ar").
fn microsoft(s: &mut Session, email: &str) -> Id {
    let account = s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("in", "Inbox", FolderKind::Inbox), folder("ar", "Archive", FolderKind::Archive)];
    let listing = GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
    apply_graph_mail(s, listing).unwrap();
    account
}

/// What became of the copies the Microsoft account `account` was asked to make, one outcome each.
fn placed(s: &mut Session, account: Id, email: &str, outcomes: &[MessageOutcome]) {
    let asked = graph_mail_request(s, account).unwrap();
    assert_eq!(asked.places.len(), outcomes.len(), "{:?}", asked.places);
    let placed = asked.places.iter().zip(outcomes).map(|(p, outcome)| PlaceDone { change: p.without_data(), outcome: outcome.clone() }).collect();
    apply_graph_mail(s, GraphMailResult { account, email: email.into(), placed, ..Default::default() }).unwrap();
}

fn made(id: &str) -> MessageOutcome {
    MessageOutcome::Done { categories: None, flag: None, folder: "in".into(), id: id.into() }
}

#[test]
fn moved_from_an_imap_account_into_a_microsoft_account_the_original_goes_once_the_copy_is_made() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (theirs, archive) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(ms, FolderKind::Archive).unwrap());
    let (one, two) = (titled(&s, "One"), titled(&s, "Two"));
    s.mailbox.message_mut(two).unwrap().read = true;
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    assert_eq!(at(&s, two), (ms, theirs, 0));
    let asked = graph_mail_request(&s, ms).unwrap();
    assert!(matches!(asked.places.as_slice(), [p] if p.message == two && p.moved && p.to == "in"), "{:?}", asked.places);
    // Nothing goes from the IMAP server until the copy is there, however saved this is.
    s.note_saved();
    assert_eq!((ops(&s, a), known(&s, a)), (vec![], vec![1, 2, 3, 4]));
    placed(&mut s, ms, OWNER, &[made("AAkA2")]);
    assert_eq!(s.mailbox.message(two).and_then(|m| m.server.as_ref()).map(|x| x.id.as_str()), Some("AAkA2"));
    assert_eq!(ops(&s, a), [delete(2)]);
    deleted(&mut s, a, &[2]);
    assert!(s.mailbox.pending.is_empty() && ops(&s, a).is_empty(), "{:?}", s.mailbox.pending);

    // Moved on in the Microsoft account before it was made: the copy is made where it is now, and
    // the original still goes only once it is.
    s.run("mail.move", &json!({"ids": [one], "folder": theirs})).unwrap();
    s.run("mail.move", &json!({"ids": [one], "folder": archive})).unwrap();
    let asked = graph_mail_request(&s, ms).unwrap();
    assert!(matches!(asked.places.as_slice(), [p] if p.message == one && p.to == "ar"), "{:?}", asked.places);
    assert!(matches!(s.mailbox.pending.as_slice(), [Pending::Move { account, uid: 1, .. }] if *account == a), "{:?}", s.mailbox.pending);
    assert_eq!(ops(&s, a), []);
    // And moved back: the IMAP server's message again, with nothing made and nothing deleted.
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    s.run("mail.move", &json!({"ids": [one], "folder": inbox})).unwrap();
    assert_eq!(at(&s, one), (a, inbox, 1));
    assert!(s.mailbox.pending.is_empty() && graph_mail_request(&s, ms).unwrap().places.is_empty());
    assert_eq!(ops(&s, a), []);
}

#[test]
fn a_move_from_an_imap_account_that_microsoft_refuses_goes_back_as_the_servers_message() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let three = titled(&s, "Three");
    let synced = s.mailbox.message(three).unwrap().synced;
    assert!(synced.is_some());
    s.run("mail.move", &json!({"ids": [three], "folder": theirs})).unwrap();
    placed(&mut s, ms, OWNER, &[MessageOutcome::Refused { status: 403 }]);
    // Back in the folder that still has it, as UID 3 there, and nothing was deleted.
    let m = s.mailbox.message(three).unwrap();
    assert_eq!((m.account, m.folder, m.uid, m.synced), (a, inbox, 3, synced));
    assert!(s.mailbox.pending.is_empty() && ops(&s, a).is_empty(), "{:?}", s.mailbox.pending);
    assert!(s.sync_status.contains("Couldn't move"), "{}", s.sync_status);
}

#[test]
fn a_copy_from_an_imap_account_into_a_microsoft_account_leaves_the_original_alone() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let one = titled(&s, "One");
    let copy = s.run("mail.copy", &json!({"ids": [one], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    assert!(
        s.mailbox.queued.iter().any(|q| matches!(q, Queued::PlaceMessage { message, copy: true, .. } if *message == copy)),
        "{:?}",
        s.mailbox.queued
    );
    placed(&mut s, ms, OWNER, &[made("AAkA1")]);
    s.note_saved();
    assert!(s.mailbox.pending.is_empty() && ops(&s, a).is_empty());
    assert_eq!(at(&s, one), (a, s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), 1));
}

#[test]
fn a_message_only_the_headers_of_which_are_here_doesnt_go_to_microsoft() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let big = titled(&s, "Big");
    let before = s.mailbox.message(big).cloned();
    for command in ["mail.move", "mail.copy"] {
        let e = s.run(command, &json!({"ids": [big], "folder": theirs})).unwrap_err().to_string();
        assert!(e.contains("Only the headers of \"Big\" are here"), "{command}: {e}");
    }
    assert_eq!(s.mailbox.message(big).cloned(), before);
    assert!(s.mailbox.pending.is_empty() && s.mailbox.queued.is_empty() && ops(&s, a).is_empty());
}

#[test]
fn mail_moves_between_two_microsoft_accounts_of_ones_own() {
    let mut s = Session::empty();
    let mine = microsoft(&mut s, OWNER);
    let other = microsoft(&mut s, "owner@outlook.example");
    let inbox = s.mailbox.folder_of_kind(mine, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    let server = mailcraft_model::sync::ServerMessage { id: "AAkA7".into(), folder: "in".into(), ..Default::default() };
    let m = s.mailbox.add_message(Message { folder: inbox, subject: "Receipt".into(), server: Some(server), ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": theirs})).unwrap();
    let moved = s.mailbox.message(m).unwrap();
    assert_eq!((moved.account, moved.moving_out.as_ref().map(|o| (o.account, o.id.as_str()))), (other, Some((mine, "AAkA7"))));
    // Made by the other's run; the first's request takes the original away only then.
    assert!(graph_mail_request(&s, mine).unwrap().messages.is_empty());
    placed(&mut s, other, "owner@outlook.example", &[made("AAkB1")]);
    let asked = graph_mail_request(&s, mine).unwrap().messages;
    assert!(matches!(asked.as_slice(), [mailcraft_model::sync::MessageChange::MovedOut { id, .. }] if id == "AAkA7"), "{asked:?}");
}

#[test]
fn a_refused_move_from_this_computer_goes_back_to_its_folder() {
    let (mut s, _, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let local = s.mailbox.folder(kept).unwrap().account;
    let notes = s.mailbox.add_folder(local, None, "Notes").unwrap();
    let m = s
        .mailbox
        .add_message(Message { folder: notes, subject: "Plans".into(), body: Body { text: "All of it.".into(), html: None }, ..Default::default() })
        .unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": theirs})).unwrap();
    placed(&mut s, ms, OWNER, &[MessageOutcome::Refused { status: 400 }]);
    assert_eq!(at(&s, m), (local, notes, 0));
}

#[test]
fn a_move_out_of_an_imap_account_is_kept_when_the_mailbox_is_opened_again() {
    let (mut s, _, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    s.run("mail.move", &json!({"ids": [titled(&s, "Two")], "folder": kept})).unwrap();
    s.run("mail.move", &json!({"ids": [titled(&s, "Three")], "folder": theirs})).unwrap();
    assert_eq!(s.mailbox.pending.len(), 2);
    // What opening the file does to it: the moves waiting are what they were.
    let mut opened: mailcraft_model::Mailbox = serde_json::from_value(serde_json::to_value(&s.mailbox).unwrap()).unwrap();
    assert_eq!(opened.repair(), Vec::<String>::new());
    assert_eq!(opened.pending, s.mailbox.pending);
}
