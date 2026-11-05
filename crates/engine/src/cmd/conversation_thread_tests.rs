// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A conversation is one thread with one subject (#795): two unrelated messages that happen to
//! have the same subject are two conversations in the list, in the reading pane and for
//! Delete, and mail kept from before conversation ids were asked for gets them from its server,
//! a run at a time.

use mailcraft_model::sync::{ServerMessage, ThreadOf};
use mailcraft_model::{Account, AccountKind, FolderKind, Id, Mailbox, Message};
use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn mail(folder: Id, subject: &str, message_id: &str, when: i64) -> Message {
    Message { folder, subject: subject.into(), message_id: message_id.into(), date: when, received: when, ..Default::default() }
}

/// A mailbox on this computer with two unrelated "Invoice for October" messages from two senders,
/// one of them answered from Sent Items.
fn invoices() -> (Session, [Id; 3]) {
    let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
    let account = mb.accounts[0].id;
    let (inbox, sent) = (mb.folder_of_kind(account, FolderKind::Inbox).unwrap(), mb.folder_of_kind(account, FolderKind::Sent).unwrap());
    let ours = mb.add_message(mail(inbox, "Invoice for October", "inv.1@one.example", 1_000)).unwrap();
    let answer = Message {
        in_reply_to: "inv.1@one.example".into(),
        references: vec!["inv.1@one.example".into()],
        ..mail(sent, "RE: Invoice for October", "inv.2@example.org", 2_000)
    };
    let answer = mb.add_message(answer).unwrap();
    let theirs = mb.add_message(mail(inbox, "Invoice for October", "inv.1@two.example", 3_000)).unwrap();
    let mut s = Session::new(mb);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.conversations", json!({"value": true}));
    (s, [ours, answer, theirs])
}

#[test]
fn two_unrelated_messages_with_one_subject_are_two_rows_and_delete_conversation_takes_one() {
    let (mut s, [ours, answer, theirs]) = invoices();
    let rows = s.rows();
    let row = |id: Id| rows.iter().find(|r| r.members.contains(&id)).unwrap();
    assert_eq!(rows.len(), 2, "two conversations, not one");
    assert_eq!((row(ours).members.clone(), row(ours).others.clone()), (vec![ours], vec![answer]));
    assert_eq!((row(theirs).members.clone(), row(theirs).others.clone()), (vec![theirs], vec![]));
    // The reading pane's conversation, as the list's.
    assert_eq!(*s.conversation(ours), [ours, answer]);
    assert_eq!(*s.conversation(theirs), [theirs]);
    // Delete on ours, its closed row picked (it stands for its conversation): theirs stays.
    run(&mut s, "mail.select", json!({"id": ours}));
    run(&mut s, "mail.delete", json!({}));
    let deleted = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Deleted).unwrap();
    let inbox = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Inbox).unwrap();
    assert_eq!(s.mailbox.message(ours).unwrap().folder, deleted);
    assert_eq!(s.mailbox.message(theirs).unwrap().folder, inbox, "the unrelated message is left where it was");
    // And opening one conversation opens it alone.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "mail.expandConversation", json!({"id": ours}));
    let rows = s.rows();
    assert!(rows.iter().filter(|r| r.under.is_some()).all(|r| r.id != theirs));
    assert!(!rows.iter().find(|r| r.id == theirs).unwrap().open);
}

/// A Microsoft account's mail kept by an earlier version: headers and a preview, and no
/// conversation id yet.
fn microsoft(ids: &[(&str, &str)]) -> (Session, Id, Vec<Id>) {
    let mut mb = Mailbox::default();
    let account =
        mb.add_account(Account { name: "me@example.org".into(), email: "me@example.org".into(), kind: AccountKind::Microsoft, ..Default::default() });
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let made: Vec<Id> = ids
        .iter()
        .enumerate()
        .map(|(n, (server, message_id))| {
            let when = 1_000 * i64::try_from(n).unwrap();
            let server = ServerMessage { id: (*server).into(), ..Default::default() };
            mb.add_message(Message { account, server: Some(server), unfetched: true, ..mail(inbox, "Invoice", message_id, when) }).unwrap()
        })
        .collect();
    (Session::new(mb), account, made)
}

#[test]
fn mail_kept_from_before_gets_its_conversation_ids_a_run_at_a_time() {
    let (mut s, account, made) = microsoft(&[("AAk-1", "a@one.example"), ("AAk-2", "b@two.example"), ("AAk-3", "c@three.example")]);
    // Until they come, they stay together as before.
    assert_eq!(*s.conversation(made[0]), made);
    // Newest first, each once.
    let asked = crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap().threads;
    assert_eq!(asked, ["AAk-3", "AAk-2", "AAk-1"]);
    // The server says two; of the third it says none: it isn't asked again.
    let threads = vec![
        ThreadOf { id: "AAk-1".into(), thread: "AAQkconvA".into(), ..Default::default() },
        ThreadOf { id: "AAk-2".into(), thread: "AAQkconvB".into(), ..Default::default() },
        ThreadOf { id: "AAk-3".into(), thread: String::new(), ..Default::default() },
        // One it wasn't asked about, or that isn't this account's, is nothing.
        ThreadOf { id: "AAk-9".into(), thread: "AAQkconvA".into(), ..Default::default() },
    ];
    let result = mailcraft_model::sync::GraphMailResult { account, email: "me@example.org".into(), threads, ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    assert!(crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap().threads.is_empty());
    let server = |id: Id| s.mailbox.message(id).unwrap().server.clone().unwrap();
    assert_eq!((server(made[0]).thread, server(made[2]).thread_asked), ("AAQkconvA".to_string(), true));
    // Now each is what its id says: A and B apart; C, still not known, alone among the known.
    assert_eq!(*s.conversation(made[0]), [made[0]]);
    assert_eq!(*s.conversation(made[1]), [made[1]]);
    assert_eq!(*s.conversation(made[2]), [made[2]]);
}

#[test]
fn a_listing_without_a_conversation_id_keeps_the_one_known() {
    let (mut s, account, made) = microsoft(&[("AAk-1", "a@one.example")]);
    s.mailbox.message_mut(made[0]).unwrap().server.as_mut().unwrap().thread = "AAQkconvA".into();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "F".into(), ..Default::default() });
    // A delta that says the message again without its conversation (as a JMAP window's marks do).
    let there = mailcraft_model::sync::GraphMessage {
        server: ServerMessage { id: "AAk-1".into(), read: true, ..Default::default() },
        subject: "Invoice".into(),
        received: 5,
        ..Default::default()
    };
    let page = mailcraft_model::sync::MessagePage { folder: "F".into(), messages: vec![there], ..Default::default() };
    let result = mailcraft_model::sync::GraphMailResult { account, email: "me@example.org".into(), pages: vec![page], ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    let server = s.mailbox.message(made[0]).unwrap().server.clone().unwrap();
    assert_eq!((server.read, server.thread.as_str()), (true, "AAQkconvA"));
}

#[test]
fn hostile_conversation_answers_are_held_to_their_sizes() {
    let (mut s, account, made) = microsoft(&[("AAk-1", "a@one.example")]);
    let mut threads: Vec<ThreadOf> =
        (0..5_000).map(|n| ThreadOf { id: format!("junk-{n}"), thread: "x".repeat(10_000), ..Default::default() }).collect();
    threads.push(ThreadOf { id: "AAk-1".into(), thread: "conv\n\u{0}".into(), ..Default::default() });
    let result = mailcraft_model::sync::GraphMailResult { account, email: "me@example.org".into(), threads, ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    // Past the most a run asks about, nothing is looked at; an id that is none is none.
    let server = s.mailbox.message(made[0]).unwrap().server.clone().unwrap();
    assert!(server.thread.is_empty());
}

#[test]
fn a_server_that_refuses_to_say_is_asked_a_few_runs_and_then_left_alone() {
    let (mut s, account, made) = microsoft(&[("AAk-1", "a@one.example")]);
    for run in 1..=mailcraft_model::sync::MAX_THREAD_TRIES {
        assert_eq!(crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap().threads, ["AAk-1"], "run {run}");
        let threads = vec![ThreadOf::refused("AAk-1")];
        let result = mailcraft_model::sync::GraphMailResult { account, email: "me@example.org".into(), threads, ..Default::default() };
        crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    }
    assert!(crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap().threads.is_empty());
    let server = s.mailbox.message(made[0]).unwrap().server.clone().unwrap();
    assert!(server.thread.is_empty() && server.thread_asked);
}

#[test]
fn a_flag_or_a_move_keeps_the_conversations_and_a_copy_works_them_out_again() {
    let (mut s, [ours, _, theirs]) = invoices();
    let threads = s.threads();
    run(&mut s, "mail.flag", json!({"id": ours}));
    assert!(std::sync::Arc::ptr_eq(&threads, &s.threads()), "a flag doesn't build them again");
    run(&mut s, "mail.select", json!({"id": theirs}));
    run(&mut s, "mail.delete", json!({}));
    assert!(std::sync::Arc::ptr_eq(&threads, &s.threads()), "nor does a move");
    // A message more: worked out again, and right.
    let inbox = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Inbox).unwrap();
    run(&mut s, "mail.copy", json!({"ids": [ours], "folder": inbox}));
    let now = s.threads();
    assert!(!std::sync::Arc::ptr_eq(&threads, &now));
    assert!(!now.same(ours, theirs));
}

#[test]
fn a_changed_header_or_conversation_id_is_a_change_of_the_conversations() {
    let (s, [ours, ..]) = invoices();
    let before = s.mailbox.clone();
    let mut after = s.mailbox.clone();
    assert!(crate::conversations::grouping_kept(&before, &after));
    after.message_mut(ours).unwrap().read = true;
    assert!(crate::conversations::grouping_kept(&before, &after));
    after.message_mut(ours).unwrap().references.push("x@example.org".into());
    assert!(!crate::conversations::grouping_kept(&before, &after));
    let mut after = s.mailbox.clone();
    after.message_mut(ours).unwrap().server = Some(ServerMessage { thread: "AAQkconvA".into(), ..Default::default() });
    assert!(!crate::conversations::grouping_kept(&before, &after));
}

#[test]
fn two_known_threads_with_their_own_conversation_ids_stay_apart() {
    // Each thread's first message, fetched whole, has a Message-ID and its server's id; their
    // answers too. Same subject, two senders.
    let (mut s, account, _) = microsoft(&[]);
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let add = |s: &mut Session, server: &str, thread: &str, message_id: &str, parent: &str, when: i64| {
        let server = ServerMessage { id: server.into(), thread: thread.into(), ..Default::default() };
        let refs = if parent.is_empty() { vec![] } else { vec![parent.to_string()] };
        let m = Message { account, server: Some(server), in_reply_to: parent.into(), references: refs, ..mail(inbox, "Invoice", message_id, when) };
        s.mailbox.add_message(m).unwrap()
    };
    let x1 = add(&mut s, "AAk-1", "AAQkconvX", "x1@x.example", "", 1);
    let y1 = add(&mut s, "AAk-2", "AAQkconvY", "y1@y.example", "", 2);
    let x2 = add(&mut s, "AAk-3", "AAQkconvX", "x2@example.org", "x1@x.example", 3);
    let y2 = add(&mut s, "AAk-4", "AAQkconvY", "y2@example.org", "y1@y.example", 4);
    assert_eq!(*s.conversation(x2), [x1, x2]);
    assert_eq!(*s.conversation(y2), [y1, y2]);
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    for id in s.conversation(x2).to_vec() {
        s.mailbox.delete_message(id).unwrap();
    }
    assert!([y1, y2].iter().all(|id| s.mailbox.message(*id).unwrap().folder == inbox));
    assert!([x1, x2].iter().all(|id| s.mailbox.message(*id).unwrap().folder == deleted));
}

/// A flag over 326,000 messages, timed (`cargo test --release -p mailcraft-engine -- --ignored
/// a_flag_over_a_large_mailbox --nocapture`).
#[test]
#[ignore]
fn a_flag_over_a_large_mailbox() {
    let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
    let account = mb.accounts[0].id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let mut first = 0;
    for n in 0..326_000i64 {
        let subject = if n % 10 == 0 { "Weekly report".to_string() } else { format!("Topic {}", n % 9_000) };
        let id = mb.add_message(mail(inbox, &subject, &format!("m{n}@example.org"), n)).unwrap();
        if n == 0 {
            first = id;
        }
    }
    let mut s = Session::new(mb);
    let started = std::time::Instant::now();
    let threads = s.threads();
    println!("build: {:?}", started.elapsed());
    let started = std::time::Instant::now();
    run(&mut s, "mail.flag", json!({"id": first}));
    let _ = s.threads();
    println!("flag and threads after: {:?}", started.elapsed());
    run(&mut s, "folder.select", json!({"id": inbox}));
    let started = std::time::Instant::now();
    run(&mut s, "mail.markAllRead", json!({}));
    let _ = s.threads();
    println!("mark all read and threads after: {:?}", started.elapsed());
    let started = std::time::Instant::now();
    run(&mut s, "mail.flag", json!({"id": first}));
    let _ = s.threads();
    println!("a flag after it, and threads after: {:?}", started.elapsed());
    assert!(std::sync::Arc::ptr_eq(&threads, &s.threads()));
    assert!(s.threads_checked().1);
}

#[test]
fn what_is_kept_points_at_each_message_as_it_is_and_hashes_it_once() {
    let (mut s, [ours, ..]) = invoices();
    let threads = s.threads();
    // Every message changed: each is hashed again once, and then pointed at as it is now.
    run(&mut s, "mail.markAllRead", json!({}));
    assert!(std::sync::Arc::ptr_eq(&threads, &s.threads()));
    let (after_all, current) = s.threads_checked();
    assert!(current, "nothing kept holds a message as it was");
    assert!(after_all >= 2, "the unread ones were changed and checked: {after_all}");
    // A flag after it: only the flagged message is hashed again.
    run(&mut s, "mail.flag", json!({"id": ours}));
    assert!(std::sync::Arc::ptr_eq(&threads, &s.threads()));
    let (after_flag, current) = s.threads_checked();
    assert!(current);
    assert!(after_flag.saturating_sub(after_all) <= 1, "{after_all} then {after_flag}");
}

/// An Exchange server account whose server doesn't know `ConversationId`, with two unrelated
/// "Invoice for October" from two senders.
fn exchange_invoices() -> (Session, Id, [Id; 2]) {
    use mailcraft_model::sync::{EwsFolderDeltas, EwsMailResult, EwsMessagePage, GraphFolder, GraphMessage};
    const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": "ada@example.org", "kind": "exchange", "ews": {"url": URL}})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let base = || EwsMailResult { account, email: "ada@example.org".into(), url: URL.into(), partial: true, ..Default::default() };
    let inbox = GraphFolder { id: "F1".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
    let folders = EwsFolderDeltas { changed: vec![inbox], afresh: true, ..Default::default() };
    crate::cmd::ews_mail::apply_ews_mail(&mut s, EwsMailResult { folders: Some(folders), hierarchy: "H1".into(), ..base() }).unwrap();
    let invoice = |id: &str, from: &str, when: i64| GraphMessage {
        server: ServerMessage { id: id.into(), size: 1_200, change_key: "CK1".into(), ..Default::default() },
        message_id: format!("{id}@{from}.example"),
        subject: "Invoice for October".into(),
        sent: when - 60,
        received: when,
        ..Default::default()
    };
    let page = EwsMessagePage {
        folder: "F1".into(),
        messages: vec![invoice("AAMk-X", "x", 1_000), invoice("AAMk-Y", "y", 2_000)],
        afresh: true,
        last: true,
        state: "S1".into(),
        ..Default::default()
    };
    crate::cmd::ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![page], ..base() }).unwrap();
    crate::cmd::ews_mail::apply_ews_mail(&mut s, EwsMailResult { whole: true, partial: false, ..base() }).unwrap();
    let id = |server: &str| {
        s.mailbox.messages.values().find(|m| m.account == account && m.server.as_ref().is_some_and(|x| x.id == server)).map(|m| m.id).unwrap()
    };
    let ids = [id("AAMk-X"), id("AAMk-Y")];
    (s, account, ids)
}

#[test]
fn mail_whose_server_wont_say_its_conversation_threads_by_its_headers_once_asked() {
    let (mut s, account, [x, y]) = exchange_invoices();
    const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";
    // Never asked yet: together, as before.
    assert_eq!(*s.conversation(x), [x, y]);
    for run in 1..=mailcraft_model::sync::MAX_THREAD_TRIES {
        let asked = crate::cmd::ews_mail::ews_mail_request(&s, account, false).unwrap().threads;
        assert_eq!(asked, ["AAMk-Y", "AAMk-X"], "run {run}");
        let threads = asked.iter().map(|id| ThreadOf::refused(id)).collect();
        let result = mailcraft_model::sync::EwsMailResult {
            account,
            email: "ada@example.org".into(),
            url: URL.into(),
            whole: true,
            threads,
            ..Default::default()
        };
        crate::cmd::ews_mail::apply_ews_mail(&mut s, result).unwrap();
    }
    assert!(crate::cmd::ews_mail::ews_mail_request(&s, account, false).unwrap().threads.is_empty());
    // Asked as often as it is: linked by their headers alone, which link nothing.
    assert_eq!(*s.conversation(x), [x]);
    assert_eq!(*s.conversation(y), [y]);
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    run(&mut s, "folder.select", json!({"id": inbox}));
    run(&mut s, "view.conversations", json!({"value": true}));
    run(&mut s, "mail.select", json!({"id": x}));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!(s.mailbox.message(y).unwrap().folder, inbox, "the other invoice is left where it was");
    assert_ne!(s.mailbox.message(x).unwrap().folder, inbox);
}
