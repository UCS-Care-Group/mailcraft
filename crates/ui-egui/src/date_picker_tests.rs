// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar to pick a day from (#329), on a field of its own: where it opens, its keys, what it
//! picks, its weeks and the calendar's ends. (In the forms and in Advanced Search: their tests.)

use egui::accesskit::Role;
use egui::{Id, Key};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT as _, Queryable as _};
use mailcraft_time::{Date, Weekday};

use super::{Calendar, Picked, moved};

fn day(s: &str) -> Date {
    Date::parse_iso(s).unwrap()
}

/// Wednesday 2026-10-14, as the sample mailbox's day.
fn today() -> Date {
    day("2026-10-14")
}

struct Field {
    text: String,
    week_start: Weekday,
    offer_none: bool,
    picks: Vec<Picked>,
}

fn field_id() -> Id {
    Id::unique("date_field")
}

/// A date field and its calendar, as a form lays them out.
fn harness(text: &str, week_start: Weekday, offer_none: bool) -> Harness<'static, Field> {
    let field = Field { text: text.into(), week_start, offer_none, picks: Vec::new() };
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(500.0, 500.0)).build_ui_state(
        |ui, f: &mut Field| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut f.text).id(field_id()));
                let calendar = Calendar {
                    field: field_id(),
                    current: Date::parse_iso(&f.text),
                    today: today(),
                    week_start: f.week_start,
                    offer_none: f.offer_none,
                    name: "Pick a date",
                };
                if let Some(p) = calendar.show(ui) {
                    match p {
                        Picked::Day(d) => f.text = d.iso(),
                        Picked::None => f.text.clear(),
                    }
                    f.picks.push(p);
                }
            });
        },
        field,
    );
    h.run_steps(2);
    h
}

fn open(h: &mut Harness<'static, Field>) {
    h.get_by_label("Pick a date").click();
    h.run_steps(3);
    assert!(is_open(h), "the calendar opened");
}

fn is_open(h: &Harness<'static, Field>) -> bool {
    h.query(By::new().role(Role::Grid)).is_some()
}

/// The day the keyboard is on: the grid is named with it.
fn active(h: &Harness<'static, Field>) -> String {
    h.get(By::new().role(Role::Grid)).accesskit_node().label().unwrap_or_default()
}

fn long(s: &str) -> String {
    crate::dates::long(day(s))
}

fn press(h: &mut Harness<'static, Field>, key: Key) {
    h.key_press(key);
    h.run_steps(2);
}

fn field_has_keyboard(h: &Harness<'static, Field>) -> bool {
    h.ctx.memory(|m| m.has_focus(field_id()))
}

#[test]
fn it_opens_on_the_fields_day_or_today() {
    let mut h = harness("2026-03-15", Weekday::Sun, false);
    open(&mut h);
    assert_eq!(active(&h), long("2026-03-15"));
    assert!(h.query_by_label(&long("2026-03-01")).is_some(), "March is shown");
    for text in ["", "someday", "2026-02-30"] {
        let mut h = harness(text, Weekday::Sun, false);
        open(&mut h);
        assert_eq!(active(&h), long("2026-10-14"), "{text:?}");
    }
}

#[test]
fn the_arrows_move_a_day_or_a_week_and_page_keys_a_month() {
    let mut h = harness("2026-10-31", Weekday::Sun, false);
    open(&mut h);
    press(&mut h, Key::ArrowRight);
    assert_eq!(active(&h), long("2026-11-01"), "into the next month");
    assert!(h.query_by_label(&long("2026-11-30")).is_some(), "November is shown");
    press(&mut h, Key::ArrowUp);
    assert_eq!(active(&h), long("2026-10-25"));
    press(&mut h, Key::ArrowDown);
    press(&mut h, Key::ArrowLeft);
    assert_eq!(active(&h), long("2026-10-31"));
    press(&mut h, Key::PageDown);
    assert_eq!(active(&h), long("2026-11-30"), "a month on, the month's last day");
    press(&mut h, Key::PageUp);
    assert_eq!(active(&h), long("2026-10-30"));
    // Nothing is picked by moving.
    assert_eq!(h.state().text, "2026-10-31");
    assert!(h.state().picks.is_empty());
}

#[test]
fn page_keys_keep_the_day_the_month_has() {
    assert_eq!(moved(day("2026-01-31"), Key::PageDown), Some(day("2026-02-28")));
    assert_eq!(moved(day("2028-01-31"), Key::PageDown), Some(day("2028-02-29")));
    assert_eq!(moved(day("2028-02-29"), Key::PageUp), Some(day("2028-01-29")));
    assert_eq!(moved(day("2026-12-31"), Key::ArrowRight), Some(day("2027-01-01")));
    assert_eq!(moved(day("2026-10-14"), Key::Home), None, "as in Outlook");
    assert_eq!(moved(day("2026-10-14"), Key::A), None);
}

#[test]
fn enter_picks_the_day_and_the_keyboard_goes_back_to_the_field() {
    let mut h = harness("2026-10-14", Weekday::Sun, false);
    open(&mut h);
    press(&mut h, Key::ArrowDown);
    press(&mut h, Key::Enter);
    assert_eq!(h.state().text, "2026-10-21");
    assert!(!is_open(&h));
    h.run_steps(2);
    assert!(field_has_keyboard(&h));
}

#[test]
fn space_does_nothing_as_in_outlook() {
    let mut h = harness("2026-10-14", Weekday::Sun, false);
    open(&mut h);
    press(&mut h, Key::ArrowRight);
    for repeat in [false, true, true] {
        h.event(egui::Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat, modifiers: Default::default() });
        h.event(egui::Event::Text(" ".into()));
        h.run_steps(2);
    }
    assert!(is_open(&h));
    assert_eq!(h.state().text, "2026-10-14");
    assert!(h.state().picks.is_empty());
}

#[test]
fn escape_closes_it_and_keeps_the_text() {
    let mut h = harness("2026-10-14", Weekday::Sun, false);
    open(&mut h);
    press(&mut h, Key::ArrowRight);
    press(&mut h, Key::Escape);
    assert!(!is_open(&h));
    assert_eq!(h.state().text, "2026-10-14");
    assert!(h.state().picks.is_empty());
    h.run_steps(2);
    assert!(field_has_keyboard(&h));
}

#[test]
fn a_click_on_a_day_picks_it() {
    let mut h = harness("2026-10-14", Weekday::Sun, false);
    open(&mut h);
    h.get_by_label(&long("2026-11-02")).click();
    h.run_steps(2);
    assert_eq!(h.state().text, "2026-11-02", "a day of the next month, shown dimmed");
    assert!(!is_open(&h));
}

#[test]
fn today_and_none() {
    let mut h = harness("2026-03-15", Weekday::Sun, true);
    open(&mut h);
    h.get_by_label("Today").click();
    h.run_steps(2);
    assert_eq!(h.state().text, "2026-10-14");
    open(&mut h);
    h.get_by_label("None").click();
    h.run_steps(2);
    assert_eq!(h.state().text, "");
    assert_eq!(h.state().picks, [Picked::Day(today()), Picked::None]);
    // Where the field may not be empty, as Outlook's Start date: Today only.
    let mut h = harness("2026-03-15", Weekday::Sun, false);
    open(&mut h);
    assert!(h.query_by_label("Today").is_some());
    assert!(h.query_by_label("None").is_none());
}

#[test]
fn tab_goes_on_to_today() {
    let mut h = harness("2026-03-15", Weekday::Sun, false);
    open(&mut h);
    press(&mut h, Key::Tab);
    assert!(is_open(&h));
    let today = h.get_by_label("Today");
    assert!(today.is_focused(), "Today has the keyboard");
    press(&mut h, Key::Enter);
    assert_eq!(h.state().text, "2026-10-14");
    assert!(!is_open(&h));
    h.run_steps(2);
    assert!(field_has_keyboard(&h), "back in the field");
}

#[test]
fn the_weeks_start_on_the_day_the_options_say() {
    // October 2026 begins on a Thursday.
    for (start, first, absent) in
        [(Weekday::Sun, "2026-09-27", "2026-09-26"), (Weekday::Mon, "2026-09-28", "2026-09-27"), (Weekday::Sat, "2026-09-26", "2026-09-25")]
    {
        let mut h = harness("2026-10-14", start, false);
        open(&mut h);
        assert!(h.query_by_label(&long(first)).is_some(), "{start:?}");
        assert!(h.query_by_label(&long(absent)).is_none(), "{start:?}");
        assert_eq!(h.query_all(By::new().role(Role::Button).label_contains(", 2026")).count(), 42, "{start:?}: six weeks");
    }
}

#[test]
fn the_month_arrows_move_the_month_and_the_day_follows() {
    let mut h = harness("2026-01-31", Weekday::Sun, false);
    open(&mut h);
    h.get_by_label("Next Month").click();
    h.run_steps(2);
    assert_eq!(active(&h), long("2026-02-28"));
    h.get_by_label("Previous Month").click();
    h.get_by_label("Previous Month").click();
    h.run_steps(2);
    assert_eq!(active(&h), long("2025-12-28"));
    // The keyboard is still in the grid.
    press(&mut h, Key::ArrowRight);
    assert_eq!(active(&h), long("2025-12-29"));
    assert_eq!(h.state().text, "2026-01-31");
}

#[test]
fn at_the_calendars_ends_it_stops() {
    let mut h = harness("0001-01-01", Weekday::Sun, false);
    open(&mut h);
    let previous = h.get_by_label("Previous Month");
    assert!(previous.accesskit_node().is_disabled());
    press(&mut h, Key::ArrowLeft);
    press(&mut h, Key::PageUp);
    assert_eq!(active(&h), long("0001-01-01"));
    // No day before the first, and none twice.
    let days: Vec<String> =
        h.query_all(By::new().role(Role::Button).label_contains(", 1")).map(|n| n.accesskit_node().label().unwrap_or_default()).collect();
    let mut unique = days.clone();
    unique.dedup();
    assert_eq!(unique, days);
    assert!(h.query_by_role_and_label(Role::Button, &long("0001-01-01")).is_some());

    let mut h = harness("9999-12-31", Weekday::Sun, false);
    open(&mut h);
    assert!(h.get_by_label("Next Month").accesskit_node().is_disabled());
    press(&mut h, Key::ArrowRight);
    press(&mut h, Key::PageDown);
    assert_eq!(active(&h), long("9999-12-31"));
}

#[test]
fn two_fields_keep_their_own_calendars() {
    let ids = [Id::unique("a"), Id::unique("b")];
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(600.0, 500.0)).build_ui_state(
        move |ui, texts: &mut [String; 2]| {
            ui.horizontal(|ui| {
                for (i, text) in texts.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(text).id(ids[i]).desired_width(90.0));
                    let name = if i == 0 { "Pick the first day" } else { "Pick the last day" };
                    let calendar =
                        Calendar { field: ids[i], current: Date::parse_iso(text), today: today(), week_start: Weekday::Sun, offer_none: true, name };
                    if let Some(Picked::Day(d)) = calendar.show(ui) {
                        *text = d.iso();
                    }
                }
            });
        },
        ["2026-01-05".to_string(), "2026-06-20".to_string()],
    );
    h.run_steps(2);
    h.get_by_label("Pick the last day").click();
    h.run_steps(3);
    assert_eq!(h.get(By::new().role(Role::Grid)).accesskit_node().label().unwrap_or_default(), long("2026-06-20"));
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state(), &["2026-01-05".to_string(), "2026-06-21".to_string()]);
}
