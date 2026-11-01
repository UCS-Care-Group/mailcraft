// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Open Shared Calendar in the window (#70): an address typed, the permission asked for once,
//! the calendar read by the account's next job, a refusal that adds nothing, and the copy that
//! is offered for removal once. Hosts are fakes; every address is invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::remote::{CalendarsDownload, OpenedRefusal, OpenedWhy, RemoteCalendar, opened_id};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{CalendarOwner, Id, Rights};
use serde_json::json;

use crate::{GraphRequest, GraphResult, MailApp, Services, SignInRequest};

const OWNER: &str = "noor@example.org";
const CY: &str = "cy@example.org";

type Answers = Arc<Mutex<VecDeque<GraphResult>>>;
/// What each job was asked: whether it was the wide one, and how many calendars were opened by address.
type Jobs = Arc<Mutex<Vec<(bool, usize)>>>;
/// What each sign-in was asked: for Graph, for other people's calendars.
type SignIns = Arc<Mutex<Vec<(bool, bool)>>>;

fn tokens(access: &str, refresh: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

fn window(answer: Result<Tokens, String>) -> (MailApp, Id, Answers, Jobs, SignIns) {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft", "tenant": "organizations"})).unwrap()["id"].as_u64().unwrap();
    app.tokens.insert(owner, tokens("own-access", "ref-1"));
    app.graph_checked.insert(owner, f64::MAX);
    let (answers, jobs, sign_ins): (Answers, Jobs, SignIns) = Default::default();
    let (to_answer, log) = (answers.clone(), jobs.clone());
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        log.lock().unwrap().push((req.directory, req.account.opened_calendars.len()));
        let (tx, rx) = mpsc::channel();
        let mut answer = to_answer.lock().unwrap().pop_front().unwrap_or_default();
        answer.account = req.account.id;
        tx.send(answer).unwrap();
        rx
    }));
    let asked = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        asked.lock().unwrap().push((req.graph, req.shared_calendars));
        let (tx, rx) = mpsc::channel();
        tx.send(answer.clone()).unwrap();
        rx
    }));
    (app, owner, answers, jobs, sign_ins)
}

fn main_calendar() -> RemoteCalendar {
    RemoteCalendar { id: "c-main".into(), name: "Calendar".into(), main: true, rights: Rights::Edit, ..Default::default() }
}

fn cy_calendar() -> RemoteCalendar {
    RemoteCalendar {
        id: opened_id(CY),
        name: "Cy Colleague".into(),
        rights: Rights::Read,
        owner: Some(CalendarOwner { address: CY.into(), name: "Cy Colleague".into() }),
        ..Default::default()
    }
}

fn listed(owner: Id, calendars: Vec<RemoteCalendar>, refused: Vec<OpenedRefusal>) -> GraphResult {
    let download = CalendarsDownload { account: owner, email: OWNER.into(), complete: true, calendars, refused };
    GraphResult { account: owner, calendars: Some(download), ..Default::default() }
}

/// Start the account's next calendar job and take in its answer.
fn job(app: &mut MailApp) {
    // (A job that brought no events counts as a failure, which waits: not what these look at.)
    app.calendar.failures.clear();
    assert!(app.calendar_tick_at(true, 1.0e9), "a calendar job was started");
    app.poll_calendar();
}

#[test]
fn an_address_is_read_by_the_next_job_at_once_and_its_calendar_is_listed_under_shared_calendars() {
    let (mut app, owner, answers, jobs, _) = window(Ok(tokens("x", "y")));
    answers.lock().unwrap().push_back(listed(owner, vec![main_calendar(), cy_calendar()], vec![]));
    app.begin_open_shared(owner, " Cy@example.org ").unwrap();
    assert_eq!(app.session.mailbox.account(owner).unwrap().opened_calendars[0].address, "Cy@example.org");
    // Not waited for: the wide stretch is asked for now, with the address on the account it was handed.
    job(&mut app);
    assert_eq!(*jobs.lock().unwrap(), [(true, 1)]);
    let calendar = app.session.mailbox.calendars.iter().find(|c| c.name == "Cy Colleague").cloned().unwrap();
    assert_eq!((calendar.rights, calendar.visible), (Rights::Read, true));
    assert!(matches!(app.open_shared.outcome, Some(Ok(ref name)) if name == "Cy Colleague"), "{:?}", app.open_shared.outcome);
    assert!(app.open_shared.waiting.is_none());
    assert!(
        app.session.sync_status.contains("Cy Colleague")
            || app.status_msg.as_ref().is_some_and(|(said, _)| said.contains("Opened the calendar of Cy Colleague"))
    );
}

#[test]
fn a_refusal_says_why_and_adds_nothing() {
    for (why, words) in [(OpenedWhy::NoRights, "don't have access"), (OpenedWhy::NoSuchPerson, "no mailbox at")] {
        let (mut app, owner, answers, _, _) = window(Ok(tokens("x", "y")));
        answers.lock().unwrap().push_back(listed(owner, vec![main_calendar()], vec![OpenedRefusal { address: CY.into(), why }]));
        app.begin_open_shared(owner, CY).unwrap();
        job(&mut app);
        let Some(Err(said)) = app.open_shared.outcome.clone() else { panic!("the dialog says why") };
        assert!(said.contains(words) && said.contains(CY), "{said}");
        assert!(app.session.mailbox.account(owner).unwrap().opened_calendars.is_empty());
        assert!(!app.session.mailbox.calendars.iter().any(|c| c.name == "Cy Colleague"));
    }
}

#[test]
fn the_owner_is_asked_first_then_microsofts_page_by_itself_and_a_yes_reads_it_at_once() {
    let (mut app, owner, answers, jobs, sign_ins) = window(Ok(Tokens { email: OWNER.into(), ..tokens("cal-access", "ref-9") }));
    answers.lock().unwrap().push_back(GraphResult { shared_calendars_consent_needed: true, ..Default::default() });
    answers.lock().unwrap().push_back(listed(owner, vec![main_calendar(), cy_calendar()], vec![]));
    app.begin_open_shared(owner, CY).unwrap();
    job(&mut app);
    // The window asks; nothing was asked of Microsoft yet.
    let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = &app.dialog else { panic!("the owner is asked first") };
    assert_eq!((command.as_str(), params["account"].as_u64()), ("ui.allowSharedCalendars", Some(owner)));
    assert!(ask.text.contains("this computer only"), "{}", ask.text);
    assert!(sign_ins.lock().unwrap().is_empty());
    // Go ahead: the page, for other people's calendars alone, never an ordinary sign-in.
    app.dialog = None;
    assert_eq!(app.run("ui.allowSharedCalendars", json!({"account": owner, "confirmed": true})).unwrap()["account"], owner);
    assert_eq!(*sign_ins.lock().unwrap(), [(true, true)]);
    app.poll_sign_in();
    // Allowed: read at once.
    job(&mut app);
    assert_eq!(jobs.lock().unwrap().len(), 2);
    assert!(app.session.mailbox.calendars.iter().any(|c| c.name == "Cy Colleague"));
}

#[test]
fn a_no_takes_off_what_was_never_read_and_an_approval_has_its_page() {
    use crate::favorites_sync::NEEDS_APPROVAL;
    for (answer, words) in [("access_denied".to_string(), "weren't allowed"), (NEEDS_APPROVAL.to_string(), "administrator has to approve")] {
        let (mut app, owner, answers, _, _) = window(Err(answer));
        answers.lock().unwrap().push_back(GraphResult { shared_calendars_consent_needed: true, ..Default::default() });
        app.begin_open_shared(owner, CY).unwrap();
        job(&mut app);
        app.dialog = None;
        app.run("ui.allowSharedCalendars", json!({"account": owner, "confirmed": true})).unwrap();
        app.poll_sign_in();
        let Some(Err(said)) = app.open_shared.outcome.clone() else { panic!("said") };
        assert!(said.contains(words), "{said}");
        assert!(app.session.mailbox.account(owner).unwrap().opened_calendars.is_empty(), "a no adds nothing");
        if words.contains("administrator") {
            assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("account"), "the approval page");
        }
    }
    // Only a Microsoft account's.
    let (mut app, _, _, _, _) = window(Err("no".into()));
    assert!(app.run("ui.allowSharedCalendars", json!({"confirmed": true})).is_err());
    assert!(app.run("ui.allowSharedCalendars", json!({"account": 999, "confirmed": true})).is_err());
}

#[test]
fn a_job_that_starts_by_itself_says_so_in_a_line_and_never_asks_twice() {
    let (mut app, owner, answers, _, sign_ins) = window(Ok(tokens("x", "y")));
    // (An entry that was opened earlier, whose permission is missing now.)
    app.execute("calendar.openShared", json!({"account": owner, "address": CY})).unwrap();
    app.open_shared.waiting = None;
    answers.lock().unwrap().push_back(GraphResult { shared_calendars_consent_needed: true, ..Default::default() });
    job(&mut app);
    assert!(app.dialog.is_none() && sign_ins.lock().unwrap().is_empty());
    assert!(app.status_msg.as_ref().is_some_and(|(said, _)| said.contains("need your permission")));
}

#[test]
fn a_person_the_server_lists_as_well_is_offered_for_removal_once() {
    let (mut app, owner, answers, _, _) = window(Ok(tokens("x", "y")));
    app.begin_open_shared(owner, CY).unwrap();
    answers.lock().unwrap().push_back(listed(owner, vec![main_calendar(), cy_calendar()], vec![]));
    job(&mut app);
    assert!(app.dialog.is_none(), "nothing listed twice yet");
    let server_has_it = RemoteCalendar {
        id: "AAMk-cy".into(),
        name: "Cy Colleague".into(),
        group: "Other Calendars".into(),
        rights: Rights::Read,
        owner: Some(CalendarOwner { address: CY.into(), name: "Cy Colleague".into() }),
        ..Default::default()
    };
    answers.lock().unwrap().push_back(listed(owner, vec![main_calendar(), cy_calendar(), server_has_it.clone()], vec![]));
    app.calendar.checked.clear();
    app.graph_checked.remove(&owner);
    job(&mut app);
    let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = &app.dialog else { panic!("offered") };
    assert_eq!((command.as_str(), params["address"].as_str()), ("calendar.removeShared", Some(CY)));
    assert!(ask.text.contains("Cy Colleague") && ask.text.contains("nothing on the server changes"), "{}", ask.text);
    // Said no: not asked again.
    app.dialog = None;
    answers.lock().unwrap().push_back(listed(owner, vec![main_calendar(), cy_calendar(), server_has_it], vec![]));
    app.graph_checked.remove(&owner);
    job(&mut app);
    assert!(app.dialog.is_none(), "offered once");
    // Yes: the copy opened by address goes, the server's stays.
    app.run("calendar.removeShared", json!({"address": CY, "confirmed": true})).unwrap();
    assert!(!app.session.mailbox.calendars.iter().any(|c| c.origin.id.starts_with("user:")));
    assert!(app.session.mailbox.calendars.iter().any(|c| c.origin.id == "AAMk-cy"));
}

#[test]
fn an_address_that_isnt_one_is_refused_before_anything_is_kept_or_asked() {
    let (mut app, owner, _, jobs, sign_ins) = window(Ok(tokens("x", "y")));
    for bad in ["", "nobody", "a/../b@example.org", "a b@example.org", "a@example.org\r\nBcc: z@example.org", "é@example.org"] {
        assert!(app.begin_open_shared(owner, bad).is_err(), "{bad:?}");
    }
    assert!(app.session.mailbox.account(owner).unwrap().opened_calendars.is_empty());
    assert!(app.open_shared.waiting.is_none() && jobs.lock().unwrap().is_empty() && sign_ins.lock().unwrap().is_empty());
}

#[test]
fn the_dialog_opens_for_the_ribbons_command_and_the_remove_question_names_the_address() {
    let (mut app, owner, answers, _, _) = window(Ok(tokens("x", "y")));
    assert_eq!(app.run("calendar.openShared", json!({})).unwrap()["pending"], "openCalendar");
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("openCalendar"));
    app.dialog = None;
    answers.lock().unwrap().push_back(listed(owner, vec![main_calendar(), cy_calendar()], vec![]));
    app.begin_open_shared(owner, CY).unwrap();
    job(&mut app);
    let id = app.session.mailbox.calendars.iter().find(|c| c.name == "Cy Colleague").map(|c| c.id).unwrap();
    let ask = crate::dialogs::confirmation(&app, "calendar.removeShared", &json!({"id": id})).expect("asks first");
    assert!(ask.text.contains(CY) && ask.text.contains("not touched"), "{}", ask.text);
}

const OPS: &str = "operations@example.org";

/// The copy of the shared mailbox's calendar that the server lists among the owner's own.
fn ops_copy() -> RemoteCalendar {
    RemoteCalendar {
        id: "AAMk-ops".into(),
        name: "Example Operations".into(),
        group: "Other Calendars".into(),
        rights: Rights::Edit,
        owner: Some(CalendarOwner { address: "Operations@example.org".into(), name: "Example Operations".into() }),
        ..Default::default()
    }
}

/// The account's list comes again with the next job.
fn list_again(app: &mut MailApp, owner: Id, answers: &Answers, calendars: Vec<RemoteCalendar>) {
    answers.lock().unwrap().push_back(listed(owner, calendars, vec![]));
    app.calendar.checked.clear();
    app.graph_checked.remove(&owner);
    job(app);
}

fn has_copy(app: &MailApp) -> bool {
    app.session.mailbox.calendars.iter().any(|c| c.origin.id == "AAMk-ops")
}

#[test]
fn a_shared_mailboxs_calendar_the_server_lists_too_is_offered_to_be_hidden_once() {
    let (mut app, owner, answers, jobs, _) = window(Ok(tokens("x", "y")));
    let shared = app.execute("account.addSharedMailbox", json!({"account": owner, "address": OPS})).unwrap()["id"].as_u64().unwrap();
    list_again(&mut app, owner, &answers, vec![main_calendar(), ops_copy()]);
    let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = &app.dialog else { panic!("offered") };
    assert_eq!((command.as_str(), params["address"].as_str()), ("calendar.hideListedCopy", Some(OPS)));
    assert!(ask.text.contains(OPS) && ask.text.contains("Example Operations") && ask.text.contains("Nothing on the server changes"), "{}", ask.text);
    assert_eq!((ask.with.clone(), ask.more.first().map(|(_, with)| with.clone())), (json!({"hide": true}), Some(json!({"hide": false}))));
    // The copy's menu offers to hide it; the shared mailbox's own calendar has nothing to show yet.
    let copy = app.session.mailbox.calendars.iter().find(|c| c.origin.id == "AAMk-ops").map(|c| c.id).unwrap();
    let own = app.session.mailbox.calendars.iter().find(|c| c.origin.account == shared).map(|c| c.id).unwrap();
    assert_eq!(crate::calendar_view::listed_twice(&app, copy), Some((OPS.to_string(), false)));
    assert_eq!(crate::calendar_view::listed_twice(&app, own), None);
    // Closed without an answer: not asked again while the window is open.
    app.dialog = None;
    list_again(&mut app, owner, &answers, vec![main_calendar(), ops_copy()]);
    assert!(app.dialog.is_none());

    // Hide the Copy: it goes, and the shared mailbox's own calendar offers to show it again.
    app.run("calendar.hideListedCopy", json!({"address": OPS, "hide": true})).unwrap();
    assert!(!has_copy(&app));
    assert_eq!(crate::calendar_view::listed_twice(&app, own), Some((OPS.to_string(), true)));
    // The account's next job is handed the answer (the host reads no copy), and none comes back.
    list_again(&mut app, owner, &answers, vec![main_calendar()]);
    assert_eq!(jobs.lock().unwrap().len(), 3);
    assert!(!has_copy(&app) && app.dialog.is_none());

    // Shown again: the account's list is read at once, and the copy is back; nothing is asked.
    app.run("calendar.hideListedCopy", json!({"address": OPS, "hide": false})).unwrap();
    assert!(app.calendar.due && !app.calendar.checked.contains_key(&owner));
    answers.lock().unwrap().push_back(listed(owner, vec![main_calendar(), ops_copy()], vec![]));
    job(&mut app);
    assert!(has_copy(&app) && app.dialog.is_none());
    // A window opened afterwards doesn't ask either: the answer is in the mailbox file.
    app.open_shared.offered.clear();
    list_again(&mut app, owner, &answers, vec![main_calendar(), ops_copy()]);
    assert!(app.dialog.is_none());
}
