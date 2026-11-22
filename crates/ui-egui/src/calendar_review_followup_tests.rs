// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a review of #1663 (CodeRabbit on #1665) found the window's own selection gestures did: a
//! Shift click took items the view doesn't draw (those behind a day's `+n more`, in the Month and
//! in the row of whole days), a range with a cancelled appointment in it was refused whole, and
//! Tab went on from the first member of a set rather than from the one in focus.
//!
//! One module of `calendar_selection_tests`, so `super::*` is that file's fixtures (`calendar`,
//! `event`, `select`, `selected`, `focused`, `click`, `chord`, `date`, `at`, `SHIFT`, `Window`,
//! `Event`, `Id`, `Modifiers`, `PointerButton`). The window is the real one; the events are the
//! sample's invented ones and appointments added through `Mailbox::add_event`; every act is a
//! pointer event or a key. No file, server, real account or wall clock.

use serde_json::json;

use super::*;

/// Add `title` at `start`, `minutes` long (a whole day when `all_day`), to the sample's calendar.
fn add(w: &mut Window, title: &str, start: DateTime, minutes: i64, all_day: bool) -> Id {
    let calendar = event(w, "Design review").calendar;
    let event = Event { calendar, title: title.into(), start, end: start.add_minutes(minutes), all_day, ..Default::default() };
    let id = w.state_mut().session.mailbox.add_event(event).expect("the sample's calendar takes an appointment");
    w.run_steps(3);
    id
}

/// Whether the view drew `id` this frame (a piece of it has a hit entry).
fn is_drawn(w: &Window, id: Id) -> bool {
    w.state().hit.rect("event", id).is_some()
}

/// The members of the selection that the view doesn't draw.
fn undrawn(w: &Window) -> Vec<(Id, Option<Date>)> {
    selected(w).into_iter().filter(|(id, _)| !is_drawn(w, *id)).collect()
}

/// Shift-click from `from` to `to` in the view in front, and the members that weren't drawn.
fn shift_range(w: &mut Window, from: Id, to: Id) -> Vec<(Id, Option<Date>)> {
    let start = w.state().hit.rect("event", from).expect("fixture: the range's first item is drawn").center();
    click(w, start, Modifiers::NONE, PointerButton::Primary);
    let end = w.state().hit.rect("event", to).expect("fixture: the range's last item is drawn").center();
    click(w, end, SHIFT, PointerButton::Primary);
    let set = selected(w);
    assert!(set.iter().any(|(id, _)| *id == from) && set.iter().any(|(id, _)| *id == to), "fixture: the range was taken: {set:?}");
    undrawn(w)
}

/// In the Month, a Shift click takes the chips that are drawn between its ends and none of those
/// a day keeps behind its `+n more`: a Delete of the set must never reach what the owner didn't
/// see selected.
#[test]
fn a_month_shift_click_takes_only_the_chips_it_draws() {
    let mut w = calendar();
    let _ = w.state_mut().execute("view.month", json!({}));
    w.run_steps(3);
    let first = add(&mut w, "Early Tuesday", at(date(10, 13), 5, 0), 15, false);
    let crowd: Vec<Id> = (0..14i64).map(|n| add(&mut w, &format!("Crowd {n}"), at(date(10, 14), 6, 0).add_minutes(n), 1, false)).collect();
    let last = add(&mut w, "Early Thursday", at(date(10, 15), 5, 0), 15, false);
    assert!(crowd.iter().any(|id| !is_drawn(&w, *id)), "fixture: Wednesday keeps some of its crowd behind +n more");
    assert!(crowd.iter().any(|id| is_drawn(&w, *id)), "fixture: and draws some of it");

    let hidden = shift_range(&mut w, first, last);
    assert!(hidden.is_empty(), "a Shift click took items the Month doesn't draw: {hidden:?}");
    assert!(selected(&w).iter().any(|(id, _)| crowd.contains(id)), "the crowd's drawn chips are in the range");
}

/// The same in the row of whole days over the hours: bars beyond the row's lanes are counted as
/// `+n more`, and a Shift click over them doesn't take them.
#[test]
fn a_shift_click_over_the_whole_day_row_takes_only_the_bars_it_draws() {
    let mut w = calendar();
    let first = add(&mut w, "Tuesday off", at(date(10, 13), 0, 0), 24 * 60, true);
    let crowd: Vec<Id> = (0..7).map(|n| add(&mut w, &format!("Whole day {n}"), at(date(10, 14), 0, 0), 24 * 60, true)).collect();
    let last = add(&mut w, "Thursday off", at(date(10, 15), 0, 0), 24 * 60, true);
    assert!(crowd.iter().any(|id| !is_drawn(&w, *id)), "fixture: the row keeps some of Wednesday's bars behind +n more");

    let hidden = shift_range(&mut w, first, last);
    assert!(hidden.is_empty(), "a Shift click took bars the row doesn't draw: {hidden:?}");
}

/// A cancelled appointment between a Shift click's ends is left out of the range, as Select All
/// leaves it out, rather than making the whole range refused.
#[test]
fn a_shift_click_over_a_cancelled_appointment_takes_the_rest() {
    let mut w = calendar();
    let calendar = event(&w, "Design review").calendar;
    let start = at(date(10, 14), 13, 0);
    let off = Event { calendar, title: "Called off".into(), start, end: start.add_minutes(30), cancelled: true, ..Default::default() };
    let off = w.state_mut().session.mailbox.add_event(off).expect("the sample's calendar takes it");
    w.run_steps(3);
    assert!(is_drawn(&w, off), "fixture: the cancelled appointment is drawn, struck through");
    let (release, dentist) = (event(&w, "Write release notes").id, event(&w, "Dentist").id);
    let from = w.state().hit.rect("event", release).expect("drawn").center();
    click(&mut w, from, Modifiers::NONE, PointerButton::Primary);
    let to = w.state().hit.rect("event", dentist).expect("drawn").center();
    click(&mut w, to, SHIFT, PointerButton::Primary);
    let set = selected(&w);
    assert!(set.iter().any(|(id, _)| *id == dentist), "the range was taken, though a cancelled appointment is in it: {set:?}");
    assert!(!set.iter().any(|(id, _)| *id == off), "and the cancelled one isn't in it: {set:?}");
}

/// Tab goes on from the member of the set that has the focus, not from the first one listed.
#[test]
fn tab_goes_on_from_the_focused_member_of_a_set() {
    let mut w = calendar();
    let saturday = date(10, 17);
    let ids: Vec<Id> = ["Tab A", "Tab B", "Tab C", "Tab D"]
        .iter()
        .zip(0i64..)
        .map(|(title, n)| add(&mut w, title, at(saturday, 5, 0).add_minutes(n), 1, false))
        .collect();
    select(&mut w, &[(ids[0], None), (ids[1], None), (ids[2], None)]);
    assert_eq!(focused(&w).0, Some(ids[2]), "fixture: the last member has the focus");
    chord(&mut w, Modifiers::NONE, egui::Key::Tab);
    assert_eq!(focused(&w).0, Some(ids[3]), "Tab goes from the focused member to the next item");
}

/// A plain click on a cancelled appointment selects it as it did before typed sets, as the one
/// item in focus, so that it can still be opened or deleted from the calendar: it is never refused
/// with a message for being clicked.
#[test]
fn a_plain_click_on_a_cancelled_appointment_selects_it() {
    let mut w = calendar();
    let calendar = event(&w, "Design review").calendar;
    let start = at(date(10, 14), 13, 0);
    let off = Event { calendar, title: "Called off".into(), start, end: start.add_minutes(30), cancelled: true, ..Default::default() };
    let off = w.state_mut().session.mailbox.add_event(off).expect("the sample's calendar takes it");
    w.run_steps(3);
    let on_it = w.state().hit.rect("event", off).expect("fixture: the cancelled appointment is drawn").center();
    click(&mut w, on_it, Modifiers::NONE, PointerButton::Primary);
    assert_eq!(crate::people_view::testing::dialog(&w), None, "a click is never refused with a message");
    assert_eq!(focused(&w), (Some(off), Some(date(10, 14))), "the cancelled appointment is the one in focus");
    assert!(selected(&w).is_empty(), "and no typed set holds it");
    chord(&mut w, Modifiers::NONE, egui::Key::Delete);
    w.run_steps(3);
    assert!(w.state().session.mailbox.event(off).is_none(), "Delete takes it out of the calendar, as before");
}

/// A Command (Ctrl) click on a cancelled appointment selects it as a plain click does, and another
/// takes it out again: it is never refused with a message (review of #1665).
#[test]
fn a_command_click_on_a_cancelled_appointment_selects_it_and_another_deselects_it() {
    let mut w = calendar();
    let calendar = event(&w, "Design review").calendar;
    let start = at(date(10, 14), 13, 0);
    let off = Event { calendar, title: "Called off".into(), start, end: start.add_minutes(30), cancelled: true, ..Default::default() };
    let off = w.state_mut().session.mailbox.add_event(off).expect("the sample's calendar takes it");
    w.run_steps(3);
    let on_it = w.state().hit.rect("event", off).expect("fixture: the cancelled appointment is drawn").center();
    click(&mut w, on_it, COMMAND, PointerButton::Primary);
    assert_eq!(crate::people_view::testing::dialog(&w), None, "a Command click is never refused with a message");
    assert_eq!(focused(&w), (Some(off), Some(date(10, 14))), "the cancelled appointment is the one in focus");
    assert!(selected(&w).is_empty(), "and no typed set holds it");
    click(&mut w, on_it, COMMAND, PointerButton::Primary);
    assert_eq!(crate::people_view::testing::dialog(&w), None, "nor is the second");
    assert_eq!(focused(&w).0, None, "which takes it out of the selection");
}

/// A Command (Ctrl) click on a cancelled appointment while a set is selected leaves the set as it
/// is, as a Command click never takes away what else is selected: it adds or removes the one item
/// it lands on, and a cancelled appointment can't be one of a set (a Shift click's range leaves it
/// out too). The status line says why it wasn't added, and nothing is refused with a message
/// (review of #1665). With no set selected it is selected alone, as a plain click selects it.
#[test]
fn a_command_click_on_a_cancelled_appointment_keeps_the_selected_set() {
    let mut w = calendar();
    let lunch = add(&mut w, "Team lunch", at(date(10, 14), 15, 0), 30, false);
    let calendar = event(&w, "Design review").calendar;
    let start = at(date(10, 14), 13, 0);
    let off = Event { calendar, title: "Called off".into(), start, end: start.add_minutes(30), cancelled: true, ..Default::default() };
    let off = w.state_mut().session.mailbox.add_event(off).expect("the sample's calendar takes it");
    w.run_steps(3);
    let review = event(&w, "Design review").id;
    let point = |w: &Window, id: Id| w.state().hit.rect("event", id).expect("fixture: the appointment is drawn").center();
    let (on_review, on_lunch, on_off) = (point(&w, review), point(&w, lunch), point(&w, off));
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    click(&mut w, on_lunch, COMMAND, PointerButton::Primary);
    let set = selected(&w);
    assert_eq!(set.len(), 2, "fixture: a set of two is selected: {set:?}");
    let before = focused(&w);
    w.state_mut().status_msg = None;
    click(&mut w, on_off, COMMAND, PointerButton::Primary);
    assert_eq!(crate::people_view::testing::dialog(&w), None, "a Command click is never refused with a message");
    assert_eq!(selected(&w), set, "the set stays as it was");
    assert_eq!(focused(&w), before, "and so does the focus");
    let said = w.state().status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default();
    assert!(said.contains("Called off") && said.contains("cancelled"), "the status line says why: {said:?}");
}
