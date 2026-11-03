// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::arrivals::Arrival;
use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use super::*;
use crate::{MailApp, Services};

/// A stand-in for the system's notifications: what it was asked to show, and clicks to hand back.
#[derive(Clone, Default)]
struct Recorder {
    shown: Arc<Mutex<Vec<DesktopAlert>>>,
    clicks: Arc<Mutex<Vec<u64>>>,
    trouble: Option<AlertTrouble>,
}

impl AlertHost for Recorder {
    fn show(&self, alert: &DesktopAlert) -> Result<(), String> {
        self.shown.lock().unwrap().push(alert.clone());
        Ok(())
    }
    fn trouble(&self) -> Option<AlertTrouble> {
        self.trouble
    }
    fn clicked(&self) -> Vec<u64> {
        std::mem::take(&mut *self.clicks.lock().unwrap())
    }
}

fn arrival(message: Id, sender: &str, subject: &str) -> Arrival {
    Arrival { message, account: 1, folder: 2, sender: sender.into(), subject: subject.into() }
}

// ---- the text ----

#[test]
fn text_is_one_plain_line_without_control_or_format_characters() {
    assert_eq!(clean("  Rowan\tPike\r\n", NAME_CHARS), "Rowan Pike");
    assert_eq!(clean("Line one\nLine two\u{0}\u{7}\u{1b}[31mred", SUBJECT_CHARS), "Line one Line two [31mred");
    // A right-to-left override would show "gpj.exe" as "exe.jpg"; zero-width marks, the byte-order
    // mark, isolates and tags are gone.
    assert_eq!(clean("invoice\u{202e}gpj.exe", SUBJECT_CHARS), "invoicegpj.exe");
    assert_eq!(clean("\u{feff}Pay\u{200b}Pal\u{200d}\u{2066}x\u{2069}\u{e0041}", SUBJECT_CHARS), "PayPalx");
    assert_eq!(clean("\u{202e}\u{200b}\n\t ", SUBJECT_CHARS), "");
    assert_eq!(clean("", NAME_CHARS), "");
    // Markup is text: it is shown as it is, never read (the host escapes it where it must).
    assert_eq!(clean("<b>Hi</b> & <img src=x>", SUBJECT_CHARS), "<b>Hi</b> & <img src=x>");
}

#[test]
fn text_is_cut_on_character_boundaries_with_an_ellipsis() {
    let exactly = "a".repeat(NAME_CHARS);
    assert_eq!(clean(&exactly, NAME_CHARS), exactly, "exactly as long as allowed: kept whole");
    let long = "é".repeat(500);
    let cut = clean(&long, NAME_CHARS);
    assert_eq!(cut.chars().count(), NAME_CHARS);
    assert!(cut.ends_with('…'));
    // Characters of four bytes, and a cut that falls just after a space.
    let emoji = "\u{1f4ec}".repeat(300);
    assert_eq!(clean(&emoji, SUBJECT_CHARS).chars().count(), SUBJECT_CHARS);
    let spaced = format!("{} tail", "x".repeat(SUBJECT_CHARS - 2));
    assert_eq!(clean(&spaced, SUBJECT_CHARS), format!("{}…", "x".repeat(SUBJECT_CHARS - 2)));
    // A huge subject is not read to its end.
    let huge = "word ".repeat(2_000_000);
    assert_eq!(clean(&huge, SUBJECT_CHARS).chars().count(), SUBJECT_CHARS);
    assert_eq!(clean("anything", 0), "");
    assert_eq!(clean("ab", 1), "…");
}

// ---- what is shown ----

#[test]
fn new_mail_is_told_of_only_while_the_window_is_not_in_front_and_the_switch_is_on() {
    let found = [arrival(10, "Rowan Pike", "Lunch?")];
    assert!(plan_mail(&found, true, true).is_empty(), "the window is in front");
    assert!(plan_mail(&found, false, false).is_empty(), "the switch is off");
    assert!(plan_mail(&[], true, false).is_empty());
    let shown = plan_mail(&found, true, false);
    assert_eq!(shown.len(), 1);
    assert_eq!((shown[0].title.as_str(), shown[0].body.as_str()), ("Rowan Pike", "Lunch?"));
    assert_eq!(shown[0].opens, Opens::Message { message: 10, folder: 2 });
    // No name and no subject still say something.
    let bare = plan_mail(&[arrival(11, "\u{200b}", "")], true, false);
    assert_eq!((bare[0].title.as_str(), bare[0].body.as_str()), ("New message", "(no subject)"));
}

#[test]
fn up_to_three_get_an_alert_each_and_more_are_counted_in_one() {
    let three: Vec<Arrival> = (1..=3).map(|n| arrival(n, "A", &format!("Subject {n}"))).collect();
    assert_eq!(plan_mail(&three, true, false).len(), 3);
    let four: Vec<Arrival> = (1..=4).map(|n| arrival(n, "A", "S")).collect();
    let one = plan_mail(&four, true, false);
    assert_eq!(one.len(), 1);
    assert_eq!((one[0].title.as_str(), one[0].body.as_str()), ("New mail", "4 new messages"));
    assert_eq!(one[0].opens, Opens::Folder { folder: 2 });
}

fn reminder(title: &str, location: &str) -> Reminder {
    let start = mailcraft_time::DateTime::new(mailcraft_time::Date::parse_iso("2026-10-14").unwrap(), 16, 30, 0).unwrap();
    Reminder { event: 7, start, end: start, all_day: false, title: title.into(), location: location.into(), minutes: 15 }
}

#[test]
fn reminders_say_what_when_and_where() {
    let due = [reminder("Design review\u{202e}", "Room 4\n(east)")];
    let shown = plan_reminders(&due, true, false);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].title, "Design review");
    assert!(shown[0].body.ends_with(" · Room 4 (east)") && shown[0].body.contains("16:30"), "{}", shown[0].body);
    assert_eq!(shown[0].opens, Opens::Reminders);
    assert!(plan_reminders(&due, false, false).is_empty(), "switched off");
    let many: Vec<Reminder> = (0..5).map(|_| reminder("x", "")).collect();
    let one = plan_reminders(&many, true, false);
    assert_eq!((one.len(), one[0].body.as_str()), (1, "5 reminders are due"));
    assert_eq!(plan_reminders(&[reminder("", "")], true, true)[0].title, "(no title)");
}

// ---- the window ----

/// A window with an IMAP account whose Inbox has been read from its server once, and a stand-in
/// for the system's notifications.
fn window() -> (MailApp, Recorder, Id, Id) {
    let mut session = Session::empty();
    let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let account = session.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let inbox = session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let recorder = Recorder::default();
    let services = Services { alert: Some(Box::new(recorder.clone())), ..Default::default() };
    let mut app = MailApp::new(session, services);
    app.autosave = false;
    deliver(&mut app, account, inbox, &[(1, "Old", "Already here")]);
    (app, recorder, account, inbox)
}

fn deliver(app: &mut MailApp, account: Id, inbox: Id, messages: &[(u32, &str, &str)]) {
    let new: Vec<Message> = messages
        .iter()
        .map(|(uid, from, subject)| Message {
            uid: *uid,
            from: Address::new(*from, "sender@sender.example"),
            subject: (*subject).into(),
            ..Default::default()
        })
        .collect();
    let uid_seen = messages.iter().map(|m| m.0).max().unwrap_or(0);
    let delta =
        FolderDelta { id: Some(inbox), remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 5, uid_seen, new, ..Default::default() };
    mailcraft_engine::cmd::account::apply_sync(&mut app.session, SyncResult { account, folders: vec![delta], ..Default::default() }).unwrap();
}

/// One frame of the window's alerts, with the window in front or not.
fn frame(app: &mut MailApp, ctx: &egui::Context, focused: bool) {
    let raw = egui::RawInput { focused, ..Default::default() };
    pass(ctx, raw, |ctx| tick(app, ctx));
}

/// A Send/Receive that brings `messages` to the Inbox, as the window sees it: under way for a
/// frame, then over.
fn run(app: &mut MailApp, ctx: &egui::Context, focused: bool, account: Id, inbox: Id, messages: &[(u32, &str, &str)]) {
    frame(app, ctx, focused);
    let (_tx, rx) = std::sync::mpsc::channel();
    app.sync_rx = Some(crate::MailRun::Imap(rx));
    frame(app, ctx, focused);
    deliver(app, account, inbox, messages);
    app.sync_rx = None;
    frame(app, ctx, focused);
}

#[test]
fn a_run_that_brings_new_mail_shows_alerts_while_the_window_is_elsewhere() {
    let (mut app, recorder, account, inbox) = window();
    let ctx = egui::Context::default();
    // In front: nothing.
    run(&mut app, &ctx, true, account, inbox, &[(2, "Rowan Pike", "Seen in the window")]);
    assert!(recorder.shown.lock().unwrap().is_empty());
    // Elsewhere: an alert each.
    run(&mut app, &ctx, false, account, inbox, &[(3, "Rowan Pike", "Lunch?"), (4, "Noor\u{202e}", "Re: plans\nnext")]);
    let shown = recorder.shown.lock().unwrap().clone();
    assert_eq!(shown.iter().map(|a| (a.title.as_str(), a.body.as_str())).collect::<Vec<_>>(), [("Rowan Pike", "Lunch?"), ("Noor", "Re: plans next")]);
    assert!(shown.iter().all(|a| a.kind == AlertKind::Mail));
    // What `ui.alerts` says.
    let said = inspect(&app, &json!({}));
    assert_eq!(said["newMail"], true);
    assert_eq!(said["available"], true);
    assert_eq!(said["shown"].as_array().unwrap().len(), 2);
    assert_eq!(said["shown"][0]["title"], "Rowan Pike");
    assert_eq!(said["shown"][0]["opens"], "message");
    assert_eq!(inspect(&app, &json!({"limit": 1}))["shown"].as_array().unwrap().len(), 1);
    // Switched off: nothing more.
    app.execute("ui.desktopAlerts", json!({"newMail": false})).unwrap();
    run(&mut app, &ctx, false, account, inbox, &[(5, "A", "Quiet")]);
    assert_eq!(recorder.shown.lock().unwrap().len(), 2);
    // A run with nothing new: nothing.
    app.execute("ui.desktopAlerts", json!({"newMail": true})).unwrap();
    run(&mut app, &ctx, false, account, inbox, &[]);
    assert_eq!(recorder.shown.lock().unwrap().len(), 2);
}

#[test]
fn a_click_brings_the_message_up_and_nothing_else_opens_anything() {
    let (mut app, recorder, account, inbox) = window();
    let ctx = egui::Context::default();
    run(&mut app, &ctx, false, account, inbox, &[(2, "Rowan Pike", "Lunch?")]);
    let alert = recorder.shown.lock().unwrap()[0].clone();
    let Opens::Message { message, .. } = alert.opens else { panic!("{alert:?}") };
    // A key the window never handed out is nobody's.
    recorder.clicks.lock().unwrap().extend([alert.key + 100, 0]);
    frame(&mut app, &ctx, false);
    assert_ne!(app.session.sel.focus(), Some(message));
    recorder.clicks.lock().unwrap().push(alert.key);
    frame(&mut app, &ctx, false);
    assert_eq!(app.session.view.folder, inbox);
    assert_eq!(app.session.sel.focus(), Some(message));
}

#[test]
fn a_mailbox_opened_during_a_run_is_not_compared_with_the_one_before() {
    let (mut app, recorder, account, inbox) = window();
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, false);
    let (_tx, rx) = std::sync::mpsc::channel();
    app.sync_rx = Some(crate::MailRun::Imap(rx));
    frame(&mut app, &ctx, false);
    deliver(&mut app, account, inbox, &[(2, "A", "B")]);
    app.session.path = Some(std::path::PathBuf::from("another.mailcraft"));
    app.sync_rx = None;
    frame(&mut app, &ctx, false);
    assert!(recorder.shown.lock().unwrap().is_empty());
}

#[test]
fn no_command_or_control_method_shows_an_alert() {
    let (mut app, recorder, ..) = window();
    let ids: Vec<String> =
        app.session.registry.all().iter().map(|c| c.id.to_string()).chain(crate::UI_COMMANDS.iter().map(|c| c.0.to_string())).collect();
    let junk = [json!({}), json!({"title": "Your bank", "body": "Sign in now", "kind": "mail", "message": 1, "id": 1, "value": true})];
    for id in ids.iter().filter(|id| !matches!(id.as_str(), "ui.open" | "ui.import" | "ui.attach")) {
        for params in &junk {
            let _ = app.execute(id, params.clone());
        }
    }
    assert!(recorder.shown.lock().unwrap().is_empty());
    // `ui.alerts` only reads.
    let (req, _reply) = crate::ControlRequest::new("ui.alerts", json!({"title": "x", "body": "y", "show": true}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle(&mut app, &ctx, &req);
    assert!(recorder.shown.lock().unwrap().is_empty());
}

#[test]
fn the_switches_are_kept_and_checked() {
    let (mut app, ..) = window();
    assert!(app.ui.alert_new_mail && app.ui.alert_reminders, "both on by default");
    assert!(app.execute("ui.desktopAlerts", json!({})).is_err());
    assert!(app.execute("ui.desktopAlerts", json!({"reminders": "no"})).is_err());
    assert_eq!(app.execute("ui.desktopAlerts", json!({"reminders": false})).unwrap(), json!({"newMail": true, "reminders": false}));
    let saved: crate::UiState = serde_json::from_value(serde_json::to_value(app.prefs()).unwrap()).unwrap();
    assert!(saved.alert_new_mail && !saved.alert_reminders);
    // An older settings file has neither: both on.
    let old: crate::UiState = serde_json::from_value(json!({"theme": "dark"})).unwrap();
    assert!(old.alert_new_mail && old.alert_reminders);
}

#[test]
fn why_alerts_cant_be_shown_is_said() {
    let (mut app, ..) = window();
    app.services.alert = Some(Box::new(Recorder { trouble: Some(AlertTrouble::NoService), ..Default::default() }));
    let said = inspect(&app, &json!({}));
    assert_eq!((said["available"].clone(), said["trouble"].clone()), (json!(false), json!("noService")));
    app.services.alert = None;
    assert_eq!(inspect(&app, &json!({}))["available"], false);
    for t in [AlertTrouble::NotInstalled, AlertTrouble::OutsideApp, AlertTrouble::TurnedOff, AlertTrouble::NoService] {
        assert!(!t.notice().is_empty());
    }
}

#[test]
fn reminders_falling_due_show_an_alert_each_once_and_again_after_a_snooze() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0), Services::default());
    app.autosave = false;
    let recorder = Recorder::default();
    app.services.alert = Some(Box::new(recorder.clone()));
    let ctx = egui::Context::default();
    pass(&ctx, egui::RawInput { focused: true, ..Default::default() }, |ctx| crate::reminders::tick(&mut app, ctx));
    let first = recorder.shown.lock().unwrap().len();
    let due = app.reminders.showing.len();
    assert!(due > 0, "the sample has a reminder due at its fixed time");
    assert_eq!(first, if due > AT_ONCE { 1 } else { due });
    assert!(recorder.shown.lock().unwrap().iter().all(|a| a.kind == AlertKind::Reminder && a.opens == Opens::Reminders));
    // Not again for the same reminders.
    app.reminders.looked = 0.0;
    pass(&ctx, egui::RawInput::default(), |ctx| crate::reminders::tick(&mut app, ctx));
    assert_eq!(recorder.shown.lock().unwrap().len(), first);
    // Switched off: a snoozed one coming back says nothing.
    app.execute("ui.desktopAlerts", json!({"reminders": false})).unwrap();
    crate::reminders::snooze(&mut app, None, 1);
    app.reminders.snoozed.values_mut().for_each(|until| *until = 0.0);
    app.reminders.looked = 0.0;
    pass(&ctx, egui::RawInput::default(), |ctx| crate::reminders::tick(&mut app, ctx));
    assert_eq!(recorder.shown.lock().unwrap().len(), first);
    // Switched on: a snoozed one coming back is told of again.
    app.execute("ui.desktopAlerts", json!({"reminders": true})).unwrap();
    crate::reminders::snooze(&mut app, None, 1);
    app.reminders.snoozed.values_mut().for_each(|until| *until = 0.0);
    app.reminders.looked = 0.0;
    pass(&ctx, egui::RawInput::default(), |ctx| crate::reminders::tick(&mut app, ctx));
    assert_eq!(recorder.shown.lock().unwrap().len(), first * 2);
}

/// One pass of a context with this input.
fn pass(ctx: &egui::Context, raw: egui::RawInput, f: impl FnOnce(&egui::Context)) {
    ctx.begin_pass(raw);
    f(ctx);
    ctx.end_pass().textures_delta.clear();
}
