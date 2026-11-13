// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The To-Do Bar (#831): drawn when a part of it is on, with what it lists.

use std::sync::Arc;

use egui::{Key, PointerButton};
use mailcraft_engine::Module;
use serde_json::json;

use super::*;
use crate::people_view::testing::{self, Window};

#[test]
fn widths_are_kept_within_bounds() {
    assert_eq!(sane_width(f32::NAN), DEFAULT_WIDTH);
    assert_eq!(sane_width(f32::INFINITY), DEFAULT_WIDTH);
    assert_eq!(sane_width(10.0), MIN_WIDTH);
    assert_eq!(sane_width(9_000.0), MAX_WIDTH);
    assert_eq!(sane_width(300.0), 300.0);
    assert_eq!(widest(1280.0), Some(MAX_WIDTH));
    assert_eq!(widest(LEAVES + 250.0), Some(250.0));
    assert_eq!(widest(LEAVES + MIN_WIDTH), Some(MIN_WIDTH));
    assert_eq!(widest(LEAVES + MIN_WIDTH - 1.0), None, "a window too narrow for the bar and the module has none");
    assert_eq!(widest(600.0), None);
    assert_eq!(widest(f32::NAN), None);
}

/// Mail in a window of `width` with the bar on: where the bar is (if it is drawn) and the reading
/// pane's width.
fn mail_at(width: f32, bar_on: bool) -> (Option<Rect>, f32) {
    let mut w = testing::window_sized(mailcraft_engine::sample::mailbox(testing::now(), 0), "view.mail", egui::vec2(width, 700.0));
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    if bar_on {
        bar(&mut w, &["calendar", "people", "tasks"]);
    }
    w.run_steps(4);
    (hit(&w, "todoBar", 0), hit(&w, "readingPane", 0).expect("the reading pane").width())
}

/// The bar leaves the module what it needs (#831): at the smallest window it isn't drawn and the
/// reading pane is as wide as without it; at 1024 px it narrows to leave the reading pane a
/// readable width; with room it is as it was.
#[test]
fn the_bar_leaves_the_reading_pane_room_in_a_narrow_window() {
    let (none, bare) = mail_at(760.0, false);
    assert!(none.is_none());
    let (bar, with) = mail_at(760.0, true);
    assert!(bar.is_none(), "no room for the bar beside the folders, the list and the reading pane: {bar:?}");
    assert!((with - bare).abs() < 1.0, "{with} {bare}");

    let (bar, with) = mail_at(1024.0, true);
    let bar = bar.expect("room for a narrow bar");
    assert!(bar.width() >= MIN_WIDTH - 1.0 && bar.width() < DEFAULT_WIDTH, "{bar:?}");
    assert!(with >= 255.0, "the reading pane at 1024 px: {with}");

    let (bar, with) = mail_at(1600.0, true);
    assert!(bar.is_some_and(|b| (b.width() - DEFAULT_WIDTH).abs() < 6.0), "{bar:?}");
    assert!(with >= 300.0, "{with}");
}

/// What the owner chose for the bar's width isn't lost to a window that was narrow for a while.
#[test]
fn a_narrow_window_does_not_change_the_width_that_was_chosen() {
    let mut w = testing::window_sized(mailcraft_engine::sample::mailbox(testing::now(), 0), "view.mail", egui::vec2(760.0, 700.0));
    w.state_mut().ui.todo_bar_width = 400.0;
    bar(&mut w, &["tasks"]);
    assert!(hit(&w, "todoBar", 0).is_none());
    assert_eq!(w.state().ui.todo_bar_width, 400.0);
}

/// The width the owner chose comes back when a window that held the bar in grows again.
#[test]
fn the_chosen_width_comes_back_in_a_wider_window() {
    let mut w = testing::window_sized(mailcraft_engine::sample::mailbox(testing::now(), 0), "view.mail", egui::vec2(1024.0, 700.0));
    w.state_mut().ui.todo_bar_width = 400.0;
    bar(&mut w, &["tasks"]);
    let held = hit(&w, "todoBar", 0).expect("room for a narrow bar").width();
    assert!(held < 300.0, "held in at 1024 px: {held}");
    assert_eq!(w.state().ui.todo_bar_width, 400.0, "what was chosen is kept while it is held in");
    w.set_size(egui::vec2(1600.0, 700.0));
    w.run_steps(6);
    let wide = hit(&w, "todoBar", 0).unwrap().width();
    assert!((wide - 400.0).abs() < 6.0, "{wide}");
    assert_eq!(w.state().ui.todo_bar_width, 400.0);
    // And held in again by a narrow one, and back.
    w.set_size(egui::vec2(1024.0, 700.0));
    w.run_steps(6);
    w.set_size(egui::vec2(1600.0, 700.0));
    w.run_steps(6);
    assert!((hit(&w, "todoBar", 0).unwrap().width() - 400.0).abs() < 6.0);
}

/// More than [`MAX_TASKS`] open tasks: the bar lists that many, and says there are more.
#[test]
fn more_open_tasks_than_the_bar_lists_are_left_to_the_tasks_module() {
    let mut mailbox = mailcraft_engine::sample::mailbox(testing::now(), 0);
    for n in 0..MAX_TASKS + 5 {
        mailbox.add_task(mailcraft_model::Task { title: format!("Sand panel {n}"), ..Default::default() });
    }
    let w = testing::window_on(mailbox, "view.mail");
    assert_eq!(task_rows(&w.state().session).len(), MAX_TASKS + 1, "one past the last listed says there are more");
}

fn window(module: &str) -> Window {
    let mut w = testing::window(module);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.run_steps(2);
    w
}

fn bar(w: &mut Window, parts: &[&str]) {
    for part in parts {
        w.state_mut().execute("view.todoBar", json!({"part": part, "value": true})).unwrap();
    }
    w.run_steps(4);
}

fn hit(w: &Window, kind: &str, id: u64) -> Option<Rect> {
    w.state().hit.rect(kind, id)
}

fn first_task(w: &Window) -> Id {
    task_rows(&w.state().session).first().map(|(_, id)| *id).unwrap()
}

#[test]
fn the_bar_is_drawn_with_the_parts_that_are_on_at_the_right_of_the_window() {
    let mut w = window("view.mail");
    assert!(hit(&w, "todoBar", 0).is_none(), "closed until it is opened");
    bar(&mut w, &["calendar", "people", "tasks"]);
    let r = hit(&w, "todoBar", 0).expect("the bar");
    assert!(r.max.x >= 1270.0 && r.width() >= MIN_WIDTH - 1.0 && r.width() <= MAX_WIDTH + 1.0, "{r:?}");
    // Its parts stand in Outlook's order: the calendar, the people, the tasks.
    let (cal, people, tasks) =
        (hit(&w, "todoCloseCalendar", 0).unwrap(), hit(&w, "todoClosePeople", 0).unwrap(), hit(&w, "todoCloseTasks", 0).unwrap());
    assert!(cal.min.y < people.min.y && people.min.y < tasks.min.y);
    // Today is on the navigator; the sample's favourites are listed.
    let today = u64::try_from(w.state().session.today().to_days()).unwrap();
    assert!(hit(&w, "todoDay", today).is_some());
    let favourites = favourites(&w.state().session.mailbox);
    assert_eq!(favourites.len(), 3, "the sample has three");
    assert!(favourites.iter().all(|id| hit(&w, "todoContact", *id).is_some()));
    // Another module has a bar of its own, closed.
    w.state_mut().execute("view.people", json!({})).unwrap();
    w.run_steps(3);
    assert!(hit(&w, "todoBar", 0).is_none());
    w.state_mut().execute("view.mail", json!({})).unwrap();
    w.run_steps(3);
    assert!(hit(&w, "todoBar", 0).is_some());
}

#[test]
fn a_part_closes_from_its_own_button_and_the_last_closes_the_bar() {
    let mut w = window("view.mail");
    bar(&mut w, &["people", "tasks"]);
    assert!(hit(&w, "todoCloseCalendar", 0).is_none());
    let close = hit(&w, "todoClosePeople", 0).unwrap();
    testing::click(&mut w, close.center(), PointerButton::Primary, 1);
    assert!(!w.state().session.view.todo_bar.mail.people);
    let close = hit(&w, "todoCloseTasks", 0).unwrap();
    testing::click(&mut w, close.center(), PointerButton::Primary, 1);
    w.run_steps(2);
    assert!(hit(&w, "todoBar", 0).is_none());
}

#[test]
fn a_task_typed_in_the_bar_is_made_and_the_mail_stays_in_front() {
    let mut w = window("view.mail");
    bar(&mut w, &["tasks"]);
    let before = w.state().session.mailbox.tasks.len();
    let field = hit(&w, "todoNewTask", 0).unwrap();
    testing::click(&mut w, field.center(), PointerButton::Primary, 1);
    testing::text(&mut w, "Water the office plants");
    testing::key(&mut w, Key::Enter);
    let s = &w.state().session;
    assert_eq!(s.mailbox.tasks.len(), before + 1);
    assert!(s.mailbox.tasks.values().any(|t| t.title == "Water the office plants"));
    assert_eq!(s.view.module, Module::Mail);
    // The box is ready for the next one, empty.
    let id = egui::Id::unique(NEW_TASK_ID);
    assert_eq!(w.ctx.data(|d| d.get_temp::<String>(id)).unwrap_or_default(), "");
    assert!(w.ctx.memory(|m| m.has_focus(id)));
}

#[test]
fn a_message_let_go_over_the_tasks_becomes_one() {
    let mut w = window("view.mail");
    bar(&mut w, &["tasks"]);
    let message = w.state_mut().session.rows().first().map(|r| r.id).unwrap();
    let area = hit(&w, "todoTaskDrop", 0).unwrap();
    // As a drag out of the list carries it, then let go over the tasks.
    egui::DragAndDrop::set_payload(&w.ctx, crate::mail_view::MessageDrag(vec![message]));
    let at = area.center();
    w.input_mut().events.push(egui::Event::PointerMoved(at));
    w.run_steps(2);
    w.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    w.run_steps(3);
    let s = &w.state().session;
    let task = s.mailbox.tasks.values().find(|t| t.message == Some(message)).expect("a task from the message").id;
    assert_eq!(s.view.module, Module::Mail, "the mail stays in front");
    assert!(task_rows(s).iter().any(|(_, id)| *id == task), "and the bar lists it");
}

#[test]
fn a_double_click_opens_a_task_or_a_contact() {
    let mut w = window("view.mail");
    bar(&mut w, &["people", "tasks"]);
    let task = first_task(&w);
    let r = hit(&w, "todoTask", task).unwrap();
    testing::click(&mut w, pos2(r.center().x + 20.0, r.center().y), PointerButton::Primary, 2);
    assert_eq!(testing::dialog(&w), Some("editTask"));
    assert_eq!(w.state().session.sel.task, Some(task));
    w.state_mut().dialog = None;
    w.run_steps(2);
    let contact = *favourites(&w.state().session.mailbox).first().unwrap();
    let r = hit(&w, "todoContact", contact).unwrap();
    testing::click(&mut w, r.center(), PointerButton::Primary, 2);
    assert_eq!(testing::dialog(&w), Some("editContact"));
    assert_eq!(w.state().session.view.module, Module::Mail);
}

#[test]
fn the_check_box_ticks_a_task_off() {
    let mut w = window("view.mail");
    bar(&mut w, &["tasks"]);
    let task = first_task(&w);
    let r = hit(&w, "todoTaskCheck", task).unwrap();
    testing::click(&mut w, r.center(), PointerButton::Primary, 1);
    assert!(w.state().session.mailbox.task(task).is_some_and(|t| t.is_complete()));
    w.run_steps(2);
    assert!(hit(&w, "todoTask", task).is_none(), "a finished task leaves the bar");
}

#[test]
fn a_day_on_the_navigator_shows_it_in_the_calendar() {
    let mut w = window("view.mail");
    bar(&mut w, &["calendar"]);
    let day = w.state().session.today().add_days(2);
    let r = hit(&w, "todoDay", u64::try_from(day.to_days()).unwrap()).unwrap();
    testing::click(&mut w, r.center(), PointerButton::Primary, 1);
    assert_eq!((w.state().session.view.module, w.state().session.view.date), (Module::Calendar, day));
}

#[test]
fn the_tasks_are_arranged_by_due_date_and_the_appointments_are_still_to_come() {
    let w = window("view.mail");
    let s = &w.state().session;
    let rows = task_rows(s);
    assert!(!rows.is_empty());
    assert!(rows.windows(2).all(|pair| pair[0].0 <= pair[1].0), "headings in order");
    assert!(rows.iter().all(|(bucket, id)| *bucket != Bucket::Completed && s.mailbox.task(*id).is_some_and(|t| !t.is_complete())));
    let now = s.local(s.now());
    let next = next_appointments(s);
    assert!(!next.is_empty() && next.len() <= MAX_EVENTS);
    assert!(next.iter().all(|o| o.end > now));
    assert!(next.windows(2).all(|pair| pair[0].start <= pair[1].start));
}

#[test]
fn the_bar_and_its_width_are_kept_between_runs() {
    let mut w = window("view.mail");
    bar(&mut w, &["calendar", "tasks"]);
    w.state_mut().execute("view.calendar", json!({})).unwrap();
    w.state_mut().execute("view.todoBar", json!({"part": "people"})).unwrap();
    let mut saved = w.state().prefs();
    saved.todo_bar_width = 333.0;
    let mut next = crate::MailApp::new(mailcraft_engine::Session::empty(), crate::Services::default());
    next.apply_prefs(serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap());
    assert_eq!(next.session.view.todo_bar, w.state().session.view.todo_bar);
    assert!(next.session.view.todo_bar.mail.calendar && next.session.view.todo_bar.calendar.people);
    assert_eq!(next.ui.todo_bar_width, 333.0);
    // A width that is no width comes back as one.
    saved.todo_bar_width = f32::NAN;
    next.apply_prefs(saved);
    assert_eq!(next.ui.todo_bar_width, DEFAULT_WIDTH);
}

#[test]
fn what_the_bar_lists_is_kept_between_frames_and_worked_out_again_when_it_changes() {
    let mut w = window("view.mail");
    let ctx = w.ctx.clone();
    // Nothing changed: the same lists, not gone through again.
    let s = &w.state().session;
    let first = s.today().start_of_month().start_of_week(s.prefs.week_start);
    assert!(Arc::ptr_eq(&task_rows_cached(&ctx, s), &task_rows_cached(&ctx, s)));
    assert!(Arc::ptr_eq(&favourites_cached(&ctx, s), &favourites_cached(&ctx, s)));
    assert!(Arc::ptr_eq(&next_appointments_cached(&ctx, s), &next_appointments_cached(&ctx, s)));
    assert!(Arc::ptr_eq(&busy_days_cached(&ctx, s, first), &busy_days_cached(&ctx, s, first)));

    // A task made: listed.
    let made = w.state_mut().execute("tasks.new", json!({"title": "Sort the stationery cupboard"})).unwrap();
    let made = made["id"].as_u64().unwrap();
    assert!(task_rows_cached(&ctx, &w.state().session).iter().any(|(_, id)| *id == made));
    // A favourite let go of: gone from the people.
    let contact = *favourites_cached(&ctx, &w.state().session).first().unwrap();
    w.state_mut().execute("people.favorite", json!({"id": contact, "value": false})).unwrap();
    assert!(!favourites_cached(&ctx, &w.state().session).contains(&contact));
    // The clock past the end of the next appointment: it is off the list.
    let next = *next_appointments_cached(&ctx, &w.state().session).first().unwrap();
    let s = &mut w.state_mut().session;
    let mut at = s.now();
    while s.local(at) < next.end {
        at += 60;
    }
    s.clock.now = Some(at + 60);
    assert!(!next_appointments_cached(&ctx, &w.state().session).contains(&next));
    // Its calendar hidden: its days are no longer busy, unless something else is on them.
    let s = &w.state().session;
    let busy = busy_days_cached(&ctx, s, first);
    let calendars: Vec<u64> = s.mailbox.calendars.iter().filter(|c| c.visible).map(|c| c.id).collect();
    for id in calendars {
        w.state_mut().execute("calendar.showCalendar", json!({"id": id, "value": false})).unwrap();
    }
    let after = busy_days_cached(&ctx, &w.state().session, first);
    assert!(busy.iter().any(|b| *b), "the sample has something on");
    assert!(after.iter().all(|b| !b), "nothing is on in a calendar that is hidden");
}
