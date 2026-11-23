// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Calendar's Copy as a command: `ui.calendarCopy` with an empty object takes the whole
//! selected set onto the calendar's clipboard, and the window's existing picked-time Paste puts
//! both members there as copies, leaving their sources and taking both copies back in one undo.
//!
//! One module of `calendar_selection_tests`, which is itself a child of `calendar_view`, so
//! `super::*` is that file's fixtures (`calendar`, `event`, `select`, `selected`, `click`,
//! `chord`, `titled`, `after_noon`, `COMMAND`, and the `egui` and `serde_json` names they import).
//! Nothing here is production code and nothing here is a test hook: the window is the real one,
//! its two items are the sample's invented local appointments, the clock is fixed, and every act
//! is a command, a pointer event or a key the owner would press. No file, server, real account,
//! native window or wall clock is used.
//!
//! The test calls no private `clip` and no mock: the command must be the copy.

use serde_json::json;

use super::*;

/// The window's Copy command, in the Calendar the fixture has in front, with the two local items
/// the typed selection names: it answers how many items it took and that it took a copy, and the
/// calendar's own Paste then brings both of them to the picked time.
#[test]
fn ui_calendar_copy_takes_the_whole_set_for_the_picked_time_paste() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    // Two distinct items of this computer's own, named as `calendar.select`'s typed items say
    // (`{id}` for a whole event; neither is an occurrence of a series).
    assert_ne!(lunch.id, dentist.id, "the fixture's two items are distinct");
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "the typed set is selected");

    // The Calendar's Copy command: empty parameters, and an answer that names only the count and
    // that this was a copy — never a title, an id, the sources or the clipboard text.
    let answer = w.state_mut().execute("ui.calendarCopy", json!({})).expect("ui.calendarCopy is a Calendar command");
    assert_eq!(answer, json!({ "items": 2, "cut": false }), "Copy takes the selected set and answers only how many, and that it was a copy");
    w.run_steps(2);

    // The picked time and the window's own Paste: both members come as copies.
    let somewhere = after_noon(&w, 2.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);

    assert_eq!(titled(&w, "Lunch with Dev").len(), 2, "the first member of the set came");
    assert_eq!(titled(&w, "Dentist").len(), 2, "and the second");
    // A copy leaves its sources where they were.
    assert!(
        titled(&w, "Lunch with Dev").iter().any(|e| e.id == lunch.id && e.start == lunch.start),
        "the source of the first copy is still where it was"
    );
    assert!(
        titled(&w, "Dentist").iter().any(|e| e.id == dentist.id && e.start == dentist.start),
        "the source of the second copy is still where it was"
    );
    // One undo takes both copies back: the set's paste is one step.
    assert!(w.state_mut().session.undo(), "a copy is undoable");
    w.run_steps(2);
    assert_eq!((titled(&w, "Lunch with Dev").len(), titled(&w, "Dentist").len()), (1, 1), "both copies went at once");
}
