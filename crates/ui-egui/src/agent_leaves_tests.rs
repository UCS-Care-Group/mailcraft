// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent doesn't take the window off what the owner is changing without asking (#494): the
//! message being written, and an appointment, a contact or a task with changes that aren't saved
//! in the dialog. The owner is asked over the window; a no leaves everything as it was.

use std::sync::mpsc::Receiver;

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use crate::agent_gate::answer;
use crate::control::{ControlRequest, Outcome};
use crate::dialogs::Dialog;
use crate::people_view::testing;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

/// `command` sent by an agent allowed nothing in advance: its answer, when it has one already.
fn agent(a: &mut MailApp, command: &str, params: Value) -> (Option<Value>, Receiver<Value>) {
    allowed(a, command, params, Vec::new())
}

/// `command` sent by an agent the owner allowed `allow` in advance (`--allow`).
fn allowed(a: &mut MailApp, command: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let out = crate::control::handle(a, &egui::Context::default(), &req.from_agent(allow));
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

/// The question on screen: its class and what it says the agent wants.
fn question(a: &MailApp) -> Option<(Class, String)> {
    a.asking.front().map(|q| (q.class, q.what.clone()))
}

/// The owner has typed in a new message and not saved it.
fn written(a: &mut MailApp, subject: &str) {
    a.execute("mail.new", json!({})).unwrap();
    a.execute("compose.set", json!({"to": "ada@example.org", "subject": subject, "body": "Half a thought"})).unwrap();
}

fn draft_with(a: &MailApp, subject: &str) -> Option<Id> {
    a.session.mailbox.messages.values().find(|m| m.draft && m.subject == subject).map(|m| m.id)
}

fn received(a: &MailApp) -> Id {
    let account = a.session.mailbox.default_account().unwrap().id;
    let inbox = a.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    a.session.mailbox.messages.values().find(|m| m.folder == inbox && !m.draft && !m.incomplete && !m.unfetched).unwrap().id
}

/// What an agent can run that closes the message being written or puts another in its place.
fn leaving(a: &MailApp) -> Vec<(&'static str, Value)> {
    let other = received(a);
    let draft = a.session.mailbox.messages.values().find(|m| m.draft).unwrap().id;
    let contact = a.session.mailbox.contacts.values().find(|c| c.address().is_some()).unwrap().id;
    vec![
        ("compose.close", json!({})),
        ("mail.new", json!({})),
        ("mail.reply", json!({"id": other})),
        ("mail.replyAll", json!({"id": other})),
        ("mail.forward", json!({"id": other})),
        ("mail.forwardAsAttachment", json!({"id": other})),
        ("mail.editDraft", json!({"id": draft})),
        ("people.email", json!({"id": contact})),
    ]
}

#[test]
fn an_agent_waits_for_the_owners_yes_before_it_leaves_their_message() {
    let mut a = app();
    written(&mut a, "Mine");
    for (id, params) in leaving(&a) {
        let before = (a.session.compose.clone(), a.session.mailbox.clone());
        let (now, rx) = agent(&mut a, id, params.clone());
        assert!(now.is_none(), "{id}: asked first");
        let (class, what) = question(&a).unwrap();
        assert_eq!(class, Class::Leave, "{id}");
        assert!(what.contains("\u{201c}Mine\u{201d}") && what.contains("Drafts"), "{id}: {what}");
        assert_eq!(a.session.compose, before.0, "{id}: nothing happens before the answer");
        answer(&mut a, false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false, "{id}");
        assert_eq!(a.session.compose, before.0, "{id}: no leaves the owner in their message");
        assert!(a.session.mailbox == before.1, "{id}");
    }
    // Yes: it is saved to Drafts, and the agent's command goes on.
    let (_, rx) = agent(&mut a, "mail.new", json!({}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(draft_with(&a, "Mine").is_some());
    assert!(a.session.compose.as_ref().is_some_and(|c| c.subject.is_empty() && !c.dirty));
}

#[test]
fn an_agent_that_would_throw_the_owners_message_away_asks_for_delete() {
    let mut a = app();
    written(&mut a, "Mine");
    for (id, mut params) in leaving(&a) {
        params["discard"] = json!(true);
        let (now, _) = agent(&mut a, id, params);
        assert!(now.is_none(), "{id}: asked first");
        let (class, what) = question(&a).unwrap();
        assert_eq!(class, Class::Delete, "{id}");
        assert!(what.contains("without saving what the owner wrote"), "{id}: {what}");
        answer(&mut a, false);
        assert_eq!(a.session.compose.as_ref().map(|c| c.subject.as_str()), Some("Mine"), "{id}");
    }
    let (_, rx) = agent(&mut a, "compose.close", json!({"discard": true}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(a.session.compose.is_none() && draft_with(&a, "Mine").is_none());
}

#[test]
fn nothing_allowed_in_advance_takes_the_owners_message_without_their_yes() {
    let mut a = app();
    written(&mut a, "Mine");
    let everything = Class::ALL.to_vec();
    for (id, params) in leaving(&a) {
        let mut discard = params.clone();
        discard["discard"] = json!(true);
        for (params, class) in [(params, Class::Leave), (discard, Class::Delete)] {
            let (now, _) = allowed(&mut a, id, params.clone(), everything.clone());
            assert!(now.is_none(), "{id} {params}: asked even so");
            assert_eq!(question(&a).unwrap().0, class, "{id} {params}");
            answer(&mut a, false);
            assert_eq!(a.session.compose.as_ref().map(|c| c.subject.as_str()), Some("Mine"), "{id} {params}");
        }
    }
    // The owner's yes in the window is what lets it go.
    let (_, rx) = allowed(&mut a, "compose.close", json!({"discard": true}), everything.clone());
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(a.session.compose.is_none() && draft_with(&a, "Mine").is_none());
    // A form the owner changed, the same.
    let mut w = form("contact", "Ada");
    let (now, _) = allowed(w.state_mut(), "ui.closeDialog", json!({}), everything);
    assert!(now.is_none(), "asked even so");
    answer(w.state_mut(), false);
    assert!(form_open(&w));
}

#[test]
fn an_agents_own_message_is_left_without_a_question() {
    let mut a = app();
    let (now, _) = agent(&mut a, "mail.new", json!({}));
    assert_eq!(now.unwrap()["ok"], true);
    let (now, _) = agent(&mut a, "compose.set", json!({"subject": "The agent's", "body": "Written by the agent"}));
    assert_eq!(now.unwrap()["ok"], true);
    let (now, _) = agent(&mut a, "mail.new", json!({}));
    assert_eq!(now.unwrap()["ok"], true, "not asked");
    assert!(a.asking.is_empty());
    assert!(draft_with(&a, "The agent's").is_some(), "saved to Drafts first");
}

/// The window with the dialog `name` open, as the owner opened it, and `typed` typed into its
/// first box.
fn form(name: &str, typed: &str) -> testing::Window {
    let mut w = testing::window("view.people");
    w.state_mut().execute("ui.dialog", json!({"name": name})).unwrap();
    w.run_steps(3);
    if !typed.is_empty() {
        testing::text(&mut w, typed);
    }
    w
}

fn form_open(w: &testing::Window) -> bool {
    matches!(w.state().dialog, Some(Dialog::Form(_)))
}

#[test]
fn an_agent_asks_before_it_closes_a_form_the_owner_changed() {
    for (name, kind) in [("contact", "contact"), ("task", "task"), ("event", "appointment")] {
        let mut w = form(name, "Ada");
        let (now, rx) = agent(w.state_mut(), "ui.closeDialog", json!({}));
        assert!(now.is_none(), "{name}: asked first");
        let (class, what) = question(w.state()).unwrap();
        assert_eq!(class, Class::Delete, "{name}");
        assert_eq!(what, format!("close the {kind} \u{201c}Ada\u{201d} without saving the changes made in it"), "{name}");
        answer(w.state_mut(), false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false);
        assert!(form_open(&w), "{name}: no leaves it open");
        // Another dialog in its place is asked about too; Send/Receive Progress isn't one.
        let (now, _) = agent(w.state_mut(), "ui.dialog", json!({"name": "rules"}));
        assert!(now.is_none(), "{name}: asked first");
        assert!(question(w.state()).unwrap().1.starts_with("open the dialog \u{201c}rules\u{201d} in place of the"), "{name}");
        answer(w.state_mut(), false);
        assert!(form_open(&w), "{name}");
        let (now, _) = agent(w.state_mut(), "ui.dialog", json!({"name": "syncProgress"}));
        assert_eq!(now.unwrap()["ok"], true, "{name}");
        assert!(form_open(&w), "{name}");
        // Yes closes it.
        let (_, rx) = agent(w.state_mut(), "ui.closeDialog", json!({}));
        answer(w.state_mut(), true);
        assert_eq!(rx.try_recv().unwrap()["ok"], true);
        assert!(w.state().dialog.is_none(), "{name}");
    }
}

#[test]
fn a_form_with_nothing_changed_since_it_opened_is_closed_without_a_question() {
    // As the owner left it.
    let mut w = form("contact", "");
    let (now, _) = agent(w.state_mut(), "ui.closeDialog", json!({}));
    assert_eq!(now.unwrap()["ok"], true);
    assert!(w.state().dialog.is_none());
    // Filled in by the agent as it opened it.
    let (now, _) = agent(w.state_mut(), "ui.dialog", json!({"name": "task", "title": "The agent's"}));
    assert_eq!(now.unwrap()["ok"], true);
    assert!(form_open(&w));
    let (now, _) = agent(w.state_mut(), "ui.closeDialog", json!({}));
    assert_eq!(now.unwrap()["ok"], true);
    assert!(w.state().dialog.is_none());
}

#[test]
fn what_the_dialog_holds_is_the_windows_to_say() {
    let mut w = form("contact", "Ada");
    // An agent that says there is nothing unsaved is asked even so.
    let (now, _) = agent(w.state_mut(), "ui.closeDialog", json!({"unsaved": null}));
    assert!(now.is_none());
    answer(w.state_mut(), false);
    let (now, _) = agent(w.state_mut(), "ui.closeDialog", Value::Null);
    assert!(now.is_none());
    answer(w.state_mut(), false);
    assert!(form_open(&w));
}

#[test]
fn a_message_the_owner_wrote_in_is_asked_about_saved_or_not() {
    let everything = Class::ALL.to_vec();
    // Saved by the window as it does every few minutes, or by an agent first.
    for saves in ["window", "agent"] {
        let mut a = app();
        written(&mut a, "Mine");
        match saves {
            "window" => a.run("compose.saveDraft", json!({})).map(|_| ()).unwrap(),
            _ => assert_eq!(agent(&mut a, "compose.saveDraft", json!({})).0.unwrap()["ok"], true),
        }
        let (now, _) = allowed(&mut a, "mail.new", json!({}), everything.clone());
        assert!(now.is_none(), "{saves}: asked");
        assert_eq!(question(&a).unwrap().0, Class::Leave, "{saves}");
        answer(&mut a, false);
        assert_eq!(a.session.compose.as_ref().map(|c| c.subject.as_str()), Some("Mine"), "{saves}");
    }
}

#[test]
fn discard_asks_the_owner_whatever_is_allowed() {
    let mut a = app();
    written(&mut a, "Mine");
    a.run("compose.saveDraft", json!({})).unwrap();
    let (now, _) = allowed(&mut a, "compose.discard", json!({}), Class::ALL.to_vec());
    assert!(now.is_none(), "asked");
    assert_eq!(question(&a).unwrap().0, Class::Delete);
    answer(&mut a, false);
    assert!(a.session.compose.is_some() && draft_with(&a, "Mine").is_some());
}

#[test]
fn an_agent_asks_before_it_closes_an_appointments_window_the_owner_changed() {
    let mut w = testing::window("view.calendar");
    let event = w.state().session.mailbox.events.values().find(|e| !e.title.is_empty() && e.recurrence.is_none()).unwrap().id;
    let window = w.state_mut().run("ui.openWindow", json!({"event": event})).unwrap()["window"].as_u64().unwrap();
    w.run_steps(4);
    testing::text(&mut w, " (moved)");
    assert_eq!(w.state().windows.unsaved(), 1);
    let everything = Class::ALL.to_vec();
    for params in [json!({}), json!({"all": true}), json!({"window": window}), json!({"event": event})] {
        let (now, rx) = allowed(w.state_mut(), "ui.closeWindow", params.clone(), everything.clone());
        assert!(now.is_none(), "{params}: asked even so");
        let (class, what) = question(w.state()).unwrap();
        assert_eq!(class, Class::Delete, "{params}");
        assert!(what.contains("(moved)\u{201d} without saving the changes made in it"), "{params}: {what}");
        answer(w.state_mut(), false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false);
        assert_eq!(w.state().windows.len(), 1, "{params}: no leaves it open");
    }
    // Another window, with nothing changed in it, is closed as before.
    let message = received(w.state());
    w.state_mut().run("ui.openWindow", json!({"message": message})).unwrap();
    w.run_steps(4);
    let (now, _) = allowed(w.state_mut(), "ui.closeWindow", json!({"message": message}), vec![Class::Delete]);
    assert_eq!(now.unwrap()["ok"], true);
    assert_eq!(w.state().windows.len(), 1);
    // Yes closes it.
    let (_, rx) = agent(w.state_mut(), "ui.closeWindow", json!({}));
    answer(w.state_mut(), true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert_eq!(w.state().windows.len(), 0);
}

#[test]
fn a_form_the_owner_is_changing_stays_whatever_dialog_an_agent_would_open() {
    let mut w = form("contact", "Ada");
    let account = w.state_mut().execute("account.add", json!({"email": "one@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    // The account's sign-in help is a dialog of its own.
    let (now, _) = allowed(w.state_mut(), "ui.approvalHelp", json!({"account": account}), Class::ALL.to_vec());
    let said = now.unwrap();
    assert_eq!(said["ok"], false, "{said}");
    assert!(said["error"].as_str().unwrap().contains("the form stays"), "{said}");
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.unsaved().is_some_and(|u| u["title"] == "Ada")));
    // A command that opens nothing leaves it as it was.
    let (now, _) = agent(w.state_mut(), "view.calendar", json!({}));
    assert_eq!(now.unwrap()["ok"], true);
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.unsaved().is_some_and(|u| u["title"] == "Ada")));
    // With nothing changed in the form, the dialog opens in its place, as before.
    w.state_mut().execute("ui.closeDialog", json!({})).unwrap();
    w.state_mut().execute("ui.dialog", json!({"name": "contact"})).unwrap();
    w.run_steps(3);
    let (now, _) = agent(w.state_mut(), "ui.approvalHelp", json!({"account": account}));
    assert_eq!(now.unwrap()["ok"], true);
    assert_eq!(w.state().dialog.as_ref().map(|d| d.name()), Some("account"));
}

#[test]
fn a_yes_lets_go_only_of_the_form_it_was_asked_about() {
    let mut w = form("contact", "Ada");
    let (_, rx) = agent(w.state_mut(), "ui.closeDialog", json!({}));
    // What the window has now isn't what the question named. (The question is over the window,
    // so the owner can't change the form under it: this checks the comparison itself.)
    w.state_mut().asking.front_mut().unwrap().params["unsaved"]["title"] = json!("Somebody else");
    answer(w.state_mut(), true);
    let said = rx.try_recv().unwrap();
    assert_eq!(said["ok"], false, "{said}");
    assert!(form_open(&w));
}

#[test]
fn opening_a_draft_in_a_window_is_asked_about_as_open_draft_is() {
    let mut a = app();
    written(&mut a, "Mine");
    let draft = a.session.mailbox.messages.values().find(|m| m.draft && m.subject != "Mine").unwrap().id;
    let (now, _) = allowed(&mut a, "ui.openWindow", json!({"message": draft}), Class::ALL.to_vec());
    assert!(now.is_none(), "asked");
    let (class, what) = question(&a).unwrap();
    assert_eq!(class, Class::Leave);
    assert!(what.starts_with("open the draft"), "{what}");
    answer(&mut a, false);
    assert_eq!(a.session.compose.as_ref().map(|c| c.subject.as_str()), Some("Mine"));
}

#[test]
fn closing_several_changed_windows_says_how_many() {
    let mut w = testing::window("view.calendar");
    let events: Vec<Id> =
        w.state().session.mailbox.events.values().filter(|e| !e.title.is_empty() && e.recurrence.is_none()).map(|e| e.id).take(2).collect();
    for event in &events {
        w.state_mut().run("ui.openWindow", json!({"event": event})).unwrap();
        w.run_steps(4);
        testing::text(&mut w, " (moved)");
    }
    assert_eq!(w.state().windows.unsaved(), 2);
    let (now, _) = agent(w.state_mut(), "ui.closeWindow", json!({"all": true}));
    assert!(now.is_none());
    let (_, what) = question(w.state()).unwrap();
    assert!(what.contains(" and 1 more window without saving"), "{what}");
}

#[test]
fn another_mailbox_in_view_asks_before_it_closes_a_changed_appointments_window() {
    let mut w = testing::window("view.calendar");
    let event = w.state().session.mailbox.events.values().find(|e| !e.title.is_empty() && e.recurrence.is_none()).unwrap().id;
    w.state_mut().run("ui.openWindow", json!({"event": event})).unwrap();
    w.run_steps(4);
    testing::text(&mut w, " (moved)");
    let everything = Class::ALL.to_vec();
    let (now, rx) = allowed(w.state_mut(), "file.new", json!({}), everything);
    assert!(now.is_none(), "asked even so");
    let (class, what) = question(w.state()).unwrap();
    assert_eq!(class, Class::Mailbox);
    assert!(what.contains("(moved)\u{201d} is closed without saving the changes made in it"), "{what}");
    answer(w.state_mut(), false);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    w.run_steps(3);
    assert_eq!(w.state().windows.unsaved(), 1, "no leaves it open");
    // Yes: another mailbox, and the window goes with the old one.
    let (_, rx) = allowed(w.state_mut(), "file.new", json!({}), Class::ALL.to_vec());
    answer(w.state_mut(), true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    w.run_steps(3);
    assert_eq!(w.state().windows.len(), 0);
}

/// The appointment `event` open in a window of its own, with " (moved)" typed into its title.
fn changing_in_a_window(w: &mut testing::Window, event: Id) {
    w.state_mut().run("ui.openWindow", json!({"event": event})).unwrap();
    w.run_steps(4);
    testing::text(w, " (moved)");
    assert_eq!(w.state().windows.unsaved(), 1);
}

#[test]
fn taking_away_an_appointment_whose_window_has_changes_is_asked_about() {
    let mut w = testing::window("view.calendar");
    let mut events = w.state().session.mailbox.events.values().filter(|e| !e.title.is_empty() && e.recurrence.is_none()).map(|e| (e.id, e.calendar));
    let ((event, calendar), (other, _)) = (events.next().unwrap(), events.next().unwrap());
    changing_in_a_window(&mut w, event);
    let everything = Class::ALL.to_vec();
    for (id, params) in [("calendar.delete", json!({"id": event})), ("calendar.removeCalendar", json!({"id": calendar}))] {
        let (now, rx) = allowed(w.state_mut(), id, params, everything.clone());
        assert!(now.is_none(), "{id}: asked even so");
        let (class, what) = question(w.state()).unwrap();
        assert_eq!(class, Class::Delete, "{id}");
        assert!(what.contains("(moved)\u{201d} is closed without saving the changes made in it"), "{id}: {what}");
        answer(w.state_mut(), false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false);
        w.run_steps(3);
        assert_eq!(w.state().windows.unsaved(), 1, "{id}: no leaves it open");
    }
    // Another appointment goes as allowed, and the window stays.
    let (now, _) = allowed(w.state_mut(), "calendar.delete", json!({"id": other}), everything);
    assert_eq!(now.unwrap()["ok"], true);
    w.run_steps(3);
    assert_eq!(w.state().windows.unsaved(), 1);
}

#[test]
fn an_undo_that_takes_away_an_appointment_being_changed_is_asked_about() {
    let mut w = testing::window("view.calendar");
    let made = w.state_mut().execute("calendar.new", json!({"title": "Made just now", "start": "2026-10-15T10:00"})).unwrap();
    let event = made["id"].as_u64().unwrap();
    changing_in_a_window(&mut w, event);
    let (now, _) = allowed(w.state_mut(), "edit.undo", json!({}), Class::ALL.to_vec());
    assert!(now.is_none(), "asked even so");
    assert!(question(w.state()).unwrap().1.contains("(moved)\u{201d} is closed without saving"));
    answer(w.state_mut(), false);
    assert!(w.state().session.mailbox.event(event).is_some());
}

#[test]
fn the_appointment_is_known_however_its_number_is_written() {
    let mut w = testing::window("view.calendar");
    let (event, calendar) =
        w.state().session.mailbox.events.values().find(|e| !e.title.is_empty() && e.recurrence.is_none()).map(|e| (e.id, e.calendar)).unwrap();
    changing_in_a_window(&mut w, event);
    w.state_mut().execute("calendar.select", json!({"id": event})).unwrap();
    // `12.0` is 12, and 0 is none (the selected one), as the commands read them.
    for (id, params) in [
        ("calendar.delete", json!({"id": event as f64})),
        ("calendar.delete", json!({"id": 0})),
        ("calendar.removeCalendar", json!({"id": calendar as f64})),
    ] {
        let (now, rx) = allowed(w.state_mut(), id, params.clone(), Class::ALL.to_vec());
        assert!(now.is_none(), "{id} {params}: asked even so");
        assert!(question(w.state()).unwrap().1.contains("(moved)\u{201d} is closed without saving"), "{id} {params}");
        answer(w.state_mut(), false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false);
        w.run_steps(3);
        assert_eq!(w.state().windows.unsaved(), 1, "{id} {params}: no leaves it open");
    }
}

/// A message from Bo with a calendar part saying `method` of the meeting `uid`, to `owner`.
fn meeting_mail(method: &str, uid: &str, owner: &str) -> mailcraft_model::Message {
    let ics = [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Example//Meetings 1.0//EN",
        &format!("METHOD:{method}"),
        "BEGIN:VEVENT",
        &format!("UID:{uid}"),
        "SUMMARY:Design review",
        "DTSTART:20261015T140000Z",
        "DTEND:20261015T150000Z",
        "ORGANIZER;CN=Bo Marsh:mailto:bo@example.org",
        &format!("ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:{owner}"),
        "END:VEVENT",
        "END:VCALENDAR",
    ]
    .join("\r\n");
    mailcraft_model::Message {
        message_id: format!("{method}.{uid}@example.org"),
        from: mailcraft_model::Address::new("Bo Marsh", "bo@example.org"),
        to: vec![mailcraft_model::Address::new("", owner)],
        subject: format!("{method}: Design review"),
        date: NOW - 3_600,
        received: NOW - 3_600,
        attachments: vec![mailcraft_model::Attachment::new("invite.ics", "text/calendar", (ics + "\r\n").into_bytes())],
        ..mailcraft_model::Message::default()
    }
}

/// A meeting Bo invited the owner to and the owner accepted, and Bo's message calling it off:
/// the meeting, and that message.
fn a_meeting_called_off(w: &mut testing::Window) -> (Id, Id) {
    let account = w.state().session.mailbox.default_account().unwrap();
    let (account, owner) = (account.id, account.email.clone());
    let inbox = w.state().session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let mut deliver = |method: &str| {
        let m = meeting_mail(method, "uid-agent-494", &owner);
        let id = w.state_mut().session.mailbox.add_message(mailcraft_model::Message { folder: inbox, ..m }).unwrap();
        w.state_mut().session.mailbox_changed();
        id
    };
    let request = deliver("REQUEST");
    let cancel = deliver("CANCEL");
    w.state_mut().execute("calendar.respond", json!({"message": request, "response": "accept"})).unwrap();
    let event = w.state().session.mailbox.events.values().find(|e| e.uid == "uid-agent-494").unwrap().id;
    (event, cancel)
}

/// What the agent's answer says, when it refused.
fn refused(now: Option<Value>) -> String {
    let now = now.unwrap();
    assert_eq!(now["ok"], false, "{now}");
    now["error"].as_str().unwrap().to_string()
}

#[test]
fn no_other_command_takes_away_an_appointment_being_changed_unasked() {
    let mut w = testing::window("view.calendar");
    let (event, cancel) = a_meeting_called_off(&mut w);
    changing_in_a_window(&mut w, event);
    // An agent takes the meeting Bo called off out of the calendar, as it may when nothing is open.
    let (now, _) = allowed(w.state_mut(), "calendar.removeCancelled", json!({"message": cancel}), Class::ALL.to_vec());
    let why = refused(now);
    assert!(why.contains("the owner is changing in the window"), "{why}");
    w.run_steps(3);
    assert!(w.state().session.mailbox.event(event).is_some());
    assert_eq!(w.state().windows.unsaved(), 1, "the window stays with the owner's changes");
    // Once the owner has saved it, the agent's command does what it says.
    w.state_mut().run("ui.eventWindow", json!({"do": "save"})).unwrap();
    w.run_steps(4);
    assert_eq!(w.state().windows.unsaved(), 0);
    let (now, _) = allowed(w.state_mut(), "calendar.removeCancelled", json!({"message": cancel}), Vec::new());
    assert_eq!(now.unwrap()["ok"], true);
    assert!(w.state().session.mailbox.event(event).is_none());
}

#[test]
fn nor_one_being_changed_in_the_dialog() {
    let mut w = testing::window("view.calendar");
    let (event, cancel) = a_meeting_called_off(&mut w);
    w.state_mut().execute("ui.dialog", json!({"name": "editEvent", "id": event})).unwrap();
    w.run_steps(3);
    testing::text(&mut w, " (moved)");
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.dirty()), "the owner is changing it in the dialog");
    let (now, _) = allowed(w.state_mut(), "calendar.removeCancelled", json!({"message": cancel}), Class::ALL.to_vec());
    let why = refused(now);
    assert!(why.contains("the owner is changing in the window"), "{why}");
    assert!(w.state().session.mailbox.event(event).is_some());
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.unsaved().unwrap()["title"].as_str().unwrap().ends_with("(moved)")));
}

#[test]
fn an_appointment_being_changed_in_the_dialog_isnt_taken_away_and_nobody_is_asked() {
    let mut w = testing::window("view.calendar");
    let event = w.state().session.mailbox.events.values().find(|e| !e.title.is_empty() && e.recurrence.is_none()).unwrap().id;
    w.state_mut().execute("ui.dialog", json!({"name": "editEvent", "id": event})).unwrap();
    w.run_steps(3);
    testing::text(&mut w, " (moved)");
    // A yes couldn't let it go: the form stays whatever the answer. So there is no question.
    for allow in [Vec::new(), Class::ALL.to_vec()] {
        let (now, _) = allowed(w.state_mut(), "calendar.delete", json!({"id": event}), allow);
        let why = refused(now);
        assert!(why.contains("the owner is changing in the dialog"), "{why}");
        assert!(w.state().asking.is_empty());
    }
    assert!(w.state().session.mailbox.event(event).is_some());
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.dirty()));
}

#[test]
fn what_the_owner_typed_in_an_appointments_window_isnt_overwritten() {
    use egui_kittest::kittest::Queryable as _;
    let mut w = testing::window("view.calendar");
    let event = w.state().session.mailbox.events.values().find(|e| !e.title.is_empty() && e.recurrence.is_none()).unwrap().id;
    changing_in_a_window(&mut w, event);
    let window = w.state().windows.of_event(event).unwrap();
    // Another time somebody proposed, over the owner's changes.
    let proposed = json!({"event": event, "proposed": {"start": "2026-10-20T10:00", "end": "2026-10-20T11:00"}});
    let why = refused(agent(w.state_mut(), "ui.openWindow", proposed).0);
    assert!(why.contains("overwrite what the owner typed"), "{why}");
    // The recurrence the owner is filling in.
    w.state_mut().run("ui.eventWindow", json!({"window": window, "recurrence": true})).unwrap();
    w.run_steps(3);
    // It ends after a number of times.
    let after = w.query_all_by_label_contains("After").last().unwrap().rect();
    testing::click(&mut w, after.center(), egui::PointerButton::Primary, 1);
    let why = refused(agent(w.state_mut(), "ui.eventWindow", json!({"window": window, "recurrence": false})).0);
    assert!(why.contains("overwrite what the owner typed"), "{why}");
    assert_eq!(w.state_mut().run("ui.eventWindow", json!({"window": window})).unwrap()["recurrence"], true, "still open");
    assert_eq!(w.state().windows.unsaved(), 1);
}

/// The window with the contact `contact`, or the task `task`, open in the dialog as the owner
/// opened it, " (moved)" typed into it.
fn changing_in_the_dialog(name: &str, id: Id) -> testing::Window {
    let mut w = testing::window(if name == "editTask" { "view.tasks" } else { "view.people" });
    w.state_mut().execute("ui.dialog", json!({"name": name, "id": id})).unwrap();
    w.run_steps(3);
    testing::text(&mut w, " (moved)");
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.dirty()), "{name}");
    w
}

#[test]
fn a_contact_or_a_task_being_changed_in_the_dialog_isnt_taken_away() {
    let s = app().session;
    let contact = s.mailbox.contacts.values().next().unwrap().id;
    let task = s.mailbox.tasks.values().next().unwrap().id;
    for (name, id, delete) in [("editContact", contact, "people.delete"), ("editTask", task, "tasks.delete")] {
        let mut w = changing_in_the_dialog(name, id);
        for allow in [Vec::new(), Class::ALL.to_vec()] {
            let why = refused(allowed(w.state_mut(), delete, json!({"id": id}), allow).0);
            assert!(why.contains("the owner is changing in the dialog"), "{name}: {why}");
            assert!(w.state().asking.is_empty(), "{name}: no question whose yes would do nothing");
        }
        // Nor another mailbox in view, which takes it away with the old one.
        let read = received(w.state());
        w.state_mut().execute("mail.markRead", json!({"id": read})).unwrap();
        let why = refused(allowed(w.state_mut(), "file.new", json!({}), Class::ALL.to_vec()).0);
        assert!(why.contains("the owner is changing in the dialog"), "{name}: {why}");
        assert!(w.state().session.can_undo(), "{name}: the steps to undo are kept");
        let s = &w.state().session.mailbox;
        assert!(s.contact(id).is_some() || s.task(id).is_some(), "{name}");
        assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.dirty()), "{name}");
    }
}

#[test]
fn an_undo_that_takes_away_a_contact_being_changed_in_the_dialog_is_refused() {
    let mut w = testing::window("view.people");
    let made = w.state_mut().execute("people.new", json!({"name": "Made Here", "email": "here@example.org"})).unwrap();
    let here = made["id"].as_u64().unwrap();
    w.state_mut().execute("ui.dialog", json!({"name": "editContact", "id": here})).unwrap();
    w.run_steps(3);
    testing::text(&mut w, " (moved)");
    let why = refused(allowed(w.state_mut(), "edit.undo", json!({}), Class::ALL.to_vec()).0);
    assert!(why.contains("the owner is changing in the dialog"), "{why}");
    assert!(w.state().session.mailbox.contact(here).is_some() && w.state().session.can_undo());
}

#[test]
fn a_merge_that_takes_away_a_contact_being_changed_is_undone() {
    let s = app().session;
    let mut contacts = s.mailbox.contacts.values().map(|c| c.id);
    let (kept, changing) = (contacts.next().unwrap(), contacts.next().unwrap());
    let mut w = changing_in_the_dialog("editContact", changing);
    let (now, _) = allowed(w.state_mut(), "people.merge", json!({"keep": kept, "others": [changing]}), Class::ALL.to_vec());
    let why = refused(now);
    assert!(why.contains("the owner is changing in the window"), "{why}");
    assert!(w.state().session.mailbox.contact(changing).is_some());
}

#[test]
fn another_mailbox_doesnt_wait_for_a_save_for_an_agent() {
    let mut a = app();
    let path = std::env::temp_dir().join(format!("mailcraft-agent-494-{}.mailcraft", std::process::id()));
    a.session.path = Some(path);
    a.services.saver = Some(Box::new(mailcraft_engine::store::Saver::spawn(|| {}).unwrap()));
    a.execute("mail.markRead", json!({"id": received(&a)})).unwrap();
    assert!(a.waits_for_save());
    // It would run once the mailbox is written, when the owner may be writing a message.
    let why = refused(allowed(&mut a, "file.new", json!({}), Class::ALL.to_vec()).0);
    assert!(why.contains("wait for the mailbox to be saved"), "{why}");
    assert!(a.after_save.is_none());
}

#[test]
fn closing_a_message_whose_saved_draft_an_agent_deleted_asks_the_owner() {
    let mut a = app();
    written(&mut a, "Mine");
    a.run("compose.saveDraft", json!({})).unwrap();
    let draft = draft_with(&a, "Mine").unwrap();
    let (now, _) = allowed(&mut a, "mail.deletePermanently", json!({"ids": [draft]}), Class::ALL.to_vec());
    assert_eq!(now.unwrap()["ok"], true);
    assert!(a.session.mailbox.message(draft).is_none());
    // What is written is saved nowhere now: closing it asks Unsaved Message.
    let _ = a.run("compose.close", json!({}));
    assert!(matches!(a.dialog, Some(Dialog::DiscardDraft { .. })));
    assert_eq!(a.session.compose.as_ref().map(|c| c.subject.as_str()), Some("Mine"));
}

#[test]
fn a_recurrence_being_filled_in_counts_as_changes_not_saved() {
    use egui_kittest::kittest::Queryable as _;
    let mut w = testing::window("view.calendar");
    let event = w.state().session.mailbox.events.values().find(|e| !e.title.is_empty() && e.recurrence.is_none()).unwrap().id;
    w.state_mut().run("ui.openWindow", json!({"event": event})).unwrap();
    w.run_steps(4);
    let window = w.state().windows.of_event(event).unwrap();
    w.state_mut().run("ui.eventWindow", json!({"window": window, "recurrence": true})).unwrap();
    w.run_steps(3);
    // It ends after a number of times: changed in the dialog, not yet in the form (that is OK's).
    let after = w.query_all_by_label_contains("After").last().unwrap().rect();
    testing::click(&mut w, after.center(), egui::PointerButton::Primary, 1);
    assert_eq!(w.state().windows.unsaved(), 0, "nothing changed in the form itself");
    let why = refused(agent(w.state_mut(), "ui.eventWindow", json!({"window": window, "do": "close"})).0);
    assert!(why.contains("overwrite what the owner typed"), "{why}");
    for (id, params) in [("ui.closeWindow", json!({"window": window})), ("calendar.delete", json!({"id": event})), ("file.new", json!({}))] {
        let (now, rx) = allowed(w.state_mut(), id, params, Class::ALL.to_vec());
        assert!(now.is_none(), "{id}: asked even so");
        answer(w.state_mut(), false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false);
        w.run_steps(3);
    }
    assert_eq!(w.state_mut().run("ui.eventWindow", json!({"window": window})).unwrap()["recurrence"], true, "still open");
}

#[test]
fn quitting_asks_about_a_message_whose_saved_draft_an_agent_deleted() {
    let mut a = app();
    written(&mut a, "Mine");
    a.run("compose.saveDraft", json!({})).unwrap();
    let draft = draft_with(&a, "Mine").unwrap();
    let (now, _) = allowed(&mut a, "mail.deletePermanently", json!({"ids": [draft]}), Class::ALL.to_vec());
    assert_eq!(now.unwrap()["ok"], true);
    assert!(!a.close_requested());
    assert!(matches!(&a.dialog, Some(Dialog::DiscardDraft { then, .. }) if then == "quit"));
}

#[test]
fn an_undo_that_would_put_back_an_earlier_draft_of_the_owners_message_is_refused() {
    let mut a = app();
    written(&mut a, "Mine");
    a.run("compose.saveDraft", json!({})).unwrap();
    a.execute("compose.set", json!({"body": "Half a thought, and the rest of it"})).unwrap();
    a.run("compose.saveDraft", json!({})).unwrap();
    let label = a.session.undo_label();
    for undo in ["edit.undo", "edit.redo"] {
        if undo == "edit.redo" {
            // The owner undid the last save themselves: a redo by the agent would put it back.
            a.run("edit.undo", json!({})).unwrap();
        }
        let why = refused(allowed(&mut a, undo, json!({}), Class::ALL.to_vec()).0);
        assert!(why.contains("earlier draft of the message the owner is writing"), "{undo}: {why}");
        assert!(a.asking.is_empty(), "{undo}");
    }
    assert!(label.is_some() && a.session.can_redo(), "nothing more was undone or redone");
    assert_eq!(a.session.compose.as_ref().map(|c| c.subject.as_str()), Some("Mine"));
}

#[test]
fn an_agents_undo_doesnt_take_away_what_the_owners_yes_saved() {
    let mut a = app();
    written(&mut a, "Mine");
    let (_, rx) = agent(&mut a, "mail.new", json!({}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(draft_with(&a, "Mine").is_some(), "saved on the owner's yes");
    let (now, rx) = agent(&mut a, "edit.undo", json!({}));
    assert!(now.is_none(), "asked first");
    let (class, what) = question(&a).unwrap();
    assert_eq!(class, Class::Delete);
    assert!(what.contains("takes away the draft \u{201c}Mine\u{201d} in Drafts"), "{what}");
    answer(&mut a, false);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    let m = received(&a);
    let _ = agent(&mut a, "mail.flag", json!({"id": m}));
    assert!(draft_with(&a, "Mine").is_some(), "the owner's text is gone");
}

#[test]
fn an_agents_undo_of_a_later_save_asks_once_the_message_is_closed() {
    // The owner saves, writes more, saves and closes the message: undoing the last save would
    // put back the draft without what was written last.
    let mut a = app();
    written(&mut a, "Mine");
    a.run("compose.saveDraft", json!({})).unwrap();
    a.execute("compose.set", json!({"body": "Half a thought, and the rest of it"})).unwrap();
    a.run("compose.saveDraft", json!({})).unwrap();
    a.run("compose.close", json!({})).unwrap();
    assert!(a.session.compose.is_none());
    let (now, _) = agent(&mut a, "edit.undo", json!({}));
    assert!(now.is_none(), "asked first");
    assert_eq!(question(&a).map(|q| q.0), Some(Class::Delete));
    answer(&mut a, false);
    let draft = draft_with(&a, "Mine").and_then(|d| a.session.mailbox.message(d)).unwrap();
    assert!(draft.body.text.contains("the rest of it"));
    // `--allow delete` covers it, as it covers deleting the draft.
    let (now, _) = allowed(&mut a, "edit.undo", json!({}), vec![Class::Delete]);
    assert_eq!(now.unwrap()["ok"], true);
}

#[test]
fn ticking_completed_on_a_task_is_a_change() {
    use egui_kittest::kittest::Queryable as _;
    let mut w = testing::window("view.tasks");
    let task = w.state().session.mailbox.tasks.values().find(|t| !t.is_complete()).unwrap().id;
    w.state_mut().execute("ui.dialog", json!({"name": "editTask", "id": task})).unwrap();
    w.run_steps(3);
    assert!(!w.state().dialog.as_ref().is_some_and(|d| matches!(d, Dialog::Form(f) if f.dirty())));
    let tick = w.query_by_label("Completed").unwrap().rect();
    testing::click(&mut w, tick.center(), egui::PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.dirty()), "only Completed ticked");
    let (now, _) = agent(w.state_mut(), "ui.closeDialog", json!({}));
    assert!(now.is_none(), "asked first");
    assert_eq!(question(w.state()).map(|q| q.0), Some(Class::Delete));
}
