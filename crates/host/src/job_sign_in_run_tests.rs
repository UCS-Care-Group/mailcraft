// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A calendar or To Do job whose renewed sign-in is somebody else's sends nothing and reads
//! nothing: the run stops before its first request, and its answer is refused whole. Played
//! against the stand-in for Microsoft Graph (`mailcraft_net::graph::standin`), with renewals that
//! ask nobody. Nothing here leaves this machine; every address is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::mpsc::channel;

use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::{Busy, CalendarFacts, CalendarPlan};
use mailcraft_model::remote::{EventChangeKind, EventFields, EventOp, EventsUpload};
use mailcraft_model::remote_tasks::{TaskOp, TaskOpKind, TasksUpload};
use mailcraft_model::sync::{GraphMailRequest, SendUp, Tokens};
use mailcraft_model::{Account, AccountKind, Address, Id, OAuth, OpenedCalendar, TaskPatch};
use mailcraft_net::NetError;
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use mailcraft_net::oauth::GraphTokens;
use serde_json::json;

use crate::calendar_job::{CalendarAuth, CalendarJobs, CalendarPolled, calendar_request};
use crate::graph::{GraphRequest, GraphResult, graph_in_with, graph_sending_with};
use crate::tasks::{TasksJob, run_with};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const OWNER: &str = "ada@example.org";
const STRANGER: &str = "mallory@example.net";

/// A renewal that names `who` as the one who signed in, with a token the stand-in takes.
fn signed_in_as(who: &str) -> Result<GraphTokens, NetError> {
    let tokens =
        Tokens { access: TOKEN.into(), refresh: "ref-2".into(), expires_at: mailcraft_time::now() + 3_600, email: who.into(), ..Default::default() };
    Ok(GraphTokens { tokens, extras_missing: false, covers_favorites: false })
}

fn stand_in() -> StandIn {
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        state.owner = OWNER.into();
        state.add_todo_list("Tasks", "defaultList");
    }
    server
}

// --- To Do ---------------------------------------------------------------------------------

fn tasks_job() -> TasksJob {
    let account = Account { id: 7, kind: AccountKind::Microsoft, email: OWNER.into(), ..Default::default() };
    let create =
        TaskOp { kind: TaskOpKind::Create, task: 3, patch: TaskPatch { title: Some("Mine".into()), ..Default::default() }, ..Default::default() };
    TasksJob {
        upload: TasksUpload { ops: vec![create], ..Default::default() }.for_account(&account),
        account,
        oauth: OAuth::default(),
        refresh: "r".into(),
        zone: mailcraft_time::Zone::UTC,
        cancel: Default::default(),
    }
}

#[test]
fn a_to_do_job_with_somebody_elses_sign_in_sends_nothing_and_reads_nothing() {
    let server = stand_in();
    let _route = server.route();
    let out = run_with(tasks_job(), &|_, _| signed_in_as(STRANGER));
    assert!(server.state().requests.is_empty(), "nothing asked of the server");
    assert!(server.state().todo.iter().all(|list| list.tasks.is_empty()), "no task made in the stranger's account");
    assert!(out.uploaded.is_none() && out.download.is_none() && out.errors.is_empty());
    assert!(!out.consent_needed && !out.sign_in_refused && !out.denied);
    // Only the tokens come back, for the poll to say whose they were and refuse them.
    assert_eq!(out.tokens.as_ref().map(|t| t.email.as_str()), Some(STRANGER));
}

#[test]
fn a_to_do_job_with_the_accounts_own_sign_in_goes_as_before() {
    // (The same job, the account's own sign-in: the stand-in is asked, so the test above shows
    // the refusal and not a route that goes nowhere.)
    let server = stand_in();
    let _route = server.route();
    let out = run_with(tasks_job(), &|_, _| signed_in_as("ADA@example.org"));
    assert!(!server.state().requests.is_empty());
    assert!(out.uploaded.is_some() && out.download.is_some(), "{:?}", out.errors);
}

// --- The calendar --------------------------------------------------------------------------

fn session() -> (Session, Id) {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let added = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap();
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

/// The calendar job due for `account`, carrying a new meeting with somebody invited.
fn plan_with_meeting(s: &mut Session, account: Id) -> CalendarPlan {
    let facts = CalendarFacts { focused: false, backstage: false, reachable: true, busy: Busy::default() };
    let look = s.calendar_due(&facts, 1_000.0);
    // (Whichever account's plan is due: only its stretch of days is borrowed.)
    let due = look.plans.into_iter().next().unwrap();
    let meeting = EventOp {
        kind: EventChangeKind::Create,
        event: 41,
        token: "mark-41".into(),
        fields: EventFields { title: Some("Planning".into()), ..Default::default() },
        meeting: true,
        ..Default::default()
    };
    CalendarPlan { account, changes: Some(EventsUpload { account, email: String::new(), ops: vec![meeting], ..Default::default() }), ..due }
}

/// Run the calendar job `plan` says through the Graph run against the stand-in, every renewal
/// naming `who`, and poll its answer.
fn run_calendar(s: &mut Session, plan: &CalendarPlan, who: &'static str) -> Box<crate::calendar_job::CalendarAnswer> {
    let mut jobs = CalendarJobs::default();
    let started = jobs.start(s, plan, auth(), 1_000.0, |req: GraphRequest| {
        let (tx, rx) = channel();
        let _ = tx.send(graph_sending_with(req, "UTC", &|_, _, _| signed_in_as(who), &|_, _| signed_in_as(who)));
        Some(rx)
    });
    assert!(started);
    match jobs.poll(s) {
        CalendarPolled::Answer(answer) => answer,
        _ => panic!("the answer came"),
    }
}

/// Nothing of the run reached the server or came back from it.
fn nothing_done(server: &StandIn, result: &GraphResult) {
    assert!(server.state().requests.is_empty(), "nothing asked of the server");
    assert!(server.state().writes().is_empty());
    assert!(result.uploaded.is_none() && result.events.is_none() && result.calendars.is_none() && result.contacts.is_none());
    assert!(result.sent.is_empty() && result.errors.is_empty());
}

#[test]
fn a_calendar_job_with_somebody_elses_sign_in_sends_nothing_and_reads_nothing() {
    let server = stand_in();
    let _route = server.route();
    let (mut s, a) = session();
    let plan = plan_with_meeting(&mut s, a);
    // The run itself stops before its first request.
    let request = calendar_request(&s, &plan, auth()).unwrap();
    assert_eq!(request.signer.as_ref().map(|o| o.id), Some(a), "the account's own sign-in is the one checked");
    let result = graph_sending_with(request, "UTC", &|_, _, _| signed_in_as(STRANGER), &|_, _| signed_in_as(STRANGER));
    nothing_done(&server, &result);
    assert_eq!(result.tokens.as_ref().map(|t| t.email.as_str()), Some(STRANGER));
    // And the job's poll refuses what comes back whole.
    let answer = run_calendar(&mut s, &plan, STRANGER);
    nothing_done(&server, &answer.result);
    assert!(answer.foreign.is_some() && answer.result.tokens.is_none(), "refused, its token not handed on");
}

#[test]
fn a_calendar_job_with_the_accounts_own_sign_in_goes_as_before() {
    let server = stand_in();
    let _route = server.route();
    let (mut s, a) = session();
    let plan = plan_with_meeting(&mut s, a);
    let answer = run_calendar(&mut s, &plan, OWNER);
    assert!(answer.foreign.is_none());
    // (The stand-in turns down a meeting with no time, but it was told: the meeting went up.)
    assert!(server.state().writes().iter().any(|(method, path)| method == "POST" && path.last().is_some_and(|p| p == "events")));
    assert!(answer.result.uploaded.is_some());
}

#[test]
fn a_shared_mailboxs_calendar_job_is_checked_against_its_owners_sign_in_before_anything_goes() {
    let server = stand_in();
    let _route = server.route();
    let (mut s, owner) = session();
    let shared = s.run("account.addSharedMailbox", &json!({"address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    let plan = plan_with_meeting(&mut s, shared);
    let request = calendar_request(&s, &plan, auth()).unwrap();
    assert_eq!(request.account.id, shared);
    assert_eq!(request.signer.as_ref().map(|o| o.id), Some(owner), "the owner's sign-in is the one checked");
    // A stranger's sign-in, by the renewal for sending from the mailbox (the meeting invites
    // people) and by the ordinary one alike: nothing goes up and nothing is read.
    let answer = run_calendar(&mut s, &plan, STRANGER);
    nothing_done(&server, &answer.result);
    assert!(answer.foreign.is_some());
    // The owner's own sign-in (which names the owner, not the shared mailbox) is not refused.
    let answer = run_calendar(&mut s, &plan, OWNER);
    assert!(answer.foreign.is_none());
    assert!(!server.state().requests.is_empty());
}

#[test]
fn the_token_for_other_peoples_calendars_is_checked_too() {
    // An account with a calendar opened by address renews a token of its own for it first: a
    // stranger's stops the run there.
    let server = stand_in();
    let _route = server.route();
    let (mut s, a) = session();
    let plan = plan_with_meeting(&mut s, a);
    let mut request = calendar_request(&s, &plan, auth()).unwrap();
    request.account.opened_calendars =
        vec![OpenedCalendar { address: "lee@example.org".into(), name: "Lee".into(), ticked: true, ..Default::default() }];
    let result = graph_in_with(request, "UTC", &|_, _| signed_in_as(STRANGER));
    nothing_done(&server, &result);
    assert_eq!(result.tokens.as_ref().map(|t| t.email.as_str()), Some(STRANGER));
}

// --- Mail's runs ---------------------------------------------------------------------------

/// A mail run of `account` through Graph, every renewal naming `who`, with `outbox` to send
/// and, when `mail`, the folders to read.
fn mail_run(
    account: Account,
    signer: Account,
    outbox: Vec<mailcraft_model::Message>,
    mail: Option<GraphMailRequest>,
    who: &'static str,
) -> GraphResult {
    let request = GraphRequest {
        account,
        oauth: OAuth { client_id: "app".into(), ..Default::default() },
        refresh: "refresh".into(),
        access: String::new(),
        outbox,
        directory: false,
        calendar: None,
        changes: None,
        asks: Vec::new(),
        settings: false,
        favorites: false,
        favorites_scopes: false,
        mail,
        mail_pieces: None,
        cancel: Default::default(),
        signer: Some(signer),
        contacts: None,
    };
    graph_sending_with(request, "UTC", &|_, _, _| signed_in_as(who), &|_, _| signed_in_as(who))
}

fn ada() -> Account {
    Account { id: 4, kind: AccountKind::Microsoft, email: OWNER.into(), ..Default::default() }
}

fn hello() -> mailcraft_model::Message {
    mailcraft_model::Message { id: 9, to: vec![Address::new("Bob", "bob@example.org")], subject: "Hello".into(), ..Default::default() }
}

#[test]
fn an_outbox_run_with_somebody_elses_sign_in_sends_nothing() {
    let server = stand_in();
    let _route = server.route();
    let out = mail_run(ada(), ada(), vec![hello()], None, STRANGER);
    assert!(server.state().requests.is_empty(), "nothing asked of the server");
    assert!(server.state().sent.is_empty(), "no message sent as the stranger");
    assert!(out.sent.is_empty() && out.held.is_empty() && out.unsure.is_empty() && out.errors.is_empty());
    assert_eq!(out.tokens.as_ref().map(|t| t.email.as_str()), Some(STRANGER), "only the tokens, for the window to refuse");
    // The account's own sign-in: the message goes as before.
    let out = mail_run(ada(), ada(), vec![hello()], None, OWNER);
    assert_eq!(out.sent, [9], "{:?}", out.errors);
    assert_eq!(server.state().sent.len(), 1);
}

#[test]
fn a_mail_run_with_somebody_elses_sign_in_reads_and_changes_nothing() {
    let server = stand_in();
    let _route = server.route();
    let request = || GraphMailRequest { account: 4, email: OWNER.into(), ..Default::default() };
    let out = mail_run(ada(), ada(), Vec::new(), Some(request()), STRANGER);
    assert!(server.state().requests.is_empty(), "nothing asked of the server");
    assert!(out.mail.is_none() && out.errors.is_empty());
    assert_eq!(out.tokens.as_ref().map(|t| t.email.as_str()), Some(STRANGER));
    // The account's own sign-in: its folders are read as before.
    let out = mail_run(ada(), ada(), Vec::new(), Some(request()), OWNER);
    assert!(!server.state().requests.is_empty());
    assert!(out.mail.is_some_and(|mail| mail.listing.is_some()));
}

#[test]
fn a_shared_mailboxs_mail_run_is_checked_against_its_owners_sign_in() {
    let server = stand_in();
    let _route = server.route();
    server.state().add_other("support@example.org", true);
    let shared = Account { id: 5, kind: AccountKind::Microsoft, email: "support@example.org".into(), shared_of: Some(4), ..Default::default() };
    // Mail waiting to go from it: the token is renewed for sending from it, and a stranger's
    // stops the run before the first request.
    let send = SendUp { message: 9, ..Default::default() };
    let request =
        || GraphMailRequest { account: 5, email: "support@example.org".into(), shared: true, sends: vec![send.clone()], ..Default::default() };
    let out = mail_run(shared.clone(), ada(), Vec::new(), Some(request()), STRANGER);
    assert!(server.state().requests.is_empty(), "nothing asked of the server");
    assert!(server.state().sent.is_empty());
    assert!(out.mail.is_none());
    // The owner's own sign-in (which names the owner, not the shared mailbox): the run goes.
    let out = mail_run(shared, ada(), Vec::new(), Some(request()), OWNER);
    assert!(!server.state().requests.is_empty());
    assert!(out.mail.is_some());
}

// --- A sign-in renewed part-way --------------------------------------------------------------

/// How many requests the stand-in had seen at each renewal.
type Renewals = std::sync::Mutex<Vec<usize>>;

#[test]
fn a_calendar_or_to_do_job_renews_its_sign_in_only_before_its_first_request() {
    // A calendar or To Do job's answer that is refused for its sign-in loses nothing done with the
    // account's own token: its jobs renew the sign-in only before the first request, even when
    // the server stops taking the token part-way, so a refused answer has nothing in it to keep.
    let server = stand_in();
    let _route = server.route();
    let seen = Renewals::default();
    let renewing = |who: &'static str| {
        seen.lock().unwrap().push(server.state().requests.len());
        signed_in_as(who)
    };
    // The calendar, with a meeting to send, a calendar opened by address and the token refused
    // after the meeting went up.
    let (mut s, a) = session();
    let plan = plan_with_meeting(&mut s, a);
    let calendar_before = server.state().requests.len();
    let mut request = calendar_request(&s, &plan, auth()).unwrap();
    request.account.opened_calendars =
        vec![OpenedCalendar { address: "lee@example.org".into(), name: "Lee".into(), ticked: true, ..Default::default() }];
    server.state().fail_next.push(("GET".into(), 401, "InvalidAuthenticationToken".into()));
    let result = graph_in_with(request, "UTC", &|_, _| renewing(OWNER));
    assert!(result.uploaded.is_some(), "the meeting went up");
    // The shared mailbox's calendar, whose meeting takes the sign-in for sending from it.
    let shared = s.run("account.addSharedMailbox", &json!({"address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    let plan = plan_with_meeting(&mut s, shared);
    let request = calendar_request(&s, &plan, auth()).unwrap();
    let shared_before = server.state().requests.len();
    server.state().fail_next.push(("GET".into(), 401, "InvalidAuthenticationToken".into()));
    let _ = graph_sending_with(request, "UTC", &|_, _, _| renewing(OWNER), &|_, _| renewing(OWNER));
    assert!(server.state().requests.len() > shared_before, "the shared mailbox's job asked the server");
    // To Do, its token refused at the first read.
    let before = server.state().requests.len();
    server.state().fail_next.push(("GET".into(), 401, "InvalidAuthenticationToken".into()));
    let out = run_with(tasks_job(), &|_, _| renewing(OWNER));
    assert!(out.uploaded.is_some() && out.sign_in_refused);
    let seen = seen.lock().unwrap().clone();
    let total = server.state().requests.len();
    assert!(total > before);
    // Each renewal came when nothing of its job had been asked yet: none part-way.
    // (Each job's count is taken before it starts, by itself.)
    assert_eq!(calendar_before, 0);
    let firsts = [calendar_before, shared_before, before];
    assert_eq!(seen.len(), 3, "one renewal a job: {seen:?}");
    assert_eq!(seen, firsts, "every renewal before the job's first request");
}

#[test]
fn a_mail_run_whose_sign_in_turns_out_somebody_elses_part_way_keeps_what_went_with_its_own() {
    // Mail's run renews part-way when the server stops taking the token (#306). The renewed one
    // is somebody else's: the run stops there, nothing is asked with it, and what went before it
    // with the account's own token (the Outbox message sent) is in the answer, to be taken in.
    let server = stand_in();
    let _route = server.route();
    let seen = Renewals::default();
    let renewals = std::sync::atomic::AtomicUsize::new(0);
    let renew = |_: &OAuth, _: &str, _: bool| {
        seen.lock().unwrap().push(server.state().requests.len());
        let first = renewals.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0;
        signed_in_as(if first { OWNER } else { STRANGER })
    };
    let request = GraphRequest {
        account: ada(),
        oauth: OAuth { client_id: "app".into(), ..Default::default() },
        refresh: "refresh".into(),
        access: String::new(),
        outbox: vec![hello()],
        directory: false,
        calendar: None,
        changes: None,
        asks: Vec::new(),
        settings: false,
        favorites: false,
        favorites_scopes: false,
        mail: Some(GraphMailRequest { account: 4, email: OWNER.into(), ..Default::default() }),
        mail_pieces: None,
        cancel: Default::default(),
        signer: Some(ada()),
        contacts: None,
    };
    // The message goes; part-way through the walk, the Inbox's read is told the token is no
    // longer good.
    let inbox = server.state().well_known("inbox");
    server.state().fail_next.push((format!("GET /v1.0/me/mailFolders/{inbox}/messages/delta"), 401, "InvalidAuthenticationToken".into()));
    let out = graph_sending_with(request, "UTC", &renew, &|o, r| renew(o, r, false));
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "renewed at the start and once part-way");
    assert_eq!(seen.first(), Some(&0));
    assert_eq!(server.state().requests.len(), seen[1], "nothing asked after the stranger's renewal");
    assert_eq!(out.sent, [9], "what went with the account's own token is kept");
    assert!(out.mail.is_some(), "and what was read with it");
    assert_eq!(server.state().sent.len(), 1);
    assert_eq!(out.tokens.as_ref().map(|t| t.email.as_str()), Some(STRANGER), "the stranger's tokens, to be refused");
}
