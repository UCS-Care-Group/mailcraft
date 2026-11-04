// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The thread of a JMAP email (`threadId`, #795): read with what a message list shows of it, and
//! asked for once, a hundred to a call, of mail listed before it was. Over the stand-in; every
//! message is invented.

use std::sync::atomic::AtomicBool;

use super::*;
use crate::standin::{SESSION, StandIn, TOKEN};
use crate::{Credentials, block_on};

const ME: &str = "ada@example.org";

fn mail(subject: &str, id: &str, answers: Option<&str>) -> Vec<u8> {
    let answers = answers.map(|a| format!("In-Reply-To: <{a}>\r\nReferences: <{a}>\r\n")).unwrap_or_default();
    format!("From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: {subject}\r\nMessage-ID: <{id}>\r\n{answers}Date: Thu, 15 Oct 2026 09:30:00 +0200\r\n\r\nHello.\r\n")
        .into_bytes()
}

fn run(stand_in: &StandIn, threads: Vec<String>) -> Vec<JmapMailResult> {
    let request = JmapMailRequest {
        account: 3,
        email: ME.into(),
        session: SESSION.into(),
        bearer: true,
        window: 50,
        list_folders: true,
        scope: JmapScope::All,
        threads,
        ..Default::default()
    };
    let client = block_on(Connection::connect(stand_in.clone(), SESSION, Credentials::bearer(TOKEN).unwrap())).unwrap();
    let mut pieces = Vec::new();
    block_on(sync_mail(&client, &request, &AtomicBool::new(false), &mut |piece| pieces.push(piece)));
    pieces
}

#[test]
fn a_window_brings_each_emails_thread_and_mail_from_before_is_asked_for_it() {
    let stand_in = StandIn::new();
    let (first, answer, other) = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        let first = d.deliver(&inbox, &mail("Invoice", "a@one.example", None), &[], "2026-01-01T00:00:01Z");
        let answer = d.deliver(&inbox, &mail("RE: Invoice", "b@one.example", Some("a@one.example")), &[], "2026-01-01T00:00:02Z");
        let other = d.deliver(&inbox, &mail("Invoice", "c@two.example", None), &[], "2026-01-01T00:00:03Z");
        (first, answer, other)
    });
    let pieces = run(&stand_in, Vec::new());
    let listed: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).collect();
    let thread = |id: &str| listed.iter().find(|m| m.server.id == id).map(|m| m.server.thread.clone()).unwrap();
    assert!(!thread(&first).is_empty());
    assert_eq!(thread(&first), thread(&answer));
    assert_ne!(thread(&first), thread(&other));
    // Asked for by id: the same threads, and none for one the server doesn't have.
    let pieces = run(&stand_in, vec![first.clone(), other.clone(), "Mgone".into()]);
    let last = pieces.last().unwrap();
    let said = |id: &str| last.threads.iter().find(|t| t.id == id).map(|t| t.thread.clone());
    assert_eq!(said(&first), Some(thread(&first)));
    assert_eq!(said(&other), Some(thread(&other)));
    assert_eq!(said("Mgone").as_deref(), Some(""));
    assert!(last.errors.is_empty(), "{:?}", last.errors);
}

#[test]
fn the_threads_are_asked_no_more_at_a_time_than_the_server_takes() {
    let stand_in = StandIn::new();
    let ids: Vec<String> = stand_in.with(|d| {
        d.max_get = 2;
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        (0..5).map(|n| d.deliver(&inbox, &mail("Note", &format!("n{n}@one.example"), None), &[], "2026-01-01T00:00:01Z")).collect()
    });
    let pieces = run(&stand_in, ids.clone());
    let last = pieces.last().unwrap();
    assert!(last.errors.is_empty(), "{:?}", last.errors);
    assert_eq!(last.threads.len(), 5, "{:?}", last.threads);
    assert!(last.threads.iter().all(|t| !t.thread.is_empty() && !t.refused));
}
