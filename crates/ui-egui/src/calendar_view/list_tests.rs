// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The List view in the window (#845), on the sample's invented appointments: the shortcut, a
//! click on a heading, a double click on a series, the keys, and the columns giving way.

use egui::Key;
use mailcraft_engine::{CalView, ListColumn};
use serde_json::json;

use super::{Col, fit};
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
