// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A calendar to pick a day from, behind a button after a date field, as Outlook's date fields
//! have (#329): the month between arrows to the one before and after, six weeks of days from the
//! first day of the week in the options, today outlined, the field's day filled, and Today (and
//! None, where the field may be empty) under it.
//!
//! The keyboard is in the month's grid while it is open, on one day at a time, as in Outlook: the
//! arrow keys move a day or a week, Enter picks the day, Escape closes the calendar (an open list
//! keeps Escape, #406), and the keyboard goes back to the field; Tab goes on to Today. Page Up and
//! Page Down move a month, which Outlook's don't; Space does nothing, as in Outlook.

use egui::accesskit::Role;
use egui::{Align2, Id, Key, Modifiers, Rect, Sense, Stroke, Ui, WidgetInfo, pos2, vec2};
use mailcraft_time::{Date, Weekday};

use crate::icons;
use crate::theme::{Tokens, regular, semibold};
use crate::widgets::icon_button;

/// The size of a day in the grid.
const CELL: egui::Vec2 = egui::Vec2::new(30.0, 24.0);

/// What was picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picked {
    Day(Date),
    /// None: the field emptied.
    None,
}

/// The calendar of one date field.
pub struct Calendar<'a> {
    /// The field it fills: its state is kept under this id, and the keyboard goes back to it.
    pub field: Id,
    /// The day the field holds, when it holds one.
    pub current: Option<Date>,
    pub today: Date,
    pub week_start: Weekday,
    /// Whether None is offered: where the field may be empty.
    pub offer_none: bool,
    /// The button's name, for its tooltip and for screen readers.
    pub name: &'a str,
}

/// The month shown and the day the keyboard is on, while the calendar is open.
#[derive(Clone, Copy, Debug)]
struct Open {
    month: Date,
    active: Date,
    /// The keyboard was in the grid the last frame.
    focused: bool,
    /// Just opened: the keyboard goes into the grid.
    fresh: bool,
}

/// The first and last days of `Date`'s range.
fn first_day() -> Date {
    Date::from_days(i64::MIN)
}

fn last_day() -> Date {
    Date::from_days(i64::MAX)
}

/// The grid's id for the field's calendar.
pub fn grid_id(field: Id) -> Id {
    field.with("date_picker_grid")
}

/// `day` moved by `key` in the month's grid; `None` for a key the grid doesn't take. Page Up and
/// Page Down keep the day of the month, the month's last when it hasn't got it.
pub fn moved(day: Date, key: Key) -> Option<Date> {
    Some(match key {
        Key::ArrowLeft => day.add_days(-1),
        Key::ArrowRight => day.add_days(1),
        Key::ArrowUp => day.add_days(-7),
        Key::ArrowDown => day.add_days(7),
        Key::PageUp => day.add_months(-1),
        Key::PageDown => day.add_months(1),
        _ => return None,
    })
}

impl Calendar<'_> {
    /// Draws the calendar button and, while it is open, the calendar; gives back what was picked.
    pub fn show(self, ui: &mut Ui) -> Option<Picked> {
        let state_id = self.field.with("date_picker");
        let grid = grid_id(self.field);
        let button = icon_button(ui, "calendar", self.name, 14.0, true);
        button.widget_info(|| WidgetInfo::labeled(Role::Button, true, self.name));
        let popup_id = egui::Popup::default_response_id(&button);
        let was: Option<Open> = ui.ctx().data(|d| d.get_temp(state_id));
        if !egui::Popup::is_id_open(ui.ctx(), popup_id) {
            // Until it opens, it opens on the field's day.
            let day = self.current.unwrap_or(self.today);
            ui.ctx().data_mut(|d| d.insert_temp(state_id, Open { month: day.start_of_month(), active: day, focused: false, fresh: true }));
        }
        let mut picked = None;
        let shown = egui::Popup::from_toggle_button_response(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| picked = self.month(ui, state_id, grid));
        let open = shown.is_some() && egui::Popup::is_id_open(ui.ctx(), popup_id);
        if picked.is_some() {
            egui::Popup::close_id(ui.ctx(), popup_id);
        }
        // Closed by a pick, Today, None or Escape while the keyboard was in it: the keyboard
        // goes back to the field. (A click elsewhere leaves it where the click put it.)
        let closed = was.is_some_and(|w| w.focused) && (!open || picked.is_some());
        let clicked_elsewhere = ui.input(|i| i.pointer.any_click()) && picked.is_none();
        // (Today or None picked from the keyboard has the keyboard as it goes.)
        let lost = picked.is_some() || ui.ctx().memory(|m| m.focused().is_none_or(|f| f == grid));
        if closed && !clicked_elsewhere && lost {
            ui.ctx().memory_mut(|m| m.request_focus(self.field));
            // And a held Escape keeps it there.
            crate::keys::give_back_on_escape(ui.ctx(), self.field);
        }
        if !open {
            ui.ctx().data_mut(|d| d.remove::<Open>(state_id));
        }
        picked
    }

    /// The open calendar: the month's heading with its arrows, the weekdays, the days, Today and
    /// None.
    fn month(&self, ui: &mut Ui, state_id: Id, grid: Id) -> Option<Picked> {
        let t = Tokens::get(ui.ctx());
        let mut open = ui.ctx().data(|d| d.get_temp::<Open>(state_id))?;
        let mut picked = None;
        let width = 7.0 * CELL.x;
        ui.set_width(width);
        // The heading: the month between the arrows to the months either side, which stop at the
        // calendar's ends.
        let (r, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
        ui.painter().text(r.center(), Align2::CENTER_CENTER, crate::dates::month_year(open.month), semibold(13.0), t.text);
        let arrows = [
            ("previous", tl!("Previous Month"), -1, r.min.x + 12.0, open.month != first_day().start_of_month()),
            ("next", tl!("Next Month"), 1, r.max.x - 12.0, open.month != last_day().start_of_month()),
        ];
        for (icon, tip, by, x, enabled) in arrows {
            let br = Rect::from_center_size(pos2(x, r.center().y), vec2(24.0, 24.0));
            let resp = ui.interact(br, state_id.with(by), if enabled { Sense::CLICK } else { Sense::hover() });
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, tip));
            if enabled && resp.hovered() {
                ui.painter().rect_filled(br, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
            }
            let ink = if enabled { t.icon } else { t.text_disabled };
            icons::paint(ui.painter(), Rect::from_center_size(br.center(), vec2(12.0, 12.0)), icon, ink, if enabled { t.accent } else { ink });
            if enabled && resp.on_hover_text(tip).clicked() {
                open.month = open.month.add_months(by);
                // The keyboard's day follows, to the same day of that month.
                open.active = open.month.add_days(i64::from(open.active.day.min(open.month.end_of_month().day)) - 1);
                // And the keyboard stays in the grid.
                open.fresh = true;
            }
        }
        // The weekdays, from the first day of the week in the options, as the language writes them.
        let (r, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
        for i in 0..7u8 {
            let weekday = Weekday::from_index(i64::from(self.week_start.index()) + i64::from(i));
            let x = r.min.x + (f32::from(i) + 0.5) * CELL.x;
            ui.painter().text(pos2(x, r.center().y), Align2::CENTER_CENTER, tl!(weekday.short()), regular(10.0), t.text_dim);
        }
        // The days: one widget has the keyboard, on one day; each day is clicked, and named for
        // screen readers, on its own.
        let (area, _) = ui.allocate_exact_size(vec2(width, 6.0 * CELL.y), Sense::hover());
        let keys = ui.interact(area, grid, Sense::click());
        if open.fresh {
            keys.request_focus();
            open.fresh = false;
        }
        let has_keys = keys.has_focus();
        if has_keys {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(grid, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, tab: false, escape: false })
            });
            for key in [Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown, Key::PageUp, Key::PageDown] {
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, key))
                    && let Some(day) = moved(open.active, key)
                {
                    open.active = day;
                    open.month = day.start_of_month();
                }
            }
            // egui clicks what has the keyboard on Enter and on Space; only Enter picks, so a held
            // Space can't pick and then type into the field.
            let space = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::Space, pressed: true, .. })));
            if keys.clicked() && keys.interact_pointer_pos().is_none() && !space {
                picked = Some(Picked::Day(open.active));
            }
        }
        keys.widget_info(|| WidgetInfo::selected(Role::Grid, true, self.current == Some(open.active), crate::dates::long(open.active)));
        // Six weeks from the start of the week the month begins in; none past the calendar's ends.
        let back = (i64::from(open.month.weekday().index()) - i64::from(self.week_start.index())).rem_euclid(7);
        for n in 0..42i64 {
            let days = open.month.to_days() - back + n;
            if days < first_day().to_days() || days > last_day().to_days() {
                continue;
            }
            let day = Date::from_days(days);
            let cell = Rect::from_min_size(pos2(area.min.x + (n % 7) as f32 * CELL.x, area.min.y + (n / 7) as f32 * CELL.y), CELL);
            let resp = ui.interact(cell, state_id.with(("day", days)), Sense::CLICK);
            let current = self.current == Some(day);
            resp.widget_info(|| WidgetInfo::selected(Role::Button, true, current, crate::dates::long(day)));
            paint_day(ui, &t, cell, day, open, current, day == self.today, resp.hovered(), has_keys);
            if resp.clicked() {
                picked = Some(Picked::Day(day));
            }
        }
        ui.add_space(4.0);
        let mut in_buttons = false;
        ui.horizontal(|ui| {
            let today = ui.button(tl!("Today"));
            in_buttons |= today.has_focus();
            if today.clicked() {
                picked = Some(Picked::Day(self.today));
            }
            if self.offer_none {
                let none = ui.button(tl!("None"));
                in_buttons |= none.has_focus();
                if none.clicked() {
                    picked = Some(Picked::None);
                }
            }
        });
        open.focused = has_keys || in_buttons;
        ui.ctx().data_mut(|d| d.insert_temp(state_id, open));
        picked
    }
}

/// One day of the grid: filled when the field holds it, outlined when it is today, a ring round
/// the keyboard's day, dimmed when it is another month's.
#[allow(clippy::too_many_arguments)]
fn paint_day(ui: &Ui, t: &Tokens, cell: Rect, day: Date, open: Open, current: bool, today: bool, hovered: bool, keys: bool) {
    let dot = cell.shrink2(vec2(2.0, 1.0));
    let p = ui.painter();
    if current {
        p.rect_filled(dot, 4.0, t.accent);
    } else if hovered {
        p.rect_filled(dot, 4.0, t.hover);
    }
    if today && !current {
        p.rect_stroke(dot, 4.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    }
    if keys && day == open.active {
        p.rect_stroke(dot.expand(1.0), 5.0, Stroke::new(2.0, t.text), egui::StrokeKind::Outside);
    }
    let other_month = day.month != open.month.month || day.year != open.month.year;
    let ink = if current {
        t.on_accent
    } else if other_month {
        t.text_dim
    } else {
        t.text
    };
    p.text(cell.center(), Align2::CENTER_CENTER, day.day.to_string(), regular(12.0), ink);
}

#[cfg(test)]
#[path = "date_picker_tests.rs"]
mod date_picker_tests;
