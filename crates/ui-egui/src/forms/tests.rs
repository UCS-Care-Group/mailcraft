// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The forms together: opening by name, and typing into them in a window without a screen.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use serde_json::json;

use super::Form;
use crate::dialogs::Dialog;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

/// The whole window, laid out a few frames so the dialog is up and its first field has the keyboard.
fn window(app: MailApp, size: (f32, f32)) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(size.0, size.1)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(6);
    harness
}

/// The window with the dialog `name` open.
fn dialog(name: &str, params: serde_json::Value) -> Harness<'static, MailApp> {
    let mut a = app();
    let mut p = params;
    p["name"] = json!(name);
    a.execute("ui.dialog", p).unwrap();
    window(a, (1280.0, 800.0))
}

fn type_text(h: &mut Harness<'static, MailApp>, text: &str) {
    h.event(egui::Event::Text(text.to_string()));
    h.run_steps(3);
}

fn press(h: &mut Harness<'static, MailApp>, key: egui::Key) {
    h.key_press(key);
    h.run_steps(4);
}

fn open_dialog(h: &Harness<'static, MailApp>) -> Option<&'static str> {
    h.state().dialog.as_ref().map(Dialog::name)
}

#[test]
fn forms_open_by_name() {
    let mut a = app();
    for (name, title, width) in [
        ("event", "New Appointment", 580.0),
        ("meeting", "New Meeting", 580.0),
        ("contact", "New Contact", 540.0),
        ("task", "New Task", 580.0),
        ("account", "Add Account", 520.0),
        ("categories", "Categories", 420.0),
        ("rules", "Rules", 620.0),
    ] {
        let f = Form::open(name, &a, &json!({})).unwrap_or_else(|| panic!("{name}"));
        assert_eq!((f.name(), f.title().as_str(), f.width()), (name, title, width));
    }
    assert!(Form::open("nonsense", &a, &json!({})).is_none());
    assert!(Form::open("", &a, &json!({"id": 1})).is_none());
    // The edit forms need something to edit: the selection, or `id`.
    for name in ["editEvent", "editContact", "editTask"] {
        assert!(Form::open(name, &a, &json!({})).is_none(), "{name}");
        assert!(Form::open(name, &a, &json!({"id": 0})).is_none(), "{name}");
    }
    let event = a.session.mailbox.events.keys().next().copied().unwrap();
    let contact = a.session.mailbox.contacts.keys().next().copied().unwrap();
    let task = a.session.mailbox.tasks.keys().next().copied().unwrap();
    for (name, id) in [("editEvent", event), ("editContact", contact), ("editTask", task)] {
        assert_eq!(Form::open(name, &a, &json!({"id": id})).map(|f| f.name()), Some(name));
    }
    (a.session.sel.event, a.session.sel.contact, a.session.sel.task) = (Some(event), Some(contact), Some(task));
    for (name, title) in [("editEvent", "Appointment"), ("editContact", "Contact"), ("editTask", "Task")] {
        let f = Form::open(name, &a, &json!({})).unwrap();
        // The first sample event is a meeting or an appointment; either way it says which.
        assert!(f.title() == title || (name == "editEvent" && f.title() == "Meeting"), "{name}: {}", f.title());
    }
    // The commands that ask for input open these forms.
    assert_eq!(a.run("tasks.new", json!({})).unwrap()["pending"], "task");
    assert_eq!(a.dialog.as_ref().map(Dialog::name), Some("task"));
    // An appointment's form opens in a window of its own; the dialog that was up stays.
    assert_eq!(a.run("calendar.edit", json!({})).unwrap()["pending"], "editEvent");
    assert_eq!((a.dialog.as_ref().map(Dialog::name), a.windows.of_event(event).is_some()), (Some("task"), true));
}

#[test]
fn the_first_field_has_the_keyboard_and_enter_saves() {
    // A task: type its title, press Enter.
    let mut h = dialog("task", json!({}));
    type_text(&mut h, "Water the plants");
    assert_eq!(open_dialog(&h), Some("task"));
    press(&mut h, egui::Key::Enter);
    assert_eq!(open_dialog(&h), None);
    assert!(h.state().session.mailbox.tasks.values().any(|t| t.title == "Water the plants" && t.due.is_none()));

    // An appointment lands on the next half hour.
    let mut h = dialog("event", json!({}));
    type_text(&mut h, "Call the printer");
    press(&mut h, egui::Key::Enter);
    assert_eq!(open_dialog(&h), None);
    let s = &h.state().session;
    let e = s.mailbox.events.values().find(|e| e.title == "Call the printer").unwrap();
    assert_eq!((e.start.iso().as_str(), e.duration_minutes(), e.reminder_minutes, s.sel.event), ("2026-10-14T16:30:00", 30, Some(15), Some(e.id)));

    // Tab moves on to the next field.
    let mut h = dialog("contact", json!({}));
    type_text(&mut h, "Noor");
    press(&mut h, egui::Key::Tab);
    type_text(&mut h, "Haddad");
    press(&mut h, egui::Key::Enter);
    assert_eq!(open_dialog(&h), None);
    assert!(h.state().session.mailbox.contacts.values().any(|c| c.first == "Noor" && c.last == "Haddad"));

    // An account is a wizard: Enter is Connect on the address page, Connect on the page with
    // the servers it suggests, and Done on the last one.
    let mut h = dialog("account", json!({}));
    type_text(&mut h, "noor@example.org");
    press(&mut h, egui::Key::Enter);
    assert_eq!(open_dialog(&h), Some("account"));
    assert!(!h.state().session.mailbox.accounts.iter().any(|a| a.email == "noor@example.org"));
    press(&mut h, egui::Key::Enter);
    assert!(h.state().session.mailbox.accounts.iter().any(|a| a.email == "noor@example.org"));
    press(&mut h, egui::Key::Enter);
    assert_eq!(open_dialog(&h), None);
}

#[test]
fn enter_waits_while_ok_is_off_and_errors_stay_in_the_form() {
    // Nothing typed: Enter does nothing.
    let mut h = dialog("task", json!({}));
    let tasks = h.state().session.mailbox.tasks.len();
    press(&mut h, egui::Key::Enter);
    assert_eq!((open_dialog(&h), h.state().session.mailbox.tasks.len()), (Some("task"), tasks));
    // A meeting needs someone to invite.
    let mut h = dialog("meeting", json!({}));
    let events = h.state().session.mailbox.events.len();
    type_text(&mut h, "Kick-off");
    press(&mut h, egui::Key::Enter);
    assert_eq!((open_dialog(&h), h.state().session.mailbox.events.len()), (Some("meeting"), events));
    // What the command refuses is said in the form, which stays open.
    let mut h = dialog("account", json!({}));
    let accounts = h.state().session.mailbox.accounts.len();
    type_text(&mut h, "not an address");
    press(&mut h, egui::Key::Enter);
    assert_eq!((open_dialog(&h), h.state().session.mailbox.accounts.len()), (Some("account"), accounts));
    assert!(h.query_by_label_contains("isn't an e-mail address").is_some());
    // Escape closes without saving.
    press(&mut h, egui::Key::Escape);
    assert_eq!((open_dialog(&h), h.state().session.mailbox.accounts.len()), (None, accounts));
}

#[test]
fn enter_in_a_multi_line_field_is_a_new_line() {
    use egui::accesskit::Role;
    let mut h = dialog("task", json!({"title": "Plan the week"}));
    // Notes is the form's one multi-line field.
    h.get_by_role(Role::MultilineTextInput).focus();
    h.run_steps(3);
    type_text(&mut h, "one");
    press(&mut h, egui::Key::Enter);
    type_text(&mut h, "two");
    assert_eq!(open_dialog(&h), Some("task"));
    assert!(!h.state().session.mailbox.tasks.values().any(|t| t.title == "Plan the week"));
    // Back in the title, Enter saves, notes and all.
    h.get_all_by_role(Role::TextInput).find(|n| n.value().as_deref() == Some("Plan the week")).unwrap().focus();
    h.run_steps(3);
    press(&mut h, egui::Key::Enter);
    assert_eq!(open_dialog(&h), None);
    let task = h.state().session.mailbox.tasks.values().find(|t| t.title == "Plan the week").unwrap();
    assert_eq!(task.notes, "one\ntwo");
}

#[test]
fn categories_are_added_one_after_another() {
    let mut h = dialog("categories", json!({}));
    type_text(&mut h, "Travel");
    press(&mut h, egui::Key::Enter);
    // The dialog stays, ready for the next name.
    assert_eq!(open_dialog(&h), Some("categories"));
    type_text(&mut h, "Reading");
    press(&mut h, egui::Key::Enter);
    let s = &h.state().session;
    let color = |name: &str| s.mailbox.category(name).map(|c| c.color.hex());
    assert_eq!((color("Travel").as_deref(), color("Reading").as_deref()), (Some("#1A9BA6"), Some("#C24E9A")));
    assert_eq!(s.mailbox.categories.len(), 8);
}

/// Never-crash: every form lays out over a mailbox full of hostile text, small and large.
#[test]
fn forms_lay_out_over_hostile_data() {
    use mailcraft_model::{Action, Address, Attendee, Category, Condition, Contact, Event, Labeled, PostalAddress, Recurrence, Rgb, Rule, Task};
    let junk = "\u{0}\u{202e}é\t<b>&amp;\n".repeat(300);
    let lines = |n: usize| vec![Labeled::new(junk.clone(), junk.clone()); n];
    for (size, theme) in [((1280.0, 800.0), "light"), ((760.0, 480.0), "dark"), ((240.0, 160.0), "light")] {
        let mut a = app();
        a.execute("ui.theme", json!({"value": theme})).unwrap();
        let mb = &mut a.session.mailbox;
        mb.categories = (0..60).map(|i| Category { name: format!("{i}{junk}"), color: Rgb(i, 0, 255 - i) }).collect();
        let contact = mb.add_contact(Contact {
            display_name: junk.clone(),
            first: junk.clone(),
            emails: lines(40),
            phones: lines(40),
            addresses: vec![PostalAddress { label: junk.clone(), street: junk.clone(), city: junk.clone(), ..Default::default() }],
            websites: vec![junk.clone()],
            notes: junk.clone(),
            categories: vec![junk.clone(); 30],
            ..Default::default()
        });
        let at = |hour| mailcraft_time::DateTime::new(mailcraft_time::Date::new(9999, 12, 31).unwrap(), hour, 59, 59).unwrap();
        let event = mb
            .add_event(Event {
                title: junk.clone(),
                location: junk.clone(),
                description: junk.clone(),
                // It ends before it starts, at the end of time.
                start: at(23),
                end: at(1),
                attendees: vec![Attendee { address: Address::new(junk.clone(), junk.clone()), ..Default::default() }; 50],
                recurrence: Some(Recurrence {
                    interval: u32::MAX,
                    nth: Some(i8::MIN),
                    count: Some(u32::MAX),
                    by_day: vec![mailcraft_time::Weekday::Sun; 40],
                    ..Default::default()
                }),
                reminder_minutes: Some(u32::MAX),
                categories: vec![junk.clone(); 30],
                calendar: 999_999,
                ..Default::default()
            })
            .unwrap();
        let task = mb.add_task(Task {
            title: junk.clone(),
            notes: junk.clone(),
            reminder: Some(i64::MAX),
            completed: Some(i64::MIN),
            categories: vec![junk.clone(); 30],
            ..Default::default()
        });
        for i in 0..40 {
            mb.rules.push(Rule {
                id: 500_000 + i,
                name: junk.clone(),
                enabled: i % 2 == 0,
                conditions: vec![Condition::BodyContains(junk.clone()); 20],
                actions: vec![Action::MoveTo(i), Action::Categorize(junk.clone())],
                stop: true,
                ..Default::default()
            });
        }
        let p = json!({"title": junk, "first": junk, "email": junk, "start": junk, "attendees": junk, "displayName": junk});
        for name in ["event", "meeting", "contact", "task", "account", "categories", "rules", "editEvent", "editContact", "editTask"] {
            let mut b = MailApp::new(Session::at(a.session.mailbox.clone(), NOW, 0), Services::default());
            b.autosave = false;
            b.ui = a.ui.clone();
            (b.session.sel.event, b.session.sel.contact, b.session.sel.task) = (Some(event), Some(contact), Some(task));
            let mut params = p.clone();
            params["name"] = json!(name);
            b.execute("ui.dialog", params).unwrap_or_else(|e| panic!("{name}: {e}"));
            let mut h = window(b, size);
            // Keys reach whatever has the keyboard, and nothing falls over.
            type_text(&mut h, &junk);
            press(&mut h, egui::Key::Tab);
            press(&mut h, egui::Key::Enter);
            assert!(h.state().dialog.is_none() || open_dialog(&h) == Some(name), "{name}");
        }
    }
}
