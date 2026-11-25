// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Asking for the password of an account's calendar servers (#209, D7).

use mailcraft_engine::Session;
use mailcraft_model::Id;
use serde_json::json;

use crate::dav_password_ask::DavPasswordAsk;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;
const CAL: &str = "https://dav.example.org";
const CARD: &str = "https://cards.example.org";

fn app(separate: bool, contacts: &str) -> (MailApp, Id) {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    a.session.dav_enabled = true;
    let id = a.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    a.execute(
        "account.davSet",
        json!({"id": id, "calendarHomes": [format!("{CAL}/cal/ada/")], "contactHomes": [format!("{contacts}/card/ada/")], "separatePassword": separate}),
    )
    .unwrap();
    (a, id)
}

#[test]
fn servers_with_a_password_of_their_own_ask_for_it_once_for_each_origin() {
    let (a, id) = app(true, CAL);
    assert_eq!(a.dav_origins_wanting_password(id), vec![CAL.to_string()], "one server: one password");
    let (a, id) = app(true, CARD);
    let mut wanted = a.dav_origins_wanting_password(id);
    wanted.sort();
    assert_eq!(wanted, vec![CARD.to_string(), CAL.to_string()]);
}

#[test]
fn nothing_is_asked_for_the_mail_password_while_the_feature_is_off_or_once_one_is_held() {
    let (a, id) = app(false, CAL);
    assert!(a.dav_origins_wanting_password(id).is_empty(), "they take the mail password");
    let (mut a, id) = app(true, CAL);
    a.session.dav_enabled = false;
    assert!(a.dav_origins_wanting_password(id).is_empty());
    assert!(a.execute("ui.dialog", json!({"name": "davPassword", "id": id})).is_err() || a.dialog.is_none());
    a.session.dav_enabled = true;
    assert!(a.hold_dav_password(id, CAL, "own"));
    assert!(a.dav_origins_wanting_password(id).is_empty());
}

#[test]
fn what_is_typed_is_held_for_its_origin_and_tried_at_once() {
    let (mut a, id) = app(true, CARD);
    a.execute("ui.dialog", json!({"name": "davPassword", "id": id})).unwrap();
    assert!(a.dialog.is_some());
    let mut ask = DavPasswordAsk::open(&a, id).unwrap();
    for (origin, password, _) in &mut ask.fields {
        *password = if origin == CAL { "cal".into() } else { String::new() };
    }
    a.dav.next = f64::MAX;
    assert!(ask.give(&mut a));
    assert_eq!(a.dav_password(id, CAL).as_deref(), Some("cal"));
    assert_eq!(a.dav_password(id, CARD), None, "an empty field gives nothing");
    assert_eq!(a.dav.next, 0.0, "the next job may start now");
    assert_eq!(a.dav_origins_wanting_password(id), vec![CARD.to_string()]);
}

#[test]
fn a_password_that_has_to_wait_for_a_forget_keeps_what_was_typed_and_the_dialog_says_so() {
    let (mut a, id) = app(true, CARD);
    let mut ask = DavPasswordAsk::open(&a, id).unwrap();
    for (origin, password, _) in &mut ask.fields {
        *password = if origin == CAL { "cal".into() } else { "card".into() };
    }
    // The old password under the contacts' name is still being removed.
    let card = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), CARD).unwrap();
    a.dav_forget_queue.push(card);
    assert!(!ask.give(&mut a), "the dialog stays open");
    assert!(ask.waiting);
    assert_eq!(a.dav_password(id, CAL).as_deref(), Some("cal"));
    let left: Vec<(&str, &str)> = ask.fields.iter().map(|(o, p, _)| (o.as_str(), p.as_str())).collect();
    assert!(left.contains(&(CAL, "")), "a held one is emptied");
    assert!(left.contains(&(CARD, "card")), "the one that waits is kept");
    // Once the forget is done, OK holds it.
    a.dav_forget_queue.clear();
    assert!(ask.give(&mut a));
    assert_eq!(a.dav_password(id, CARD).as_deref(), Some("card"));
}
