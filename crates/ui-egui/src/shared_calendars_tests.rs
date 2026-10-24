// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A shared mailbox's calendars in the window (#70): read when ticked and not before, with the
//! owner's sign-in and a token of their own, the owner's permission asked for once and by itself,
//! and nothing of them an alert. Hosts are fakes; every address is invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::{Module, Session};
use mailcraft_model::remote::{CalendarAsk, CalendarsDownload, EventsDownload, RemoteCalendar, RemoteEvent};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{CalendarOwner, Id, Rights};
use serde_json::json;

use crate::shared_calendars::Permission;
use crate::{GraphRequest, GraphResult, MailApp, Services, SignInRequest};

const OWNER: &str = "noor@example.org";
const SHARED: &str = "support@example.org";
const NOW: i64 = 1_791_993_600;

/// What each job was asked: the account, its address, the refresh token, the access token it was
/// handed, whether it was the wide one, the calendars asked for, and whether it carried changes.
type Job = (Id, String, String, String, bool, Vec<CalendarAsk>, bool);
type Jobs = Arc<Mutex<Vec<Job>>>;
type Answers = Arc<Mutex<VecDeque<GraphResult>>>;
/// What each sign-in was asked: the account, for shared mailboxes, for their calendars.
type SignIns = Arc<Mutex<Vec<(Id, bool, bool)>>>;

fn tokens(access: &str, refresh: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

/// A window with a Microsoft account signed in this session and a shared mailbox under it, the
/// calendar in front, and hosts that note what they are asked.
fn window(page: Result<Tokens, String>) -> (MailApp, Id, Id, Answers, Jobs, SignIns) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.tokens.insert(owner, tokens("own-access", "ref-1"));
    // (The owner's own calendar and contacts are not what these look at.)
    app.graph_checked.insert(owner, f64::MAX);
    app.calendar.checked.insert(owner, f64::MAX);
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    app.session.view.module = Module::Calendar;
    let (answers, jobs, sign_ins): (Answers, Jobs, SignIns) = Default::default();
    let (to_answer, log) = (answers.clone(), jobs.clone());
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        log.lock().unwrap().push((
            req.account.id,
            req.account.email.clone(),
            req.refresh.clone(),
            req.access.clone(),
            req.directory,
            req.asks.clone(),
            req.changes.is_some(),
        ));
        let (tx, rx) = mpsc::channel();
        tx.send(to_answer.lock().unwrap().pop_front().unwrap_or_default()).unwrap();
        rx
    }));
    let log = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.account, req.shared, req.shared_calendars));
        let (tx, rx) = mpsc::channel();
        tx.send(page.clone()).unwrap();
        rx
    }));
    // The window's first look at the calendar asks for the owner's own, which these don't look at.
    assert!(app.calendar_tick_at(true, crate::now_ms()));
    app.poll_calendar();
    jobs.lock().unwrap().clear();
    (app, owner, shared, answers, jobs, sign_ins)
}

fn calendar_of(app: &MailApp, account: Id, name: &str) -> mailcraft_model::Calendar {
    app.session.mailbox.calendars.iter().find(|c| c.origin.account == account && c.name == name).cloned().unwrap_or_else(|| panic!("no {name}"))
}

fn tick(app: &mut MailApp) -> bool {
    app.calendar_tick_at(true, crate::now_ms())
}

fn listing(shared: Id) -> CalendarsDownload {
    let owner = Some(CalendarOwner { address: SHARED.into(), name: "Support".into() });
    let all = vec![
        RemoteCalendar {
            id: "sh-main".into(),
            name: "Calendar".into(),
            main: true,
            rights: Rights::Read,
            owner: owner.clone(),
            ..Default::default()
        },
        RemoteCalendar { id: "sh-rota".into(), name: "Rota".into(), rights: Rights::Read, owner, ..Default::default() },
    ];
    CalendarsDownload { account: shared, email: SHARED.into(), complete: true, calendars: all, ..Default::default() }
}

fn events(shared: Id, calendar: &str, title: &str) -> EventsDownload {
    let event = RemoteEvent {
        id: format!("e-{title}"),
        title: title.into(),
        start: NOW + 600,
        end: NOW + 4_200,
        reminder_minutes: Some(15),
        ..Default::default()
    };
    EventsDownload {
        account: shared,
        email: SHARED.into(),
        calendar: calendar.into(),
        from: NOW - 86_400,
        to: NOW + 86_400,
        complete: true,
        events: vec![event],
        ..Default::default()
    }
}

#[test]
fn nothing_of_a_shared_mailboxs_calendars_is_asked_until_one_is_ticked() {
    let (mut app, _, shared, _, jobs, sign_ins) = window(Ok(tokens("cal-access", "ref-2")));
    // It has a calendar to tick, unticked; and nothing asks any server for it.
    assert!(!calendar_of(&app, shared, SHARED).visible);
    assert!(!tick(&mut app) && jobs.lock().unwrap().is_empty() && sign_ins.lock().unwrap().is_empty());
    assert!(app.shared_calendar_accounts().is_empty());
}

#[test]
fn ticking_one_reads_the_list_and_what_is_ticked_with_the_owners_sign_in_and_nothing_more() {
    let (mut app, owner, shared, answers, jobs, _) = window(Ok(tokens("cal-access", "ref-2")));
    let calendar = calendar_of(&app, shared, SHARED).id;
    app.execute("calendar.showCalendar", json!({"id": calendar})).unwrap();
    answers.lock().unwrap().push_back(GraphResult {
        account: shared,
        tokens: Some(tokens("cal-access", "ref-9")),
        calendars: Some(listing(shared)),
        events: Some(events(shared, "", "Stand-up")),
        ..Default::default()
    });
    assert!(tick(&mut app));
    {
        let jobs = jobs.lock().unwrap();
        assert_eq!(jobs.len(), 1);
        let (account, email, refresh, access, directory, asks, changes) = jobs[0].clone();
        // The shared mailbox's own request, with the owner's refresh token and none of the mail's
        // access tokens; the wide stretch (the list), and only what is ticked.
        assert_eq!((account, email.as_str(), refresh.as_str(), access.as_str(), directory, changes), (shared, SHARED, "ref-1", "", true, false));
        assert_eq!(asks.iter().map(|ask| ask.calendar.as_str()).collect::<Vec<_>>(), [""]);
    }
    app.poll_calendar();
    // The list came in under the shared mailbox, unticked but for the one that was ticked; its
    // events are its own.
    assert!(!calendar_of(&app, shared, "Rota").visible);
    assert!(calendar_of(&app, shared, "Calendar").visible);
    assert!(app.session.mailbox.events.values().any(|e| e.title == "Stand-up" && e.origin.account == shared));
    // Its token is its own, in memory; the refresh token that came back is the owner's.
    assert_eq!(app.shared_calendar_tokens.get(&shared).map(|t| t.access.as_str()), Some("cal-access"));
    assert_eq!(app.tokens.get(&owner).map(|t| (t.access.as_str(), t.refresh.as_str())), Some(("own-access", "ref-9")));
    assert!(!app.graph_tokens.contains_key(&shared), "not mixed with the mail's");
    assert_eq!(app.shared_calendar_perm.get(&shared), Some(&Permission::Given));
    // Nothing of it is a reminder; and a change to it is refused, so nothing waits to go.
    assert!(mailcraft_engine::cmd::view::due_reminders(&app.session, app.session.local_now()).is_empty());
    let event = app.session.mailbox.events.values().find(|e| e.title == "Stand-up").map(|e| e.id).unwrap();
    assert!(app.execute("calendar.edit", json!({"id": event, "title": "Changed"})).is_err());
    assert!(app.calendar_changes(shared).is_none());

    // Tick the other one: it is asked for with the next look, with the main one, and not before.
    let rota = calendar_of(&app, shared, "Rota").id;
    app.execute("calendar.showCalendar", json!({"id": rota})).unwrap();
    app.calendar.checked.remove(&shared);
    app.graph_checked.insert(shared, f64::MAX);
    answers.lock().unwrap().push_back(GraphResult { account: shared, more_events: vec![events(shared, "sh-rota", "Rota")], ..Default::default() });
    assert!(tick(&mut app));
    let asked: Vec<String> = jobs.lock().unwrap()[1].5.iter().map(|ask| ask.calendar.clone()).collect();
    assert_eq!(asked, ["", "sh-rota"]);
    app.poll_calendar();
    assert!(app.session.mailbox.events.values().any(|e| e.title == "Rota" && e.origin.account == shared));
    // (A job that brought only another calendar is no failure.)
    assert!(!app.calendar.failures.contains_key(&shared));
}

#[test]
fn the_owner_is_asked_once_by_itself_when_the_sign_in_does_not_cover_shared_calendars() {
    let (mut app, owner, shared, answers, jobs, sign_ins) = window(Ok(tokens("cal-access", "ref-2")));
    let calendar = calendar_of(&app, shared, SHARED).id;
    app.execute("calendar.showCalendar", json!({"id": calendar})).unwrap();
    answers.lock().unwrap().push_back(GraphResult { account: shared, consent_needed: true, ..Default::default() });
    assert!(tick(&mut app));
    app.poll_calendar();
    // Found missing: nothing is asked of any server again until the owner answers; the window asks.
    assert_eq!(app.shared_calendar_perm.get(&shared), Some(&Permission::Needed));
    assert!(!tick(&mut app) || jobs.lock().unwrap().len() == 1);
    let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = &app.dialog else { panic!("the owner is asked first") };
    assert_eq!((command.as_str(), params["account"].as_u64()), ("ui.allowSharedCalendars", Some(shared)));
    assert!(ask.text.contains(SHARED) && ask.text.contains("Microsoft's page opens"), "{}", ask.text);
    assert!(sign_ins.lock().unwrap().is_empty(), "no page yet");
    assert_eq!(jobs.lock().unwrap().len(), 1);
    // Not meant without a yes.
    assert!(app.run("ui.allowSharedCalendars", json!({"account": shared})).is_err());
    // (An account that isn't there, or doesn't sign in with Microsoft, has no calendars to allow.)
    assert!(app.run("ui.allowSharedCalendars", json!({"account": 9_999, "confirmed": true})).is_err());
    // Go ahead: Microsoft's page, for shared calendars, by itself, for the owner.
    app.dialog = None;
    assert_eq!(app.run("ui.allowSharedCalendars", json!({"account": shared, "confirmed": true})).unwrap()["account"], owner);
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, true, true)]);
    app.poll_sign_in();
    // Allowed: read at once, with what came back, and never asked again.
    assert_eq!(app.shared_calendar_tokens.get(&shared).map(|t| t.access.as_str()), Some("cal-access"));
    assert!(!app.shared_calendar_perm.contains_key(&shared));
    answers.lock().unwrap().push_back(GraphResult { account: shared, events: Some(events(shared, "", "Stand-up")), ..Default::default() });
    assert!(tick(&mut app));
    assert_eq!(jobs.lock().unwrap()[1].3, "cal-access", "the token that covers it goes along");
    app.poll_calendar();
    assert_eq!(app.shared_calendar_perm.get(&shared), Some(&Permission::Given));
    assert_eq!(sign_ins.lock().unwrap().len(), 1);
    assert!(app.dialog.is_none());
}

#[test]
fn a_no_is_said_and_an_administrators_approval_has_its_page_and_ask_again_asks_again() {
    use crate::favorites_sync::NEEDS_APPROVAL;
    let (mut app, _, shared, answers, jobs, _) = window(Err(NEEDS_APPROVAL.into()));
    let calendar = calendar_of(&app, shared, SHARED).id;
    app.execute("calendar.showCalendar", json!({"id": calendar})).unwrap();
    answers.lock().unwrap().push_back(GraphResult { account: shared, consent_needed: true, ..Default::default() });
    assert!(tick(&mut app));
    app.poll_calendar();
    tick(&mut app);
    app.dialog = None;
    app.run("ui.allowSharedCalendars", json!({"account": shared, "confirmed": true})).unwrap();
    app.poll_sign_in();
    assert!(matches!(app.shared_calendar_perm.get(&shared), Some(Permission::Refused(why)) if why.contains("administrator has to approve")));
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("account"), "the page that says what to do next");
    // Nothing is asked of the server meanwhile, and File › Info says so, with a way to ask again.
    app.dialog = None;
    assert!(!tick(&mut app));
    assert_eq!(jobs.lock().unwrap().len(), 1);
    let (line, again) = app.shared_calendar_line(shared).unwrap();
    assert!(line.starts_with("Calendars: not allowed") && again, "{line}");
    app.ask_shared_calendars_again(shared);
    assert!(matches!(&app.dialog, Some(crate::dialogs::Dialog::Confirm { command, .. }) if command == "ui.allowSharedCalendars"));
}

#[test]
fn file_info_says_where_the_permission_stands() {
    let (mut app, _, shared, _, _, _) = window(Ok(tokens("cal-access", "ref-2")));
    let say = |app: &MailApp| app.shared_calendar_line(shared).unwrap();
    assert_eq!(say(&app), ("Calendars: not asked yet (ticking one of them asks)".to_string(), false));
    for (perm, line, again) in [
        (Permission::Given, "Calendars: allowed", false),
        (Permission::Needed, "Calendars: waiting for your permission", true),
        (Permission::Asking, "Calendars: asking for your permission…", false),
        (Permission::Refused("by the organisation".into()), "Calendars: not allowed (by the organisation)", true),
    ] {
        app.shared_calendar_perm.insert(shared, perm);
        assert_eq!(say(&app), (line.to_string(), again));
    }
    // Only a shared mailbox has one.
    let owner = app.session.mailbox.owner_of(shared).map(|o| o.id).unwrap();
    assert!(app.shared_calendar_line(owner).is_none());
}

#[test]
fn removing_the_shared_mailbox_takes_its_calendars_and_events_off_this_computer() {
    let (mut app, _, shared, answers, _, _) = window(Ok(tokens("cal-access", "ref-2")));
    let calendar = calendar_of(&app, shared, SHARED).id;
    app.execute("calendar.showCalendar", json!({"id": calendar})).unwrap();
    answers.lock().unwrap().push_back(GraphResult {
        account: shared,
        calendars: Some(listing(shared)),
        events: Some(events(shared, "", "Stand-up")),
        ..Default::default()
    });
    assert!(tick(&mut app));
    app.poll_calendar();
    assert!(app.session.mailbox.events.values().any(|e| e.origin.account == shared));
    app.execute("account.removeSharedMailbox", json!({"id": shared})).unwrap();
    assert!(app.session.mailbox.calendars.iter().all(|c| c.origin.account != shared));
    assert!(app.session.mailbox.events.values().all(|e| e.origin.account != shared));
    // And nothing more is asked for it.
    app.calendar.checked.clear();
    assert!(app.shared_calendar_accounts().is_empty());
}
