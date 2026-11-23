// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! CodeRabbit's second pass over #1665 and review-05's follow-ups, held against the window: a
//! Command or Shift click on a cancelled appointment leaves what is selected as it is (a set of
//! one, and the older single focus, included), a set drawn in two blocks is never pasted into the
//! front calendar of one, Command Search shows the Calendar's Copy, Cut and Paste as they will
//! run, and the owner's Paste of a cut whose appointment has unsaved changes in its own window.
//!
//! One module of `calendar_selection_tests`, so `super::*` is that file's fixtures (`calendar`,
//! `event`, `selected`, `focused`, `click`, `chord`, `after_noon`, `date`, `at`, `COMMAND`,
//! `SHIFT`, `Window`, `Event`, `Id`, `DateTime`, `Modifiers`, `PointerButton`). The window is the
//! real one; the events are the sample's invented ones and appointments added through
//! `Mailbox::add_event`; every act is a pointer event, a key or a command the owner picks. No
//! file, server, real account or wall clock.

use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_model::{Calendar, Rights};
use serde_json::json;

use super::*;
use crate::forms::Form;
use crate::people_view::testing::window_on;

/// Add `title` at `start`, `minutes` long, to the sample's calendar; `cancelled` when it is called off.
fn add(w: &mut Window, title: &str, start: DateTime, minutes: i64, cancelled: bool) -> Id {
    let calendar = event(w, "Design review").calendar;
    let event = Event { calendar, title: title.into(), start, end: start.add_minutes(minutes), cancelled, ..Default::default() };
    let id = w.state_mut().session.mailbox.add_event(event).expect("the sample's calendar takes an appointment");
    w.run_steps(3);
    id
}

fn point(w: &Window, id: Id) -> egui::Pos2 {
    w.state().hit.rect("event", id).expect("fixture: the appointment is drawn").center()
}

/// What the status line says, and nothing said is "".
fn said(w: &Window) -> String {
    w.state().status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default()
}

/// A plain click makes a set of one; a Command click on a cancelled appointment keeps it, and
/// says why (review-05 NB4: a set of one, not only of two).
#[test]
fn a_command_click_on_a_cancelled_appointment_keeps_a_set_of_one() {
    let mut w = calendar();
    let off = add(&mut w, "Called off", at(date(10, 14), 13, 0), 30, true);
    let review = event(&w, "Design review").id;
    let spot = point(&w, review);
    click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
    let set = selected(&w);
    assert_eq!(set, vec![(review, None)], "fixture: a set of one");
    w.state_mut().status_msg = None;
    let spot = point(&w, off);
    click(&mut w, spot, COMMAND, PointerButton::Primary);
    assert_eq!(selected(&w), set, "the set of one stays");
    assert!(said(&w).contains("Called off") && said(&w).contains("cancelled"), "{:?}", said(&w));
}

/// With an appointment in focus the older way (`calendar.select` by `id`, which the meeting
/// dialogs and agents use), a Command click on a cancelled one keeps that focus, as a Command
/// click on an ordinary appointment does (review-05 NB1).
#[test]
fn a_command_click_on_a_cancelled_appointment_keeps_the_older_focus() {
    let mut w = calendar();
    let off = add(&mut w, "Called off", at(date(10, 14), 13, 0), 30, true);
    let review = event(&w, "Design review").id;
    w.state_mut().execute("calendar.select", json!({ "id": review })).expect("the older single selection");
    w.run_steps(2);
    assert!(w.state().session.sel.events.is_empty(), "fixture: no typed set, only the focus");
    w.state_mut().status_msg = None;
    let spot = point(&w, off);
    click(&mut w, spot, COMMAND, PointerButton::Primary);
    assert_eq!(focused(&w).0, Some(review), "the focus is kept");
    assert!(said(&w).contains("cancelled"), "and the status line says why: {:?}", said(&w));
}

/// A Shift click that ends on a cancelled appointment keeps what is selected, rather than asking
/// for a range the engine refuses whole (CodeRabbit, calendar_view.rs:1028).
#[test]
fn a_shift_click_on_a_cancelled_appointment_keeps_the_selection() {
    let mut w = calendar();
    let off = add(&mut w, "Called off", at(date(10, 14), 13, 0), 30, true);
    let review = event(&w, "Design review").id;
    let spot = point(&w, review);
    click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
    let set = selected(&w);
    w.state_mut().status_msg = None;
    let spot = point(&w, off);
    click(&mut w, spot, SHIFT, PointerButton::Primary);
    assert_eq!(selected(&w), set, "the selection is as it was");
    assert_eq!(crate::people_view::testing::dialog(&w), None, "no refusal comes up");
    assert!(said(&w).contains("Called off"), "{:?}", said(&w));
}

/// The sample's calendar with "Behind" laid over it (in front), and "Beside" in a block of its
/// own, each with an appointment of its own on the sample's Wednesday morning.
fn two_blocks() -> (Window, Id, Id, Id) {
    let mut mailbox = mailcraft_engine::sample::mailbox(crate::people_view::testing::now(), 0);
    let (behind, beside) = (911, 912);
    for (id, name) in [(behind, "Behind"), (beside, "Beside")] {
        mailbox.calendars.push(Calendar { id, name: name.into(), visible: true, rights: Rights::Edit, ..Calendar::default() });
    }
    let nine = at(date(10, 14), 9, 0);
    let away = mailbox
        .add_event(Event { title: "Beside one".into(), start: nine, end: nine.add_minutes(30), calendar: beside, ..Event::default() })
        .expect("the calendar beside takes an appointment");
    let mut w = window_on(mailbox, "view.calendar");
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.state_mut().execute("view.overlay", json!({ "calendar": behind })).expect("Behind is laid over the first calendar");
    w.run_steps(3);
    let panes = mailcraft_engine::cmd::calendar_settings::panes(&w.state().session);
    let home = event(&w, "Design review");
    let first = panes.iter().find(|pane| pane.calendars.contains(&home.calendar)).expect("fixture: the sample's calendar is drawn");
    assert!(first.calendars.contains(&behind) && first.front == behind, "fixture: Behind is over the sample's calendar, in front: {panes:?}");
    assert!(!first.calendars.contains(&beside), "fixture: Beside is a block of its own");
    (w, home.id, away, behind)
}

/// A cut of one appointment drawn behind the front calendar and one from another block, pasted
/// at a time picked in the first block, would take the first into the front calendar: it is
/// refused, and nothing moves (CodeRabbit, calendar_view.rs:3808).
#[test]
fn a_set_from_two_blocks_is_never_pasted_into_the_front_calendar_of_one() {
    let (mut w, home, away, _) = two_blocks();
    let items = json!({ "items": [{ "id": home }, { "id": away }] });
    w.state_mut().execute("calendar.select", items).expect("both are selected");
    w.run_steps(2);
    chord(&mut w, COMMAND, egui::Key::X);
    let before = w.state().session.mailbox.clone();
    let spot = after_noon(&w, 3.0);
    click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
    w.state_mut().status_msg = None;
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert!(w.state().session.mailbox == before, "nothing was moved");
    assert!(said(&w).contains("two goes"), "the status line says how it can be done: {:?}", said(&w));
}

/// Command Search lists the Calendar's Copy, Cut and Paste as they will be once it has gone, which
/// is when they run (CodeRabbit, dialogs.rs:2154): not dimmed by the search window itself.
#[test]
fn command_search_shows_the_calendars_copy_cut_and_paste_as_they_will_run() {
    let mut w = calendar();
    let review = event(&w, "Design review").id;
    let spot = point(&w, review);
    click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
    let on = |w: &Window, id: &str| crate::dialogs::all_commands(w.state()).into_iter().find(|entry| entry.0 == id).map(|entry| entry.4);
    w.state_mut().execute("ui.dialog", json!({ "name": "commands", "query": "Copy" })).expect("Command Search opens");
    w.run_steps(3);
    assert_eq!(crate::people_view::testing::dialog(&w), Some("commands"), "fixture: Command Search is open");
    assert_eq!(on(&w, "ui.calendarCopy"), Some(true), "Copy is ready for once the search has gone");
    assert_eq!(on(&w, "ui.calendarCut"), Some(true), "and so is Cut");
    assert_eq!(on(&w, "ui.calendarPaste"), Some(false), "Paste has nothing to paste yet");
    w.state_mut().dialog = None;
    w.run_steps(2);
    w.state_mut().execute("ui.calendarCopy", json!({})).expect("fixture: the review is copied");
    let spot = after_noon(&w, 3.0);
    click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
    w.state_mut().execute("ui.dialog", json!({ "name": "commands", "query": "Paste" })).expect("Command Search opens");
    w.run_steps(3);
    assert_eq!(on(&w, "ui.calendarPaste"), Some(true), "with a copy and a time picked, Paste is ready too");
}

/// The owner's own Paste of a cut whose appointment is open with unsaved changes in a window of
/// its own is the owner's own move, as dragging it or Alt+Down is: it goes through, and the window
/// keeps what was typed in it (CodeRabbit, lib.rs:1562: the help no longer promises a question
/// about that window; only a route that can't ask refuses such a paste, `paste_safety_tests`).
#[test]
fn the_owners_paste_of_a_cut_whose_window_has_unsaved_changes_is_their_own_move() {
    let mut w = calendar();
    let lunch = add(&mut w, "Team lunch", at(date(10, 14), 15, 0), 30, false);
    let opened = w.state_mut().execute("ui.openWindow", json!({ "event": lunch })).expect("the appointment opens in a window of its own");
    let window = opened["window"].as_u64().expect("a window number");
    {
        let form = w.state_mut().windows.form_mut(window).and_then(Form::event_mut).expect("the window shows the appointment's form");
        form.take_proposal(at(date(10, 16), 10, 0), at(date(10, 16), 11, 0), false);
    }
    assert_eq!(w.state().windows.unsaved(), 1, "fixture: the window has a change that isn't saved");
    w.state_mut().execute("calendar.select", json!({ "items": [{ "id": lunch }] })).expect("Team lunch is selected");
    w.state_mut().execute("ui.calendarCut", json!({})).expect("it is cut");
    let spot = after_noon(&w, 4.0);
    click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
    let start = w.state().session.mailbox.event(lunch).map(|event| event.start);
    w.state_mut().status_msg = None;
    let answer = w.state_mut().run("ui.calendarPaste", json!({}));
    w.run_steps(3);
    let dialog = crate::people_view::testing::dialog(&w);
    eprintln!("owner paste with an unsaved window: answer={answer:?} dialog={dialog:?} status={:?}", said(&w));
    assert_eq!(answer, Ok(json!({ "items": 1, "cut": true })), "the owner's paste goes through");
    assert_eq!(dialog, None, "nothing is asked about the window");
    assert_ne!(w.state().session.mailbox.event(lunch).map(|event| event.start), start, "the appointment moved, as the owner's own move");
    assert_eq!(w.state().windows.unsaved(), 1, "the owner's unsaved changes are still there");
}

/// The last entry of an item's own menu, which the entries above it are found by.
const MENU_LAST: &str = "Save as iCalendar…";

fn button(node: &egui_kittest::Node<'_>) -> bool {
    node.accesskit_node().role() == egui::accesskit::Role::Button
}

/// Open the menu of the appointment `over` with a secondary click and choose `label` in it, as
/// the owner does.
fn from_its_menu(w: &mut Window, over: Id, label: &str) {
    let spot = point(w, over);
    click(w, spot, Modifiers::NONE, PointerButton::Secondary);
    let anchor = w
        .query_all_by_label_contains(MENU_LAST)
        .find(|node| button(node) && node.accesskit_node().label().is_some_and(|said| said.trim().starts_with(MENU_LAST)))
        .map(|node| node.rect())
        .expect("the item's own menu is open");
    let mut found: Vec<egui::Rect> = w
        .query_all_by_label_contains(label)
        .filter(|node| button(node) && node.accesskit_node().label().is_some_and(|said| said.trim() == label))
        .map(|node| node.rect())
        .filter(|rect| (rect.min.x - anchor.min.x).abs() < 1.0 && rect.min.y < anchor.min.y)
        .collect();
    assert_eq!(found.len(), 1, "the menu has one entry called {label}");
    let entry = found.pop().expect("one entry").center();
    click(w, entry, Modifiers::NONE, PointerButton::Primary);
    w.run_steps(2);
}

/// The menu's Copy, Cut and Delete, chosen in the menu itself, each act on the whole set
/// (CodeRabbit nitpick, calendar_selection_tests.rs:422/465/503: those tests call `clip` or build
/// the payload themselves, and would pass with the entries gone or wired to something else).
#[test]
fn the_items_menu_copies_cuts_and_deletes_the_whole_set() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    let both = json!({ "items": [{ "id": lunch.id }, { "id": dentist.id }] });

    w.state_mut().execute("calendar.select", both.clone()).expect("both are selected");
    w.run_steps(2);
    from_its_menu(&mut w, lunch.id, "Copy");
    let spot = after_noon(&w, 2.0);
    click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!((titled(&w, "Lunch with Dev").len(), titled(&w, "Dentist").len()), (2, 2), "the menu's Copy took both");

    w.state_mut().execute("calendar.select", both.clone()).expect("both are selected again");
    w.run_steps(2);
    from_its_menu(&mut w, dentist.id, "Cut");
    let spot = after_noon(&w, 3.0);
    click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    let moved = |w: &Window, id: Id| w.state().session.mailbox.event(id).map(|event| event.start);
    assert_eq!(moved(&w, lunch.id), Some(at(date(10, 14), 15, 0)), "the menu's Cut moved the earliest to the picked time");
    assert_ne!(moved(&w, dentist.id), Some(dentist.start), "and the other with it");

    w.state_mut().execute("calendar.select", both).expect("both are selected again");
    w.run_steps(2);
    from_its_menu(&mut w, lunch.id, "Delete");
    w.run_steps(3);
    let gone = |w: &Window, id: Id| w.state().session.mailbox.event(id).is_none();
    assert!(gone(&w, lunch.id) && gone(&w, dentist.id), "the menu's Delete took both");
}

/// Select All over more than a selection holds says so in the status line, not only in the
/// command's answer (CodeRabbit nitpick, calendar_selection_tests.rs:410).
#[test]
fn select_all_over_more_than_it_takes_says_so_in_the_status_line() {
    let mut w = calendar();
    let calendar = event(&w, "Design review").calendar;
    for n in 0..250i64 {
        let start = at(date(10, 14), 0, 0).add_minutes(n);
        let extra = Event { calendar, title: format!("Extra {n}"), start, end: start.add_minutes(1), ..Default::default() };
        let _ = w.state_mut().session.mailbox.add_event(extra);
    }
    w.run_steps(3);
    w.state_mut().status_msg = None;
    chord(&mut w, COMMAND, egui::Key::A);
    w.run_steps(3);
    assert_eq!(w.state().session.sel.events.len(), 200, "fixture: a selection holds 200");
    let line = said(&w);
    assert!(
        line.contains("200") && line.contains("More calendar items remain"),
        "the status line says how many were taken, and that there are more: {line:?}"
    );
}
