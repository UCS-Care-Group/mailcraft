//! Notifications through a stand-in for the host: what they say, and when they aren't sent.

use std::sync::{Arc, Mutex, mpsc};

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_engine::cmd::view::Reminder;
use mailcraft_model::sync::{FolderDelta, SyncRequest, SyncResult};
use mailcraft_model::{Address, Body, FolderKind, Message};
use serde_json::json;

use super::{Notice, NoticeKind};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Shown = Arc<Mutex<Vec<Notice>>>;
type Pipe = Arc<Mutex<Option<mpsc::Sender<SyncResult>>>>;

/// An app whose host records the notices it is asked to show, and whose Send/Receive pieces
/// the test hands over itself.
fn rig() -> (MailApp, Shown, Pipe, u64) {
    let shown: Shown = Default::default();
    let pipe: Pipe = Default::default();
    let mut services = Services::default();
    let into = shown.clone();
    services.notify = Some(Box::new(move |n| into.lock().unwrap().push(n)));
    let pipe2 = pipe.clone();
    services.sync = Some(Box::new(move |_req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        *pipe2.lock().unwrap() = Some(tx);
        rx
    }));
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    app.autosave = false;
    let id = app.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].clone();
    (app, shown, pipe, id.as_u64().unwrap())
}

fn mail(uid: u32, from: &str, subject: &str, received: i64) -> Message {
    Message {
        uid,
        subject: subject.into(),
        from: Address::new(from, "sender@example.org"),
        date: received,
        received,
        body: Body { text: "The body is private: never in a notification.".into(), html: None },
        ..Default::default()
    }
}

/// One Send/Receive that brings `inbox` to the Inbox and `junk` to Junk E-mail, in two pieces.
fn run(app: &mut MailApp, pipe: &Pipe, account: u64, inbox: Vec<Message>, junk: Vec<Message>) {
    let folder = |kind| app.session.mailbox.folder_of_kind(account, kind).unwrap();
    let (inbox_id, junk_id) = (folder(FolderKind::Inbox), folder(FolderKind::Junk));
    let delta = |id, remote: &str, kind, new: Vec<Message>| FolderDelta {
        id: Some(id),
        remote: remote.into(),
        kind,
        uid_validity: 7,
        uid_seen: 100,
        new,
        ..Default::default()
    };
    app.execute("ui.sync", json!({"account": account, "password": "pw"})).unwrap();
    let send = |p: SyncResult| pipe.lock().unwrap().as_ref().unwrap().send(p).unwrap();
    send(SyncResult { account, partial: true, folders: vec![delta(inbox_id, "INBOX", FolderKind::Inbox, inbox)], ..Default::default() });
    send(SyncResult { account, folders: vec![delta(junk_id, "Junk", FolderKind::Junk, junk)], ..Default::default() });
    app.poll_sync();
    assert!(!app.syncing());
}

#[test]
fn new_mail_is_one_notice_per_run_that_says_only_how_many() {
    let (mut app, shown, pipe, account) = rig();
    assert!(app.ui.notifications && !app.ui.notification_details, "count on, sender and subject off");
    run(
        &mut app,
        &pipe,
        account,
        vec![mail(1, "Ola Nordmann", "Older", NOW - 60), mail(2, "Kari Nordmann", "Lunch on Friday?", NOW)],
        vec![mail(3, "Spammer", "Win!", NOW + 5)],
    );
    // Junk E-mail isn't new mail to be told about.
    assert_eq!(*shown.lock().unwrap(), [Notice { kind: NoticeKind::NewMail, title: "2 new messages".into(), body: String::new() }]);

    // A run that brings nothing says nothing.
    shown.lock().unwrap().clear();
    run(&mut app, &pipe, account, Vec::new(), Vec::new());
    assert!(shown.lock().unwrap().is_empty());

    // One message, and sender and subject switched on: the newest one's, never its text.
    app.execute("ui.notificationDetails", json!({"value": true})).unwrap();
    run(&mut app, &pipe, account, vec![mail(4, "Kari Nordmann", "Agenda\r\nfor   Monday", NOW + 60)], Vec::new());
    let notices = shown.lock().unwrap().clone();
    assert_eq!(notices, [Notice { kind: NoticeKind::NewMail, title: "1 new message".into(), body: "Kari Nordmann\nAgenda for Monday".into() }]);
    assert!(!notices.iter().any(|n| n.body.contains("private") || n.title.contains("private")));

    // Of several, the newest; a nameless sender by address, a subjectless message said so.
    shown.lock().unwrap().clear();
    let mut nameless = mail(6, "", "", NOW + 300);
    nameless.from = Address::new("", "who@example.org");
    run(&mut app, &pipe, account, vec![mail(5, "Ola Nordmann", "Earlier", NOW + 120), nameless], Vec::new());
    assert_eq!(shown.lock().unwrap().first().map(|n| n.body.clone()).unwrap(), "who@example.org\n(no subject)");
}

#[test]
fn nothing_is_sent_when_switched_off_or_while_the_window_is_in_front() {
    let (mut app, shown, pipe, account) = rig();
    assert_eq!(app.execute("ui.notifications", json!({"value": false})).unwrap(), json!({"value": false, "available": true}));
    run(&mut app, &pipe, account, vec![mail(1, "Ola", "Hi", NOW)], Vec::new());
    assert!(shown.lock().unwrap().is_empty());
    // And what was counted then isn't told later.
    app.execute("ui.notifications", json!({})).unwrap();
    assert!(app.ui.notifications);
    run(&mut app, &pipe, account, Vec::new(), Vec::new());
    assert!(shown.lock().unwrap().is_empty());

    // The owner is looking at the window: it shows there.
    let ctx = egui::Context::default();
    ctx.run_ui(egui::RawInput { focused: true, ..Default::default() }, |_| {}).textures_delta.clear();
    app.ctx = Some(ctx.clone());
    run(&mut app, &pipe, account, vec![mail(2, "Ola", "Again", NOW)], Vec::new());
    assert!(shown.lock().unwrap().is_empty());
    // Elsewhere: the system says it.
    ctx.run_ui(egui::RawInput { focused: false, ..Default::default() }, |_| {}).textures_delta.clear();
    run(&mut app, &pipe, account, vec![mail(3, "Ola", "Third", NOW)], Vec::new());
    assert_eq!(shown.lock().unwrap().len(), 1);
}

#[test]
fn a_host_without_notifications_is_fine() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    assert_eq!(app.execute("ui.notifications", json!({"value": true})).unwrap()["available"], false);
    super::run_over(&mut app);
    let before = super::last_id(&app.session);
    super::took_in(&mut app, before);
    super::run_over(&mut app);
    super::reminders(&app, &[reminder("Standup", 10)]);
}

fn reminder(title: &str, minutes: i64) -> Reminder {
    let start = mailcraft_time::DateTime::from_timestamp(NOW + minutes * 60, 0);
    Reminder { event: 1, start, end: start.add_minutes(30), all_day: false, title: title.into(), location: "Room 4".into(), minutes }
}

#[test]
fn reminders_say_their_subject_and_time() {
    let (app, shown, ..) = rig();
    super::reminders(&app, &[reminder("Design review", 10)]);
    assert_eq!(
        *shown.lock().unwrap(),
        [Notice { kind: NoticeKind::Reminder, title: "Design review".into(), body: "4:10 PM (in 10 minutes)".into() }]
    );

    // Another day says the day; an all-day one says so; a long title is cut.
    shown.lock().unwrap().clear();
    let mut all_day = reminder("Holiday", 24 * 60);
    all_day.all_day = true;
    super::reminders(&app, &[reminder(&"x".repeat(500), 3 * 24 * 60), all_day]);
    let notices = shown.lock().unwrap().clone();
    assert_eq!(notices.len(), 2);
    assert!(notices.iter().all(|n| !n.body.contains("Room 4")), "the location isn't told");
    assert!(notices[0].title.chars().count() <= super::MAX_FIELD && notices[0].title.ends_with('…'));
    assert!(notices[0].body.contains("4:00 PM") && notices[0].body.contains("in 3 days"), "{}", notices[0].body);
    assert!(notices[1].body.starts_with("Thursday") && notices[1].body.contains("All day"), "{}", notices[1].body);

    // Many at once: a few by name, and one that counts the rest.
    shown.lock().unwrap().clear();
    let many: Vec<Reminder> = (0..7).map(|i| reminder(&format!("Meeting {i}"), i)).collect();
    super::reminders(&app, &many);
    let titles: Vec<String> = shown.lock().unwrap().iter().map(|n| n.title.clone()).collect();
    assert_eq!(titles, ["Meeting 0", "Meeting 1", "Meeting 2", "4 more reminders"]);
}

#[test]
fn the_switches_are_kept_with_the_window_settings() {
    let (mut app, ..) = rig();
    app.execute("ui.notificationDetails", json!({})).unwrap();
    app.execute("ui.notifications", json!({"value": false})).unwrap();
    let saved = serde_json::to_string(&app.prefs()).unwrap();
    assert!(saved.contains(r#""notifications":false"#) && saved.contains(r#""notificationDetails":true"#), "{saved}");
    // Settings from before there were notifications: on for the count, off for the rest.
    let old: crate::UiState = serde_json::from_str(r#"{"tab": "Home"}"#).unwrap();
    assert!(old.notifications && !old.notification_details);
}

#[test]
fn file_options_says_what_this_computer_cant_do() {
    let window = |app: MailApp| {
        let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 4000.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
        harness.run_steps(3);
        harness
    };
    // No credential store and no notifications (a Linux desktop without a Secret Service, say).
    let mut bare = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    bare.autosave = false;
    let harness = window(bare);
    assert!(harness.query_by_label_contains("Remembering isn't available here").is_some());
    assert!(harness.query_by_label_contains("can't show notifications here").is_some());
    // Both there: neither is said.
    let (mut app, ..) = rig();
    app.services.secrets = Some(Box::new(Nothing));
    let harness = window(app);
    assert!(harness.query_by_label_contains("Remembering isn't available here").is_none());
    assert!(harness.query_by_label_contains("can't show notifications here").is_none());
    assert!(harness.query_by_label_contains("Show a notification for new mail and reminders").is_some());
}

/// A credential store that keeps nothing.
struct Nothing;

impl crate::SecretStore for Nothing {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
    fn set(&self, _key: &str, _secret: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _key: &str) {}
}
