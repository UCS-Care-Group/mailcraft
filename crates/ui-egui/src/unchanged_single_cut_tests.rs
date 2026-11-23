// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The other half of `legacy_single_stale_tests`: a cut of one whole meeting that an account's
//! server keeps, pasted with nothing replacing it, must go through when the owner answers the
//! legacy `calendar.move` question. This is the positive control that keeps a stale-source
//! refusal from being "fixed" by refusing every unchanged legacy confirmation, and it also holds
//! the cut receipt: the yes that moved the meeting uses up the cut, and one Undo puts both the
//! meeting and its queued server change back.
//!
//! One module of `calendar_late_approval_tests`, which is itself a child of
//! `calendar_selection_tests`, so `super::*` is that file's private fixtures (`kept_window`,
//! `clipboard`, `open_question`,
//! `button_rect`, `starts_at`) and `super::super::*` is `calendar_selection_tests`'s (`click`,
//! `chord`, `selected`, `drawn`, `after_noon`, `date`, `at`, `COMMAND`). Nothing here is
//! production code or a test hook: the window is the real one (`people_view::testing::window_on`
//! inside `kept_window`), its events are synthetic (`example.org`, `Mailbox::add_event`), the
//! clock is fixed, and every act is a pointer event, a key or the question's own button. No
//! file, server, real account, native window or wall clock is used.
//!
//! WHAT IS HELD (source read 2026-11-15): with no replacement under the cut meeting,
//! the legacy `calendar.move` question names the event that is still there, so the answer must
//! move that event itself — same UID, origin, organizer, people, title and calendar — to the
//! picked time, must take the cut off the calendar's clipboard, must queue the one update the
//! account's server is told about, and must be one Undo step. This is the positive control: the
//! unchanged case must go through, before and after a correct legacy binding.

use std::sync::Arc;

use egui::{Modifiers, PointerButton};
use mailcraft_model::Event;
use serde_json::json;

use super::super::{COMMAND, after_noon, at, chord, click, date, drawn, selected};
use super::{button_rect, clipboard, kept_window, open_question, starts_at};

#[test]
fn an_unchanged_legacy_move_of_one_cut_meeting_is_approved_and_moves_that_event() {
    // One whole, non-recurring meeting the owner keeps: an attendee makes the move a message the
    // account's server would send, which is what makes the paste ask at all.
    let (mut w, lunch, _other) = kept_window();
    assert!(lunch.recurrence.is_none(), "fixture: one whole, non-recurring meeting");
    assert!(!lunch.origin.guest, "fixture: the owner's own meeting, not one they were invited to");
    assert!(lunch.organizer.is_some(), "fixture: the owner organizes it");
    assert!(!lunch.attendees.is_empty(), "fixture: somebody invited, so a move writes to them");

    // A click first: the calendar is what the keys are for.
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    w.state_mut().execute("calendar.select", json!({ "items": [{ "id": lunch.id }] })).expect("the one meeting is selected as a typed item");
    w.run_steps(2);
    assert_eq!(selected(&w), [(lunch.id, None)], "fixture: a typed set of one whole meeting");

    let source = w.state().session.mailbox.events.get(&lunch.id).cloned().expect("the meeting is there before it is cut");
    let before = w.state().session.mailbox.clone();
    let opening = w.state().session.openings();
    let queued_before = before.event_changes.iter().filter(|change| change.event == lunch.id).count();

    // Ctrl+X: the meeting goes on the clipboard as a cut, and stays where it is.
    chord(&mut w, COMMAND, egui::Key::X);
    let cut = clipboard(&w).expect("Cut puts the one meeting on the calendar's clipboard");
    assert!(cut.cut, "the clipboard holds a cut, not a copy");
    assert_eq!(cut.items.len(), 1, "the one selected meeting is on it");
    assert_eq!(cut.items[0].id, lunch.id, "and it is that meeting");
    assert_eq!(cut.sources.len(), 1, "with the source pointer it captured");
    assert!(Arc::ptr_eq(&cut.sources[0], &source), "the cut is of the meeting that was there, not a later replacement under its id");
    assert_eq!(starts_at(&w, lunch.id), Some(lunch.start), "a cut stays where it is until it is pasted");

    // A time picked, and Paste: the legacy single-item move question, with nothing moved yet.
    let picked_time = at(date(10, 14), 15, 0);
    let picked = after_noon(&w, 3.0);
    click(&mut w, picked, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    let (command, action, _cancel, _text) = open_question(&w, "a cut of one kept meeting must be asked about before it moves");
    assert_eq!(command, "calendar.move", "the legacy single-item move question was asked");
    assert_ne!(command, "calendar.paste", "a whole event is not the batch paste path");
    assert!(w.state().session.mailbox == before, "nothing moves while the question waits: no member is moved and none is dropped");
    assert_eq!(w.state().session.openings(), opening, "the question did not open another mailbox");
    assert!(clipboard(&w).is_some(), "the cut stays on the clipboard while its question waits");

    // The owner presses the question's own button — not `go_ahead` directly, which would bypass
    // the renderer, the binding check and the cut receipt.
    let go = button_rect(&w, &action).unwrap_or_else(|| panic!("the question goes ahead with a button that says {action:?}"));
    click(&mut w, go.center(), Modifiers::NONE, PointerButton::Primary);
    w.run_steps(3);
    assert!(w.state().dialog.is_none(), "the answer closes the question");

    // The meeting itself moved: the same id, UID, origin, organizer, people, title and calendar,
    // at the time that was picked, and with the length it had.
    let moved = w.state().session.mailbox.event(lunch.id).cloned().expect("the meeting that was cut is the meeting that moved");
    let expected = Event {
        start: picked_time,
        end: picked_time.add_minutes(lunch.duration_minutes()),
        start_repeated: false,
        end_repeated: false,
        ..lunch.clone()
    };
    assert_eq!(moved, expected, "the yes moves the meeting that was cut: the same UID and metadata, at the picked time");
    assert_eq!(
        (moved.uid.as_str(), moved.title.as_str()),
        (lunch.uid.as_str(), lunch.title.as_str()),
        "the UID and title are the ones the question showed"
    );
    assert_eq!(
        (moved.origin.clone(), moved.organizer.clone()),
        (lunch.origin.clone(), lunch.organizer.clone()),
        "and it is still the same server's meeting, organized by the same person"
    );
    assert_eq!(moved.attendees, lunch.attendees, "the people invited are the people the server would write to");
    assert_eq!(moved.calendar, lunch.calendar, "and it stayed in its calendar");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len(), "a cut moves the meeting: none is copied and none is dropped");
    assert_eq!(w.state().session.openings(), opening, "and it opened no other mailbox");

    // What the account's server is told: one queued change for this meeting, with the owner's yes.
    let queued_after = w.state().session.mailbox.event_changes.iter().filter(|change| change.event == lunch.id).count();
    assert_eq!(queued_after, queued_before.saturating_add(1), "the move of a meeting an account's server keeps queues one change for it");
    assert!(
        w.state().session.mailbox.event_changes.iter().any(|change| change.event == lunch.id && change.send),
        "the yes that was pressed is the yes the server is told about"
    );

    // The confirmed paste is the paste that used up the cut.
    assert!(clipboard(&w).is_none(), "the yes that moved the meeting must take its cut off the calendar's clipboard");
    let moved_to = starts_at(&w, lunch.id);
    let later = after_noon(&w, 5.0);
    click(&mut w, later, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!(starts_at(&w, lunch.id), moved_to, "the spent cut is not pasted again: the meeting stays where the confirmed paste put it");

    // One step back puts the meeting where it was, and takes the queued change back with it.
    assert!(w.state_mut().session.undo(), "a cut and paste of one meeting is one step");
    w.run_steps(2);
    assert_eq!(w.state().session.mailbox.event(lunch.id), Some(&lunch), "one step puts the meeting back as it was, UID and all");
    assert_eq!(w.state().session.mailbox.event_changes, before.event_changes, "and takes back the queued update, so nothing is sent");
}
