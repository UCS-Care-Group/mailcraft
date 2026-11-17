// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar's job in flight, for every front end: the request a plan makes, and its answer
//! taken in. The job is played here (nothing leaves this machine).

use std::sync::mpsc::{Receiver, channel};

use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::{Busy, CalendarFacts, CalendarPlan};
use mailcraft_model::remote::{EventsDownload, RemoteEvent};
use mailcraft_model::{Id, OAuth};
use serde_json::json;

use crate::calendar_job::{CalendarAuth, CalendarJobs, CalendarPolled, calendar_request, take_answer};
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
