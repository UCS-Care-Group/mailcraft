// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message dragged onto Calendar, Tasks or People in the module bar (#873), with the pointer as
//! a hand moves it: a new appointment, task or contact opens filled in from it, and nothing is
//! made until it is saved; Mail takes nothing, and Escape calls the drag off.

use egui::{CursorIcon, Pos2};
use egui_kittest::Harness;
use mailcraft_engine::{Module, Session};
use mailcraft_model::{Address, Body, FolderKind, Id, Message};
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The sample, with a message on top of the Inbox that has HTML in it.
fn app() -> (MailApp, Id) {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let account = s.mailbox.accounts[0].id;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let html = "<p>Bring the <b>budget</b>.</p><p style=\"display:none\">hidden words</p>";
    let m = Message {
        account,
        folder: inbox,
        subject: "Quarterly review".into(),
        from: Address::new("Robin Vale", "robin@example.org"),
        date: NOW + 60,
        body: Body { text: "plain alternative".into(), html: Some(html.into()) },
        ..Default::default()
    };
    let id = s.mailbox.add_message(m).unwrap();
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    // No reminders: their window would stand over the module bar.
    for e in s.mailbox.events.values_mut() {
        std::sync::Arc::make_mut(e).reminder_minutes = None;
    }
    for t in s.mailbox.tasks.values_mut() {
        std::sync::Arc::make_mut(t).reminder = None;
    }
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    (app, id)
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(3);
    harness
}

/// Where the module bar's icon for `m` is.
fn module(harness: &Harness<'static, MailApp>, m: Module) -> Pos2 {
    let n = Module::ALL.iter().position(|x| *x == m).unwrap() as u64;
    harness.state().hit.rect("module", n).unwrap().center()
}

/// The row of message `id` pressed and moved to `to` a step a frame, still held.
fn drag_to(harness: &mut Harness<'static, MailApp>, id: Id, to: Pos2) {
    let start = harness.state().hit.rect("message", id).unwrap().center();
    harness.event(egui::Event::PointerMoved(start));
    harness.step();
    harness.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    harness.step();
    for k in 1..=8 {
        harness.event(egui::Event::PointerMoved(start + (to - start) * (k as f32 / 8.0)));
        harness.step();
    }
}

fn let_go(harness: &mut Harness<'static, MailApp>, at: Pos2) {
    harness.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    harness.run_steps(3);
}

/// What nothing a drop opened leaves: the items, the message, the module in view.
fn unchanged(harness: &Harness<'static, MailApp>) -> (usize, usize, usize, Module) {
    let s = &harness.state().session;
    (s.mailbox.events.len(), s.mailbox.tasks.len(), s.mailbox.contacts.len(), s.view.module)
}

/// The new appointments open in windows of their own: what each would make.
fn new_events(harness: &mut Harness<'static, MailApp>) -> Vec<Value> {
    let listed = crate::windows::inspect(harness.state());
    let ids: Vec<u64> = listed.as_array().unwrap().iter().filter(|w| w["kind"] == "newEvent").map(|w| w["id"].as_u64().unwrap()).collect();
    ids.into_iter().map(|id| harness.state_mut().windows.form_mut(id).and_then(|f| f.event()).unwrap().command().1).collect()
}

#[test]
fn a_message_dropped_on_calendar_opens_a_new_appointment_filled_in_from_it() {
    let (app, id) = app();
    let mut harness = window(app);
    let before = unchanged(&harness);
    let (folder, flag, read) = {
        let m = harness.state().session.mailbox.message(id).unwrap();
        (m.folder, m.flag, m.read)
    };
    let calendar = module(&harness, Module::Calendar);
    drag_to(&mut harness, id, calendar);
    // Over Calendar the drag is taken: the pointer is a folder's that takes it, not "not allowed".
    assert_ne!(harness.output().platform_output.cursor_icon, CursorIcon::NotAllowed);
    // And its icon is outlined as the place it goes; no other is.
    assert!(harness.state().hit.rect("moduleDrop", 1).is_some());
    assert!([0, 2, 3].iter().all(|n| harness.state().hit.rect("moduleDrop", *n).is_none()));
    let_go(&mut harness, calendar);
    let made = new_events(&mut harness);
    assert_eq!(made.len(), 1, "one window: {made:?}");
    let fields = &made[0];
    assert_eq!(fields["title"], "Quarterly review");
    let text = fields["description"].as_str().unwrap();
    assert!(text.contains("Bring the budget.") && !text.contains("hidden") && !text.contains("alternative"), "{text}");
    // 16:00 today: the next half hour.
    assert_eq!(fields["start"].as_str().map(|s| &s[..16]), Some("2026-10-14T16:30"));
    // Nothing is made until it is saved, the window stays in Mail and the message as it was.
    assert_eq!(unchanged(&harness), before);
    let m = harness.state().session.mailbox.message(id).unwrap();
    assert_eq!((m.folder, m.flag, m.read), (folder, flag, read));
}

#[test]
fn a_message_dropped_on_tasks_opens_a_new_task_and_on_people_a_new_contact() {
    let (app, id) = app();
    let mut harness = window(app);
    let before = unchanged(&harness);
    let tasks = module(&harness, Module::Tasks);
    drag_to(&mut harness, id, tasks);
    assert_ne!(harness.output().platform_output.cursor_icon, CursorIcon::NotAllowed);
    let_go(&mut harness, tasks);
    let (command, fields) = match &harness.state().dialog {
        Some(Dialog::Form(Form::Task(f))) => f.command(),
        _ => panic!("no task form"),
    };
    assert_eq!(command, "tasks.new");
    assert_eq!(fields["title"], "Quarterly review");
    assert!(fields["notes"].as_str().unwrap().contains("Bring the budget."), "{fields}");
    assert_eq!(unchanged(&harness), before);
    assert_eq!(harness.state().session.mailbox.message(id).unwrap().flag, mailcraft_model::FlagStatus::None);

    harness.state_mut().dialog = None;
    harness.run_steps(2);
    let people = module(&harness, Module::People);
    drag_to(&mut harness, id, people);
    let_go(&mut harness, people);
    let (command, fields) = match &harness.state().dialog {
        Some(Dialog::Form(Form::Contact(f))) => f.command(),
        _ => panic!("no contact form"),
    };
    assert_eq!(command, "people.new");
    assert_eq!((&fields["first"], &fields["last"]), (&json!("Robin"), &json!("Vale")), "{fields}");
    assert!(fields.to_string().contains("robin@example.org"), "{fields}");
    assert_eq!(unchanged(&harness), before);
}

#[test]
fn mail_takes_nothing_and_escape_calls_the_drag_off() {
    let (app, id) = app();
    let mut harness = window(app);
    let before = unchanged(&harness);
    // Let go over Mail: nothing opens, and the pointer said so on the way.
    let mail = module(&harness, Module::Mail);
    drag_to(&mut harness, id, mail);
    assert_eq!(harness.output().platform_output.cursor_icon, CursorIcon::NotAllowed);
    assert!(harness.state().hit.rect("moduleDrop", 0).is_none());
    let_go(&mut harness, mail);
    assert!(new_events(&mut harness).is_empty() && harness.state().dialog.is_none());
    assert_eq!(unchanged(&harness), before);
    // Escape on the way to Calendar: letting go there opens nothing.
    let calendar = module(&harness, Module::Calendar);
    drag_to(&mut harness, id, calendar);
    harness.event(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
    harness.step();
    let_go(&mut harness, calendar);
    assert!(new_events(&mut harness).is_empty() && harness.state().dialog.is_none());
    assert_eq!(unchanged(&harness), before);
    // A click on Calendar still just goes there.
    harness.event(egui::Event::PointerMoved(calendar));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos: calendar, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(2);
    assert_eq!(harness.state().session.view.module, Module::Calendar);
    assert!(new_events(&mut harness).is_empty());
}

#[test]
fn several_messages_dropped_open_one_appointment_and_say_so() {
    let (app, id) = app();
    let mut harness = window(app);
    let other = harness.state_mut().session.rows().iter().map(|r| r.id).find(|r| *r != id).unwrap();
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.state_mut().execute("mail.select", json!({"id": other, "extend": true})).unwrap();
    harness.run_steps(2);
    let calendar = module(&harness, Module::Calendar);
    drag_to(&mut harness, id, calendar);
    let_go(&mut harness, calendar);
    let made = new_events(&mut harness);
    assert_eq!(made.len(), 1);
    // The one grabbed, though the other was selected last.
    assert_eq!(made[0]["title"], "Quarterly review");
    let said = harness.state().status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert!(said.contains("2 messages") && said.contains("first"), "{said}");
}
