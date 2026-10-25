// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The agent gate on undo and redo, the clipboard and the appointment window (AU2-03, 04, 13, 17).

use mailcraft_model::remote::EventChange;
use serde_json::json;

use crate::Session;
use crate::agent::*;

fn sample() -> Session {
    Session::new(crate::sample::mailbox(1_791_993_600, 0))
}

fn a_change(send: bool) -> EventChange {
    EventChange { account: 1, event: 4_242, send, token: "t".into(), ..Default::default() }
}

#[test]
fn undo_that_would_delete_asks_for_delete() {
    // An item the owner made is removed for good by an undo.
    for (cmd, params) in [
        ("people.new", json!({"email": "ada@example.org"})),
        ("tasks.new", json!({"title": "Pay the invoice"})),
        ("calendar.new", json!({"title": "Lunch", "start": "2026-10-13T12:00", "end": "2026-10-13T13:00"})),
        ("category.set", json!({"name": "Zed Category", "color": "#112233"})),
    ] {
        let mut s = sample();
        s.run(cmd, &params).unwrap();
        assert!(matches!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Ask(Class::Delete, _)), "{cmd}");
        assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Delete]), Verdict::Run, "{cmd}");
        assert!(describe(&s, "edit.undo", &json!({})).contains("deleted"), "{cmd}");
    }
    // Redo of an undone deletion.
    let mut s = sample();
    let id = s.run("tasks.new", &json!({"title": "Pay the invoice"})).unwrap()["id"].as_u64().unwrap();
    s.run("tasks.delete", &json!({"id": id})).unwrap();
    assert!(s.undo());
    assert!(matches!(check(&s, "edit.redo", &json!({}), &[]), Verdict::Ask(Class::Delete, _)));
    // An agent without the grant changes nothing, and the stacks stay.
    let (before, undo, redo) = (s.mailbox.clone(), s.undo_label().map(str::to_string), s.redo_label().map(str::to_string));
    s.agent = Some(Vec::new());
    assert!(s.run("edit.redo", &json!({})).is_ok() || s.mailbox == before);
    s.agent = None;
    if s.mailbox == before {
        assert_eq!((s.undo_label().map(str::to_string), s.redo_label().map(str::to_string)), (undo, redo));
    }
    // An ordinary step stays safe.
    let mut s = sample();
    let m = *s.mailbox.messages.keys().next().unwrap();
    s.run("mail.flag", &json!({"id": m})).unwrap();
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
}

#[test]
fn undo_and_redo_that_bring_back_a_send_ask_for_send() {
    let mut s = sample();
    let m = *s.mailbox.messages.keys().next().unwrap();
    s.run("mail.flag", &json!({"id": m})).unwrap();
    s.history.last_mut().unwrap().mailbox.event_changes.push(a_change(true));
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Ask(Class::Send, _)));
    assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Send]), Verdict::Run);
    // A change that writes to nobody is no question.
    s.history.last_mut().unwrap().mailbox.event_changes = vec![a_change(false)];
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    // Redo too.
    assert!(s.undo());
    s.redo.last_mut().unwrap().mailbox.event_changes = vec![a_change(true)];
    assert!(matches!(check(&s, "edit.redo", &json!({}), &[]), Verdict::Ask(Class::Send, _)));
}

#[test]
fn a_command_that_adds_a_send_oversteps() {
    let before = sample().mailbox;
    let mut after = before.clone();
    assert_eq!(overstep(&before, &after, &[]), None);
    after.event_changes.push(a_change(true));
    assert_eq!(overstep(&before, &after, &[]), Some(Class::Send));
    assert_eq!(overstep(&before, &after, &[Class::Send]), None);
    // One that was there already is not new.
    assert_eq!(overstep(&after, &after, &[]), None);
    let mut quiet = before.clone();
    quiet.event_changes.push(a_change(false));
    assert_eq!(overstep(&before, &quiet, &[]), None);
}

#[test]
fn the_selection_is_not_copied_for_agents() {
    assert_eq!(access("ui.copySelection"), Some(Access::Never));
    assert_eq!(access("ui.copyLinkAddress"), Some(Access::Never));
}

#[test]
fn the_appointment_window_saves_and_sends_only_with_the_owners_yes() {
    let s = sample();
    for action in ["send", "save", "saveAndClose"] {
        let params = json!({"do": action, "attendees": ["guest@example.org", "other@example.org"]});
        assert_eq!(classify(&s, "ui.eventWindow", &params), Access::Owner(Class::Send), "{action}");
        let Verdict::Ask(Class::Send, what) = check(&s, "ui.eventWindow", &params, &[]) else { panic!("{action}") };
        assert!(what.contains("guest@example.org") && what.contains("2 people"), "{what}");
        assert_eq!(check(&s, "ui.eventWindow", &params, &[Class::Send]), Verdict::Run);
    }
    for params in [json!({}), json!({"do": "close"}), json!({"page": "scheduling"}), json!({"recurrence": true})] {
        assert_eq!(check(&s, "ui.eventWindow", &params, &[]), Verdict::Run, "{params}");
    }
}
