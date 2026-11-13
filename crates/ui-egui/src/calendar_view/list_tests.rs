// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The List view in the window (#845), on the sample's invented appointments: the shortcut, a
//! click on a heading, a double click on a series, the keys, and the columns giving way.

use egui::Key;
use mailcraft_engine::cmd::calendar_list::{Group, sections};
use mailcraft_engine::{CalView, ListArrange, ListColumn, Session};
use serde_json::json;

use super::{Col, LINE_H, fit, lines, listed as listing, selected_line};
use crate::calendar_view::tests::{CTRL, NOW, Rig};

/// Ctrl with Alt (Command with Ctrl on a Mac): what the calendar's views are chosen with.
fn views_key() -> egui::Modifiers {
    if cfg!(target_os = "macos") { egui::Modifiers { ctrl: true, ..CTRL } } else { egui::Modifiers { alt: true, ..CTRL } }
}

fn listed() -> Rig {
    let mut rig = Rig::sample();
    rig.chord(views_key(), Key::Num5);
    assert_eq!(rig.app.session.view.calendar, CalView::List);
    rig.frames(2);
    rig
}

fn row(rig: &Rig, title: &str) -> egui::Rect {
    rig.app.hit.rect("listRow", rig.event(title).id).unwrap_or_else(|| panic!("\"{title}\" isn't listed"))
}

/// Where the heading of `column` is.
fn heading(rig: &Rig, column: ListColumn) -> egui::Rect {
    let at = Col::ALL.iter().position(|col| *col == Col::Of(column)).unwrap();
    rig.app.hit.rect("listHeading", at as u64).unwrap()
}

#[test]
fn a_heading_sorts_the_list_and_a_second_click_turns_it_round() {
    let mut rig = listed();
    rig.run("calendar.listArrange", json!({"by": "none"}));
    assert_eq!((rig.app.session.view.list_sort, rig.app.session.view.list_descending), (ListColumn::Start, false));
    rig.click(heading(&rig, ListColumn::Subject).center(), 1);
    assert_eq!((rig.app.session.view.list_sort, rig.app.session.view.list_descending), (ListColumn::Subject, false));
    assert!(row(&rig, "Budget sync").min.y < row(&rig, "Write release notes").min.y);
    rig.click(heading(&rig, ListColumn::Subject).center(), 1);
    assert_eq!((rig.app.session.view.list_sort, rig.app.session.view.list_descending), (ListColumn::Subject, true));
    // The rows are drawn in that order: Z first.
    assert!(row(&rig, "Write release notes").min.y < row(&rig, "Budget sync").min.y);
    // The icons' columns don't sort.
    let icons = rig.app.hit.rect("listHeading", 0).unwrap();
    rig.click(icons.center(), 1);
    assert_eq!((rig.app.session.view.list_sort, rig.app.session.view.list_descending), (ListColumn::Subject, true));
}

#[test]
fn a_double_click_opens_the_series_and_asks_about_no_day() {
    let mut rig = listed();
    let standup = rig.event("Team stand-up").id;
    rig.click(row(&rig, "Team stand-up").center(), 1);
    assert_eq!(rig.selected(), (Some(standup), None));
    rig.click(row(&rig, "Team stand-up").center(), 2);
    assert_eq!(rig.dialog(), None);
    assert!(rig.app.windows.of_event(standup).is_some());
    rig.run("ui.closeWindow", json!({"all": true}));
    // Down goes to the next row; Enter opens it; Alt with = leaves for the month.
    rig.key(Key::ArrowDown);
    let next = rig.app.session.sel.event.unwrap();
    assert_ne!(next, standup);
    assert_eq!(rig.selected().1, None);
    rig.key(Key::Enter);
    assert!(rig.app.windows.of_event(next).is_some());
    rig.run("ui.closeWindow", json!({"all": true}));
    rig.chord(egui::Modifiers::ALT, Key::Equals);
    assert_eq!(rig.app.session.view.calendar, CalView::Month);
}

#[test]
fn a_chosen_day_scrolls_the_list_to_it() {
    // Sixty more days, one appointment each, before the sample's: more than the window shows.
    let mut mailbox = mailcraft_engine::sample::mailbox(NOW, 0);
    let first = mailcraft_time::Date::new(2026, 8, 1).unwrap();
    for n in 0..60 {
        let start = mailcraft_time::DateTime::new(first.add_days(n), 9, 0, 0).unwrap();
        let filler = mailcraft_model::Event { title: format!("Filler {n}"), start, end: start.add_minutes(30), ..Default::default() };
        mailbox.add_event(filler).unwrap();
    }
    let mut rig = Rig::new(mailbox);
    rig.run("view.list", json!({}));
    rig.run("calendar.listArrange", json!({"by": "none"}));
    rig.run("view.goToDate", json!({"date": "2026-09-10"}));
    rig.frames(2);
    // Its first row is at the top, the day before's out of sight above it.
    assert!(rig.app.hit.rect("listRow", rig.event("Filler 40").id).is_some());
    assert!(rig.app.hit.rect("listRow", rig.event("Filler 39").id).is_none());
    // The same day again, after scrolling away, comes back to it.
    rig.run("view.goToDate", json!({"date": "2026-08-01"}));
    rig.frames(2);
    assert!(rig.app.hit.rect("listRow", rig.event("Filler 40").id).is_none());
    rig.run("view.goToDate", json!({"date": "2026-09-10"}));
    rig.frames(2);
    assert!(rig.app.hit.rect("listRow", rig.event("Filler 39").id).is_none());
    assert!(rig.app.hit.rect("listRow", rig.event("Filler 40").id).is_some());
}

#[test]
fn the_columns_give_way_and_subject_and_start_stay() {
    let all = fit(1600.0);
    assert_eq!(all.len(), 9);
    assert!(all.last().is_some_and(|(_, x, w)| x + w <= 1600.0 + 0.5));
    let narrow = fit(360.0);
    let cols: Vec<Col> = narrow.iter().map(|(col, ..)| *col).collect();
    assert_eq!(cols, [Col::Of(ListColumn::Subject), Col::Of(ListColumn::Start)]);
    // Calendar goes first, then Categories.
    let some = fit(1000.0).iter().map(|(col, ..)| *col).collect::<Vec<_>>();
    assert!(!some.contains(&Col::Of(ListColumn::Calendar)) && some.contains(&Col::Of(ListColumn::Location)));
    for width in [0.0, f32::NAN, -5.0, f32::INFINITY] {
        assert_eq!(fit(width).len(), 2, "{width}");
    }
}

#[test]
fn how_the_list_is_sorted_and_arranged_comes_back_after_a_restart() {
    let mut rig = listed();
    rig.run("calendar.listSort", json!({"by": "subject", "descending": true}));
    rig.run("calendar.listArrange", json!({"by": "categories"}));
    let saved = serde_json::to_value(rig.app.prefs()).unwrap();
    let restart = |saved: serde_json::Value| {
        let mut again = crate::MailApp::new(Session::at(rig.app.session.mailbox.clone(), NOW, 0), crate::Services::default());
        again.apply_prefs(serde_json::from_value(saved).unwrap());
        let v = &again.session.view;
        (v.calendar, v.list_sort, v.list_descending, v.list_arrange)
    };
    assert_eq!(restart(saved.clone()), (CalView::List, ListColumn::Subject, true, ListArrange::Categories));
    // Settings from before the List view start on its own order; a column that doesn't sort isn't
    // sorted by.
    let mut older = saved.clone();
    for key in ["listSort", "listDescending", "listArrange"] {
        older["view"].as_object_mut().unwrap().remove(key);
    }
    assert_eq!(restart(older), (CalView::List, ListColumn::Start, false, ListArrange::Recurrence));
    let mut odd = saved;
    odd["view"]["listSort"] = json!("categories");
    assert_eq!(restart(odd).1, ListColumn::Start);
}

#[test]
fn switching_to_the_list_asks_nothing_about_the_day_that_was_selected() {
    let mut rig = Rig::sample();
    let standup = rig.event("Team stand-up").id;
    let day = rig.event("Team stand-up").start.date.add_days(7);
    rig.run("calendar.select", json!({"id": standup, "day": day.iso()}));
    rig.chord(views_key(), Key::Num5);
    assert_eq!(rig.selected(), (Some(standup), None));
    // Delete is about the series: nobody is asked about one day of it, and the series goes.
    rig.key(Key::Delete);
    if let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &rig.app.dialog {
        panic!("asked: {}", ask.title);
    }
    assert!(rig.app.session.mailbox.event(standup).is_none());
}

#[test]
fn of_an_event_under_two_categories_the_row_clicked_is_the_one_selected() {
    let mut mailbox = mailcraft_model::Mailbox::with_local_account("Ada", "ada@example.org");
    mailbox.events.clear();
    let day = mailcraft_time::Date::new(2026, 10, 14).unwrap();
    for (n, (title, categories)) in [("Both", &["Errands", "Garden"][..]), ("Errand", &["Errands"]), ("Garden", &["Garden"])].into_iter().enumerate()
    {
        let start = mailcraft_time::DateTime::new(day.add_days(n as i64), 9, 0, 0).unwrap();
        let categories = categories.iter().map(|c| c.to_string()).collect();
        mailbox
            .add_event(mailcraft_model::Event { title: title.into(), start, end: start.add_minutes(30), categories, ..Default::default() })
            .unwrap();
    }
    let mut rig = Rig::new(mailbox);
    rig.run("view.list", json!({}));
    rig.run("calendar.listArrange", json!({"by": "categories"}));
    // Errands: Both, Errand; Garden: Both, Garden. Garden's Both is two lines under Errand.
    let second = row(&rig, "Errand").translate(egui::vec2(0.0, 2.0 * LINE_H));
    rig.click(second.center(), 1);
    let both = rig.event("Both").id;
    assert_eq!(rig.selected(), (Some(both), None));
    // Only that row is the one selected; chosen by a script, it is the first.
    let all = sections(&rig.app.session);
    let at = lines(&all);
    let garden = (both, Group::Category(Some("Garden".into())));
    assert_eq!(selected_line(&all, &at, Some(both), Some(&garden)), Some(4));
    assert_eq!(selected_line(&all, &at, Some(both), None), Some(1));
    assert_eq!(selected_line(&all, &at, Some(both), Some(&(rig.event("Garden").id, garden.1.clone()))), Some(1));
    // Down goes on from it, within Garden.
    rig.key(Key::ArrowDown);
    assert_eq!(rig.selected().0, Some(rig.event("Garden").id));
    rig.key(Key::ArrowUp);
    rig.key(Key::ArrowUp);
    assert_eq!(rig.selected().0, Some(rig.event("Errand").id));
}

#[test]
fn two_presses_of_down_in_one_frame_move_two_rows() {
    let mut mailbox = mailcraft_model::Mailbox::with_local_account("Ada", "ada@example.org");
    mailbox.events.clear();
    let day = mailcraft_time::Date::new(2026, 10, 14).unwrap();
    for n in 0..5 {
        let start = mailcraft_time::DateTime::new(day.add_days(n), 9, 0, 0).unwrap();
        mailbox.add_event(mailcraft_model::Event { title: format!("Item {n}"), start, end: start.add_minutes(30), ..Default::default() }).unwrap();
    }
    let mut rig = Rig::new(mailbox);
    rig.run("view.list", json!({}));
    rig.run("calendar.listArrange", json!({"by": "none"}));
    rig.click(row(&rig, "Item 0").center(), 1);
    assert_eq!(rig.selected().0, Some(rig.event("Item 0").id));
    rig.keys_at_once(&[Key::ArrowDown, Key::ArrowDown]);
    assert_eq!(rig.selected().0, Some(rig.event("Item 2").id));
    rig.keys_at_once(&[Key::ArrowDown, Key::ArrowDown, Key::ArrowUp]);
    assert_eq!(rig.selected().0, Some(rig.event("Item 3").id));
}

#[test]
fn the_rows_are_worked_out_again_only_when_the_mailbox_or_a_list_setting_changes() {
    let ctx = egui::Context::default();
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    s.run("view.list", &json!({})).unwrap();
    let first = listing(&ctx, &s);
    assert!(first.items > 0 && first.lines.len() >= first.items);
    // Nothing changed: the same answer, not another one like it.
    assert!(std::sync::Arc::ptr_eq(&first, &listing(&ctx, &s)));
    // A change to the mailbox: a new one, with the row in it.
    let before = first.items;
    s.run("calendar.new", &json!({"title": "Added", "start": "2026-10-20T09:00:00", "end": "2026-10-20T10:00:00"})).unwrap();
    let second = listing(&ctx, &s);
    assert!(!std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(second.items, before + 1);
    assert!(std::sync::Arc::ptr_eq(&second, &listing(&ctx, &s)));
    // Each of the three settings.
    let mut last = second;
    for (command, params) in [
        ("calendar.listSort", json!({"by": "subject"})),
        ("calendar.listSort", json!({"by": "subject"})),
        ("calendar.listArrange", json!({"by": "categories"})),
        ("calendar.listArrange", json!({"by": "none"})),
    ] {
        s.run(command, &params).unwrap();
        let now = listing(&ctx, &s);
        assert!(!std::sync::Arc::ptr_eq(&last, &now), "{command} {params}");
        last = now;
    }
    // A calendar shown or hidden changes what is listed.
    let calendar = s.mailbox.calendars.iter().find(|c| c.visible).map(|c| c.id).unwrap();
    s.run("calendar.showCalendar", &json!({"id": calendar, "value": false})).unwrap();
    let hidden = listing(&ctx, &s);
    assert!(!std::sync::Arc::ptr_eq(&last, &hidden));
    assert!(hidden.items < last.items);
}

#[test]
fn a_big_calendar_is_listed_once_and_then_every_frame_is_cheap() {
    let ctx = egui::Context::default();
    let mut mailbox = mailcraft_model::Mailbox::with_local_account("Ada", "ada@example.org");
    mailbox.events.clear();
    let day = mailcraft_time::Date::new(2026, 1, 1).unwrap();
    for n in 0..3_000_i64 {
        let start = mailcraft_time::DateTime::new(day.add_days(n % 500), u8::try_from(n % 24).unwrap(), 0, 0).unwrap();
        let categories = (0..16).map(|k| format!("Category {}", (n + k * 3) % 16)).collect();
        mailbox
            .add_event(mailcraft_model::Event {
                title: format!("Subject {}", n * 7919 % 3001),
                start,
                end: start.add_minutes(30),
                categories,
                ..Default::default()
            })
            .unwrap();
    }
    let mut s = Session::at(mailbox, NOW, 0);
    s.run("view.list", &json!({})).unwrap();
    s.run("calendar.listArrange", &json!({"by": "categories"})).unwrap();
    s.run("calendar.listSort", &json!({"by": "subject"})).unwrap();
    let started = std::time::Instant::now();
    let first = listing(&ctx, &s);
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "the first frame took {:?}", started.elapsed());
    assert_eq!((first.items, first.sections.len()), (3_000, 16));
    // The frames after it (the window's, and the status bar's) only look it up.
    let started = std::time::Instant::now();
    for _ in 0..1_000 {
        assert!(std::sync::Arc::ptr_eq(&first, &listing(&ctx, &s)));
    }
    assert!(started.elapsed() < std::time::Duration::from_millis(500), "a thousand frames took {:?}", started.elapsed());
}
