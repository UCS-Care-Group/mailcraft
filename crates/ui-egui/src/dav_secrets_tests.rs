// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The passwords of an account's calendar and contacts servers (#209): held per origin, and what
//! was kept for a server that changed or went is not given to the next.

use mailcraft_engine::Session;
use mailcraft_model::Id;
use serde_json::json;

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;
const A: &str = "https://dav.example.org";
const B: &str = "https://cards.example.org";

fn app() -> (MailApp, Id) {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    a.session.dav_enabled = true;
    let id = a.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    a.execute("account.davSet", json!({"id": id, "calendarHomes": [format!("{A}/cal/")], "contactHomes": [format!("{B}/c/")]})).unwrap();
    a.dav_passwords.insert((id, A.into()), "one".into());
    a.dav_passwords.insert((id, B.into()), "two".into());
    (a, id)
}

#[test]
fn a_password_is_held_for_its_origin_and_never_named_like_the_mail_password() {
    let (a, id) = app();
    let account = a.session.mailbox.account(id).unwrap();
    let (cal, card) = (MailApp::dav_secret_key(account, A).unwrap(), MailApp::dav_secret_key(account, B).unwrap());
    assert_ne!(cal, card);
    assert_ne!(cal, MailApp::secret_key("password", account));
    assert_eq!(MailApp::dav_secret_keys(account).len(), 2);
}

#[test]
fn a_home_that_goes_takes_only_its_origins_password() {
    let (mut a, id) = app();
    a.execute("account.davSet", json!({"id": id, "contactHomes": []})).unwrap();
    assert_eq!(a.dav_passwords.get(&(id, A.to_string())).map(String::as_str), Some("one"));
    assert!(!a.dav_passwords.contains_key(&(id, B.to_string())));
}

#[test]
fn another_user_name_or_certificate_gives_the_new_server_none_of_the_old_ones() {
    let (mut a, id) = app();
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    assert!(a.dav_passwords.is_empty());
    let (mut a, id) = app();
    a.execute("account.davSet", json!({"id": id, "trusted": "ab".repeat(32)})).unwrap();
    assert!(a.dav_passwords.is_empty());
    // The password flag is no other server.
    let (mut a, id) = app();
    a.execute("account.davSet", json!({"id": id, "separatePassword": true})).unwrap();
    assert_eq!(a.dav_passwords.len(), 2);
}

#[test]
fn putting_the_server_away_or_removing_the_account_forgets_them() {
    let (mut a, id) = app();
    a.execute("account.davOff", json!({"id": id})).unwrap();
    assert!(a.dav_passwords.is_empty());
    let (mut a, id) = app();
    a.dav_passwords.insert((id + 1000, A.into()), "other account".into());
    a.execute("account.remove", json!({"id": id})).unwrap();
    assert_eq!(a.dav_passwords.len(), 1, "only that account's go");
}

#[test]
fn another_mailbox_drops_them_all() {
    let (mut a, _) = app();
    a.execute("file.new", json!({})).unwrap();
    assert!(a.dav_passwords.is_empty());
}
