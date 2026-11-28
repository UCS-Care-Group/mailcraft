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
        let start = s.mailbox.clone();
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
        // Worked out on several threads, at first and brought in step since, they are the same.
        for threads in [2, 3, 16] {
            let on = crate::conversations::threads_on(&s.mailbox, &s.mailbox, threads);
            prop_assert_eq!(conversations(&s, &on), conversations(&s, &afresh), "built on {} threads", threads);
            let on = crate::conversations::threads_on(&start, &s.mailbox, threads);
            prop_assert_eq!(conversations(&s, &on), conversations(&s, &afresh), "built on {} threads, then in step", threads);
        }
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

/// How long the conversations take to work out over a large mailbox of real conversations: of
/// one to eight messages each, replies naming what they answer, and every twentieth conversation
/// of a topic many share ("Weekly report"). `MAILCRAFT_CONVERSATIONS_BENCH_MESSAGES` makes it
/// another size than 326,000. Run with `--release --ignored --nocapture`.
#[test]
#[ignore = "a benchmark: builds a mailbox of 326,000 messages in memory"]
fn conversations_bench() {
    let messages: u32 = std::env::var("MAILCRAFT_CONVERSATIONS_BENCH_MESSAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(326_000);
    let (mut s, inboxes) = mailbox();
    let mut n = 0u32;
    let mut conversation = 0u32;
    while n < messages {
        let size = 1 + conversation % 8;
        let topic = if conversation.is_multiple_of(20) { "Weekly report".to_string() } else { format!("Plans for the harbour, part {conversation}") };
        let mut references: Vec<String> = Vec::new();
        for k in 0..size {
            let id = format!("c{conversation}.{k}@example.org");
            let m = Message {
                folder: inboxes[(conversation % 2) as usize],
                subject: if k == 0 { topic.clone() } else { format!("Re: {topic}") },
                message_id: format!("<{id}>"),
                in_reply_to: references.last().cloned().unwrap_or_default(),
                references: references.clone(),
                ..Default::default()
            };
            s.mailbox.add_message(m).unwrap();
            references.push(format!("<{id}>"));
            n += 1;
        }
        conversation += 1;
    }
    s.mailbox_changed();
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let mut built = Vec::new();
    let mut afresh = Vec::new();
    for _ in 0..15 {
        let t = std::time::Instant::now();
        let threads = ThreadIndex::build(s.mailbox.messages.values().map(|m| m.as_ref()));
        afresh.push(ms(t.elapsed()));
        drop(threads);
        let fresh = Session::new(s.mailbox.clone());
        let t = std::time::Instant::now();
        let _ = fresh.threads();
        built.push(ms(t.elapsed()));
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let cores = std::thread::available_parallelism().map_or(1, |p| p.get());
    eprintln!(
        "CONVERSATIONS_BENCH {} messages, {cores} cores: ThreadIndex::build median {:.1} ms {afresh:.0?}; first build median {:.1} ms {built:.0?}",
        s.mailbox.messages.len(),
        median(&mut afresh),
        median(&mut built)
    );
    let _ = s.threads();
    let mut pieces = Vec::new();
    let mut joined = Vec::new();
    for p in 0..20u32 {
        for k in 0..50u32 {
            // A reply to an original there is, in its account and its topic.
            let c = (p * 50 + k) % conversation;
            let topic = if c.is_multiple_of(20) { "Weekly report".to_string() } else { format!("Plans for the harbour, part {c}") };
            let m = Message {
                folder: inboxes[(c % 2) as usize],
                subject: format!("Re: {topic}"),
                message_id: format!("<new{p}.{k}@example.org>"),
                in_reply_to: format!("<c{c}.0@example.org>"),
                ..Default::default()
            };
            joined.push((s.mailbox.add_message(m).unwrap(), format!("c{c}.0@example.org")));
        }
        s.mailbox_changed();
        let t = std::time::Instant::now();
        let _ = s.threads();
        pieces.push(ms(t.elapsed()));
    }
    // Outside the timer: each reply is of its original's conversation.
    let threads = s.threads();
    for (id, original) in joined {
        let original = s.mailbox.messages.values().find(|m| m.message_id.trim_matches(['<', '>']) == original).map(|m| m.id);
        assert!(original.is_some_and(|o| threads.same(id, o)));
    }
    eprintln!("CONVERSATIONS_BENCH a piece of 50 new messages brought in step: median {:.1} ms {pieces:.1?}", median(&mut pieces));
    assert_eq!(s.threads_built(), 1);
}

/// Worked out on threads over a mailbox of many chunks, then brought in step with changes, the
/// conversations are those of a build over every message on one.
#[test]
fn conversations_worked_out_on_threads_are_those_built_on_one() {
    let (mut s, inboxes) = mailbox();
    for n in 0..6_000u32 {
        let m = Message {
            folder: inboxes[(n % 2) as usize],
            subject: if n % 3 == 0 { format!("Topic {}", n % 70) } else { format!("Re: Topic {}", n % 70) },
            message_id: format!("more{n}@example.org"),
            in_reply_to: if n % 4 == 0 { format!("more{}@example.org", n.saturating_sub(70)) } else { String::new() },
            ..Default::default()
        };
        s.mailbox.add_message(m).unwrap();
    }
    s.mailbox_changed();
    let start = s.mailbox.clone();
    for n in 0..60u8 {
        let change = match n % 4 {
            0 => Change::Add { other: n % 8 == 0, subject: usize::from(n % 5), own: n % 12, answers: Some(n % 7), thread: None },
            1 => Change::Remove(usize::from(n) * 97),
            2 => Change::Subject(usize::from(n) * 131, usize::from(n % 5)),
            _ => Change::Answers(usize::from(n) * 59, Some(n % 12)),
        };
        apply(&mut s, inboxes, &change);
    }
    let afresh = ThreadIndex::build(s.mailbox.messages.values().map(|m| m.as_ref()));
    for threads in [1, 2, 3, 4, 7] {
        let on = crate::conversations::threads_on(&s.mailbox, &s.mailbox, threads);
        assert_eq!(conversations(&s, &on), conversations(&s, &afresh), "built on {threads} threads");
        let on = crate::conversations::threads_on(&start, &s.mailbox, threads);
        assert_eq!(conversations(&s, &on), conversations(&s, &afresh), "built on {threads} threads, then in step");
    }
}

/// A thread that gives no answer has the whole of the work done again on one: however many fail,
/// none's panic is passed on.
#[test]
fn threads_that_fail_give_no_answer_rather_than_a_panic() {
    let doubled = crate::conversations::on_threads(vec![1, 2, 3, 4], |n| n * 2);
    assert_eq!(doubled, Some(vec![2, 4, 6, 8]), "in their order");
    let failing = crate::conversations::on_threads(vec![1, 2, 3, 4], |n| {
        assert!(n > 2, "a worker that fails");
        n
    });
    assert_eq!(failing, None, "two threads failed, the last item (done here) didn't");
}

/// Many messages of one large topic gone at once are brought in step, not built afresh.
#[test]
fn many_messages_of_one_topic_gone_at_once_are_brought_in_step() {
    let (mut s, inboxes) = mailbox();
    for n in 0..3_000u32 {
        let m = Message {
            folder: inboxes[0],
            subject: if n == 0 { "Weekly report".into() } else { "Re: Weekly report".into() },
            message_id: format!("week{n}@example.org"),
            in_reply_to: if n % 5 == 0 { String::new() } else { format!("week{}@example.org", n - 1) },
            ..Default::default()
        };
        s.mailbox.add_message(m).unwrap();
    }
    s.mailbox_changed();
    let _ = s.threads();
    let gone: Vec<Id> = s.mailbox.messages.values().filter(|m| m.message_id.starts_with("week")).map(|m| m.id).step_by(4).collect();
    assert!(gone.len() > 700, "{} gone", gone.len());
    for id in gone {
        s.mailbox.remove_message(id).unwrap();
    }
    s.mailbox_changed();
    let kept = s.threads();
    assert_eq!(s.threads_built(), 1, "brought in step");
    let afresh = ThreadIndex::build(s.mailbox.messages.values().map(|m| m.as_ref()));
    assert_eq!(conversations(&s, &kept), conversations(&s, &afresh));
}

/// Two topics that share a hash are told apart by their topic itself, and one topic of two
/// accounts too.
#[test]
fn messages_of_a_shared_hash_are_put_by_their_topic() {
    let (mut s, inboxes) = mailbox();
    let mut add = |folder, subject: &str| {
        let id = s
            .mailbox
            .add_message(Message { folder, subject: subject.into(), message_id: format!("<{subject}{folder}@example.org>"), ..Default::default() })
            .unwrap();
        s.mailbox.message(id).unwrap().clone()
    };
    let (a, b, c) = (add(inboxes[0], "Plans"), add(inboxes[0], "Invoice"), add(inboxes[1], "Plans"));
    let mut into = Vec::new();
    crate::conversations::by_topic(vec![(&a, a.topic_key()), (&b, b.topic_key()), (&c, c.topic_key())], &mut into);
    assert_eq!(into.len(), 3);
    let mut into = Vec::new();
    crate::conversations::by_topic(vec![(&a, a.topic_key()), (&a, a.topic_key())], &mut into);
    assert_eq!(into.len(), 1);
}
