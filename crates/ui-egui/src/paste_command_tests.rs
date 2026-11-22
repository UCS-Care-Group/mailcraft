// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Calendar's Paste as a command that cannot ask: `ui.calendarPaste` with an empty object
//! moves the cut that `ui.calendarCut` put on the calendar's clipboard to the time a real pointer
//! picked, answering `{"items": 2, "cut": true}` in the same call — no dialog, no queued
//! acknowledgement. Both items keep their identity, metadata, lengths and their distance from one
//! another; nothing is duplicated; the cut is spent; a second Paste is an explicit `Err` that
//! changes nothing; and one Undo restores both events and their queued changes, in the same
//! mailbox opening.
//!
//! One module of `calendar_selection_tests`, which is itself a child of `calendar_view`, so
//! `super::*` is that file's fixtures (`calendar`, `event`, `select`, `selected`, `titled`,
//! `click`, `after_noon`, `date`, `at`, `Event`, `Id`, `DateTime`, `Window`, `Modifiers`,
//! `PointerButton`), and `super::super` is `calendar_view`'s own private `Clip`, `CLIP_ID`, `Slot`
//! and `load_slot` — read from the module that defines them, as `calendar_review_regression_tests`
//! and `calendar_late_approval_tests` do, rather than assumed through `super::*` (no ancestor
//! between here and `calendar_view` imports or redefines either name). No private helper's
//! visibility is widened and nothing here is a production hook.

use serde_json::json;

use super::super::{CLIP_ID, Clip, Slot, load_slot};
use super::*;

/// The `Clip` the window keeps on the calendar's clipboard, read where `calendar_view` put it:
/// the items, the cut flag, the text, the mailbox it was taken from and its source pointers.
fn clipboard(w: &Window) -> Option<Clip> {
    w.ctx.data(|data| data.get_temp::<Clip>(egui::Id::unique(CLIP_ID)))
}

/// The time the pointer picked, as the grid stored the pick: the destination this Paste must obey
/// rather than any start a caller could name. Its own calendar is the block's front one.
fn picked(w: &Window) -> Slot {
    load_slot(&w.ctx).expect("the click picked a time in the calendar")
}

/// Where `id` starts now, or `None` when it is no longer there.
fn starts_at(w: &Window, id: Id) -> Option<DateTime> {
    w.state().session.mailbox.event(id).map(|event| event.start)
}

/// The Calendar's Paste as the programmatic route runs it: after a real Cut of two local items,
/// one `execute` with an empty object puts both where the pointer picked, answers synchronously
/// how many moved and that this was a cut, invents no question, spends the cut, refuses a second
/// Paste explicitly without changing anything, and leaves one Undo that restores both events and
/// the queued changes, in the same mailbox opening.
#[test]
fn ui_calendar_paste_moves_the_cut_set_to_the_picked_time_and_one_undo_puts_it_back() {
    let mut w = calendar();
    let (lunch_id, dentist_id) = (event(&w, "Lunch with Dev").id, event(&w, "Dentist").id);
    // UIDs of their own, through the model's own field, so "the UID came along" is a real check
    // and not two empty strings. Neither repeats, both are this computer's own, and neither tells
    // anybody — so the Paste is a plain local move and no question is invented.
    for (id, uid) in [(lunch_id, "lunch-0001@example.org"), (dentist_id, "dentist-0002@example.org")] {
        w.state_mut().session.mailbox.event_mut(id).expect("the sample has the item").uid = uid.to_string();
    }
    w.run_steps(2);
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    assert_ne!(lunch.id, dentist.id, "the fixture's two items are distinct");
    for item in [&lunch, &dentist] {
        assert!(item.recurrence.is_none(), "\"{}\" is one whole item, not a series", item.title);
        assert!(item.origin.is_local(), "\"{}\" lives in this mailbox only", item.title);
        assert!(!item.origin.guest, "\"{}\" is the owner's own", item.title);
        assert!(item.organizer.is_none() && item.attendees.is_empty(), "\"{}\" tells nobody", item.title);
    }
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "the typed set is selected");

    let before = w.state().session.mailbox.clone();
    let opening = w.state().session.openings();

    // The Cut this Paste is the other half of: empty parameters, and an answer that names only the
    // count and that it was a cut.
    let cut_answer = w.state_mut().execute("ui.calendarCut", json!({})).expect("ui.calendarCut is a Calendar command");
    assert_eq!(cut_answer, json!({ "items": 2, "cut": true }), "Cut takes the set and answers only how many, and that it was a cut");
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "a plain local cut invents no question");
    w.run_steps(2);

    // What it left on the calendar's clipboard: a cut of both members, in selection order, from
    // this mailbox — and nothing moved yet.
    let cut = clipboard(&w).expect("Cut puts the set on the calendar's clipboard");
    assert!(cut.cut, "the clipboard holds a cut, not a copy");
    assert_eq!(cut.items.iter().map(|item| item.id).collect::<Vec<_>>(), vec![lunch.id, dentist.id], "both members, in the order selected");
    assert_eq!(cut.sources.len(), 2, "with the source pointers it captured");
    assert_eq!(cut.opening, opening, "of the mailbox it was taken from");
    assert_eq!((starts_at(&w, lunch.id), starts_at(&w, dentist.id)), (Some(lunch.start), Some(dentist.start)), "a cut leaves its sources in place");

    // The destination: a real pointer click on empty hours of the week in view, read back as the
    // grid stored the pick — never a start this test names or a clipboard it injects itself.
    let somewhere = after_noon(&w, 3.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    let slot = picked(&w);
    assert!(!slot.all_day && slot.text.is_none(), "the click picked empty hours, not a whole day or a field being typed into");
    let picked_time = slot.start;
    assert_eq!(picked_time, at(date(10, 14), 15, 0), "fixture: the click picked Wednesday's 15:00, as `after_noon(3.0)` means");

    // The command under test: empty parameters, one synchronous answer, no dialog and no queued
    // acknowledgement.
    let answer = w.state_mut().execute("ui.calendarPaste", json!({})).expect("ui.calendarPaste is a Calendar command");
    assert_eq!(answer, json!({ "items": 2, "cut": true }), "Paste moves the cut set and answers only how many, and that it was a cut");
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "a plain local cut pasted by a command asks nothing");
    w.run_steps(2);

    // Both events themselves, at the picked time: the earliest timed source is the anchor, and the
    // other keeps its own length and the wall-clock minutes between it and the anchor.
    let (anchor, other) = if (lunch.start, lunch.end, lunch.id) <= (dentist.start, dentist.end, dentist.id) {
        (lunch.as_ref(), dentist.as_ref())
    } else {
        (dentist.as_ref(), lunch.as_ref())
    };
    let expected_anchor = Event { start: picked_time, end: picked_time.add_minutes(anchor.start.minutes_until(anchor.end)), ..(*anchor).clone() };
    let other_start = picked_time.add_minutes(anchor.start.minutes_until(other.start));
    let expected_other = Event { start: other_start, end: other_start.add_minutes(other.start.minutes_until(other.end)), ..(*other).clone() };
    let moved_anchor = w.state().session.mailbox.event(anchor.id).cloned().expect("the anchored member is the event that moved");
    let moved_other = w.state().session.mailbox.event(other.id).cloned().expect("the other member is the event that moved");
    assert_eq!(
        moved_anchor, expected_anchor,
        "the anchored member keeps its id, UID, title, origin, organizer, people, calendar and length, at the picked time"
    );
    assert_eq!(moved_other, expected_other, "the other keeps its own length and the time between it and the anchor");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len(), "a cut moves them: no source is duplicated and none is dropped");
    assert_eq!((titled(&w, "Lunch with Dev").len(), titled(&w, "Dentist").len()), (1, 1), "one of each, still");
    assert_eq!(w.state().session.openings(), opening, "the paste moved items within the mailbox they were cut from");

    // The successful move is what used the cut up.
    assert!(clipboard(&w).is_none(), "the paste that moved them takes the cut off the calendar's clipboard");

    // A second Paste has nothing to paste: an explicit refusal, and nothing of the mailbox, the
    // selection or the clipboard changes.
    let moved_to = (starts_at(&w, lunch.id), starts_at(&w, dentist.id));
    let kept = selected(&w);
    assert!(
        w.state_mut().execute("ui.calendarPaste", json!({})).is_err(),
        "a second Paste has nothing left to paste, and says so rather than doing nothing silently"
    );
    w.run_steps(2);
    assert_eq!((starts_at(&w, lunch.id), starts_at(&w, dentist.id)), moved_to, "the spent cut is not pasted again");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len(), "and nothing was added or taken away");
    assert_eq!(selected(&w), kept, "and the selection the first Paste left is unchanged");
    assert!(clipboard(&w).is_none(), "and no clipboard came back");
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "the refusal asks nothing");

    // One step back: both where they were, and the queued changes as they were, in the same
    // mailbox opening. (These two items are local, so nothing is queued either way; the
    // server-queue path is `unchanged_single_cut_tests`'.)
    assert!(w.state_mut().session.undo(), "a cut and paste of a set is one step");
    w.run_steps(2);
    assert_eq!(w.state().session.mailbox.event(lunch.id), Some(lunch.as_ref()), "one step puts the first member back as it was");
    assert_eq!(w.state().session.mailbox.event(dentist.id), Some(dentist.as_ref()), "and the second");
    assert_eq!(w.state().session.mailbox.event_changes, before.event_changes, "and takes back exactly the queued changes it made");
    assert_eq!(w.state().session.openings(), opening, "no other mailbox was opened");
}
