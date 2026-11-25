// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars and Contacts in File › Info (#209, D7): the settings as typed, the password held for
//! its own origin only, Stop using these servers, and Find with a stand-in for the host's search.

use mailcraft_engine::Session;
use mailcraft_model::{DavProblem, Id};
use serde_json::json;

use crate::dav_settings::{DavSettings, status};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;
const CAL: &str = "https://dav.example.org";
const CARD: &str = "https://cards.example.org";

fn app() -> (MailApp, Id) {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    a.session.dav_enabled = true;
    let id = a.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    (a, id)
}

#[test]
fn the_window_opens_only_while_the_feature_is_on_and_for_an_account_that_may_have_servers() {
    let (mut a, id) = app();
    assert!(DavSettings::open(&a, id).is_some());
    a.execute("ui.dialog", json!({"name": "davSettings", "id": id})).unwrap();
    assert!(a.dialog.is_some());
    a.session.dav_enabled = false;
    assert!(DavSettings::open(&a, id).is_none());
    a.session.dav_enabled = true;
    assert!(DavSettings::open(&a, 999_999).is_none());
}

#[test]
fn saving_keeps_what_was_typed_and_sends_nothing() {
    let (mut a, id) = app();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.calendars = format!(" {CAL}/calendars/ada/ ");
    d.contacts = format!("{CAL}/addressbooks/ada/");
    d.username = "ada-dav".into();
    assert!(d.save(&mut a));
    let server = a.session.mailbox.account(id).unwrap().dav.clone().unwrap();
    assert_eq!(server.calendar_homes, vec![format!("{CAL}/calendars/ada/")]);
    assert_eq!(server.contact_homes, vec![format!("{CAL}/addressbooks/ada/")]);
    assert_eq!(server.username, "ada-dav");
    assert!(!server.separate_password);
    assert!(a.dav_passwords.is_empty(), "the mail password is looked up at each job, never held");
    // Opened again, the window shows what is kept.
    let d = DavSettings::open(&a, id).unwrap();
    assert_eq!(d.calendars, format!("{CAL}/calendars/ada/"));
    assert_eq!(d.username, "ada-dav");
}

#[test]
fn an_address_the_command_refuses_is_said_and_nothing_is_kept() {
    let (mut a, id) = app();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.calendars = "http://dav.example.org/calendars/ada/".into();
    assert!(!d.save(&mut a));
    assert!(!d.error.is_empty());
    assert!(a.session.mailbox.account(id).unwrap().dav.is_none());
}

#[test]
fn a_password_of_their_own_is_held_for_the_origin_the_field_names_and_no_other() {
    let (mut a, id) = app();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.calendars = format!("{CAL}/calendars/ada/");
    d.contacts = format!("{CARD}/addressbooks/ada/");
    d.separate = true;
    d.password = "own".into();
    assert_eq!(d.password_origin().as_deref(), Some(CAL));
    assert!(d.save(&mut a));
    assert_eq!(a.dav_password(id, CAL).as_deref(), Some("own"));
    assert_eq!(a.dav_password(id, CARD), None, "never lent to another origin");
    assert_eq!(a.dav_origins_wanting_password(id), vec![CARD.to_string()], "File › Info asks for the contacts' own");
}

#[test]
fn one_server_for_calendars_and_contacts_takes_one_password() {
    let (mut a, id) = app();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.calendars = format!("{CAL}/calendars/ada/");
    d.contacts = format!("{CAL}/addressbooks/ada/");
    d.separate = true;
    d.password = "own".into();
    assert!(d.save(&mut a));
    assert!(a.dav_origins_wanting_password(id).is_empty());
    // Only contacts: the field is theirs.
    let (mut a, id) = app();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.contacts = format!("{CARD}/addressbooks/ada/");
    d.separate = true;
    d.password = "own".into();
    assert_eq!(d.password_origin().as_deref(), Some(CARD));
    assert!(d.save(&mut a));
    assert_eq!(a.dav_password(id, CARD).as_deref(), Some("own"));
}

#[test]
fn a_password_typed_while_the_box_is_off_is_not_held() {
    let (mut a, id) = app();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.calendars = format!("{CAL}/calendars/ada/");
    d.password = "typed, then the box was cleared".into();
    assert!(d.save(&mut a));
    assert!(a.dav_passwords.is_empty());
}

#[test]
fn stop_using_forgets_the_servers() {
    let (mut a, id) = app();
    a.execute("account.davSet", json!({"id": id, "calendarHomes": [format!("{CAL}/calendars/ada/")]})).unwrap();
    a.execute("account.davOff", json!({"id": id})).unwrap();
    assert!(a.session.mailbox.account(id).unwrap().dav.is_none());
    assert!(DavSettings::open(&a, id).unwrap().calendars.is_empty());
}

#[test]
fn the_status_says_nothing_before_a_read_and_names_the_trouble_after_one() {
    let (mut a, id) = app();
    assert_eq!(status(&a, a.session.mailbox.account(id).unwrap()), None, "not set up");
    a.execute("account.davSet", json!({"id": id, "calendarHomes": [format!("{CAL}/calendars/ada/")]})).unwrap();
    assert_eq!(status(&a, a.session.mailbox.account(id).unwrap()), None, "nothing read yet");
    a.dav.shown.insert(id, DavProblem::WrongPassword);
    assert_eq!(status(&a, a.session.mailbox.account(id).unwrap()).as_deref(), Some("The server wouldn't sign in."));
}

/// What the stand-in search was given, and its way to talk back to the window.
#[derive(Default)]
struct Searches {
    jobs: Vec<crate::dav_find::DavFindJob>,
    tell: Vec<std::sync::mpsc::Sender<crate::dav_find::DavFindEvent>>,
}

type Shared = std::sync::Arc<std::sync::Mutex<Searches>>;

/// A window whose host keeps each search for the test to answer.
fn finding() -> (MailApp, Id, Shared) {
    let (mut a, id) = app();
    let shared: Shared = Default::default();
    let kept = shared.clone();
    a.services.dav_find = Some(Box::new(move |job| {
        let (tell, told) = std::sync::mpsc::channel();
        let mut kept = kept.lock().unwrap();
        kept.jobs.push(job);
        kept.tell.push(tell);
        told
    }));
    (a, id, shared)
}

#[test]
fn find_signs_in_with_the_typed_password_only_when_the_servers_have_their_own() {
    let (mut a, id, shared) = finding();
    a.passwords.insert(id, "mail".into());
    let mut d = DavSettings::open(&a, id).unwrap();
    // The box ticked and nothing typed: the mail password is never the secret.
    d.separate = true;
    assert!(!d.find(&a));
    assert!(shared.lock().unwrap().jobs.is_empty());
    d.password = "own".into();
    assert!(d.find(&a));
    // The box off: the mail password, whatever was typed before.
    let mut d = DavSettings::open(&a, id).unwrap();
    d.password = "typed, then the box was cleared".into();
    assert!(d.find(&a));
    let searches = shared.lock().unwrap();
    let given: Vec<&str> = searches.jobs.iter().map(|j| j.password.as_str()).collect();
    assert_eq!(given, ["own", "mail"]);
    assert_eq!(searches.jobs[0].mail_host, "imap.example.org");
    assert_eq!(searches.jobs[0].email, "ada@example.org");
}

#[test]
fn nothing_is_searched_without_a_password_or_a_host_that_can() {
    let (a, id) = app();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.separate = true;
    d.password = "own".into();
    assert!(!d.find(&a), "no host service");
    let (a2, id2, shared) = finding();
    let mut d = DavSettings::open(&a2, id2).unwrap();
    assert!(!d.find(&a2), "no mail password yet");
    assert!(shared.lock().unwrap().jobs.is_empty());
}

#[test]
fn the_question_is_put_to_the_owner_and_the_answer_goes_back_once() {
    let (a, id, shared) = finding();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.separate = true;
    d.password = "own".into();
    assert!(d.find(&a));
    let tell = shared.lock().unwrap().tell[0].clone();
    tell.send(crate::dav_find::DavFindEvent::Question { host: "dav.example.net".into(), why: mailcraft_model::DavFindQuestion::Elsewhere }).unwrap();
    d.poll();
    assert_eq!(d.finding.as_ref().unwrap().question.as_ref().map(|(h, _)| h.as_str()), Some("dav.example.net"));
    d.answer(false);
    d.answer(true);
    let searches = shared.lock().unwrap();
    let answers: Vec<bool> = searches.jobs[0].answers.try_iter().collect();
    assert_eq!(answers, [false], "one answer for one question");
}

#[test]
fn what_was_found_fills_the_addresses_and_save_holds_the_password_there() {
    let (mut a, id, shared) = finding();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.separate = true;
    d.password = "own".into();
    assert!(d.find(&a));
    let found = mailcraft_model::DavFound {
        calendar_homes: vec![format!("{CAL}/calendars/ada/")],
        contact_homes: vec![format!("{CAL}/addressbooks/ada/")],
        calendars: 2,
        address_books: 1,
        passed_over: 1,
    };
    shared.lock().unwrap().tell[0].send(crate::dav_find::DavFindEvent::Finished(Ok(found))).unwrap();
    d.poll();
    assert!(d.finding.is_none());
    assert_eq!(d.calendars, format!("{CAL}/calendars/ada/"));
    assert_eq!(d.contacts, format!("{CAL}/addressbooks/ada/"));
    assert!(a.session.mailbox.account(id).unwrap().dav.is_none(), "found is not kept until Save");
    assert!(d.save(&mut a));
    assert_eq!(a.dav_password(id, CAL).as_deref(), Some("own"));
    assert!(a.dav_origins_wanting_password(id).is_empty());
}

#[test]
fn a_failed_search_keeps_what_was_typed_and_says_why() {
    let (a, id, shared) = finding();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.separate = true;
    d.password = "own".into();
    d.calendars = format!("{CAL}/typed/");
    assert!(d.find(&a));
    shared.lock().unwrap().tell[0].send(crate::dav_find::DavFindEvent::Finished(Err(mailcraft_model::DavFindFailure::SignInRefused))).unwrap();
    d.poll();
    assert_eq!(d.found, Some(Err(mailcraft_model::DavFindFailure::SignInRefused)));
    assert_eq!(d.calendars, format!("{CAL}/typed/"));
}

#[test]
fn a_dialog_that_goes_away_stops_its_search() {
    let (a, id, shared) = finding();
    let mut d = DavSettings::open(&a, id).unwrap();
    d.separate = true;
    d.password = "own".into();
    assert!(d.find(&a));
    let cancel = shared.lock().unwrap().jobs[0].cancel.clone();
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
    drop(d);
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
}
