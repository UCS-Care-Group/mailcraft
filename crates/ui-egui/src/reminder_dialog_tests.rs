// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Reminder (#948): what the dialog asks and runs, and the reminders window with a message's
//! reminder in it.

use mailcraft_engine::Session;
use mailcraft_model::{Address, FlagStatus, FolderKind, Id, Mailbox, Message};
use serde_json::json;

use super::{AddReminder, Outcome, Target};
use crate::{MailApp, Services};

const NOW: i64 = 1_800_000_000;

struct Rig {
    app: MailApp,
    got: Id,
    mine: Id,
}

fn rig() -> Rig {
    let mut mailbox = Mailbox::with_local_account("Ann", "ann@example.org");
    let account = mailbox.accounts[0].id;
    let inbox = mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let sent = mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    let got = mailbox
        .add_message(Message {
            account,
            folder: inbox,
            subject: "Kickoff".into(),
            from: Address::new("Dan", "dan@example.org"),
            read: true,
            ..Default::default()
        })
        .unwrap();
    let mine = mailbox
        .add_message(Message {
            account,
            folder: sent,
            subject: "Budget".into(),
            message_id: "m@example.org".into(),
            to: vec![Address::new("Bob", "bob@example.org")],
            read: true,
            ..Default::default()
        })
        .unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    Rig { app, got, mine }
}

#[test]
fn the_dialog_asks_for_a_time_and_runs_add_reminder_on_the_selection() {
    let mut r = rig();
    // Nothing selected: no dialog.
    r.app.execute("mail.select", json!({"ids": []})).unwrap();
    assert!(AddReminder::open(&r.app, Target::Messages).is_none());
    r.app.execute("mail.select", json!({"id": r.got})).unwrap();
    let mut d = AddReminder::open(&r.app, Target::Messages).unwrap();
    // Mail that came can't be reminded "if nobody replies".
    assert!(!d.can_no_reply);
    d.date = "2027-01-20".into();
    d.time = "09:30".into();
    let (id, params) = d.command();
    assert_eq!((id, params.clone()), ("mail.reminder", json!({"at": "2027-01-20T09:30", "ifNoReply": false})));
    assert!(matches!(d.go(&mut r.app), Outcome::Close));
    let m = r.app.session.mailbox.message(r.got).unwrap();
    assert_eq!(m.flag, FlagStatus::Flagged);
    assert!(m.follow_up.is_some());
    // A time that isn't one is said, and the dialog stays.
    d.date = "nonsense".into();
    assert!(matches!(d.go(&mut r.app), Outcome::Open));
    assert!(!d.error.is_empty());
}

#[test]
fn for_mail_you_sent_it_offers_days_and_if_nobody_replies() {
    let mut r = rig();
    r.app.execute("mail.select", json!({"id": r.mine})).unwrap();
    let mut d = AddReminder::open(&r.app, Target::Messages).unwrap();
    assert!(d.can_no_reply);
    d.no_reply = true;
    d.days = 5;
    assert_eq!(d.command(), ("mail.reminder", json!({"days": 5, "ifNoReply": true})));
    assert!(matches!(d.go(&mut r.app), Outcome::Close));
    let f = r.app.session.mailbox.message(r.mine).unwrap().follow_up.unwrap();
    assert!(f.if_no_reply && f.at == NOW + 5 * 86_400);
    // A date instead of days.
    d.days = 0;
    d.date = "2027-02-01".into();
    d.time = "08:00".into();
    assert_eq!(d.command().1["at"], "2027-02-01T08:00");
}

#[test]
fn in_the_composer_it_runs_follow_up_for_the_message_being_written() {
    let mut r = rig();
    assert!(AddReminder::open(&r.app, Target::Composing).is_none());
    r.app.execute("mail.new", json!({"to": "bob@example.org"})).unwrap();
    let mut d = AddReminder::open(&r.app, Target::Composing).unwrap();
    assert_eq!(d.name(), "followUpDate");
    d.days = 0;
    d.date = "2027-03-01".into();
    d.time = "10:00".into();
    assert_eq!(d.command().0, "compose.followUp");
    assert!(matches!(d.go(&mut r.app), Outcome::Close));
    assert!(r.app.session.compose.as_ref().unwrap().follow_up.is_some());
}

/// A pass of the window: logic and layout.
fn pass(ctx: &egui::Context, app: &mut MailApp, time: &mut f64) {
    *time += 1.0 / 60.0;
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1280.0, 800.0));
    let output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), time: Some(*time), ..Default::default() }, |ui| {
        let ctx = ui.ctx().clone();
        app.logic(&ctx);
        app.ui(ui);
    });
    output.drop_without_applying_deltas();
}

#[test]
fn the_reminders_window_shows_a_raised_message_reminder_and_dismiss_ends_it() {
    let mut r = rig();
    r.app.execute("mail.reminder", json!({"ids": [r.got], "at": NOW + 60})).unwrap();
    r.app.session.clock.now = Some(NOW + 120);
    let (ctx, mut time) = (egui::Context::default(), 0.0);
    for _ in 0..4 {
        pass(&ctx, &mut r.app, &mut time);
    }
    assert_eq!(r.app.reminders.mail.iter().map(|m| m.id).collect::<Vec<_>>(), [r.got]);
    assert_eq!(r.app.reminders.mail[0].subject, "Kickoff");
    // Snooze puts it off, in the mailbox file.
    r.app.execute("ui.snoozeReminders", json!({"minutes": 10})).unwrap();
    assert!(r.app.reminders.mail.is_empty());
    assert!(r.app.session.mailbox.message(r.got).unwrap().follow_up.unwrap().postponed_until.is_some());
    r.app.session.clock.now = Some(NOW + 120 + 601);
    r.app.reminders.looked = 0.0;
    for _ in 0..3 {
        pass(&ctx, &mut r.app, &mut time);
    }
    assert_eq!(r.app.reminders.mail.len(), 1);
    // Dismiss All dismisses it.
    r.app.execute("ui.dismissReminder", json!({"all": true})).unwrap();
    assert!(r.app.reminders.mail.is_empty());
    assert!(r.app.session.mailbox.due_follow_ups(NOW + 10_000).is_empty());
    // Opening the message selects it in its folder.
    crate::reminders::open_mail(&mut r.app, r.got);
    assert_eq!(r.app.session.sel.messages, [r.got]);
}
