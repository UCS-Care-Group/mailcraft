// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A conversation is one thread with one topic (#795).

use super::*;
use crate::sync::ServerMessage;
use crate::{Account, FolderKind, Mailbox};

fn mail(id: Id, subject: &str, message_id: &str) -> Message {
    Message { id, account: 1, subject: subject.into(), message_id: message_id.into(), ..Default::default() }
}

fn reply(id: Id, subject: &str, message_id: &str, parent: &str) -> Message {
    Message { in_reply_to: parent.into(), references: vec![parent.into()], ..mail(id, subject, message_id) }
}

/// A Microsoft account's message as its listing brings it: headers and a preview, no
/// `In-Reply-To` or `References`, and the server's conversation id.
fn listed(id: Id, subject: &str, message_id: &str, thread: &str) -> Message {
    Message { server: Some(ServerMessage { id: format!("srv-{id}"), thread: thread.into(), ..Default::default() }), ..mail(id, subject, message_id) }
}

#[test]
fn two_unrelated_messages_with_one_subject_are_two_conversations() {
    let a = mail(1, "Invoice for October", "a.1@one.example");
    let b = mail(2, "Invoice for October", "b.2@two.example");
    let threads = ThreadIndex::build([&a, &b]);
    assert!(!threads.same(1, 2));
    assert_eq!(threads.members(1), [1]);
    assert_eq!(threads.members(2), [2]);
}

#[test]
fn a_reply_chain_is_one_conversation_through_its_references() {
    let first = mail(1, "Lunch?", "first@example.org");
    let answer = reply(2, "RE: lunch?", "answer@example.org", "first@example.org");
    // Its references alone name the first; it answers the second.
    let again = Message {
        in_reply_to: "answer@example.org".into(),
        references: vec!["<first@example.org>".into(), "answer@example.org".into()],
        ..mail(3, "Re: RE: Lunch?", "again@example.org")
    };
    let other = mail(4, "Lunch?", "other@example.org");
    let threads = ThreadIndex::build([&first, &answer, &again, &other]);
    assert_eq!(threads.members(3), [1, 2, 3]);
    assert_eq!(threads.key(1), ThreadKey(1));
    assert_eq!(threads.members(4), [4]);
}

#[test]
fn children_of_a_parent_that_isnt_here_are_one_conversation() {
    let a = reply(5, "Re: Plan", "a@example.org", "gone@example.org");
    let b = reply(6, "Re: Plan", "b@example.org", "gone@example.org");
    assert!(ThreadIndex::build([&a, &b]).same(5, 6));
}

#[test]
fn the_subject_must_still_match() {
    let a = mail(1, "Plan", "a@example.org");
    let b = reply(2, "Something else", "b@example.org", "a@example.org");
    let threads = ThreadIndex::build([&a, &b]);
    assert!(!threads.same(1, 2));
    // Nor are two accounts' messages ever one.
    let c = Message { account: 2, ..reply(3, "Re: Plan", "c@example.org", "a@example.org") };
    assert!(!ThreadIndex::build([&a, &c]).same(1, 3));
    // And a message without a subject is its own.
    let d = mail(4, "", "d@example.org");
    let e = reply(5, "", "e@example.org", "d@example.org");
    assert!(!ThreadIndex::build([&d, &e]).same(4, 5));
}

#[test]
fn a_servers_conversation_id_links_messages_listed_without_their_headers() {
    let a = listed(1, "Budget", "a@example.org", "AAQkADconv1");
    let b = listed(2, "RE: Budget", "b@example.org", "AAQkADconv1");
    let c = listed(3, "Budget", "c@example.org", "AAQkADconv2");
    // The same id, another topic: two.
    let d = listed(4, "Holidays", "d@example.org", "AAQkADconv1");
    let threads = ThreadIndex::build([&a, &b, &c, &d]);
    assert_eq!(threads.members(1), [1, 2]);
    assert_eq!(threads.members(3), [3]);
    assert_eq!(threads.members(4), [4]);
}

#[test]
fn messages_listed_before_conversation_ids_were_asked_for_stay_together_until_they_come() {
    // Listed by a version that didn't ask for the conversation id: nothing links them, nor could.
    let a = listed(1, "Invoice", "a@example.org", "");
    let b = listed(2, "Invoice", "b@example.org", "");
    // Known: linked to nothing, it is its own.
    let c = listed(3, "Invoice", "c@example.org", "AAQkADconv3");
    let d = mail(4, "Invoice", "d@example.org");
    let threads = ThreadIndex::build([&a, &b, &c, &d]);
    assert!(unknown(&a) && !unknown(&c) && !unknown(&d));
    assert_eq!(threads.members(1), [1, 2]);
    assert_eq!(threads.members(3), [3]);
    assert_eq!(threads.members(4), [4]);
    // Once their ids come, they are what the ids say.
    let a = listed(1, "Invoice", "a@example.org", "AAQkADconvA");
    let b = listed(2, "Invoice", "b@example.org", "AAQkADconvB");
    assert!(!ThreadIndex::build([&a, &b, &c, &d]).same(1, 2));
    // A message with no Message-ID at all is never known.
    assert!(unknown(&mail(5, "Invoice", "")));
}

#[test]
fn a_thread_moved_between_accounts_of_other_kinds_stays_one() {
    // On an IMAP server (Gmail): headers only.
    let first = mail(1, "Trip", "trip.1@example.org");
    let answer = reply(2, "Re: Trip", "trip.2@example.org", "trip.1@example.org");
    // Moved to a Microsoft account: the server gives each its own conversation id there, or none
    // yet; the headers still link them.
    let moved = |m: &Message, thread: &str| Message {
        account: 7,
        server: Some(ServerMessage { id: format!("srv-{}", m.id), thread: thread.into(), ..Default::default() }),
        ..m.clone()
    };
    let (a, b) = (moved(&first, "AAQkADx"), moved(&answer, "AAQkADy"));
    assert!(ThreadIndex::build([&a, &b]).same(1, 2));
    let (a, b) = (moved(&first, ""), moved(&answer, ""));
    assert!(ThreadIndex::build([&a, &b]).same(1, 2));
    // And on to this computer: the server's part goes, the headers stay.
    let local = |m: &Message| Message { account: 9, server: None, ..m.clone() };
    assert!(ThreadIndex::build([&local(&a), &local(&b)]).same(1, 2));
}

#[test]
fn hostile_headers_and_ids_are_capped_and_never_panic() {
    // Ten thousand references: only the last 32 link.
    let many: Vec<String> = (0..10_000).map(|n| format!("ref.{n}@example.org")).collect();
    let flood = Message { references: many, ..mail(1, "Flood", "flood@example.org") };
    let early = mail(2, "Flood", "ref.10@example.org");
    let late = mail(3, "Flood", "ref.9999@example.org");
    let threads = ThreadIndex::build([&flood, &early, &late]);
    assert!(!threads.same(1, 2), "a reference past the last 32 links nothing");
    assert!(threads.same(1, 3));
    assert!(threads.work() < 200, "{}", threads.work());
    // A megabyte of an id links nothing, however many share it.
    let huge = "x".repeat(1 << 20);
    let (a, b) = (mail(4, "Huge", &huge), reply(5, "Huge", "b@example.org", &huge));
    assert!(!ThreadIndex::build([&a, &b]).same(4, 5));
    // Nor does a conversation id that isn't printable ASCII of at most 256 bytes: it is none.
    let long = "c".repeat(257);
    for junk in ["conv\nid", "c\u{e9}", long.as_str()] {
        let a = Message { in_reply_to: "p6@example.org".into(), ..listed(6, "Odd", "a6@example.org", junk) };
        let b = Message { in_reply_to: "p7@example.org".into(), ..listed(7, "Odd", "a7@example.org", junk) };
        assert!(!ThreadIndex::build([&a, &b]).same(6, 7), "{junk:?}");
        assert!(unknown(&listed(6, "Odd", "a6@example.org", junk)), "{junk:?}");
    }
    // Messages that name each other, and one that names itself, are one conversation.
    let a = reply(8, "Loop", "loop.a@example.org", "loop.b@example.org");
    let b = reply(9, "Re: loop", "loop.b@example.org", "loop.a@example.org");
    let c = reply(10, "Re: loop", "loop.c@example.org", "loop.c@example.org");
    let threads = ThreadIndex::build([&a, &b, &c]);
    assert_eq!(threads.members(8), [8, 9]);
    assert_eq!(threads.members(10), [10]);
    // The same message twice (a copy in another folder) is one conversation.
    let (a, b) = (mail(11, "Copy", "copy@example.org"), mail(12, "Copy", "copy@example.org"));
    assert!(ThreadIndex::build([&a, &b]).same(11, 12));
}

/// 326,000 messages, as the largest mailbox the window is measured with, 30,000 of them of one
/// topic (a ticket system's notices) in chains of replies, linked by headers and by conversation
/// ids: the work stays linear.
#[test]
fn a_large_mailbox_with_one_large_topic_is_linear_work() {
    const MESSAGES: u64 = 326_000;
    const SAME: u64 = 30_000;
    let messages: Vec<Message> = (1..=MESSAGES)
        .map(|id| {
            if id <= SAME {
                // Chains of 100: each answers the one before; every tenth chain is a server's.
                let chain = id / 100;
                let parent = if id % 100 == 0 { String::new() } else { format!("n.{}@example.org", id - 1) };
                let thread = if chain % 10 == 0 { format!("conv-{chain}") } else { String::new() };
                Message {
                    in_reply_to: parent.clone(),
                    references: if parent.is_empty() { Vec::new() } else { vec![format!("n.{}@example.org", chain * 100), parent] },
                    server: (!thread.is_empty()).then(|| ServerMessage { thread, ..Default::default() }),
                    ..mail(id, "Your ticket was updated", &format!("n.{id}@example.org"))
                }
            } else {
                mail(id, &format!("Topic {}", id % 50_000), &format!("n.{id}@example.org"))
            }
        })
        .collect();
    let threads = ThreadIndex::build(&messages);
    assert!(threads.work() <= 12 * MESSAGES, "{} steps for {MESSAGES} messages", threads.work());
    assert_eq!(threads.members(150).len(), 100);
    assert!(threads.same(101, 199) && !threads.same(99, 101));
}

#[test]
fn delete_conversation_leaves_an_unrelated_message_of_the_same_subject() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { name: "me@example.org".into(), email: "me@example.org".into(), ..Default::default() });
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let sent = mb.folder_of_kind(account, FolderKind::Sent).unwrap();
    let add = |mb: &mut Mailbox, folder, m: Message| mb.add_message(Message { account, folder, ..m }).unwrap();
    let ours = add(&mut mb, inbox, mail(0, "Invoice for October", "ours.1@one.example"));
    let answer = add(&mut mb, sent, reply(0, "RE: Invoice for October", "ours.2@example.org", "ours.1@one.example"));
    let theirs = add(&mut mb, inbox, mail(0, "Invoice for October", "theirs.1@two.example"));
    assert_eq!(mb.conversation(ours), [ours, answer]);
    assert_eq!(mb.conversation(theirs), [theirs]);
    for id in mb.conversation(ours) {
        mb.delete_message(id).unwrap();
    }
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    assert_eq!(mb.message(theirs).unwrap().folder, inbox);
    assert!([ours, answer].iter().all(|id| mb.message(*id).unwrap().folder == deleted));
}

#[test]
fn two_threads_never_join_through_mail_kept_from_before() {
    // L1 and L2 kept from before (no conversation id, headers only), from two senders; R1 answers
    // L1 and R2 answers L2, each with its server's own conversation id.
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { name: "me@example.org".into(), email: "me@example.org".into(), ..Default::default() });
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let add = |mb: &mut Mailbox, m: Message| mb.add_message(Message { account, folder: inbox, ..m }).unwrap();
    let answers = |m: Message, parent: &str| Message { in_reply_to: parent.into(), references: vec![parent.into()], ..m };
    let l1 = add(&mut mb, listed(0, "Invoice for October", "l1@x.example", ""));
    let l2 = add(&mut mb, listed(0, "Invoice for October", "l2@y.example", ""));
    let r1 = add(&mut mb, answers(listed(0, "RE: Invoice for October", "r1@example.org", "AAQkconvA"), "l1@x.example"));
    let r2 = add(&mut mb, answers(listed(0, "RE: Invoice for October", "r2@example.org", "AAQkconvB"), "l2@y.example"));
    assert_eq!(mb.conversation(r1), [l1, r1]);
    assert_eq!(mb.conversation(r2), [l2, r2]);
    for id in mb.conversation(r1) {
        mb.delete_message(id).unwrap();
    }
    assert!([l2, r2].iter().all(|id| mb.message(*id).unwrap().folder == inbox), "Y's mail is left where it was");
}

#[test]
fn mail_its_server_was_asked_about_links_by_its_headers_alone() {
    let asked = |id: Id, message_id: &str| {
        let mut m = listed(id, "Invoice for October", message_id, "");
        if let Some(server) = m.server.as_mut() {
            server.thread_asked = true;
        }
        m
    };
    let (x, y) = (asked(1, "x@x.example"), asked(2, "y@y.example"));
    assert!(!unknown(&x) && !unknown(&y));
    assert!(!ThreadIndex::build([&x, &y]).same(1, 2));
    // Its answer still joins it, by its headers.
    let answer = Message { in_reply_to: "x@x.example".into(), ..asked(3, "RE: Invoice for October") };
    let answer = Message { subject: "RE: Invoice for October".into(), message_id: "r@example.org".into(), ..answer };
    assert!(ThreadIndex::build([&x, &y, &answer]).same(1, 3));
    // Without a Message-ID it is still not known.
    assert!(unknown(&asked(4, "")));
}
