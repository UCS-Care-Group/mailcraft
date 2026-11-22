// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Two source-only Paste-route regressions, meant as the last child of
//! `calendar_selection_tests`:
//!
//! ```text
//! #[path = "paste_route_tests.rs"]
//! mod paste_route_tests;
//! ```
//!
//! One: a single whole local appointment, cut with the real Cut, a real pointer-picked slot and a
//! real unsaved appointment form, is refused by the programmatic `execute("ui.calendarPaste", {})`
//! with the exact `Clip`, mailbox, queued changes, selection, opening, undo history and form left
//! alone; the canonical `calendar.move` payload tells nobody, and `windows::removing_unsaved`
//! does not see it, while the cut-paste payload does — the single-item guard seam. Two: the
//! owner's `run("ui.calendarPaste", {})` on a cut server-kept meeting returns `pending: confirm`
//! with the cut retained, and its real Cancel and real approve buttons are both reached.
//!
//! Nothing here was compiled or run. The fixture clock is the harness's fixed one, named by
//! [`fixture_clock`]; the selection is never silently restored by frame steps after a pick, and
//! test two re-picks with the pointer before re-asking and checks the pick at once.

use std::sync::Arc;

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::calendar::{Mail, mail_of};
use mailcraft_model::{Address, Attendee, Event, Origin};
use serde_json::json;

use super::super::{CLIP_ID, Clip, Slot, load_slot};
use super::*;
use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::people_view::testing::{dialog, now, window_on};

/// The harness's fixed clock — Wednesday 2026-10-14 16:00 UTC — named once here so no fixture
/// reads the wall clock.
fn fixture_clock() -> i64 {
    now()
}

/// The `Clip` the window keeps on the calendar's clipboard.
fn clipboard(w: &Window) -> Option<Clip> {
    w.ctx.data(|data| data.get_temp::<Clip>(egui::Id::unique(CLIP_ID)))
}

/// The time the pointer picked, as the grid stored it.
fn picked(w: &Window) -> Slot {
    load_slot(&w.ctx).expect("the click picked a time in the calendar")
}

/// Where `id` starts now, or `None` when it is no longer there.
fn starts_at(w: &Window, id: Id) -> Option<DateTime> {
    w.state().session.mailbox.event(id).map(|event| event.start)
}

/// What Undo and Redo would do, and how many steps have moved.
fn undo_state(w: &Window) -> (bool, Option<&'static str>, Option<&'static str>, u64) {
    let s = &w.state().session;
    (s.can_undo(), s.undo_label(), s.redo_label(), s.undo_steps_moved())
}

/// A start in the `YYYY-MM-DDTHH:MM` spelling a command takes.
fn minute_iso(t: DateTime) -> String {
    format!("{}T{:02}:{:02}", t.date.iso(), t.hour, t.minute)
}

/// The open question: its command, its approve button, its cancel button and its text.
fn open_question(w: &Window, why: &str) -> (String, String, String, String) {
    match &w.state().dialog {
        Some(Dialog::Confirm { command, ask, .. }) => {
            (command.clone(), ask.action.clone(), ask.cancel.clone().unwrap_or_else(|| "Cancel".to_string()), ask.text.clone())
        }
        Some(other) => panic!("{why}: the window is asking something else ({})", other.name()),
        None => panic!("{why}: no question is open"),
    }
}

/// The lowest button of the window that says exactly `label`.
fn button_rect(w: &Window, label: &str) -> Option<Rect> {
    let mut found: Vec<Rect> = w
        .query_all_by_label_contains(label)
        .filter(|node| node.accesskit_node().role() == Role::Button)
        .filter(|node| node.accesskit_node().label().is_some_and(|said| said.trim() == label))
        .map(|node| node.rect())
        .collect();
    found.sort_by(|a, b| b.min.y.total_cmp(&a.min.y));
    found.first().copied()
}

/// A window on the sample at the named fixture clock, in the Week view and ready to be clicked.
fn local_window() -> Window {
    let mut w = window_on(mailcraft_engine::sample::mailbox(fixture_clock(), 0), "view.calendar");
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
    w
}

/// A window on the sample whose `Lunch with Dev` an invented account's server keeps: a remote id,
/// an organizer and one person invited, through the model's own fields. No server is reached.
fn kept_lunch() -> (Window, Event) {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(fixture_clock(), 0), fixture_clock(), 0);
    let added = session
        .run("account.add", &json!({ "email": "zoe@example.org", "name": "Zoe", "incoming": { "host": "imap.example.org" } }))
        .expect("an account is added");
    let account = added["id"].as_u64().expect("the account's id");
    let id = session.mailbox.events.values().find(|event| event.title == "Lunch with Dev").map(|event| event.id).expect("the sample has it");
    let lunch = {
        let event = session.mailbox.event_mut(id).expect("the event is there");
        event.origin = Origin { account, id: format!("server-{id}"), ..Origin::default() };
        event.organizer = Some(Address::new("Dev Example", "dev@example.org"));
        event.attendees = vec![Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Attendee::default() }];
        event.clone()
    };
    let mut w = window_on(session.mailbox, "view.calendar");
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
    (w, lunch)
}

#[test]
fn programmatic_paste_refuses_a_single_local_cut_with_an_unsaved_form_and_keeps_everything() {
    let mut w = local_window();
    let lunch = event(&w, "Lunch with Dev");
    assert!(lunch.recurrence.is_none(), "fixture: one whole appointment, not a series");
    assert!(lunch.origin.is_local(), "fixture: it lives in this mailbox only");
    assert!(lunch.organizer.is_none() && lunch.attendees.is_empty(), "fixture: it tells nobody");
    select(&mut w, &[(lunch.id, None)]);
    let opening = w.state().session.openings();

    let cut_answer = w.state_mut().execute("ui.calendarCut", json!({})).expect("ui.calendarCut is a Calendar command");
    assert_eq!(cut_answer, json!({ "items": 1, "cut": true }), "Cut takes the one appointment and sorts nothing else");
    w.run_steps(2);
    let cut = clipboard(&w).expect("Cut puts the appointment on the calendar's clipboard");
    assert!(cut.cut, "the clipboard holds a cut");
    assert_eq!(cut.items.len(), 1, "one appointment is on it");
    assert_eq!(cut.items[0].id, lunch.id, "and it is the one that was selected");
    assert_eq!(cut.opening, opening, "from this mailbox");
    assert_eq!(cut.sources.len(), 1, "with the source pointer it captured");
    let source = w.state().session.mailbox.events.get(&lunch.id).cloned().expect("the appointment is still there");
    assert!(Arc::ptr_eq(&cut.sources[0], &source), "the cut is of that appointment itself");

    // The destination: a real click on empty hours, read back immediately as the grid stored it.
    let somewhere = after_noon(&w, 3.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    let slot = picked(&w);
    assert!(!slot.all_day && slot.text.is_none(), "fixture: empty hours were picked");
    assert_eq!(slot.start, at(date(10, 14), 15, 0), "fixture: Wednesday's 15:00");

    // The owner's own appointment window, with a proposed time in its fields and not saved.
    let opened = w.state_mut().execute("ui.openWindow", json!({ "event": lunch.id })).expect("the appointment opens in a window");
    assert_eq!(opened["event"], json!(lunch.id), "fixture: the window is about the appointment that was cut");
    let form_window = opened["window"].as_u64().expect("a window number");
    {
        let form = w.state_mut().windows.form_mut(form_window).and_then(Form::event_mut).expect("the window shows the appointment's form");
        form.take_proposal(at(date(11, 6), 10, 0), at(date(11, 6), 11, 0), false);
    }
    assert_eq!(w.state().windows.unsaved(), 1, "fixture: the appointment's window has an unsaved change");
    let form_before = w.state_mut().windows.form_mut(form_window).and_then(Form::event_mut).map(|form| form.clone()).expect("the form is still open");
    assert_eq!((form_before.title(), form_before.editing()), ("Appointment".to_string(), Some(lunch.id)), "fixture: the form is the appointment's");

    let before = w.state().session.mailbox.clone();
    let selection = selected(&w);
    let undo = undo_state(&w);
    assert_eq!(picked(&w).start, slot.start, "fixture: the pick survived opening the form");
    assert!(cut.is(&clipboard(&w).expect("the cut survived opening the form")), "fixture: the cut is unchanged");

    // The canonical single-item move tells nobody and the window's helper does not see its
    // unsaved source; the cut-paste payload does. That is the guard seam this test pins.
    let legacy = json!({ "id": lunch.id, "start": minute_iso(slot.start) });
    assert_eq!(mail_of(&w.state().session, "calendar.move", &legacy), None, "fixture: moving this local appointment writes to nobody");
    assert!(
        crate::windows::removing_unsaved(w.state(), "calendar.move", &legacy).is_none(),
        "fixture: the canonical legacy move does not detect the unsaved source"
    );
    let lost = crate::windows::removing_unsaved(w.state(), "calendar.paste", &json!({ "items": [{ "id": lunch.id }], "cut": true }))
        .expect("the cut-paste reader sees the window whose unsaved change would be taken away");
    assert_eq!(lost["title"], json!("Lunch with Dev"), "and names it");

    // The command under test: no parameters, no dialog, no queued question.
    let why = w
        .state_mut()
        .execute("ui.calendarPaste", json!({}))
        .expect_err("the programmatic Paste must refuse a cut that would close an unsaved appointment window");
    assert!(!why.contains("unknown command"), "ui.calendarPaste must be the Calendar's own command: {why}");
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "the refusal opens no question");

    assert!(w.state().session.mailbox == before, "no event moved, none was copied, none was dropped");
    assert_eq!(w.state().session.mailbox.event_changes, before.event_changes, "and no server change was queued");
    assert_eq!(selected(&w), selection, "the selection is as the refusal found it");
    assert_eq!(w.state().session.openings(), opening, "and no other mailbox was opened");
    assert_eq!(undo_state(&w), undo, "and no undo step was added or redo step moved");
    assert_eq!(starts_at(&w, lunch.id), Some(lunch.start), "the cut appointment is still where it was");

    let kept = clipboard(&w).expect("a refused paste keeps the cut, to be pasted again");
    assert!(cut.is(&kept), "the very cut it found is still there");
    assert_eq!(kept.items, cut.items, "with the same items, in the same order");
    assert_eq!((kept.cut, &kept.text, kept.opening), (cut.cut, &cut.text, cut.opening), "and the same cut flag, clipboard text and mailbox");
    assert!(kept.sources.iter().zip(&cut.sources).all(|(now, was)| Arc::ptr_eq(now, was)), "of the very events it captured");

    assert_eq!(w.state().windows.unsaved(), 1, "the owner's form still has its unsaved change");
    assert_eq!(w.state().windows.of_event(lunch.id), Some(form_window), "in the same open window");
    let form_after = w.state_mut().windows.form_mut(form_window).and_then(Form::event_mut).map(|form| form.clone()).expect("the form is still open");
    assert_eq!(form_after, form_before, "and the exact form, its fields and its unsaved change, as the refusal found it");
    assert_eq!(picked(&w).start, slot.start, "and the picked time is still there");
}

#[test]
fn owner_paste_asks_and_cancel_then_approve_reaches_the_real_buttons_and_consumes_the_cut() {
    let (mut w, lunch) = kept_lunch();
    assert!(lunch.recurrence.is_none(), "fixture: one whole meeting, not a series");
    assert!(!lunch.origin.guest, "fixture: the owner's own meeting");
    assert_eq!(lunch.origin.id, format!("server-{}", lunch.id), "fixture: kept by the account under its own remote id");
    assert!(lunch.organizer.is_some() && !lunch.attendees.is_empty(), "fixture: people the server would tell");

    // A click first, then the one meeting as a typed set.
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    w.state_mut().execute("calendar.select", json!({ "items": [{ "id": lunch.id }] })).expect("the one meeting is selected as a typed item");
    w.run_steps(2);
    assert_eq!(selected(&w), [(lunch.id, None)], "fixture: a typed set of one whole meeting");

    let source = w.state().session.mailbox.events.get(&lunch.id).cloned().expect("the meeting is there before it is cut");
    let before = w.state().session.mailbox.clone();
    let opening = w.state().session.openings();
    let undo = undo_state(&w);
    let queued_before = before.event_changes.iter().filter(|change| change.event == lunch.id).count();

    // Ctrl+X: the meeting goes on the clipboard as a cut and stays where it is.
    chord(&mut w, COMMAND, egui::Key::X);
    let cut = clipboard(&w).expect("Cut puts the one meeting on the calendar's clipboard");
    assert!(cut.cut, "the clipboard holds a cut");
    assert_eq!(cut.items.len(), 1, "one meeting is on it");
    assert_eq!(cut.items[0].id, lunch.id, "and it is that meeting");
    assert_eq!(cut.opening, opening, "from this mailbox");
    assert_eq!(cut.sources.len(), 1, "with the source pointer it captured");
    assert!(Arc::ptr_eq(&cut.sources[0], &source), "the cut is of the meeting that was there");
    assert_eq!(starts_at(&w, lunch.id), Some(lunch.start), "a cut stays where it is until pasted");

    // The destination, picked by the real pointer and checked at once.
    let somewhere = after_noon(&w, 3.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    let slot = picked(&w);
    assert!(!slot.all_day && slot.text.is_none(), "fixture: empty hours were picked");
    assert_eq!(slot.start, at(date(10, 14), 15, 0), "fixture: Wednesday's 15:00");
    assert!(cut.is(&clipboard(&w).expect("the cut survived the pick")), "fixture: the same cut is still on the clipboard");
    assert_eq!(
        mail_of(&w.state().session, "calendar.move", &json!({ "id": lunch.id, "start": minute_iso(slot.start) })),
        Some(Mail::Update(1)),
        "fixture: moving this meeting writes to the one person invited, so the owner's yes is required"
    );

    // Flags a question's yes would add do not move it by themselves: only `{}` is a Paste, and a
    // bare `confirmed` on the engine's batch paste is still unsent.
    for flags in [json!({ "confirmed": true }), json!({ "send": true })] {
        let why = w
            .state_mut()
            .execute("ui.calendarPaste", flags.clone())
            .expect_err("the programmatic Paste takes no parameters, so no caller can forge the owner's yes");
        assert!(why.contains("parameters"), "the refusal is Paste's own parameter refusal: {why}");
        let why = w.state_mut().run("ui.calendarPaste", flags).expect_err("the owner route also refuses unsupported parameters");
        assert!(why.contains("parameters"), "the owner route validates the original parameters: {why}");
    }
    let direct = json!({ "items": [{ "id": lunch.id }], "start": minute_iso(slot.start), "cut": true });
    assert!(w.state_mut().execute("calendar.paste", direct).is_err(), "the batch paste without the owner's send is refused, not used as a shortcut");
    assert!(w.state().session.mailbox == before, "none of those attempts moved or queued anything");
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "and none of them put up a question");
    assert!(cut.is(&clipboard(&w).expect("the cut survived the attempts")), "nor touched the cut");
    assert_eq!(picked(&w).start, slot.start, "fixture: the pick is still where the pointer put it");

    // The owner's own Paste: the real confirmation, pending, with the cut kept for its answer.
    let said = w.state_mut().run("ui.calendarPaste", json!({})).expect("the owner's Paste is a Calendar command");
    assert_eq!(said, json!({ "pending": "confirm" }), "the owner's Paste waits for the question instead of moving anything");
    w.run_steps(3);
    let (command, action, cancel, text) = open_question(&w, "a cut meeting that writes to people must be asked about before it moves");
    assert_eq!(command, "calendar.move", "the one-meeting paste asks the legacy move question");
    assert!(text.contains("person") || text.contains("people"), "the question says who would be told: {text}");
    assert_eq!(dialog(&w), Some("confirm"), "the question is the window's own confirmation");
    assert!(w.state().session.mailbox == before, "nothing moves while the question waits");
    assert_eq!(w.state().session.openings(), opening, "no other mailbox was opened");
    assert_eq!(undo_state(&w), undo, "and no undo step was added");
    let waiting = clipboard(&w).expect("the cut stays on the clipboard while its question waits");
    assert!(cut.is(&waiting), "the exact cut is kept with the question");

    // Cancel, through the question's own drawn button.
    let nowhere = button_rect(&w, &cancel).unwrap_or_else(|| panic!("the question has a button that says {cancel:?}"));
    click(&mut w, nowhere.center(), Modifiers::NONE, PointerButton::Primary);
    w.run_steps(3);
    assert_eq!(dialog(&w), None, "Cancel closes the question");
    assert!(w.state().session.mailbox == before, "Cancel moves nothing and queues nothing");
    assert!(cut.is(&clipboard(&w).expect("a cancelled cut stays on the clipboard")), "the exact cut is preserved for another Paste");

    // A fresh pointer pick (never a selection restored by frame steps), checked at once, then the
    // owner asks again and this time approves through the question's own approve button.
    let again = after_noon(&w, 3.0);
    click(&mut w, again, Modifiers::NONE, PointerButton::Primary);
    assert_eq!(picked(&w).start, slot.start, "fixture: the re-pick put the destination back");
    let said = w.state_mut().run("ui.calendarPaste", json!({})).expect("the owner's Paste is a Calendar command");
    assert_eq!(said, json!({ "pending": "confirm" }), "the second Paste asks again, with the cut still on the clipboard");
    w.run_steps(3);
    let (again_command, again_action, _, _) = open_question(&w, "the cut is still there, so the owner's Paste asks again");
    assert_eq!(again_command, command, "the same kind of question");
    assert_eq!(again_action, action, "the same approve action is offered again");
    assert!(cut.is(&clipboard(&w).expect("the cut is still on the clipboard before the yes")), "and the same exact cut");

    let go = button_rect(&w, &again_action).unwrap_or_else(|| panic!("the question goes ahead with a button that says {again_action:?}"));
    click(&mut w, go.center(), Modifiers::NONE, PointerButton::Primary);
    w.run_steps(3);
    assert_eq!(dialog(&w), None, "the yes closes the question");

    // The meeting itself moved, keeping its identity, people and length; one queued update was
    // sent because this is the yes that was pressed.
    let picked_time = at(date(10, 14), 15, 0);
    let expected = Event {
        start: picked_time,
        end: picked_time.add_minutes(lunch.duration_minutes()),
        start_repeated: false,
        end_repeated: false,
        ..lunch.clone()
    };
    let moved = w.state().session.mailbox.event(lunch.id).cloned().expect("the meeting that was cut is the meeting that moved");
    assert_eq!(moved, expected, "the same UID, title, origin, organizer, people and calendar, at the picked time");
    assert_eq!(moved.attendees, lunch.attendees, "the people the server was told about are the ones invited");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len(), "a cut moves it: none is copied or dropped");
    assert_eq!(w.state().session.openings(), opening, "and no other mailbox was opened");
    let queued_after = w.state().session.mailbox.event_changes.iter().filter(|change| change.event == lunch.id).count();
    assert_eq!(queued_after, queued_before.saturating_add(1), "the approved move queues one change for the meeting");
    assert!(
        w.state().session.mailbox.event_changes.iter().any(|change| change.event == lunch.id && change.send),
        "and it is queued as the owner's yes, to be sent"
    );

    // The approved paste is the only thing that consumed the cut.
    assert!(clipboard(&w).is_none(), "the yes that moved the meeting takes its cut off the calendar's clipboard");
    let moved_to = starts_at(&w, lunch.id);
    assert!(w.state_mut().run("ui.calendarPaste", json!({})).is_err(), "a spent cut has nothing to paste, and the owner's Paste says so");
    assert_eq!(starts_at(&w, lunch.id), moved_to, "the spent cut is not pasted again");
    assert!(clipboard(&w).is_none(), "and no clipboard came back");

    // One step back: the meeting and its queued update are as they were.
    assert!(w.state_mut().session.undo(), "a cut and its approved paste is one step");
    w.run_steps(2);
    assert_eq!(w.state().session.mailbox.event(lunch.id), Some(&lunch), "one step puts the meeting back, UID and all");
    assert_eq!(w.state().session.mailbox.event_changes, before.event_changes, "and takes back the queued update, so nothing is sent");
}
