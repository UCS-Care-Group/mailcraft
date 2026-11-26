// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1897: the conversations kept are brought in step with a change by working out again only
//! the topics it touched, and they come out as a build over every message does.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{FolderKind, Id, Mailbox, Message, ThreadIndex};
use proptest::prelude::*;

use crate::Session;

const SUBJECTS: [&str; 5] = ["Invoice for October", "RE: Invoice for October", "Plans", "Fw: plans", ""];

/// A change to the mailbox, by a message's place among those there are (wrapped around).
#[derive(Clone, Debug)]
enum Change {
    Add { other: bool, subject: usize, own: u8, answers: Option<u8>, thread: Option<u8> },
    Remove(usize),
    Subject(usize, usize),
    Answers(usize, Option<u8>),
    Thread(usize, Option<u8>),
    Read(usize),
}

fn change() -> impl Strategy<Value = Change> {
    let id = || prop::option::of(0u8..12);
    prop_oneof![
        4 => (any::<bool>(), 0..SUBJECTS.len(), 0u8..12, id(), id())
            .prop_map(|(other, subject, own, answers, thread)| Change::Add { other, subject, own, answers, thread }),
        2 => any::<usize>().prop_map(Change::Remove),
        1 => (any::<usize>(), 0..SUBJECTS.len()).prop_map(|(at, subject)| Change::Subject(at, subject)),
        1 => (any::<usize>(), id()).prop_map(|(at, answers)| Change::Answers(at, answers)),
        1 => (any::<usize>(), id()).prop_map(|(at, thread)| Change::Thread(at, thread)),
        1 => any::<usize>().prop_map(Change::Read),
    ]
}

fn message_id(n: u8) -> String {
    format!("m{n}@example.org")
}

/// Two accounts, and enough messages in them to fill more than one chunk of the map of messages.
fn mailbox() -> (Session, [Id; 2]) {
    let mut s = Session::new(Mailbox::with_local_account("Avery", "avery@example.org"));
    let other = s.run("account.add", &serde_json::json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    let other = other["id"].as_u64().unwrap();
    let first = s.mailbox.accounts[0].id;
    let inboxes = [s.mailbox.folder_of_kind(first, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap()];
    for n in 0..1_100u32 {
        let m = Message {
            folder: inboxes[(n % 2) as usize],
            subject: format!("Topic {}", n % 40),
            message_id: format!("base{n}@example.org"),
            in_reply_to: if n % 3 == 0 { format!("base{}@example.org", n.saturating_sub(40)) } else { String::new() },
            ..Default::default()
        };
        s.mailbox.add_message(m).unwrap();
    }
    s.mailbox_changed();
    (s, inboxes)
}

fn pick(s: &Session, at: usize) -> Option<Id> {
    let ids: Vec<Id> = s.mailbox.messages.keys().copied().collect();
    ids.get(at % ids.len().max(1)).copied()
}

fn apply(s: &mut Session, inboxes: [Id; 2], change: &Change) {
    let mb = &mut s.mailbox;
    match change {
        Change::Add { other, subject, own, answers, thread } => {
            let m = Message {
                folder: inboxes[usize::from(*other)],
                subject: SUBJECTS[*subject].into(),
                message_id: message_id(*own),
                in_reply_to: answers.map(message_id).unwrap_or_default(),
                server: thread.map(|t| ServerMessage { thread: format!("AAQkconv{t}"), ..Default::default() }),
                ..Default::default()
            };
            mb.add_message(m).unwrap();
        }
        Change::Remove(at) => {
            if let Some(id) = pick(s, *at) {
                s.mailbox.remove_message(id).unwrap();
            }
        }
        Change::Subject(at, subject) => {
            if let Some(id) = pick(s, *at) {
                s.mailbox.message_mut(id).unwrap().subject = SUBJECTS[*subject].into();
            }
        }
        Change::Answers(at, answers) => {
            if let Some(id) = pick(s, *at) {
                s.mailbox.message_mut(id).unwrap().in_reply_to = answers.map(message_id).unwrap_or_default();
            }
        }
        Change::Thread(at, thread) => {
            if let Some(id) = pick(s, *at) {
                s.mailbox.message_mut(id).unwrap().server = thread.map(|t| ServerMessage { thread: format!("AAQkconv{t}"), ..Default::default() });
            }
        }
        Change::Read(at) => {
            if let Some(id) = pick(s, *at) {
                let m = s.mailbox.message_mut(id).unwrap();
                m.read = !m.read;
            }
        }
    }
    s.mailbox_changed();
}

/// Every message's conversation, as `threads` has it.
fn conversations(s: &Session, threads: &ThreadIndex) -> Vec<(Id, Id, Vec<Id>)> {
    s.mailbox.messages.keys().map(|id| (*id, threads.key(*id).0, threads.members(*id))).collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn conversations_brought_in_step_are_those_built_afresh(changes in prop::collection::vec(change(), 1..40), every in 1usize..6) {
        let (mut s, inboxes) = mailbox();
        let _ = s.threads();
        for (n, change) in changes.iter().enumerate() {
            apply(&mut s, inboxes, change);
            // Asked after some of the changes, not each: several come together, as a frame takes
            // in several pieces.
            if n % every == 0 {
                let kept = s.threads();
                let afresh = ThreadIndex::build(s.mailbox.messages.values().map(|m| m.as_ref()));
                prop_assert_eq!(conversations(&s, &kept), conversations(&s, &afresh), "after {:?}", &changes[..=n]);
            }
        }
        let kept = s.threads();
        let afresh = ThreadIndex::build(s.mailbox.messages.values().map(|m| m.as_ref()));
        prop_assert_eq!(conversations(&s, &kept), conversations(&s, &afresh));
        // Brought in step each time: built over every message only once.
        prop_assert_eq!(s.threads_built(), 1);
    }
}

/// New mail of one account, a piece at a time, doesn't have every message of the mailbox worked
/// out again: only its topics are (#1897).
#[test]
fn new_mail_works_out_its_own_topics_only() {
    let (mut s, inboxes) = mailbox();
    let _ = s.threads();
    for n in 0..200u8 {
        apply(&mut s, inboxes, &Change::Add { other: true, subject: usize::from(n % 4), own: n % 12, answers: Some(n % 7), thread: None });
        let _ = s.threads();
    }
    assert_eq!(s.threads_built(), 1);
    // A mailbox opened in place of this one is worked out afresh.
    let mut other = s.mailbox.clone();
    for id in other.messages.keys().copied().collect::<Vec<_>>() {
        other.message_mut(id).unwrap().subject = "Everything".into();
    }
    s.mailbox = other;
    s.mailbox_changed();
    let threads = s.threads();
    assert_eq!(s.threads_built(), 2);
    let afresh = ThreadIndex::build(s.mailbox.messages.values().map(|m| m.as_ref()));
    assert_eq!(conversations(&s, &threads), conversations(&s, &afresh));
}
