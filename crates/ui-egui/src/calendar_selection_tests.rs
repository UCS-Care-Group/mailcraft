// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! More than one calendar item at a time, through the window's own pointer and keys: a Command
//! (Ctrl) click adds and takes away, Shift-click takes the range in view, a right click keeps the
//! set, and Copy, Cut, Paste and Delete act on the whole set, once, with the clipboard belonging
//! to the mailbox it was taken from.
//!
//! The set itself is put in place through `Selection::events` — the shared state the engine's
//! `calendar.select` writes — so that what the set's keys do is tested without depending on how
//! the set is made. Everything else is the real window: the sample's invented events, drawn
//! where the calendar draws them, and clicked where the window's own hit entries say they are.

use std::sync::Arc;

use egui::{Modifiers, PointerButton, Pos2, Rect};
use mailcraft_engine::SelectedEvent;
use mailcraft_model::{Event, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use super::HOUR_H;
use crate::people_view::testing::{Window, window};

/// Ctrl, or Command on a Mac, as the window's own input reports it — the calendar's own Ctrl,
/// which is this.
const MAC: bool = cfg!(target_os = "macos");
const COMMAND: Modifiers = Modifiers { alt: false, ctrl: !MAC, shift: false, mac_cmd: MAC, command: true };
/// Shift alone.
const SHIFT: Modifiers = Modifiers { alt: false, ctrl: false, shift: true, mac_cmd: false, command: false };

/// The sample's calendar in the Week view, ready to be clicked, on the week of its today
/// (Wednesday 14 October 2026).
fn calendar() -> Window {
    let mut w = window("view.calendar");
    // (The sample has reminders that are due, whose window would come up over the grid.)
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
    w
}

/// A day of the week the sample is on.
fn date(month: u8, day: u8) -> Date {
    Date::new(2026, month, day).expect("a day of the sample's week")
}

/// A time of day on `day`.
fn at(day: Date, hour: u8, minute: u8) -> DateTime {
    DateTime::new(day, hour, minute, 0).expect("a time of day")
}

/// The sample's event whose title begins with `title` (its first occurrence in view is the one
/// [`drawn`] gives for it).
fn event(w: &Window, title: &str) -> Arc<Event> {
    w.state()
        .session
        .mailbox
        .events
        .values()
        .find(|e| e.title.starts_with(title))
        .cloned()
        .unwrap_or_else(|| panic!("the sample has no event \"{title}\""))
}

/// Every event of the mailbox with exactly this title, in time order: a copy is told from the
/// event it was made of by there being two of them.
fn titled(w: &Window, title: &str) -> Vec<Event> {
    let mut all: Vec<Event> = w.state().session.mailbox.events.values().filter(|e| e.title == title).map(|e| (**e).clone()).collect();
    all.sort_by_key(|e| (e.start, e.id));
    all
}

/// Where the drawn `title` is.
fn drawn(w: &Window, title: &str) -> Rect {
    let id = event(w, title).id;
    w.state().hit.rect("event", id).unwrap_or_else(|| panic!("\"{title}\" isn't drawn"))
}

/// A point of Wednesday's hours, `hours` after noon: empty grid, which a click picks out (the
/// sample's review there ends at noon).
fn after_noon(w: &Window, hours: f32) -> Pos2 {
    let review = drawn(w, "Design review");
    egui::pos2(review.center().x, review.max.y + hours * HOUR_H + 6.0)
}

/// Click `pos` with `modifiers` held and `button` down, a moment after whatever came before (so
/// that a second click in the same place isn't a double click).
fn click(w: &mut Window, pos: Pos2, modifiers: Modifiers, button: PointerButton) {
    w.run_steps(45);
    w.input_mut().events.push(egui::Event::ModifiersChanged(modifiers));
    w.input_mut().events.push(egui::Event::PointerMoved(pos));
    w.step();
    for pressed in [true, false] {
        w.input_mut().events.push(egui::Event::ModifiersChanged(modifiers));
        w.input_mut().events.push(egui::Event::PointerButton { pos, button, pressed, modifiers });
        w.step();
    }
    w.input_mut().events.push(egui::Event::ModifiersChanged(Modifiers::NONE));
    w.run_steps(2);
}

/// `key` pressed and let go with `modifiers` held.
fn chord(w: &mut Window, modifiers: Modifiers, key: egui::Key) {
    for pressed in [true, false] {
        w.input_mut().events.push(egui::Event::ModifiersChanged(modifiers));
        w.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        w.step();
    }
    w.input_mut().events.push(egui::Event::ModifiersChanged(Modifiers::NONE));
    w.run_steps(2);
}

/// What the window says is selected, in selection order, as `calendar.select`'s items say it: the
/// id and, for an occurrence of a series, its own day.
fn selected(w: &Window) -> Vec<(Id, Option<Date>)> {
    w.state().session.sel.events.iter().map(|item| (item.id, item.day)).collect()
}

/// The focused item, as the legacy `event` and `event_day` keep it.
fn focused(w: &Window) -> (Option<Id>, Option<Date>) {
    (w.state().session.sel.event, w.state().session.sel.event_day)
}

/// Select `items`, the last of them focused, as a Command click of each and the engine's
/// `calendar.select` with them leave the window: the seam for what the set's keys do. The day of
/// a whole event is `None`; of an occurrence, its own day. `event_day` stays the displayed date
/// of the one in focus, as it does for a click.
fn select(w: &mut Window, items: &[(Id, Option<Date>)]) {
    let (last, day) = *items.last().unwrap_or_else(|| panic!("a selection of at least one item"));
    let shown = day.or_else(|| w.state().session.mailbox.event(last).map(|e| e.start.date));
    let sel = &mut w.state_mut().session.sel;
    sel.events = items.iter().map(|(id, day)| SelectedEvent { id: *id, day: *day }).collect();
    sel.event = Some(last);
    sel.event_day = shown;
    w.run_steps(2);
}

/// The key an occurrence's own entry in the hit map is recorded under, beside the series' own
/// `event` entry: the occurrence's day is part of it, so that the fifth stand-up of the week can
/// be asked for rather than given the first one drawn. This spelling is the test's expectation of
/// that key; what is required is that each occurrence is addressable on its own.
fn occurrence_hit(event: Id, day: Date) -> String {
    format!("eventOccurrence:{event}:{}", day.iso())
}

/// Where the occurrence of `event` on its own `day` is drawn.
fn occurrence(w: &Window, event: Id, day: Date) -> Rect {
    w.state()
        .hit
        .rect(&occurrence_hit(event, day), 0)
        .unwrap_or_else(|| panic!("the occurrence of event {event} on {} has no entry of its own", day.iso()))
}

#[test]
fn a_command_click_adds_a_second_item_and_takes_the_one_it_lands_on_away() {
    let mut w = calendar();
    let review = event(&w, "Design review");
    let lunch = event(&w, "Lunch with Dev");

    // On one thing: that one, as a plain click does.
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, COMMAND, PointerButton::Primary);
    assert_eq!(selected(&w), [(review.id, None)], "a Command click on nothing selected takes that one");

    // On another: both, in the order they were clicked.
    let on_lunch = drawn(&w, "Lunch with Dev").center();
    click(&mut w, on_lunch, COMMAND, PointerButton::Primary);
    assert_eq!(selected(&w), [(review.id, None), (lunch.id, None)], "the one clicked first stays");
    assert_eq!(focused(&w), (Some(lunch.id), Some(lunch.start.date)), "the one clicked last has the focus");

    // On the one that is on again: it goes, and the other stays.
    let on_lunch = drawn(&w, "Lunch with Dev").center();
    click(&mut w, on_lunch, COMMAND, PointerButton::Primary);
    assert_eq!(selected(&w), [(review.id, None)], "the one it landed on was taken away");
    assert_eq!(focused(&w), (Some(review.id), Some(review.start.date)), "the one left has the focus");
}

#[test]
fn a_shift_click_takes_the_range_that_is_in_view() {
    let mut w = calendar();
    let release = event(&w, "Write release notes");
    let dentist = event(&w, "Dentist");
    let standup = event(&w, "Team stand-up");

    let from = drawn(&w, "Write release notes").center();
    click(&mut w, from, Modifiers::NONE, PointerButton::Primary);
    let to = drawn(&w, "Dentist").center();
    click(&mut w, to, SHIFT, PointerButton::Primary);

    let set = selected(&w);
    let has = |id: Id| set.iter().any(|(selected_id, _)| *selected_id == id);
    assert!(has(release.id), "the item the range starts at is in it: {set:?}");
    assert!(has(dentist.id), "and the one it ends at: {set:?}");
    assert!(has(event(&w, "Lunch with Dev").id), "and the days between them: {set:?}");
    assert!(has(event(&w, "Design review").id), "and the days between them: {set:?}");
    // An occurrence of a series in the range is a member by its own day, and the Monday before
    // the range is not one.
    assert!(set.contains(&(standup.id, Some(date(10, 14)))), "Wednesday's stand-up is in it: {set:?}");
    assert!(!set.contains(&(standup.id, Some(date(10, 12)))), "Monday's stand-up is before it: {set:?}");
    let (focus, _) = focused(&w);
    assert!(focus == Some(release.id) || focus == Some(dentist.id), "one of the two ends has the focus: {focus:?}");
}

#[test]
fn a_right_click_keeps_the_set_it_lands_on_and_elsewhere_makes_a_new_one() {
    let mut w = calendar();
    let review = event(&w, "Design review");
    let lunch = event(&w, "Lunch with Dev");
    select(&mut w, &[(review.id, None), (lunch.id, None)]);

    // The menu that comes up over one of them is about all of them.
    let on_lunch = drawn(&w, "Lunch with Dev").center();
    click(&mut w, on_lunch, Modifiers::NONE, PointerButton::Secondary);
    assert_eq!(selected(&w), [(review.id, None), (lunch.id, None)], "the right click keeps the set it landed on");
    chord(&mut w, Modifiers::NONE, egui::Key::Escape);

    // Over something else, it is about that one alone.
    let roadmap = event(&w, "Q4 roadmap");
    let on_roadmap = drawn(&w, "Q4 roadmap").center();
    click(&mut w, on_roadmap, Modifiers::NONE, PointerButton::Secondary);
    assert_eq!(selected(&w), [(roadmap.id, None)], "a right click elsewhere leaves the old set behind");
    chord(&mut w, Modifiers::NONE, egui::Key::Escape);
}

#[test]
fn a_copy_of_a_set_pastes_every_member_and_one_undo_takes_them_back() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    // (A click first: the calendar is what the keys are for.)
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);

    chord(&mut w, COMMAND, egui::Key::C);
    let somewhere = after_noon(&w, 2.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);

    assert_eq!(titled(&w, "Lunch with Dev").len(), 2, "one more than the sample's own");
    assert_eq!(titled(&w, "Dentist").len(), 2, "the second member of the set came too");
    // The events themselves are still where they were: a copy is a copy.
    assert!(titled(&w, "Lunch with Dev").iter().any(|e| e.id == lunch.id && e.start == lunch.start));
    assert!(titled(&w, "Dentist").iter().any(|e| e.id == dentist.id && e.start == dentist.start));
    // One step back takes away both copies, not one.
    assert!(w.state_mut().session.undo(), "a copy is undoable");
    w.run_steps(2);
    assert_eq!((titled(&w, "Lunch with Dev").len(), titled(&w, "Dentist").len()), (1, 1), "both copies went at once");
}

#[test]
fn cutting_a_set_moves_every_member_and_keeps_the_clipboard_while_it_is_refused() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);

    chord(&mut w, COMMAND, egui::Key::X);
    assert_eq!((event(&w, "Lunch with Dev").start, event(&w, "Dentist").start), (lunch.start, dentist.start), "nothing moves until it is pasted");
    // Nowhere picked, so the paste is refused — and what was cut is still there to paste.
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(2);
    assert_eq!((event(&w, "Lunch with Dev").start, event(&w, "Dentist").start), (lunch.start, dentist.start), "a refused paste moves nothing");

    // A time picked, and Paste: both move, as one step.
    let somewhere = after_noon(&w, 3.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    let moved = (event(&w, "Lunch with Dev"), event(&w, "Dentist"));
    assert_eq!(
        (moved.0.start, moved.0.end),
        (at(date(10, 14), 15, 0), at(date(10, 14), 16, 0)),
        "the earliest of them lands where the time was picked"
    );
    assert_eq!(moved.1.id, dentist.id, "a cut moves the event itself, not a copy of it");
    assert_ne!(moved.1.start, dentist.start, "and the second member of the set moves too");

    // A cut is used up by the paste that went through.
    let later = after_noon(&w, 5.0);
    click(&mut w, later, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!(event(&w, "Lunch with Dev").start, at(date(10, 14), 15, 0), "the cut clipboard is spent");

    // One step back brings both to where they were.
    assert!(w.state_mut().session.undo(), "a cut and paste is undoable");
    w.run_steps(2);
    assert_eq!((event(&w, "Lunch with Dev").start, event(&w, "Dentist").start), (lunch.start, dentist.start), "one step takes both back");
}

#[test]
fn delete_takes_the_whole_set_in_one_step() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);

    chord(&mut w, Modifiers::NONE, egui::Key::Delete);
    w.run_steps(3);
    assert!(w.state().session.mailbox.event(lunch.id).is_none(), "the first member went");
    assert!(w.state().session.mailbox.event(dentist.id).is_none(), "and so did the second");

    assert!(w.state_mut().session.undo(), "a delete of a set is undoable");
    w.run_steps(2);
    assert_eq!((event(&w, "Lunch with Dev").start, event(&w, "Dentist").start), (lunch.start, dentist.start), "one step brings both back");
}

#[test]
fn a_clipboard_of_another_mailbox_pastes_nothing() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    chord(&mut w, COMMAND, egui::Key::C);
    let before = w.state().session.mailbox.events.len();

    // The same mailbox opened again: its contents are equal (its events have the same ids), its
    // generation is not, and what was copied out of the one it replaced is not for it.
    let same = w.state().session.mailbox.clone();
    w.state_mut().session.replace_mailbox(same);
    w.run_steps(3);

    let somewhere = after_noon(&w, 2.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!(w.state().session.mailbox.events.len(), before, "nothing came of a clipboard of another mailbox");
    assert_eq!(titled(&w, "Lunch with Dev").len(), 1, "and no copy of it either");
}

#[test]
fn each_occurrence_of_a_series_has_an_entry_of_its_own() {
    let mut w = calendar();
    let standup = event(&w, "Team stand-up");
    let (monday, wednesday) = (date(10, 12), date(10, 14));
    let (on_monday, on_wednesday) = (occurrence(&w, standup.id, monday), occurrence(&w, standup.id, wednesday));
    assert_ne!(on_monday, on_wednesday, "the two occurrences are drawn on their own days");

    // Clicking each takes the occurrence it is drawn for.
    click(&mut w, on_monday.center(), Modifiers::NONE, PointerButton::Primary);
    assert_eq!(focused(&w), (Some(standup.id), Some(monday)), "Monday's own occurrence");
    click(&mut w, on_wednesday.center(), Modifiers::NONE, PointerButton::Primary);
    assert_eq!(focused(&w), (Some(standup.id), Some(wednesday)), "Wednesday's");

    // And with Command the series is two members of the set, each on its own day.
    click(&mut w, on_monday.center(), COMMAND, PointerButton::Primary);
    assert_eq!(selected(&w), [(standup.id, Some(wednesday)), (standup.id, Some(monday))]);
}

#[test]
fn every_view_that_shows_an_event_takes_it_the_same_way() {
    for view in ["view.day", "view.workWeek", "view.week", "view.month"] {
        let mut w = calendar();
        let _ = w.state_mut().execute(view, json!({}));
        w.run_steps(3);
        let review = event(&w, "Design review");
        let standup = event(&w, "Team stand-up");
        let on_review = drawn(&w, "Design review").center();
        click(&mut w, on_review, COMMAND, PointerButton::Primary);
        // (The occurrence is addressed by its own key: the month shows more than one stand-up.)
        let on_standup = occurrence(&w, standup.id, date(10, 14)).center();
        click(&mut w, on_standup, COMMAND, PointerButton::Primary);
        assert_eq!(selected(&w), [(review.id, None), (standup.id, Some(date(10, 14)))], "{view}");
    }
}

#[test]
fn a_whole_day_bar_and_a_month_chip_are_each_addressable() {
    let mut w = calendar();
    let _ = w.state_mut().execute("view.month", json!({}));
    w.run_steps(3);
    let planning = event(&w, "Planning days");
    let review = event(&w, "Design review");
    let bar = occurrence(&w, planning.id, planning.start.date);
    assert!(w.state().hit.rect(&occurrence_hit(planning.id, review.start.date), 0).is_none(), "another day is not this bar's");
    assert!(w.state().hit.rect(&occurrence_hit(review.id, review.start.date), 0).is_some(), "the review's chip has its own entry");

    // A click on the bar takes that whole-day item, as a click in a month cell does.
    click(&mut w, bar.center(), Modifiers::NONE, PointerButton::Primary);
    assert_eq!(selected(&w), [(planning.id, None)], "the bar is the item it names");
}

#[test]
fn select_all_takes_only_what_it_may_and_says_the_range_holds_more() {
    let mut w = calendar();
    let calendar = event(&w, "Design review").calendar;
    let day = date(10, 14);
    // More than a selection takes, all in the week in view.
    for n in 0..250i64 {
        let start = at(day, 0, 0).add_minutes(n);
        let extra = Event { calendar, title: format!("Extra {n}"), start, end: start.add_minutes(1), ..Default::default() };
        let _ = w.state_mut().session.mailbox.add_event(extra);
    }
    w.run_steps(3);
    chord(&mut w, COMMAND, egui::Key::A);
    w.run_steps(3);

    assert_eq!(w.state().session.sel.events.len(), 200, "a selection holds no more than 200 items");
    assert_eq!(w.state().ran.1, "calendar.selectAll", "the key runs Select All");
    let answer = w.state().ran.2.as_ref().ok().cloned().unwrap_or_default();
    assert_eq!(answer.get("capped").and_then(serde_json::Value::as_bool), Some(true), "the answer says the range holds more");
}

#[test]
fn copy_from_the_context_menu_takes_the_whole_set() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);

    // What the menu's Copy does (it is the same act as the key's).
    let ctx = w.ctx.clone();
    super::clip(w.state_mut(), &ctx, false);
    w.run_steps(2);
    let somewhere = after_noon(&w, 2.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!(titled(&w, "Lunch with Dev").len(), 2, "the first member of the set came");
    assert_eq!(titled(&w, "Dentist").len(), 2, "and the second");
}

#[test]
fn a_cut_whose_source_changed_since_is_not_pasted_again() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    chord(&mut w, COMMAND, egui::Key::X);
    assert_eq!(event(&w, "Lunch with Dev").start, lunch.start, "nothing moves until it is pasted");
    // (What is held is a cut of both: a stale cut, not an empty clipboard, is what Paste refuses.)
    let held = w.ctx.data(|d| d.get_temp::<super::Clip>(egui::Id::unique(super::CLIP_ID))).expect("Ctrl+X put the set on the calendar's clipboard");
    assert!(held.cut, "as a cut");
    assert_eq!(held.items.iter().map(|item| item.id).collect::<Vec<_>>(), vec![lunch.id, dentist.id], "of both members");

    // The source changes behind the clipboard's back (an edit replaces the shared event).
    w.state_mut().session.mailbox.event_mut(lunch.id).expect("the source is there").title.push_str(" (changed)");
    w.run_steps(2);
    let somewhere = after_noon(&w, 4.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);

    assert_eq!(event(&w, "Lunch with Dev").start, lunch.start, "what is no longer what was cut is not moved");
    assert_eq!(event(&w, "Dentist").start, dentist.start, "nor the rest of the set it belonged to");
}

#[test]
fn cut_from_the_context_menu_moves_the_set_and_is_spent_by_the_paste() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    // What the menu's Cut does.
    let ctx = w.ctx.clone();
    super::clip(w.state_mut(), &ctx, true);
    w.run_steps(2);

    let somewhere = after_noon(&w, 3.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!(event(&w, "Lunch with Dev").start, at(date(10, 14), 15, 0), "the earliest member lands where the time was picked");
    assert_eq!(event(&w, "Dentist").id, dentist.id, "a cut moves the event itself");
    let moved = event(&w, "Dentist").start;

    // Spent: a second Paste does nothing.
    let later = after_noon(&w, 5.0);
    click(&mut w, later, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!((event(&w, "Dentist").start, event(&w, "Lunch with Dev").start), (moved, at(date(10, 14), 15, 0)), "the cut clipboard is spent");
}

#[test]
fn delete_from_the_context_menu_takes_the_pinned_set_in_one_step() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    // What the menu's Delete names: the set itself, pinned.
    let items = json!(
        w.state()
            .session
            .sel
            .events
            .iter()
            .map(|item| match item.day {
                Some(day) => json!({ "id": item.id, "day": day.iso() }),
                None => json!({ "id": item.id }),
            })
            .collect::<Vec<_>>()
    );
    let _ = w.state_mut().run("calendar.delete", json!({ "items": items, "scope": "occurrence" }));
    w.run_steps(3);

    assert!(w.state().session.mailbox.event(lunch.id).is_none(), "the first member went");
    assert!(w.state().session.mailbox.event(dentist.id).is_none(), "and the second");
    assert!(w.state_mut().session.undo(), "one step");
    w.run_steps(2);
    assert!(
        w.state().session.mailbox.event(lunch.id).is_some() && w.state().session.mailbox.event(dentist.id).is_some(),
        "one step brings both back"
    );
}

#[path = "calendar_multiselect_edge_tests.rs"]
mod edge_tests;

#[path = "calendar_review_regression_tests.rs"]
mod review_regressions;

#[path = "calendar_late_approval_tests.rs"]
mod late_approval_tests;

#[path = "calendar_copy_command_tests.rs"]
mod copy_command_tests;

#[path = "main_context_guard_tests.rs"]
mod main_context_guard_tests;

#[path = "cut_command_tests.rs"]
mod cut_command_tests;

#[path = "command_search_clipboard_tests.rs"]
mod command_search_clipboard_tests;

#[path = "anonymous_clock_ui_tests.rs"]
mod anonymous_clock_ui_tests;

#[path = "native_window_guard_tests.rs"]
mod native_window_guard_tests;

#[path = "paste_command_tests.rs"]
mod paste_command_tests;

#[path = "paste_safety_tests.rs"]
mod paste_safety_tests;

#[path = "paste_route_tests.rs"]
mod paste_route_tests;

#[path = "calendar_review_followup_tests.rs"]
mod review_followup_tests;

#[path = "calendar_coderabbit2_tests.rs"]
mod coderabbit2_tests;

#[path = "calendar_one_item_tests.rs"]
mod one_item_tests;
