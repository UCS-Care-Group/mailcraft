// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The edges of a selected set of calendar items that the set's own keys do not reach: one
//! occurrence of a series cut and pasted across the change of the clocks, a set whose items come
//! from two blocks of the grid, a Shift click that goes backwards, a copy whose event's id the
//! mailbox gave to another appointment, and the three things an item's menu does to a set.
//!
//! One module of `calendar_selection_tests`, which is itself a child of `calendar_view`, so
//! `super::*` is that file's fixtures (its `calendar()`, `click`, `chord`, `select`, `drawn`,
//! `event`, `titled`, `occurrence`, `focused`, `selected`, `HOUR_H`) and `super::super::*` is
//! `calendar_view`'s own `Clip`, `clip`, `paste` and `paste_calendar`. Nothing here is production
//! code and nothing here is a test hook: the window is the real one, its events are synthetic
//! (invented people, `example.org`), and every act is a pointer event, a key, a command or a
//! button the owner would press.
//!
//! Fixtures only: the sample mailbox, appointments added through `Mailbox::add_event`, and
//! `Session::in_zone` with a fixed clock. No files, servers, real accounts or wall clock.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_model::{Calendar, Freq, Recurrence, Rights};
use mailcraft_time::{Timestamp, Zone};
use serde_json::json;

use super::super::{CLIP_ID, Clip, Slot, paste_calendar};
use super::*;
use crate::people_view::testing::{dialog, now, window_on};
use crate::{MailApp, Services};

/// The last entry of a calendar item's menu. Every entry of that menu is a button of its one
/// column, so a name the ribbon also has ("Copy", "Delete") is still found as the menu's.
const MENU_LAST: &str = "Save as iCalendar…";

/// The week in the sample's calendar, ready to be clicked: what [`super::calendar`] is, without
/// the fixed offset that window pins.
fn week() -> Window {
    let mut w = window_on(mailcraft_engine::sample::mailbox(now(), 0), "view.calendar");
    ready(&mut w);
    w
}

/// The window's reminder windows out of the way, the Week view in front.
fn ready(w: &mut Window) {
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
}

/// A window on `mailbox` whose calendar is shown in `zone`'s clock: the zone has to be the
/// session's own (`Session::in_zone`), because a session pinned to an offset (`Session::at`,
/// which the other fixtures use) keeps that offset whatever the options say.
fn window_in(mailbox: mailcraft_model::Mailbox, zone: &str) -> Window {
    let zone = Zone::named(zone).expect("a zone of the bundled database");
    let mut app = MailApp::new(mailcraft_engine::Session::in_zone(mailbox, now(), zone), Services::default());
    app.autosave = false;
    app.execute("view.calendar", json!({})).expect("the calendar view opens");
    let mut w = Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    ready(&mut w);
    w
}

/// The middle of the 15:00 row of the day whose `title` is drawn from 11:30 to 12:00, in the block
/// it is drawn in: three hours below a block that ends at noon, which is how [`super::after_noon`]
/// reads 15:00 off the sample's review. (Every block this file reads a time off ends at 12:00.)
fn at_three(w: &Window, title: &str) -> Pos2 {
    let block = drawn(w, title);
    egui::pos2(block.center().x, block.max.y + 3.0 * HOUR_H + 6.0)
}

/// Whether `node` is a button the owner can press (a menu entry, a question's answer).
fn button(node: &egui_kittest::Node<'_>) -> bool {
    node.accesskit_node().role() == Role::Button
}

/// The item of the window's open menu called `label`, where it is drawn.
fn menu_entry<'a>(w: &'a Window, label: &'a str) -> Rect {
    let anchor = w
        .query_all_by_label_contains(MENU_LAST)
        .find(|node| button(node) && node.accesskit_node().label().is_some_and(|said| said.trim().starts_with(MENU_LAST)))
        .map(|node| node.rect())
        .unwrap_or_else(|| panic!("an item's own menu is not open (nothing says {MENU_LAST})"));
    let mut found: Vec<Rect> = w
        .query_all_by_label_contains(label)
        .filter(|node| button(node) && node.accesskit_node().label().is_some_and(|said| said.trim() == label))
        .map(|node| node.rect())
        .filter(|rect| (rect.min.x - anchor.min.x).abs() < 1.0 && rect.min.y < anchor.min.y)
        .collect();
    assert_eq!(found.len(), 1, "the menu has one entry called {label}");
    found.pop().expect("one entry")
}

/// Choose `label` in the window's open menu, as the owner does.
fn choose_in_menu(w: &mut Window, label: &str) {
    let at = menu_entry(w, label).center();
    click(w, at, Modifiers::NONE, PointerButton::Primary);
    w.run_steps(2);
}

/// Choose the one button of the window called `label`: a question's own answer.
fn choose_button(w: &mut Window, label: &str) {
    let mut found: Vec<Rect> = w
        .query_all_by_label_contains(label)
        .filter(|node| button(node) && node.accesskit_node().label().is_some_and(|said| said.trim() == label))
        .map(|node| node.rect())
        .collect();
    assert_eq!(found.len(), 1, "one button says {label}");
    let at = found.pop().expect("one button").center();
    click(w, at, Modifiers::NONE, PointerButton::Primary);
    w.run_steps(3);
}

/// The title of the question that is open, when the one that is open is a question.
fn question(w: &Window) -> Option<String> {
    match &w.state().dialog {
        Some(crate::dialogs::Dialog::Confirm { ask, .. }) => Some(ask.title.clone()),
        _ => None,
    }
}

/// One occurrence of a series that repeats by another zone's clock is cut and pasted at 15:00,
/// weeks after the clocks changed: what was cut lands at 15:00 exactly, as an event of its own,
/// with the weeks either side of it where they were, and the whole series is never what moves.
///
/// The series repeats on Wednesday at 05:00 in Europe/Berlin, which the window shows in
/// America/New_York: before the change of the clocks there (8 March 2026) its Wednesday reads on
/// the Tuesday before, so the day the series names an occurrence by and the day the owner sees it
/// on are two different days — and the wall time between the series' own start (January) and the
/// time asked for (19 May) crosses that change, so a delta worked out on the series' start rather
/// than on the occurrence lands an hour out.
#[test]
fn a_cut_occurrence_lands_at_the_time_asked_for_across_the_change_of_the_clocks() {
    let mut mailbox = mailcraft_engine::sample::mailbox(now(), 0);
    let calendar = mailbox.default_calendar().expect("the sample has a calendar");
    // 2026-01-07 05:00 in Berlin, which reads 2026-01-06 23:00 in New York.
    let first = at(date(1, 6), 23, 0);
    let series = mailbox
        .add_event(Event {
            title: "Berlin stand-up".into(),
            location: "Haus 3".into(),
            start: first,
            end: first.add_minutes(45),
            start_zone: "Europe/Berlin".into(),
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            calendar,
            ..Default::default()
        })
        .expect("the calendar takes an event");
    // A plain appointment, ending at noon, to read the 15:00 row of the day of the paste off.
    let anchor = at(date(5, 19), 11, 30);
    mailbox
        .add_event(Event { title: "Slot anchor".into(), start: anchor, end: anchor.add_minutes(30), calendar, ..Default::default() })
        .expect("the calendar takes an event");
    let mut w = window_in(mailbox, "America/New_York");
    let _ = w.state_mut().execute("view.goToDate", json!({ "date": date(5, 19).iso() }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);

    // Anchor.shift is -1 (Berlin Jan 7, New York Jan 6): the March 11 occurrence has intrinsic key March 10.
    let cut_day = date(3, 10);
    let shown_day = date(3, 11);
    let next_week = cut_day.add_days(7);
    let before = w.state().session.mailbox.event(series).cloned().expect("the series is there");
    let (its_start, neighbour_before) = {
        let here = |instant: Timestamp| w.state().session.offset_at(instant);
        (before.occurrence_times(cut_day, &here).0, before.occurrence_times(next_week, &here).0)
    };
    assert_eq!(its_start, at(shown_day, 0, 0), "fixture: intrinsic March 10 is drawn March 11 while the zones disagree");
    assert_ne!(shown_day, cut_day, "fixture: the day it is named by and the day it is shown on differ");

    w.state_mut()
        .execute("calendar.select", json!({"items": [{"id": series, "day": cut_day.iso()}]}))
        .expect("a real intrinsic occurrence is selected");
    w.run_steps(2);
    assert_eq!(selected(&w), [(series, Some(cut_day))], "fixture: the real reader retains the canonical key");
    assert_eq!(focused(&w), (Some(series), Some(shown_day)), "fixture: legacy focus is the displayed day");
    chord(&mut w, COMMAND, egui::Key::X);
    assert_eq!(w.state().session.mailbox.event(series).map(|event| event.start), Some(before.start), "a cut moves nothing until it is pasted");

    // The time asked for: 15:00 on the 19th of May, after the clocks changed on both sides.
    let picked = at_three(&w, "Slot anchor");
    click(&mut w, picked, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);

    // Cutting one occurrence is about that occurrence: whatever is asked, the series as a whole
    // is not what moves.
    assert_eq!(
        w.state().session.mailbox.event(series).map(|event| event.start),
        Some(before.start),
        "the series as a whole has not moved: a cut of one occurrence is not a cut of the series"
    );
    // The window may ask which of the series is meant; answering says the one that was cut.
    if w.state().dialog.is_some() {
        assert_eq!(question(&w).as_deref(), Some("Move Recurring Item"), "the question is which of the series, and only that");
        choose_button(&mut w, "Just This One");
    }

    let after = w.state().session.mailbox.event(series).cloned().expect("the series is still there");
    assert!(after.recurrence.is_some(), "the series still repeats");
    assert_eq!(after.start, before.start, "and starts where it did");
    assert_eq!(after.exceptions, vec![cut_day], "only the occurrence that was cut left it, named by its own day");
    let its_own: Vec<Event> = titled(&w, "Berlin stand-up").into_iter().filter(|event| event.recurrence.is_none()).collect();
    assert_eq!(its_own.len(), 1, "the occurrence that was cut is an event of its own now: {its_own:?}");
    let one = its_own.first().expect("the occurrence on its own");
    assert_ne!(one.id, series, "with an identity of its own");
    assert_eq!(one.start, at(date(5, 19), 15, 0), "it lands at the 15:00 that was asked for, not an hour either side of it");
    assert_eq!(one.end, at(date(5, 19), 15, 45), "and keeps its length");
    assert_eq!(one.location, "Haus 3", "and what it carried");
    assert_eq!(one.calendar, calendar, "and its calendar");
    // The weeks either side of the one that went are where they were.
    let neighbour_after = {
        let here = |instant: Timestamp| w.state().session.offset_at(instant);
        after.occurrence_times(next_week, &here).0
    };
    assert_eq!(neighbour_after, neighbour_before, "the next week's occurrence is where it was");

    // One step back puts the occurrence back in the series and takes the event of its own away.
    assert!(w.state_mut().session.undo(), "a cut and paste of one occurrence is one step");
    w.run_steps(2);
    let back = w.state().session.mailbox.event(series).cloned().expect("the series is there");
    assert!(back.exceptions.is_empty(), "the occurrence is back in the series: {:?}", back.exceptions);
    assert_eq!(titled(&w, "Berlin stand-up").len(), 1, "and the event of its own is gone");
}

/// Two items of two blocks, copied where a time is picked in one of them, go to that block's
/// calendar: both, by their own offsets, in one step.
///
/// The calendar a paste is into is worked out for the whole set: with one item of the set already
/// in the calendar picked in, a rule that looks only at the others leaves every item in its own
/// calendar, so the item of the other block never arrives.
#[test]
fn a_set_of_two_blocks_pastes_into_the_block_the_time_was_picked_in() {
    let mut mailbox = mailcraft_engine::sample::mailbox(now(), 0);
    let own = mailbox.default_calendar().expect("the sample has a calendar");
    let there = 900;
    mailbox.calendars.push(Calendar { id: there, name: "Second".into(), visible: true, rights: Rights::Edit, ..Default::default() });
    let day = date(10, 14);
    let (nine, ten) = (at(day, 10, 0), at(day, 11, 0));
    let one = mailbox
        .add_event(Event { title: "Block A item".into(), start: nine, end: nine.add_minutes(30), calendar: own, ..Default::default() })
        .expect("the first calendar takes an event");
    let other = mailbox
        .add_event(Event { title: "Block B item".into(), start: ten, end: ten.add_minutes(30), calendar: there, ..Default::default() })
        .expect("the second calendar takes an event");
    let anchor = at(day, 11, 30);
    mailbox
        .add_event(Event { title: "Slot anchor".into(), start: anchor, end: anchor.add_minutes(30), calendar: there, ..Default::default() })
        .expect("the second calendar takes an event");
    let mut w = window_on(mailbox, "view.calendar");
    ready(&mut w);

    // (A click first: the calendar is what the keys are for.)
    let point = drawn(&w, "Block A item").center();
    click(&mut w, point, Modifiers::NONE, PointerButton::Primary);
    // The set, as a Command click of each leaves it, copied by the key that copies.
    select(&mut w, &[(one, None), (other, None)]);
    chord(&mut w, COMMAND, egui::Key::C);
    let clip = {
        let ctx = w.ctx.clone();
        ctx.data(|data| data.get_temp::<Clip>(egui::Id::unique(CLIP_ID))).expect("what Copy put on the clipboard")
    };
    // The pick the pointer makes in the second block, and the calendar it is into.
    let picked = Slot::timed(day, 15 * 60, 30).in_calendar(there);
    assert_eq!(
        paste_calendar(w.state(), &clip, &picked),
        Some(there),
        "a set of two blocks goes into the calendar its time was picked in, though one of its items is already there"
    );

    // And the paste itself, at 15:00 in the second block.
    let somewhere = at_three(&w, "Slot anchor");
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    let came: Vec<Event> = titled(&w, "Block A item").into_iter().filter(|event| event.calendar == there).collect();
    assert_eq!(came.len(), 1, "the item of the other block came into this one: {:?}", titled(&w, "Block A item"));
    assert_eq!(
        (came[0].start, came[0].end),
        (at(day, 15, 0), at(day, 15, 30)),
        "the earliest item lands where the time was picked, and keeps its length"
    );
    let stayed: Vec<Event> = titled(&w, "Block B item").into_iter().filter(|event| event.calendar == there).collect();
    let copy = stayed.iter().find(|event| event.start == at(day, 16, 0)).expect("the second member, an hour after the first");
    assert_eq!((copy.start, copy.end), (at(day, 16, 0), at(day, 16, 30)), "the other keeps its offset from the one that was anchored");
    // One step takes both copies away.
    assert!(w.state_mut().session.undo(), "a paste of a set is one step");
    w.run_steps(2);
    assert_eq!(titled(&w, "Block A item").len(), 1, "the copy of the first went");
    assert_eq!(titled(&w, "Block B item").len(), 1, "and the copy of the second");
}

/// A Shift click that goes backwards has the focus on the occurrence it landed on, and takes the
/// range that is drawn between the two — not everything there is.
///
/// The range is built in the order of the items' times, so the item the range ends at is the
/// latest of them; a focus taken from the end of that list is the item clicked first, and the
/// next key that acts on the focus then acts on the wrong occurrence.
#[test]
fn a_backward_shift_click_focuses_what_it_landed_on_and_stays_in_view() {
    let mut w = week();
    let lunch = event(&w, "Lunch with Dev");
    let release = event(&w, "Write release notes");
    let standup = event(&w, "Team stand-up");
    let roadmap = event(&w, "Q4 roadmap");
    assert!(release.start < lunch.start, "fixture: the one clicked last is the earlier of the two");

    let point = drawn(&w, "Lunch with Dev").center();

    click(&mut w, point, Modifiers::NONE, PointerButton::Primary);
    let point = drawn(&w, "Write release notes").center();
    click(&mut w, point, SHIFT, PointerButton::Primary);

    assert_eq!(
        focused(&w),
        (Some(release.id), Some(date(10, 13))),
        "the occurrence the Shift click landed on has the focus, not the one the range started at"
    );
    let set = selected(&w);
    let has = |id: Id| set.iter().any(|(member, _)| *member == id);
    assert!(has(release.id) && has(lunch.id), "both ends are in the range: {set:?}");
    assert!(has(event(&w, "Design review").id), "and the days between them: {set:?}");
    assert!(set.contains(&(standup.id, Some(date(10, 14)))), "an occurrence of a series is a member by its own day: {set:?}");
    // Bounded by what is drawn between the two clocks: nothing before the one it landed on, and
    // nothing after the one it came from.
    assert!(!set.contains(&(standup.id, Some(date(10, 12)))), "Monday's stand-up is before the range: {set:?}");
    assert!(!has(roadmap.id), "an item after the range is not in it: {set:?}");
}

/// A copy is of the event it was taken from, even when the mailbox gave that event's id to
/// another appointment. Command-created ids remain monotonic across Undo, so the model's public
/// retained-id import path constructs that replacement explicitly; a paste must not copy it.
///
/// The mailbox is the same one throughout (`Session::openings` is what a clipboard of another
/// mailbox is told by, and it does not change here), so what the clipboard holds has to be told
/// from the appointment that stands in the place of what it was taken from.
#[test]
fn a_copy_whose_id_was_given_to_another_appointment_is_not_pasted() {
    let mut w = week();
    let made = |w: &mut Window, title: &str, from: &str, to: &str| -> Id {
        let answer =
            w.state_mut().execute("calendar.new", json!({ "title": title, "start": from, "end": to })).expect("the window makes an appointment");
        answer["id"].as_u64().expect("the answer names the appointment")
    };
    let holder = made(&mut w, "Holder", "2026-10-14T13:00", "2026-10-14T13:30");
    w.run_steps(3); // The new appointment must be painted before its hit rectangle is read.
    // (A click first: the calendar is what the keys are for.)
    let point = drawn(&w, "Holder").center();
    click(&mut w, point, Modifiers::NONE, PointerButton::Primary);
    select(&mut w, &[(holder, None)]);
    chord(&mut w, COMMAND, egui::Key::C);
    let opening = w.state().session.openings();

    // The appointment goes with the step undone. The model also accepts a retained id when it
    // is absent, as imported records can: exercise that independently of the monotonic allocator.
    assert!(w.state_mut().session.undo(), "a new appointment is a step of its own");
    w.run_steps(2);
    assert!(w.state().session.mailbox.event(holder).is_none(), "the appointment went with the step undone");
    let other = w
        .state_mut()
        .session
        .mailbox
        .add_event(Event {
            id: holder,
            title: "Something else".into(),
            uid: "replacement@example.org".into(),
            start: at(date(10, 14), 16, 0),
            end: at(date(10, 14), 16, 30),
            ..Event::default()
        })
        .expect("the model accepts a retained id that is no longer present");
    assert_eq!(other, holder, "fixture: the retained id now names a different appointment");
    assert_eq!(opening, w.state().session.openings(), "fixture: it is the same mailbox the copy was taken in");
    let events = w.state().session.mailbox.events.len();
    // (What Ctrl+C put on the clipboard is a copy of what the id named then: a stale copy, not an
    // empty clipboard, is what Paste is to refuse.)
    let held = w.ctx.data(|d| d.get_temp::<Clip>(egui::Id::unique(CLIP_ID))).expect("Ctrl+C put the copy on the calendar's clipboard");
    assert!(!held.cut && held.items.iter().map(|item| item.id).eq([holder]), "a copy of what the id named then: {:?}", held.items);

    // Paste where the sample's review ends: nothing comes of a copy whose event is gone.
    let somewhere = at_three(&w, "Design review");
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    w.state_mut().status_msg = None;
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    let said = w.state().status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default();
    assert!(said.contains("copy it again"), "Paste says the copy is no longer what was copied: {said:?}");
    assert_eq!(titled(&w, "Something else").len(), 1, "a copy of what holds the id now was made: {:?}", titled(&w, "Something else"));
    assert_eq!(w.state().session.mailbox.events.len(), events, "nothing came of a copy of an event that is gone");
    assert_eq!(
        w.state().session.mailbox.event(other).map(|event| event.title.clone()),
        Some("Something else".to_string()),
        "and what holds the id was left as it was"
    );
}

/// An item's own menu does to the whole set what its entries say, once each: Copy, Cut and
/// Delete, each on the set the menu was opened over, and each one step to undo.
#[test]
fn the_item_menu_copies_cuts_and_deletes_the_whole_set_once() {
    let mut w = week();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    let set = [(lunch.id, None), (dentist.id, None)];

    // Copy: both members go on the clipboard, and a Paste puts both where a time is picked.
    select(&mut w, &set);
    let point = drawn(&w, "Lunch with Dev").center();
    click(&mut w, point, Modifiers::NONE, PointerButton::Secondary);
    assert_eq!(selected(&w), set, "the menu was opened over the set, which it keeps");
    choose_in_menu(&mut w, "Copy");
    assert!(!egui::Popup::is_any_open(&w.ctx), "choosing an entry closes the menu");
    let somewhere = at_three(&w, "Design review");
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!(titled(&w, "Lunch with Dev").len(), 2, "the first member of the set came");
    assert_eq!(titled(&w, "Dentist").len(), 2, "and the second");
    assert!(w.state_mut().session.undo(), "a paste of a set is one step");
    w.run_steps(2);
    assert_eq!((titled(&w, "Lunch with Dev").len(), titled(&w, "Dentist").len()), (1, 1), "one step took both copies");

    // Cut: the same set moves, both members, and one step puts both back.
    select(&mut w, &set);
    let point = drawn(&w, "Dentist").center();
    click(&mut w, point, Modifiers::NONE, PointerButton::Secondary);
    choose_in_menu(&mut w, "Cut");
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    let moved =
        (w.state().session.mailbox.event(lunch.id).map(|event| event.start), w.state().session.mailbox.event(dentist.id).map(|event| event.start));
    assert_eq!(moved.0, Some(at(date(10, 14), 15, 0)), "the earliest member lands where the time was picked");
    let spacing = lunch.start.minutes_until(dentist.start);
    assert_eq!(moved.1, Some(at(date(10, 14), 15, 0).add_minutes(spacing)), "and the second keeps its time after the first: {moved:?}");
    assert!(w.state_mut().session.undo(), "a cut and paste of a set is one step");
    w.run_steps(2);
    assert_eq!(w.state().session.mailbox.event(lunch.id).map(|event| event.start), Some(lunch.start), "one step puts them back");
    assert_eq!(w.state().session.mailbox.event(dentist.id).map(|event| event.start), Some(dentist.start));

    // Delete: the set itself, in one step, and one step back.
    select(&mut w, &set);
    let point = drawn(&w, "Lunch with Dev").center();
    click(&mut w, point, Modifiers::NONE, PointerButton::Secondary);
    choose_in_menu(&mut w, "Delete");
    w.run_steps(3);
    assert!(w.state().session.mailbox.event(lunch.id).is_none(), "the first member went");
    assert!(w.state().session.mailbox.event(dentist.id).is_none(), "and the second");
    assert!(w.state_mut().session.undo(), "a Delete of a set is one step");
    w.run_steps(2);
    assert!(
        w.state().session.mailbox.event(lunch.id).is_some() && w.state().session.mailbox.event(dentist.id).is_some(),
        "one step brings both back"
    );
}

/// Delete of one occurrence of a series, from the item's menu, is about that occurrence: the
/// series stays, only the day that was clicked leaves it, and one step puts it back.
#[test]
fn deleting_one_occurrence_from_the_menu_leaves_the_series_alone() {
    let mut w = week();
    let standup = event(&w, "Team stand-up");
    let day = date(10, 14);
    let on_the_day = occurrence(&w, standup.id, day);
    click(&mut w, on_the_day.center(), Modifiers::NONE, PointerButton::Secondary);
    choose_in_menu(&mut w, "Delete");

    assert_eq!(dialog(&w), Some("confirm"), "which of the series is meant is asked before anything goes");
    assert_eq!(question(&w).as_deref(), Some("Delete Recurring Item"), "and the question is the one about a series");
    choose_button(&mut w, "Just This One");

    let after = w.state().session.mailbox.event(standup.id).cloned().expect("the series is still there");
    assert!(after.recurrence.is_some(), "the series still repeats");
    assert_eq!(after.exceptions, vec![day], "only the occurrence that was clicked left the series");
    assert!(w.state().hit.rect(&occurrence_hit(standup.id, day), 0).is_none(), "and it is not drawn any more");
    assert!(w.state().hit.rect(&occurrence_hit(standup.id, day.add_days(1)), 0).is_some(), "while the day after it is");
    assert!(w.state_mut().session.undo(), "a Delete of one occurrence is one step");
    w.run_steps(2);
    assert!(
        w.state().session.mailbox.event(standup.id).is_some_and(|series| series.exceptions.is_empty()),
        "one step puts the occurrence back in the series"
    );
}

/// A same-day time change must retain the occurrence identity even though picking the slot
/// clears focus. This is separate from the zoned cross-week fixture above.
#[test]
fn a_same_day_cut_moves_only_the_occurrence_that_was_cut() {
    let mut w = week();
    let standup = event(&w, "Team stand-up");
    let day = date(10, 14);
    let before = (*standup).clone();
    let point = occurrence(&w, standup.id, day).center();
    click(&mut w, point, Modifiers::NONE, PointerButton::Primary);
    assert_eq!(selected(&w), [(standup.id, Some(day))]);
    chord(&mut w, COMMAND, egui::Key::X);
    let point = at_three(&w, "Design review");
    click(&mut w, point, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!(
        w.state().session.mailbox.event(standup.id).map(|event| event.start),
        Some(before.start),
        "one selected occurrence must never shift the whole series"
    );
    if w.state().dialog.is_some() {
        assert_eq!(question(&w).as_deref(), Some("Move Recurring Item"));
        choose_button(&mut w, "Just This One");
    }
    let after = w.state().session.mailbox.event(standup.id).unwrap();
    assert_eq!(after.exceptions, vec![day]);
    let moved: Vec<Event> = titled(&w, "Team stand-up").into_iter().filter(|event| event.recurrence.is_none()).collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved.first().unwrap().start, at(day, 15, 0));
    assert!(w.state_mut().session.undo());
    w.run_steps(2);
    assert_eq!(w.state().session.mailbox.event(standup.id).unwrap().exceptions, before.exceptions);
    assert_eq!(titled(&w, "Team stand-up").len(), 1);
}

/// Open in an item's menu opens the item the menu was opened over, as a double click on it does,
/// not the member of the set that happens to have the focus (CodeRabbit on #1665,
/// calendar_view.rs:2437).
#[test]
fn open_in_the_item_menu_opens_the_item_it_was_opened_over() {
    let mut w = week();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    assert_eq!(focused(&w).0, Some(dentist.id), "fixture: the other member has the focus");
    let point = drawn(&w, "Lunch with Dev").center();
    click(&mut w, point, Modifiers::NONE, PointerButton::Secondary);
    assert_eq!(selected(&w).len(), 2, "fixture: the menu was opened over the set, which it keeps");
    choose_in_menu(&mut w, "Open");
    w.run_steps(3);
    assert_eq!(focused(&w).0, Some(lunch.id), "Open is about the item the menu was opened over");
    assert!(w.state().windows.of_event(lunch.id).is_some(), "Lunch's own window opened");
    assert!(w.state().windows.of_event(dentist.id).is_none(), "and not Dentist's");
}

// ---- F6: one cut day of a series that repeats by a zone of its own ----

/// How the owner cuts and pastes in [`cut_and_paste_at_noon`]: the grid's keys, or the ribbon's Cut and
/// Paste (`ui.calendarCut`, then `ui.calendarPaste` run as the owner's own command).
#[derive(Clone, Copy, Debug)]
enum ZonedRoute {
    Keys,
    Ribbon,
}

/// What the owner answers the question with.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ZonedAnswer {
    JustThisOne,
    EntireSeries,
    Cancel,
}

/// A window in New York's clock with "London sync", every Thursday at 14:00 by London's clock
/// from 3 September 2026 (09:00 in New York), and an appointment from 11:00 to 11:30 on `day` to
/// read the 12:00 row of that day off. Answers the window and the series' id.
fn london_series(day: Date) -> (Window, Id) {
    let mut mailbox = mailcraft_engine::sample::mailbox(now(), 0);
    let calendar = mailbox.default_calendar().expect("the sample has a calendar");
    let first = at(date(9, 3), 9, 0);
    let series = mailbox
        .add_event(Event {
            title: "London sync".into(),
            start: first,
            end: first.add_minutes(30),
            start_zone: "Europe/London".into(),
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            calendar,
            ..Default::default()
        })
        .expect("the calendar takes a series");
    let anchor = at(day, 11, 0);
    mailbox
        .add_event(Event { title: "Noon anchor".into(), start: anchor, end: anchor.add_minutes(30), calendar, ..Default::default() })
        .expect("the calendar takes an event");
    let mut w = window_in(mailbox, "America/New_York");
    let _ = w.state_mut().execute("view.goToDate", json!({ "date": day.iso() }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
    (w, series)
}

/// The middle of the 12:00 row of the day "Noon anchor" (11:00 to 11:30) is drawn on.
fn at_noon(w: &Window) -> Pos2 {
    let block = drawn(w, "Noon anchor");
    egui::pos2(block.center().x, block.max.y + 0.5 * HOUR_H + 6.0)
}

/// The "London sync" days that are events of their own now, as (start, end), earliest first.
fn london_on_their_own(w: &Window) -> Vec<(DateTime, DateTime)> {
    titled(w, "London sync").into_iter().filter(|event| event.recurrence.is_none()).map(|event| (event.start, event.end)).collect()
}

/// Where the day `day` of `series` starts in the window's clock.
fn starts_on(w: &Window, series: &Event, day: Date) -> DateTime {
    let here = |instant: Timestamp| w.state().session.offset_at(instant);
    series.occurrence_times(day, &here).0
}

/// Click the day `day` of "London sync", cut it by `route`, pick 12:00 on that same day and paste
/// it by `route`. Answers the series as it was before the cut.
fn cut_and_paste_at_noon(w: &mut Window, series: Id, day: Date, route: ZonedRoute) -> Event {
    let spot = occurrence(w, series, day).center();
    click(w, spot, Modifiers::NONE, PointerButton::Primary);
    assert_eq!(selected(w), [(series, Some(day))], "fixture: a plain click makes a set of one");
    let before = w.state().session.mailbox.event(series).map(|event| (*event).clone()).expect("fixture: the series is there");
    match route {
        ZonedRoute::Keys => chord(w, COMMAND, egui::Key::X),
        ZonedRoute::Ribbon => {
            w.state_mut().run("ui.calendarCut", json!({})).expect("the ribbon's Cut takes the day");
            w.run_steps(2);
        }
    }
    let noon = at_noon(w);
    click(w, noon, Modifiers::NONE, PointerButton::Primary);
    match route {
        ZonedRoute::Keys => chord(w, COMMAND, egui::Key::V),
        ZonedRoute::Ribbon => {
            let said = w.state_mut().run("ui.calendarPaste", json!({}));
            assert_eq!(said.as_ref().ok().and_then(|said| said.get("pending")).and_then(|p| p.as_str()), Some("confirm"), "{said:?}");
        }
    }
    w.run_steps(3);
    before
}

/// The series `before` moved whole as main's Cut and Paste moved it (`calendar.move`, `"scope":
/// "series"`): by as much as the day that was cut, read from the series' own first time of day
/// on the day it is shown (main's clipboard kept `DateTime { date: shown, ..series.start }`), to
/// the time picked. Run on the mailbox as it was, in a session of the window's clock.
fn as_main_moved_it(mailbox: &mailcraft_model::Mailbox, before: &Event, shown: Date, picked: DateTime) -> Event {
    let zone = Zone::named("America/New_York").expect("a zone of the bundled database");
    let mut main = mailcraft_engine::Session::in_zone(mailbox.clone(), now(), zone);
    let by = DateTime { date: shown, ..before.start }.minutes_until(picked);
    let start = crate::calendar_view::iso_minute(before.start.add_minutes(by));
    main.run("calendar.move", &json!({ "id": before.id, "start": start, "scope": "series" })).expect("main's move of the whole series");
    main.mailbox.event(before.id).map(|event| (*event).clone()).expect("the series is there")
}

/// One day of a series that repeats by London's clock, seen in New York's, cut and pasted at
/// another time of the same day by the owner (the keys, the ribbon), is asked about as every
/// other route asks about one day of a series: "Move Recurring Item" (F6 on #1665). Nothing
/// moves until it is answered. "Just This One" pastes that day by `calendar.paste`, which puts it
/// at the time picked, also in the week between the change of the clocks in London (25 October)
/// and in New York (1 November); "The Entire Series" moves the series as main's Cut and Paste
/// did; Cancel changes nothing and keeps the cut. Each answer is one Undo.
#[test]
fn one_cut_day_of_a_zoned_series_pasted_on_its_day_asks_which_and_lands_where_asked() {
    // 15 October: summer time on both sides (14:00 in London is 09:00 here); 29 October: London
    // is on winter time and New York still on summer time (14:00 in London is 10:00 here).
    for (day, shown_at) in [(date(10, 15), 9), (date(10, 29), 10)] {
        for route in [ZonedRoute::Keys, ZonedRoute::Ribbon] {
            for answer in [ZonedAnswer::JustThisOne, ZonedAnswer::EntireSeries, ZonedAnswer::Cancel] {
                let case = format!("{} by {route:?}, {answer:?}", day.iso());
                let (mut w, series) = london_series(day);
                let first = w.state().session.mailbox.event(series).map(|event| (*event).clone()).expect("fixture: the series");
                assert_eq!(starts_on(&w, &first, day), at(day, shown_at, 0), "{case}: fixture: where that day is shown");
                let mailbox = w.state().session.mailbox.clone();
                let before = cut_and_paste_at_noon(&mut w, series, day, route);
                assert_eq!(question(&w).as_deref(), Some("Move Recurring Item"), "{case}: the series question is asked");
                assert!(w.state().session.mailbox == mailbox, "{case}: nothing moves until it is answered");
                let picked = at(day, 12, 0);
                match answer {
                    ZonedAnswer::Cancel => {
                        choose_button(&mut w, "Cancel");
                        assert!(question(&w).is_none(), "{case}: the question is gone");
                        assert!(w.state().session.mailbox == mailbox, "{case}: nothing changed");
                        // The cut is still on the clipboard: a time picked again and a paste ask again.
                        let noon = at_noon(&w);
                        click(&mut w, noon, Modifiers::NONE, PointerButton::Primary);
                        chord(&mut w, COMMAND, egui::Key::V);
                        w.run_steps(3);
                        assert_eq!(question(&w).as_deref(), Some("Move Recurring Item"), "{case}: the cut is kept, to be pasted again");
                        continue;
                    }
                    ZonedAnswer::JustThisOne => {
                        choose_button(&mut w, "Just This One");
                        let after = w.state().session.mailbox.event(series).map(|event| (*event).clone()).expect("the series stays");
                        assert_eq!(after.exceptions, vec![day], "{case}: only that day left the series");
                        assert_eq!((after.start, after.end), (before.start, before.end), "{case}: the series is where it was");
                        assert_eq!(london_on_their_own(&w), [(picked, picked.add_minutes(30))], "{case}: and that day is at the time picked");
                    }
                    ZonedAnswer::EntireSeries => {
                        choose_button(&mut w, "The Entire Series");
                        let after = w.state().session.mailbox.event(series).map(|event| (*event).clone()).expect("the series stays");
                        assert!(after.exceptions.is_empty() && london_on_their_own(&w).is_empty(), "{case}: the series moves whole");
                        let main = as_main_moved_it(&mailbox, &before, day, picked);
                        assert_eq!(
                            (after.start, after.end, &after.start_zone, &after.recurrence),
                            (main.start, main.end, &main.start_zone, &main.recurrence),
                            "{case}: as main's Cut and Paste moved it"
                        );
                        // Every Thursday at 12:00 here, but for the week the clocks disagree, which
                        // reads an hour later, as it did on main: the series moves by its own
                        // time of day, not by that week's.
                        assert_eq!(starts_on(&w, &after, date(10, 22)), at(date(10, 22), 12, 0), "{case}: a week of summer time on both sides");
                        assert_eq!(starts_on(&w, &after, date(11, 5)), at(date(11, 5), 12, 0), "{case}: a week of winter time on both sides");
                        let lands = if shown_at == 9 { 12 } else { 13 };
                        assert_eq!(starts_on(&w, &after, day), at(day, lands, 0), "{case}: that day, as on main");
                    }
                }
                assert!(question(&w).is_none(), "{case}: nothing more is asked of a local series");
                assert!(w.state_mut().session.undo(), "{case}: one step");
                w.run_steps(2);
                assert_eq!(w.state().session.mailbox.event(series).map(|event| (*event).clone()), Some(before), "{case}: one Undo puts it back");
                assert!(london_on_their_own(&w).is_empty(), "{case}: and takes the day of its own away");
            }
        }
    }
}

/// The control channel's Paste (`ui.calendarPaste` by `execute`, the route that never asks) of
/// one cut day of a series that repeats by a zone of its own stays as it was: no question, just
/// that day, at the time picked, by `calendar.paste` (F6 on #1665 changes the owner's routes only).
#[test]
fn the_control_channels_paste_of_one_cut_day_of_a_zoned_series_never_asks() {
    for day in [date(10, 15), date(10, 29)] {
        let (mut w, series) = london_series(day);
        let spot = occurrence(&w, series, day).center();
        click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
        let before = w.state().session.mailbox.event(series).map(|event| (*event).clone()).expect("fixture: the series");
        w.state_mut().execute("ui.calendarCut", json!({})).expect("the control channel's Cut takes the day");
        let noon = at_noon(&w);
        click(&mut w, noon, Modifiers::NONE, PointerButton::Primary);
        let said = w.state_mut().execute("ui.calendarPaste", json!({})).expect("the control channel's Paste goes through");
        w.run_steps(3);
        assert_eq!(said, json!({ "items": 1, "cut": true }), "{}: it answers counts only", day.iso());
        assert!(question(&w).is_none(), "{}: it never asks", day.iso());
        let after = w.state().session.mailbox.event(series).map(|event| (*event).clone()).expect("the series stays");
        assert_eq!(after.exceptions, vec![day], "{}: just that day", day.iso());
        assert_eq!(after.start, before.start, "{}: the series is where it was", day.iso());
        let picked = at(day, 12, 0);
        assert_eq!(london_on_their_own(&w), [(picked, picked.add_minutes(30))], "{}: at the time picked", day.iso());
    }
}
