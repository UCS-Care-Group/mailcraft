// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Subscribed calendars in the window (#209): the address is kept like a password and nowhere
//! else, a calendar is looked at when it is due and not before, a look that fails keeps what was
//! there, and Unsubscribe forgets the address. The host is a fake; every address is invented.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::subscription::SubscriptionOutcome;
use serde_json::json;

use crate::{MailApp, SecretStore, Services, SubscriptionDone, SubscriptionJob};

const NOW: i64 = 1_791_993_600;
const SECRET: &str = "T0KEN-s3cret-9f2a";
const ADDRESS: &str = "webcal://cal.example.org/private/T0KEN-s3cret-9f2a/basic.ics";
const FILE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a@example.org\r\nSUMMARY:Home match\r\nDTSTART:20261114T150000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[derive(Clone, Default)]
struct Store {
    kept: Arc<Mutex<HashMap<String, String>>>,
    /// A store that takes a delete and does nothing (a locked keyring).
    stuck: Arc<Mutex<bool>>,
}

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.kept.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), mailcraft_host::SecretError> {
        self.kept.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        if !*self.stuck.lock().unwrap() {
            self.kept.lock().unwrap().remove(key);
        }
    }
}

/// What the fake host was asked: calendar, address, etag.
type Asked = Arc<Mutex<Vec<(Id, String, String)>>>;
type Answers = Arc<Mutex<VecDeque<SubscriptionOutcome>>>;

fn window(store: Option<&Store>, remember: bool) -> (MailApp, Asked, Answers) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.ui.remember_sign_in = remember;
    app.services.secrets = store.map(|s| Arc::new(s.clone()) as Arc<dyn SecretStore>);
    let (asked, answers): (Asked, Answers) = Default::default();
    let (log, queue) = (asked.clone(), answers.clone());
    app.services.subscription = Some(Box::new(move |job: SubscriptionJob| {
        log.lock().unwrap().push((job.calendar, job.address.clone(), job.etag.clone()));
        let outcome = queue.lock().unwrap().pop_front().unwrap_or(SubscriptionOutcome::NotModified);
        let (tx, rx) = mpsc::channel();
        tx.send(SubscriptionDone { calendar: job.calendar, outcome }).unwrap();
        rx
    }));
    (app, asked, answers)
}

fn changed(etag: &str) -> SubscriptionOutcome {
    SubscriptionOutcome::Changed { text: FILE.into(), etag: etag.into(), last_modified: String::new() }
}

/// Two frames: the look is started, then taken in.
fn looks(app: &mut MailApp) {
    app.subscribed_tick();
    app.subscribed_tick();
}

fn subscribe(app: &mut MailApp) -> Id {
    app.execute("calendar.subscribe", json!({"address": ADDRESS})).unwrap()["id"].as_u64().unwrap()
}

fn held(app: &MailApp, calendar: Id) -> usize {
    app.session.mailbox.events.values().filter(|e| e.calendar == calendar).count()
}

#[test]
fn subscribing_reads_the_file_at_once_with_the_address_the_owner_gave() {
    let store = Store::default();
    let (mut app, asked, answers) = window(Some(&store), true);
    answers.lock().unwrap().push_back(changed("\"v1\""));
    let id = subscribe(&mut app);
    // Nothing was asked of the host by subscribing itself.
    assert!(asked.lock().unwrap().is_empty());
    looks(&mut app);
    assert_eq!(asked.lock().unwrap().as_slice(), [(id, ADDRESS.to_string(), String::new())]);
    assert_eq!(held(&app, id), 1);
    assert_eq!(app.session.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().etag, "\"v1\"");
}

#[test]
fn the_address_goes_to_the_credential_store_and_to_no_file_of_ours() {
    let store = Store::default();
    let (mut app, _, answers) = window(Some(&store), true);
    answers.lock().unwrap().push_back(changed(""));
    let id = subscribe(&mut app);
    looks(&mut app);
    let name = app.session.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().secret_name(id);
    assert_eq!(store.get(&name).as_deref(), Some(ADDRESS));
    assert!(name.contains("cal.example.org") && !name.contains(SECRET));
    // Not in the mailbox, nor the settings (which name the entry, never its contents).
    assert!(!serde_json::to_string(&app.session.mailbox).unwrap().contains(SECRET));
    let settings = serde_json::to_string(&app.ui).unwrap();
    assert!(!settings.contains(SECRET) && settings.contains(&name), "{settings}");
    // Nor in what the job says of itself.
    let job = SubscriptionJob { calendar: id, address: ADDRESS.into(), etag: String::new(), last_modified: String::new() };
    assert!(!format!("{job:?}").contains(SECRET));
}

#[test]
fn without_remembering_the_address_lives_in_memory_and_the_store_is_left_alone() {
    let store = Store::default();
    let (mut app, asked, answers) = window(Some(&store), false);
    answers.lock().unwrap().push_back(changed(""));
    let id = subscribe(&mut app);
    looks(&mut app);
    assert!(store.kept.lock().unwrap().is_empty());
    assert!(!app.subscription_needs_address(id));
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[test]
fn after_a_restart_without_the_address_the_calendar_stays_and_asks_for_it_again() {
    let (mut app, _, answers) = window(None, false);
    answers.lock().unwrap().push_back(changed(""));
    let id = subscribe(&mut app);
    looks(&mut app);
    // The mailbox as it is saved and opened again, in a window that has no address.
    let saved = serde_json::to_string(&app.session.mailbox).unwrap();
    let mailbox: mailcraft_model::Mailbox = serde_json::from_str(&saved).unwrap();
    let (mut again, asked, _) = window(None, false);
    again.session = Session::at(mailbox, NOW + 7_200, 0);
    assert!(again.subscription_needs_address(id));
    looks(&mut again);
    assert!(asked.lock().unwrap().is_empty(), "nothing is looked at without its address");
    assert_eq!(held(&again, id), 1, "what it held is shown");
    // The address typed again, at the same host, brings the look.
    let wrong = again.execute("calendar.subscriptionSettings", json!({"id": id, "address": "https://evil.example.net/x.ics"}));
    assert!(wrong.is_err() && again.subscription_needs_address(id));
    again.execute("calendar.subscriptionSettings", json!({"id": id, "address": ADDRESS})).unwrap();
    assert!(!again.subscription_needs_address(id));
    looks(&mut again);
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[test]
fn a_remembered_address_is_found_again_in_the_store_after_a_restart() {
    let store = Store::default();
    let (mut app, _, answers) = window(Some(&store), true);
    answers.lock().unwrap().push_back(changed(""));
    let id = subscribe(&mut app);
    looks(&mut app);
    let saved = serde_json::to_string(&app.session.mailbox).unwrap();
    let mailbox: mailcraft_model::Mailbox = serde_json::from_str(&saved).unwrap();
    let (mut again, asked, _) = window(Some(&store), true);
    again.session = Session::at(mailbox, NOW + 7_200, 0);
    assert!(again.subscription_needs_address(id), "until the store has been read");
    // The background read of the store (what `secret_read` brings) is stood in for.
    let name = again.session.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().secret_name(id);
    again.secret_cache.insert(name, store.get(&again.session.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().secret_name(id)).unwrap());
    assert!(!again.subscription_needs_address(id));
    looks(&mut again);
    assert_eq!(asked.lock().unwrap().as_slice(), [(id, ADDRESS.to_string(), String::new())]);
}

#[test]
fn a_calendar_is_looked_at_when_due_and_the_next_look_asks_with_the_marks() {
    let (mut app, asked, answers) = window(None, false);
    answers.lock().unwrap().push_back(changed("\"v1\""));
    let id = subscribe(&mut app);
    looks(&mut app);
    // Not due again: nothing more is asked, however often the window looks.
    for _ in 0..3 {
        app.subscribed.reset_clock();
        looks(&mut app);
    }
    assert_eq!(asked.lock().unwrap().len(), 1);
    // An hour on it is, and the file's marks go with the question.
    app.session.clock.now = Some(NOW + 3_600);
    app.subscribed.reset_clock();
    looks(&mut app);
    assert_eq!(asked.lock().unwrap().len(), 2);
    assert_eq!(asked.lock().unwrap()[1], (id, ADDRESS.to_string(), "\"v1\"".to_string()));
}

#[test]
fn send_receive_and_update_now_look_ahead_of_time() {
    let (mut app, asked, answers) = window(None, false);
    answers.lock().unwrap().push_back(changed(""));
    let id = subscribe(&mut app);
    looks(&mut app);
    assert_eq!(asked.lock().unwrap().len(), 1);
    // The owner's Send/Receive (it can't connect anywhere here, and says so).
    let _ = app.execute("ui.sync", json!({}));
    looks(&mut app);
    assert_eq!(asked.lock().unwrap().len(), 2);
    // A run by itself (quiet) leaves the calendars to their own time.
    let _ = app.execute("ui.sync", json!({"quiet": true}));
    looks(&mut app);
    assert_eq!(asked.lock().unwrap().len(), 2);
    app.execute("calendar.refreshSubscribed", json!({"id": id})).unwrap();
    looks(&mut app);
    assert_eq!(asked.lock().unwrap().len(), 3);
}

#[test]
fn nothing_is_looked_at_while_working_offline() {
    let (mut app, asked, _) = window(None, false);
    subscribe(&mut app);
    app.session.prefs.offline = true;
    looks(&mut app);
    assert!(asked.lock().unwrap().is_empty());
    app.session.prefs.offline = false;
    app.subscribed.reset_clock();
    looks(&mut app);
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[test]
fn a_look_that_fails_keeps_the_events_and_the_owner_is_told_once() {
    let (mut app, _, answers) = window(None, false);
    answers.lock().unwrap().push_back(changed(""));
    let id = subscribe(&mut app);
    looks(&mut app);
    app.session.clock.now = Some(NOW + 3_600);
    answers.lock().unwrap().push_back(SubscriptionOutcome::Failed { failure: "status:404".into() });
    app.subscribed.reset_clock();
    looks(&mut app);
    assert_eq!(held(&app, id), 1);
    let said = app.status_msg.clone().map(|(text, _)| text).unwrap_or_default();
    assert!(said.contains("Couldn't update") && said.contains("no longer there") && !said.contains(SECRET), "{said}");
    // The same trouble again says nothing new.
    app.status_msg = None;
    app.session.clock.now = Some(NOW + 7_200);
    answers.lock().unwrap().push_back(SubscriptionOutcome::Failed { failure: "status:404".into() });
    app.subscribed.reset_clock();
    looks(&mut app);
    assert!(app.status_msg.is_none());
    assert!(app.session.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().failed().is_some());
}

#[test]
fn unsubscribing_forgets_the_address_everywhere() {
    let store = Store::default();
    let (mut app, asked, answers) = window(Some(&store), true);
    answers.lock().unwrap().push_back(changed(""));
    let id = subscribe(&mut app);
    looks(&mut app);
    let name = app.session.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().secret_name(id);
    assert!(store.get(&name).is_some() && app.ui.remembered_keys.contains(&name));
    app.execute("calendar.unsubscribe", json!({"id": id, "confirmed": true})).unwrap();
    assert!(app.session.mailbox.calendar(id).is_none());
    assert_eq!(held(&app, id), 0);
    assert!(store.get(&name).is_none() && !app.secret_cache.contains_key(&name) && !app.ui.remembered_keys.contains(&name));
    app.subscribed.reset_clock();
    looks(&mut app);
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[test]
fn a_refused_address_asks_nothing_and_keeps_nothing() {
    let store = Store::default();
    let (mut app, asked, _) = window(Some(&store), true);
    let error = app.begin_subscribe("http://cal.example.org/private/T0KEN-s3cret-9f2a/x.ics", "").unwrap_err();
    assert!(!error.contains(SECRET), "{error}");
    assert_eq!(MailApp::https_offer("http://cal.example.org/x.ics?t=1").as_deref(), Some("https://cal.example.org/x.ics?t=1"));
    looks(&mut app);
    assert!(asked.lock().unwrap().is_empty() && store.kept.lock().unwrap().is_empty());
    assert!(app.session.mailbox.calendars.iter().all(|c| !c.is_subscribed()));
}

#[test]
fn a_mailbox_that_takes_the_place_of_the_open_one_drops_the_addresses() {
    let (mut app, asked, _) = window(None, false);
    let id = subscribe(&mut app);
    assert!(!app.subscription_needs_address(id));
    // Another mailbox is opened: calendar ids there mean other calendars, and there is nothing to ask.
    app.execute("file.new", json!({})).unwrap();
    looks(&mut app);
    assert!(app.subscription_needs_address(id));
    assert!(app.subscribed.addresses_are_empty());
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn an_entry_the_store_would_not_delete_stays_listed_for_the_next_try() {
    let store = Store::default();
    let (mut app, _, answers) = window(Some(&store), true);
    answers.lock().unwrap().push_back(changed(""));
    let id = subscribe(&mut app);
    looks(&mut app);
    let name = app.session.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().secret_name(id);
    *store.stuck.lock().unwrap() = true;
    app.execute("calendar.unsubscribe", json!({"id": id, "confirmed": true})).unwrap();
    // The calendar is gone and its address is out of memory, but the entry is still in the store,
    // and so still named in the settings: Clear and the next start can take it out.
    assert!(app.session.mailbox.calendar(id).is_none() && !app.secret_cache.contains_key(&name));
    assert!(store.get(&name).is_some() && app.ui.remembered_keys.contains(&name));
}

#[test]
fn turning_remembering_off_keeps_the_addresses_already_read_for_the_session() {
    let store = Store::default();
    let (mut app, asked, answers) = window(Some(&store), true);
    answers.lock().unwrap().push_back(changed(""));
    let id = subscribe(&mut app);
    looks(&mut app);
    let saved = serde_json::to_string(&app.session.mailbox).unwrap();
    let mailbox: mailcraft_model::Mailbox = serde_json::from_str(&saved).unwrap();
    // A restart: the address is the store's, read in the background.
    let (mut again, asked_again, _) = window(Some(&store), true);
    again.session = Session::at(mailbox, NOW + 7_200, 0);
    let name = again.session.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().secret_name(id);
    again.secret_cache.insert(name.clone(), ADDRESS.to_string());
    again.ui.remembered_keys.push(name.clone());
    assert!(!again.subscription_needs_address(id));
    again.execute("ui.rememberSignIn", json!({"value": false})).unwrap();
    // The store has let go of it, the window has not.
    assert!(store.get(&name).is_none() && again.secret_cache.is_empty());
    assert!(!again.subscription_needs_address(id));
    looks(&mut again);
    assert_eq!(asked_again.lock().unwrap().as_slice(), [(id, ADDRESS.to_string(), String::new())]);
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[test]
fn a_subscribed_calendar_is_read_only_in_the_view_and_working_in_it_is_not_taken_up() {
    let (mut app, _, _) = window(None, false);
    let id = subscribe(&mut app);
    assert!(crate::calendar_view::read_only(&app, id));
    let own = app.session.mailbox.calendars.iter().find(|c| !c.is_subscribed()).map(|c| c.id).unwrap();
    assert!(!crate::calendar_view::read_only(&app, own));
}
