// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail moved between accounts (#451): out of an IMAP account into a Microsoft account or onto
//! this computer, and between two Microsoft accounts. The servers' side of each run is written
//! out by hand, as `mailcraft-net` answers it. (Against the fake IMAP server and the Graph
//! stand-in: `apps/mailcraft-cli/tests/moves_between_accounts.rs`.)

use mailcraft_model::sync::{
    Deleted, FolderDelta, FolderListing, GraphFolder, GraphMailResult, GraphMessage, MessageChange, MessageChangeDone, MessageOutcome, MessagePage,
    PlaceDone, Placed, RemoteFlags, RemoteOp, ServerMessage, SyncRequest, SyncResult,
};
use mailcraft_model::{Address, Body, FolderKind, Id, KeptBody, Message, Pending, Queued};
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
fn a_selection_with_a_message_that_cant_go_to_microsoft_moves_and_copies_none_of_it() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    // "Big" last: "One" and "Two" are on their way before it is refused.
    let ids = [titled(&s, "One"), titled(&s, "Two"), titled(&s, "Big")];
    let before = s.mailbox.clone();
    for command in ["mail.move", "mail.copy"] {
        let e = s.run(command, &json!({"ids": ids, "folder": theirs})).unwrap_err().to_string();
        assert!(e.contains("Only the headers of \"Big\" are here"), "{command}: {e}");
        assert!(s.mailbox == before, "{command}: the mailbox is as it was");
    }
    assert!(ops(&s, a).is_empty() && graph_mail_request(&s, ms).unwrap().places.is_empty());
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

#[test]
fn a_move_out_of_an_imap_account_whose_copy_is_there_already_is_kept_when_the_mailbox_is_opened_again() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let b = second_imap(&mut s);
    let (theirs, others) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    // "One" is on the other IMAP server already, as UID 50 there: its original waits to be deleted.
    let one = titled(&s, "One");
    s.run("mail.move", &json!({"ids": [one], "folder": others})).unwrap();
    let asked = sync_request(&s, b).unwrap();
    apply_sync(&mut s, SyncResult { account: b, placed: vec![put(&asked, one, 50, 9)], ..Default::default() }).unwrap();
    // "Two" is on Microsoft's server, where Outlook moved it on into the Archive.
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    placed(&mut s, ms, OWNER, &[made("AAkA2")]);
    let archived = GraphMessage { server: ServerMessage { id: "AAkA2".into(), folder: "ar".into(), ..Default::default() }, ..Default::default() };
    let page = MessagePage { folder: "ar".into(), messages: vec![archived], ..Default::default() };
    apply_graph_mail(&mut s, GraphMailResult { account: ms, email: OWNER.into(), pages: vec![page], ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.message(two).unwrap().folder, s.mailbox.folder_of_kind(ms, FolderKind::Archive).unwrap());
    // Opened again before the IMAP server deleted either: both originals still go.
    let mut opened: mailcraft_model::Mailbox = serde_json::from_value(serde_json::to_value(&s.mailbox).unwrap()).unwrap();
    assert_eq!(opened.repair(), Vec::<String>::new());
    s.mailbox = opened;
    assert_eq!(ops(&s, a), [delete(1), delete(2)]);
}

/// A second IMAP account, whose Inbox follows its server's "INBOX".
fn second_imap(s: &mut Session) -> Id {
    let servers = json!({"email": "blake@other.example", "incoming": {"host": "imap.other.example"}, "outgoing": {"host": "smtp.other.example"}});
    let b = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let inbox = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 9, ..Default::default() };
    apply_sync(s, SyncResult { account: b, folders: vec![inbox], ..Default::default() }).unwrap();
    b
}

#[test]
fn a_refused_move_whose_original_was_deleted_elsewhere_stays_on_this_computer() {
    let gone =
        |uid| FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 4, removed: vec![uid], ..Default::default() };
    let in_imported = |s: &Session, id: Id, local: Id| {
        let m = s.mailbox.message(id).unwrap();
        let f = s.mailbox.folder(m.folder).unwrap();
        assert_eq!((m.account, f.name.as_str()), (local, "Imported"));
        assert!(s.mailbox.pending.is_empty() && s.mailbox.queued.is_empty(), "{:?} {:?}", s.mailbox.pending, s.mailbox.queued);
    };
    // Into another IMAP account, which won't take it, while its original was deleted on a phone.
    let (mut s, a, kept) = imap();
    let local = s.mailbox.folder(kept).unwrap().account;
    let b = second_imap(&mut s);
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap()})).unwrap();
    apply_sync(&mut s, SyncResult { account: a, folders: vec![gone(2)], ..Default::default() }).unwrap();
    apply_sync(&mut s, SyncResult { account: b, unplaced: vec![two], ..Default::default() }).unwrap();
    apply_sync(&mut s, SyncResult { account: a, folders: vec![gone(2)], ..Default::default() }).unwrap();
    in_imported(&s, two, local);
    // Into a Microsoft account, which won't take it.
    let (mut s, a, kept) = imap();
    let local = s.mailbox.folder(kept).unwrap().account;
    let ms = microsoft(&mut s, OWNER);
    let three = titled(&s, "Three");
    s.run("mail.move", &json!({"ids": [three], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap()})).unwrap();
    apply_sync(&mut s, SyncResult { account: a, folders: vec![gone(3)], ..Default::default() }).unwrap();
    placed(&mut s, ms, OWNER, &[MessageOutcome::Refused { status: 403 }]);
    apply_sync(&mut s, SyncResult { account: a, folders: vec![gone(3)], ..Default::default() }).unwrap();
    in_imported(&s, three, local);
}

#[test]
fn a_refused_move_goes_back_to_the_folder_on_this_computer_it_came_from() {
    let (mut s, a, kept) = imap();
    let b = second_imap(&mut s);
    let two = titled(&s, "Two");
    // Onto this computer, saved, and on into the other IMAP account while the first server
    // deletes its original.
    s.run("mail.move", &json!({"ids": [two], "folder": kept})).unwrap();
    s.note_saved();
    assert_eq!(ops(&s, a), [delete(2)]);
    s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap()})).unwrap();
    deleted(&mut s, a, &[2]);
    apply_sync(&mut s, SyncResult { account: b, unplaced: vec![two], ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.message(two).unwrap().folder, kept);
}

#[test]
fn a_message_whose_text_couldnt_be_read_here_leaves_no_server() {
    let unreadable =
        |s: &mut Session, id: Id| s.mailbox.message_mut(id).unwrap().kept = Some(Box::new(KeptBody { failed: true, ..Default::default() }));
    let (mut s, _, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    // Not into another account.
    let one = titled(&s, "One");
    unreadable(&mut s, one);
    let refused = s.run("mail.move", &json!({"ids": [one], "folder": theirs})).unwrap_err();
    assert!(refused.to_string().contains("The text of \"One\" couldn't be read on this computer"), "{refused}");
    // Not put on a server while it waits to go there.
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    unreadable(&mut s, two);
    assert!(graph_mail_request(&s, ms).unwrap().places.is_empty());
    let (mut s, a, kept) = imap();
    let b = second_imap(&mut s);
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap()})).unwrap();
    unreadable(&mut s, two);
    assert!(sync_request(&s, b).unwrap().places.is_empty());
    assert!(ops(&s, a).is_empty(), "{:?}", ops(&s, a));
    // Onto this computer: its original stays on the server, saved or not.
    let three = titled(&s, "Three");
    s.run("mail.move", &json!({"ids": [three], "folder": kept})).unwrap();
    unreadable(&mut s, three);
    s.note_saved();
    assert!(ops(&s, a).is_empty(), "{:?}", ops(&s, a));
}

/// Microsoft's message "Two" (AAkA2, in its Inbox), moved into the IMAP account's Inbox, and what
/// Microsoft's delta then says of its folders (`pages`) before the IMAP server refuses the copy:
/// the session, the Microsoft account and the message.
fn refused_after(pages: Vec<MessagePage>) -> (Session, Id, Id) {
    let (mut s, a, ms, two) = moved_out_of_microsoft(pages);
    apply_sync(&mut s, SyncResult { account: a, unplaced: vec![two], ..Default::default() }).unwrap();
    (s, ms, two)
}

/// [`refused_after`] up to the refusal: the session, the IMAP account, the Microsoft account and
/// the message.
fn moved_out_of_microsoft(pages: Vec<MessagePage>) -> (Session, Id, Id, Id) {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (theirs, inbox) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap());
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    placed(&mut s, ms, OWNER, &[made("AAkA2")]);
    deleted(&mut s, a, &[2]);
    s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
    apply_graph_mail(&mut s, GraphMailResult { account: ms, email: OWNER.into(), pages, ..Default::default() }).unwrap();
    (s, a, ms, two)
}

#[test]
fn a_refused_move_out_of_microsoft_goes_back_only_where_the_original_still_is() {
    let removed = || MessagePage { folder: "in".into(), removed: vec!["AAkA2".into()], ..Default::default() };
    // Deleted on a phone meanwhile: kept on this computer.
    let (s, _, two) = refused_after(vec![removed()]);
    let m = s.mailbox.message(two).unwrap();
    assert_eq!((m.server.as_ref(), s.mailbox.folder(m.folder).unwrap().name.as_str()), (None, "Imported"));
    assert!(s.mailbox.pending.is_empty() && s.mailbox.queued.is_empty(), "{:?} {:?}", s.mailbox.pending, s.mailbox.queued);
    // Moved back into Microsoft instead: made there again, from the whole message here.
    let (mut s, _, ms, two) = moved_out_of_microsoft(vec![removed()]);
    s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap()})).unwrap();
    assert!(s.mailbox.message(two).unwrap().server.is_none());
    assert_eq!(graph_mail_request(&s, ms).unwrap().places.iter().map(|p| p.message).collect::<Vec<_>>(), [two]);
    // Moved into the Archive in Outlook meanwhile (its folders read in either order): back there.
    let archived = || {
        let there = GraphMessage { server: ServerMessage { id: "AAkA2".into(), folder: "ar".into(), ..Default::default() }, ..Default::default() };
        MessagePage { folder: "ar".into(), messages: vec![there], ..Default::default() }
    };
    for pages in [vec![removed(), archived()], vec![archived(), removed()]] {
        let (s, ms, two) = refused_after(pages);
        let m = s.mailbox.message(two).unwrap();
        assert_eq!(m.folder, s.mailbox.folder_of_kind(ms, FolderKind::Archive).unwrap());
        assert_eq!(m.server.as_ref().map(|x| (x.id.as_str(), x.folder.as_str())), Some(("AAkA2", "ar")));
    }
}

/// The IMAP server listed its Inbox (UIDVALIDITY 7) without these UIDs, and with the flags of
/// the others it was asked about, as the first piece of a listing has them.
fn listed(s: &mut Session, account: Id, removed: Vec<u32>) {
    let flags = (1..=4).filter(|uid| !removed.contains(uid)).map(|uid| RemoteFlags { uid, ..Default::default() }).collect();
    let inbox = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 4, removed, flags, ..Default::default() };
    apply_sync(s, SyncResult { account, folders: vec![inbox], ..Default::default() }).unwrap();
}

/// A whole run of the Microsoft account, every folder read or passed over: with nothing new, or
/// (`removed`) with its Inbox's AAkA2 gone.
fn microsoft_listed(s: &mut Session, ms: Id, removed: bool) {
    let pages = if removed { vec![MessagePage { folder: "in".into(), removed: vec!["AAkA2".into()], ..Default::default() }] } else { vec![] };
    apply_graph_mail(s, GraphMailResult { account: ms, email: OWNER.into(), pages, whole: true, ..Default::default() }).unwrap();
}

#[test]
fn put_back_after_a_refused_copy_it_is_kept_should_its_original_go_before_its_server_lists_it_again() {
    let imported = |s: &Session, id: Id| s.mailbox.message(id).and_then(|m| s.mailbox.folder(m.folder)).map(|f| f.name.clone());
    // IMAP: refused, back as the server's UID 2; then the server lists its Inbox (`clean` times
    // with it, the first of which may have been asked for before), and then without it.
    for (clean, kept) in [(0, true), (1, true), (2, false)] {
        let (mut s, a, _) = imap();
        let b = second_imap(&mut s);
        let two = titled(&s, "Two");
        s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap()})).unwrap();
        apply_sync(&mut s, SyncResult { account: b, unplaced: vec![two], ..Default::default() }).unwrap();
        assert_eq!(at(&s, two).2, 2);
        // (Kept in the mailbox file.)
        let opened: mailcraft_model::Mailbox = serde_json::from_value(serde_json::to_value(&s.mailbox).unwrap()).unwrap();
        assert_eq!(opened.message(two).unwrap().returned, 2);
        // (A listing in pieces is one listing: only its first piece names what is still there.)
        let rest = FolderDelta {
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 6,
            new: vec![from_server(6, "Six")],
            ..Default::default()
        };
        apply_sync(&mut s, SyncResult { account: a, folders: vec![rest], ..Default::default() }).unwrap();
        for _ in 0..clean {
            listed(&mut s, a, vec![]);
        }
        listed(&mut s, a, vec![2]);
        assert_eq!(imported(&s, two), kept.then(|| "Imported".to_string()), "after {clean} listings");
    }
    // Microsoft: the same, by whole runs (a folder with nothing new sends no page).
    for (clean, kept) in [(0, true), (1, true), (2, false)] {
        let (mut s, ms, two) = refused_after(vec![]);
        for _ in 0..clean {
            microsoft_listed(&mut s, ms, false);
        }
        microsoft_listed(&mut s, ms, true);
        assert_eq!(imported(&s, two), kept.then(|| "Imported".to_string()), "after {clean} runs");
    }
    // A folder listed afresh without the original before the refusal: kept here too.
    let afresh = MessagePage {
        folder: "in".into(),
        afresh: true,
        caught_up: true,
        link: "https://graph.example.org/delta?$deltatoken=3".into(),
        ..Default::default()
    };
    let (s, _, two) = refused_after(vec![afresh]);
    assert_eq!(imported(&s, two).as_deref(), Some("Imported"));
    // Moved on by its owner, it is the server's message as any other.
    let (mut s, ms, two) = refused_after(vec![]);
    s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Archive).unwrap()})).unwrap();
    assert_eq!(s.mailbox.message(two).unwrap().returned, 0);
    // A copy of one put back is a message of its own, not one put back.
    let (mut s, ms, two) = refused_after(vec![]);
    let local = s.mailbox.local_account();
    let notes = s.mailbox.add_folder(local, None, "Notes").unwrap();
    let gmail = s.mailbox.folders.iter().find(|f| f.remote == "INBOX").unwrap().id;
    for folder in [notes, s.mailbox.folder_of_kind(ms, FolderKind::Archive).unwrap(), gmail] {
        let copy = s.run("mail.copy", &json!({"ids": [two], "folder": folder})).unwrap()["copies"][0].as_u64().unwrap();
        assert_eq!((s.mailbox.message(two).unwrap().returned, s.mailbox.message(copy).unwrap().returned), (2, 0));
    }
}

#[test]
fn undoing_a_move_on_keeps_what_is_known_of_the_original_since() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let other = microsoft(&mut s, "second@example.org");
    let (mut s, two) = {
        let (theirs, inbox) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap());
        let two = titled(&s, "Two");
        s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
        placed(&mut s, ms, OWNER, &[made("AAkA2")]);
        deleted(&mut s, a, &[2]);
        s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
        (s, two)
    };
    // On into the other Microsoft account; meanwhile the first says its copy left its Inbox.
    s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap()})).unwrap();
    microsoft_listed(&mut s, ms, true);
    s.run("edit.undo", &json!({})).unwrap();
    let m = s.mailbox.message(two).unwrap();
    assert_eq!(m.account, a);
    assert!(m.moving_out.as_ref().is_some_and(|out| out.id == "AAkA2" && out.left), "{:?}", m.moving_out);
}

/// A message on this computer, whole, in `folder`.
fn kept_here(s: &mut Session, folder: Id, subject: &str) -> Id {
    let m = Message { folder, uid: 0, read: true, ..from_server(0, subject) };
    s.mailbox.add_message(m).unwrap()
}

/// The IMAP server took the message as `asked` gave it, and said where (`uid`, `uid_validity`; 0
/// when it didn't).
fn put(asked: &SyncRequest, message: Id, uid: u32, uid_validity: u32) -> Placed {
    let place = asked.places.iter().find(|p| p.message.id == message).unwrap();
    let flags = RemoteFlags { draft: false, ..RemoteFlags::of(&place.message, 0) };
    Placed { message, to_folder: place.to_folder, uid, uid_validity, flags }
}

#[test]
fn where_an_imap_server_says_it_put_a_message_is_believed_only_as_far_as_it_can_be() {
    let (mut s, a, kept) = imap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let ids: Vec<Id> = ["Said", "Other life", "Taken", "Unsaid"].iter().map(|subject| kept_here(&mut s, kept, subject)).collect();
    s.run("mail.move", &json!({"ids": ids, "folder": inbox})).unwrap();
    let asked = sync_request(&s, a).unwrap();
    assert_eq!(crate::cmd::account::unpushed(&s, a), 4);
    assert_eq!(
        asked.places.iter().map(|p| (p.message.id, p.to.as_str())).collect::<Vec<_>>(),
        ids.iter().map(|id| (*id, "INBOX")).collect::<Vec<_>>()
    );
    assert!(asked.places.iter().all(|p| p.message.body.text.ends_with("the whole of it.")));
    // A UID of the mailbox the folder follows, which nothing here has: the message's own. One of
    // another life of the mailbox, one that "One" has, and none at all: expected there.
    let placed = vec![put(&asked, ids[0], 50, 7), put(&asked, ids[1], 51, 99), put(&asked, ids[2], 1, 7), put(&asked, ids[3], 0, 0)];
    apply_sync(&mut s, SyncResult { account: a, placed, ..Default::default() }).unwrap();
    let uids: Vec<u32> = ids.iter().map(|id| at(&s, *id).2).collect();
    assert_eq!(uids, [50, 0, 0, 0]);
    assert!(ids.iter().all(|id| s.mailbox.message(*id).is_some_and(|m| m.synced.is_some())));
    assert_eq!(at(&s, titled(&s, "One")), (a, inbox, 1));
    assert!(s.mailbox.queued.is_empty() && s.mailbox.pending.is_empty(), "{:?} {:?}", s.mailbox.queued, s.mailbox.pending);
    assert!(sync_request(&s, a).unwrap().places.is_empty());
    // What is expected is known by its copy when that comes down, by the marker it carries: not
    // here twice.
    let copy = carrying(&asked, ids[3], from_server(52, "Unsaid"));
    let news = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 52, new: vec![copy], ..Default::default() };
    apply_sync(&mut s, SyncResult { account: a, folders: vec![news], ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.messages.values().filter(|m| m.subject == "Unsaid").count(), 1);
    assert_eq!(at(&s, ids[3]), (a, inbox, 52));
    assert!(s.mailbox.message(ids[3]).unwrap().placed.is_empty());
}

/// `m` as the server has the copy `asked` put there of `message`: carrying its marker.
fn carrying(asked: &SyncRequest, message: Id, m: Message) -> Message {
    let marker = asked.places.iter().find(|p| p.message.id == message).unwrap().marker.clone();
    Message { headers: vec![(mailcraft_mime::PLACE_HEADER.to_string(), marker)], ..m }
}

#[test]
fn mail_that_merely_looks_like_a_copy_the_server_didnt_say_where_it_put_is_not_taken_for_it() {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let theirs = s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    let asked = sync_request(&s, b).unwrap();
    apply_sync(&mut s, SyncResult { account: b, placed: vec![put(&asked, two, 0, 0)], ..Default::default() }).unwrap();
    // Somebody else's message with the same Message-ID, the same date and subject, from the
    // same sender: new mail, and the original stays where it is.
    let news = |new: Vec<Message>| {
        let uid_seen = new.iter().map(|m| m.uid).max().unwrap_or(0);
        SyncResult {
            account: b,
            folders: vec![FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 9, uid_seen, new, ..Default::default() }],
            ..Default::default()
        }
    };
    let look_alike = Message { body: Body { text: "Something else.".into(), html: None }, ..from_server(5, "Two") };
    apply_sync(&mut s, news(vec![look_alike])).unwrap();
    assert_eq!(at(&s, two), (b, theirs, 0));
    assert_eq!(s.mailbox.messages.values().filter(|m| m.subject == "Two").count(), 2);
    assert!(!ops(&s, a).contains(&delete(2)), "{:?}", ops(&s, a));
    // The copy carrying its marker: that one is it, and the original goes.
    apply_sync(&mut s, news(vec![carrying(&asked, two, from_server(6, "Two"))])).unwrap();
    assert_eq!(at(&s, two), (b, theirs, 6));
    assert_eq!(s.mailbox.messages.values().filter(|m| m.subject == "Two").count(), 2);
    assert_eq!(ops(&s, a), [delete(2)]);
}

#[test]
fn a_message_moved_on_or_away_while_it_was_put_on_an_imap_server_follows_there() {
    let (mut s, a, kept) = imap();
    let (inbox, archive) = (s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap());
    let servers = json!({"email": "ada@far.example", "incoming": {"host": "imap.far.example"}, "outgoing": {"host": "smtp.far.example"}});
    let b = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let far = FolderDelta {
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 3,
        uid_seen: 9,
        new: vec![from_server(9, "Far")],
        ..Default::default()
    };
    apply_sync(&mut s, SyncResult { account: b, folders: vec![far], ..Default::default() }).unwrap();
    let (moved_on, called_off, refused) =
        (kept_here(&mut s, kept, "Moved on"), kept_here(&mut s, kept, "Called off"), kept_here(&mut s, kept, "Refused"));
    let far = titled(&s, "Far");
    s.run("mail.move", &json!({"ids": [moved_on, called_off, refused, far], "folder": inbox})).unwrap();
    let asked = sync_request(&s, a).unwrap();
    assert_eq!(asked.places.len(), 4);
    // While the server takes them in: two moved on in the account, one back to this computer.
    s.run("mail.move", &json!({"ids": [moved_on, far], "folder": archive})).unwrap();
    s.run("mail.move", &json!({"ids": [called_off], "folder": kept})).unwrap();
    let placed = vec![put(&asked, moved_on, 50, 7), put(&asked, called_off, 51, 7), put(&asked, far, 52, 7)];
    apply_sync(&mut s, SyncResult { account: a, placed, unplaced: vec![refused], ..Default::default() }).unwrap();
    // Moved there from where the server put them; the copy that wasn't wanted goes again; the
    // original of the one from the other server goes from there, being on this one now.
    let pending = |s: &Session, owner: Id| {
        s.mailbox.pending.iter().filter(|p| p.account() == owner).map(|p| p.source()).map(|(f, u)| (f.to_string(), u)).collect::<Vec<_>>()
    };
    assert_eq!(pending(&s, a), [("INBOX".to_string(), 50), ("INBOX".to_string(), 51), ("INBOX".to_string(), 52)]);
    assert!(s.mailbox.pending.iter().any(|p| matches!(p, Pending::Move { message, to, .. } if *message == moved_on && *to == archive)));
    assert!(s.mailbox.pending.iter().any(|p| matches!(p, Pending::Delete { account, uid: 51, .. } if *account == a)));
    assert!(s.mailbox.pending.iter().any(|p| matches!(p, Pending::Delete { account, from, uid: 9, .. } if *account == b && from == "INBOX")));
    // The one the server wouldn't take is back where it was, nothing waiting for it.
    assert_eq!(at(&s, refused), (s.mailbox.folder(kept).unwrap().account, kept, 0));
    assert_eq!(at(&s, called_off), (s.mailbox.folder(kept).unwrap().account, kept, 0));
    assert!(s.mailbox.queued.is_empty(), "{:?}", s.mailbox.queued);
    assert!(sync_request(&s, a).unwrap().places.is_empty());
}

#[test]
fn a_flag_changed_while_the_message_went_on_the_server_goes_up_after_it() {
    let (mut s, a, kept) = imap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let m = kept_here(&mut s, kept, "Read here");
    s.run("mail.move", &json!({"ids": [m], "folder": inbox})).unwrap();
    let asked = sync_request(&s, a).unwrap();
    // Put there read; marked unread here meanwhile.
    s.run("mail.markUnread", &json!({"ids": [m]})).unwrap();
    apply_sync(&mut s, SyncResult { account: a, placed: vec![put(&asked, m, 50, 7)], ..Default::default() }).unwrap();
    assert!(!s.mailbox.message(m).unwrap().read);
    let unread = |op: &RemoteOp| matches!(op, RemoteOp::Flags { uid: 50, read: Some(false), .. });
    assert!(ops(&s, a).iter().any(unread), "{:?}", ops(&s, a));
}

#[test]
fn moved_back_while_its_original_was_being_deleted_it_goes_on_its_server_again() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (theirs, inbox) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap());
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    placed(&mut s, ms, OWNER, &[made("AAkA2")]);
    assert_eq!(ops(&s, a), [delete(2)]);
    // While the IMAP server deletes it, it is moved back there.
    s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
    assert_eq!(at(&s, two), (a, inbox, 2));
    deleted(&mut s, a, &[2]);
    // Only here now: put on the IMAP server again from the whole message. Microsoft's copy goes
    // only once it is there.
    assert_eq!(at(&s, two), (a, inbox, 0));
    let asked = sync_request(&s, a).unwrap();
    assert_eq!(asked.places.iter().map(|p| p.message.id).collect::<Vec<_>>(), [two]);
    let moved_out =
        |s: &Session| graph_mail_request(s, ms).unwrap().messages.iter().any(|c| matches!(c, MessageChange::MovedOut { id, .. } if id == "AAkA2"));
    assert!(!moved_out(&s) && !s.mailbox.queued.iter().any(|q| matches!(q, Queued::DeleteMessage { .. })), "{:?}", s.mailbox.queued);
    apply_sync(&mut s, SyncResult { account: a, placed: vec![put(&asked, two, 60, 7)], ..Default::default() }).unwrap();
    assert_eq!(at(&s, two), (a, inbox, 60));
    assert!(moved_out(&s));
}

#[test]
fn moved_back_into_another_folder_of_its_imap_account_while_its_original_was_deleted_it_goes_there_again() {
    let (mut s, a, kept) = imap();
    let archive = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": kept})).unwrap();
    s.note_saved();
    assert_eq!(ops(&s, a), [delete(2)]);
    // While the IMAP server deletes it, it is moved into the account's Archive.
    s.run("mail.move", &json!({"ids": [two], "folder": archive})).unwrap();
    deleted(&mut s, a, &[2]);
    assert_eq!(at(&s, two), (a, archive, 0));
    assert_eq!(sync_request(&s, a).unwrap().places.iter().map(|p| (p.message.id, p.to.as_str())).collect::<Vec<_>>(), [(two, "Archive")]);
    assert!(s.mailbox.pending.is_empty() && ops(&s, a).is_empty(), "{:?}", s.mailbox.pending);
}

#[test]
fn mail_waiting_for_a_microsoft_folder_that_goes_from_the_server_goes_back_where_it_came_from() {
    let (mut s, a, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (archive, inbox) = (s.mailbox.folder_of_kind(ms, FolderKind::Archive).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap());
    let here = kept_here(&mut s, kept, "From here");
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [here, two], "folder": archive})).unwrap();
    // Archive is deleted in Outlook: a complete listing without it.
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let listing = GraphMailResult {
        account: ms,
        email: OWNER.into(),
        listing: Some(FolderListing { folders: vec![folder("in", "Inbox", FolderKind::Inbox)], complete: true }),
        ..Default::default()
    };
    apply_graph_mail(&mut s, listing).unwrap();
    assert!(s.mailbox.folder(archive).is_none());
    assert_eq!(at(&s, here), (s.mailbox.folder(kept).unwrap().account, kept, 0));
    assert_eq!(at(&s, two), (a, inbox, 2));
    assert!(s.mailbox.queued.is_empty() && s.mailbox.pending.is_empty(), "{:?} {:?}", s.mailbox.queued, s.mailbox.pending);
}

#[test]
fn a_run_takes_so_much_mail_to_put_on_a_server_and_the_rest_waits() {
    let (mut s, a, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (inbox, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap());
    let big = |s: &mut Session, subject: &str| {
        let m = kept_here(s, kept, subject);
        s.mailbox.message_mut(m).unwrap().body.text = "x".repeat(usize::try_from(mailcraft_model::sync::PLACE_BYTES / 2).unwrap());
        m
    };
    let imap_ones: Vec<Id> = (0..3).map(|n| big(&mut s, &format!("Big {n}"))).collect();
    let graph_ones: Vec<Id> = (3..6).map(|n| big(&mut s, &format!("Big {n}"))).collect();
    s.run("mail.move", &json!({"ids": imap_ones, "folder": inbox})).unwrap();
    s.run("mail.move", &json!({"ids": graph_ones, "folder": theirs})).unwrap();
    // Two halves fill a run; the third waits for the next.
    assert_eq!((sync_request(&s, a).unwrap().places.len(), crate::cmd::account::unpushed(&s, a)), (2, 3));
    assert_eq!(graph_mail_request(&s, ms).unwrap().places.len(), 2);
}

#[test]
fn moved_back_into_microsoft_after_the_imap_server_took_it_the_copy_there_goes() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (theirs, inbox) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap());
    // "Two" is Microsoft's message, moved there from the IMAP server.
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    placed(&mut s, ms, OWNER, &[made("AAkA2")]);
    deleted(&mut s, a, &[2]);
    // Into the IMAP account, where the server takes it as UID 50; then back.
    s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
    let asked = sync_request(&s, a).unwrap();
    apply_sync(&mut s, SyncResult { account: a, placed: vec![put(&asked, two, 50, 7)], ..Default::default() }).unwrap();
    assert_eq!(at(&s, two), (a, inbox, 50));
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    // Microsoft's message again, and the IMAP server's copy goes.
    assert_eq!(s.mailbox.message(two).and_then(|m| m.server.as_ref()).map(|x| x.id.as_str()), Some("AAkA2"));
    assert_eq!(ops(&s, a), [delete(50)]);
    deleted(&mut s, a, &[50]);
    assert!(s.mailbox.pending.is_empty() && s.mailbox.queued.is_empty(), "{:?} {:?}", s.mailbox.pending, s.mailbox.queued);
}

/// Microsoft's message "Two", moved into the IMAP account, where the server took it as UID 50;
/// then moved back while Microsoft's run deleted its copy there: the session, the IMAP account,
/// the Microsoft account, the message, and the deletion the run was asked for.
fn moved_back_while_microsoft_deleted_it() -> (Session, Id, Id, Id, MessageChange) {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (theirs, inbox) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap());
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    placed(&mut s, ms, OWNER, &[made("AAkA2")]);
    deleted(&mut s, a, &[2]);
    s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
    let asked = sync_request(&s, a).unwrap();
    apply_sync(&mut s, SyncResult { account: a, placed: vec![put(&asked, two, 50, 7)], ..Default::default() }).unwrap();
    let run = graph_mail_request(&s, ms).unwrap();
    let gone = run.messages.into_iter().find(|c| matches!(c, MessageChange::MovedOut { id, .. } if id == "AAkA2")).unwrap();
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    (s, a, ms, two, gone)
}

/// Microsoft deleted its copy, as asked (`change`), and its delta says so (with `swept`).
fn microsoft_deleted(s: &mut Session, ms: Id, change: MessageChange, swept: bool) {
    let outcome = MessageOutcome::Done { categories: None, flag: None, folder: String::new(), id: String::new() };
    microsoft_answered(s, ms, change, outcome, swept);
}

/// What Microsoft said (`outcome`) of deleting its copy, as asked (`change`).
fn microsoft_answered(s: &mut Session, ms: Id, change: MessageChange, outcome: MessageOutcome, swept: bool) {
    let messages = vec![MessageChangeDone { change, outcome, there: None }];
    let pages = if swept { vec![MessagePage { folder: "in".into(), removed: vec!["AAkA2".into()], ..Default::default() }] } else { vec![] };
    apply_graph_mail(s, GraphMailResult { account: ms, email: OWNER.into(), messages, pages, whole: swept, ..Default::default() }).unwrap();
}

#[test]
fn moved_back_into_microsoft_while_its_copy_there_was_deleted_it_goes_on_that_server_again() {
    let server_id = |s: &Session, two| s.mailbox.message(two).and_then(|m| m.server.as_ref()).map(|x| x.id.clone());
    // The IMAP server deletes its copy first, then Microsoft says its own is gone.
    let (mut s, a, ms, two, gone) = moved_back_while_microsoft_deleted_it();
    deleted(&mut s, a, &[50]);
    microsoft_deleted(&mut s, ms, gone, true);
    assert_eq!((at(&s, two).0, server_id(&s, two)), (ms, None));
    placed(&mut s, ms, OWNER, &[made("AAkA9")]);
    assert_eq!(server_id(&s, two).as_deref(), Some("AAkA9"));
    // Microsoft answers first: the IMAP server's copy stays until Microsoft has one again.
    let (mut s, a, ms, two, gone) = moved_back_while_microsoft_deleted_it();
    microsoft_deleted(&mut s, ms, gone, false);
    assert!(ops(&s, a).is_empty() && server_id(&s, two).is_none(), "{:?}", ops(&s, a));
    placed(&mut s, ms, OWNER, &[made("AAkA9")]);
    assert_eq!(ops(&s, a), [delete(50)]);
    // Microsoft refuses: its copy is still there, and is the message's.
    let (mut s, a, ms, two, gone) = moved_back_while_microsoft_deleted_it();
    microsoft_answered(&mut s, ms, gone, MessageOutcome::Refused { status: 403 }, false);
    assert_eq!(server_id(&s, two).as_deref(), Some("AAkA2"));
    assert!(graph_mail_request(&s, ms).unwrap().places.is_empty());
    assert_eq!(ops(&s, a), [delete(50)]);
}

#[test]
fn removing_an_account_puts_back_the_mail_its_server_wasnt_given_yet() {
    let (mut s, a, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (theirs, inbox) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap());
    let local = s.mailbox.folder(kept).unwrap().account;
    let (here, copied) = (kept_here(&mut s, kept, "From here"), kept_here(&mut s, kept, "Copied"));
    s.run("mail.move", &json!({"ids": [here], "folder": inbox})).unwrap();
    s.run("mail.copy", &json!({"ids": [copied], "folder": inbox})).unwrap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    // The Microsoft account goes: "Two" is the IMAP server's message again.
    s.run("account.remove", &json!({"id": ms})).unwrap();
    assert_eq!(at(&s, two), (a, inbox, 2));
    // The IMAP account goes: what was moved in from this computer is back there; the copy goes
    // with the account, and its source stays.
    s.run("account.remove", &json!({"id": a})).unwrap();
    assert_eq!(at(&s, here), (local, kept, 0));
    assert_eq!(s.mailbox.messages.values().filter(|m| m.subject == "Copied").map(|m| m.folder).collect::<Vec<_>>(), [kept]);
    assert!(s.mailbox.queued.is_empty() && s.mailbox.pending.is_empty(), "{:?} {:?}", s.mailbox.queued, s.mailbox.pending);
}

#[test]
fn what_goes_into_a_microsoft_account_from_elsewhere_is_counted_for_the_window_to_say() {
    let (mut s, a, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (theirs, their_archive) =
        (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(ms, FolderKind::Archive).unwrap());
    let (inbox, archive) = (s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap());
    let (one, two, three) = (titled(&s, "One"), titled(&s, "Two"), titled(&s, "Three"));
    let into = |s: &mut Session, command: &str, ids: &[Id], folder: Id| {
        s.run(command, &json!({"ids": ids, "folder": folder})).unwrap()["intoMicrosoft"].clone()
    };
    // Within its account, or onto this computer: Microsoft makes nothing.
    assert_eq!(into(&mut s, "mail.move", &[one], archive), 0);
    assert_eq!(into(&mut s, "mail.copy", &[two], kept), 0);
    // Into the Microsoft account, moved or copied: Microsoft makes each.
    assert_eq!(into(&mut s, "mail.move", &[one, three], theirs), 2);
    assert_eq!(into(&mut s, "mail.copy", &[two], theirs), 1);
    // On within that account while it waits, or back to where its original is: said already, or
    // not made there.
    assert_eq!(into(&mut s, "mail.move", &[one], their_archive), 0);
    assert_eq!(into(&mut s, "mail.move", &[three], inbox), 0);
}

/// Take out of the content folder of the mailbox file at `path` the file that keeps `bytes`.
fn lose_kept(path: &std::path::Path, bytes: &[u8]) {
    let mut dirs = vec![crate::store::blob_dir(path)];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                dirs.push(p);
            } else if std::fs::read(&p).unwrap() == bytes {
                std::fs::remove_file(&p).unwrap();
                return;
            }
        }
    }
    panic!("no file keeps it");
}

#[test]
fn moved_to_this_computer_its_original_stays_while_a_file_of_it_cant_be_read_here() {
    let (mut s, a, kept) = imap();
    let two = titled(&s, "Two");
    let plan = "The plan, line by line.\n".repeat(200).into_bytes();
    s.mailbox.message_mut(two).unwrap().attachments.push(mailcraft_model::Attachment::new("plan.txt", "text/plain", plan.clone()));
    s.run("mail.move", &json!({"ids": [two], "folder": kept})).unwrap();
    let dir = std::env::temp_dir().join(format!("mailcraft-451-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("moved.mailcraft");
    s.run("file.save", &json!({"path": path})).unwrap();
    // Opened again: the attachment is on disk, read back whole, and the server's copy may go.
    let mut s = Session::empty();
    s.run("file.open", &json!({"path": path})).unwrap();
    assert!(s.mailbox.message(two).unwrap().attachments[0].data.is_empty(), "kept outside memory");
    assert_eq!(ops(&s, a), [delete(2)]);
    // Opened without the file that keeps it: the server's is all there is of it, and it stays.
    lose_kept(&path, &plan);
    crate::store::clear_cache();
    let mut s = Session::empty();
    s.run("file.open", &json!({"path": path})).unwrap();
    assert!(s.content(two, crate::Part::Attachment(0)).is_err());
    assert_eq!(ops(&s, a), []);
    // Counted as waiting without its files being read (a host asks every few seconds).
    assert_eq!(crate::cmd::account::unpushed(&s, a), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn moved_on_from_a_copy_made_in_another_microsoft_account_that_copy_goes_only_once_it_is_safe_where_it_goes() {
    let (mut s, _, kept) = imap();
    let mine = microsoft(&mut s, OWNER);
    let other = microsoft(&mut s, "owner@outlook.example");
    let (inbox, theirs) = (s.mailbox.folder_of_kind(mine, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap());
    let server = mailcraft_model::sync::ServerMessage { id: "AAkA7".into(), folder: "in".into(), ..Default::default() };
    let body = Body { text: "All of it.".into(), html: None };
    let m = s.mailbox.add_message(Message { folder: inbox, subject: "Receipt".into(), server: Some(server), body, ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": theirs})).unwrap();
    placed(&mut s, other, "owner@outlook.example", &[made("AAkB1")]);
    // The first account's run takes the original away now: the copy is there.
    let asked = graph_mail_request(&s, mine).unwrap().messages;
    assert!(matches!(asked.as_slice(), [MessageChange::MovedOut { id, .. }] if id == "AAkA7"), "{asked:?}");
    // Moved on to this computer while that run may be on its way: the copy stands for it on a
    // server until the message is saved here, and the original goes as it would have.
    s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
    assert!(graph_mail_request(&s, other).unwrap().messages.is_empty());
    let asked = graph_mail_request(&s, mine).unwrap().messages;
    assert!(matches!(asked.as_slice(), [MessageChange::Delete { id }] if id == "AAkA7"), "{asked:?}");
    s.note_saved();
    let asked = graph_mail_request(&s, other).unwrap().messages;
    assert!(matches!(asked.as_slice(), [MessageChange::MovedOut { id, .. }] if id == "AAkB1"), "{asked:?}");

    // Back where the original still is: it is the message again, and the copy goes at once.
    let n = s
        .mailbox
        .add_message(Message {
            folder: inbox,
            subject: "Again".into(),
            server: Some(mailcraft_model::sync::ServerMessage { id: "AAkA8".into(), folder: "in".into(), ..Default::default() }),
            body: Body { text: "Again.".into(), html: None },
            ..Default::default()
        })
        .unwrap();
    s.run("mail.move", &json!({"ids": [n], "folder": theirs})).unwrap();
    placed(&mut s, other, "owner@outlook.example", &[made("AAkB2")]);
    s.run("mail.move", &json!({"ids": [n], "folder": inbox})).unwrap();
    let back = s.mailbox.message(n).unwrap();
    assert!(back.server.as_ref().is_some_and(|server| server.id == "AAkA8") && back.moving_out.is_none(), "{back:?}");
    let asked = graph_mail_request(&s, other).unwrap().messages;
    assert!(asked.iter().any(|c| matches!(c, MessageChange::Delete { id } if id == "AAkB2")), "{asked:?}");
    assert!(!graph_mail_request(&s, mine).unwrap().messages.iter().any(|c| matches!(c, MessageChange::Delete { id } if id == "AAkA8")));
}

#[test]
fn moved_back_while_its_original_was_deleted_and_the_answer_lost_it_is_kept_here() {
    let in_folder = |s: &Session, id: Id| s.mailbox.message(id).and_then(|m| s.mailbox.folder(m.folder)).map(|f| f.name.clone());
    // From an IMAP account to this computer and back while the server deletes the original;
    // the answer is lost, and the next listing hasn't got it.
    let (mut s, a, kept) = imap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": kept})).unwrap();
    s.note_saved();
    assert_eq!(ops(&s, a), [delete(2)]);
    s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
    assert_eq!(at(&s, two), (a, inbox, 2));
    let listing =
        FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 4, removed: vec![2], ..Default::default() };
    apply_sync(&mut s, SyncResult { account: a, folders: vec![listing], ..Default::default() }).unwrap();
    assert_eq!(in_folder(&s, two).as_deref(), Some("Imported"));

    // From a Microsoft account and back while its run removes the server's copy, likewise.
    let (mut s, _, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let server = mailcraft_model::sync::ServerMessage { id: "AAkA7".into(), folder: "in".into(), ..Default::default() };
    let body = Body { text: "All of it.".into(), html: None };
    let m = s.mailbox.add_message(Message { folder: theirs, subject: "Receipt".into(), server: Some(server), body, ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
    s.note_saved();
    assert!(!graph_mail_request(&s, ms).unwrap().messages.is_empty());
    s.run("mail.move", &json!({"ids": [m], "folder": theirs})).unwrap();
    let removed = MessagePage { folder: "in".into(), removed: vec!["AAkA7".into()], ..Default::default() };
    apply_graph_mail(&mut s, GraphMailResult { account: ms, email: OWNER.into(), pages: vec![removed], whole: true, ..Default::default() }).unwrap();
    assert_eq!(in_folder(&s, m).as_deref(), Some("Imported"));
}

#[test]
fn undone_while_its_original_was_deleted_and_the_answer_lost_it_is_kept_here() {
    let in_folder = |s: &Session, id: Id| s.mailbox.message(id).and_then(|m| s.mailbox.folder(m.folder)).map(|f| f.name.clone());
    // From an IMAP account to this computer, undone while the server deletes the original; the
    // answer is lost, and the next listing hasn't got it.
    let (mut s, a, kept) = imap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": kept})).unwrap();
    s.note_saved();
    assert_eq!(ops(&s, a), [delete(2)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(at(&s, two).0, a);
    let listing =
        FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 4, removed: vec![2], ..Default::default() };
    apply_sync(&mut s, SyncResult { account: a, folders: vec![listing], ..Default::default() }).unwrap();
    assert_eq!(in_folder(&s, two).as_deref(), Some("Imported"));
    // Undone before the run started, and the server still lists it: it is the server's again.
    let (mut s, a, kept) = imap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": kept})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    let flags = [1, 2, 3, 4].map(|uid| RemoteFlags { uid, ..Default::default() }).to_vec();
    let listing = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 4, flags, ..Default::default() };
    for _ in 0..3 {
        apply_sync(&mut s, SyncResult { account: a, folders: vec![listing.clone()], ..Default::default() }).unwrap();
    }
    assert_eq!(s.mailbox.message(two).map(|m| (m.account, m.uid, m.returned)), Some((a, 2, 0)));

    // From a Microsoft account, undone while its run removes the server's copy, likewise.
    let (mut s, _, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let server = mailcraft_model::sync::ServerMessage { id: "AAkA7".into(), folder: "in".into(), ..Default::default() };
    let body = Body { text: "All of it.".into(), html: None };
    let m = s.mailbox.add_message(Message { folder: theirs, subject: "Receipt".into(), server: Some(server), body, ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
    s.note_saved();
    assert!(!graph_mail_request(&s, ms).unwrap().messages.is_empty());
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(at(&s, m).0, ms);
    let removed = MessagePage { folder: "in".into(), removed: vec!["AAkA7".into()], ..Default::default() };
    apply_graph_mail(&mut s, GraphMailResult { account: ms, email: OWNER.into(), pages: vec![removed], whole: true, ..Default::default() }).unwrap();
    assert_eq!(in_folder(&s, m).as_deref(), Some("Imported"));
}

#[test]
fn mail_waiting_to_go_on_a_server_that_cant_be_read_here_is_said_after_each_run() {
    let (mut s, a, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (gmail, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap());
    // A file of it kept outside memory, which can't be read back (this mailbox has no file).
    let unreadable = |subject: &str| {
        let mut file = mailcraft_model::Attachment::new("plan.pdf", "application/pdf", Vec::new());
        file.kept = Some(mailcraft_model::ContentRef { sha256: [7; 32], len: 4096 });
        Message {
            folder: kept,
            subject: subject.into(),
            body: Body { text: "Plans.".into(), html: None },
            attachments: vec![file],
            ..Default::default()
        }
    };
    let plans = s.mailbox.add_message(unreadable("Plans")).unwrap();
    let notes = s.mailbox.add_message(unreadable("Notes")).unwrap();
    s.run("mail.move", &json!({"ids": [plans], "folder": gmail})).unwrap();
    s.run("mail.move", &json!({"ids": [notes], "folder": theirs})).unwrap();
    assert!(sync_request(&s, a).unwrap().places.is_empty());
    assert!(graph_mail_request(&s, ms).unwrap().places.is_empty());
    let said = apply_sync(&mut s, SyncResult { account: a, ..Default::default() }).unwrap();
    assert_eq!(said["total"]["errors"], json!(["Couldn't put \"Plans\" on the server: it couldn't be read on this computer"]));
    let said = apply_graph_mail(&mut s, GraphMailResult { account: ms, email: OWNER.into(), ..Default::default() }).unwrap();
    assert_eq!(said["errors"], json!(["Couldn't put \"Notes\" on the server: it couldn't be read on this computer"]));
    // It still waits where it was put: nothing of it is lost.
    assert!(s.mailbox.queued.iter().any(|q| matches!(q, Queued::PlaceMessage { message, .. } if *message == plans)));
}

#[test]
fn mail_from_another_mailbox_doesnt_go_into_an_outbox_where_it_would_be_sent() {
    let (mut s, a, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let b = second_imap(&mut s);
    let local = s.mailbox.folder(kept).unwrap().account;
    let notes = s.mailbox.add_folder(local, None, "Notes").unwrap();
    let mine = s
        .mailbox
        .add_message(Message { folder: notes, subject: "Mine".into(), body: Body { text: "Mine.".into(), html: None }, ..Default::default() })
        .unwrap();
    let two = titled(&s, "Two");
    // A Microsoft account's Outbox sends what lies in it: refused.
    let ms_outbox = s.mailbox.folder_of_kind(ms, FolderKind::Outbox).unwrap();
    for command in ["mail.move", "mail.copy"] {
        for id in [two, mine] {
            let refused = s.run(command, &json!({"ids": [id], "folder": ms_outbox})).unwrap_err();
            assert_eq!(refused.to_string(), mailcraft_model::INTO_OUTBOX);
        }
    }
    assert!(s.mailbox.queued.is_empty() && s.mailbox.pending.is_empty());
    assert_eq!((at(&s, two).0, at(&s, mine).1), (a, notes));
    // An IMAP account's holds it, as it does what is moved there within the account (#607).
    let b_outbox = s.mailbox.folder_of_kind(b, FolderKind::Outbox).unwrap();
    s.run("mail.copy", &json!({"ids": [two], "folder": b_outbox})).unwrap();
    s.run("mail.move", &json!({"ids": [mine], "folder": b_outbox})).unwrap();
    assert_eq!(s.mailbox.messages_in(b_outbox).count(), 2);
    assert!(s.mailbox.messages_in(b_outbox).all(|m| m.held == mailcraft_model::MOVED_INTO_OUTBOX));
    assert!(sync_request(&s, b).unwrap().outbox.is_empty());
    // Back into the Outbox of the account it came from, while its original there waits to go:
    // out of Gmail into Microsoft and back, held; out of Microsoft to this computer and back,
    // refused.
    let (theirs, own) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap());
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    let server = mailcraft_model::sync::ServerMessage { id: "AAkA7".into(), folder: "in".into(), ..Default::default() };
    let body = Body { text: "All of it.".into(), html: None };
    let receipt =
        s.mailbox.add_message(Message { folder: theirs, subject: "Receipt".into(), server: Some(server), body, ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [receipt], "folder": kept})).unwrap();
    for command in ["mail.move", "mail.copy"] {
        let refused = s.run(command, &json!({"ids": [receipt], "folder": ms_outbox})).unwrap_err();
        assert_eq!(refused.to_string(), mailcraft_model::INTO_OUTBOX);
    }
    s.run("mail.move", &json!({"ids": [two], "folder": own})).unwrap();
    assert_eq!((at(&s, two).1, at(&s, receipt).1), (own, kept));
    assert_eq!(s.mailbox.message(two).unwrap().held, mailcraft_model::MOVED_INTO_OUTBOX);
    assert!(sync_request(&s, a).unwrap().outbox.is_empty());
}

#[test]
fn a_message_put_in_an_outbox_by_mistake_moves_into_another_account_as_any_other() {
    let (mut s, a, _) = imap();
    let (b, ms) = (second_imap(&mut s), microsoft(&mut s, OWNER));
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    let (two, three) = (titled(&s, "Two"), titled(&s, "Three"));
    s.run("mail.move", &json!({"ids": [two, three], "folder": outbox})).unwrap();
    for (id, to) in [(two, b), (three, ms)] {
        let inbox = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
        s.run("mail.move", &json!({"ids": [id], "folder": inbox})).unwrap();
        assert_eq!(at(&s, id), (to, inbox, 0));
        assert!(s.mailbox.message(id).unwrap().held.is_empty());
    }
    // Its original goes from the first server once the copy is there, as any other's.
    assert!(ops(&s, a).is_empty(), "{:?}", ops(&s, a));
    let asked = sync_request(&s, b).unwrap();
    apply_sync(&mut s, SyncResult { account: b, placed: vec![put(&asked, two, 50, 9)], ..Default::default() }).unwrap();
    placed(&mut s, ms, OWNER, &[made("M1")]);
    assert_eq!(ops(&s, a), [delete(2), delete(3)]);
}

#[test]
fn a_message_put_in_an_outbox_by_mistake_is_copied_into_another_account_and_stays_held() {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let (outbox, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": outbox})).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [two], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    assert!(s.mailbox.message(copy).unwrap().held.is_empty());
    assert_eq!(s.mailbox.message(two).unwrap().held, mailcraft_model::MOVED_INTO_OUTBOX);
    assert_eq!(sync_request(&s, b).unwrap().places.iter().map(|p| p.message.id).collect::<Vec<_>>(), [copy]);
}

#[test]
fn a_draft_and_mail_a_server_wouldnt_send_stay_in_their_own_mailbox() {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let (outbox, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    let (two, three) = (titled(&s, "Two"), titled(&s, "Three"));
    s.mailbox.message_mut(two).unwrap().draft = true;
    s.mailbox.message_mut(three).unwrap().held = "The server wouldn't send it.".into();
    s.run("mail.move", &json!({"ids": [two, three], "folder": outbox})).unwrap();
    for (id, subject) in [(two, "Two"), (three, "Three")] {
        for command in ["mail.move", "mail.copy"] {
            let refused = s.run(command, &json!({"ids": [id], "folder": theirs})).unwrap_err().to_string();
            assert!(refused.contains(&format!("The draft \"{subject}\" stays in its own mailbox")), "{refused}");
        }
    }
}

#[test]
fn a_message_refused_after_leaving_an_outbox_comes_back_held_and_isnt_sent() {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let (outbox, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    // A copy: no server has it, so the Outbox is all it can go back to.
    let two = titled(&s, "Two");
    let copy = s.run("mail.copy", &json!({"ids": [two], "folder": outbox})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.move", &json!({"ids": [copy], "folder": theirs})).unwrap();
    assert!(s.mailbox.message(copy).unwrap().held.is_empty());
    apply_sync(&mut s, SyncResult { account: b, unplaced: vec![copy], ..Default::default() }).unwrap();
    assert_eq!(at(&s, copy), (a, outbox, 0));
    assert_eq!(s.mailbox.message(copy).unwrap().held, mailcraft_model::MOVED_INTO_OUTBOX);
    assert!(sync_request(&s, a).unwrap().outbox.is_empty(), "it would go to everybody it names");
}

#[test]
fn a_message_queued_with_send_and_moved_out_comes_back_held_when_refused() {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let (outbox, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Queued", "account": a})).unwrap();
    let queued = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(sync_request(&s, a).unwrap().outbox.iter().map(|m| m.id).collect::<Vec<_>>(), [queued]);
    s.run("mail.move", &json!({"ids": [queued], "folder": theirs})).unwrap();
    apply_sync(&mut s, SyncResult { account: b, unplaced: vec![queued], ..Default::default() }).unwrap();
    assert_eq!(at(&s, queued), (a, outbox, 0));
    assert_eq!(s.mailbox.message(queued).unwrap().held, mailcraft_model::MOVED_INTO_OUTBOX);
    assert!(sync_request(&s, a).unwrap().outbox.is_empty(), "taken out of the queue, it goes only by Send Again");
}

#[test]
fn a_move_out_of_an_outbox_undone_leaves_it_held_and_isnt_sent() {
    for into_microsoft in [false, true] {
        let (mut s, a, _) = imap();
        let to = if into_microsoft { microsoft(&mut s, OWNER) } else { second_imap(&mut s) };
        let (outbox, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap(), s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap());
        let two = titled(&s, "Two");
        let copy = s.run("mail.copy", &json!({"ids": [two], "folder": outbox})).unwrap()["copies"][0].as_u64().unwrap();
        s.run("mail.move", &json!({"ids": [two], "folder": outbox})).unwrap();
        s.run("mail.move", &json!({"ids": [two, copy], "folder": theirs})).unwrap();
        s.run("edit.undo", &json!({})).unwrap();
        for id in [two, copy] {
            let m = s.mailbox.message(id).unwrap();
            assert_eq!((m.account, m.folder, m.held.as_str()), (a, outbox, mailcraft_model::MOVED_INTO_OUTBOX), "{into_microsoft}");
        }
        assert!(sync_request(&s, a).unwrap().outbox.is_empty(), "{into_microsoft}");
    }
}

#[test]
fn a_move_into_an_outbox_undone_leaves_it_no_longer_held() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (outbox, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap(), s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap());
    let server = ServerMessage { id: "R1".into(), folder: "in".into(), ..Default::default() };
    let rota = Message { folder: theirs, subject: "Rota".into(), server: Some(server), ..Default::default() };
    let rota = s.mailbox.add_message(rota).unwrap();
    s.run("mail.move", &json!({"ids": [rota], "folder": outbox})).unwrap();
    assert_eq!(s.mailbox.message(rota).unwrap().held, mailcraft_model::MOVED_INTO_OUTBOX);
    s.run("edit.undo", &json!({})).unwrap();
    let m = s.mailbox.message(rota).unwrap();
    assert_eq!((m.account, m.folder, m.held.as_str()), (ms, theirs, ""));
}

#[test]
fn a_message_held_in_an_outbox_isnt_put_on_a_server_again_there() {
    let (mut s, a, _) = imap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": outbox})).unwrap();
    s.mailbox.place_again(two);
    assert!(!s.mailbox.queued.iter().any(|q| matches!(q, Queued::PlaceMessage { .. })), "Send Again would file it twice");
}

#[test]
fn a_message_with_its_original_on_a_server_goes_back_there_when_refused_after_an_outbox() {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let (outbox, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": outbox})).unwrap();
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    apply_sync(&mut s, SyncResult { account: b, unplaced: vec![two], ..Default::default() }).unwrap();
    assert_eq!(at(&s, two), (a, s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), 2));
    assert!(s.mailbox.message(two).unwrap().held.is_empty());
}

#[test]
fn a_message_put_back_from_an_outbox_is_no_longer_held() {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let theirs = s.mailbox.folder_of_kind(b, FolderKind::Outbox).unwrap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    assert_eq!(s.mailbox.message(two).unwrap().held, mailcraft_model::MOVED_INTO_OUTBOX);
    s.run("account.remove", &json!({"id": b})).unwrap();
    assert_eq!(at(&s, two), (a, s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), 2));
    assert!(s.mailbox.message(two).unwrap().held.is_empty());
}

#[test]
fn a_message_held_in_an_account_now_microsofts_comes_back_held_when_refused() {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let (outbox, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    let two = titled(&s, "Two");
    let copy = s.run("mail.copy", &json!({"ids": [two], "folder": outbox})).unwrap()["copies"][0].as_u64().unwrap();
    // As `account.graphSwitch` leaves it: the Outbox and what is held in it stay.
    s.mailbox.accounts.iter_mut().find(|account| account.id == a).unwrap().kind = mailcraft_model::AccountKind::Microsoft;
    s.run("mail.move", &json!({"ids": [copy], "folder": theirs})).unwrap();
    apply_sync(&mut s, SyncResult { account: b, unplaced: vec![copy], ..Default::default() }).unwrap();
    assert_eq!(at(&s, copy), (a, outbox, 0));
    assert_eq!(s.mailbox.message(copy).unwrap().held, mailcraft_model::MOVED_INTO_OUTBOX);
    assert_eq!(super::graph_send::waiting(&s.mailbox, a), 0, "it would go to everybody it names");
}

/// "Two" moved into the Inbox of a second IMAP account, whose server took it and didn't say
/// where: the session, the first account, the second, the message and what the server was asked.
fn waiting_for_its_copy() -> (Session, Id, Id, Id, SyncRequest) {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let theirs = s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    let asked = sync_request(&s, b).unwrap();
    apply_sync(&mut s, SyncResult { account: b, placed: vec![put(&asked, two, 0, 0)], ..Default::default() }).unwrap();
    assert!(!marker(&s, two).is_empty());
    (s, a, b, two, asked)
}

fn marker(s: &Session, id: Id) -> String {
    s.mailbox.message(id).unwrap().placed.clone()
}

#[test]
fn a_message_that_moves_on_leaves_the_marker_of_the_copy_it_waited_for() {
    // On in the account.
    let (mut s, _, b, two, _) = waiting_for_its_copy();
    let later = s.mailbox.add_folder(b, None, "Later").unwrap();
    s.run("mail.move", &json!({"ids": [two], "folder": later})).unwrap();
    assert!(marker(&s, two).is_empty());
    // Into a third account.
    let (mut s, _, _, two, _) = waiting_for_its_copy();
    let ms = microsoft(&mut s, OWNER);
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
    assert!(marker(&s, two).is_empty());
    // Back to where its original is, with the account it went to.
    let (mut s, a, b, two, _) = waiting_for_its_copy();
    s.run("account.remove", &json!({"id": b})).unwrap();
    assert_eq!(at(&s, two), (a, s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), 2));
    assert!(marker(&s, two).is_empty());
    // A copy of it never waited for that copy.
    let (mut s, _, b, two, _) = waiting_for_its_copy();
    let later = s.mailbox.add_folder(b, None, "Later").unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [two], "folder": later})).unwrap()["copies"][0].as_u64().unwrap();
    assert!(marker(&s, copy).is_empty() && !marker(&s, two).is_empty());
}

#[test]
fn a_later_copy_with_a_uid_leaves_no_marker_to_lose_the_server_message_by() {
    let (mut s, _, b, two, _) = waiting_for_its_copy();
    s.mailbox.place_again(two);
    let asked = sync_request(&s, b).unwrap();
    apply_sync(&mut s, SyncResult { account: b, placed: vec![put(&asked, two, 60, 9)], ..Default::default() }).unwrap();
    assert!(marker(&s, two).is_empty());
    // The server numbers the mailbox anew, with a change here still to go up: the message is
    // found again by its Message-ID, and isn't here twice.
    let m = s.mailbox.message_mut(two).unwrap();
    m.read = !m.read;
    let renumbered = FolderDelta {
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 10,
        uid_seen: 70,
        new: vec![from_server(70, "Two")],
        ..Default::default()
    };
    apply_sync(&mut s, SyncResult { account: b, folders: vec![renumbered], ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.messages.values().filter(|m| m.subject == "Two").count(), 1);
    assert_eq!(s.mailbox.message(two).unwrap().uid, 70);
}

#[test]
fn a_move_on_undone_waits_for_its_copy_by_its_marker_again() {
    let (mut s, _, b, two, asked) = waiting_for_its_copy();
    let waited = marker(&s, two);
    let ms = microsoft(&mut s, OWNER);
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(marker(&s, two), waited);
    let copy = carrying(&asked, two, from_server(52, "Two"));
    let news = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 9, uid_seen: 52, new: vec![copy], ..Default::default() };
    apply_sync(&mut s, SyncResult { account: b, folders: vec![news], ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.messages.values().filter(|m| m.subject == "Two").count(), 1);
    assert_eq!(s.mailbox.message(two).unwrap().uid, 52);
}

#[test]
fn removing_the_account_mail_was_moved_into_lets_its_original_go_or_brings_it_back() {
    let delete_asked =
        |s: &Session, ms: Id| graph_mail_request(s, ms).unwrap().messages.iter().any(|c| matches!(c, MessageChange::Delete { id } if id == "AAkA2"));
    // From the IMAP account into Microsoft, made there: the original goes from the IMAP server
    // as it would have.
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap()})).unwrap();
    placed(&mut s, ms, OWNER, &[made("AAkA2")]);
    s.run("account.remove", &json!({"id": ms})).unwrap();
    assert_eq!(ops(&s, a), [delete(2)]);
    // Into another IMAP account whose server didn't say where it put it, not come down yet: back
    // where the first server has it, shown and kept.
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let (two, inbox) = (titled(&s, "Two"), s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap());
    s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap()})).unwrap();
    let asked = sync_request(&s, b).unwrap();
    apply_sync(&mut s, SyncResult { account: b, placed: vec![put(&asked, two, 0, 0)], ..Default::default() }).unwrap();
    s.run("account.remove", &json!({"id": b})).unwrap();
    assert_eq!(at(&s, two), (a, inbox, 2));
    assert!(s.mailbox.pending.is_empty() && ops(&s, a).is_empty() && known(&s, a).contains(&2), "{:?}", s.mailbox.pending);
    // Out of Microsoft into the IMAP account, not known there yet: back in Microsoft as that
    // server's message, nothing deleted.
    let (mut s, a, ms, two) = moved_out_of_microsoft(vec![]);
    let asked = sync_request(&s, a).unwrap();
    apply_sync(&mut s, SyncResult { account: a, placed: vec![put(&asked, two, 0, 0)], ..Default::default() }).unwrap();
    s.run("account.remove", &json!({"id": a})).unwrap();
    let m = s.mailbox.message(two).unwrap();
    assert_eq!((m.account, m.server.as_ref().map(|x| x.id.as_str())), (ms, Some("AAkA2")));
    assert!(!delete_asked(&s, ms));
    // Known there: the original goes from Microsoft as it would have.
    let (mut s, a, ms, two) = moved_out_of_microsoft(vec![]);
    let asked = sync_request(&s, a).unwrap();
    apply_sync(&mut s, SyncResult { account: a, placed: vec![put(&asked, two, 60, 7)], ..Default::default() }).unwrap();
    s.run("account.remove", &json!({"id": a})).unwrap();
    assert!(s.mailbox.message(two).is_none());
    assert!(delete_asked(&s, ms));
    // Onto this computer, whole in the saved file, and that account removed: the copy goes with
    // it, so the message is back where the IMAP server has it, and nothing is deleted there.
    let (mut s, a, kept) = imap();
    let (two, inbox, local) = (titled(&s, "Two"), s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), s.mailbox.folder(kept).unwrap().account);
    s.run("mail.move", &json!({"ids": [two], "folder": kept})).unwrap();
    s.note_saved();
    assert_eq!(ops(&s, a), [delete(2)]);
    s.run("account.remove", &json!({"id": local})).unwrap();
    assert_eq!(at(&s, two), (a, inbox, 2));
    assert!(s.mailbox.pending.is_empty() && ops(&s, a).is_empty(), "{:?}", s.mailbox.pending);
    // Out of Microsoft onto this computer, the same: back in Microsoft as that server's message.
    let (mut s, a, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let (two, local) = (titled(&s, "Two"), s.mailbox.folder(kept).unwrap().account);
    s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap()})).unwrap();
    placed(&mut s, ms, OWNER, &[made("AAkA2")]);
    deleted(&mut s, a, &[2]);
    s.run("mail.move", &json!({"ids": [two], "folder": kept})).unwrap();
    s.note_saved();
    assert!(!graph_mail_request(&s, ms).unwrap().messages.is_empty());
    s.run("account.remove", &json!({"id": local})).unwrap();
    let m = s.mailbox.message(two).unwrap();
    assert_eq!((m.account, m.server.as_ref().map(|x| x.id.as_str())), (ms, Some("AAkA2")));
    assert!(graph_mail_request(&s, ms).unwrap().messages.is_empty(), "{:?}", graph_mail_request(&s, ms).unwrap().messages);
}

/// Delete `folder` for good: into Deleted Items first when it isn't there yet.
fn delete_for_good(s: &mut Session, folder: Id) {
    while s.mailbox.folder(folder).is_some() {
        s.run("folder.delete", &json!({"id": folder})).unwrap();
    }
}

#[test]
fn a_folder_deleted_for_good_with_mail_moved_into_it_takes_its_original_along() {
    // Into a folder of another IMAP account, not put there yet: deleted on the first server, as
    // a message deleted for good is, and nothing is put on the second.
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    let parent = s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap();
    let projects = s.run("folder.new", &json!({"parent": parent, "name": "Projects"})).unwrap()["id"].as_u64().unwrap();
    let two = titled(&s, "Two");
    s.run("mail.move", &json!({"ids": [two], "folder": projects})).unwrap();
    delete_for_good(&mut s, projects);
    assert_eq!(ops(&s, a), [delete(2)]);
    assert!(sync_request(&s, b).unwrap().places.is_empty() && s.mailbox.queued.is_empty(), "{:?}", s.mailbox.queued);
    // Within the IMAP account, the same.
    let (mut s, a, _) = imap();
    let projects = FolderDelta { remote: "Projects".into(), kind: FolderKind::Custom, uid_validity: 12, ..Default::default() };
    apply_sync(&mut s, SyncResult { account: a, folders: vec![projects], ..Default::default() }).unwrap();
    let projects = s.mailbox.folders.iter().find(|f| f.remote == "Projects").unwrap().id;
    let one = titled(&s, "One");
    s.run("mail.move", &json!({"ids": [one], "folder": projects})).unwrap();
    delete_for_good(&mut s, projects);
    assert!(ops(&s, a).contains(&delete(1)), "{:?}", ops(&s, a));
    // Out of Microsoft into the IMAP account: deleted on Microsoft's server.
    let (mut s, a, ms) = {
        let (mut s, a, _) = imap();
        let ms = microsoft(&mut s, OWNER);
        let two = titled(&s, "Two");
        s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap()})).unwrap();
        placed(&mut s, ms, OWNER, &[made("AAkA2")]);
        deleted(&mut s, a, &[2]);
        (s, a, ms)
    };
    let projects = FolderDelta { remote: "Projects".into(), kind: FolderKind::Custom, uid_validity: 12, ..Default::default() };
    apply_sync(&mut s, SyncResult { account: a, folders: vec![projects], ..Default::default() }).unwrap();
    let projects = s.mailbox.folders.iter().find(|f| f.remote == "Projects").unwrap().id;
    s.run("mail.move", &json!({"ids": [titled(&s, "Two")], "folder": projects})).unwrap();
    delete_for_good(&mut s, projects);
    let asked = graph_mail_request(&s, ms).unwrap().messages;
    assert!(asked.iter().any(|c| matches!(c, MessageChange::Delete { id } if id == "AAkA2")), "{asked:?}");
    assert!(sync_request(&s, a).unwrap().places.is_empty());
    // Within the Microsoft account, not moved on its server yet: deleted where the server has
    // it. What the server has in the folder goes with the folder there.
    let mut s = Session::empty();
    let ms = microsoft(&mut s, OWNER);
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![
        folder("in", "Inbox", FolderKind::Inbox),
        folder("pr", "Projects", FolderKind::Custom),
        folder("de", "Deleted Items", FolderKind::Deleted),
    ];
    let listing =
        GraphMailResult { account: ms, email: OWNER.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
    apply_graph_mail(&mut s, listing).unwrap();
    let (inbox, projects) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_by_server(ms, "pr").unwrap());
    let on_server = |id: &str, folder: &str| Some(ServerMessage { id: id.into(), folder: folder.into(), ..Default::default() });
    let receipt =
        s.mailbox.add_message(Message { folder: inbox, subject: "Receipt".into(), server: on_server("AAkA7", "in"), ..Default::default() }).unwrap();
    s.mailbox.add_message(Message { folder: projects, subject: "Notes".into(), server: on_server("AAkA8", "pr"), ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [receipt], "folder": projects})).unwrap();
    delete_for_good(&mut s, projects);
    let asked = graph_mail_request(&s, ms).unwrap().messages;
    let deletes: Vec<&str> = asked.iter().filter_map(|c| if let MessageChange::Delete { id } = c { Some(id.as_str()) } else { None }).collect();
    assert_eq!(deletes, ["AAkA7"], "{asked:?}");
}

#[test]
fn a_copy_made_on_microsofts_server_doesnt_say_who_got_a_blind_copy() {
    let (mut s, _, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let to = vec![Address::new("Bob", "bob@example.org")];
    let rota = Message { folder: kept, to, bcc: vec![Address::new("Eve", "eve@example.net")], ..from_server(0, "Rota") };
    let rota = s.mailbox.add_message(rota).unwrap();
    s.run("mail.copy", &json!({"ids": [rota], "folder": s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap()})).unwrap();
    let places = graph_mail_request(&s, ms).unwrap().places;
    let asked = serde_json::to_string(&places).unwrap();
    assert!(places.len() == 1 && asked.contains("bob@example.org") && !asked.contains("eve@example.net"), "{asked}");
    // Here it still says who got one.
    assert_eq!(s.mailbox.message(rota).map(|m| m.bcc.len()), Some(1));
}

#[test]
fn moving_many_at_once_does_what_moving_each_in_turn_does() {
    // Mail in every state a move meets: a move waiting on its server, a copy waiting to be put on
    // one, a message being moved out of Microsoft, and plain server mail.
    let (mut s, a, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let b = second_imap(&mut s);
    let (archive, theirs) = (s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap(), s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap());
    let (one, two, three) = (titled(&s, "One"), titled(&s, "Two"), titled(&s, "Three"));
    s.run("mail.move", &json!({"ids": [one], "folder": archive})).unwrap();
    s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    let here = kept_here(&mut s, kept, "From here");
    s.run("mail.move", &json!({"ids": [here], "folder": theirs})).unwrap();
    let server = ServerMessage { id: "AAkA7".into(), folder: "in".into(), ..Default::default() };
    let body = Body { text: "All of it.".into(), html: None };
    let out = s.mailbox.add_message(Message { folder: theirs, subject: "Receipt".into(), server: Some(server), body, ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [out], "folder": kept})).unwrap();
    let ids = [one, two, three, here, out];
    let folders =
        [kept, archive, s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), theirs, s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap()];
    // All but the part of a copy's marker that nobody knows.
    let known = |mut mb: mailcraft_model::Mailbox| {
        for q in &mut mb.queued {
            if let Queued::PlaceMessage { marker, .. } = q {
                *marker = marker.rsplit_once('-').map(|(known, _)| known.to_string()).unwrap_or_default();
            }
        }
        serde_json::to_value(&mb).unwrap()
    };
    for folder in folders {
        let (mut together, mut each) = (s.mailbox.clone(), s.mailbox.clone());
        let moved = together.move_messages(&ids, folder).unwrap();
        for id in ids {
            each.move_message(id, folder).unwrap();
        }
        assert_eq!(known(together), known(each), "into {folder}");
        assert_eq!(moved.len(), ids.iter().filter(|id| s.mailbox.message(**id).is_some_and(|m| m.folder != folder)).count());
    }
}

#[test]
fn what_is_moving_out_of_a_server_is_found_at_once_as_it_is_for_each_message() {
    let (mut s, a, kept) = imap();
    let archive = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    // Out of the IMAP account onto this computer, and within it.
    s.run("mail.move", &json!({"ids": [titled(&s, "Two")], "folder": kept})).unwrap();
    s.run("mail.move", &json!({"ids": [titled(&s, "Three")], "folder": archive})).unwrap();
    // Out of a Microsoft account onto this computer.
    let ms = microsoft(&mut s, OWNER);
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let server = mailcraft_model::sync::ServerMessage { id: "AAkA7".into(), folder: "in".into(), ..Default::default() };
    let body = Body { text: "All of it.".into(), html: None };
    let m = s.mailbox.add_message(Message { folder: inbox, subject: "Receipt".into(), server: Some(server), body, ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
    let each: std::collections::HashSet<Id> = s.mailbox.messages.keys().copied().filter(|id| s.mailbox.moving_out_of_a_server(*id)).collect();
    assert_eq!(each, [titled(&s, "Two"), m].into_iter().collect());
    assert_eq!(s.mailbox.moving_out_of_servers(), each);
}

#[test]
fn ids_named_wrongly_never_move_or_copy_the_selected_message_onto_another_server() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let (one, two) = (titled(&s, "One"), titled(&s, "Two"));
    s.run("mail.select", &json!({"ids": [one]})).unwrap();
    // What a look-up that found nothing gives, an id as text, zero, a fraction, a list with a
    // wrong entry, or both at once names no message: the selected one isn't taken instead.
    let wrong = [
        json!({"id": null}),
        json!({"id": "2"}),
        json!({"id": 0}),
        json!({"id": 2.5}),
        json!({"ids": null}),
        json!({"ids": "2"}),
        json!({"ids": []}),
        json!({"ids": [two, "x"]}),
        json!({"id": two, "ids": [two]}),
    ];
    for command in ["mail.move", "mail.copy"] {
        for params in &wrong {
            let mut params = params.clone();
            params["folder"] = json!(theirs);
            assert!(s.run(command, &params).is_err(), "{command} {params}");
            // An agent's are refused before the owner is asked about the selected one.
            assert!(matches!(crate::agent::check(&s, command, &params, &[]), crate::agent::Verdict::Refuse(_)), "{command} {params}");
        }
    }
    assert!(s.mailbox.queued.is_empty() && s.mailbox.pending.is_empty());
    assert_eq!((at(&s, one).0, at(&s, two).0, s.mailbox.messages.len()), (a, a, 4));
    // Left out, they mean the selection, as ever; one id, the message it names.
    s.run("mail.copy", &json!({"id": two, "folder": theirs})).unwrap();
    assert_eq!(s.sel.messages, [one]);
    s.run("mail.move", &json!({"folder": theirs})).unwrap();
    assert_eq!((at(&s, one).0, at(&s, two).0, s.mailbox.messages.len()), (ms, a, 5));
}

#[test]
fn an_agent_is_asked_before_it_puts_mail_on_another_accounts_server() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, a, kept) = imap();
    let ms = microsoft(&mut s, OWNER);
    let b = second_imap(&mut s);
    let (theirs, other) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    let archive = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    let two = titled(&s, "Two");
    let mine = kept_here(&mut s, kept, "Mine");
    for command in ["mail.move", "mail.copy"] {
        // Onto another account's server, from another server or from this computer: as a send.
        for (id, folder) in [(two, theirs), (two, other), (mine, theirs), (mine, other)] {
            assert_eq!(classify(&s, command, &json!({"ids": [id], "folder": folder})), Access::Owner(Class::Send), "{command} {id} {folder}");
        }
        // Within its account, or onto this computer: as ever.
        for (id, folder) in [(two, archive), (two, kept)] {
            assert_eq!(classify(&s, command, &json!({"ids": [id], "folder": folder})), Access::Safe, "{command} {id} {folder}");
        }
        // The selection likewise. Named wrongly, the command refuses them.
        s.run("mail.select", &json!({"ids": [two]})).unwrap();
        assert_eq!(classify(&s, command, &json!({"folder": theirs})), Access::Owner(Class::Send), "{command}");
        assert_eq!(classify(&s, command, &json!({"id": null, "folder": theirs})), Access::Safe, "{command}");
    }
    // The question says what goes where.
    let what = describe(&s, "mail.move", &json!({"ids": [two], "folder": theirs}));
    assert!(what.starts_with("move 1 message (\u{201c}Two\u{201d}) onto the server of owner@example.org, into \u{201c}Inbox\u{201d}"), "{what}");
}

#[test]
fn an_agent_is_asked_before_a_rule_puts_mail_on_another_accounts_server() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, a, kept) = imap();
    let b = second_imap(&mut s);
    let (inbox, other) = (s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    let two = titled(&s, "Two");
    let rule = |folder: Id| json!({"name": "R", "conditions": [{"type": "subjectContains", "value": "two"}], "actions": [{"type": "moveTo", "value": folder}]});
    // Made, made from a message, switched on or run: into another account's folder, as a send;
    // onto this computer, a change of rules as ever.
    assert_eq!(classify(&s, "rules.add", &rule(other)), Access::Owner(Class::Send));
    assert_eq!(classify(&s, "rules.add", &rule(kept)), Access::Owner(Class::Settings));
    assert_eq!(classify(&s, "rules.fromMessage", &json!({"id": two, "folder": other})), Access::Owner(Class::Send));
    assert_eq!(classify(&s, "rules.fromMessage", &json!({"id": two, "folder": kept})), Access::Owner(Class::Settings));
    // Made from a message of the account it files into: it files the other account's new mail
    // there too.
    assert_eq!(classify(&s, "rules.fromMessage", &json!({"id": two, "folder": inbox})), Access::Owner(Class::Send));
    let made = s.run("rules.add", &rule(other)).unwrap()["id"].as_u64().unwrap();
    assert_eq!(classify(&s, "rules.run", &json!({"folder": inbox})), Access::Owner(Class::Send));
    assert_eq!(classify(&s, "rules.run", &json!({"folder": other})), Access::Owner(Class::Settings));
    s.run("rules.enable", &json!({"id": made, "value": false})).unwrap();
    assert_eq!(classify(&s, "rules.run", &json!({"folder": inbox})), Access::Owner(Class::Settings));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": made})), Access::Owner(Class::Send));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": made, "value": false})), Access::Owner(Class::Settings));
    // The question says where the mail goes.
    let what = describe(&s, "rules.add", &rule(other));
    assert!(what.contains("onto the server of blake@other.example, into"), "{what}");
    let what = describe(&s, "rules.fromMessage", &json!({"id": two, "folder": other}));
    assert!(what.contains("onto the server of blake@other.example, into"), "{what}");
    s.run("rules.enable", &json!({"id": made, "value": true})).unwrap();
    let what = describe(&s, "rules.run", &json!({"folder": inbox}));
    assert!(what.contains("which move mail onto the server of blake@other.example"), "{what}");
}

#[test]
fn a_message_a_damaged_file_has_without_a_folder_goes_into_its_own_inbox_and_onto_no_server() {
    let (mut s, a, _) = imap();
    let b = second_imap(&mut s);
    // The IMAP account first: its Inbox is the mailbox's.
    let at_a = s.mailbox.accounts.iter().position(|x| x.id == a).unwrap();
    let first = s.mailbox.accounts.remove(at_a);
    s.mailbox.accounts.insert(0, first);
    let (inbox_a, inbox_b) = (s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap());
    let lost = s.mailbox.add_message(Message { folder: inbox_b, ..from_server(5, "Lost") }).unwrap();
    let stray = s.mailbox.add_message(Message { folder: inbox_b, ..from_server(6, "Stray") }).unwrap();
    s.mailbox.message_mut(lost).unwrap().folder = 999_999;
    let m = s.mailbox.message_mut(stray).unwrap();
    (m.folder, m.account) = (999_998, 999_997);
    s.mailbox.repair();
    // Into its own account's Inbox; one of no account into the first Inbox. Here alone: no
    // server is given a copy, and none is told anything.
    assert_eq!((at(&s, lost), at(&s, stray)), ((b, inbox_b, 0), (a, inbox_a, 0)));
    assert!(s.mailbox.queued.is_empty() && s.mailbox.pending.is_empty(), "{:?} {:?}", s.mailbox.queued, s.mailbox.pending);
}

#[test]
fn a_copys_marker_cant_be_worked_out_from_the_mailbox() {
    // The same mailbox and the same move, twice: all else the same, the markers differ.
    let markers: Vec<String> = (0..2)
        .map(|_| {
            let (mut s, a, _) = imap();
            let b = second_imap(&mut s);
            let two = titled(&s, "Two");
            s.run("mail.move", &json!({"ids": [two], "folder": s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap()})).unwrap();
            assert_eq!(at(&s, two).0, b, "{a}");
            s.mailbox.queued.iter().find_map(|q| if let Queued::PlaceMessage { marker, .. } = q { Some(marker.clone()) } else { None }).unwrap()
        })
        .collect();
    let (known, unknown): (Vec<&str>, Vec<&str>) = markers.iter().filter_map(|m| m.rsplit_once('-')).unzip();
    assert_eq!(known[0], known[1]);
    assert_ne!(unknown[0], unknown[1]);
    assert!(markers.iter().all(|m| m.len() <= 200 && m.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')), "{markers:?}");
}

#[test]
fn a_rule_files_new_microsoft_mail_into_gmail_once_it_has_come_down_whole() {
    let (mut s, a, _) = imap();
    let ms = microsoft(&mut s, OWNER);
    let gmail = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    // Read once already: what comes now is new mail, which the rules sort.
    s.mailbox.folder_mut(inbox).unwrap().server.as_mut().unwrap().caught_up = true;
    let rule = json!({"name": "Rota", "conditions": [{"type": "subjectContains", "value": "rota"}], "actions": [{"type": "moveTo", "value": gmail}]});
    s.run("rules.add", &rule).unwrap();
    let there = GraphMessage {
        server: ServerMessage { id: "R1".into(), folder: "in".into(), ..Default::default() },
        from: Address::new("", "boss@example.org"),
        subject: "Rota".into(),
        message_id: "rota@example.org".into(),
        ..Default::default()
    };
    let page = MessagePage { folder: "in".into(), messages: vec![there], ..Default::default() };
    apply_graph_mail(&mut s, GraphMailResult { account: ms, email: OWNER.into(), pages: vec![page], partial: true, ..Default::default() }).unwrap();
    // Filed at once, as only its headers and preview: Microsoft's run fetches it whole first,
    // and Gmail is given nothing of it until then.
    let rota = titled(&s, "Rota");
    assert_eq!((at(&s, rota), s.mailbox.message(rota).unwrap().unfetched), ((a, gmail, 0), true));
    let fetch = crate::cmd::graph_down::fetch_queue_passing_over(&s.mailbox, ms, 10, 1_800_000_000, &Default::default());
    assert_eq!(fetch, ["R1"]);
    assert!(sync_request(&s, a).unwrap().places.is_empty());
    let mime = b"Message-ID: <rota@example.org>\r\nFrom: boss@example.org\r\nSubject: Rota\r\n\r\nThe rota, all of it.\r\n".to_vec();
    let fetched = vec![mailcraft_model::sync::GraphFetched { id: "R1".into(), mime }];
    let result = mailcraft_model::sync::GraphFetchResult { account: ms, email: OWNER.into(), fetched, ..Default::default() };
    crate::cmd::graph_mail::apply_graph_fetch(&mut s, result).unwrap();
    // Whole: Gmail is given it, and once Gmail has it, Microsoft's original goes.
    let asked = sync_request(&s, a).unwrap();
    assert_eq!(asked.places.iter().map(|p| p.message.body.text.as_str()).collect::<Vec<_>>(), ["The rota, all of it."]);
    apply_sync(&mut s, SyncResult { account: a, placed: vec![put(&asked, rota, 40, 7)], ..Default::default() }).unwrap();
    assert_eq!(at(&s, rota), (a, gmail, 40));
    let changes = graph_mail_request(&s, ms).unwrap().messages;
    assert!(changes.iter().any(|c| matches!(c, MessageChange::MovedOut { message, id } if *message == rota && id == "R1")), "{changes:?}");
}
