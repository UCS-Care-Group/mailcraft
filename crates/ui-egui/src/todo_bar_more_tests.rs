// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The To-Do Bar's height shared out as Outlook shares it (the tasks take what is left and scroll
//! inside it, the navigator stacks months when there is room), and appointments and contacts
//! dragged onto its tasks to become tasks (#831).

use egui::{Event, Modifiers, PointerButton, Pos2};
use mailcraft_engine::{Module, TodoParts};
use mailcraft_model::{Contact, Task};
use serde_json::json;

use super::*;
use crate::people_view::testing::{self, Window};

fn parts(calendar: bool, people: bool, tasks: bool) -> TodoParts {
    TodoParts { calendar, people, tasks }
}

fn window_of(mailbox: mailcraft_model::Mailbox, module: &str, height: f32) -> Window {
    let mut w = testing::window_sized(mailbox, module, egui::vec2(1280.0, height));
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.run_steps(2);
    w
}

fn sample() -> mailcraft_model::Mailbox {
    mailcraft_engine::sample::mailbox(testing::now(), 0)
}

fn bar(w: &mut Window, on: &[&str]) {
    for part in on {
        w.state_mut().execute("view.todoBar", json!({"part": part, "value": true})).unwrap();
    }
    w.run_steps(4);
}

fn hit(w: &Window, kind: &str, id: u64) -> Option<Rect> {
    w.state().hit.rect(kind, id)
}

/// How many months the navigator shows.
fn months(w: &Window) -> u8 {
    (0..MAX_MONTHS).take_while(|k| hit(w, "todoMonth", u64::from(*k)).is_some()).count() as u8
}

/// A drag with the main button from `from` to `to`, a frame for each step, let go at `to`.
fn drag(w: &mut Window, from: Pos2, to: Pos2) {
    w.run_steps(45);
    w.input_mut().events.push(Event::PointerMoved(from));
    w.step();
    w.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    w.step();
    for i in 1..=12u8 {
        w.input_mut().events.push(Event::PointerMoved(from + (to - from) * (f32::from(i) / 12.0)));
        w.step();
    }
    w.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    w.run_steps(3);
}

#[test]
fn the_height_goes_to_the_appointments_then_to_more_months_and_the_tasks_keep_their_least() {
    // Two appointments on one day, then one on the next.
    let agenda = [0.0, GROUP_H + EVENT_H, GROUP_H + 2.0 * EVENT_H, 2.0 * GROUP_H + 3.0 * EVENT_H];
    let people = 3.0 * PERSON_H;
    let all = parts(true, true, true);
    // Tall: every appointment, and three months.
    let tall = plan(2_000.0, all, &agenda, people);
    assert_eq!((tall.months, tall.events, tall.fits), (MAX_MONTHS, 3, true));
    assert_eq!(tall.people, people);
    // An ordinary window: one month, and as many appointments as leave the tasks their room.
    let fixed = HEAD_H + MONTH_H + AFTER_NAV + HEAD_H + people + TASKS_HEAD + TASKS_LEAST + FOOT;
    let some = plan(fixed + agenda[2] + 1.0, all, &agenda, people);
    assert_eq!((some.months, some.events, some.fits), (1, 2, true));
    // Too short for one appointment and the tasks' least: the whole bar scrolls, all at its own height.
    let short = plan(fixed + agenda[1] - 1.0, all, &agenda, people);
    assert_eq!((short.months, short.events, short.fits), (1, 3, false));
    // Many favourites over the tasks are capped, and their list scrolls inside that.
    assert_eq!(plan(2_000.0, all, &agenda, 40.0 * PERSON_H).people, PEOPLE_MOST);
    // As the last part, the favourites take what is left.
    let last = plan(1_000.0, parts(true, true, false), &agenda, people);
    assert!(last.fits && last.people > people, "{last:?}");
    // The calendar alone stacks months as far as they fit, and never more than three.
    let one_more = HEAD_H + 2.0 * MONTH_H + MONTH_GAP + AFTER_NAV + agenda[3] + FOOT;
    assert_eq!(plan(one_more, parts(true, false, false), &agenda, 0.0).months, 2);
    assert_eq!(plan(one_more - 1.0, parts(true, false, false), &agenda, 0.0).months, 1);
    assert_eq!(plan(f32::INFINITY, parts(true, false, false), &agenda, 0.0).months, 1, "no height is no room");
    assert!(!plan(f32::NAN, all, &agenda, people).fits);
    // No appointments: the quiet line.
    assert_eq!(plan(2_000.0, parts(true, false, true), &[QUIET_H], 0.0).events, 0);
}

#[test]
fn the_tasks_take_the_rest_of_the_bar_and_scroll_inside_it() {
    let mut mailbox = sample();
    for n in 0..60 {
        mailbox.add_task(Task { title: format!("Sand panel {n}"), ..Default::default() });
    }
    let mut w = window_of(mailbox, "view.mail", 800.0);
    bar(&mut w, &["calendar", "people", "tasks"]);
    let bar_rect = hit(&w, "todoBar", 0).unwrap();
    let drop = hit(&w, "todoTaskDrop", 0).unwrap();
    // The tasks reach down to the foot of the bar, not past it: the bar itself doesn't scroll.
    assert!((drop.max.y - bar_rect.max.y).abs() < 1.5, "{drop:?} in {bar_rect:?}");
    let calendar = hit(&w, "todoCloseCalendar", 0).unwrap();
    assert!(calendar.min.y >= bar_rect.min.y && calendar.min.y < bar_rect.min.y + HEAD_H);
    // Under the box, the tasks are listed past the bar's foot: their own list scrolls.
    let rows = task_rows(&w.state().session);
    let last = rows.last().map(|(_, id)| *id).unwrap();
    assert!(hit(&w, "todoTask", last).is_some_and(|r| r.min.y > bar_rect.max.y));
    // The box to type in stays where it is under the tasks' heading.
    let close_tasks = hit(&w, "todoCloseTasks", 0).unwrap();
    let field = hit(&w, "todoNewTask", 0).unwrap();
    assert!(field.min.y > close_tasks.max.y && field.max.y < close_tasks.max.y + ADD_H + 4.0);
    assert_eq!(months(&w), 1, "an ordinary window has room for one month");
}

#[test]
fn a_tall_bar_stacks_months_and_a_day_is_drawn_once() {
    let mut w = window_of(sample(), "view.mail", 1_500.0);
    bar(&mut w, &["calendar", "tasks"]);
    assert!(months(&w) >= 2, "{}", months(&w));
    let (first, second) = (hit(&w, "todoMonth", 0).unwrap(), hit(&w, "todoMonth", 1).unwrap());
    assert!(second.min.y > first.max.y + 6.0 * WEEK_H);
    // Back over the first month, on over the last.
    let (back, on) = (hit(&w, "todoMonthStep", 0).unwrap(), hit(&w, "todoMonthStep", 1).unwrap());
    assert!((back.center().y - first.center().y).abs() < 1.0);
    let last = hit(&w, "todoMonth", u64::from(months(&w) - 1)).unwrap();
    assert!((on.center().y - last.center().y).abs() < 1.0);
    // The first of next month is in the second grid, not dimmed at the end of the first.
    let today = w.state().session.today();
    let next = today.start_of_month().add_months(1);
    let day = hit(&w, "todoDay", u64::try_from(next.to_days()).unwrap()).unwrap();
    assert!(day.min.y > second.min.y, "{day:?} {second:?}");
    // A click there shows that day.
    testing::click(&mut w, day.center(), PointerButton::Primary, 1);
    assert_eq!((w.state().session.view.module, w.state().session.view.date), (Module::Calendar, next));
    // Next Month moves both months on.
    let mut w = window_of(sample(), "view.mail", 1_500.0);
    bar(&mut w, &["calendar", "tasks"]);
    let on = hit(&w, "todoMonthStep", 1).unwrap().center();
    testing::click(&mut w, on, PointerButton::Primary, 1);
    w.run_steps(2);
    let after = next.add_months(1);
    let day = hit(&w, "todoDay", u64::try_from(after.to_days()).unwrap()).unwrap();
    assert!(day.min.y > hit(&w, "todoMonth", 1).unwrap().min.y);
}

#[test]
fn many_favourites_are_capped_and_the_tasks_stay_under_them() {
    let mut mailbox = sample();
    for n in 0..20 {
        mailbox.add_contact(Contact { first: format!("Joiner{n:02}"), last: "Example".into(), favorite: true, ..Default::default() });
    }
    let mut w = window_of(mailbox, "view.mail", 900.0);
    bar(&mut w, &["people", "tasks"]);
    let people = hit(&w, "todoClosePeople", 0).unwrap();
    let tasks = hit(&w, "todoCloseTasks", 0).unwrap();
    let gap = tasks.center().y - people.center().y;
    assert!((gap - (HEAD_H + PEOPLE_MOST)).abs() < 2.0, "{gap}");
}

#[test]
fn an_appointment_dragged_out_of_the_calendar_onto_the_tasks_becomes_one_and_stays_put() {
    let mut w = window_of(sample(), "view.calendar", 800.0);
    w.state_mut().execute("view.week", json!({})).unwrap();
    bar(&mut w, &["tasks"]);
    let s = &w.state().session;
    let today = s.today();
    let shown = mailcraft_engine::cmd::calendar::occurrences(s, today, today.add_days(3));
    let o = shown.iter().find(|o| s.mailbox.event(o.event).is_some_and(|e| !e.all_day && e.recurrence.is_none())).copied().unwrap();
    let (id, title, start) = {
        let e = s.mailbox.event(o.event).unwrap();
        (e.id, e.title.clone(), e.start)
    };
    let block = hit(&w, "event", id).unwrap();
    let drop = hit(&w, "todoTaskDrop", 0).unwrap();
    let before = w.state().session.mailbox.tasks.len();
    drag(&mut w, block.center(), drop.center());
    let s = &w.state().session;
    assert_eq!(s.mailbox.tasks.len(), before + 1, "a task from the appointment");
    assert!(s.mailbox.tasks.values().any(|t| t.title == title && t.notes.contains("When:")));
    assert_eq!(s.mailbox.event(id).map(|e| e.start), Some(start), "the appointment didn't move");
    assert_eq!(s.view.module, Module::Calendar, "the calendar stays in front");
    // Without the bar's tasks, a drag only moves it in the calendar, as before.
    w.state_mut().execute("view.todoBar", json!({"part": "off"})).unwrap();
    w.run_steps(3);
    assert!(!takes_drops(&w.ctx));
}

#[test]
fn a_contact_dragged_out_of_people_or_the_bar_onto_the_tasks_becomes_one() {
    let mut w = window_of(sample(), "view.people", 800.0);
    bar(&mut w, &["people", "tasks"]);
    let s = &w.state().session;
    let contact = s.mailbox.contacts.values().find(|c| !c.favorite).map(|c| c.id).unwrap();
    let name = s.mailbox.contact(contact).unwrap().name();
    let row = hit(&w, "contact", contact).unwrap();
    let drop = hit(&w, "todoTaskDrop", 0).unwrap();
    drag(&mut w, row.center(), drop.center());
    let s = &w.state().session;
    assert!(s.mailbox.tasks.values().any(|t| t.title == name), "a task named {name}");
    assert_eq!(s.view.module, Module::People);
    // A favourite in the bar, dragged down onto the tasks below it.
    w.state_mut().execute("view.mail", json!({})).unwrap();
    bar(&mut w, &["people", "tasks"]);
    let favourite = *favourites(&w.state().session.mailbox).first().unwrap();
    let name = w.state().session.mailbox.contact(favourite).unwrap().name();
    let row = hit(&w, "todoContact", favourite).unwrap();
    let drop = hit(&w, "todoTaskDrop", 0).unwrap();
    drag(&mut w, row.center(), pos2(drop.center().x, drop.max.y - 20.0));
    let s = &w.state().session;
    assert!(s.mailbox.tasks.values().any(|t| t.title == name), "a task named {name}");
    assert_eq!(s.view.module, Module::Mail);
    // A click on a contact still selects it.
    let row = hit(&w, "todoContact", favourite).unwrap();
    testing::click(&mut w, row.center(), PointerButton::Primary, 1);
    assert_eq!(w.state().session.sel.contact, Some(favourite));
}

#[test]
fn one_of_the_next_appointments_dragged_onto_the_tasks_becomes_one() {
    let mut w = window_of(sample(), "view.mail", 900.0);
    bar(&mut w, &["calendar", "tasks"]);
    let next = *next_appointments(&w.state().session).first().unwrap();
    let title = w.state().session.mailbox.event(next.event).unwrap().title.clone();
    let row = hit(&w, "todoEvent", next.event).unwrap();
    let drop = hit(&w, "todoTaskDrop", 0).unwrap();
    drag(&mut w, row.center(), pos2(drop.center().x, drop.max.y - 20.0));
    let s = &w.state().session;
    assert!(s.mailbox.tasks.values().any(|t| t.title == title), "a task named {title}");
    assert_eq!(s.view.module, Module::Mail);
}

/// Review of #831: with the bar's tasks on, a calendar drag also carries its appointment, and
/// egui takes Escape for that; Escape still gives the move up.
#[test]
fn escape_still_gives_up_a_move_in_the_calendar_with_the_bar_on() {
    let mut w = window_of(sample(), "view.calendar", 800.0);
    w.state_mut().execute("view.week", json!({})).unwrap();
    bar(&mut w, &["tasks"]);
    let s = &w.state().session;
    let today = s.today();
    let shown = mailcraft_engine::cmd::calendar::occurrences(s, today, today.add_days(3));
    let o = shown.iter().find(|o| s.mailbox.event(o.event).is_some_and(|e| !e.all_day && e.recurrence.is_none())).copied().unwrap();
    let start = s.mailbox.event(o.event).unwrap().start;
    let block = hit(&w, "event", o.event).unwrap();
    let (from, to) = (block.center(), block.center() + egui::vec2(0.0, 90.0));
    w.run_steps(45);
    w.input_mut().events.push(Event::PointerMoved(from));
    w.step();
    w.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    w.step();
    for i in 1..=8u8 {
        w.input_mut().events.push(Event::PointerMoved(from + (to - from) * (f32::from(i) / 8.0)));
        w.step();
    }
    testing::key(&mut w, egui::Key::Escape);
    w.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    w.run_steps(3);
    let s = &w.state().session;
    assert_eq!(s.mailbox.event(o.event).map(|e| e.start), Some(start), "Escape gave the move up");
    // Without Escape the same drag moves it, as before.
    let tasks = s.mailbox.tasks.len();
    drag(&mut w, from, to);
    assert_ne!(w.state().session.mailbox.event(o.event).map(|e| e.start), Some(start));
    assert_eq!(w.state().session.mailbox.tasks.len(), tasks, "no task: it wasn't let go over the bar");
}

/// Review of #831: only the main button carries a contact to the tasks.
#[test]
fn a_contact_dragged_with_another_button_makes_no_task() {
    let mut w = window_of(sample(), "view.mail", 800.0);
    bar(&mut w, &["people", "tasks"]);
    let favourite = *favourites(&w.state().session.mailbox).first().unwrap();
    let from = hit(&w, "todoContact", favourite).unwrap().center();
    let drop = hit(&w, "todoTaskDrop", 0).unwrap();
    let to = pos2(drop.center().x, drop.max.y - 20.0);
    let tasks = w.state().session.mailbox.tasks.len();
    w.run_steps(45);
    w.input_mut().events.push(Event::PointerMoved(from));
    w.step();
    w.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    w.step();
    for i in 1..=12u8 {
        w.input_mut().events.push(Event::PointerMoved(from + (to - from) * (f32::from(i) / 12.0)));
        w.step();
    }
    w.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    w.run_steps(3);
    assert_eq!(w.state().session.mailbox.tasks.len(), tasks);
}

#[test]
fn another_display_zone_works_the_calendar_out_again_though_the_mailbox_is_the_same() {
    let mut w = window_of(sample(), "view.mail", 800.0);
    let ctx = w.ctx.clone();
    let s = &w.state().session;
    let first = s.today().start_of_month().start_of_week(s.prefs.week_start);
    let (rev, wall) = (s.rev(), s.local(s.now()));
    let (busy, next) = (busy_days_cached(&ctx, s, first), next_appointments_cached(&ctx, s));
    // Another zone, at the same offset now: the mailbox and the minute are as they were.
    let s = &mut w.state_mut().session;
    s.clock.offset = None;
    s.clock.zone = mailcraft_time::Zone::named("Africa/Abidjan");
    let s = &w.state().session;
    assert_eq!((s.rev(), s.local(s.now())), (rev, wall));
    assert_eq!(s.zone().id(), "Africa/Abidjan");
    assert!(!Arc::ptr_eq(&busy, &busy_days_cached(&ctx, s, first)), "the busy days are worked out again");
    assert!(!Arc::ptr_eq(&next, &next_appointments_cached(&ctx, s)), "the next appointments are worked out again");
}
