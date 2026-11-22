// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Calendar's Cut as a command: `ui.calendarCut` with an empty object takes the whole
//! selected set onto the calendar's clipboard as a cut, and the window's existing picked-time
//! Paste moves the members themselves — same ids, UIDs and metadata, none duplicated — and one
//! Undo puts both back where they were, with the queued changes as they were.
//!
//! One module of `calendar_selection_tests`, which is itself a child of `calendar_view`, so
//! `super::*` is that file's fixtures (`calendar`, `event`, `select`, `selected`, `titled`,
//! `click`, `chord`, `after_noon`, `COMMAND`, `date`, `at`, `Event`, `Id`, `DateTime`, `Window`,
//! `Modifiers`, `PointerButton`), and `super::super` is `calendar_view`'s own private `Clip` and
//! `CLIP_ID` — read from the module that defines them, as `calendar_late_approval_tests` does,
//! rather than assumed through `super::*` (no ancestor between here and `calendar_view` imports
//! or redefines either name). Nothing here is production code or a test hook: the window is the
//! real one, its two items are the sample's invented local appointments, the clock is fixed, and
//! every act is a command, a pointer event or a key the owner would press. No file, server, real
//! account, native window or wall clock is used.

use serde_json::json;

use super::super::{CLIP_ID, Clip};
use super::*;

/// The `Clip` the window keeps on the calendar's clipboard, read where `calendar_view` put it:
/// the items, the cut flag, the text, the mailbox it was taken from and its source pointers.
fn clipboard(w: &Window) -> Option<Clip> {
    w.ctx.data(|data| data.get_temp::<Clip>(egui::Id::unique(CLIP_ID)))
}

/// Where `id` starts now, or `None` when it is no longer there.
fn starts_at(w: &Window, id: Id) -> Option<DateTime> {
    w.state().session.mailbox.event(id).map(|event| event.start)
}

/// The Calendar's Cut: the whole typed set goes onto the calendar's clipboard as a cut, and the
/// window's own Paste at a picked time moves both members themselves — identity, people,
/// calendar, lengths and spacing kept, no copy left behind — using up the cut. One Undo puts both
/// back, with the queued changes.
#[test]
fn ui_calendar_cut_moves_the_whole_set_and_one_undo_puts_it_back() {
    let mut w = calendar();
    let (lunch_id, dentist_id) = (event(&w, "Lunch with Dev").id, event(&w, "Dentist").id);
    // UIDs of their own, through the model's own field, so "the UID came along" is a real check
    // and not two empty strings. Neither item repeats, and both are this computer's own, with
    // nobody to tell — so the paste is a plain local move and no question is invented.
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

    // The Calendar's Cut: empty parameters, and an answer that names only the count and that this
    // was a cut — never a title, an id, a source, a mailbox or an owner/local-control field.
    let answer = w.state_mut().execute("ui.calendarCut", json!({})).expect("ui.calendarCut is a Calendar command");
    assert_eq!(answer, json!({ "items": 2, "cut": true }), "Cut takes the set and answers only how many, and that it was a cut");
    assert!(w.state().dialog.is_none(), "a plain local cut invents no question");
    w.run_steps(2);

    // What it left on the calendar's clipboard: a cut of both members, in selection order, from
    // this mailbox — and nothing moved yet.
    let cut = clipboard(&w).expect("Cut puts the set on the calendar's clipboard");
    assert!(cut.cut, "the clipboard holds a cut, not a copy");
    let on_clip: Vec<Id> = cut.items.iter().map(|item| item.id).collect();
    assert_eq!(on_clip, vec![lunch.id, dentist.id], "both members, in the order selected");
    assert_eq!(cut.sources.len(), 2, "with the source pointers it captured");
    assert_eq!(cut.opening, opening, "of the mailbox it was taken from");
    assert_eq!((starts_at(&w, lunch.id), starts_at(&w, dentist.id)), (Some(lunch.start), Some(dentist.start)), "a cut leaves its sources in place");

    // The picked time and the window's own Paste (the real Ctrl+V route, not a command this test
    // invents): both members move as the events themselves.
    let picked_time = at(date(10, 14), 15, 0);
    let picked = after_noon(&w, 3.0);
    click(&mut w, picked, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert!(w.state().dialog.is_none(), "a local move of uninvited items asks nothing");

    let moved = w.state().session.mailbox.event(lunch.id).cloned().expect("the first member is the event that moved");
    let other = w.state().session.mailbox.event(dentist.id).cloned().expect("the second member is the event that moved");
    let expected = Event { start: picked_time, end: picked_time.add_minutes(lunch.duration_minutes()), ..(*lunch).clone() };
    assert_eq!(moved, expected, "the anchored member keeps its id, UID, origin, organizer, people and calendar, at the picked time");
    let offset = lunch.start.minutes_until(dentist.start);
    let expected_other = Event {
        start: picked_time.add_minutes(offset),
        end: picked_time.add_minutes(offset).add_minutes(dentist.duration_minutes()),
        ..(*dentist).clone()
    };
    assert_eq!(other, expected_other, "the second keeps its own length and the time between it and the anchor");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len(), "a cut moves them: no source is duplicated and none is dropped");
    assert_eq!((titled(&w, "Lunch with Dev").len(), titled(&w, "Dentist").len()), (1, 1), "one of each, still");

    // The successful move is what used the cut up.
    assert!(clipboard(&w).is_none(), "the paste that moved them takes the cut off the calendar's clipboard");
    let moved_to = (starts_at(&w, lunch.id), starts_at(&w, dentist.id));
    let later = after_noon(&w, 5.0);
    click(&mut w, later, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!((starts_at(&w, lunch.id), starts_at(&w, dentist.id)), moved_to, "the spent cut is not pasted again");

    // One step back: both where they were, and the queued changes as they were. (These two items
    // are local, so nothing is queued either way; the server-queue path is `unchanged_single_cut_tests`'.)
    assert!(w.state_mut().session.undo(), "a cut and paste of a set is one step");
    w.run_steps(2);
    assert_eq!(w.state().session.mailbox.event(lunch.id), Some(lunch.as_ref()), "one step puts the first member back as it was");
    assert_eq!(w.state().session.mailbox.event(dentist.id), Some(dentist.as_ref()), "and the second");
    assert_eq!(w.state().session.mailbox.event_changes, before.event_changes, "and takes back exactly the queued changes it made");
    assert_eq!(w.state().session.openings(), opening, "no other mailbox was opened");
}

#[test]
fn ui_calendar_cut_is_registered_as_an_owner_clipboard_command() {
    assert!(crate::UI_COMMANDS.iter().any(|command| command.0 == "ui.calendarCut"), "Cut is a registered window command");
    assert_eq!(mailcraft_engine::agent::access("ui.calendarCut"), Some(mailcraft_engine::agent::Access::Never));
}

#[test]
fn ui_calendar_cut_is_refused_to_agents_even_with_every_grant() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    select(&mut w, &[(lunch.id, None)]);
    w.state_mut().execute("ui.calendarCopy", json!({})).expect("a baseline copy");
    let before = clipboard(&w).expect("the baseline copy");
    let request = crate::control::ControlRequest::new("engine.execute", json!({"command": "ui.calendarCut", "params": {}})).0;
    let request = request.from_agent(mailcraft_engine::agent::Class::ALL.to_vec());
    let ctx = w.ctx.clone();
    let outcome = crate::control::handle(w.state_mut(), &ctx, &request);
    assert!(matches!(outcome, crate::control::Outcome::Done(ref answer) if answer["ok"] == false), "no agent grant may take the clipboard");
    let after = clipboard(&w).expect("the owner's copy is retained");
    assert_eq!(after.items, before.items);
    assert_eq!((after.cut, after.text, after.opening), (before.cut, before.text, before.opening));
    assert!(after.sources.iter().zip(&before.sources).all(|(now, was)| Arc::ptr_eq(now, was)));
    assert_eq!(after.sources.len(), before.sources.len());
    assert_eq!(w.state().session.mailbox.event(lunch.id), Some(lunch.as_ref()));
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "refusal asks nothing");
}

#[test]
fn ui_calendar_cut_rejects_parameters_without_replacing_an_existing_copy() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    select(&mut w, &[(lunch.id, None)]);
    w.state_mut().execute("ui.calendarCopy", json!({})).expect("a baseline copy");
    let before = clipboard(&w).expect("the baseline copy");
    let opening = w.state().session.openings();
    let selection = selected(&w);
    for params in [json!({"cut": true}), json!({"items": []}), json!({"owner": true}), json!(null), json!([]), json!(""), json!(false)] {
        assert!(w.state_mut().execute("ui.calendarCut", params.clone()).is_err(), "reject {params:?}");
        let after = clipboard(&w).expect("refusal retains the original copy");
        assert_eq!(after.items, before.items);
        assert_eq!((after.cut, &after.text, after.opening), (before.cut, &before.text, before.opening));
        assert_eq!(after.sources.len(), before.sources.len());
        assert!(after.sources.iter().zip(&before.sources).all(|(now, was)| Arc::ptr_eq(now, was)));
        assert_eq!(w.state().session.mailbox.event(lunch.id), Some(lunch.as_ref()));
        assert_eq!(selected(&w), selection);
        assert_eq!(w.state().session.openings(), opening);
        assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "refusal asks nothing");
    }
}

/// An agent holding every grant can drive none of the Calendar's three window clipboard commands
/// through the control channel: Copy, Cut and Paste are each refused, ask nothing, and leave the
/// owner's clipboard and the mailbox as they were.
#[test]
fn agents_cannot_drive_calendar_copy_cut_or_paste_with_every_grant() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    select(&mut w, &[(lunch.id, None)]);
    w.state_mut().execute("ui.calendarCopy", json!({})).expect("a baseline copy");
    let before = clipboard(&w).expect("the baseline copy");
    let mailbox = w.state().session.mailbox.clone();
    for command in ["ui.calendarCopy", "ui.calendarCut", "ui.calendarPaste"] {
        // Each could act now were it the agent's: Copy and Cut have the item selected, and Paste a
        // time picked to put the copy at, so a refusal is about who asks, never about a missing
        // pick (review-02 N1; CodeRabbit on #1665, cut_command_tests.rs:185).
        if command == "ui.calendarPaste" {
            let picked = after_noon(&w, 3.0);
            click(&mut w, picked, Modifiers::NONE, PointerButton::Primary);
            assert!(super::super::paste_ready(w.state()), "fixture: Paste could put the copy at the picked time now");
        } else {
            assert!(super::super::copy_ready(w.state()), "fixture: {command} could take the selected item now");
        }
        assert_eq!(mailcraft_engine::agent::access(command), Some(mailcraft_engine::agent::Access::Never), "{command} is never an agent's");
        let request = crate::control::ControlRequest::new("engine.execute", json!({"command": command, "params": {}})).0;
        let request = request.from_agent(mailcraft_engine::agent::Class::ALL.to_vec());
        let ctx = w.ctx.clone();
        let outcome = crate::control::handle(w.state_mut(), &ctx, &request);
        assert!(matches!(outcome, crate::control::Outcome::Done(ref answer) if answer["ok"] == false), "{command}: no agent grant may reach it");
        let after = clipboard(&w).expect("the owner's copy is retained");
        assert_eq!(after.items, before.items, "{command}");
        assert_eq!((after.cut, &after.text, after.opening), (before.cut, &before.text, before.opening), "{command}");
        assert_eq!(after.sources.len(), before.sources.len(), "{command}");
        assert!(after.sources.iter().zip(&before.sources).all(|(now, was)| Arc::ptr_eq(now, was)), "{command}");
        assert!(w.state().session.mailbox == mailbox, "{command} changed nothing");
        assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "{command}: refusal asks nothing");
    }
}
