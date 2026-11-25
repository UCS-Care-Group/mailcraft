// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars and Contacts in File › Info (#209, D7): the settings as typed, the password held for
//! its own origin only, and Stop using these servers.

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
