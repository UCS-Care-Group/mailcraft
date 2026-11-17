// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar's job in flight, for every front end: the request a plan makes, and its answer
//! taken in. The job is played here (nothing leaves this machine).

use std::sync::mpsc::{Receiver, channel};

use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::{Busy, CalendarFacts, CalendarPlan};
use mailcraft_model::remote::{EventsDownload, RemoteEvent};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Id, OAuth};
use serde_json::json;

use crate::calendar_job::{CalendarAnswer, CalendarAuth, CalendarJobs, CalendarPolled, calendar_request, take_answer};
use crate::graph::{GraphRequest, GraphResult};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn session() -> (Session, Id) {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn auth() -> CalendarAuth {
    CalendarAuth {
        oauth: OAuth { client_id: "app".into(), ..Default::default() },
        refresh: "refresh".into(),
        access: String::new(),
        favorites: false,
    }
}

fn plan(s: &mut Session, account: Id) -> CalendarPlan {
    let facts = CalendarFacts { focused: false, backstage: false, reachable: true, busy: Busy::default() };
    let look = s.calendar_due(&facts, 1_000.0);
    look.plans.into_iter().find(|p| p.account == account).unwrap()
}

#[test]
fn a_plan_makes_a_request_for_the_calendar_alone() {
    let (mut s, a) = session();
    let plan = plan(&mut s, a);
    let req: GraphRequest = calendar_request(&s, &plan, auth()).unwrap();
    assert_eq!((req.account.id, req.refresh.as_str(), req.oauth.client_id.as_str()), (a, "refresh", "app"));
    assert!(req.mail.is_none() && req.mail_pieces.is_none() && req.outbox.is_empty(), "nothing of the mail");
    assert!(!req.favorites && !req.favorites_scopes);
    assert_eq!(req.directory, plan.directory);
    assert_eq!(req.calendar, Some(plan.range(s.now())));
    let gone = CalendarPlan { account: 999, ..plan };
    assert!(calendar_request(&s, &gone, auth()).is_none());
}

#[test]
fn a_job_started_is_told_to_the_engine_and_its_answer_taken_in() {
    let (mut s, a) = session();
    let plan = plan(&mut s, a);
    let mut jobs = CalendarJobs::default();
    let (from, to) = plan.range(s.now());
    let started = jobs.start(&mut s, &plan, auth(), 1_000.0, |req| {
        let (tx, rx) = channel();
        let events = EventsDownload {
            account: req.account.id,
            email: req.account.email.clone(),
            from,
            to,
            complete: true,
            events: vec![RemoteEvent {
                id: "e-1".into(),
                change_key: "ck".into(),
                title: "Budget review".into(),
                start: NOW + 3600,
                end: NOW + 7200,
                ..Default::default()
            }],
            ..Default::default()
        };
        let _ = tx.send(GraphResult { account: req.account.id, events: Some(events), ..Default::default() });
        Some(rx)
    });
    assert!(started && jobs.busy() && jobs.account() == Some(a));
    // One at a time.
    assert!(!jobs.start(&mut s, &plan, auth(), 1_000.0, |_| panic!("not handed")));
    let answer = loop {
        match jobs.poll(&s) {
            CalendarPolled::Answer(answer) => break answer,
            CalendarPolled::Nothing => {}
            CalendarPolled::Stale => panic!("the same mailbox"),
        }
    };
    assert!(!answer.stopped_early && answer.account == a);
    let taken = take_answer(&mut s, &answer, false, 2_000.0);
    assert!(taken.taken && taken.errors.is_empty());
    assert!(s.mailbox.events.values().any(|e| e.title == "Budget review"));
}

#[test]
fn nothing_starts_without_a_way_to_run_it() {
    let (mut s, a) = session();
    let plan = plan(&mut s, a);
    let mut jobs = CalendarJobs::default();
    assert!(!jobs.start(&mut s, &plan, auth(), 1_000.0, |_| None::<Receiver<GraphResult>>));
    assert!(!jobs.busy());
}

#[test]
fn a_job_that_ends_without_an_answer_says_so_and_one_for_another_mailbox_is_let_go_of() {
    let (mut s, a) = session();
    let plan = plan(&mut s, a);
    let mut jobs = CalendarJobs::default();
    assert!(jobs.start(&mut s, &plan, auth(), 1_000.0, |_| Some(channel().1)));
    match jobs.poll(&s) {
        CalendarPolled::Answer(answer) => assert!(answer.stopped_early && answer.result.events.is_none()),
        _ => panic!("an answer"),
    }
    let (tx, rx) = channel::<GraphResult>();
    assert!(jobs.start(&mut s, &plan, auth(), 1_000.0, |_| Some(rx)));
    let mailbox = s.mailbox.clone();
    s.replace_mailbox(mailbox);
    let _ = tx.send(GraphResult::default());
    assert!(matches!(jobs.poll(&s), CalendarPolled::Stale));
    assert!(!jobs.busy());
}

/// An answer to a calendar job that brings an event, with `tokens`.
fn answered(s: &mut Session, jobs: &mut CalendarJobs, plan: &CalendarPlan, tokens: Tokens) -> Box<CalendarAnswer> {
    let (from, to) = plan.range(s.now());
    let started = jobs.start(s, plan, auth(), 1_000.0, |req| {
        let (tx, rx) = channel();
        let events = EventsDownload {
            account: req.account.id,
            email: req.account.email.clone(),
            from,
            to,
            complete: true,
            events: vec![RemoteEvent { id: "e-9".into(), title: "Not yours".into(), start: NOW + 3600, end: NOW + 7200, ..Default::default() }],
            ..Default::default()
        };
        let _ = tx.send(GraphResult { account: req.account.id, events: Some(events), tokens: Some(tokens), ..Default::default() });
        Some(rx)
    });
    assert!(started);
    loop {
        if let CalendarPolled::Answer(answer) = jobs.poll(s) {
            break answer;
        }
    }
}

#[test]
fn an_answer_with_somebody_elses_sign_in_is_refused_whole() {
    let (mut s, a) = session();
    let plan = plan(&mut s, a);
    let mut jobs = CalendarJobs::default();
    let theirs = Tokens { refresh: "theirs".into(), access: "theirs".into(), email: "mallory@example.net".into(), ..Default::default() };
    let answer = answered(&mut s, &mut jobs, &plan, theirs);
    assert_eq!(answer.foreign.as_ref().map(|f| f.signed_in_as.as_str()), Some("mallory@example.net"));
    assert!(answer.result.tokens.is_none() && answer.result.events.is_none(), "nothing of it handed on");
    // Taken as a job that failed, whatever the front end says: nothing reaches the mailbox.
    let taken = take_answer(&mut s, &answer, false, 2_000.0);
    assert!(!taken.taken);
    assert!(!s.mailbox.events.values().any(|e| e.title == "Not yours"));
}

#[test]
fn the_accounts_own_sign_in_is_taken_in_as_before() {
    let (mut s, a) = session();
    let plan = plan(&mut s, a);
    let mut jobs = CalendarJobs::default();
    let own = Tokens { refresh: "newer".into(), email: "ADA@example.org".into(), ..Default::default() };
    let answer = answered(&mut s, &mut jobs, &plan, own);
    assert!(answer.foreign.is_none());
    assert_eq!(answer.result.tokens.as_ref().map(|t| t.refresh.as_str()), Some("newer"));
    assert!(take_answer(&mut s, &answer, false, 2_000.0).taken);
    assert!(s.mailbox.events.values().any(|e| e.title == "Not yours"));
}

#[test]
fn a_shared_mailboxs_calendar_is_judged_by_the_sign_in_it_is_opened_through() {
    let (mut s, owner) = session();
    let shared = s.run("account.addSharedMailbox", &json!({"address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.owner_of(shared).map(|o| o.id), Some(owner));
    let mut jobs = CalendarJobs::default();
    // The owner's own sign-in, which names the owner and not the shared mailbox: its own.
    let plan = CalendarPlan { account: shared, ..plan(&mut s, owner) };
    let own = Tokens { refresh: "r".into(), email: "ada@example.org".into(), ..Default::default() };
    assert!(answered(&mut s, &mut jobs, &plan, own).foreign.is_none());
    // Somebody else's: refused.
    let theirs = Tokens { refresh: "r".into(), email: "mallory@example.net".into(), ..Default::default() };
    assert!(answered(&mut s, &mut jobs, &plan, theirs).foreign.is_some());
}
