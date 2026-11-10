// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Message, ServerFolder};
use serde_json::json;

use super::*;
use crate::Session;

/// A session with an IMAP account; its id and its Inbox's.
fn session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    (s, account, inbox)
}

fn message(uid: u32, from: &str, subject: &str) -> Message {
    Message { uid, from: Address::new(from, "someone@sender.example"), subject: subject.into(), date: 1_700_000_000, ..Default::default() }
}

/// What the server says of its Inbox: these messages are new, and it has been read up to the last.
fn inbox_delta(inbox: Id, messages: Vec<Message>) -> FolderDelta {
    let uid_seen = messages.iter().map(|m| m.uid).max().unwrap_or(0);
    FolderDelta { id: Some(inbox), remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen, new: messages, ..Default::default() }
}

fn apply(s: &mut Session, account: Id, folders: Vec<FolderDelta>) {
    s.run("sync.apply", &json!({"result": SyncResult { account, folders, ..Default::default() }})).unwrap();
}

fn subjects(found: &[Arrival]) -> Vec<&str> {
    found.iter().map(|a| a.subject.as_str()).collect()
}

/// An account whose Inbox has been read once: what the next run brings is new mail.
fn read_once() -> (Session, Id, Id) {
    let (mut s, account, inbox) = session();
    apply(&mut s, account, vec![inbox_delta(inbox, vec![message(1, "Old", "Already here")])]);
    (s, account, inbox)
}

#[test]
fn new_mail_in_an_inbox_read_before_arrives_with_its_sender_and_subject() {
    let (mut s, account, inbox) = read_once();
    let before = mark(&s.mailbox);
    apply(&mut s, account, vec![inbox_delta(inbox, vec![message(2, "Rowan Pike", "Lunch?"), message(3, "", "No name")])]);
    let found = arrivals(&s.mailbox, &before);
    assert_eq!(subjects(&found), ["Lunch?", "No name"]);
    assert_eq!(found[0].sender, "Rowan Pike");
    assert_eq!(found[1].sender, "someone@sender.example", "no name: the address");
    assert!(found.iter().all(|a| a.account == account && a.folder == inbox));
    // Asked again, with a mark taken after the run: nothing.
    assert!(arrivals(&s.mailbox, &mark(&s.mailbox)).is_empty());
}

#[test]
fn an_accounts_first_download_is_not_new_mail() {
    let (mut s, account, inbox) = session();
    let before = mark(&s.mailbox);
    apply(&mut s, account, vec![inbox_delta(inbox, vec![message(1, "A", "One"), message(2, "B", "Two")])]);
    assert!(arrivals(&s.mailbox, &before).is_empty());
    // Even when the first download comes in pieces: the mark is taken before the run.
    apply(&mut s, account, vec![inbox_delta(inbox, vec![message(3, "C", "Three")])]);
    assert!(arrivals(&s.mailbox, &before).is_empty());
}

#[test]
fn mail_a_rule_moves_out_of_the_inbox_is_not_new_mail() {
    let (mut s, account, inbox) = read_once();
    let receipts = s.mailbox.add_folder(account, None, "Receipts").unwrap();
    let rule = json!({"name": "Receipts", "conditions": [{"type": "subjectContains", "value": "receipt"}], "actions": [{"type": "moveTo", "value": receipts}]});
    s.run("rules.add", &rule).unwrap();
    let before = mark(&s.mailbox);
    apply(&mut s, account, vec![inbox_delta(inbox, vec![message(2, "Shop", "Your receipt"), message(3, "Rowan", "Hello")])]);
    assert_eq!(subjects(&arrivals(&s.mailbox, &before)), ["Hello"]);
}

#[test]
fn junk_and_other_folders_are_not_new_mail() {
    let (mut s, account, inbox) = read_once();
    let junk = s.mailbox.folder_of_kind(account, FolderKind::Junk).unwrap();
    let projects = s.mailbox.add_folder(account, None, "Projects").unwrap();
    // Both read from the server before.
    let other = |id: Id, remote: &str, kind: FolderKind, uid: u32, subject: &str| FolderDelta {
        id: Some(id),
        remote: remote.into(),
        kind,
        uid_validity: 3,
        uid_seen: uid,
        new: vec![message(uid, "X", subject)],
        ..Default::default()
    };
    apply(&mut s, account, vec![other(junk, "Junk", FolderKind::Junk, 1, "old junk"), other(projects, "Projects", FolderKind::Custom, 1, "old")]);
    let before = mark(&s.mailbox);
    apply(
        &mut s,
        account,
        vec![
            other(junk, "Junk", FolderKind::Junk, 2, "Win a prize"),
            other(projects, "Projects", FolderKind::Custom, 2, "Filed by the server"),
            inbox_delta(inbox, vec![message(5, "Rowan", "In the Inbox")]),
        ],
    );
    assert_eq!(subjects(&arrivals(&s.mailbox, &before)), ["In the Inbox"]);
    // A message moved by hand into Junk after it arrived is not new mail either.
    let before = mark(&s.mailbox);
    apply(&mut s, account, vec![inbox_delta(inbox, vec![message(6, "Spammer", "Moved away")])]);
    let id = s.mailbox.messages.values().find(|m| m.subject == "Moved away").unwrap().id;
    s.mailbox.move_message(id, junk).unwrap();
    assert!(arrivals(&s.mailbox, &before).is_empty());
}

#[test]
fn read_mail_drafts_and_mail_already_here_are_not_new_mail() {
    let (mut s, account, inbox) = read_once();
    let before = mark(&s.mailbox);
    let read = Message { read: true, ..message(2, "Seen", "Read on the phone") };
    apply(&mut s, account, vec![inbox_delta(inbox, vec![read])]);
    // A message moved into the Inbox by hand was here before the run.
    let local = s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let draft = s.mailbox.add_message(Message { folder: local, subject: "draft".into(), draft: true, ..Default::default() }).unwrap();
    let before_move = mark(&s.mailbox);
    s.mailbox.move_message(draft, inbox).unwrap();
    assert!(arrivals(&s.mailbox, &before).iter().all(|a| a.subject != "Read on the phone"));
    assert!(arrivals(&s.mailbox, &before_move).is_empty());
}

#[test]
fn an_inbox_numbered_anew_is_not_new_mail() {
    let (mut s, account, inbox) = read_once();
    let before = mark(&s.mailbox);
    let renumbered = FolderDelta { uid_validity: 99, ..inbox_delta(inbox, vec![message(1, "Old", "Already here"), message(2, "New", "Again")]) };
    apply(&mut s, account, vec![renumbered]);
    assert!(arrivals(&s.mailbox, &before).is_empty());
}

#[test]
fn a_shared_mailbox_and_a_local_account_are_not_watched() {
    let mut s = Session::empty();
    let owner = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.mailbox.add_shared_mailbox(owner, "team@example.org").unwrap();
    let local = s.mailbox.add_account(mailcraft_model::Account { name: "Local".into(), email: "me@local.example".into(), ..Default::default() });
    // Every Inbox read to the end once, as Graph and IMAP say.
    let inboxes: Vec<(Id, Id)> =
        [owner, shared, local].iter().filter_map(|a| s.mailbox.folder_of_kind(*a, FolderKind::Inbox).map(|f| (*a, f))).collect();
    assert_eq!(inboxes.len(), 3);
    for (_, f) in &inboxes {
        s.mailbox.folder_mut(*f).unwrap().server = Some(ServerFolder { id: format!("F{f}"), caught_up: true, ..Default::default() });
    }
    let before = mark(&s.mailbox);
    for (account, f) in &inboxes {
        s.mailbox.add_message(Message { folder: *f, subject: format!("to {account}"), ..Default::default() }).unwrap();
    }
    assert_eq!(subjects(&arrivals(&s.mailbox, &before)), [format!("to {owner}")], "only the owner's own Microsoft account");
}

#[test]
fn a_microsoft_inbox_whose_first_round_has_not_ended_is_a_first_download() {
    let mut s = Session::empty();
    let owner = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F".into(), delta: "next page".into(), ..Default::default() });
    let before = mark(&s.mailbox);
    s.mailbox.add_message(Message { folder: inbox, subject: "page two".into(), ..Default::default() }).unwrap();
    assert!(arrivals(&s.mailbox, &before).is_empty());
}

#[test]
fn a_flood_is_capped() {
    let (mut s, account, inbox) = read_once();
    let before = mark(&s.mailbox);
    let many: Vec<Message> = (2..(MOST as u32 + 50)).map(|uid| message(uid, "Bulk", "Same")).collect();
    apply(&mut s, account, vec![inbox_delta(inbox, many)]);
    assert_eq!(arrivals(&s.mailbox, &before).len(), MOST);
    // A mark of an empty mailbox, and one from another mailbox with ids beyond all of these.
    assert!(arrivals(&s.mailbox, &InboxMark { last_id: Id::MAX, inboxes: vec![(inbox, 7)] }).is_empty());
    assert!(arrivals(&Mailbox::default(), &before).is_empty());
}

#[test]
fn nothing_arrives_in_an_account_a_newer_version_added() {
    // Its Inbox was read to the end by that version; a message put into it here is no new mail.
    let mut s = Session::empty();
    let newer = mailcraft_model::Account { email: "ada@jmap.example".into(), kind: mailcraft_model::AccountKind::Unknown, ..Default::default() };
    let newer = s.mailbox.add_account(newer);
    let inbox = s.mailbox.folder_of_kind(newer, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "inbox".into(), caught_up: true, ..Default::default() });
    let before = mark(&s.mailbox);
    s.mailbox.add_message(Message { folder: inbox, subject: "Copied in".into(), ..Default::default() }).unwrap();
    assert!(arrivals(&s.mailbox, &before).is_empty());
}

/// A Microsoft account's Inbox with the server folder as given; the account and the Inbox.
fn microsoft_inbox(server: ServerFolder) -> (Session, Id, Id) {
    let mut s = Session::empty();
    let owner = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(server);
    (s, owner, inbox)
}

#[test]
fn a_microsoft_inbox_a_run_stopped_partway_through_still_raises_a_notice_next_time() {
    // #609: the first round ended; a later round stopped between delta pages (not caught up).
    let server = ServerFolder { id: "F".into(), delta: "next page".into(), caught_up: false, round_ended: true, ..Default::default() };
    let (mut s, _, inbox) = microsoft_inbox(server);
    let before = mark(&s.mailbox);
    s.mailbox.add_message(Message { folder: inbox, subject: "after the partial run".into(), ..Default::default() }).unwrap();
    assert_eq!(subjects(&arrivals(&s.mailbox, &before)), ["after the partial run"]);
}

#[test]
fn a_caught_up_microsoft_inbox_of_a_file_from_before_round_ended_arrives() {
    let server = ServerFolder { id: "F".into(), delta: "link".into(), caught_up: true, round_ended: false, ..Default::default() };
    let (mut s, _, inbox) = microsoft_inbox(server);
    let before = mark(&s.mailbox);
    s.mailbox.add_message(Message { folder: inbox, subject: "old file".into(), ..Default::default() }).unwrap();
    assert_eq!(arrivals(&s.mailbox, &before).len(), 1);
}

#[test]
fn a_partial_graph_run_after_a_caught_up_one_leaves_the_inbox_watched() {
    use crate::cmd::graph_down::{Down, ServerIndex, take_page};
    use mailcraft_model::sync::MessagePage;
    let server = ServerFolder { id: "F".into(), delta: "link".into(), caught_up: true, round_ended: true, ..Default::default() };
    let (mut s, owner, inbox) = microsoft_inbox(server);
    let mut index = ServerIndex::default();
    // The run stops after a page that does not end the round.
    let page = MessagePage { folder: "F".into(), link: "next page".into(), caught_up: false, ..Default::default() };
    take_page(&mut s.mailbox, &mut index, owner, &page, &mut Down::default());
    let kept = s.mailbox.folder(inbox).and_then(|f| f.server.as_ref()).unwrap();
    assert!(!kept.caught_up && kept.round_ended);
    let before = mark(&s.mailbox);
    s.mailbox.add_message(Message { folder: inbox, subject: "next run".into(), ..Default::default() }).unwrap();
    assert_eq!(subjects(&arrivals(&s.mailbox, &before)), ["next run"]);
}
