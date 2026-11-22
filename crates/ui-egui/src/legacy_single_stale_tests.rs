// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A cut of one whole meeting that an account's server keeps, pasted while a Send/Receive replaces
//! that meeting under the same id: the legacy `calendar.move` question that the paste asks must not
//! move the replacement when the owner says yes.
//!
//! One module of `calendar_late_approval_tests`, which is itself a child of
//! `calendar_selection_tests`, so `super::*` is that file's private fixtures (`kept_window`,
//! `clipboard`, `starts_at`,
//! `open_question`, `button_rect`) and `super::super::*` is `calendar_selection_tests`'s
//! (`click`, `chord`, `selected`, `drawn`, `after_noon`, `COMMAND`). Nothing here is production
//! code or a test hook: the window is the real one, the events are synthetic (`example.org`,
//! `Mailbox::add_event`), the clock is fixed, and every act is a pointer event, a key or the
//! question's own button. No file, server, real account, native window or wall clock is used.
//!
//! WHAT IS HELD: a cut of one whole event goes by `paste_single` to the legacy `calendar.move`.
//! When this test was written (source read 2026-11-15) that question was left unbound
//! (`names_a_set` is false for the legacy single `id`), so a Send/Receive that replaced the event
//! (same id, new shared pointer, another UID and time) while the question was open had the yes
//! move the replacement. The question is now held by the one-event binding; this test exercises
//! that check with a replacement event under the same id. The required outcome: the yes is refused,
//! the replacement must not move, no change may be queued, and the cut must stay on the
//! clipboard.

use std::sync::Arc;

use egui::{Modifiers, PointerButton};
use mailcraft_model::Event;
use serde_json::json;

use super::super::{COMMAND, after_noon, chord, click, drawn, selected};
use super::{button_rect, clipboard, kept_window, open_question, starts_at};

#[test]
fn a_stale_legacy_move_question_refuses_the_event_that_replaced_its_source() {
    // One whole, non-recurring meeting the owner keeps: an attendee makes the move a message the
    // account's server would send, which is what makes the paste ask at all.
    let (mut w, lunch, _other) = kept_window();
    assert!(lunch.recurrence.is_none(), "fixture: one whole, non-recurring meeting");
    assert!(!lunch.origin.guest, "fixture: the owner's own meeting, not one they were invited to");
    assert!(!lunch.attendees.is_empty(), "fixture: somebody invited, so a move writes to them");

    // A click first: the calendar is what the keys are for.
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    w.state_mut().execute("calendar.select", json!({ "items": [{ "id": lunch.id }] })).expect("the one meeting is selected as a typed item");
    w.run_steps(2);
    assert_eq!(selected(&w), [(lunch.id, None)], "fixture: a typed set of one whole meeting");

    // Ctrl+X: the meeting goes on the clipboard as a cut and stays where it is.
    let opening = w.state().session.openings();
    chord(&mut w, COMMAND, egui::Key::X);
    let cut = clipboard(&w).expect("Cut puts the one meeting on the calendar's clipboard");
    assert!(cut.cut, "the clipboard holds a cut, not a copy");
    assert_eq!(cut.items.len(), 1, "the one selected meeting is on it");
    assert_eq!(cut.items[0].id, lunch.id, "and it is that meeting");
    assert_eq!(starts_at(&w, lunch.id), Some(lunch.start), "a cut stays where it is until it is pasted");

    // A time picked, and Paste: the legacy single-item move question, with nothing moved yet.
    let before = w.state().session.mailbox.clone();
    let picked = after_noon(&w, 3.0);
    click(&mut w, picked, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    let (command, action, _cancel, _text) = open_question(&w, "a cut of one kept meeting must be asked about before it moves");
    assert_eq!(command, "calendar.move", "the legacy single-item move question was asked");
    assert_ne!(command, "calendar.paste", "a whole event is not the batch paste path");
    assert!(w.state().session.mailbox == before, "nothing moves while the question waits");
    assert_eq!(w.state().session.openings(), opening, "the question did not open another mailbox");
    assert!(clipboard(&w).is_some(), "the cut stays on the clipboard while its question waits");

    // What a Send/Receive can bring while the question is open: the same id and title, another
    // UID and time. `Mailbox::add_event` keeps a named id when it is free, so removing the old
    // record first is the supported way to replace it under that id; no allocator is assumed.
    let old = w.state().session.mailbox.events.get(&lunch.id).cloned().expect("the cut meeting is still there");
    let replacement = Event {
        uid: "stale-source-replacement@example.org".into(),
        start: lunch.start.add_minutes(90),
        end: lunch.end.add_minutes(90),
        ..lunch.clone()
    };
    assert_eq!((replacement.id, replacement.title.as_str()), (lunch.id, lunch.title.as_str()), "fixture: same id and title");
    assert_ne!(replacement.uid, lunch.uid, "fixture: a different UID");
    assert_ne!(replacement.start, lunch.start, "fixture: a different time");
    w.state_mut().session.mailbox.remove_event(lunch.id).expect("the old record is taken out");
    let kept_id = w.state_mut().session.mailbox.add_event(replacement.clone()).expect("the replacement is added");
    assert_eq!(kept_id, lunch.id, "the replacement keeps the id the question names");
    let now = w.state().session.mailbox.events.get(&lunch.id).cloned().expect("the replacement is under the same id");
    assert!(!Arc::ptr_eq(&old, &now), "the source was replaced, not mutated through the clipboard's pointer");
    assert_eq!((now.uid.as_str(), now.start), (replacement.uid.as_str(), replacement.start), "the replacement is what it says");
    assert_eq!(w.state().session.openings(), opening, "replacing a record does not open another mailbox");

    // The same legacy question is still up, still naming the same id.
    let (still, action_again, _cancel, _text) = open_question(&w, "the question stays open while the source is replaced");
    assert_eq!((still.as_str(), action_again), ("calendar.move", action.clone()), "fixture: the same question is still up");
    let after_replacement = w.state().session.mailbox.clone();

    // The owner presses the question's own button — not `go_ahead` directly, which would bypass
    // the renderer, the binding check and the cut receipt.
    let go = button_rect(&w, &action).unwrap_or_else(|| panic!("the question goes ahead with a button that says {action:?}"));
    click(&mut w, go.center(), Modifiers::NONE, PointerButton::Primary);
    w.run_steps(3);
    assert!(w.state().dialog.is_none(), "the answer closes the question");

    // Refused: the replacement is exactly what came in, nothing moved or was queued, and the
    // clipboard still holds the cut that was asked about.
    assert_eq!(
        w.state().session.mailbox.event(lunch.id),
        Some(&replacement),
        "a yes to a question about the cut source must not move the event that replaced it"
    );
    assert_eq!(w.state().session.mailbox.events, after_replacement.events, "no event was moved, copied or removed");
    assert_eq!(w.state().session.mailbox.event_changes, after_replacement.event_changes, "the stale yes queued no change");
    assert_eq!(w.state().session.openings(), opening, "and it opened no other mailbox");

    let kept_cut = clipboard(&w).expect("a refused stale question leaves the cut on the clipboard");
    assert!(kept_cut.cut, "it is still the cut it was");
    assert_eq!(kept_cut.items.len(), 1, "for the one meeting that was cut");
    assert_eq!(kept_cut.items[0].id, lunch.id, "the same meeting");
    assert_eq!(kept_cut.sources.len(), 1, "with the source pointer it captured");
    assert!(Arc::ptr_eq(&kept_cut.sources[0], &old), "still the event that was cut, not its replacement");
}
