// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! AU3-17: after an IMAP mailbox is numbered anew (its UIDVALIDITY changed), a move or a
//! permanent deletion made here and not pushed yet goes to the one copy of that mail on the
//! server, and only once the mailbox has been read whole: when several messages have its
//! Message-ID, in whichever pieces of a download or runs they come, the change is dropped and
//! the owner told, never made on another message. Every message here is invented.

use mailcraft_engine::Session;
use mailcraft_engine::cmd::account::{apply_sync, sync_request};
use mailcraft_model::sync::{FolderDelta, RemoteOp, SyncResult};
use mailcraft_model::{Address, Body, FolderKind, Id, Importance, Message, Pending};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug)]
enum Change {
    Delete,
    Move,
}

fn from_server(uid: u32, subject: &str) -> Message {
    Message {
        uid,
        subject: subject.into(),
        message_id: format!("{}@mail.example", subject.to_lowercase()),
        from: Address::new("", "x@example.org"),
        date: 100 + i64::from(uid),
        ..Default::default()
    }
}

fn news(id: Option<Id>, remote: &str, kind: FolderKind, uid_validity: u32, new: Vec<Message>) -> FolderDelta {
    let uid_seen = new.iter().map(|m| m.uid).max().unwrap_or(0);
    FolderDelta { id, remote: remote.into(), kind, uid_validity, uid_seen, new, ..Default::default() }
}

fn apply(s: &mut Session, result: SyncResult) -> Value {
    let applied = apply_sync(s, result).unwrap();
    assert!(s.mailbox.clone().repair().is_empty(), "{:?}", s.mailbox.clone().repair());
    applied
}

/// An IMAP account read once: One, Two and Three in its INBOX (UIDVALIDITY 7), and an Archive.
fn synced() -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let a = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let first = SyncResult {
        account: a,
        folders: vec![
            FolderDelta {
                complete: true,
                ..news(None, "INBOX", FolderKind::Inbox, 7, vec![from_server(1, "One"), from_server(2, "Two"), from_server(3, "Three")])
            },
            FolderDelta { complete: true, ..news(None, "Archive", FolderKind::Archive, 8, vec![]) },
        ],
        ..Default::default()
    };
    apply(&mut s, first);
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let archive = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    (s, a, inbox, archive)
}

/// Three changed here (deleted for good, or filed), not pushed yet; then the server numbers the
/// INBOX anew (7 to 70). The copy of Three that is the server's, and one more message the sender
/// made to look like it to the letter, but for its text.
fn changed_then_renumbered(change: Change) -> (Session, Id, Id, Message, Message) {
    let (mut s, a, inbox, archive) = synced();
    let three = s.mailbox.messages_in(inbox).find(|m| m.subject == "Three").map(|m| Message::clone(m)).unwrap();
    match change {
        Change::Delete => s.run("mail.deletePermanently", &json!({"id": three.id})).unwrap(),
        Change::Move => s.run("mail.move", &json!({"ids": [three.id], "folder": archive})).unwrap(),
    };
    let text = |text: &str| Body { text: text.into(), html: None };
    let legit = Message { id: 0, folder: 0, uid: 30, body: text("The real one."), ..three.clone() };
    let lookalike = Message { id: 0, folder: 0, uid: 31, body: text("Not the real one."), ..three };
    (s, a, inbox, legit, lookalike)
}

fn piece(a: Id, inbox: Id, new: Vec<Message>, first: bool, complete: bool) -> SyncResult {
    let removed = if first { vec![1, 2, 3] } else { Vec::new() };
    SyncResult {
        account: a,
        partial: !complete,
        folders: vec![FolderDelta { removed, complete, ..news(Some(inbox), "INBOX", FolderKind::Inbox, 70, new) }],
        ..Default::default()
    }
}

fn destructive(s: &Session, a: Id) -> Vec<RemoteOp> {
    sync_request(s, a).unwrap().ops.into_iter().filter(|op| matches!(op, RemoteOp::Delete { .. } | RemoteOp::Move { .. })).collect()
}

fn texts(s: &Session, inbox: Id) -> Vec<(u32, String)> {
    let mut now: Vec<(u32, String)> = s.mailbox.messages_in(inbox).map(|m| (m.uid, m.body.text.clone())).collect();
    now.sort();
    now
}

fn told(answers: &[Value]) -> bool {
    answers.iter().any(|r| r["errors"].as_array().is_some_and(|e| e.iter().any(|n| n.as_str().is_some_and(|n| n.contains("not made on the server")))))
}

/// v02: the first piece of the renumbered INBOX brings only the newest message; the second, both
/// copies of Three. Nothing is deleted or moved on the server, both copies stay, and the owner is
/// told.
#[test]
fn two_copies_in_a_later_piece_get_no_change() {
    for change in [Change::Delete, Change::Move] {
        let (mut s, a, inbox, legit, lookalike) = changed_then_renumbered(change);
        let one = apply(&mut s, piece(a, inbox, vec![from_server(40, "Newest")], true, false));
        assert!(destructive(&s, a).is_empty(), "{change:?}");
        let two = apply(&mut s, piece(a, inbox, vec![legit, lookalike], false, true));
        assert!(destructive(&s, a).is_empty(), "{change:?}: {:?}", destructive(&s, a));
        assert!(s.mailbox.pending.is_empty(), "{change:?}: {:?}", s.mailbox.pending);
        assert!(told(&[one, two]), "{change:?}");
        assert_eq!(texts(&s, inbox), [(30, "The real one.".into()), (31, "Not the real one.".into()), (40, String::new())], "{change:?}");
    }
}

/// The copies in two pieces, and in two runs: the first comes alone, before the mailbox has been
/// read whole, and takes nothing; the second makes two, and the change is dropped.
#[test]
fn copies_counted_across_pieces_and_runs() {
    for change in [Change::Delete, Change::Move] {
        // Two pieces of one run.
        let (mut s, a, inbox, legit, lookalike) = changed_then_renumbered(change);
        let one = apply(&mut s, piece(a, inbox, vec![lookalike.clone(), from_server(40, "Newest")], true, false));
        assert!(destructive(&s, a).is_empty(), "{change:?}: nothing goes before the mailbox is read whole");
        let two = apply(&mut s, piece(a, inbox, vec![legit.clone()], false, true));
        assert!(destructive(&s, a).is_empty() && s.mailbox.pending.is_empty(), "{change:?}");
        assert!(told(&[one, two]));
        assert_eq!(s.mailbox.count(inbox), 3);

        // A run cut short, then the next one, the mailbox no longer new in its numbers.
        let (mut s, a, inbox, legit, lookalike) = changed_then_renumbered(change);
        let one = apply(&mut s, SyncResult { partial: false, ..piece(a, inbox, vec![lookalike], true, false) });
        assert!(destructive(&s, a).is_empty());
        let two = apply(&mut s, SyncResult { partial: false, ..piece(a, inbox, vec![legit], false, true) });
        assert!(destructive(&s, a).is_empty() && s.mailbox.pending.is_empty(), "{change:?}");
        assert!(told(&[one, two]));
    }
}

/// One copy alone, the mailbox read whole: the change goes to it, it goes from here, and the
/// next run makes the change on the server.
#[test]
fn one_copy_the_mailbox_read_whole_takes_the_change() {
    for change in [Change::Delete, Change::Move] {
        let (mut s, a, inbox, legit, _) = changed_then_renumbered(change);
        apply(&mut s, piece(a, inbox, vec![from_server(40, "Newest")], true, false));
        apply(&mut s, piece(a, inbox, vec![legit], false, true));
        let ops = destructive(&s, a);
        match change {
            Change::Delete => assert_eq!(ops, [RemoteOp::Delete { from: "INBOX".into(), uid: 30 }]),
            Change::Move => assert!(matches!(ops.as_slice(), [RemoteOp::Move { uid: 30, to, .. }] if to == "Archive"), "{ops:?}"),
        }
        assert!(matches!(s.mailbox.pending.as_slice(), [p] if p.source() == ("INBOX", 30)));
        assert!(!s.mailbox.pending.iter().any(|p| matches!(p, Pending::Delete { uid: 0, .. } | Pending::Move { uid: 0, .. })));
        assert_eq!(texts(&s, inbox), [(40, String::new())], "{change:?}");
    }
}

/// One copy that isn't the same mail (another sender), the mailbox read whole: dropped, and said.
#[test]
fn a_copy_that_differs_gets_no_change() {
    for change in [Change::Delete, Change::Move] {
        let (mut s, a, inbox, legit, _) = changed_then_renumbered(change);
        let other = Message { from: Address::new("", "real@example.org"), ..legit };
        let r = apply(&mut s, piece(a, inbox, vec![other], true, true));
        assert!(destructive(&s, a).is_empty() && s.mailbox.pending.is_empty(), "{change:?}");
        assert!(told(&[r]));
    }
}

/// While a change waits, the copy it waits for is neither new mail nor the rules' to act on; a
/// message that only carries its Message-ID (another sender) is new mail as any other, and the
/// owner's rules act on it.
#[test]
fn only_the_awaited_mail_is_kept_from_the_rules() {
    for change in [Change::Delete, Change::Move] {
        let (mut s, a, inbox, legit, _) = changed_then_renumbered(change);
        let rule = json!({"name": "Three", "conditions": [{"type": "subjectContains", "value": "Three"}], "actions": [{"type": "setImportance", "value": "high"}]});
        s.run("rules.add", &rule).unwrap();
        let other = Message { uid: 32, from: Address::new("", "someone@example.org"), ..legit.clone() };
        let r = apply(&mut s, piece(a, inbox, vec![legit, from_server(40, "Newest")], true, false));
        assert_eq!(r["new"].as_u64(), Some(1), "{change:?}: only Newest is new: {r}");
        let copy = s.mailbox.messages_in(inbox).find(|m| m.uid == 30).map(|m| m.importance).unwrap();
        assert_eq!(copy, Importance::Normal, "{change:?}: the rules left the awaited copy alone");
        let r = apply(&mut s, piece(a, inbox, vec![other], false, false));
        assert_eq!(r["new"].as_u64(), Some(1), "{change:?}: {r}");
        let other = s.mailbox.messages_in(inbox).find(|m| m.uid == 32).map(|m| m.importance).unwrap();
        assert_eq!(other, Importance::High, "{change:?}: a message with the id alone goes through the rules");
    }
}
