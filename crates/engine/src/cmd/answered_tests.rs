// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The replied and forwarded marks (#154): an original is marked once its reply or forward has
//! gone (the server took it), not when Send is pressed; a reply that never went leaves no mark.

use mailcraft_model::sync::{FolderDelta, GraphMailResult, RemoteOp, SendDone, SendOutcome, ServerMessage, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use crate::Session;
use crate::cmd::account::sync_request;
use crate::cmd::graph_mail::apply_graph_mail;

/// A session with an IMAP account whose Inbox has one message from its server; the account's
/// and that message's ids.
fn imap() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let original = Message {
        uid: 5,
        message_id: "plans@example.org".into(),
        subject: "Plans".into(),
        from: Address::new("Bo", "bo@example.org"),
        date: 100,
        ..Default::default()
    };
    let first = SyncResult {
        account: a,
        folders: vec![FolderDelta {
            id: None,
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 5,
            new: vec![original],
            ..Default::default()
        }],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": first})).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let id = s.mailbox.messages_in(inbox).next().unwrap().id;
    (s, a, id)
}

/// Answers `original` (`mail.reply` or `mail.forward`) and presses Send: the queued message's id.
fn send(s: &mut Session, how: &str, original: Id) -> Id {
    s.run(how, &json!({"id": original})).unwrap();
    if how == "mail.forward" {
        s.run("compose.set", &json!({"to": "cy@example.org"})).unwrap();
    }
    let r = s.run("compose.send", &json!({})).unwrap();
    assert_eq!(r["queued"], true);
    r["id"].as_u64().unwrap()
}

fn sent(s: &mut Session, account: Id, ids: Vec<Id>) {
    s.run("sync.apply", &json!({"result": SyncResult { account, sent: ids, ..Default::default() }})).unwrap();
}

fn answered(s: &Session, id: Id) -> (bool, bool) {
    let m = s.mailbox.message(id).unwrap();
    (m.answered, m.forwarded)
}

/// Whether the next synchronisation sets `\Answered` on the server.
fn pushes_answered(s: &Session, account: Id) -> bool {
    sync_request(s, account).unwrap().ops.iter().any(|op| matches!(op, RemoteOp::Flags { uid: 5, answered: Some(true), .. }))
}

#[test]
fn an_imap_reply_marks_the_original_once_the_server_took_it_and_the_mark_goes_up_then() {
    let (mut s, a, original) = imap();
    let reply = send(&mut s, "mail.reply", original);
    assert_eq!(answered(&s, original), (false, false), "not when Send is pressed");
    assert!(!pushes_answered(&s, a), "nor is the server told yet");
    // A run that couldn't send it: it waits in the Outbox, and the original isn't marked.
    sent(&mut s, a, Vec::new());
    assert_eq!(answered(&s, original), (false, false));
    sent(&mut s, a, vec![reply]);
    assert_eq!(answered(&s, original), (true, false));
    assert!(s.mailbox.message(reply).unwrap().answers.is_none());
    assert!(pushes_answered(&s, a), "the server is told with the next run");
}

#[test]
fn an_imap_forward_marks_the_original_forwarded_once_sent() {
    let (mut s, a, original) = imap();
    let forward = send(&mut s, "mail.forward", original);
    assert_eq!(answered(&s, original), (false, false));
    sent(&mut s, a, vec![forward]);
    assert_eq!(answered(&s, original), (false, true));
}

#[test]
fn a_reply_deleted_or_moved_to_drafts_unsent_leaves_no_mark() {
    let (mut s, a, original) = imap();
    let reply = send(&mut s, "mail.reply", original);
    s.run("mail.delete", &json!({"ids": [reply]})).unwrap();
    sent(&mut s, a, Vec::new());
    assert_eq!(answered(&s, original), (false, false));
    let again = send(&mut s, "mail.reply", original);
    let drafts = s.mailbox.folder_of_kind(a, FolderKind::Drafts).unwrap();
    s.run("mail.move", &json!({"ids": [again], "folder": drafts})).unwrap();
    sent(&mut s, a, Vec::new());
    assert_eq!(answered(&s, original), (false, false));
    assert!(!pushes_answered(&s, a));
}

#[test]
fn an_earlier_sent_replys_mark_stays_when_a_later_one_is_deleted_unsent() {
    let (mut s, a, original) = imap();
    let first = send(&mut s, "mail.reply", original);
    sent(&mut s, a, vec![first]);
    let second = send(&mut s, "mail.reply", original);
    s.run("mail.delete", &json!({"ids": [second]})).unwrap();
    assert_eq!(answered(&s, original), (true, false));
}

#[test]
fn a_reply_kept_as_a_draft_and_sent_later_marks_the_original_once_sent() {
    let (mut s, a, original) = imap();
    s.run("mail.reply", &json!({"id": original})).unwrap();
    let draft = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    s.run("compose.close", &json!({})).unwrap();
    s.run("mail.editDraft", &json!({"id": draft})).unwrap();
    let reply = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(answered(&s, original), (false, false));
    sent(&mut s, a, vec![reply]);
    assert_eq!(answered(&s, original), (true, false));
}

#[test]
fn a_reply_whose_original_is_another_message_now_marks_nothing() {
    let (mut s, a, original) = imap();
    let reply = send(&mut s, "mail.reply", original);
    assert_eq!(s.mailbox.message(reply).unwrap().answers.as_ref().map(|o| o.message_id.as_str()), Some("plans@example.org"));
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let other = s.mailbox.add_message(Message { folder: inbox, message_id: "other@example.org".into(), ..Default::default() }).unwrap();
    s.mailbox.message_mut(reply).unwrap().answers.as_mut().unwrap().message = other;
    sent(&mut s, a, vec![reply]);
    assert_eq!((answered(&s, original), answered(&s, other)), ((false, false), (false, false)));
}

#[test]
fn undoing_a_send_not_gone_yet_leaves_the_original_unmarked() {
    let (mut s, a, original) = imap();
    let reply = send(&mut s, "mail.reply", original);
    assert!(s.undo());
    assert!(s.mailbox.message(reply).is_none());
    assert_eq!(answered(&s, original), (false, false));
    assert!(s.redo());
    assert_eq!(answered(&s, original), (false, false));
    sent(&mut s, a, vec![reply]);
    assert_eq!(answered(&s, original), (true, false));
}

/// A session with a Microsoft account kept in step through Graph, with one message of its server
/// in the Inbox; the account's and that message's ids.
fn graph() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let original = Message {
        account: a,
        folder: inbox,
        subject: "Plans".into(),
        from: Address::new("Bo", "bo@example.org"),
        server: Some(ServerMessage { id: "AAk1".into(), folder: "in".into(), ..Default::default() }),
        ..Default::default()
    };
    let id = s.mailbox.add_message(original).unwrap();
    (s, a, id)
}

fn graph_done(s: &mut Session, account: Id, message: Id, outcome: SendOutcome) {
    let result = GraphMailResult {
        account,
        email: "ada@example.org".into(),
        sent: vec![SendDone { message, outcome, draft: None, draft_left: String::new() }],
        ..Default::default()
    };
    apply_graph_mail(s, result).unwrap();
}

#[test]
fn a_graph_reply_marks_the_original_once_microsoft_took_it_and_a_held_one_never() {
    let (mut s, a, original) = graph();
    let reply = send(&mut s, "mail.reply", original);
    assert_eq!(answered(&s, original), (false, false));
    graph_done(&mut s, a, reply, SendOutcome::Held { lost: false, why: "refused".into() });
    assert_eq!(answered(&s, original), (false, false), "held: not sent");
    graph_done(&mut s, a, reply, SendOutcome::Held { lost: true, why: String::new() });
    assert_eq!(answered(&s, original), (false, false), "may have gone: not known to be sent");
    s.run("mail.delete", &json!({"ids": [reply]})).unwrap();
    assert_eq!(answered(&s, original), (false, false), "deleted unsent");
    let forward = send(&mut s, "mail.forward", original);
    graph_done(&mut s, a, forward, SendOutcome::Sent);
    assert_eq!(answered(&s, original), (false, true));
    let reply = send(&mut s, "mail.reply", original);
    graph_done(&mut s, a, reply, SendOutcome::Sent);
    assert_eq!(answered(&s, original), (true, true));
}

#[test]
fn a_local_accounts_reply_marks_the_original_at_once() {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let original = s
        .mailbox
        .add_message(Message { account: a, folder: inbox, subject: "Plans".into(), from: Address::new("", "bo@example.org"), ..Default::default() })
        .unwrap();
    s.run("mail.reply", &json!({"id": original})).unwrap();
    let r = s.run("compose.send", &json!({})).unwrap();
    assert_eq!(r["queued"], false);
    assert_eq!(answered(&s, original), (true, false));
    let filed = s.mailbox.message(r["id"].as_u64().unwrap()).unwrap();
    assert!(filed.answers.is_none());
}
