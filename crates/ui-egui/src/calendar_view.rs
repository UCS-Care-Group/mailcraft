// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar: the side pane (date navigator, calendars by group), the header strip, and the day,
//! work-week, week and month grids, with what the pointer and the keyboard do in them: picking a
//! time and typing into it, moving, resizing and copying events by dragging, and the keys of the
//! grid.
//!
//! Several calendars that are shown stand side by side, each in a block of its own with a tab
//! over it; the arrow on a tab lays a calendar over the others (overlay), where the one in front
//! has its items solid and the rest dimmer. What is picked, typed, dragged or dropped in a block
//! is that block's calendar's.
//!
//! The view reads the session and draws it; whatever it changes, it changes by running a command.

use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use mailcraft_engine::CalView;
use mailcraft_engine::cmd::calendar::occurrences;
use mailcraft_engine::cmd::calendar_extra::{DEFAULT_TIME_SCALE, MAX_DAYS, TIME_SCALES, time_scale};
use mailcraft_engine::cmd::calendar_settings::{self, MAX_PANES};
use mailcraft_engine::cmd::view::calendar_range;
use mailcraft_model::{Event, Freq, Id, Importance, Occurrence, Recurrence, Response as Reply, ShowAs};
use mailcraft_time::{Date, DateTime, Weekday};
use serde_json::{Value, json};

use crate::theme::{Tokens, c32, medium, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// Height of the strip with Today, Back and Forward and the days in view.
const HEADER_H: f32 = 46.0;
/// Width of the hour labels beside the time grid.
const GUTTER_W: f32 = 62.0;
/// Height of the weekday and day-number row above the time grid.
const DAY_HEAD_H: f32 = 52.0;
/// Height of one row of the hours: half an hour at the usual time scale, an hour or five
/// minutes at the others ([`Scale`]).
const ROW_H: f32 = 24.0;
/// Height of an hour at the usual time scale.
#[cfg(test)]
const HOUR_H: f32 = 48.0;
/// The least height of an event block, however short the event.
const MIN_BLOCK_H: f32 = 18.0;
/// Height of an all-day bar and of a month chip, and the gap under each.
const BAR_H: f32 = 20.0;
const BAR_GAP: f32 = 2.0;
/// Width of the coloured edge on the left of a block or bar.
const EDGE_W: f32 = 5.0;
/// Height of the weekday row above the month grid, and of the day number in a month cell; and
/// the width of the column of week numbers left of the weeks, when they are shown.
const MONTH_HEAD_H: f32 = 28.0;
const NUM_H: f32 = 26.0;
const WEEK_NUMBER_W: f32 = 30.0;
/// The date navigator: height of a week, of the weekday initials over a month and of its name;
/// what a month takes in all; the margin left of the weeks (a click there shows the week, and
/// the week numbers go there); and the most months it shows one under the other.
const MINI_ROW_H: f32 = 22.0;
const MINI_HEAD_H: f32 = 18.0;
const MINI_TITLE_H: f32 = 26.0;
const MINI_MONTH_H: f32 = MINI_TITLE_H + MINI_HEAD_H + 6.0 * MINI_ROW_H + 6.0;
const MINI_MARGIN: f32 = 18.0;
const MAX_MINI_MONTHS: usize = 6;
const DAY_MINUTES: i64 = 24 * 60;
/// Dragging an event snaps to this many minutes, or to the time scale's row when that is finer.
const DRAG_STEP: i64 = 15;
/// How fast the hours scroll while an event is dragged against their top or bottom edge, in
/// points a second.
const AUTO_SCROLL: f32 = 240.0;
/// Most occurrences laid out in one frame, blocks in one day column, bars in one row of days, and
/// rows in one month cell: a hostile calendar costs bounded work.
const MAX_ITEMS: usize = 5000;
const MAX_BLOCKS: usize = 200;
const MAX_BARS: usize = 200;
const MAX_ROWS: usize = 24;

/// Where the mini month, the time grid's first scroll and a drag keep their state between frames.
const MINI_MONTH_ID: &str = "calendar_mini_month";
const NAV_DRAG_ID: &str = "calendar_nav_drag";
const GRID_VIEW_ID: &str = "calendar_grid_view";
const DRAG_ID: &str = "calendar_drag";
/// Where the question a drop with the right button asks is kept until it is answered.
const DROP_MENU_ID: &str = "calendar_drop_menu";
/// Where the last frame leaves the next one where the parts of the calendar were, and whether
/// the calendar had the keyboard; and where an event that was copied or cut waits for Paste.
const ZONES_ID: &str = "calendar_zones";
const KEYS_ID: &str = "calendar_keys";
const CLIP_ID: &str = "calendar_clip";
/// Where the stretch of empty calendar that is picked out is kept, and the field that is typed
/// into it.
const SLOT_ID: &str = "calendar_slot";
const SLOT_ENTRY_ID: &str = "calendar_slot_entry";
/// Longest title typed into a slot.
const MAX_TITLE: usize = 500;
/// Height of the tabs over the calendars, when more than one is shown; the room between two
/// blocks of calendars side by side; and the narrowest a tab is drawn with its name.
const TAB_H: f32 = 28.0;
const PANE_GAP: f32 = 10.0;
const MIN_TAB_W: f32 = 30.0;
/// Most groups of calendars whose folding is remembered, and the longest heading of one.
const MAX_FOLDED: usize = 200;
const MAX_HEADING: usize = 200;

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    // A picked-out slot that wasn't drawn in the last frame (another view, other days) is
    // forgotten; and picking an event lets go of the slot.
    if let Some(slot) = load_slot(ui.ctx())
        && (slot.frame.saturating_add(1) < ui.ctx().cumulative_frame_nr() || (slot.text.is_none() && app.session.sel.event.is_some()))
    {
        store_slot(ui.ctx(), None);
    }
    if app.session.view.folder_pane {
        side_pane(app, ui);
    }
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let mut frame = Frame::begin(ui);
        header(app, ui);
        connect_notice(app, ui);
        local_notice(app, ui);
        // After the header, whose buttons change the range; and after the list of calendars,
        // whose check boxes change what is shown.
        let (mut from, mut to) = calendar_range(&app.session);
        let mut panes = pane_views(app);
        // A pick belongs to its block: to the calendar in front there, whichever that is now.
        if let Some(mut slot) = load_slot(ui.ctx()) {
            let front = panes.get(pane_of(&panes, slot.calendar)).map_or(0, |pane| pane.front);
            if slot.calendar != front {
                slot.calendar = front;
                store_slot(ui.ctx(), Some(slot));
            }
        }
        let mut items = gather(app, from, to, &t, &panes);
        // The keys of the grid, before it is drawn so that it shows what they did. One that
        // goes to other days has those days drawn.
        if keyboard(app, ui, &mut frame, &items) {
            (from, to) = calendar_range(&app.session);
            panes = pane_views(app);
            items = gather(app, from, to, &t, &panes);
        }
        if app.session.view.calendar == CalView::Month {
            month_grid(app, ui, &mut frame, from, to, &items, &panes);
        } else {
            time_grid(app, ui, &mut frame, from, to, &items, &panes);
        }
        // Last, when every part has said where it is: a drag may end in another part than it
        // began in.
        frame.finish(app, ui, &items);
        drop_menu(app, ui, &frame.zones, &items);
        // For the next frame: where everything was, and whether the keyboard was the calendar's.
        let free = has_keyboard(app, ui.ctx());
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::unique(ZONES_ID), frame.zones);
            d.insert_temp(egui::Id::unique(KEYS_ID), free);
        });
    });
}

// ---- pure parts ----

/// Round `minute` to the nearest multiple of `step`.
pub(crate) fn snap(minute: i64, step: i64) -> i64 {
    let step = step.max(1);
    minute.saturating_add(step / 2).div_euclid(step).saturating_mul(step)
}

/// Round `minute` down to a multiple of `step`.
pub(crate) fn snap_down(minute: i64, step: i64) -> i64 {
    let step = step.max(1);
    minute.div_euclid(step).saturating_mul(step)
}

/// How the hours are drawn: what one row of them stands for (the time scale), and what follows
/// from that.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Scale {
    /// The minutes a row stands for: what a click picks out, and how long a new appointment is.
    pub(crate) slot: i64,
    /// Height of an hour, in points.
    pub(crate) hour_h: f32,
}

impl Scale {
    /// The scale of `minutes` a row; one that isn't offered is the usual one.
    pub(crate) fn of(minutes: u8) -> Scale {
        let slot = i64::from(if TIME_SCALES.contains(&minutes) { minutes } else { DEFAULT_TIME_SCALE });
        Scale { slot, hour_h: ROW_H * 60.0 / slot as f32 }
    }

    /// What dragging an event snaps to: a quarter of an hour, or the row when that is finer.
    pub(crate) fn step(self) -> i64 {
        self.slot.clamp(1, DRAG_STEP)
    }

    /// How far below midnight `minute` of the day is.
    pub(crate) fn y(self, minute: i64) -> f32 {
        minute.clamp(0, DAY_MINUTES) as f32 * self.hour_h / 60.0
    }

    /// The minute of the day `y` points below midnight (0 to 1440).
    pub(crate) fn minute(self, y: f32) -> i64 {
        if !y.is_finite() || self.hour_h.is_nan() || self.hour_h <= 0.0 {
            return 0;
        }
        ((y / self.hour_h * 60.0).floor() as i64).clamp(0, DAY_MINUTES)
    }

    /// Height of the whole day.
    fn height(self) -> f32 {
        24.0 * self.hour_h
    }
}

/// Where a dragged block starts: `origin` minutes into its day, moved by `delta` minutes, snapped
/// to `step` and kept inside the day. A move of less than half a step leaves it where it was.
pub(crate) fn drag_minute(origin: i64, delta: i64, step: i64) -> i64 {
    let step = step.clamp(1, DAY_MINUTES);
    let origin = origin.clamp(0, DAY_MINUTES - 1);
    if delta.saturating_abs().saturating_mul(2) < step {
        return origin;
    }
    snap(origin.saturating_add(delta), step).clamp(0, DAY_MINUTES - step)
}

/// The row of the hours at `minute` of `day`, `slot` minutes long (half an hour at the usual
/// time scale): what a click there picks out, and a double click makes an appointment of.
pub(crate) fn slot_at(day: Date, minute: i64, slot: i64) -> (DateTime, DateTime) {
    let slot = slot.clamp(1, DAY_MINUTES);
    let start = snap_down(minute.clamp(0, DAY_MINUTES), slot).min(DAY_MINUTES - slot);
    let start = DateTime::at_midnight(day).add_minutes(start);
    (start, start.add_minutes(slot))
}

/// A stretch of empty calendar that is picked out, as a click or a drag over the grid (or the
/// arrow keys) leaves it: rows of one day in the time grid, whole days in the month and in the
/// all-day row. What is typed then starts an entry right there, and Enter makes an appointment
/// of it.
/// What the menu over a picked range makes of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Appointment,
    Meeting,
    AllDay,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Slot {
    /// From, and up to (not including).
    pub(crate) start: DateTime,
    pub(crate) end: DateTime,
    pub(crate) all_day: bool,
    /// Where the pick began, for a drag (or Shift with an arrow) that goes on from there.
    anchor: DateTime,
    /// Where it has got to: the row, or the day, at its other end.
    cursor: DateTime,
    /// The minutes of one row, for a pick of times.
    step: i64,
    /// What is being typed; `None` while the stretch is only picked out.
    pub(crate) text: Option<String>,
    /// The field for the text is new: it takes the keyboard.
    fresh: bool,
    /// The frame it was last drawn in.
    frame: u64,
    /// The calendar whose block it is picked in, which is where what is typed goes; 0 for the
    /// one being worked in (no calendar is shown).
    pub(crate) calendar: Id,
}

impl Slot {
    /// The row at `minute` of `day`, `step` minutes long.
    pub(crate) fn timed(day: Date, minute: i64, step: i64) -> Slot {
        let step = step.clamp(1, DAY_MINUTES);
        let (start, end) = slot_at(day, minute, step);
        Slot { start, end, all_day: false, anchor: start, cursor: start, step, text: None, fresh: false, frame: 0, calendar: 0 }
    }

    /// The whole of `day`.
    pub(crate) fn day(day: Date) -> Slot {
        let start = DateTime::at_midnight(day);
        let (end, step) = (start.add_days(1), DAY_MINUTES);
        Slot { start, end, all_day: true, anchor: start, cursor: start, step, text: None, fresh: false, frame: 0, calendar: 0 }
    }

    /// The same pick in the block of `calendar`.
    pub(crate) fn in_calendar(self, calendar: Id) -> Slot {
        Slot { calendar, ..self }
    }

    /// Stretch a pick of rows from where it began to the one at `minute` of its day.
    pub(crate) fn stretch_to(&mut self, minute: i64) {
        let (start, end) = slot_at(self.anchor.date, minute, self.step);
        self.cursor = start;
        self.start = self.anchor.min(start);
        self.end = self.anchor.add_minutes(self.step).max(end);
    }

    /// Stretch a pick of days from where it began to `day`.
    pub(crate) fn stretch_to_day(&mut self, day: Date) {
        let day = DateTime::at_midnight(day);
        self.cursor = day;
        self.start = self.anchor.min(day);
        self.end = self.anchor.max(day).add_days(1);
    }

    /// The same pick on another day, in rows of `step` minutes: the same rows of that day, or
    /// the whole of it.
    fn on_day(&self, day: Date, step: i64) -> Slot {
        if self.all_day {
            return Slot::day(day).in_calendar(self.calendar);
        }
        let mut slot = Slot::timed(day, i64::from(self.anchor.minutes_of_day()), step).in_calendar(self.calendar);
        slot.stretch_to(i64::from(self.cursor.minutes_of_day()));
        slot
    }

    /// The last day it touches.
    fn last(&self) -> Date {
        last_day(self.start, self.end)
    }

    /// The first and the last minute of its day, for a pick of half hours.
    fn minutes(&self) -> (i64, i64) {
        let start = i64::from(self.start.minutes_of_day());
        let end = if self.end.date > self.start.date { DAY_MINUTES } else { i64::from(self.end.minutes_of_day()) };
        (start, end.max(start + 1))
    }

    /// What `calendar.new` is given to make an appointment called `title` of it, in the calendar
    /// of its block.
    pub(crate) fn appointment(&self, title: &str) -> Value {
        let made = if self.all_day {
            json!({"title": title, "start": self.start.date.iso(), "end": self.last().iso(), "allDay": true})
        } else {
            json!({"title": title, "start": iso_minute(self.start), "end": iso_minute(self.end)})
        };
        self.there(made)
    }

    /// What `ui.openWindow` is given for the window a double click (or Enter) opens for it: a
    /// new appointment for that time, in the calendar of its block.
    fn form(&self) -> Value {
        self.form_for(Kind::Appointment)
    }

    /// The same, for the kind of item the menu over a pick chose.
    fn form_for(&self, kind: Kind) -> Value {
        let all_day = self.all_day || kind == Kind::AllDay;
        let new = if kind == Kind::Meeting { "meeting" } else { "appointment" };
        let form = if all_day {
            json!({"new": new, "start": self.start.date.iso(), "end": self.last().iso(), "allDay": true})
        } else {
            json!({"new": new, "start": iso_minute(self.start), "end": iso_minute(self.end)})
        };
        self.there(form)
    }

    /// Whether `other`, a pick at the place of a click, lies inside this pick (of the same kind
    /// and calendar, not being typed into): a right click there leaves the pick as it is.
    fn covers(&self, other: &Slot) -> bool {
        self.text.is_none() && self.all_day == other.all_day && self.calendar == other.calendar && other.start >= self.start && other.start < self.end
    }

    /// `params` with the calendar of the block the pick is in, when it is in one.
    fn there(&self, mut params: Value) -> Value {
        if let (true, Some(fields)) = (self.calendar != 0, params.as_object_mut()) {
            fields.insert("calendar".into(), json!(self.calendar));
        }
        params
    }
}

fn load_slot(ctx: &egui::Context) -> Option<Slot> {
    ctx.data(|d| d.get_temp(egui::Id::unique(SLOT_ID)))
}

fn store_slot(ctx: &egui::Context, slot: Option<Slot>) {
    let id = egui::Id::unique(SLOT_ID);
    ctx.data_mut(|d| match slot {
        Some(slot) => {
            d.insert_temp(id, slot);
        }
        None => d.remove::<Slot>(id),
    });
}

/// Keep the pick a part of the calendar ended the frame with. `mine` says which picks are that
/// part's (half hours, or whole days): the other part's is left alone unless this one replaced it.
fn keep_slot(ctx: &egui::Context, was: &Option<Slot>, mut now: Option<Slot>, mine: impl Fn(&Slot) -> bool) {
    if now.is_none() && !was.as_ref().is_some_and(mine) {
        return;
    }
    // It was looked after this frame: a pick nobody looks after any more is forgotten.
    let frame = ctx.cumulative_frame_nr();
    if let Some(slot) = &mut now {
        slot.frame = frame;
    }
    let same = match (was, &now) {
        (Some(was), Some(now)) => Slot { frame, ..was.clone() } == *now,
        (None, None) => true,
        _ => false,
    };
    store_slot(ctx, now);
    if !same {
        ctx.request_repaint();
    }
}

/// Whether what is typed is the calendar's: nothing else has the keyboard (a field, a dialog,
/// File, a menu, a window that lies over the calendar).
fn has_keyboard(app: &MailApp, ctx: &egui::Context) -> bool {
    app.dialog.is_none()
        && !app.ui.backstage
        && !app.windows.over_main()
        && ctx.memory(|m| m.focused().is_none())
        && !egui::Popup::is_any_open(ctx)
        && ctx.data(|d| d.get_temp::<DropMenu>(egui::Id::unique(DROP_MENU_ID)).is_none())
}

/// What became of a picked-out slot this frame.
enum Entry {
    /// It stays as it is.
    Stay,
    /// It is let go of (Escape).
    Drop,
    /// What was typed is to become an appointment.
    Make(String),
    /// The full form is asked for (Enter on a slot nothing was typed into).
    Form,
}

/// Draw the picked-out `slot` at `rect` (its mark when `mark`: the month and the all-day row
/// tint their cells themselves) and let the owner type into it. Typing starts an entry in the
/// slot; Enter makes the appointment; Escape gives the entry up; a click anywhere else makes the
/// appointment when something was typed, as leaving the field with Tab does.
fn slot_entry(app: &mut MailApp, ui: &mut Ui, rect: Rect, slot: &mut Slot, mark: bool) -> Entry {
    let t = Tokens::get(ui.ctx());
    slot.frame = ui.ctx().cumulative_frame_nr();
    let p = ui.painter().clone();
    if mark {
        p.rect_filled(rect, 2.0, t.selection);
        p.rect_stroke(rect, 2.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
    }
    let entry = egui::Id::unique(SLOT_ENTRY_ID);
    let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    if slot.text.is_none() {
        // The keyboard is the calendar's while nothing else has it.
        if !has_keyboard(app, ui.ctx()) {
            return Entry::Stay;
        }
        if escape {
            return Entry::Drop;
        }
        if enter {
            return Entry::Form;
        }
        // (What comes with Alt or Ctrl held is a key of the calendar's, not text.)
        let typed: String = ui.input(|i| {
            let chord = i.modifiers.alt || i.modifiers.command;
            let text = i.events.iter().filter_map(|e| if let egui::Event::Text(text) = e { Some(text.as_str()) } else { None });
            text.flat_map(str::chars).filter(|c| !c.is_control() && !chord).take(MAX_TITLE).collect()
        });
        if typed.trim().is_empty() {
            return Entry::Stay;
        }
        // What was typed is shown at once; the field comes with the next frame, and takes what
        // is typed from then on.
        let field = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y.max(rect.min.y + BAR_H)));
        p.rect_filled(field, 2.0, t.surface);
        p.rect_stroke(field, 2.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
        paint_clipped(&p, pos2(field.min.x + 6.0, field.min.y + BAR_H / 2.0), field.width() - 10.0, &typed, semibold(11.5), t.text);
        slot.text = Some(typed);
        slot.fresh = true;
        ui.ctx().request_repaint();
        return Entry::Stay;
    }
    let Some(text) = slot.text.as_mut() else { return Entry::Stay };
    let field = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y.max(rect.min.y + BAR_H)));
    p.rect_filled(field, 2.0, t.surface);
    p.rect_stroke(field, 2.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
    // A press anywhere else ends the entry, and what was typed becomes an appointment.
    let away = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|pos| !field.contains(pos)));
    if away {
        let title = text.trim().to_string();
        slot.text = None;
        return if title.is_empty() { Entry::Stay } else { Entry::Make(title) };
    }
    if std::mem::take(&mut slot.fresh) {
        ui.memory_mut(|m| m.request_focus(entry));
    }
    let inner = Rect::from_min_max(pos2(field.min.x + 4.0, field.min.y), pos2(field.max.x - 2.0, field.min.y + BAR_H.max(20.0)));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let edit =
        egui::TextEdit::singleline(text).id(entry).frame(egui::Frame::NONE).desired_width(inner.width()).font(semibold(11.5)).char_limit(MAX_TITLE);
    let resp = child.add(edit);
    if resp.lost_focus() {
        let title = text.trim().to_string();
        slot.text = None;
        return if escape || title.is_empty() { Entry::Stay } else { Entry::Make(title) };
    }
    Entry::Stay
}

/// Whether an event can go to another day as a whole. A series tied to days of the week, or
/// one that happens every day, keeps its days: only its time changes, and an occurrence of it
/// that is taken to another day goes there on its own ([`alone`]).
pub(crate) fn moves_across_days(rule: Option<&Recurrence>) -> bool {
    rule.is_none_or(|r| r.by_day.is_empty() && r.freq != Freq::Daily)
}

/// `2026-10-14T09:30`, as commands read a date and time.
pub(crate) fn iso_minute(t: DateTime) -> String {
    format!("{}T{:02}:{:02}", t.date.iso(), t.hour, t.minute)
}

/// The days `from` to `to`, at most `cap` of them and at least one.
pub(crate) fn days_of(from: Date, to: Date, cap: i64) -> Vec<Date> {
    let n = from.days_until(to).clamp(0, cap.max(1) - 1);
    (0..=n).map(|i| from.add_days(i)).collect()
}

/// The last day an occurrence touches: one that ends at midnight doesn't reach into the next day.
pub(crate) fn last_day(start: DateTime, end: DateTime) -> Date {
    if end > start { end.add_minutes(-1).date.max(start.date) } else { start.date }
}

/// The title of the header strip: `October 14, 2026`, `October 11 – 17, 2026`,
/// `September 28 – October 4, 2026`, `October 2026`.
pub(crate) fn range_title(view: CalView, date: Date, from: Date, to: Date) -> String {
    let fmt = crate::i18n::fmt;
    let year = |d: Date| d.year.to_string();
    match view {
        CalView::Month => crate::dates::month_year(date),
        // (The Day view names its day, or its days when it shows several.)
        CalView::Day | CalView::Week | CalView::WorkWeek => {
            use crate::dates::{long_without_weekday, month_day_long, month_name};
            if from.year != to.year {
                format!("{} – {}", long_without_weekday(from), long_without_weekday(to))
            } else if from.month != to.month {
                fmt(tl!("{from} – {to}, {year}"), &[("from", &month_day_long(from)), ("to", &month_day_long(to)), ("year", &year(to))])
            } else if from.day != to.day {
                let (first, last) = (from.day.to_string(), to.day.to_string());
                fmt(
                    tl!("{month} {first} – {last}, {year}"),
                    &[("month", month_name(from.month)), ("first", &first), ("last", &last), ("year", &year(to))],
                )
            } else {
                long_without_weekday(from)
            }
        }
    }
}

/// Side-by-side columns for blocks that overlap. `spans` are `(start, end)` in minutes, sorted by
/// start; the answer gives each block its column and the number of columns in its cluster, so a
/// block is `1 / columns` of the day wide. A block without a length counts as one minute.
pub(crate) fn pack_columns(spans: &[(i64, i64)]) -> Vec<(usize, usize)> {
    let mut out = vec![(0usize, 1usize); spans.len()];
    // Where the last block of each column ends, for the cluster being built.
    let mut ends: Vec<i64> = Vec::new();
    let (mut cluster, mut cluster_end) = (0usize, i64::MIN);
    let close = |out: &mut Vec<(usize, usize)>, range: std::ops::Range<usize>, columns: usize| {
        for slot in out.iter_mut().take(range.end).skip(range.start) {
            slot.1 = columns.max(1);
        }
    };
    for (i, &(start, end)) in spans.iter().enumerate() {
        let end = end.max(start.saturating_add(1));
        // Nothing before reaches this far: the cluster is complete.
        if i > cluster && start >= cluster_end {
            close(&mut out, cluster..i, ends.len());
            ends.clear();
            cluster = i;
            cluster_end = i64::MIN;
        }
        let column = match ends.iter().position(|e| *e <= start) {
            Some(c) => c,
            None => {
                ends.push(end);
                ends.len() - 1
            }
        };
        if let Some(e) = ends.get_mut(column) {
            *e = end;
        }
        if let Some(slot) = out.get_mut(i) {
            slot.0 = column;
        }
        cluster_end = cluster_end.max(end);
    }
    close(&mut out, cluster..spans.len(), ends.len());
    out
}

/// Lanes for bars that span days: each `(first, last)` day span, in the order given, gets the
/// first lane that is free on all of its days.
pub(crate) fn assign_lanes(spans: &[(usize, usize)]) -> Vec<usize> {
    // The last day taken in each lane.
    let mut lanes: Vec<usize> = Vec::new();
    spans
        .iter()
        .map(|&(first, last)| {
            let last = last.max(first);
            match lanes.iter().position(|taken| *taken < first) {
                Some(lane) => {
                    if let Some(taken) = lanes.get_mut(lane) {
                        *taken = last;
                    }
                    lane
                }
                None => {
                    lanes.push(last);
                    lanes.len() - 1
                }
            }
        })
        .collect()
}

/// How many rows of a month cell with room for `rows` hold events when showing everything takes
/// `needed` rows: all of them, or one less to leave a row for `+n more`.
pub(crate) fn chip_limit(needed: usize, rows: usize) -> usize {
    if needed <= rows { rows } else { rows.saturating_sub(1) }
}

/// One week of the month grid: bars in lanes across its days, and each day's timed events in the
/// rows the bars leave free.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct WeekLayout {
    /// `(item, first day, last day, row)`, days counted from the start of the week.
    pub bars: Vec<(usize, usize, usize, usize)>,
    /// `(item, day, row)`.
    pub chips: Vec<(usize, usize, usize)>,
    /// Everything that happens on each day, laid out or not.
    pub totals: [usize; 7],
    /// The rows each day needs to show everything.
    pub rows: [usize; 7],
}

/// Lay out week number `week` of a month grid. `spans` gives, for each item, the first and last
/// day it touches (counted from the first day of the grid) and whether it is drawn as a bar; a
/// timed event shows on the day it starts.
pub(crate) fn layout_week(spans: &[(i64, i64, bool)], week: i64) -> WeekLayout {
    let lo = week.saturating_mul(7);
    let hi = lo.saturating_add(6);
    let day_of = |n: i64| usize::try_from(n.clamp(lo, hi) - lo).unwrap_or(0).min(6);
    let mut out = WeekLayout::default();
    let mut bars: Vec<(usize, usize, usize)> = Vec::new();
    let mut timed: Vec<(usize, usize)> = Vec::new();
    for (item, &(first, last, bar)) in spans.iter().enumerate() {
        let last = last.max(first);
        if bar {
            if last < lo || first > hi {
                continue;
            }
            let (a, b) = (day_of(first), day_of(last));
            for total in out.totals.iter_mut().take(b + 1).skip(a) {
                *total += 1;
            }
            bars.push((item, a, b));
        } else if (lo..=hi).contains(&first) {
            let day = day_of(first);
            if let Some(total) = out.totals.get_mut(day) {
                *total += 1;
            }
            timed.push((item, day));
        }
    }
    // Earlier and longer bars take the upper lanes.
    bars.sort_by_key(|&(item, a, b)| (a, std::cmp::Reverse(b), item));
    bars.truncate(MAX_BARS);
    let lanes = assign_lanes(&bars.iter().map(|&(_, a, b)| (a, b)).collect::<Vec<_>>());
    let mut taken: [Vec<bool>; 7] = Default::default();
    for (&(item, a, b), lane) in bars.iter().zip(lanes) {
        if lane >= MAX_ROWS {
            continue;
        }
        out.bars.push((item, a, b, lane));
        for column in taken.iter_mut().take(b + 1).skip(a) {
            if column.len() <= lane {
                column.resize(lane + 1, false);
            }
            if let Some(slot) = column.get_mut(lane) {
                *slot = true;
            }
        }
    }
    for (item, day) in timed {
        let Some(column) = taken.get_mut(day) else { continue };
        let row = column.iter().position(|used| !*used).unwrap_or(column.len());
        if row >= MAX_ROWS {
            continue;
        }
        if row == column.len() {
            column.push(true);
        } else if let Some(slot) = column.get_mut(row) {
            *slot = true;
        }
        out.chips.push((item, day, row));
    }
    for (day, column) in taken.iter().enumerate() {
        let laid_out = column.iter().filter(|used| **used).count();
        let (Some(rows), Some(total)) = (out.rows.get_mut(day), out.totals.get(day)) else { continue };
        // Whatever the caps left out still needs a row, so the cell says `+n more`.
        *rows = if *total > laid_out { MAX_ROWS + 1 } else { column.len() };
    }
    out
}

/// Text cut to `max` characters: a hostile title costs bounded layout work.
fn clip_text(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => s.get(..i).unwrap_or(s),
        None => s,
    }
}

fn mix(a: Color32, b: Color32, k: f32) -> Color32 {
    a.lerp_to_gamma(b, if k.is_finite() { k.clamp(0.0, 1.0) } else { 0.0 })
}

/// The light or the dark ink of the theme, whichever reads on `bg`.
fn ink_on(bg: Color32, t: &Tokens) -> Color32 {
    let luma = 0.299 * f32::from(bg.r()) + 0.587 * f32::from(bg.g()) + 0.114 * f32::from(bg.b());
    let (light, dark) = if t.dark { (t.text, t.surface) } else { (t.surface, t.text) };
    if luma < 150.0 { light } else { dark }
}

fn text_w(ui: &Ui, text: &str, font: FontId) -> f32 {
    ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, Color32::WHITE).size().x)
}

fn to_index(n: i64) -> usize {
    usize::try_from(n).unwrap_or(0)
}

// ---- what is in view ----

/// A calendar that is shown, as its tab has it.
#[derive(Clone, Debug, PartialEq)]
struct Tab {
    id: Id,
    name: String,
    color: Color32,
    /// The account may read it and not change it.
    read_only: bool,
    /// It lies over other calendars because the owner laid it there (`view.overlay`).
    overlaid: bool,
}

/// One block of the grid as it is drawn: a calendar by itself, or the calendars that lie over
/// one another, with the one of them that is in front.
#[derive(Clone, Debug, Default, PartialEq)]
struct PaneView {
    tabs: Vec<Tab>,
    /// The calendar in front, which is the one worked in there; 0 when no calendar is shown.
    front: Id,
}

/// The blocks of the grid, from left to right ([`calendar_settings::panes`]): always one at
/// least, and no more than there is room for.
fn pane_views(app: &MailApp) -> Vec<PaneView> {
    let mailbox = &app.session.mailbox;
    let overlaid = calendar_settings::overlaid(&app.session);
    let panes = calendar_settings::panes(&app.session);
    // A tab says its calendar's name, and whose calendar it is only where two of those in view
    // are called the same (two accounts' "Calendar").
    let plain = |calendar: &mailcraft_model::Calendar| {
        let name = calendar.name.trim();
        if name.is_empty() { tl!("Calendar").to_string() } else { tl!(clip_text(name, 200)).to_string() }
    };
    let names: Vec<String> = panes.iter().flat_map(|pane| &pane.calendars).filter_map(|id| mailbox.calendar(*id)).take(400).map(plain).collect();
    let mut views: Vec<PaneView> = Vec::new();
    for (n, pane) in panes.into_iter().take(MAX_PANES).enumerate() {
        let tab = |id: &Id| {
            let calendar = mailbox.calendar(*id)?;
            let name = plain(calendar);
            let twice = names.iter().filter(|other| other.eq_ignore_ascii_case(&name)).count() > 1;
            Some(Tab {
                id: calendar.id,
                name: if twice { calendar_label(&app.session, calendar) } else { name },
                color: c32(calendar.shown_color()),
                read_only: read_only(app, calendar.id),
                overlaid: overlaid && n == 0,
            })
        };
        views.push(PaneView { tabs: pane.calendars.iter().take(200).filter_map(tab).collect(), front: pane.front });
    }
    if views.is_empty() {
        views.push(PaneView::default());
    }
    views
}

/// Whether the calendars have tabs over them: there is more than one of them in view.
fn tabbed(panes: &[PaneView]) -> bool {
    panes.len() > 1 || panes.first().is_some_and(|pane| pane.tabs.len() > 1)
}

/// The block that `calendar` is in; the first one for a calendar that is in none.
fn pane_of(panes: &[PaneView], calendar: Id) -> usize {
    panes.iter().position(|pane| pane.tabs.iter().any(|tab| tab.id == calendar)).unwrap_or(0)
}

/// Whether a calendar is one the account may read and not change.
fn read_only(app: &MailApp, calendar: Id) -> bool {
    let mailbox = &app.session.mailbox;
    mailbox.calendar(calendar).is_some_and(|c| !c.can_edit() && mailcraft_engine::cmd::remote::keeper(mailbox, &c.origin).is_some())
}

/// Working in a block makes its calendar the one being worked in (where the ribbon's New
/// Appointment goes too), unless that calendar can only be read.
fn work_in(app: &mut MailApp, calendar: Id) {
    if calendar != 0 && app.session.view.in_calendar != calendar && !read_only(app, calendar) {
        let _ = app.execute("calendar.setCurrent", json!({"id": calendar}));
    }
}

/// One occurrence in view, with its event and its colours.
#[derive(Clone)]
struct Item {
    occ: Occurrence,
    ev: Arc<Event>,
    /// What it is tinted with: its first category's colour, or else its calendar's.
    color: Color32,
    /// Its calendar's colour, which the edge down its left is always in.
    edge: Color32,
    /// The colours of its first few categories.
    tags: Vec<Color32>,
    /// The block it is drawn in.
    pane: usize,
    /// Its calendar lies under another one: it is drawn dimmer.
    dim: bool,
    /// Its calendar shows only when its owner is busy (#70): a block with no title of its own,
    /// which says how the time shows.
    free_busy: bool,
}

impl Item {
    fn minutes(&self) -> i64 {
        self.occ.start.minutes_until(self.occ.end).max(0)
    }

    /// Whole-day events and ones a day long or longer are bars across days, not blocks in the hours.
    fn is_bar(&self) -> bool {
        self.ev.all_day || self.minutes() >= DAY_MINUTES
    }

    fn last_day(&self) -> Date {
        last_day(self.occ.start, self.occ.end)
    }

    fn title(&self) -> &str {
        let title = self.ev.title.trim();
        if !title.is_empty() {
            clip_text(title, 200)
        } else if self.free_busy {
            tl!(self.ev.show_as.label())
        } else {
            tl!("(No title)")
        }
    }

    fn times(&self, twelve: bool) -> String {
        format!("{} – {}", crate::dates::clock(self.occ.start, twelve), crate::dates::clock(self.occ.end, twelve))
    }
}

/// Everything that happens on the days `from` to `to` in the calendars that are shown, each
/// with the block of `panes` it is drawn in.
fn gather(app: &MailApp, from: Date, to: Date, t: &Tokens, panes: &[PaneView]) -> Vec<Item> {
    let mailbox = &app.session.mailbox;
    occurrences(&app.session, from, to)
        .into_iter()
        .take(MAX_ITEMS)
        .filter_map(|occ| {
            // The shared pointer, not a copy of the event.
            let ev = mailbox.events.get(&occ.event)?.clone();
            // An event is in its category's colour when it has one, else in its calendar's;
            // more categories than one show as marks. Its edge is its calendar's colour either
            // way, so calendars side by side or over one another are told apart.
            let edge = mailbox.calendar(ev.calendar).map_or(t.blue, |c| c32(c.shown_color()));
            let color = ev.categories.first().map_or(edge, |name| c32(mailbox.category_color(name)));
            let tags = ev.categories.iter().skip(1).take(3).map(|name| c32(mailbox.category_color(name))).collect();
            let pane = pane_of(panes, ev.calendar);
            let dim = panes.get(pane).is_some_and(|pane| pane.tabs.len() > 1 && pane.front != ev.calendar);
            let free_busy = mailbox.calendar(ev.calendar).is_some_and(|c| c.rights == mailcraft_model::Rights::FreeBusy);
            Some(Item { occ, ev, color, edge, tags, pane, dim, free_busy })
        })
        .collect()
}

fn is_selected(app: &MailApp, item: &Item) -> bool {
    app.session.sel.event == Some(item.ev.id)
        && (item.ev.recurrence.is_none() || app.session.sel.event_day.is_none_or(|day| day == item.occ.start.date))
}

/// Equal columns of days.
#[derive(Clone, Copy, Debug)]
struct Cols {
    left: f32,
    width: f32,
    n: usize,
}

impl Cols {
    fn x(&self, i: usize) -> f32 {
        self.left + self.width * i as f32
    }

    /// The rectangle over columns `first` to `last`.
    fn span(&self, first: usize, last: usize, top: f32, bottom: f32) -> Rect {
        Rect::from_min_max(pos2(self.x(first), top), pos2(self.x(last.max(first) + 1), bottom))
    }

    /// The column at `x`; the nearest one when `x` is outside them all.
    fn index_at(&self, x: f32) -> usize {
        if !x.is_finite() || self.width <= 0.0 || self.width.is_nan() {
            return 0;
        }
        (((x - self.left) / self.width).floor().max(0.0) as usize).min(self.n.saturating_sub(1))
    }

    /// The right edge of the last column.
    fn right(&self) -> f32 {
        self.x(self.n)
    }
}

/// The blocks of calendars side by side, each with the same columns of days.
#[derive(Clone, Copy, Debug)]
struct Blocks {
    /// The columns of the first block.
    first: Cols,
    /// How far after a block's columns the next block's start.
    pitch: f32,
    count: usize,
}

impl Blocks {
    /// `count` blocks of `days` columns each in the `width` from `left` on, with a gap between
    /// one block and the next.
    fn of(left: f32, width: f32, days: usize, count: usize) -> Blocks {
        let (count, days) = (count.clamp(1, MAX_PANES), days.clamp(1, 64));
        let gap = if count > 1 { PANE_GAP } else { 0.0 };
        let width = if width.is_finite() { width } else { 0.0 };
        let column = ((width - gap * (count - 1) as f32) / (count * days) as f32).max(1.0);
        Blocks { first: Cols { left, width: column, n: days }, pitch: column * days as f32 + gap, count }
    }

    /// The columns of block number `block` (of the last one, for a number beyond it).
    fn cols(&self, block: usize) -> Cols {
        Cols { left: self.first.left + self.pitch * block.min(self.count.saturating_sub(1)) as f32, ..self.first }
    }

    /// The block at `x`; the nearest one when `x` is outside them all. (The gap after a block
    /// counts as that block's.)
    fn block_at(&self, x: f32) -> usize {
        if !x.is_finite() || self.pitch.is_nan() || self.pitch <= 0.0 {
            return 0;
        }
        (((x - self.first.left) / self.pitch).floor().max(0.0) as usize).min(self.count.saturating_sub(1))
    }

    /// The columns of the block at `x`.
    fn at(&self, x: f32) -> Cols {
        self.cols(self.block_at(x))
    }

    fn left(&self) -> f32 {
        self.first.left
    }
}

// ---- side pane ----

fn side_pane(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let width = if app.ui.folder_pane_width.is_finite() { app.ui.folder_pane_width.clamp(200.0, 360.0) } else { 230.0 };
    let panel = egui::Panel::left("calendar_side")
        .default_size(width)
        .size_range(200.0..=360.0)
        .resizable(true)
        .frame(egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)));
    panel.show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // As many months as the pane has room for over the list of calendars; one at the least.
        let groups = calendar_groups(app);
        let list_h: f32 = groups.iter().map(|group| 30.0 + if group.folded { 0.0 } else { group.rows.len() as f32 * 26.0 }).sum();
        // (The line under a calendar that is on this computer only takes of the room that is
        // left over under the list; where there is none, the pane scrolls.)
        // The account whose sign-in lacks only the optional extras, which a quiet line says.
        let extras = extras_account(app);
        let foot = if extras.is_some() { EXTRAS_H } else { 0.0 };
        let room = ui.available_height() - list_h - foot - 28.0;
        let months = if room.is_finite() { ((room / MINI_MONTH_H).floor().max(1.0) as usize).min(MAX_MINI_MONTHS) } else { 1 };
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            date_navigator(app, ui, months);
            ui.add_space(12.0);
            calendar_list(app, ui, groups);
            if let Some(account) = extras {
                extras_line(app, ui, account);
            }
            ui.add_space(8.0);
        });
    });
}

/// Days being dragged over in the date navigator: from the one the drag began on to the one
/// under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct NavDrag {
    anchor: Date,
    to: Date,
}

/// The days a drag from `anchor` to `to` shows: the first of them and how many, from the
/// earlier day on and no more than the Day view takes.
pub(crate) fn dragged_days(anchor: Date, to: Date) -> (Date, i64) {
    let (first, last) = (anchor.min(to), anchor.max(to));
    // (When there are more, the ones nearest to where the drag began.)
    let count = (first.days_until(last) + 1).clamp(1, i64::from(MAX_DAYS));
    (if to < anchor { anchor.add_days(1 - count) } else { first }, count)
}

/// The week of the year a row of days that begins on `first` is: that of its Monday, as ISO
/// 8601 counts weeks.
pub(crate) fn week_number(first: Date) -> u8 {
    (0..7).map(|i| first.add_days(i)).find(|day| day.weekday() == Weekday::Mon).unwrap_or(first).iso_week()
}

/// The date navigator: `months` small months one under the other, from the one the calendar
/// shows (the arrows over them go a month back and on). Days on which something happens are
/// bold; the days the calendar shows have a band behind them. A click on a day goes there, a
/// click left of a week shows that week, and a drag over days shows exactly those days.
fn date_navigator(app: &mut MailApp, ui: &mut Ui, months: usize) {
    let t = Tokens::get(ui.ctx());
    let months = months.clamp(1, MAX_MINI_MONTHS);
    let view_date = app.session.view.date;
    let today = app.session.today();
    let (range_from, range_to) = calendar_range(&app.session);
    let week_start = app.session.prefs.week_start;
    let numbers = app.session.prefs.week_numbers;
    let span = i32::try_from(months).unwrap_or(1) - 1;
    // The months shown follow the calendar when it goes to a day they don't show, and until
    // then stay where the arrows put them.
    let memory = egui::Id::unique(MINI_MONTH_ID);
    let kept: Option<(Date, Date)> = ui.ctx().data(|d| d.get_temp(memory));
    let shows = |first: Date, day: Date| (first..=first.add_months(span).end_of_month()).contains(&day);
    let mut first_month = match kept {
        Some((followed, month)) if followed == view_date || shows(month, view_date) => month.start_of_month(),
        _ => view_date.start_of_month(),
    };
    let width = ui.available_width();
    let pad = 10.0;
    let x0 = ui.cursor().min.x + pad + MINI_MARGIN;
    let cell_w = ((width - 2.0 * pad - MINI_MARGIN) / 7.0).max(1.0);

    // The days on which something happens, from the first day shown to the last.
    let first_day = first_month.start_of_week(week_start);
    let last_day_shown = first_month.add_months(span).start_of_week(week_start).add_days(41);
    let reach = first_day.days_until(last_day_shown).clamp(0, 400);
    let mut busy = vec![false; to_index(reach) + 1];
    for o in occurrences(&app.session, first_day, last_day_shown).into_iter().take(MAX_ITEMS) {
        let lo = first_day.days_until(o.start.date).max(0);
        let hi = first_day.days_until(last_day(o.start, o.end)).min(reach);
        for day in lo..=hi {
            if let Some(slot) = busy.get_mut(to_index(day)) {
                *slot = true;
            }
        }
    }
    let is_busy = |day: Date| busy.get(to_index(first_day.days_until(day))).copied().unwrap_or(false) && day >= first_day;

    let drag_id = egui::Id::unique(NAV_DRAG_ID);
    let mut drag: Option<NavDrag> = ui.ctx().data(|d| d.get_temp(drag_id));
    let (mut held, mut let_go) = (false, false);
    // Where each day was drawn, for the day under the pointer while days are dragged over.
    let mut cells: Vec<(Rect, Date)> = Vec::with_capacity(months * 42);
    let mut step = 0;
    for m in 0..months {
        let month = first_month.add_months(i32::try_from(m).unwrap_or(0));
        // Its name; over the first of them, the arrows.
        let (r, _) = ui.allocate_exact_size(vec2(width, MINI_TITLE_H), Sense::hover());
        if m == 0 {
            for (i, (icon, tip, by)) in [("next", "Next Month", 1), ("previous", "Previous Month", -1)].into_iter().enumerate() {
                let br = Rect::from_center_size(pos2(r.max.x - pad - 12.0 - i as f32 * 26.0, r.center().y), vec2(24.0, 24.0));
                let resp = ui.interact(br, ui.scope_id().with(("mini_step", by)), Sense::click());
                app.hit.add("miniStep", if by > 0 { 1 } else { 0 }, br);
                if resp.hovered() {
                    ui.painter().rect_filled(br, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
                }
                icons::paint(ui.painter(), Rect::from_center_size(br.center(), vec2(12.0, 12.0)), icon, t.icon, t.accent);
                if resp.on_hover_text(tl!(tip)).clicked() {
                    step = by;
                }
            }
        }
        let title = crate::dates::month_year(month);
        let title_w = if m == 0 { width - 2.0 * pad - 64.0 } else { width - 2.0 * pad };
        paint_clipped(ui.painter(), pos2(x0 + 5.0, r.center().y), title_w.max(0.0), &title, semibold(13.0), t.text);
        if let Ok(id) = u64::try_from(month.to_days()) {
            app.hit.add("miniMonth", id, r);
        }

        let (r, _) = ui.allocate_exact_size(vec2(width, MINI_HEAD_H), Sense::hover());
        for i in 0..7u8 {
            let weekday = Weekday::from_index(i64::from(week_start.index()) + i64::from(i));
            let initial: String = tl!(weekday.short()).chars().take(1).collect();
            ui.painter().text(pos2(x0 + (f32::from(i) + 0.5) * cell_w, r.center().y), Align2::CENTER_CENTER, initial, regular(10.5), t.text_dim);
        }

        // Six weeks from the start of the week the month begins in. Only the first month shows
        // the days before it, and only the last the days after: no day is there twice.
        let first = month.start_of_week(week_start);
        let shown = |day: Date| {
            let own = day.month == month.month && day.year == month.year;
            own || (m == 0 && day < month) || (m + 1 == months && day > month)
        };
        let (grid, _) = ui.allocate_exact_size(vec2(width, 6.0 * MINI_ROW_H), Sense::hover());
        let p = ui.painter().clone();
        for row in 0..6i64 {
            let top = grid.min.y + row as f32 * MINI_ROW_H;
            let day_of = |col: i64| first.add_days(row * 7 + col);
            let Some(week_first) = (0..7).map(day_of).find(|day| shown(*day)) else { continue };
            // A band behind the days the calendar shows.
            let in_view = |col: &i64| shown(day_of(*col)) && (range_from..=range_to).contains(&day_of(*col));
            if let (Some(lo), Some(hi)) = ((0..7).find(in_view), (0..7).rev().find(in_view)) {
                let band = Rect::from_min_max(pos2(x0 + lo as f32 * cell_w, top + 1.0), pos2(x0 + (hi + 1) as f32 * cell_w, top + MINI_ROW_H - 1.0));
                p.rect_filled(band.shrink2(vec2(1.0, 0.0)), 10.0, t.selection);
            }
            // Left of the week: a click shows that week; its number in the year, when wanted.
            let margin = Rect::from_min_max(pos2(x0 - MINI_MARGIN - 4.0, top), pos2(x0 - 2.0, top + MINI_ROW_H));
            let resp = ui.interact(margin, ui.scope_id().with(("mini_week", m, row)), Sense::click());
            if let Ok(id) = u64::try_from(week_first.to_days()) {
                app.hit.add("miniWeek", id, margin);
            }
            if resp.hovered() {
                p.rect_filled(margin.shrink2(vec2(2.0, 2.0)), 4.0, t.hover);
            }
            if numbers {
                let number = week_number(day_of(0)).to_string();
                p.text(pos2(margin.max.x - 5.0, margin.center().y), Align2::RIGHT_CENTER, number, regular(9.5), t.text_dim);
            }
            if resp.on_hover_text(tl!("Show this week")).clicked() {
                let _ = app.run("view.goToDate", json!({"date": week_first.iso()}));
                let _ = app.run("view.week", json!({}));
            }
            for col in 0..7i64 {
                let day = day_of(col);
                if !shown(day) {
                    continue;
                }
                let cell = Rect::from_min_size(pos2(x0 + col as f32 * cell_w, top), vec2(cell_w, MINI_ROW_H));
                let center = cell.center();
                let resp = ui.interact(cell, ui.scope_id().with(("mini_day", m, row * 7 + col)), Sense::click_and_drag());
                cells.push((cell, day));
                if let Ok(id) = u64::try_from(day.to_days()) {
                    app.hit.add("miniDay", id, cell);
                }
                let is_today = day == today;
                if is_today {
                    p.circle_filled(center, (MINI_ROW_H / 2.0 - 1.0).min(cell_w / 2.0), t.accent);
                } else if resp.hovered() && drag.is_none() {
                    p.circle_filled(center, (MINI_ROW_H / 2.0 - 1.0).min(cell_w / 2.0), t.hover);
                }
                let outside = day.month != month.month || day.year != month.year;
                let ink = if is_today {
                    t.on_accent
                } else if outside {
                    t.text_disabled
                } else {
                    t.text
                };
                // A day on which something happens is bold.
                let bold = is_busy(day);
                if let (true, Ok(id)) = (bold, u64::try_from(day.to_days())) {
                    app.hit.add("miniBusy", id, cell);
                }
                let font = if is_today || bold { semibold(11.0) } else { regular(11.0) };
                p.text(center, Align2::CENTER_CENTER, day.day.to_string(), font, ink);
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    drag = Some(NavDrag { anchor: day, to: day });
                }
                if (resp.dragged_by(egui::PointerButton::Primary) || resp.drag_stopped()) && drag.is_some_and(|d| d.anchor == day) {
                    held = true;
                    let_go |= resp.drag_stopped();
                } else if resp.clicked() {
                    let _ = app.run("view.goToDate", json!({"date": day.iso()}));
                }
            }
        }
        ui.add_space(6.0);
    }
    if step != 0 {
        first_month = first_month.add_months(step);
    }
    ui.ctx().data_mut(|d| d.insert_temp(memory, (view_date, first_month)));

    // Days being dragged over: they are marked as the pointer goes, and shown when it lets go.
    // Escape gives the drag up.
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    match drag.filter(|_| held && !escape) {
        Some(mut over) => {
            let pointer = ui.input(|i| i.pointer.interact_pos());
            if let Some(day) = pointer.and_then(|pos| cells.iter().find(|(cell, _)| cell.contains(pos)).map(|(_, day)| *day)) {
                over.to = day;
            }
            let (start, count) = dragged_days(over.anchor, over.to);
            if let_go {
                ui.ctx().data_mut(|d| d.remove::<NavDrag>(drag_id));
                let _ = if count > 1 {
                    app.run("view.days", json!({"count": count, "start": start.iso()}))
                } else {
                    app.run("view.goToDate", json!({"date": start.iso()}))
                };
            } else {
                ui.ctx().data_mut(|d| d.insert_temp(drag_id, over));
                let end = start.add_days(count - 1);
                let mark = t.accent.linear_multiply(0.28);
                for (cell, _) in cells.iter().filter(|(_, day)| (start..=end).contains(day)) {
                    ui.painter().rect_filled(cell.shrink2(vec2(0.0, 2.0)), 0.0, mark);
                }
            }
        }
        None if drag.is_some() => ui.ctx().data_mut(|d| d.remove::<NavDrag>(drag_id)),
        None => {}
    }
}

/// What a calendar is called where the account it is listed under isn't beside it (the form, a
/// menu): a calendar that an account's server keeps says whose it is.
pub(crate) fn calendar_label(session: &mailcraft_engine::Session, calendar: &mailcraft_model::Calendar) -> String {
    let name = if calendar.name.trim().is_empty() { tl!("Calendar") } else { tl!(clip_text(calendar.name.trim(), 200)) };
    match mailcraft_engine::cmd::remote::keeper(&session.mailbox, &calendar.origin) {
        Some(account) => {
            let whose = if account.name.trim().is_empty() { account.email.trim() } else { account.name.trim() };
            format!("{name} ({})", clip_text(whose, 100))
        }
        None => name.to_string(),
    }
}

/// Height of the quiet line under the list of calendars.
const EXTRAS_H: f32 = 44.0;

/// One calendar of the list.
#[derive(Clone, Debug, PartialEq)]
struct CalendarRow {
    id: Id,
    name: String,
    /// The colour it is shown in, and the one the owner chose for it here, when one was.
    color: Color32,
    own_color: Option<mailcraft_model::Rgb>,
    visible: bool,
    /// The account may read it and not change it.
    read_only: bool,
    /// It lies over other calendars (or under them).
    overlaid: bool,
}

/// A heading of the list of calendars with the calendars under it.
#[derive(Clone, Debug, PartialEq)]
struct ListGroup {
    /// What the heading says, which is also what its folding is remembered by.
    title: String,
    /// The calendars under it are folded away.
    folded: bool,
    rows: Vec<CalendarRow>,
}

/// What a group of calendars is remembered by when it is folded away: its heading, on one line.
fn group_key(title: &str) -> String {
    title.trim().chars().filter(|c| !c.is_control()).take(MAX_HEADING).collect()
}

/// The calendars as the list has them ([`calendar_settings::calendar_groups`]): the account's
/// own first, its main calendar at the top, then its other groups by their names, the calendars
/// of other people and places, and last what is kept on this computer.
fn calendar_groups(app: &MailApp) -> Vec<ListGroup> {
    let mailbox = &app.session.mailbox;
    let lying = &app.session.view.overlay;
    let stacked = calendar_settings::overlaid(&app.session);
    let row = |id: &Id| {
        let c = mailbox.calendar(*id)?;
        Some(CalendarRow {
            id: c.id,
            name: c.name.clone(),
            color: c32(c.shown_color()),
            own_color: c.own_color,
            visible: c.visible,
            read_only: read_only(app, c.id),
            overlaid: stacked && c.visible && lying.contains(&c.id),
        })
    };
    let folded = |title: &str| app.ui.collapsed_calendar_groups.iter().any(|key| *key == group_key(title));
    let groups = calendar_settings::calendar_groups(&app.session);
    groups
        .iter()
        .take(100)
        .map(|group| ListGroup { title: group.title.clone(), folded: folded(&group.title), rows: group.calendars.iter().filter_map(row).collect() })
        .collect()
}

/// The window command that is the list's own (`UI_COMMANDS` has it): fold the calendars of a
/// group away under its heading, or bring them back.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if id != "ui.toggleCalendarGroup" {
        return None;
    }
    let group = p.get("group").and_then(Value::as_str).map(group_key).unwrap_or_default();
    if group.is_empty() {
        return Some(Err(tl!("Say which group of calendars (`group`: its heading)").to_string()));
    }
    let folded = &mut app.ui.collapsed_calendar_groups;
    let fold = p.get("value").and_then(Value::as_bool).unwrap_or(!folded.contains(&group));
    folded.retain(|key| *key != group);
    if fold {
        // (Headings of groups long gone make room for this one.)
        let extra = (folded.len() + 1).saturating_sub(MAX_FOLDED);
        folded.drain(..extra.min(folded.len()));
        folded.push(group.clone());
    }
    Some(Ok(json!({"group": group, "collapsed": fold})))
}

/// The list of calendars, group by group: a heading folds its calendars away, a coloured check
/// box shows or hides a calendar, and a click on its name makes it the one being worked in,
/// which is where new items go.
fn calendar_list(app: &mut MailApp, ui: &mut Ui, groups: Vec<ListGroup>) {
    let t = Tokens::get(ui.ctx());
    let current = mailcraft_engine::cmd::calendar::current_calendar(&app.session);
    let several = groups.iter().flat_map(|group| &group.rows).filter(|row| row.visible).count() > 1;
    let only_here = local_only(app);
    for (n, group) in groups.iter().enumerate() {
        group_heading(app, ui, &t, n, group);
        if !group.folded {
            for row in &group.rows {
                calendar_row(app, ui, &t, row, row.id == current, several);
                if only_here.contains(&row.id) {
                    local_line(app, ui, &t, row.id);
                }
            }
        }
        ui.add_space(6.0);
    }
}

/// The line under a calendar of the list that is kept on this computer only, beside a calendar
/// that the account's server keeps: what is in it is not in the owner's Outlook calendar.
fn local_line(app: &mut MailApp, ui: &mut Ui, t: &Tokens, calendar: Id) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), LOCAL_LINE_H), Sense::hover());
    let inner = Rect::from_min_max(pos2(r.min.x + 40.0, r.min.y), pos2(r.max.x - 8.0, r.max.y));
    let font = egui::text::TextFormat { font_id: regular(11.0), color: t.orange, ..Default::default() };
    let mut job = egui::text::LayoutJob::default();
    job.append(tl!(LOCAL_ONLY), 0.0, font);
    job.wrap = egui::text::TextWrapping { max_width: inner.width().max(20.0), max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(inner.min, galley, t.orange);
    app.hit.add("calendarLocalLine", calendar, r);
    let tip =
        tl!("Items in this calendar are kept on this computer and nowhere else. Above the calendar you can move them to your account's calendar.");
    ui.interact(r, ui.scope_id().with(("calendar_local_line", calendar)), Sense::hover()).on_hover_text(tip);
}

/// The engine's names of the groups of calendars in the interface language; any other is the
/// server's and stays as it is.
fn group_title(title: &str) -> &str {
    match title {
        "My Calendars" => tl!("My Calendars"),
        "Other Calendars" => tl!("Other Calendars"),
        "Shared Calendars" => tl!("Shared Calendars"),
        "On this computer" => tl!("On this computer"),
        other => other,
    }
}

/// The heading of a group of calendars: a click folds the group away, or brings it back.
fn group_heading(app: &mut MailApp, ui: &mut Ui, t: &Tokens, n: usize, group: &ListGroup) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with(("calendar_group", n)), Sense::click());
    app.hit.add("calendarGroup", n as u64, r);
    if resp.hovered() {
        ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), 4.0, t.hover);
    }
    let icon = if group.folded { "collapsed" } else { "expanded" };
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 13.0, r.center().y), vec2(12.0, 12.0)), icon, t.icon, t.accent);
    paint_clipped(ui.painter(), pos2(r.min.x + 24.0, r.center().y), r.width() - 30.0, group_title(&group.title), regular(14.667), t.text);
    let tip = if group.folded { tl!("Show the calendars of this group") } else { tl!("Fold the calendars of this group away") };
    if resp.on_hover_text(tip).clicked() {
        let _ = app.run("ui.toggleCalendarGroup", json!({"group": group.title}));
    }
}

/// What the right button offers for a calendar of the list: to show it alone, to lay it over
/// the others or stand it beside them, and a colour of the owner's choosing.
fn calendar_menu(ui: &mut Ui, app: &mut MailApp, row: &CalendarRow, several: bool) {
    use crate::widgets::{mi, mi_check};
    ui.set_min_width(210.0);
    mi(ui, app, "Show Only This Calendar", "calendar.showOnly", json!({"id": row.id}));
    if row.visible && several {
        mi_check(ui, app, "Overlay", row.overlaid, "view.overlay", json!({"calendar": row.id}));
    }
    ui.separator();
    ui.menu_button(tl!("Colour"), |ui| {
        // Whatever the calendar's own colour is (its server's, for a calendar a server keeps).
        mi_check(ui, app, "Automatic", row.own_color.is_none(), "calendar.color", json!({"id": row.id, "color": null}));
        ui.separator();
        for (name, rgb) in calendar_settings::CALENDAR_COLORS {
            let label = format!("       {}", tl!(name));
            let resp = ui.add(egui::Button::new(label).selected(row.own_color == Some(rgb)).min_size(vec2(170.0, 0.0)));
            let swatch = Rect::from_center_size(pos2(resp.rect.min.x + 15.0, resp.rect.center().y), vec2(12.0, 12.0));
            ui.painter().rect_filled(swatch, 3.0, c32(rgb));
            if resp.clicked() {
                let _ = app.run("calendar.color", json!({"id": row.id, "color": rgb.hex()}));
                ui.close();
            }
        }
    });
    // One opened by address is the owner's own list's: it comes off it here (#70).
    let opened = app.session.mailbox.calendar(row.id).is_some_and(|c| mailcraft_model::remote::opened_address(&c.origin.id).is_some());
    if opened {
        ui.separator();
        mi(ui, app, "Remove Shared Calendar", "calendar.removeShared", json!({"id": row.id}));
    }
    // A shared mailbox's calendar that the server lists under Shared Calendars too (#70): the
    // copy is hidden, or shown again, here.
    match listed_twice(app, row.id) {
        Some((address, true)) => {
            ui.separator();
            mi(ui, app, "Show the Copy under Shared Calendars", "calendar.hideListedCopy", json!({"address": address, "hide": false}));
        }
        Some((address, false)) => {
            ui.separator();
            mi(ui, app, "Hide This Copy", "calendar.hideListedCopy", json!({"address": address, "hide": true}));
        }
        None => {}
    }
}

/// For a calendar of the list that is a shared mailbox's (#70): the mailbox's address and `true`
/// when the copy of its calendar under Shared Calendars is hidden. For that copy itself: the
/// mailbox's address and `false`. `None` for any other calendar.
pub(crate) fn listed_twice(app: &MailApp, calendar: Id) -> Option<(String, bool)> {
    let mailbox = &app.session.mailbox;
    let calendar = mailbox.calendar(calendar)?;
    let account = mailbox.account(calendar.origin.account)?;
    if account.is_shared() {
        let owner = mailbox.owner_of(account.id)?;
        let address = account.email.trim().to_string();
        return (owner.listed_twice_answer(&address) == Some(true)).then_some((address, true));
    }
    if calendar.origin.id.is_empty() || mailcraft_model::remote::opened_address(&calendar.origin.id).is_some() {
        return None;
    }
    let by = calendar.owner.as_ref()?.address.trim().to_string();
    let shared = mailbox.shared_mailboxes(account.id).into_iter().find(|s| s.email.trim().eq_ignore_ascii_case(&by))?;
    Some((shared.email.trim().to_string(), false))
}

/// The account whose sign-in covers everything but the optional extras (the mailbox's own
/// categories and working hours), when there is one: its calendar works, and one more sign-in
/// would add them.
fn extras_account(app: &MailApp) -> Option<Id> {
    let mut accounts = app.session.mailbox.accounts.iter().take(50).map(|account| account.id);
    accounts.find(|id| app.graph_extras_missing(*id) && !app.graph_consent_needed(*id))
}

/// The quiet line at the foot of the list of calendars while the sign-in of `account` lacks
/// only the optional extras: what would come of connecting once more, and the way to do it.
/// (A calendar that can't be read at all says so across the top instead: [`connect_notice`].)
fn extras_line(app: &mut MailApp, ui: &mut Ui, account: Id) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), EXTRAS_H), Sense::hover());
    let inner = r.shrink2(vec2(12.0, 5.0));
    let resp = ui.interact(inner, ui.scope_id().with("calendar_extras"), Sense::click());
    app.hit.add("calendarExtras", account, inner);
    // Its first word is the link.
    let text = tl!("Connect to show your category colours and working hours");
    let (link, rest) = text.split_once(' ').unwrap_or((text, ""));
    let part = |color: Color32, underline: bool| {
        let underline = if underline { Stroke::new(1.0, color) } else { Stroke::NONE };
        egui::text::TextFormat { font_id: regular(11.5), color, underline, ..Default::default() }
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(link, 0.0, part(t.accent_text, resp.hovered()));
    job.append(&format!(" {rest}"), 0.0, part(t.text_dim, false));
    job.wrap = egui::text::TextWrapping { max_width: inner.width().max(20.0), max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(inner.min, galley, t.text_dim);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let tip =
        tl!("Sign in once more, so that MailCraft may read the categories and the working hours your mailbox has. Your calendar works as it is.");
    if resp.on_hover_text(tip).clicked()
        && let Err(e) = app.run("ui.connectMicrosoft", json!({"account": account}))
    {
        app.status(e);
    }
}

/// One calendar of the list: the check box shows or hides it, the name makes it the current
/// one (unless it can only be read, which it says), and the right button has its menu.
fn calendar_row(app: &mut MailApp, ui: &mut Ui, t: &Tokens, row: &CalendarRow, current: bool, several: bool) {
    let (id, color, visible) = (row.id, row.color, row.visible);
    let name = row.name.as_str();
    {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        let (check, label) = (Rect::from_min_max(r.min, pos2(r.min.x + 36.0, r.max.y)), Rect::from_min_max(pos2(r.min.x + 36.0, r.min.y), r.max));
        let resp = ui.interact(check, ui.scope_id().with(("calendar", id)), Sense::click());
        let named = ui.interact(label, ui.scope_id().with(("calendar_name", id)), Sense::click());
        app.hit.add("calendar", id, check);
        app.hit.add("calendarName", id, label);
        if current {
            ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), 4.0, t.selection);
        } else if resp.hovered() || named.hovered() {
            ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), 4.0, t.hover);
        }
        let p = ui.painter();
        let b = Rect::from_center_size(pos2(r.min.x + 24.0, r.center().y), vec2(15.0, 15.0));
        if visible {
            p.rect_filled(b, 3.0, color);
            let c = b.center();
            let mark = vec![pos2(c.x - 3.6, c.y + 0.2), pos2(c.x - 1.1, c.y + 2.7), pos2(c.x + 3.8, c.y - 2.8)];
            p.add(Shape::line(mark, Stroke::new(1.6, ink_on(color, t))));
        } else {
            p.rect_stroke(b, 3.0, Stroke::new(1.5, color), StrokeKind::Inside);
        }
        let text = if name.trim().is_empty() { tl!("Calendar") } else { tl!(clip_text(name.trim(), 200)) };
        // One that can only be read has a lock at the end of its row.
        let lock = if row.read_only { 18.0 } else { 0.0 };
        paint_clipped(
            p,
            pos2(r.min.x + 40.0, r.center().y),
            (r.width() - 50.0 - lock).max(0.0),
            text,
            if current { semibold(12.5) } else { regular(12.5) },
            if visible { t.text } else { t.text_dim },
        );
        if row.read_only {
            icons::paint(p, Rect::from_center_size(pos2(r.max.x - 17.0, r.center().y), vec2(12.0, 12.0)), "private", t.text_dim, t.text_dim);
            app.hit.add("calendarReadOnly", id, r);
        }
        if resp.clicked() {
            let _ = app.run("calendar.showCalendar", json!({"id": id}));
        }
        // Its menu, from the box or from the name.
        for part in [&resp, &named] {
            part.context_menu(|ui| calendar_menu(ui, app, row, several));
        }
        let cant = tl!("This calendar can only be read: new items can't go to it");
        let tip = if row.read_only { cant } else { tl!("New items go to the calendar that is picked here") };
        if named.on_hover_text(tip).clicked() {
            if row.read_only {
                app.status(cant);
            } else {
                let _ = app.run("calendar.setCurrent", json!({"id": id}));
                // Of the calendars that lie over one another, it comes to the front.
                if row.overlaid {
                    let _ = app.execute("view.frontCalendar", json!({"calendar": id}));
                }
            }
        }
    }
}

/// The row of File › Options under the working hours: whether those are used even when the
/// account's mailbox has its own, and what the mailbox has.
pub(crate) fn work_hours_option(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let mut own = app.session.prefs.own_work_hours;
    let tip = tl!(
        "Without this, the calendar uses the working hours your Microsoft 365 mailbox has, once MailCraft may read them. The ones above are used until then."
    );
    if ui.checkbox(&mut own, tl!("Use my own working hours")).on_hover_text(tip).changed() {
        let _ = app.run("view.workHours", json!({"own": own}));
    }
    let Some(theirs) = app.session.prefs.mailbox_hours.as_ref().and_then(|kept| kept.tidy()) else { return };
    let twelve = app.session.prefs.twelve_hour;
    let clock = |minutes: u16| crate::dates::time(u8::try_from(minutes / 60 % 24).unwrap_or(0), u8::try_from(minutes % 60).unwrap_or(0), twelve);
    // (The days in the order of the week as it is counted here.)
    let first = i64::from(app.session.prefs.week_start.index());
    let mut in_order = theirs.days.clone();
    in_order.sort_by_key(|day| (i64::from(day.index()) - first).rem_euclid(7));
    let days: Vec<&str> = in_order.iter().take(7).map(|day| tl!(day.short())).collect();
    let mut said = crate::i18n::fmt(
        tl!("Your mailbox has: {days} · {from} – {to}"),
        &[("days", &days.join(", ")), ("from", &clock(theirs.start)), ("to", &clock(theirs.end))],
    );
    if !theirs.zone.is_empty() {
        said.push_str(&format!(" ({})", clip_text(&theirs.zone, 64)));
    }
    ui.label(egui::RichText::new(said).font(regular(11.5)).color(t.text_dim));
}

// ---- header strip ----

/// A line under the header while an account's calendar waits for the owner's say-so (a sign-in
/// that doesn't cover the calendar, or not changing it): until then nothing comes from the server
/// and nothing goes to it, and that must not look like a calendar that is merely quiet.
fn connect_notice(app: &mut MailApp, ui: &mut Ui) {
    let Some(account) = app.session.mailbox.accounts.iter().take(50).map(|a| a.id).find(|id| app.graph_consent_needed(*id)) else { return };
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(r, 0.0, t.today);
    p.hline(r.x_range(), r.max.y - 0.5, Stroke::new(1.0, t.border));
    let action = tl!("Connect");
    let wide = text_w(ui, action, medium(12.0)) + 26.0;
    let button = Rect::from_min_size(pos2(r.max.x - 12.0 - wide, r.center().y - 11.0), vec2(wide, 22.0));
    let resp = ui.interact(button, ui.scope_id().with("calendar_connect"), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.surface
    };
    p.rect(button, 4.0, fill, Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
    p.text(button.center(), Align2::CENTER_CENTER, action, medium(12.0), t.text);
    let text = tl!(
        "This calendar isn't connected to your account yet. What changes elsewhere doesn't show here, and what you change here stays on this computer."
    );
    paint_clipped(&p, pos2(r.min.x + 12.0, r.center().y), button.min.x - r.min.x - 24.0, text, regular(12.0), t.text);
    app.hit.add("calendarConnect", account, button);
    if resp.on_hover_text(tl!("Sign in once more, so that MailCraft may show and change your calendar")).clicked()
        && let Err(e) = app.run("ui.connectMicrosoft", json!({"account": account}))
    {
        app.status(e);
    }
}

/// What a calendar that is kept on this computer says of itself, beside a calendar that the
/// account's server keeps: on its block, and in the list of calendars.
pub(crate) const LOCAL_ONLY: &str = "Only on this computer: not in your Outlook calendar";
/// Height of that line under a calendar's row in the list.
const LOCAL_LINE_H: f32 = 30.0;

/// The calendars that are kept on this computer only although the account has a calendar on its
/// server, and hold something ([`mailcraft_engine::cmd::remote::only_here`]): what is in them
/// is in nobody's Outlook calendar.
fn local_only(app: &MailApp) -> Vec<Id> {
    mailcraft_engine::cmd::remote::only_here(&app.session.mailbox).into_iter().map(|(calendar, _)| calendar).take(50).collect()
}

/// A line under the header while items that are shown are kept on this computer only, although
/// the account has a calendar on its server: drawn beside that calendar they look the part, and
/// a change to one of them goes nowhere. The line says so, and offers to move them where they
/// are thought to be.
fn local_notice(app: &mut MailApp, ui: &mut Ui) {
    let mailbox = &app.session.mailbox;
    let shown = |calendar: &Id| mailbox.calendar(*calendar).is_some_and(|c| c.visible);
    let only_here = mailcraft_engine::cmd::remote::only_here(mailbox);
    let items: usize = only_here.iter().filter(|(calendar, _)| shown(calendar)).map(|(_, items)| *items).sum();
    if items == 0 {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(r, 0.0, t.today);
    p.hline(r.x_range(), r.max.y - 0.5, Stroke::new(1.0, t.border));
    // A mark in the colour of a warning, so the line isn't taken for a heading.
    p.rect_filled(Rect::from_min_size(r.min, vec2(4.0, r.height())), 0.0, t.orange);
    let action = tl!("Move to My Account's Calendar");
    let wide = text_w(ui, action, medium(12.0)) + 26.0;
    let button = Rect::from_min_size(pos2(r.max.x - 12.0 - wide, r.center().y - 11.0), vec2(wide, 22.0));
    let resp = ui.interact(button, ui.scope_id().with("calendar_local_only"), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.surface
    };
    p.rect(button, 4.0, fill, Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
    p.text(button.center(), Align2::CENTER_CENTER, action, medium(12.0), t.text);
    let text = tln!(
        items,
        "{n} item here is only on this computer: not in your Outlook calendar. A change to it stays here.",
        "{n} items here are only on this computer: not in your Outlook calendar. A change to one of them stays here."
    );
    paint_clipped(&p, pos2(r.min.x + 14.0, r.center().y), button.min.x - r.min.x - 26.0, &text, regular(12.0), t.text);
    app.hit.add("calendarLocalOnly", items as u64, button);
    let tip = tl!("Put them into the calendar of your account, where Outlook and your other devices have them too. You are asked first.");
    if resp.on_hover_text(tip).clicked()
        && let Err(e) = app.run("calendar.moveToAccount", json!({}))
    {
        app.status(e);
    }
}

fn header(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let p = ui.painter().clone();
    let (top, height) = (r.center().y - 14.0, 28.0);

    let label = tl!("Today");
    let tr = Rect::from_min_size(pos2(r.min.x + 12.0, top), vec2(text_w(ui, label, medium(12.0)) + 26.0, height));
    let resp = ui.interact(tr, ui.scope_id().with("calendar_today"), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.surface
    };
    p.rect(tr, 4.0, fill, Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
    p.text(tr.center(), Align2::CENTER_CENTER, label, medium(12.0), t.text);
    if resp.on_hover_text(crate::dates::long(app.session.today())).clicked() {
        let _ = app.run("view.today", json!({}));
    }

    let mut x = tr.max.x + 8.0;
    for (icon, tip, command) in [("previous", "Back", "view.previous"), ("next", "Forward", "view.next")] {
        let br = Rect::from_min_size(pos2(x, top), vec2(28.0, height));
        let resp = ui.interact(br, ui.scope_id().with(command), Sense::click());
        if resp.hovered() {
            p.rect_filled(br, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
        }
        icons::paint(&p, Rect::from_center_size(br.center(), vec2(16.0, 16.0)), icon, t.icon, t.accent);
        if resp.on_hover_text(tl!(tip)).clicked() {
            let _ = app.run(command, json!({}));
        }
        x = br.max.x + 2.0;
    }
    x += 10.0;

    // Last, so it names what the buttons just changed to. (The views themselves are on the
    // ribbon, where the one in use is marked: Home > Arrange.)
    let (from, to) = calendar_range(&app.session);
    let title = range_title(app.session.view.calendar, app.session.view.date, from, to);
    paint_clipped(&p, pos2(x, r.center().y), (r.max.x - x - 16.0).max(0.0), &title, semibold(17.0), t.text);
    p.hline(r.x_range(), r.max.y - 0.5, Stroke::new(1.0, t.border));
}

// ---- how an event is drawn ----

#[derive(Clone, Copy, PartialEq)]
enum Edge {
    Solid,
    /// Slanted lines.
    Hatched,
    Dotted,
    Hollow,
}

/// The colours of one event: a soft fill of its calendar's colour, a stronger edge, and the inks.
struct Look {
    fill: Color32,
    edge: Color32,
    kind: Edge,
    outline: bool,
    ink: Color32,
    dim: Color32,
}

/// An event is a block tinted with its colour `base` (its category's, or its calendar's) with a
/// stronger edge down its left in its calendar's colour, which also says how the time shows:
/// busy is solid, tentative hatched, working elsewhere dotted, free hollow (and the block
/// outlined and nearly empty), out of office purple whatever the calendar's colour. Cancelled
/// events fade; so, when `dim`, do those of a calendar that lies under another.
fn look(ev: &Event, base: Color32, calendar: Color32, t: &Tokens, hovered: bool, dim: bool) -> Look {
    let soft = if t.dark { 0.36 } else { 0.20 };
    let soft = if hovered { soft + 0.08 } else { soft };
    let purple = mix(t.blue, t.red, 0.5);
    let (edge, strength, kind, outline) = match ev.show_as {
        ShowAs::Busy => (calendar, 1.0, Edge::Solid, false),
        ShowAs::WorkingElsewhere => (calendar, 0.8, Edge::Dotted, false),
        ShowAs::Tentative => (calendar, 0.6, Edge::Hatched, false),
        ShowAs::Free => (calendar, 0.2, Edge::Hollow, true),
        ShowAs::OutOfOffice => (purple, 1.0, Edge::Solid, false),
    };
    if dim && !ev.cancelled {
        // Under the calendar in front: there to be seen through, not to be read first.
        return Look {
            fill: mix(t.surface, base, soft * strength * 0.5),
            edge: mix(t.surface, edge, 0.5),
            kind,
            outline,
            ink: t.text_dim,
            dim: mix(t.surface, t.text_dim, 0.75),
        };
    }
    if ev.cancelled {
        return Look {
            fill: mix(t.surface, base, soft * strength * 0.5),
            edge: mix(t.surface, edge, 0.45),
            kind,
            outline,
            ink: t.text_disabled,
            dim: t.text_disabled,
        };
    }
    Look { fill: mix(t.surface, base, soft * strength), edge, kind, outline, ink: t.text, dim: t.text_dim }
}

/// The edge down the left of a block or bar; `rounded` is the radius of its outer corners.
fn paint_edge(p: &Painter, r: Rect, look: &Look, t: &Tokens, rounded: u8) {
    let radius = CornerRadius { nw: rounded, sw: rounded, ne: 0, se: 0 };
    match look.kind {
        Edge::Solid => {
            p.rect_filled(r, radius, look.edge);
        }
        Edge::Hollow => {
            p.rect_filled(r, radius, t.surface);
            p.rect_stroke(r, radius, Stroke::new(1.0, look.edge), StrokeKind::Inside);
        }
        Edge::Dotted => {
            let (dash, gap) = (2.0, 2.0);
            p.rect_filled(r, radius, t.surface);
            // Only the part in view: a block can be a day tall.
            let seen = r.intersect(p.clip_rect());
            if seen.is_positive() {
                // Whole periods from the top of the edge, so the pattern stays put while scrolling.
                let skip = ((seen.min.y - r.min.y) / (dash + gap)).floor().max(0.0) * (dash + gap);
                let path = [pos2(r.center().x, r.min.y + skip), pos2(r.center().x, seen.max.y)];
                p.extend(Shape::dashed_line(&path, Stroke::new(r.width(), look.edge), dash, gap));
            }
        }
        Edge::Hatched => {
            p.rect_filled(r, radius, t.surface);
            let seen = r.intersect(p.clip_rect());
            if seen.is_positive() {
                // Slanted lines, counted from the top of the edge so they stay put while
                // scrolling, and only the ones in view.
                let (pitch, slant) = (4.0, r.width());
                let p = p.with_clip_rect(seen);
                let first = ((seen.min.y - r.min.y) / pitch).floor().max(0.0) as usize;
                let lines = (((seen.height() + slant) / pitch).ceil().max(0.0) as usize).min(4000);
                for k in first..=first.saturating_add(lines) {
                    let y = r.min.y + k as f32 * pitch;
                    p.line_segment([pos2(r.min.x, y), pos2(r.max.x, y - slant)], Stroke::new(1.2, look.edge));
                }
            }
        }
    }
}

/// One line of text cut to `width`, struck through when `strike` (a cancelled event).
fn paint_line(p: &Painter, pos: egui::Pos2, width: f32, text: &str, font: FontId, color: Color32, strike: bool) -> f32 {
    if !strike {
        return paint_clipped(p, pos, width, text, font, color);
    }
    if width <= 4.0 || text.is_empty() {
        return 0.0;
    }
    let format = egui::text::TextFormat { font_id: font, color, strikethrough: Stroke::new(1.0, color), ..Default::default() };
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), format);
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = p.layout_job(job);
    let size = galley.size();
    p.galley(pos2(pos.x, pos.y - size.y / 2.0), galley, color);
    size.x
}

/// The small marks of an event (private, meeting, repeats, its categories), right to left from
/// `right`. Returns where the text beside them has to stop.
fn paint_marks(p: &Painter, item: &Item, look: &Look, right: f32, left: f32, y: f32) -> f32 {
    let mut x = right;
    // How much it matters comes first (a red mark for high, a blue arrow down for low, in their
    // own colours whatever the item's), then that it is held online.
    let importance = match item.ev.importance {
        Importance::High => Some("importanceHigh"),
        Importance::Low => Some("importanceLow"),
        Importance::Normal => None,
    };
    if let Some(icon) = importance.filter(|_| x - 13.0 > left + 28.0) {
        icons::paint(p, Rect::from_center_size(pos2(x - 6.0, y), vec2(12.0, 12.0)), icon, look.dim, look.dim);
        x -= 14.0;
    }
    for (on, icon) in [
        (item.ev.online.is_some(), "onlineMeeting"),
        (item.ev.private, "private"),
        (item.ev.is_meeting(), "meeting"),
        (item.ev.recurrence.is_some(), "recurrence"),
    ] {
        if on && x - 13.0 > left + 28.0 {
            icons::paint(p, Rect::from_center_size(pos2(x - 6.0, y), vec2(12.0, 12.0)), icon, look.dim, look.dim);
            x -= 14.0;
        }
    }
    for color in &item.tags {
        if x - 10.0 > left + 28.0 {
            p.rect_filled(Rect::from_center_size(pos2(x - 5.0, y), vec2(8.0, 8.0)), 2.0, *color);
            x -= 11.0;
        }
    }
    x
}

/// An event block in the time grid: title, then the time and the place when there is room.
fn paint_block(p: &Painter, rect: Rect, item: &Item, t: &Tokens, twelve: bool, selected: bool, hovered: bool) {
    let look = look(&item.ev, item.color, item.edge, t, hovered, item.dim);
    p.rect_filled(rect, 3.0, look.fill);
    if look.outline {
        p.rect_stroke(rect, 3.0, Stroke::new(1.0, mix(t.surface, look.edge, 0.7)), StrokeKind::Inside);
    }
    paint_edge(p, Rect::from_min_max(rect.min, pos2(rect.min.x + EDGE_W.min(rect.width()), rect.max.y)), &look, t, 3);
    if selected {
        p.rect_stroke(rect, 3.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
    }
    if rect.width() < 22.0 {
        return;
    }
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let (left, right) = (rect.min.x + EDGE_W + 5.0, rect.max.x - 4.0);
    let strike = item.ev.cancelled;
    let place = clip_text(item.ev.location.trim(), 200);
    if rect.height() < 32.0 {
        // One line: the title, and the place after it when it fits.
        let y = rect.center().y;
        let stop = paint_marks(&p, item, &look, right, left, y);
        let used = paint_line(&p, pos2(left, y), stop - left, item.title(), semibold(11.5), look.ink, strike);
        if !place.is_empty() && stop - left - used > 48.0 {
            paint_clipped(&p, pos2(left + used + 8.0, y), stop - left - used - 8.0, place, regular(10.5), look.dim);
        }
        return;
    }
    let y = rect.min.y + 11.0;
    let stop = paint_marks(&p, item, &look, right, left, y);
    paint_line(&p, pos2(left, y), stop - left, item.title(), semibold(11.5), look.ink, strike);
    paint_clipped(&p, pos2(left, y + 15.0), right - left, &item.times(twelve), regular(10.5), look.dim);
    if !place.is_empty() && rect.height() >= 46.0 {
        paint_clipped(&p, pos2(left, y + 29.0), right - left, place, regular(10.5), look.dim);
    }
}

/// A bar for a whole-day or multi-day event. `starts` and `ends` say whether this piece is where
/// the event begins and finishes: a piece cut by the edge of the view is flat on that side.
fn paint_bar(p: &Painter, rect: Rect, item: &Item, t: &Tokens, twelve: bool, starts: bool, ends: bool, selected: bool, hovered: bool) {
    let look = look(&item.ev, item.color, item.edge, t, hovered, item.dim);
    let (west, east) = (if starts { 3 } else { 0 }, if ends { 3 } else { 0 });
    let radius = CornerRadius { nw: west, sw: west, ne: east, se: east };
    p.rect_filled(rect, radius, look.fill);
    if look.outline {
        p.rect_stroke(rect, radius, Stroke::new(1.0, mix(t.surface, look.edge, 0.7)), StrokeKind::Inside);
    }
    if starts {
        paint_edge(p, Rect::from_min_max(rect.min, pos2(rect.min.x + EDGE_W.min(rect.width()), rect.max.y)), &look, t, 3);
    }
    if selected {
        p.rect_stroke(rect, radius, Stroke::new(2.0, t.accent), StrokeKind::Inside);
    }
    if rect.width() < 22.0 {
        return;
    }
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let y = rect.center().y;
    let mut left = rect.min.x + EDGE_W + 5.0;
    let stop = paint_marks(&p, item, &look, rect.max.x - 4.0, left, y);
    // A timed event that runs over days says when it starts.
    if !item.ev.all_day && starts {
        left += paint_clipped(&p, pos2(left, y), stop - left, &crate::dates::clock(item.occ.start, twelve), regular(10.5), look.dim) + 5.0;
    }
    let used = paint_line(&p, pos2(left, y), stop - left, item.title(), semibold(11.5), look.ink, item.ev.cancelled);
    let place = clip_text(item.ev.location.trim(), 200);
    if !place.is_empty() && stop - left - used > 48.0 {
        paint_clipped(&p, pos2(left + used + 8.0, y), stop - left - used - 8.0, place, regular(10.5), look.dim);
    }
}

/// A timed event in a month cell: a chip tinted like its block in the hours, with the same
/// edge on its left, then the time and the title.
fn paint_chip(p: &Painter, rect: Rect, item: &Item, t: &Tokens, twelve: bool, selected: bool, hovered: bool) {
    let look = look(&item.ev, item.color, item.edge, t, hovered, item.dim);
    p.rect_filled(rect, 3.0, look.fill);
    if look.outline {
        p.rect_stroke(rect, 3.0, Stroke::new(1.0, mix(t.surface, look.edge, 0.7)), StrokeKind::Inside);
    }
    paint_edge(p, Rect::from_min_max(rect.min, pos2(rect.min.x + EDGE_W.min(rect.width()), rect.max.y)), &look, t, 3);
    if selected {
        p.rect_stroke(rect, 3.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
    }
    if rect.width() < 22.0 {
        return;
    }
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let y = rect.center().y;
    let mut left = rect.min.x + EDGE_W + 5.0;
    let stop = paint_marks(&p, item, &look, rect.max.x - 3.0, left + 40.0, y);
    left += paint_clipped(&p, pos2(left, y), stop - left, &crate::dates::clock(item.occ.start, twelve), regular(10.5), look.dim) + 5.0;
    paint_line(&p, pos2(left, y), stop - left, item.title(), semibold(11.5), look.ink, item.ev.cancelled);
}

// ---- acting on an event ----

/// `Wed, Oct 14 · 11:00 AM – 12:00 PM`, or the days of an event that runs over several.
fn when_label(item: &Item, twelve: bool) -> String {
    let (start, end) = (item.occ.start, item.occ.end);
    let last = item.last_day();
    let day = crate::dates::weekday_month_day;
    if item.ev.all_day {
        if last == start.date {
            format!("{} · {}", day(start.date), tl!("All day"))
        } else {
            format!("{} – {} · {}", day(start.date), day(last), tl!("All day"))
        }
    } else if last == start.date {
        // (When this occurrence is: one of a series by another zone's clock is not always at
        // the time of the first.)
        format!("{} · {} – {}", day(start.date), crate::dates::clock(start, twelve), crate::dates::clock(end, twelve))
    } else {
        format!("{} {} – {} {}", day(start.date), crate::dates::clock(start, twelve), day(end.date), crate::dates::clock(end, twelve))
    }
}

/// When an occurrence is by the clock of its event's own zone (`3:00 PM – 4:00 PM (Eastern
/// Time)`), for an event that has a zone whose clock shows another time than the calendar's.
fn own_time(s: &mailcraft_engine::Session, item: &Item) -> Option<String> {
    item.ev.zone_of_end()?;
    let at = Event { start: item.occ.start, end: item.occ.end, ..(*item.ev).clone() };
    mailcraft_model::zoned::own_time_label(&at, &|instant| s.offset_at(instant), s.prefs.twelve_hour)
}

/// What hovering an event says: title, when (and when that is where the event is at home),
/// where, who, how it repeats.
fn tooltip(ui: &mut Ui, item: &Item, twelve: bool, own: Option<&str>) {
    let t = Tokens::get(ui.ctx());
    let ev = &item.ev;
    ui.set_max_width(300.0);
    let line = |ui: &mut Ui, text: &str| ui.label(egui::RichText::new(text).font(regular(11.5)).color(t.text_dim));
    ui.label(egui::RichText::new(item.title()).font(semibold(12.5)).color(t.text));
    ui.label(egui::RichText::new(when_label(item, twelve)).font(regular(11.5)).color(t.text));
    if let Some(own) = own {
        line(ui, own);
    }
    let place = clip_text(ev.location.trim(), 200);
    if !place.is_empty() {
        line(ui, place);
    }
    if ev.is_meeting() {
        let count = ev.attendees.len();
        let people = tln!(count, "{n} attendee", "{n} attendees");
        match &ev.organizer {
            Some(who) => line(ui, &format!("{} · {people}", crate::i18n::fmt(tl!("Organizer: {name}"), &[("name", clip_text(who.display(), 100))]))),
            None => line(ui, &people),
        };
    }
    if let Some(rule) = &ev.recurrence {
        line(ui, &rule.describe(ev.start.date));
    }
    if !ev.categories.is_empty() {
        let names: Vec<&str> = ev.categories.iter().take(6).map(|c| clip_text(c, 60)).collect();
        line(ui, &names.join(", "));
    }
    let mut state: Vec<&str> = Vec::new();
    if ev.cancelled {
        state.push(tl!("Cancelled"));
    }
    if ev.show_as != ShowAs::Busy {
        state.push(tl!(ev.show_as.label()));
    }
    if ev.private {
        state.push(tl!("Private"));
    }
    match ev.importance {
        Importance::High => state.push(tl!("High importance")),
        Importance::Low => state.push(tl!("Low importance")),
        Importance::Normal => {}
    }
    if let Some(online) = &ev.online {
        state.push(if online.provider == mailcraft_model::OnlineProvider::Teams { tl!("Teams meeting") } else { tl!("Online meeting") });
    }
    if !state.is_empty() {
        line(ui, &state.join(" · "));
    }
}

/// A double click, or a third click hard on its heels (which egui counts as a triple one).
fn opens(resp: &Response) -> bool {
    resp.double_clicked() || resp.triple_clicked()
}

/// Whether an event is a meeting somebody else organizes, to which the mailbox's owner is
/// invited (its server says so, or its organizer is none of the owner's accounts); and what the
/// owner has answered so far, when the owner is among the people invited.
pub(crate) fn invitation(app: &MailApp, ev: &Event) -> (bool, Option<Reply>) {
    let own = |address: &mailcraft_model::Address| {
        let email = address.email.trim();
        !email.is_empty() && app.session.mailbox.accounts.iter().take(50).any(|account| account.email.trim().eq_ignore_ascii_case(email))
    };
    let invited = ev.is_meeting() && (ev.origin.guest || ev.organizer.as_ref().is_some_and(|who| !own(who)));
    (invited, ev.attendees.iter().take(500).find(|a| own(&a.address)).map(|a| a.response))
}

/// Clicks on a drawn event: select, open on a double click, the context menu, and the tooltip.
fn event_input(app: &mut MailApp, resp: Response, item: &Item, tips: bool) {
    let id = item.ev.id;
    let day = item.occ.start.date.iso();
    if opens(&resp) {
        // Open acts on the selection: in a window of its own, or in the form here where there
        // are no such windows.
        let _ = app.execute("calendar.select", json!({"id": id, "day": day}));
        if app.run_or_else("ui.openWindow", json!({"event": id})).is_err() {
            let _ = app.run("calendar.edit", json!({}));
        }
    } else if resp.clicked() {
        let _ = app.run("calendar.select", json!({"id": id, "day": day}));
    }
    // What is clicked is in its calendar's block: that calendar is the one worked in from now
    // on, and of the calendars that lie over one another it comes to the front.
    if resp.clicked() || opens(&resp) {
        if item.dim {
            let _ = app.execute("view.frontCalendar", json!({"calendar": item.ev.calendar}));
        } else {
            work_in(app, item.ev.calendar);
        }
    }
    // The menu acts on the event under the pointer.
    if resp.secondary_clicked() {
        let _ = app.execute("calendar.select", json!({"id": id, "day": day}));
    }
    let (repeats, show_as, private) = (item.ev.recurrence.is_some(), item.ev.show_as, item.ev.private);
    // An event of the mailbox's own can be put into another calendar (into a server's too, which
    // then has it). One that a server keeps can go into another calendar of the same account
    // (the engine says what can't, and why: a meeting, a series); out of the account it can't,
    // as taking it out would delete it there.
    let own = mailcraft_engine::cmd::remote::keeper(&app.session.mailbox, &item.ev.origin).is_none();
    let account = item.ev.origin.account;
    let others: Vec<(Id, String)> = {
        let calendars = app.session.mailbox.calendars.iter().take(50).filter(|c| c.id != item.ev.calendar);
        let fits = |c: &&mailcraft_model::Calendar| own || (c.origin.account == account && c.can_edit());
        calendars.filter(fits).map(|c| (c.id, calendar_label(&app.session, c))).collect()
    };
    let importance = item.ev.importance;
    let proposable = !own && item.ev.origin.guest && item.ev.response_options.allow_proposals;
    // The link to join an online meeting by, when its server gave one that is one.
    let join = item.ev.join_link().and_then(mailcraft_model::OnlineMeeting::clean_link);
    let online = item.ev.online.as_ref().map(|online| online.provider.label());
    // Somebody else's meeting can be answered from here; what the owner answered so far is ticked.
    let (invited, answered) = invitation(app, &item.ev);
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        use crate::widgets::{mi, mi_check};
        mi(ui, app, "Open", "calendar.edit", json!({}));
        if invited {
            ui.separator();
            for (label, code, answer) in
                [("Accept", "accepted", Reply::Accepted), ("Tentative", "tentative", Reply::Tentative), ("Decline", "declined", Reply::Declined)]
            {
                // (What the command refuses, the status bar says.)
                mi_check(ui, app, label, answered == Some(answer), "calendar.respond", json!({"id": id, "response": code}));
            }
            // Another time, for a meeting whose organizer takes proposals and whose server
            // carries them (its dialog says who is written to, and sends from its own buttons).
            if ui.add_enabled(proposable, egui::Button::new(tl!("Propose New Time…")).min_size(vec2(190.0, 0.0))).clicked() {
                let _ = app.run("calendar.propose", json!({"id": id}));
                ui.close();
            }
            ui.separator();
        }
        // Joined in the system's browser, by the link its server gave: shown where it leads.
        if let (Some(link), Some(with)) = (&join, online) {
            let label = crate::i18n::fmt(tl!("Join {service} Meeting"), &[("service", tl!(with))]);
            if ui.add(egui::Button::new(label).min_size(vec2(190.0, 0.0))).on_hover_text(link.as_str()).clicked() {
                let _ = app.run("ui.joinMeeting", json!({"event": id}));
                ui.close();
            }
        }
        mi(ui, app, "Forward…", "calendar.forward", json!({"id": id}));
        if repeats {
            mi(ui, app, "Forward This Occurrence…", "calendar.forward", json!({"id": id, "scope": "occurrence", "occurrence": day}));
        }
        mi(ui, app, "Delete", "calendar.delete", json!({"id": id}));
        if repeats {
            mi(ui, app, "Delete This Occurrence", "calendar.delete", json!({"id": id, "occurrence": day}));
        }
        if !others.is_empty() {
            ui.menu_button(tl!("Move to Calendar"), |ui| {
                for (calendar, label) in &others {
                    mi(ui, app, label, "calendar.edit", json!({"id": id, "calendar": calendar}));
                }
            });
        }
        ui.separator();
        ui.menu_button(tl!("Show As"), |ui| {
            for s in ShowAs::ALL {
                mi_check(ui, app, s.label(), s == show_as, "calendar.showAs", json!({"id": id, "value": s.code()}));
            }
        });
        mi_check(ui, app, "Private", private, "calendar.private", json!({"id": id}));
        ui.menu_button(tl!("Importance"), |ui| {
            for (label, value) in [("High", Importance::High), ("Normal", Importance::Normal), ("Low", Importance::Low)] {
                mi_check(ui, app, label, importance == value, "calendar.importance", json!({"id": id, "value": value.code()}));
            }
        });
        ui.separator();
        mi(ui, app, "Save as iCalendar…", "file.exportCalendar", json!({"id": id}));
    });
    if tips {
        let twelve = app.session.prefs.twelve_hour;
        let session = &app.session;
        resp.on_hover_ui(|ui| tooltip(ui, item, twelve, own_time(session, item).as_deref()));
    }
}

/// Clicks on empty calendar: one picks the slot out (and lets go of the event that was
/// selected), two open a new appointment for it. `pick` is the slot under the pointer.
fn empty_input(app: &mut MailApp, resp: &Response, slot: &mut Option<Slot>, pick: impl Fn() -> Slot) {
    if opens(resp) {
        let _ = app.run("ui.openWindow", pick().form());
        *slot = None;
    } else if resp.clicked() {
        let _ = app.run("calendar.select", json!({"id": null}));
        let picked = pick();
        // The block that is clicked in is the calendar being worked in from now on.
        work_in(app, picked.calendar);
        *slot = Some(picked);
    }
    // The right button inside the pick leaves it as it is; anywhere else it picks what is under
    // it. Either way the menu makes something of the pick.
    if resp.secondary_clicked() {
        let picked = pick();
        if !slot.as_ref().is_some_and(|s| s.covers(&picked)) {
            let _ = app.run("calendar.select", json!({"id": null}));
            work_in(app, picked.calendar);
            *slot = Some(picked);
        }
    }
    if let Some(picked) = slot.as_ref().filter(|s| s.text.is_none()).cloned() {
        resp.context_menu(|ui| {
            ui.set_min_width(200.0);
            use crate::widgets::mi;
            mi(ui, app, "New Appointment", "ui.openWindow", picked.form_for(Kind::Appointment));
            mi(ui, app, "New Meeting", "ui.openWindow", picked.form_for(Kind::Meeting));
            mi(ui, app, "New All Day Event", "ui.openWindow", picked.form_for(Kind::AllDay));
        });
    }
}

/// What a slot's entry came to: make the appointment, open the form, or let go of the slot.
fn finish_entry(app: &mut MailApp, slot: &mut Option<Slot>, entry: Entry) {
    match entry {
        Entry::Stay => {}
        Entry::Drop => *slot = None,
        Entry::Make(title) => {
            if let Some(picked) = slot.take() {
                // The new appointment is selected in the slot's place.
                let _ = app.run("calendar.new", picked.appointment(&title));
            }
        }
        Entry::Form => {
            if let Some(picked) = slot.take() {
                let _ = app.run("ui.openWindow", picked.form());
            }
        }
    }
}

/// Show `day` in the Day view (what `+n more` does).
fn open_day(app: &mut MailApp, day: Date) {
    let _ = app.run("view.goToDate", json!({"date": day.iso()}));
    let _ = app.run("view.day", json!({}));
}

/// `+3 more` in `rect`; a click shows the day.
fn more_link(app: &mut MailApp, ui: &Ui, rect: Rect, salt: (&str, usize), count: usize, day: Date) {
    let t = Tokens::get(ui.ctx());
    let resp = ui.interact(rect, ui.scope_id().with(salt), Sense::click());
    if let Ok(id) = u64::try_from(day.to_days()) {
        app.hit.add("more", id, rect);
    }
    if resp.hovered() {
        ui.painter().rect_filled(rect, 3.0, t.hover);
    }
    let text = crate::i18n::fmt(tl!("+{n} more"), &[("n", &count.to_string())]);
    paint_clipped(
        ui.painter(),
        pos2(rect.min.x + 5.0, rect.center().y),
        rect.width() - 8.0,
        &text,
        regular(10.5),
        if resp.hovered() { t.accent_text } else { t.text_dim },
    );
    if resp.clicked() {
        open_day(app, day);
    }
}

// ---- dragging an event ----

/// The part of a drawn event that was grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grip {
    /// The event itself: it moves.
    Move,
    /// Its upper edge in the hours, or the left end of a bar: when it starts.
    Start,
    /// Its lower edge, or the right end of a bar: when it ends.
    End,
}

/// What of the calendar is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Under {
    /// A minute of a day, in the hours.
    Time { day: Date, minute: i64 },
    /// A day: its place in the row of whole days, or its cell of the month.
    Day(Date),
}

/// When an event is, or would be: from, up to (not including), and whether as whole days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: DateTime,
    pub(crate) end: DateTime,
    pub(crate) all_day: bool,
}

/// How a drawn piece of an event can be grabbed.
#[derive(Clone, Copy, Debug)]
struct Piece {
    /// A bar across days (its ends change its days) rather than a block in the hours (whose
    /// edges change its times).
    bar: bool,
    /// The event begins, and ends, in this piece: those are the edges that can be dragged.
    starts: bool,
    ends: bool,
}

/// The part of a piece drawn at `rect` that a press at `pos` grabs.
fn grip_of(rect: Rect, pos: egui::Pos2, piece: Piece) -> Grip {
    let (lo, hi, at) = if piece.bar { (rect.min.x, rect.max.x, pos.x) } else { (rect.min.y, rect.max.y, pos.y) };
    // A quarter of it at each end, at most a few points: the middle is always there to move it by.
    let reach = ((hi - lo) / 4.0).clamp(0.0, if piece.bar { 7.0 } else { 5.0 });
    if piece.ends && at >= hi - reach {
        Grip::End
    } else if piece.starts && at <= lo + reach {
        Grip::Start
    } else {
        Grip::Move
    }
}

/// A time without its seconds, which a drag doesn't keep.
fn to_minute(t: DateTime) -> DateTime {
    DateTime::at_midnight(t.date).add_minutes(i64::from(t.minutes_of_day()))
}

/// The key that makes a drop a copy: Ctrl, and Option on a Mac (where Ctrl makes a click the
/// right button's).
fn copy_key(modifiers: egui::Modifiers) -> bool {
    if cfg!(target_os = "macos") { modifiers.alt } else { modifiers.ctrl }
}

/// An event being dragged: to another time or day, by an edge to another length, with Ctrl held
/// to be copied there, or with the right button to be asked which.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Drag {
    event: Id,
    /// The occurrence that was grabbed, to the minute.
    start: DateTime,
    end: DateTime,
    all_day: bool,
    /// It is drawn as a bar across days rather than as a block in the hours.
    bar: bool,
    grip: Grip,
    /// With the right button: the drop asks whether to move or to copy.
    menu: bool,
    /// What was under the pointer when it was grabbed.
    press: Under,
    /// It repeats: a series stays whole days, or stays in the hours, whatever it is dropped on.
    repeats: bool,
    /// When it would be if it were dropped now; `None` where it can't be dropped.
    pub(crate) target: Option<Span>,
    /// The drop makes a copy (Ctrl is held).
    pub(crate) copy: bool,
    cancelled: bool,
    /// The block it was grabbed in, and the one the pointer is over now (for an edge that is
    /// dragged: its own).
    pane: usize,
    over: usize,
    /// The calendar of another block that it is over: dropped there it goes into that calendar,
    /// or a copy of it does. 0 over its own block.
    pub(crate) into: Id,
}

impl Drag {
    /// The occurrence as it is.
    fn here(&self) -> Span {
        Span { start: self.start, end: self.end, all_day: self.all_day }
    }

    fn minutes(&self) -> i64 {
        self.start.minutes_until(self.end).max(0)
    }

    /// When the occurrence would be with the pointer over `under`. In the hours it goes by
    /// `step` minutes, and a pointer that has gone less than half a step leaves it as it is;
    /// whole days dropped into the hours get the half hour of the `slot` they land in.
    pub(crate) fn span(&self, under: Under, step: i64, slot: i64) -> Span {
        let step = step.clamp(1, DAY_MINUTES);
        let here = self.here();
        let midnight = DateTime::at_midnight;
        let nudge = |from: i64, to: i64| to.saturating_sub(from).saturating_abs().saturating_mul(2) < step;
        match (self.grip, self.press, under) {
            // Down the hours by as much as the pointer went; across them, to another day.
            (Grip::Move, Under::Time { day: from, minute: was }, Under::Time { day, minute }) => {
                let at = drag_minute(i64::from(self.start.minutes_of_day()), minute.saturating_sub(was), step);
                let start = midnight(self.start.date.add_days(from.days_until(day))).add_minutes(at);
                Span { start, end: start.add_minutes(self.minutes()), all_day: false }
            }
            // Out of the hours into the row of whole days: it takes that day.
            (Grip::Move, Under::Time { .. }, Under::Day(day)) if !self.repeats => {
                Span { start: midnight(day), end: midnight(day).add_days(1), all_day: true }
            }
            // Whole days, and a timed event in the month, go by days and keep their time of day.
            (Grip::Move, Under::Day(from), Under::Day(day)) => {
                let days = from.days_until(day);
                Span { start: self.start.add_days(days), end: self.end.add_days(days), all_day: self.all_day }
            }
            // Whole days into the hours: half an hour where they land. (An event that is long
            // rather than whole days starts there and stays as long.)
            (Grip::Move, Under::Day(_), Under::Time { day, minute }) if !self.repeats => {
                if self.all_day {
                    let (start, _) = slot_at(day, minute, slot);
                    Span { start, end: start.add_minutes(30), all_day: false }
                } else {
                    let start = midnight(day).add_minutes(snap_down(minute.clamp(0, DAY_MINUTES - step), step));
                    Span { start, end: start.add_minutes(self.minutes()), all_day: false }
                }
            }
            // The lower edge of a block: when it ends, on the day it ends, never less than a
            // step after it starts.
            (Grip::End, Under::Time { minute: was, .. }, Under::Time { minute, .. }) if !self.bar && !nudge(was, minute) => {
                let day = last_day(self.start, self.end);
                let ends = midnight(day).minutes_until(self.end);
                let at = snap(ends.saturating_add(minute.saturating_sub(was)), step).clamp(0, DAY_MINUTES);
                Span { end: midnight(day).add_minutes(at).max(self.start.add_minutes(step)), ..here }
            }
            // The upper edge: when it starts, on the day it starts.
            (Grip::Start, Under::Time { minute: was, .. }, Under::Time { minute, .. }) if !self.bar && !nudge(was, minute) => {
                let starts = i64::from(self.start.minutes_of_day());
                let at = snap(starts.saturating_add(minute.saturating_sub(was)), step).clamp(0, DAY_MINUTES - step);
                Span { start: midnight(self.start.date).add_minutes(at).min(self.end.add_minutes(-step)), ..here }
            }
            // The ends of a bar: its last day, never before the first; and its first, never
            // after the last.
            (Grip::End, _, Under::Day(day)) if self.bar => {
                let end = self.end.add_days(last_day(self.start, self.end).days_until(day.max(self.start.date)));
                if end > self.start { Span { end, ..here } } else { here }
            }
            (Grip::Start, _, Under::Day(day)) if self.bar => {
                let start = self.start.add_days(self.start.date.days_until(day.min(last_day(self.start, self.end))));
                if start < self.end { Span { start, ..here } } else { here }
            }
            // Anywhere that edge, or that event, can't go: it stays as it is.
            _ => here,
        }
    }
}

/// The hours as they were drawn this frame.
#[derive(Clone, Copy, Debug)]
struct Hours {
    cols: Blocks,
    /// Where midnight is, scrolled as the hours are.
    top: f32,
    /// The part of them that is in view.
    seen: Rect,
    scale: Scale,
}

/// The month as it was drawn this frame.
#[derive(Clone, Copy, Debug)]
struct MonthCells {
    cols: Blocks,
    area: Rect,
    /// Height of a week.
    cell_h: f32,
    weeks: usize,
}

/// Where the parts of the calendar were drawn this frame, so a drag that began in one of them
/// can be followed into another.
#[derive(Clone, Copy, Debug, Default)]
struct Zones {
    /// The first day drawn.
    from: Date,
    hours: Option<Hours>,
    /// The row of whole days over the hours: its columns and where it is.
    all_day: Option<(Blocks, Rect)>,
    month: Option<MonthCells>,
    /// The calendar in front in each block, from left to right; 0 beyond the last block.
    fronts: [Id; MAX_PANES],
}

impl Zones {
    /// The blocks of calendars, as whichever part of the calendar was drawn has them.
    fn blocks(&self) -> Option<Blocks> {
        self.month.map(|m| m.cols).or(self.hours.map(|h| h.cols)).or(self.all_day.map(|(cols, _)| cols))
    }

    /// The block at `pos` (the nearest one, for a point beside them all).
    fn block_at(&self, pos: egui::Pos2) -> usize {
        self.blocks().map_or(0, |blocks| blocks.block_at(pos.x))
    }

    /// What of the calendar is at `pos`; `None` away from it. (In which block is another
    /// matter: [`Zones::block_at`]. Every block has the same days.)
    fn under(&self, pos: egui::Pos2) -> Option<Under> {
        if !pos.x.is_finite() || !pos.y.is_finite() {
            return None;
        }
        let day = |n: usize| self.from.add_days(i64::try_from(n).unwrap_or(0));
        if let Some(m) = &self.month {
            if !m.area.contains(pos) || m.cell_h <= 0.0 {
                return None;
            }
            let week = (((pos.y - m.area.min.y) / m.cell_h).floor().max(0.0) as usize).min(m.weeks.saturating_sub(1));
            return Some(Under::Day(day(week.saturating_mul(7).saturating_add(m.cols.at(pos.x).index_at(pos.x)))));
        }
        let left = |cols: &Blocks| cols.left() - GUTTER_W;
        if let Some((cols, row)) = &self.all_day
            && pos.y < row.max.y
        {
            // The row of whole days, and the days' headings over it.
            let inside = pos.y >= row.min.y - DAY_HEAD_H && pos.x >= left(cols);
            return inside.then(|| Under::Day(day(cols.at(pos.x).index_at(pos.x))));
        }
        let h = self.hours.as_ref()?;
        if pos.x < left(&h.cols) {
            return None;
        }
        // Below (or above) the hours in view: their last (or first) row there is.
        let y = pos.y.clamp(h.seen.min.y, h.seen.max.y.max(h.seen.min.y));
        Some(Under::Time { day: day(h.cols.at(pos.x).index_at(pos.x)), minute: h.scale.minute(y - h.top) })
    }
}

/// What the parts of the calendar tell one another while a frame is laid out: where each was
/// drawn, the drag that is going on, and what the keyboard asked the hours to show.
struct Frame {
    zones: Zones,
    /// Where they were in the last frame, for what happens before they are drawn again.
    before: Zones,
    /// The drag as the last frame left it, or as it began in this one.
    drag: Option<Drag>,
    /// The dragged event was drawn this frame, and still has the pointer.
    alive: bool,
    /// The button was let go this frame.
    dropped: bool,
    pointer: Option<egui::Pos2>,
    /// Minutes of the day the hours are to scroll into view (the keyboard moved the pick there).
    reveal: Option<(i64, i64)>,
}

impl Frame {
    fn begin(ui: &Ui) -> Frame {
        Frame {
            zones: Zones::default(),
            before: ui.ctx().data(|d| d.get_temp(egui::Id::unique(ZONES_ID))).unwrap_or_default(),
            drag: ui.ctx().data(|d| d.get_temp(egui::Id::unique(DRAG_ID))),
            alive: false,
            dropped: false,
            pointer: ui.input(|i| i.pointer.interact_pos()),
            reveal: None,
        }
    }

    /// Whether `item` is the occurrence being dragged.
    fn holds(&self, item: &Item) -> bool {
        self.drag.is_some_and(|d| d.event == item.ev.id && d.start == to_minute(item.occ.start))
    }

    /// Whether `item` fades where it is: it is being moved away, and its ghost shows where to.
    fn fades(&self, item: &Item) -> bool {
        self.holds(item) && self.drag.is_some_and(|d| !d.cancelled && !d.copy && !d.menu && d.target.is_some())
    }

    /// Follow what the pointer does to a drawn piece of `item` at `rect`: its edges say what
    /// they are for, a drag of it begins here, and one that is going on is kept alive. `under`
    /// says what of the calendar a point of this part is.
    fn track(&mut self, ui: &Ui, resp: &Response, item: &Item, rect: Rect, piece: Piece, under: &dyn Fn(egui::Pos2) -> Under) {
        let (primary, secondary) =
            ui.input(|i| (i.pointer.button_down(egui::PointerButton::Primary), i.pointer.button_down(egui::PointerButton::Secondary)));
        if self.drag.is_none()
            && resp.hovered()
            && !primary
            && !secondary
            && let Some(pos) = self.pointer
            && grip_of(rect, pos, piece) != Grip::Move
        {
            ui.ctx().set_cursor_icon(if piece.bar { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
        }
        if resp.drag_started() && (primary || secondary) {
            let press = ui.input(|i| i.pointer.press_origin()).or(self.pointer).unwrap_or(rect.center());
            // The right button always moves (or copies): which, the drop asks.
            let menu = secondary && !primary;
            let start = to_minute(item.occ.start);
            self.drag = Some(Drag {
                event: item.ev.id,
                start,
                end: to_minute(item.occ.end).max(start),
                all_day: item.ev.all_day,
                bar: item.is_bar(),
                grip: if menu { Grip::Move } else { grip_of(rect, press, piece) },
                menu,
                press: under(press),
                repeats: item.ev.recurrence.is_some(),
                target: None,
                copy: false,
                cancelled: false,
                pane: item.pane,
                over: item.pane,
                into: 0,
            });
        }
        if (resp.dragged() || resp.drag_stopped()) && self.holds(item) {
            self.alive = true;
            self.dropped |= resp.drag_stopped();
        }
    }

    /// The drag at the end of the frame, when every part has been drawn: where the event would
    /// land now, its ghost there, and the drop when the button was let go.
    fn finish(&mut self, app: &mut MailApp, ui: &Ui, items: &[Item]) {
        let id = egui::Id::unique(DRAG_ID);
        let Some(mut drag) = self.drag.take() else { return };
        if !self.alive {
            // The dragged event went away (deleted, hidden, another view).
            ui.ctx().data_mut(|d| d.remove::<Drag>(id));
            return;
        }
        let scale = Scale::of(time_scale(&app.session));
        // Ctrl as the keyboard holds it, or as the button that was let go came with it (which
        // is all a script's drag says).
        let (escape, copy) = ui.input(|i| {
            let let_go = |e: &egui::Event| matches!(e, egui::Event::PointerButton { pressed: false, modifiers, .. } if copy_key(*modifiers));
            (i.key_pressed(egui::Key::Escape), copy_key(i.modifiers) || i.events.iter().any(let_go))
        });
        drag.cancelled |= escape;
        drag.copy = copy && drag.grip == Grip::Move && !drag.menu;
        let under = self.pointer.and_then(|pos| self.zones.under(pos));
        drag.target = under.map(|under| drag.span(under, scale.step(), scale.slot)).filter(|_| !drag.cancelled);
        // Over another calendar's block, the event itself would go into that calendar (an edge
        // that is dragged stays its own block's, wherever the pointer strays).
        let moving = drag.grip == Grip::Move && drag.target.is_some();
        drag.over = self.pointer.filter(|_| moving).map_or(drag.pane, |pos| self.zones.block_at(pos));
        drag.into = if drag.over == drag.pane { 0 } else { self.zones.fronts.get(drag.over).copied().unwrap_or(0) };
        if self.dropped {
            ui.ctx().data_mut(|d| d.remove::<Drag>(id));
            match drag.target {
                // The right button: what to do is asked where it was let go.
                Some(span) if drag.menu => {
                    if span != drag.here() || drag.into != 0 {
                        let ask = DropMenu { drag, span, pos: self.pointer.unwrap_or_default() };
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(DROP_MENU_ID), ask));
                    }
                }
                Some(span) => drop_event(app, &drag, span, drag.copy),
                None => {}
            }
            return;
        }
        ui.ctx().data_mut(|d| d.insert_temp(id, drag));
        let Some(span) = drag.target else { return };
        let item = items.iter().find(|i| i.ev.id == drag.event);
        if let Some(at) = paint_ghost(ui, &self.zones, item, span, drag.copy, app.session.prefs.twelve_hour, drag.over) {
            app.hit.add(if drag.copy { "ghostCopy" } else { "ghost" }, drag.event, at);
        }
        ui.ctx().set_cursor_icon(match (drag.grip, drag.bar) {
            (Grip::Move, _) if drag.copy => egui::CursorIcon::Copy,
            (Grip::Move, _) => egui::CursorIcon::Grabbing,
            (_, true) => egui::CursorIcon::ResizeHorizontal,
            (_, false) => egui::CursorIcon::ResizeVertical,
        });
    }
}

// ---- the keyboard ----

/// An event that was copied or cut in the calendar, for Paste to put where a time is picked.
#[derive(Clone, Debug, PartialEq)]
struct Clip {
    event: Id,
    /// The occurrence that was selected then, to the minute.
    start: DateTime,
    cut: bool,
    /// What went on the system's clipboard for it: a Paste of anything else isn't this event.
    text: String,
}

/// The keys of the grid, while nothing else has the keyboard: the arrows move the pick (Shift
/// stretches it), Home and End go to the ends of the working day, Page Up and Page Down by a
/// screen; Tab goes from event to event and Enter opens the one that is selected; Alt with Up
/// or Down moves that event by a row (with Shift, its end); Ctrl with C, X and V copy, cut and
/// paste it where a time is picked; Alt with a digit shows that many days, Alt with = the
/// month and Alt with - the week. (Delete is the main window's key, and deletes the selected
/// event.) Returns whether the days in view changed.
fn keyboard(app: &mut MailApp, ui: &Ui, frame: &mut Frame, items: &[Item]) -> bool {
    let ctx = ui.ctx().clone();
    let wanted = |e: &egui::Event| matches!(e, egui::Event::Key { pressed: true, .. } | egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_));
    let events: Vec<egui::Event> = ctx.input(|i| i.events.iter().filter(|e| wanted(e)).take(32).cloned().collect());
    if events.is_empty() {
        return false;
    }
    let free = has_keyboard(app, &ctx);
    // Tab is egui's key for going from field to field, and by now it has given the keyboard to
    // one: what counts is whether the calendar had it before.
    let had = ctx.data(|d| d.get_temp::<bool>(egui::Id::unique(KEYS_ID))).unwrap_or(false);
    let asking = ctx.data(|d| d.get_temp::<DropMenu>(egui::Id::unique(DROP_MENU_ID)).is_some());
    let open = app.dialog.is_none() && !app.ui.backstage && !app.windows.over_main() && !egui::Popup::is_any_open(&ctx) && !asking;
    let mut moved = false;
    for event in events {
        match event {
            egui::Event::Key { key: egui::Key::Tab, modifiers, .. } if !modifiers.alt && !modifiers.command => {
                if had && open {
                    ctx.memory_mut(|m| {
                        if let Some(field) = m.focused() {
                            m.surrender_focus(field);
                        }
                        m.move_focus(egui::FocusDirection::None);
                    });
                    step_selection(app, &ctx, frame, items, !modifiers.shift);
                }
            }
            _ if !free => {}
            egui::Event::Copy => clip(app, &ctx, false),
            egui::Event::Cut => clip(app, &ctx, true),
            egui::Event::Paste(text) => paste(app, &ctx, Some(&text)),
            egui::Event::Key { key, modifiers, .. } => moved |= key_pressed(app, &ctx, frame, key, modifiers),
            _ => {}
        }
    }
    moved
}

/// The number a digit key stands for in "show that many days": 1 to 9, and 10 for 0.
fn days_key(key: egui::Key) -> Option<u8> {
    use egui::Key;
    let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9, Key::Num0];
    keys.iter().position(|k| *k == key).and_then(|at| u8::try_from(at + 1).ok())
}

/// One key of the calendar's ([`keyboard`]). Returns whether the days in view changed.
fn key_pressed(app: &mut MailApp, ctx: &egui::Context, frame: &mut Frame, key: egui::Key, modifiers: egui::Modifiers) -> bool {
    use egui::Key;
    let (alt, shift, command) = (modifiers.alt, modifiers.shift, modifiers.command);
    match key {
        _ if alt && !command && !shift && days_key(key).is_some() => {
            // From the day that is picked, or the selected event's, or the one in view.
            let picked = load_slot(ctx).map(|slot| slot.start.date);
            let day = picked.or(app.session.sel.event.and(app.session.sel.event_day)).unwrap_or(app.session.view.date);
            app.run("view.days", json!({"count": days_key(key), "start": day.iso()})).is_ok()
        }
        Key::Equals | Key::Plus if alt && !command => app.run("view.month", json!({})).is_ok(),
        Key::Minus if alt && !command => app.run("view.week", json!({})).is_ok(),
        Key::ArrowUp | Key::ArrowDown if alt && !command => {
            nudge_event(app, frame, key == Key::ArrowDown, shift);
            false
        }
        Key::C | Key::X if command && !alt && !shift => {
            clip(app, ctx, key == Key::X);
            false
        }
        Key::V if command && !alt && !shift => {
            paste(app, ctx, None);
            false
        }
        // Enter on a pick is the pick's own ([`slot_entry`]); with an event selected it opens it.
        Key::Enter if !alt && !command && load_slot(ctx).is_none() => {
            if let Some(id) = app.session.sel.event
                && app.run_or_else("ui.openWindow", json!({"event": id})).is_err()
            {
                let _ = app.run("calendar.edit", json!({}));
            }
            false
        }
        Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End | Key::PageUp | Key::PageDown if !alt && !command => {
            move_pick(app, ctx, frame, key, shift)
        }
        _ => false,
    }
}

/// The arrows, Home, End, Page Up and Page Down: move what is picked (with Shift, stretch it).
/// With nothing picked they start from the selected event, or from the start of the working
/// day. Returns whether the days in view changed.
fn move_pick(app: &mut MailApp, ctx: &egui::Context, frame: &mut Frame, key: egui::Key, shift: bool) -> bool {
    use egui::Key;
    let scale = Scale::of(time_scale(&app.session));
    let month = app.session.view.calendar == CalView::Month;
    let (from, to) = calendar_range(&app.session);
    // Home and End go to the ends of the working day: the mailbox's own, or the local options'.
    let work = calendar_settings::work_hours(&app.session);
    let (work_start, work_end) = (work.start.clamp(0, DAY_MINUTES - 60), work.end.clamp(0, DAY_MINUTES));
    let in_view = |day: &Date| (from..=to).contains(day);
    let selected = app.session.sel.event.and_then(|id| {
        let e = app.session.mailbox.event(id)?;
        let day = app.session.sel.event_day.unwrap_or(e.start.date);
        Some((day, e.all_day || e.duration_minutes() >= DAY_MINUTES, i64::from(e.start.minutes_of_day()), e.calendar))
    });
    let slot = match (load_slot(ctx).filter(|slot| slot.text.is_none()), selected.filter(|(day, ..)| in_view(day))) {
        (Some(slot), _) => slot,
        // From the selected event, in its calendar's block.
        (None, Some((day, whole, minute, calendar))) => {
            let slot = if month || whole { Slot::day(day) } else { Slot::timed(day, minute, scale.slot) };
            slot.in_calendar(front_of(app, calendar))
        }
        (None, None) => {
            // Nothing to start from: the key picks the start of the working day, today's when
            // today is in view, in the block of the calendar being worked in, and goes no further.
            let today = app.session.today();
            let day = [today, app.session.view.date].into_iter().find(in_view).unwrap_or(from);
            let slot = if month { Slot::day(day) } else { Slot::timed(day, work_start, scale.slot) };
            let current = mailcraft_engine::cmd::calendar::current_calendar(&app.session);
            return place(app, ctx, frame, slot.in_calendar(front_of(app, current)));
        }
    };
    let day = slot.cursor.date;
    let back = matches!(key, Key::ArrowLeft | Key::ArrowUp | Key::PageUp | Key::Home);
    let dir = if back { -1 } else { 1 };
    let next = if slot.all_day {
        let target = match key {
            Key::ArrowLeft | Key::ArrowRight => day.add_days(dir),
            Key::ArrowUp | Key::ArrowDown if month => day.add_days(7 * dir),
            // Out of the row of whole days, down into the hours.
            Key::ArrowDown => return place(app, ctx, frame, Slot::timed(day, work_start, scale.slot).in_calendar(slot.calendar)),
            // The first and the last day of the week.
            Key::Home | Key::End if month => from.add_days(from.days_until(day).div_euclid(7) * 7 + if back { 0 } else { 6 }),
            Key::PageUp | Key::PageDown if month => return app.run(if back { "view.previous" } else { "view.next" }, json!({})).is_ok(),
            _ => return false,
        };
        if shift {
            let mut slot = slot;
            slot.stretch_to_day(target);
            slot
        } else {
            Slot::day(target).in_calendar(slot.calendar)
        }
    } else {
        let step = scale.slot;
        let at = i64::from(slot.cursor.minutes_of_day());
        // A screen of rows, less one so the eye keeps its place.
        let screen = frame.before.hours.map_or(8, |hours| ((hours.seen.height() / ROW_H).floor() as i64 - 1).clamp(1, 24 * 60));
        let target = match key {
            // Up out of the hours: the row of whole days.
            Key::ArrowUp if at == 0 && !shift => return place(app, ctx, frame, Slot::day(day).in_calendar(slot.calendar)),
            Key::ArrowUp => at - step,
            Key::ArrowDown => at + step,
            Key::PageUp => at - screen * step,
            Key::PageDown => at + screen * step,
            Key::Home => work_start,
            Key::End => work_end - step,
            Key::ArrowLeft | Key::ArrowRight if !shift => return place(app, ctx, frame, slot.on_day(day.add_days(dir), step)),
            _ => return false,
        };
        let target = target.clamp(0, DAY_MINUTES - step);
        if shift {
            let mut slot = slot.on_day(day, step);
            slot.stretch_to(target);
            slot
        } else {
            Slot::timed(day, target, step).in_calendar(slot.calendar)
        }
    };
    place(app, ctx, frame, next)
}

/// The calendar in front in the block that `calendar` is in (in the first block, when it is in
/// none): where a pick that the keyboard makes belongs.
fn front_of(app: &MailApp, calendar: Id) -> Id {
    let panes = calendar_settings::panes(&app.session);
    let pane = panes.iter().find(|pane| pane.calendars.contains(&calendar)).or(panes.first());
    pane.map_or(0, |pane| pane.front)
}

/// The calendar an event would go into were it put where `slot` is picked: the one of the
/// slot's block, when that is another block than the event's own; `None` within its own block.
fn other_block(app: &MailApp, ev: &Event, slot: &Slot) -> Option<Id> {
    let panes = calendar_settings::panes(&app.session);
    let block = |calendar: Id| panes.iter().position(|pane| pane.calendars.contains(&calendar));
    (slot.calendar != 0 && block(slot.calendar).is_some() && block(slot.calendar) != block(ev.calendar)).then_some(slot.calendar)
}

/// Why an event that repeats isn't taken to another calendar by dragging or pasting.
fn series_stays() -> &'static str {
    tl!(
        "An item that repeats can't be taken to another calendar this way. Hold Ctrl to copy this occurrence there, or use Move to Calendar in its menu for the whole series."
    )
}

/// Pick `slot` out for the keyboard: the event that was selected is let go of, the days in view
/// follow the pick, and the hours scroll to it. Returns whether the days in view changed.
fn place(app: &mut MailApp, ctx: &egui::Context, frame: &mut Frame, mut slot: Slot) -> bool {
    let _ = app.execute("calendar.select", json!({"id": null}));
    work_in(app, slot.calendar);
    let day = slot.cursor.date;
    let (from, to) = calendar_range(&app.session);
    let mut moved = false;
    if !(from..=to).contains(&day) {
        // The view goes to that day. (A work week has no weekend: the next day that it shows.)
        let dir = if day < from { -1 } else { 1 };
        let mut target = day;
        for _ in 0..7 {
            let _ = app.execute("view.goToDate", json!({"date": target.iso()}));
            let (from, to) = calendar_range(&app.session);
            if (from..=to).contains(&target) {
                break;
            }
            target = target.add_days(dir);
        }
        if target != day {
            slot = slot.on_day(target, slot.step);
        }
        moved = true;
    }
    slot.frame = ctx.cumulative_frame_nr();
    if !slot.all_day {
        frame.reveal = Some(slot.minutes());
    }
    store_slot(ctx, Some(slot));
    ctx.request_repaint();
    moved
}

/// Tab and Shift+Tab: select the next event in view, or the one before, in the order of their
/// times; from a time that is picked, the first event from then on (or the last before it).
fn step_selection(app: &mut MailApp, ctx: &egui::Context, frame: &mut Frame, items: &[Item], forward: bool) {
    let n = items.len();
    if n == 0 {
        return;
    }
    let next = match (items.iter().position(|item| is_selected(app, item)), load_slot(ctx)) {
        (Some(at), _) => {
            if forward {
                (at + 1) % n
            } else {
                (at + n - 1) % n
            }
        }
        (None, Some(slot)) if forward => items.iter().position(|item| item.occ.start >= slot.start).unwrap_or(0),
        (None, Some(slot)) => items.iter().rposition(|item| item.occ.start < slot.start).unwrap_or(n - 1),
        (None, None) => {
            if forward {
                0
            } else {
                n - 1
            }
        }
    };
    let Some(item) = items.get(next) else { return };
    let _ = app.execute("calendar.select", json!({"id": item.ev.id, "day": item.occ.start.date.iso()}));
    store_slot(ctx, None);
    if !item.is_bar() {
        let at = i64::from(item.occ.start.minutes_of_day());
        frame.reveal = Some((at, at.saturating_add(item.minutes()).min(DAY_MINUTES)));
    }
    ctx.request_repaint();
}

/// Alt with Up or Down: the selected event a row earlier or later (in the month, a week); with
/// Shift it ends a row earlier or later instead. A series goes as a whole, as when it is dragged.
fn nudge_event(app: &mut MailApp, frame: &mut Frame, later: bool, end_only: bool) {
    let Some(ev) = app.session.sel.event.and_then(|id| app.session.mailbox.events.get(&id).cloned()) else { return };
    let scale = Scale::of(time_scale(&app.session));
    let day = app.session.sel.event_day.unwrap_or(ev.start.date);
    let sign = if later { 1 } else { -1 };
    let (at, length) = (i64::from(ev.start.minutes_of_day()), ev.duration_minutes());
    let (params, lands, shows) = if app.session.view.calendar == CalView::Month {
        if end_only {
            return;
        }
        // The same day of the week before, or after.
        let (start, end) = (ev.start.add_days(7 * sign), ev.end.add_days(7 * sign));
        let mut params = if ev.all_day {
            json!({"id": ev.id, "start": start.date.iso(), "end": last_day(start, end).iso()})
        } else {
            json!({"id": ev.id, "start": iso_minute(start)})
        };
        alone(&mut params, &ev, day, day.add_days(7 * sign));
        (params, day.add_days(7 * sign), None)
    } else if ev.all_day || length >= DAY_MINUTES {
        // Whole days have no row of the hours to go to.
        return;
    } else if end_only {
        // Longer or shorter by a row; never shorter than dragging its edge could make it.
        let length = length + sign * scale.slot;
        if length < scale.step() || at + length > 2 * DAY_MINUTES {
            return;
        }
        let params = json!({"id": ev.id, "start": iso_minute(ev.start), "end": iso_minute(ev.start.add_minutes(length))});
        (params, day, Some((at, (at + length).min(DAY_MINUTES))))
    } else {
        // Up or down a row, as far as its day goes.
        let to = at + sign * scale.slot;
        if to < 0 || to > DAY_MINUTES - scale.step() {
            return;
        }
        (json!({"id": ev.id, "start": iso_minute(ev.start.add_minutes(sign * scale.slot))}), day, Some((to, (to + length).min(DAY_MINUTES))))
    };
    // An item that repeats, somebody else's meeting, one with people invited: asked about first.
    if let Ok(answer) = app.run("calendar.move", params)
        && answer.get("pending").is_none()
    {
        let now = answer.get("id").and_then(Value::as_u64).unwrap_or(ev.id);
        let _ = app.execute("calendar.select", json!({"id": now, "day": lands.iso()}));
        frame.reveal = shows;
    }
}

/// Ctrl+C and Ctrl+X: remember the selected event for Paste. Its title goes on the system's
/// clipboard, as text, so that a Paste comes at all and can be told from one of something
/// else. A cut event stays where it is until it is pasted, which then moves it.
fn clip(app: &mut MailApp, ctx: &egui::Context, cut: bool) {
    let Some(ev) = app.session.sel.event.and_then(|id| app.session.mailbox.events.get(&id).cloned()) else { return };
    let day = app.session.sel.event_day.unwrap_or(ev.start.date);
    let title = clip_text(ev.title.trim(), 200);
    let text = if title.is_empty() { tl!("(No title)").to_string() } else { title.to_string() };
    ctx.copy_text(text.clone());
    let start = to_minute(DateTime { date: day, ..ev.start });
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(CLIP_ID), Clip { event: ev.id, start, cut, text }));
}

/// Ctrl+V: put the event that was copied (a copy of it) or cut (the event itself) where a time
/// is picked: at that time, as long as it was; on a picked day, at its own time of day; whole
/// days on the day that is picked. `text` is what the system's clipboard holds, when the Paste
/// came with it.
fn paste(app: &mut MailApp, ctx: &egui::Context, text: Option<&str>) {
    let id = egui::Id::unique(CLIP_ID);
    let Some(clip) = ctx.data(|d| d.get_temp::<Clip>(id)) else { return };
    // Something else was copied since: the clipboard isn't the calendar's any more.
    if text.is_some_and(|text| text.trim() != clip.text.trim()) {
        ctx.data_mut(|d| d.remove::<Clip>(id));
        return;
    }
    // One Paste can come twice, as the key and as the clipboard's text: once is enough.
    let (now, guard) = (ctx.input(|i| i.time), egui::Id::unique((CLIP_ID, "pasted")));
    if ctx.data(|d| d.get_temp::<f64>(guard)).is_some_and(|at| (now - at).abs() < 0.4) {
        return;
    }
    let Some(ev) = app.session.mailbox.events.get(&clip.event).cloned() else {
        ctx.data_mut(|d| d.remove::<Clip>(id));
        return;
    };
    let Some(slot) = load_slot(ctx).filter(|slot| slot.text.is_none()) else {
        app.status(tl!("Pick a time in the calendar first, then paste"));
        return;
    };
    ctx.data_mut(|d| d.insert_temp(guard, now));
    let start = if ev.all_day {
        DateTime::at_midnight(slot.start.date)
    } else if slot.all_day {
        to_minute(DateTime { date: slot.start.date, ..ev.start })
    } else {
        slot.start
    };
    // A time picked in another calendar's block: the event goes into that calendar, or its copy.
    let into = other_block(app, &ev, &slot);
    if clip.cut {
        if into.is_some() && ev.recurrence.is_some() {
            app.status(series_stays());
            return;
        }
        // Cut and paste is a move: a series goes as a whole, by as much as the occurrence
        // that was cut does.
        let by = clip.start.minutes_until(start);
        let (to, end) = (ev.start.add_minutes(by), ev.end.add_minutes(by));
        let mut params = if ev.all_day {
            json!({"id": ev.id, "start": to.date.iso(), "end": last_day(to, end).iso()})
        } else {
            json!({"id": ev.id, "start": iso_minute(to)})
        };
        alone(&mut params, &ev, clip.start.date, start.date);
        let command = into_calendar(&mut params, into, "calendar.move");
        // It is the occurrence that was cut that moves: of a series, the owner is asked whether
        // that one or all of them.
        let _ = app.execute("calendar.select", json!({"id": ev.id, "day": clip.start.date.iso()}));
        if let Ok(answer) = app.run(command, params)
            && answer.get("pending").is_none()
        {
            let now = answer.get("id").and_then(Value::as_u64).unwrap_or(ev.id);
            let _ = app.execute("calendar.select", json!({"id": now, "day": start.date.iso()}));
        }
        ctx.data_mut(|d| d.remove::<Clip>(id));
    } else {
        let mut params = if ev.all_day {
            json!({"id": ev.id, "start": start.date.iso(), "allDay": true})
        } else {
            json!({"id": ev.id, "start": iso_minute(start), "allDay": false})
        };
        into_calendar(&mut params, into, "calendar.copy");
        let _ = app.run("calendar.copy", params);
    }
}

/// The command that does what `command` with `params` would and also puts the event into
/// `calendar`, when there is one to put it into: a move is then an edit, which can say both
/// when and where (the engine refuses what can't change calendars, and says why).
fn into_calendar(params: &mut Value, calendar: Option<Id>, command: &'static str) -> &'static str {
    let (Some(calendar), Some(fields)) = (calendar, params.as_object_mut()) else { return command };
    fields.insert("calendar".into(), json!(calendar));
    if command == "calendar.move" { "calendar.edit" } else { command }
}

// ---- day, work week, week ----

/// What the pieces of the time grid share.
struct Grid<'a> {
    t: Tokens,
    /// The blocks of calendars side by side, each with a column for every day.
    blocks: Blocks,
    from: Date,
    days: &'a [Date],
    items: &'a [Item],
    today: Date,
    twelve: bool,
    scale: Scale,
    panes: &'a [PaneView],
}

impl Grid<'_> {
    /// The calendar in front in block `pane`: where what is picked there belongs.
    fn front(&self, pane: usize) -> Id {
        self.panes.get(pane).map_or(0, |pane| pane.front)
    }
}

/// A whole-day or multi-day event in the all-day row.
struct Bar {
    item: usize,
    first: usize,
    last: usize,
    /// It begins, and finishes, on a day in view.
    starts: bool,
    ends: bool,
    lane: usize,
}

/// The part of a timed occurrence that falls on one day, in minutes of that day.
#[derive(Clone, Copy)]
struct Seg {
    item: usize,
    start: i64,
    end: i64,
}

/// The calendar in front in each block, as the parts of the calendar remember it for a frame.
fn fronts(panes: &[PaneView]) -> [Id; MAX_PANES] {
    let mut fronts = [0; MAX_PANES];
    for (front, pane) in fronts.iter_mut().zip(panes) {
        *front = pane.front;
    }
    fronts
}

fn time_grid(app: &mut MailApp, ui: &mut Ui, frame: &mut Frame, from: Date, to: Date, items: &[Item], panes: &[PaneView]) {
    let t = Tokens::get(ui.ctx());
    let days = days_of(from, to, i64::from(MAX_DAYS));
    let n = days.len();
    let full = ui.available_rect_before_wrap();
    // (A second time zone has a column of its own in the time bar, left of the first.)
    let gutter = GUTTER_W + second_zone_width(app);
    let blocks = Blocks::of(full.min.x + gutter, full.width() - gutter, n, panes.len());
    let count = blocks.count;
    let scale = Scale::of(time_scale(&app.session));
    let g = Grid { t, blocks, from, days: &days, items, today: app.session.today(), twelve: app.session.prefs.twelve_hour, scale, panes };
    frame.zones.from = from;
    frame.zones.fronts = fronts(panes);

    // Whole-day and multi-day events go in the all-day row, the rest in their days' columns:
    // each in the block of its calendar.
    let mut bars: Vec<Vec<Bar>> = (0..count).map(|_| Vec::new()).collect();
    let mut segs: Vec<Vec<Vec<Seg>>> = (0..count).map(|_| vec![Vec::new(); n]).collect();
    let mut hidden: Vec<Vec<usize>> = vec![vec![0usize; n]; count];
    let last_col = i64::try_from(n).unwrap_or(1) - 1;
    for (i, item) in items.iter().enumerate() {
        let (first, last) = (from.days_until(item.occ.start.date), from.days_until(item.last_day()));
        if last < 0 || first > last_col {
            continue;
        }
        let pane = item.pane.min(count.saturating_sub(1));
        let (Some(bars), Some(segs), Some(hidden)) = (bars.get_mut(pane), segs.get_mut(pane), hidden.get_mut(pane)) else { continue };
        let (lo, hi) = (to_index(first.max(0)), to_index(last.min(last_col)));
        if item.is_bar() {
            bars.push(Bar { item: i, first: lo, last: hi, starts: first >= 0, ends: last <= last_col, lane: usize::MAX });
            continue;
        }
        for d in lo..=hi {
            let (Some(day), Some(list), Some(more)) = (days.get(d), segs.get_mut(d), hidden.get_mut(d)) else { continue };
            if list.len() >= MAX_BLOCKS {
                *more += 1;
                continue;
            }
            let start = if item.occ.start.date == *day { i64::from(item.occ.start.minutes_of_day()) } else { 0 };
            let end = match item.occ.end.date.cmp(day) {
                std::cmp::Ordering::Equal => i64::from(item.occ.end.minutes_of_day()),
                std::cmp::Ordering::Greater => DAY_MINUTES,
                std::cmp::Ordering::Less => start,
            };
            list.push(Seg { item: i, start, end: end.max(start) });
        }
    }
    // Earlier and longer bars take the upper lanes; what doesn't fit is counted as `+n more`.
    let lane_cap = if n == 1 { 8 } else { 4 };
    for (bars, hidden) in bars.iter_mut().zip(hidden.iter_mut()) {
        bars.sort_by_key(|b| (b.first, std::cmp::Reverse(b.last), b.item));
        let lanes = assign_lanes(&bars.iter().take(MAX_BARS).map(|b| (b.first, b.last)).collect::<Vec<_>>());
        for (i, bar) in bars.iter_mut().enumerate() {
            bar.lane = lanes.get(i).copied().unwrap_or(usize::MAX);
            if bar.lane >= lane_cap {
                for more in hidden.iter_mut().take(bar.last + 1).skip(bar.first) {
                    *more += 1;
                }
            }
        }
        bars.retain(|b| b.lane < lane_cap);
    }

    if tabbed(panes) {
        tab_strip(app, ui, &blocks, panes);
    }
    let heads = day_headers(ui, &g);
    zone_labels(app, ui, &g, Rect::from_min_max(pos2(full.min.x, heads.min.y), pos2(blocks.left(), heads.max.y)));
    all_day_row(app, ui, &g, frame, &bars, &hidden);
    hour_grid(app, ui, &g, frame, &mut segs);
}

/// How wide the second time zone's column of the time bar is; nothing without one.
const SECOND_ZONE_W: f32 = 52.0;

fn second_zone_width(app: &MailApp) -> f32 {
    if mailcraft_engine::cmd::zones::second_zone(&app.session).is_some() { SECOND_ZONE_W } else { 0.0 }
}

/// The instant the time bar's zones are compared at: noon of the first day in view (a week in
/// January is labelled as the zones are in January).
fn bar_instant(app: &MailApp, g: &Grid) -> mailcraft_time::Timestamp {
    let noon = DateTime { hour: 12, ..DateTime::at_midnight(g.from) };
    mailcraft_model::zoned::instant_at(noon, &|instant| app.session.offset_at(instant))
}

/// What the time bar says over its hours (`area`: the bar's part of the days' headings): the
/// zone the calendar is shown in, when that is worth saying (the owner chose it, gave it a
/// name, or has a second zone beside it), and the second zone over its own column. Hovering one
/// says which zone it is.
fn zone_labels(app: &mut MailApp, ui: &mut Ui, g: &Grid, area: Rect) {
    let t = g.t;
    let prefs = &app.session.prefs;
    let second = mailcraft_engine::cmd::zones::second_zone(&app.session);
    let shown = app.session.zone();
    let worth = second.is_some() || !prefs.zone_label.trim().is_empty() || mailcraft_time::Zone::named(&prefs.time_zone).is_some();
    let at = bar_instant(app, g);
    let split = area.min.x + if second.is_some() { SECOND_ZONE_W } else { 0.0 };
    let mut label = |zone: mailcraft_time::Zone, given: &str, cell: Rect, key: u64| {
        let text = crate::zones::bar_label(zone, given, at);
        let p = ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
        p.text(pos2(cell.max.x - 8.0, cell.max.y - 6.0), Align2::RIGHT_BOTTOM, clip_text(&text, 16), regular(10.5), t.text_dim);
        app.hit.add("zoneLabel", key, cell);
        ui.interact(cell, ui.scope_id().with(("zone_label", key)), Sense::hover()).on_hover_text(crate::zones::row(zone, at));
    };
    if let Some(second) = second {
        label(second, &prefs.second_zone_label, Rect::from_min_max(area.min, pos2(split, area.max.y)), 2);
    }
    if worth && !shown.is_local() {
        label(shown, &prefs.zone_label, Rect::from_min_max(pos2(split, area.min.y), area.max), 1);
    }
}

/// The tabs over the calendars, when more than one is shown: over each block the name of its
/// calendar on a tab in its colour (over calendars that lie on one another, a tab for each, the
/// one in front the strongest), with a line in that colour under them. A click on a tab makes
/// its calendar the one being worked in, and brings it to the front of those it lies with; the
/// arrow on it lays the calendar over the others, or stands it beside them again; the cross
/// hides it.
fn tab_strip(app: &mut MailApp, ui: &mut Ui, blocks: &Blocks, panes: &[PaneView]) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), TAB_H), Sense::hover());
    let p = ui.painter().clone();
    let current = mailcraft_engine::cmd::calendar::current_calendar(&app.session);
    let stacked = panes.first().is_some_and(|pane| pane.tabs.iter().any(|tab| tab.overlaid));
    let only_here = local_only(app);
    for (k, pane) in panes.iter().enumerate().take(blocks.count) {
        let cols = blocks.cols(k);
        let (left, right) = (cols.left, cols.right());
        let Some(front) = pane.tabs.iter().find(|tab| tab.id == pane.front).or(pane.tabs.first()) else { continue };
        p.rect_filled(Rect::from_min_max(pos2(left, r.max.y - 3.0), pos2(right, r.max.y)), 0.0, front.color);
        let room = (right - left - 2.0).max(0.0);
        let each = room / pane.tabs.len().max(1) as f32;
        for (i, tab) in pane.tabs.iter().enumerate() {
            // Where the whole block is: what is in a calendar's block is found by it.
            app.hit.add("pane", tab.id, Rect::from_min_max(pos2(left, r.min.y), pos2(right, r.max.y)));
            if each < 6.0 {
                continue;
            }
            // A calendar that is on this computer only says so on its block: what is drawn there
            // is not in the Outlook calendar it stands beside.
            let named = if only_here.contains(&tab.id) {
                let plain = tab.name.trim();
                let line = tl!(LOCAL_ONLY);
                if plain.eq_ignore_ascii_case(mailcraft_engine::cmd::remote::LOCAL_CALENDAR) {
                    line.to_string()
                } else {
                    format!("{plain} · {line}")
                }
            } else {
                tab.name.trim().to_string()
            };
            let name = clip_text(&named, 120);
            // It can be laid over the others when there is one to its left to lie on; one that
            // lies there already can be stood beside them again.
            let arrow = if tab.overlaid {
                Some(false)
            } else if k > 0 || stacked {
                Some(true)
            } else {
                None
            };
            // A calendar by itself has a tab as wide as its name needs; those that lie over one
            // another share the block's width.
            let wanted = text_w(ui, name, medium(12.0)) + 38.0 + if arrow.is_some() { 20.0 } else { 0.0 };
            let width = if pane.tabs.len() == 1 { wanted.min(room) } else { each - 2.0 };
            let x0 = left + 1.0 + i as f32 * each;
            let rect = Rect::from_min_max(pos2(x0, r.min.y + 3.0), pos2(x0 + width.max(4.0), r.max.y - 3.0));
            let is_front = tab.id == front.id;
            let resp = ui.interact(rect, ui.scope_id().with(("pane_tab", tab.id)), Sense::click());
            app.hit.add("paneTab", tab.id, rect);
            let strength = match (is_front, resp.hovered()) {
                (true, _) => 0.62,
                (false, true) => 0.36,
                (false, false) => 0.2,
            };
            let fill = mix(t.surface, tab.color, strength);
            p.rect_filled(rect, CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 }, fill);
            let ink = if is_front { ink_on(fill, &t) } else { t.text_dim };
            let (mut text_left, mut text_right) = (rect.min.x + 8.0, rect.max.x - 6.0);
            let roomy = rect.width() >= MIN_TAB_W + 44.0;
            if let (Some(over), true) = (arrow, roomy) {
                let at = Rect::from_center_size(pos2(rect.min.x + 12.0, rect.center().y), vec2(16.0, 16.0));
                let arrow = ui.interact(at, ui.scope_id().with(("pane_arrow", tab.id)), Sense::click());
                app.hit.add("paneArrow", tab.id, at);
                if arrow.hovered() {
                    p.rect_filled(at, 3.0, mix(fill, ink, 0.2));
                }
                icons::paint(&p, at.shrink(3.0), if over { "previous" } else { "next" }, ink, ink);
                let tip = if over { tl!("Lay this calendar over the others") } else { tl!("Stand this calendar beside the others") };
                if arrow.on_hover_text(tip).clicked() {
                    let _ = app.run("view.overlay", json!({"calendar": tab.id, "value": over}));
                }
                text_left = at.max.x + 3.0;
            }
            if roomy {
                let at = Rect::from_center_size(pos2(rect.max.x - 11.0, rect.center().y), vec2(16.0, 16.0));
                let close = ui.interact(at, ui.scope_id().with(("pane_close", tab.id)), Sense::click());
                app.hit.add("paneClose", tab.id, at);
                if close.hovered() {
                    p.rect_filled(at, 3.0, mix(fill, ink, 0.2));
                }
                let (c, d) = (at.center(), 3.5);
                p.line_segment([pos2(c.x - d, c.y - d), pos2(c.x + d, c.y + d)], Stroke::new(1.4, ink));
                p.line_segment([pos2(c.x - d, c.y + d), pos2(c.x + d, c.y - d)], Stroke::new(1.4, ink));
                if close.on_hover_text(tl!("Hide this calendar")).clicked() {
                    let _ = app.run("calendar.showCalendar", json!({"id": tab.id, "value": false}));
                }
                text_right = at.min.x - 3.0;
            }
            // The calendar being worked in has its name the heavier.
            let font = if tab.id == current { semibold(12.0) } else { medium(12.0) };
            paint_clipped(&p, pos2(text_left, rect.center().y), text_right - text_left, name, font, ink);
            let tip = if tab.read_only {
                crate::i18n::fmt(tl!("{calendar} · This calendar can only be read"), &[("calendar", name)])
            } else {
                name.to_string()
            };
            if resp.on_hover_text(tip).clicked() {
                let _ = app.run("view.frontCalendar", json!({"calendar": tab.id}));
            }
        }
    }
}

/// The weekday and the day number over each column, in every block; today's stands out.
fn day_headers(ui: &mut Ui, g: &Grid) -> Rect {
    let t = g.t;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), DAY_HEAD_H), Sense::hover());
    for k in 0..g.blocks.count {
        let cols = g.blocks.cols(k);
        for (i, day) in g.days.iter().enumerate() {
            let cell = cols.span(i, i, r.min.y, r.max.y);
            let p = ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
            let is_today = *day == g.today;
            if is_today {
                p.rect_filled(cell, 0.0, t.today);
                p.rect_filled(Rect::from_min_max(cell.min, pos2(cell.max.x, cell.min.y + 3.0)), 0.0, t.accent);
            }
            // (Columns that many calendars leave narrow have the day in smaller figures.)
            let name = if cell.width() >= 96.0 { day.weekday().name() } else { day.weekday().short() };
            let figures = if cell.width() >= 44.0 { 20.0 } else { 13.0 };
            let inset = if cell.width() >= 44.0 { 10.0 } else { 4.0 };
            let (name_font, number_font) = if is_today { (semibold(11.5), semibold(figures)) } else { (regular(11.5), regular(figures)) };
            paint_clipped(
                &p,
                pos2(cell.min.x + inset, cell.min.y + 16.0),
                cell.width() - inset - 4.0,
                tl!(name),
                name_font,
                if is_today { t.accent_text } else { t.text_dim },
            );
            p.text(
                pos2(cell.min.x + inset, cell.min.y + 36.0),
                Align2::LEFT_CENTER,
                day.day.to_string(),
                number_font,
                if is_today { t.accent_text } else { t.text },
            );
            ui.painter().vline(cell.min.x, r.y_range(), Stroke::new(1.0, t.border));
        }
        if g.blocks.count > 1 {
            ui.painter().vline(cols.right(), r.y_range(), Stroke::new(1.0, t.border));
        }
    }
    r
}

/// The row under the headers with whole-day and multi-day events as bars across their days, in
/// every block its calendar's.
fn all_day_row(app: &mut MailApp, ui: &mut Ui, g: &Grid, frame: &mut Frame, bars: &[Vec<Bar>], hidden: &[Vec<usize>]) {
    let t = g.t;
    let lanes = bars.iter().flatten().map(|b| b.lane + 1).max().unwrap_or(0).max(1);
    // In the gutter, on two lines when the interface language's words are longer than it is wide.
    let mut job = egui::text::LayoutJob::simple(tl!("All day").to_string(), regular(10.5), t.text_dim, GUTTER_W - 12.0);
    job.halign = egui::Align::RIGHT;
    let label = ui.painter().layout_job(job);
    let bars_h = (lanes as f32 * (BAR_H + BAR_GAP) + 3.0).max(label.size().y + 4.0);
    let more_h = if hidden.iter().flatten().any(|n| *n > 0) { 17.0 } else { 0.0 };
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), bars_h + more_h), Sense::hover());
    frame.zones.all_day = Some((g.blocks, r));
    let p = ui.painter().clone();
    let y = if label.rows.len() > 1 { r.min.y + 1.0 } else { r.min.y + 3.0 + BAR_H / 2.0 - label.size().y / 2.0 };
    p.galley(pos2(g.blocks.left() - 8.0, y), label, t.text_dim);
    // The pick of whole days, when there is one on the days in view: a click on a day here makes
    // it, and a drag along the row picks the days it passes, in the block it began in.
    let was = load_slot(ui.ctx());
    let in_view = |s: &Slot| g.days.first().is_some_and(|first| s.last() >= *first) && g.days.last().is_some_and(|last| s.start.date <= *last);
    let mut slot = was.clone().filter(|s| s.all_day && in_view(s));
    let mut entry_at: Option<Rect> = None;
    // The days first, so the bars over them take the clicks.
    for k in 0..g.blocks.count {
        let cols = g.blocks.cols(k);
        let front = g.front(k);
        let day_at = |pos: egui::Pos2| g.days.get(cols.index_at(pos.x)).copied().unwrap_or(g.from);
        for (i, day) in g.days.iter().enumerate() {
            let cell = cols.span(i, i, r.min.y, r.max.y);
            if *day == g.today {
                p.rect_filled(cell, 0.0, t.today);
            }
            let here = |s: &&Slot| pane_of(g.panes, s.calendar) == k && (s.start.date..=s.last()).contains(day);
            if let Some(picked) = slot.as_ref().filter(here) {
                p.rect_filled(cell.shrink2(vec2(1.0, 1.0)), 0.0, t.selection);
                if entry_at.is_none() {
                    entry_at = Some(Rect::from_min_max(pos2(cell.min.x + 3.0, r.min.y + 3.0), pos2(cell.max.x - 3.0, r.min.y + 3.0 + BAR_H)));
                    if let Ok(id) = u64::try_from(picked.start.date.to_days()) {
                        app.hit.add("slot", id, cell);
                    }
                }
            }
            p.vline(cell.min.x, r.y_range(), Stroke::new(1.0, t.border));
            let resp = ui.interact(cell, ui.scope_id().with(("all_day", k, i)), Sense::click_and_drag());
            if let Ok(id) = u64::try_from(day.to_days()) {
                app.hit.add("allDay", id, cell);
            }
            empty_input(app, &resp, &mut slot, || Slot::day(*day).in_calendar(front));
            if resp.drag_started_by(egui::PointerButton::Primary) {
                let _ = app.execute("calendar.select", json!({"id": null}));
                work_in(app, front);
                slot = Some(Slot::day(*day).in_calendar(front));
            }
            if resp.dragged_by(egui::PointerButton::Primary)
                && let (Some(stretched), Some(pos)) = (slot.as_mut().filter(|s| s.text.is_none()), frame.pointer)
            {
                stretched.stretch_to_day(day_at(pos));
            }
        }
        if g.blocks.count > 1 {
            p.vline(cols.right(), r.y_range(), Stroke::new(1.0, t.border));
        }
    }
    let under = |pos: egui::Pos2| Under::Day(g.days.get(g.blocks.at(pos.x).index_at(pos.x)).copied().unwrap_or(g.from));
    for (k, bars) in bars.iter().enumerate().take(g.blocks.count) {
        let cols = g.blocks.cols(k);
        for bar in bars {
            let Some(item) = g.items.get(bar.item) else { continue };
            let top = r.min.y + 3.0 + bar.lane as f32 * (BAR_H + BAR_GAP);
            let span = cols.span(bar.first, bar.last, top, top + BAR_H);
            let rect = Rect::from_min_max(
                pos2(span.min.x + if bar.starts { 3.0 } else { 0.0 }, span.min.y),
                pos2(span.max.x - if bar.ends { 3.0 } else { 0.0 }, span.max.y),
            );
            let resp = ui.interact(rect, ui.scope_id().with(("bar", item.ev.id, item.occ.start.date.to_days())), Sense::click_and_drag());
            app.hit.add("event", item.ev.id, rect);
            // Dragging moves it by days, its ends change its first and last day, and down into
            // the hours it becomes an appointment there.
            frame.track(ui, &resp, item, rect, Piece { bar: true, starts: bar.starts, ends: bar.ends }, &under);
            let dragging = frame.drag.is_some();
            paint_bar(&p, rect, item, &t, g.twelve, bar.starts, bar.ends, is_selected(app, item), resp.hovered() && !dragging);
            if frame.fades(item) {
                p.rect_filled(rect, 3.0, t.surface.linear_multiply(0.55));
            }
            event_input(app, resp, item, !dragging);
        }
    }
    if more_h > 0.0 {
        for (k, hidden) in hidden.iter().enumerate().take(g.blocks.count) {
            let cols = g.blocks.cols(k);
            for (i, count) in hidden.iter().enumerate().filter(|(_, n)| **n > 0) {
                let Some(day) = g.days.get(i) else { continue };
                let cell = cols.span(i, i, r.max.y - more_h, r.max.y - 2.0);
                more_link(app, ui, cell.shrink2(vec2(3.0, 0.0)), ("all_day_more", k * 100 + i), *count, *day);
            }
        }
    }
    // What is typed with days picked out starts a whole-day entry, over the bars.
    if let (Some(at), Some(picked)) = (entry_at, slot.as_mut()) {
        let entry = slot_entry(app, ui, at, picked, false);
        finish_entry(app, &mut slot, entry);
    }
    keep_slot(ui.ctx(), &was, slot, |s| s.all_day);
    p.hline(r.x_range(), r.max.y - 0.5, Stroke::new(1.0, t.border_strong));
}

/// The 24 hours, scrollable: shading for working hours, the lines, the now line and the blocks,
/// in every block of calendars its own.
fn hour_grid(app: &mut MailApp, ui: &mut Ui, g: &Grid, frame: &mut Frame, segs: &mut [Vec<Vec<Seg>>]) {
    let t = g.t;
    let (blocks, scale) = (g.blocks, g.scale);
    let now = app.session.local_now();
    // The working hours in effect: the mailbox's own, or the ones of the local options.
    let work = calendar_settings::work_hours(&app.session);
    let (work_start, work_end) = (work.start.clamp(0, DAY_MINUTES), work.end.clamp(0, DAY_MINUTES));
    // The second time zone of the time bar: how many minutes its clock is ahead of the calendar's.
    let ahead = mailcraft_engine::cmd::zones::second_zone(&app.session).map(|zone| {
        let at = bar_instant(app, g);
        i64::from(zone.offset_at(at)) - i64::from(app.session.offset_at(at))
    });

    // The first time, and when the view or the time scale changes, the working day starts near
    // the top.
    let view = (app.session.view.calendar, scale.slot);
    let memory = egui::Id::unique(GRID_VIEW_ID);
    let shown: Option<(CalView, i64)> = ui.ctx().data(|d| d.get_temp(memory));
    ui.ctx().data_mut(|d| d.insert_temp(memory, view));
    let mut area = egui::ScrollArea::vertical().id_salt("calendar_hours").auto_shrink([false, false]);
    if shown != Some(view) {
        area = area.vertical_scroll_offset(scale.y((work_start - 60).max(0)));
    }
    let reveal = frame.reveal.take();
    area.show(ui, |ui| {
        let (rect, background) = ui.allocate_exact_size(vec2(ui.available_width(), scale.height()), Sense::click_and_drag());
        let top = rect.min.y;
        let seen = ui.clip_rect();
        frame.zones.hours = Some(Hours { cols: blocks, top, seen, scale });
        let p = ui.painter().clone();

        // A line for every row of the scale, the hours' own stronger, and only those in view (a
        // day at five minutes a row is 288 of them).
        let rows = (DAY_MINUTES / scale.slot.max(1)).clamp(24, 24 * 60);
        let row_h = scale.height() / rows as f32;
        let first = to_index((((seen.min.y - top) / row_h).floor() as i64).clamp(0, rows));
        let last = to_index((((seen.max.y - top) / row_h).ceil() as i64).clamp(0, rows));
        let faint = Stroke::new(1.0, t.border.linear_multiply(0.45));
        let today_at = g.days.iter().position(|d| *d == g.today);
        let now_y = top + now.seconds_of_day() as f32 / 3600.0 * scale.hour_h;
        for k in 0..blocks.count {
            let cols = blocks.cols(k);
            let (left, right) = (cols.left, cols.right());
            for (i, day) in g.days.iter().enumerate() {
                let col = cols.span(i, i, top, rect.max.y);
                let (lit, off) = if *day == g.today { (t.today, mix(t.off_hours, t.today, 0.6)) } else { (t.surface, t.off_hours) };
                p.rect_filled(col, 0.0, off);
                if work.days.contains(&day.weekday()) && work_end > work_start {
                    let hours = Rect::from_min_max(pos2(col.min.x, top + scale.y(work_start)), pos2(col.max.x, top + scale.y(work_end)));
                    p.rect_filled(hours, 0.0, lit);
                }
            }
            for row in first..=last {
                let minute = i64::try_from(row).unwrap_or(0).saturating_mul(scale.slot);
                let y = top + scale.y(minute);
                if minute % 60 != 0 {
                    p.hline(egui::Rangef::new(left, right), y, faint);
                    continue;
                }
                // The hours are said once, in the time bar left of the first block.
                p.hline(egui::Rangef::new(if k == 0 { left - 6.0 } else { left }, right), y, Stroke::new(1.0, t.border));
                if k == 0
                    && let Ok(hour) = u8::try_from(minute / 60)
                    && hour < 24
                {
                    paint_hour(&p, pos2(left - 8.0, y + 2.0), hour, g.twelve, &t);
                    // The same moment by the second zone's clock, in its own column.
                    if let Some(ahead) = ahead {
                        paint_clock(&p, pos2(rect.min.x + SECOND_ZONE_W - 8.0, y + 2.0), (minute + ahead).rem_euclid(DAY_MINUTES), g.twelve, &t);
                    }
                }
            }
            if k == 0 && ahead.is_some() {
                p.vline(rect.min.x + SECOND_ZONE_W, rect.y_range(), Stroke::new(1.0, t.border.linear_multiply(0.6)));
            }
            // At an hour a row, the half hours still show, faintly.
            if scale.slot >= 60 {
                for row in first..last {
                    p.hline(egui::Rangef::new(left, right), top + (row as f32 + 0.5) * row_h, faint);
                }
            }
            for i in 0..=cols.n {
                p.vline(cols.x(i), rect.y_range(), Stroke::new(1.0, t.border));
            }
            // Now: a line across today's column, under the blocks, and its mark in the time bar.
            if let Some(i) = today_at {
                p.hline(egui::Rangef::new(cols.x(i), cols.x(i + 1)), now_y, Stroke::new(2.0, t.accent));
                if k == 0 {
                    p.hline(egui::Rangef::new(rect.min.x + 6.0, left - 6.0), now_y, Stroke::new(2.0, t.accent));
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs(30));
                }
            }
        }

        // The time bar, left of the days: its menu (the right button) changes the time scale.
        let bar = Rect::from_min_max(pos2(rect.min.x, seen.min.y.max(top)), pos2(blocks.left() - 1.0, seen.max.y.min(rect.max.y)));
        if bar.is_positive() {
            let resp = ui.interact(bar, ui.scope_id().with("time_bar"), Sense::click());
            app.hit.add("timeBar", 0, bar);
            resp.context_menu(|ui| {
                ui.set_min_width(200.0);
                time_scale_menu(ui, app);
            });
        }

        // Empty grid: a click picks the row under it out, a drag picks a stretch of them, a
        // double click starts an appointment there, in the block's calendar. What is typed then
        // goes into the pick.
        let pointer = frame.pointer;
        let day_at = |pos: egui::Pos2| g.days.get(blocks.at(pos.x).index_at(pos.x)).copied().unwrap_or(g.from);
        let slot_under =
            |pos: egui::Pos2| Slot::timed(day_at(pos), scale.minute(pos.y - top), scale.slot).in_calendar(g.front(blocks.block_at(pos.x)));
        let was = load_slot(ui.ctx());
        let mut slot = was.clone().filter(|s| !s.all_day && g.days.contains(&s.start.date));
        empty_input(app, &background, &mut slot, || slot_under(pointer.unwrap_or(rect.center())));
        if background.drag_started_by(egui::PointerButton::Primary) {
            let press = ui.input(|i| i.pointer.press_origin()).or(pointer).unwrap_or(rect.center());
            let _ = app.execute("calendar.select", json!({"id": null}));
            let picked = slot_under(press);
            work_in(app, picked.calendar);
            slot = Some(picked);
        }
        if background.dragged_by(egui::PointerButton::Primary)
            && let (Some(picked), Some(pos)) = (slot.as_mut().filter(|s| s.text.is_none()), pointer)
        {
            picked.stretch_to(scale.minute(pos.y - top));
        }
        // Its mark goes under the blocks; the field that is typed into, over them (below).
        let picked_at = slot.as_ref().and_then(|picked| {
            let d = g.days.iter().position(|day| *day == picked.start.date)?;
            let cols = blocks.cols(pane_of(g.panes, picked.calendar));
            let (from, to) = picked.minutes();
            Some(Rect::from_min_max(pos2(cols.x(d) + 1.0, top + scale.y(from) + 1.0), pos2(cols.x(d + 1) - 2.0, top + scale.y(to) - 1.0)))
        });
        if let (Some(at), Some(picked)) = (picked_at, slot.as_ref()) {
            p.rect_filled(at, 2.0, t.selection);
            p.rect_stroke(at, 2.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
            if let Ok(id) = u64::try_from(picked.start.date.to_days()) {
                app.hit.add("slot", id, at);
            }
        }

        let under = |pos: egui::Pos2| Under::Time { day: day_at(pos), minute: scale.minute(pos.y - top) };
        let min_minutes = (MIN_BLOCK_H / scale.hour_h * 60.0).ceil() as i64;
        for (k, segs) in segs.iter_mut().enumerate().take(blocks.count) {
            let cols = blocks.cols(k);
            for (d, list) in segs.iter_mut().enumerate() {
                let Some(day) = g.days.get(d) else { continue };
                // Blocks that overlap share the day's width, in columns.
                list.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end), s.item));
                let packed = pack_columns(&list.iter().map(|s| (s.start, s.end.max(s.start + min_minutes))).collect::<Vec<_>>());
                // A strip on the right stays free, so there is always empty grid to click.
                let (x0, x1) = (cols.x(d) + 1.0, cols.x(d + 1) - if cols.width >= 70.0 { 10.0 } else { 2.0 });
                for (seg, (column, columns)) in list.iter().zip(packed) {
                    let Some(item) = g.items.get(seg.item) else { continue };
                    let w = (x1 - x0) / columns.max(1) as f32;
                    let y0 = top + scale.y(seg.start);
                    let y1 = (top + scale.y(seg.end)).max(y0 + MIN_BLOCK_H);
                    let block = Rect::from_min_max(pos2(x0 + column as f32 * w, y0 + 1.0), pos2(x0 + (column + 1) as f32 * w - 1.0, y1 - 1.0));
                    let resp =
                        ui.interact(block, ui.scope_id().with(("block", item.ev.id, item.occ.start.date.to_days(), d)), Sense::click_and_drag());
                    // Dragging moves the event: to another time down the column, to another day
                    // across, up into the row of whole days, into another calendar's block. Its
                    // upper and lower edge, on the days it starts and ends, change when it does.
                    let piece = Piece { bar: false, starts: item.occ.start.date == *day, ends: item.last_day() == *day };
                    frame.track(ui, &resp, item, block, piece, &under);
                    let dragging = frame.drag.is_some();
                    if block.intersects(seen) {
                        app.hit.add("event", item.ev.id, block.intersect(seen));
                        paint_block(&p, block, item, &t, g.twelve, is_selected(app, item), resp.hovered() && !dragging);
                        if frame.fades(item) {
                            // The block fades where it was while its ghost shows where it is going.
                            p.rect_filled(block, 3.0, t.surface.linear_multiply(0.55));
                        }
                    }
                    event_input(app, resp, item, !dragging);
                }
            }
        }
        // What is typed with a slot picked out starts an entry in it.
        if let (Some(at), Some(picked)) = (picked_at, slot.as_mut()) {
            let entry = slot_entry(app, ui, at, picked, false);
            finish_entry(app, &mut slot, entry);
        }
        keep_slot(ui.ctx(), &was, slot, |s| !s.all_day);

        // The keyboard moved the pick: the hours follow it.
        if let Some((from, to)) = reveal {
            let rows = Rect::from_min_max(pos2(rect.min.x, top + scale.y(from) - 2.0), pos2(rect.max.x, top + scale.y(to) + 2.0));
            // At once: a key that is held down goes row by row, and the hours with it.
            ui.scroll_to_rect_animation(rows, None, egui::style::ScrollAnimation::none());
        }
        // While an event is dragged against their upper or lower edge the hours scroll along,
        // at about five hours a second. (Above them is the row of whole days, which is
        // somewhere to drop an event; an edge that is being dragged only has the hours.)
        if let (Some(drag), Some(pos)) = (frame.drag.filter(|d| !d.cancelled), pointer) {
            let step = AUTO_SCROLL * ui.input(|i| i.stable_dt).clamp(0.0, 0.05);
            let edge = drag.grip != Grip::Move;
            let pull = if pos.y < seen.min.y + 24.0 && (pos.y >= seen.min.y || (edge && !drag.bar)) {
                step
            } else if pos.y > seen.max.y - 24.0 && !(edge && drag.bar) {
                -step
            } else {
                0.0
            };
            if pull != 0.0 {
                ui.scroll_with_delta_animation(vec2(0.0, pull), egui::style::ScrollAnimation::none());
                ui.ctx().request_repaint();
            }
        }
    });
}

/// An hour in the time bar, its right end at `at` and its top there: the hour in large figures
/// with `AM` or `PM` (or the `00` of a 24-hour clock) small beside them.
fn paint_hour(p: &Painter, at: egui::Pos2, hour: u8, twelve: bool, t: &Tokens) {
    let (figures, small) = if twelve {
        let hour12 = if hour.is_multiple_of(12) { 12 } else { hour % 12 };
        (hour12.to_string(), if hour < 12 { tl!("AM") } else { tl!("PM") })
    } else {
        (format!("{hour:02}"), "00")
    };
    let suffix = p.text(pos2(at.x, at.y + 1.0), Align2::RIGHT_TOP, small, regular(9.0), t.text_dim);
    p.text(pos2(suffix.min.x - 2.0, at.y), Align2::RIGHT_TOP, figures, regular(14.0), t.text_dim);
}

/// A time of day in the time bar's second column (`minutes` since midnight), its right end at
/// `at`: as [`paint_hour`] for a full hour, and `9:30` in small figures for a zone that is half
/// or three quarters of an hour apart.
fn paint_clock(p: &Painter, at: egui::Pos2, minutes: i64, twelve: bool, t: &Tokens) {
    let (hour, minute) = (u8::try_from(minutes.div_euclid(60).clamp(0, 23)).unwrap_or(0), u8::try_from(minutes.rem_euclid(60)).unwrap_or(0));
    if minute == 0 {
        return paint_hour(p, at, hour, twelve, t);
    }
    let (figures, small) = if twelve {
        let hour12 = if hour.is_multiple_of(12) { 12 } else { hour % 12 };
        (format!("{hour12}:{minute:02}"), if hour < 12 { tl!("AM") } else { tl!("PM") })
    } else {
        (format!("{hour:02}:{minute:02}"), "")
    };
    let suffix = p.text(pos2(at.x, at.y + 3.0), Align2::RIGHT_TOP, small, regular(8.0), t.text_dim);
    p.text(pos2(suffix.min.x - 1.0, at.y + 2.0), Align2::RIGHT_TOP, figures, regular(11.0), t.text_dim);
}

/// The time scales as a menu's entries, the one in use ticked: what a row of the hours stands
/// for. (The time bar's menu, and View › Time Scale.)
pub(crate) fn time_scale_menu(ui: &mut Ui, app: &mut MailApp) {
    let current = time_scale(&app.session);
    for minutes in TIME_SCALES {
        let label = match minutes {
            60 => "60 Minutes",
            30 => "30 Minutes",
            15 => "15 Minutes",
            10 => "10 Minutes",
            6 => "6 Minutes",
            _ => "5 Minutes",
        };
        crate::widgets::mi_check(ui, app, label, minutes == current, "view.timeScale", json!({"minutes": minutes}));
    }
}

/// `Wed, Oct 14`, or `Oct 14 – Oct 16` for more days than one.
fn days_label(first: Date, last: Date) -> String {
    use crate::dates::{month_day, weekday_month_day};
    if last > first { format!("{} – {}", month_day(first), month_day(last)) } else { weekday_month_day(first) }
}

/// Where a dragged event would land, drawn where the calendar in view shows such a time: its
/// block in the hours, its bar in the row of whole days, or its place in the month, in the
/// block of calendars number `pane`, with the new time on it. A copy says so with a plus.
/// Returns where the first piece of it was drawn.
fn paint_ghost(ui: &Ui, zones: &Zones, item: Option<&Item>, span: Span, copy: bool, twelve: bool, pane: usize) -> Option<Rect> {
    let t = Tokens::get(ui.ctx());
    let color = item.map_or(t.accent, |i| i.color);
    // Nearly solid, so it reads over whatever it is dragged across.
    let fill = mix(t.surface, color, if t.dark { 0.5 } else { 0.3 }).gamma_multiply(0.94);
    let last = last_day(span.start, span.end);
    let (a, b) = (zones.from.days_until(span.start.date), zones.from.days_until(last));
    let whole = span.all_day || span.start.minutes_until(span.end) >= DAY_MINUTES;
    // The pieces it is drawn in, each with the part of the calendar it stays inside.
    let mut pieces: Vec<(Rect, Rect)> = Vec::new();
    let inset = |r: Rect| Rect::from_min_max(pos2(r.min.x + 3.0, r.min.y), pos2(r.max.x - 3.0, r.max.y));
    if let Some(m) = &zones.month {
        // Whole days over the cells of each week they touch; a time of day on the day it starts.
        let b = if whole { b } else { a };
        for week in 0..m.weeks.min(6) {
            let lo = i64::try_from(week * 7).unwrap_or(0);
            let (x, y) = (a.max(lo), b.min(lo + 6));
            if x > y {
                continue;
            }
            let top = m.area.min.y + week as f32 * m.cell_h + NUM_H;
            pieces.push((inset(m.cols.cols(pane).span(to_index(x - lo), to_index(y - lo), top, top + BAR_H)), m.area));
        }
    } else if whole {
        if let Some((blocks, row)) = &zones.all_day {
            let cols = blocks.cols(pane);
            let (x, y) = (a.max(0), b.min(i64::try_from(cols.n).unwrap_or(1) - 1));
            if x <= y {
                pieces.push((inset(cols.span(to_index(x), to_index(y), row.min.y + 3.0, row.min.y + 3.0 + BAR_H)), *row));
            }
        }
    } else if let Some(h) = &zones.hours {
        let start = a.saturating_mul(DAY_MINUTES).saturating_add(i64::from(span.start.minutes_of_day()));
        let end = start.saturating_add(span.start.minutes_until(span.end).max(h.scale.step()));
        let cols = h.cols.cols(pane);
        for i in 0..cols.n {
            let day_start = i64::try_from(i).unwrap_or(0) * DAY_MINUTES;
            let (from, to) = (start.max(day_start), end.min(day_start + DAY_MINUTES));
            if from >= to {
                continue;
            }
            let y0 = h.top + h.scale.y(from - day_start);
            let y1 = (h.top + h.scale.y(to - day_start)).max(y0 + MIN_BLOCK_H);
            pieces.push((Rect::from_min_max(pos2(cols.x(i) + 1.0, y0 + 1.0), pos2(cols.x(i + 1) - 2.0, y1 - 1.0)), h.seen));
        }
    }
    let when = if span.all_day {
        days_label(span.start.date, last)
    } else {
        format!("{} – {}", crate::dates::clock(span.start, twelve), crate::dates::clock(span.end, twelve))
    };
    let title = item.map(|i| i.title()).unwrap_or_default();
    for (n, (rect, within)) in pieces.iter().enumerate() {
        let p = ui.painter().with_clip_rect(within.intersect(ui.clip_rect()));
        p.rect_filled(*rect, 3.0, fill);
        p.rect_stroke(*rect, 3.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
        if n > 0 {
            continue;
        }
        // The first piece says when, and what.
        let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
        let (left, width) = (rect.min.x + 8.0, rect.width() - if copy { 30.0 } else { 12.0 });
        if rect.height() < 32.0 {
            let y = rect.center().y;
            let used = paint_clipped(&p, pos2(left, y), width, &when, semibold(11.5), t.text);
            if width - used > 48.0 {
                paint_clipped(&p, pos2(left + used + 8.0, y), width - used - 8.0, title, regular(10.5), t.text_dim);
            }
        } else {
            let y = rect.min.y + 11.0;
            paint_clipped(&p, pos2(left, y), width, &when, semibold(11.5), t.text);
            paint_clipped(&p, pos2(left, y + 15.0), width, title, regular(10.5), t.text_dim);
        }
        if copy {
            // A plus in a disc: letting go here makes a copy.
            let c = pos2(rect.max.x - 11.0, rect.min.y + rect.height().min(22.0) / 2.0);
            p.circle_filled(c, 7.0, t.accent);
            p.line_segment([pos2(c.x - 3.5, c.y), pos2(c.x + 3.5, c.y)], Stroke::new(1.5, t.on_accent));
            p.line_segment([pos2(c.x, c.y - 3.5), pos2(c.x, c.y + 3.5)], Stroke::new(1.5, t.on_accent));
        }
    }
    pieces.first().map(|(rect, within)| rect.intersect(*within))
}

/// Finish a drag: the event becomes what its occurrence was dragged to (a series as a whole, by
/// as much as that occurrence went), or a copy of it is made there; dropped in the block of
/// another calendar, it (or the copy) goes into that calendar.
fn drop_event(app: &mut MailApp, drag: &Drag, span: Span, copy: bool) {
    let Some(ev) = app.session.mailbox.events.get(&drag.event).cloned() else { return };
    // Dropped in another calendar's block: it goes into that calendar, or its copy does.
    let into = Some(drag.into).filter(|into| *into != 0 && *into != ev.calendar && drag.grip == Grip::Move);
    if drag.cancelled || (span == drag.here() && into.is_none()) {
        return;
    }
    let id = drag.event;
    let _ = app.execute("calendar.select", json!({"id": id, "day": drag.start.date.iso()}));
    // One day of a series can't be in another calendar than the rest, and a drag of one of its
    // days doesn't take the whole series there: that is said, and nothing is moved.
    if into.is_some() && !copy && ev.recurrence.is_some() {
        app.status(series_stays());
        return;
    }
    let last = |from: DateTime, to: DateTime| last_day(from, to).iso();
    let (command, mut params) = if copy {
        // A copy is of the occurrence, at the time it was dropped on.
        let params = if span.all_day {
            json!({"id": id, "start": span.start.date.iso(), "end": last(span.start, span.end), "allDay": true})
        } else {
            json!({"id": id, "start": iso_minute(span.start), "end": iso_minute(span.end), "allDay": false})
        };
        ("calendar.copy", params)
    } else if span.all_day != ev.all_day {
        // Between the hours and whole days. As a new one of its kind would be: whole days
        // don't take the day up and remind the afternoon before, an appointment is busy time
        // with the usual reminder; what the owner set otherwise stays.
        let usual = app.session.prefs.default_reminder;
        let whole_days = usual.map(|_| mailcraft_engine::cmd::calendar::ALL_DAY_REMINDER);
        let mut params = if span.all_day {
            json!({"id": id, "allDay": true, "start": span.start.date.iso(), "end": last(span.start, span.end)})
        } else {
            json!({"id": id, "allDay": false, "start": iso_minute(span.start), "end": iso_minute(span.end)})
        };
        let (show_from, show_to, remind_from, remind_to) =
            if span.all_day { (ShowAs::Busy, ShowAs::Free, usual, whole_days) } else { (ShowAs::Free, ShowAs::Busy, whole_days, usual) };
        if let Some(fields) = params.as_object_mut() {
            if ev.show_as == show_from {
                fields.insert("showAs".into(), json!(show_to.code()));
            }
            if ev.reminder_minutes == remind_from && remind_from != remind_to {
                fields.insert("reminder".into(), json!(remind_to));
            }
        }
        ("calendar.edit", params)
    } else {
        // The event goes by as much as its occurrence did, at each end.
        let (start, end) = (ev.start.add_minutes(drag.start.minutes_until(span.start)), ev.end.add_minutes(drag.end.minutes_until(span.end)));
        let mut params = if ev.all_day {
            json!({"id": id, "start": start.date.iso(), "end": last(start, end)})
        } else if drag.minutes() == span.start.minutes_until(span.end) {
            json!({"id": id, "start": iso_minute(start)})
        } else {
            json!({"id": id, "start": iso_minute(start), "end": iso_minute(end)})
        };
        alone(&mut params, &ev, drag.start.date, span.start.date);
        ("calendar.move", params)
    };
    // Into another calendar at the time it has: only the calendar changes.
    if into.is_some() && !copy && span == drag.here() {
        params = json!({"id": id});
    }
    // (The engine says what can't change calendars, and why: an event that a server keeps.)
    let command = into_calendar(&mut params, into, command);
    // An item that repeats, somebody else's meeting, one with people invited: the change is
    // asked about first, and made when it is answered.
    let done = app.run(command, params);
    if let (false, Ok(answer)) = (copy, done)
        && answer.get("pending").is_none()
    {
        // The selection follows the occurrence that was dragged (which, changed on its own, is
        // an event of its own now).
        let now = answer.get("id").and_then(Value::as_u64).unwrap_or(id);
        let _ = app.execute("calendar.select", json!({"id": now, "day": span.start.date.iso()}));
    }
}

/// A move of the occurrence of `ev` on `day` that lands it on another day, for a series that
/// keeps its days (every weekday, every Tuesday and Thursday: [`moves_across_days`]): such a
/// series can't go to another day as a whole, so it is that one occurrence that goes, and the
/// parameters say so. Any other move says nothing, and the owner is asked which is meant.
fn alone(params: &mut Value, ev: &Event, day: Date, lands: Date) {
    if ev.recurrence.is_some()
        && !moves_across_days(ev.recurrence.as_ref())
        && lands != day
        && let Some(fields) = params.as_object_mut()
    {
        fields.insert("scope".into(), json!("occurrence"));
        fields.insert("occurrence".into(), json!(day.iso()));
    }
}

/// An event that was dragged with the right button and let go: what to do with it is asked there.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DropMenu {
    drag: Drag,
    span: Span,
    pos: egui::Pos2,
}

/// The question a drop with the right button asks, where the button was let go: Move, Copy or
/// Cancel. A press anywhere else, or Escape, is Cancel. Until it is answered the event's ghost
/// stays where it would go.
fn drop_menu(app: &mut MailApp, ui: &Ui, zones: &Zones, items: &[Item]) {
    let id = egui::Id::unique(DROP_MENU_ID);
    let Some(ask) = ui.ctx().data(|d| d.get_temp::<DropMenu>(id)) else { return };
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let item = items.iter().find(|i| i.ev.id == ask.drag.event);
    // The event went away, or a dialog came up: nothing is asked any more.
    if item.is_none() || app.dialog.is_some() {
        ctx.data_mut(|d| d.remove::<DropMenu>(id));
        return;
    }
    paint_ghost(ui, zones, item, ask.span, false, app.session.prefs.twelve_hour, ask.drag.over);
    // `Some(copy)` for Move and Copy, `None` for Cancel.
    let mut answer: Option<Option<bool>> = None;
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(ask.pos).constrain(true).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
            for (n, (label, deed)) in [("Move", Some(false)), ("Copy", Some(true)), ("Cancel", None)].into_iter().enumerate() {
                if deed.is_none() {
                    ui.separator();
                }
                let (r, resp) = ui.allocate_exact_size(vec2(150.0, 26.0), Sense::click());
                app.hit.add("dropMenu", n as u64, r);
                if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
                }
                ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, tl!(label), regular(12.5), t.text);
                if resp.clicked() {
                    answer = Some(deed);
                }
            }
        });
    });
    let away = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|pos| !area.response.rect.contains(pos)));
    if answer.is_none() && (away || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
        answer = Some(None);
    }
    let Some(deed) = answer else { return };
    ctx.data_mut(|d| d.remove::<DropMenu>(id));
    if let Some(copy) = deed {
        drop_event(app, &ask.drag, ask.span, copy);
    }
}

// ---- month ----

fn month_grid(app: &mut MailApp, ui: &mut Ui, frame: &mut Frame, from: Date, to: Date, items: &[Item], panes: &[PaneView]) {
    let t = Tokens::get(ui.ctx());
    // Coming back to a time grid scrolls it to the working day again.
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(GRID_VIEW_ID), (CalView::Month, 0i64)));
    let days = days_of(from, to, 42);
    let weeks = days.len().div_ceil(7).clamp(1, 6);
    let today = app.session.today();
    let twelve = app.session.prefs.twelve_hour;
    let shown = app.session.view.date;

    // With week numbers, a narrow column of them left of the days; then a month for every
    // block of calendars, side by side.
    let numbers = app.session.prefs.week_numbers;
    let whole = ui.available_rect_before_wrap();
    let margin = if numbers { WEEK_NUMBER_W.min(whole.width() / 4.0) } else { 0.0 };
    let blocks = Blocks::of(whole.min.x + margin, whole.width() - margin, 7, panes.len());
    if tabbed(panes) {
        tab_strip(app, ui, &blocks, panes);
    }
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), MONTH_HEAD_H), Sense::hover());
    let (full, _) = ui.allocate_exact_size(vec2(ui.available_width(), ui.available_height().max(60.0)), Sense::hover());
    let area = Rect::from_min_max(pos2(full.min.x + margin, full.min.y), full.max);
    let p = ui.painter().clone();
    for k in 0..blocks.count {
        let cols = blocks.cols(k);
        for i in 0..7usize {
            let weekday = Weekday::from_index(i64::from(from.weekday().index()) + i64::try_from(i).unwrap_or(0));
            let name = if cols.width >= 96.0 { weekday.name() } else { weekday.short() };
            let inset = if cols.width >= 44.0 { 10.0 } else { 3.0 };
            paint_clipped(&p, pos2(cols.x(i) + inset, head.center().y), cols.width - inset - 4.0, tl!(name), regular(11.5), t.text_dim);
        }
    }
    p.hline(head.x_range(), head.max.y - 0.5, Stroke::new(1.0, t.border));

    let cell_h = (area.height() / weeks as f32).max(1.0);
    frame.zones.from = from;
    frame.zones.fronts = fronts(panes);
    frame.zones.month = Some(MonthCells { cols: blocks, area, cell_h, weeks });
    // The day of the cell at a point, for an event that is grabbed there.
    let under = |pos: egui::Pos2| {
        let week = (((pos.y - area.min.y) / cell_h).floor().max(0.0) as usize).min(weeks - 1);
        Under::Day(days.get(week * 7 + blocks.at(pos.x).index_at(pos.x)).copied().unwrap_or(from))
    };
    let pitch = BAR_H + BAR_GAP;
    let rows = (((cell_h - NUM_H - 2.0) / pitch).floor().max(0.0) as usize).min(MAX_ROWS);
    // Each block lays its own calendar's items out: which of the items they are, and for each
    // the first and last day it touches and whether it is a bar.
    let mut mine: Vec<Vec<usize>> = (0..blocks.count).map(|_| Vec::new()).collect();
    let mut spans: Vec<Vec<(i64, i64, bool)>> = (0..blocks.count).map(|_| Vec::new()).collect();
    for (i, item) in items.iter().enumerate() {
        let pane = item.pane.min(blocks.count.saturating_sub(1));
        if let (Some(mine), Some(spans)) = (mine.get_mut(pane), spans.get_mut(pane)) {
            mine.push(i);
            spans.push((from.days_until(item.occ.start.date), from.days_until(item.last_day()), item.is_bar()));
        }
    }
    // The days that are picked out, when any in view are: a click on a day picks it, a drag
    // over days picks them all, and what is typed then is a whole-day entry on them, in the
    // calendar of the block they are picked in.
    let was = load_slot(ui.ctx());
    let in_view = |s: &Slot| days.first().is_some_and(|first| s.last() >= *first) && days.last().is_some_and(|last| s.start.date <= *last);
    let mut slot = was.clone().filter(|s| s.all_day && in_view(s));
    let picked = slot.clone();
    let picked_in = picked.as_ref().map(|s| pane_of(panes, s.calendar));
    let mut entry_at: Option<Rect> = None;
    let pointer = frame.pointer;
    for week in 0..weeks {
        let top = area.min.y + week as f32 * cell_h;
        let day_at = |d: usize| days.get(week * 7 + d).copied();
        // Which week of the year it is, when that is wanted: a click on it shows the week.
        if let Some(first) = day_at(0).filter(|_| numbers) {
            let cell = Rect::from_min_max(pos2(full.min.x, top), pos2(area.min.x, top + cell_h));
            let resp = ui.interact(cell, ui.scope_id().with(("month_week", week)), Sense::click());
            if let Ok(id) = u64::try_from(first.to_days()) {
                app.hit.add("weekNumber", id, cell);
            }
            if resp.hovered() {
                p.rect_filled(cell, 0.0, t.hover);
            }
            let at = pos2(cell.center().x, cell.min.y + NUM_H / 2.0 + 1.0);
            p.text(at, Align2::CENTER_CENTER, week_number(first).to_string(), regular(10.5), t.text_dim);
            p.hline(cell.x_range(), cell.max.y - 0.5, Stroke::new(1.0, t.border));
            if resp.on_hover_text(tl!("Show this week")).clicked() {
                let _ = app.run("view.goToDate", json!({"date": first.iso()}));
                let _ = app.run("view.week", json!({}));
            }
        }
        for k in 0..blocks.count {
            let cols = blocks.cols(k);
            let front = panes.get(k).map_or(0, |pane| pane.front);
            let (Some(mine), Some(spans)) = (mine.get(k), spans.get(k)) else { continue };
            let item_at = |index: usize| mine.get(index).and_then(|i| items.get(*i));
            let layout = layout_week(spans, i64::try_from(week).unwrap_or(0));
            // How many rows of each day hold events, and how many events are left for `+n more`.
            // A cell too low for two rows keeps its row for an event and says so beside the day
            // number.
            let compact = rows < 2;
            let mut limit = [0usize; 7];
            for (slot, needed) in limit.iter_mut().zip(layout.rows) {
                *slot = if compact { rows } else { chip_limit(needed, rows) };
            }
            let limit_of = |d: usize| limit.get(d).copied().unwrap_or(0);
            let mut more = layout.totals;

            // The cells first, so what is drawn in them takes the clicks.
            for d in 0..7usize {
                let Some(day) = day_at(d) else { continue };
                let cell = cols.span(d, d, top, top + cell_h);
                let is_today = day == today;
                if is_today {
                    p.rect_filled(cell, 0.0, t.today);
                } else if day.month != shown.month || day.year != shown.year {
                    p.rect_filled(cell, 0.0, t.off_hours);
                }
                if let Some(picked) = picked.as_ref().filter(|s| picked_in == Some(k) && (s.start.date..=s.last()).contains(&day)) {
                    p.rect_filled(cell.shrink2(vec2(1.0, 1.0)), 0.0, t.selection);
                    if entry_at.is_none() {
                        entry_at =
                            Some(Rect::from_min_max(pos2(cell.min.x + 3.0, cell.min.y + NUM_H), pos2(cell.max.x - 3.0, cell.min.y + NUM_H + BAR_H)));
                        if let Ok(id) = u64::try_from(picked.start.date.to_days()) {
                            app.hit.add("slot", id, cell);
                        }
                    }
                }
                p.vline(cell.min.x, cell.y_range(), Stroke::new(1.0, t.border));
                p.hline(cell.x_range(), cell.max.y - 0.5, Stroke::new(1.0, t.border));
                // The first of a month says which month.
                let label = if day.day == 1 { crate::dates::month_day(day) } else { day.day.to_string() };
                let font = if is_today || day.day == 1 { semibold(11.5) } else { regular(11.5) };
                let inset = if cell.width() >= 44.0 { 9.0 } else { 4.0 };
                let at = pos2(cell.min.x + inset, cell.min.y + NUM_H / 2.0 + 1.0);
                let clipped = p.with_clip_rect(cell.intersect(p.clip_rect()));
                if is_today {
                    let w = text_w(ui, &label, font.clone()).max(8.0);
                    clipped.rect_filled(Rect::from_min_max(pos2(at.x - 6.0, at.y - 10.0), pos2(at.x + w + 6.0, at.y + 10.0)), 10.0, t.accent);
                }
                let ink = if is_today {
                    t.on_accent
                } else if day.month != shown.month || day.year != shown.year {
                    t.text_dim
                } else {
                    t.text
                };
                clipped.text(at, Align2::LEFT_CENTER, label, font, ink);
                let resp = ui.interact(cell, ui.scope_id().with(("month_cell", k, week, d)), Sense::click_and_drag());
                if let Ok(id) = u64::try_from(day.to_days()) {
                    app.hit.add("day", id, cell);
                }
                empty_input(app, &resp, &mut slot, || Slot::day(day).in_calendar(front));
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    let _ = app.execute("calendar.select", json!({"id": null}));
                    work_in(app, front);
                    slot = Some(Slot::day(day).in_calendar(front));
                }
                // A drag that began on this day picks the days up to the one under the pointer,
                // in this block.
                if resp.dragged_by(egui::PointerButton::Primary)
                    && let (Some(stretched), Some(pos)) = (slot.as_mut().filter(|s| s.text.is_none()), pointer)
                    && area.contains(pos)
                {
                    let row = (((pos.y - area.min.y) / cell_h).floor().max(0.0) as usize).min(weeks - 1);
                    if let Some(under) = days.get(row * 7 + cols.index_at(pos.x)) {
                        stretched.stretch_to_day(*under);
                    }
                }
            }
            if blocks.count > 1 {
                p.vline(cols.right(), egui::Rangef::new(top, top + cell_h), Stroke::new(1.0, t.border));
            }

            let row_top = |row: usize| top + NUM_H + row as f32 * pitch;
            for &(index, first, last, row) in &layout.bars {
                let Some(item) = item_at(index) else { continue };
                let (begins, finishes) = (from.days_until(item.occ.start.date), from.days_until(item.last_day()));
                let week_start = i64::try_from(week * 7).unwrap_or(0);
                // A bar shows on the days that have a row for it, in pieces where a day in between hasn't.
                let mut d = first;
                while d <= last.min(6) {
                    if row >= limit_of(d) || day_at(d).is_none() {
                        d += 1;
                        continue;
                    }
                    let mut end = d;
                    while end < last.min(6) && row < limit_of(end + 1) && day_at(end + 1).is_some() {
                        end += 1;
                    }
                    for slot in more.iter_mut().take(end + 1).skip(d) {
                        *slot = slot.saturating_sub(1);
                    }
                    let (starts, ends) =
                        (week_start + i64::try_from(d).unwrap_or(0) == begins, week_start + i64::try_from(end).unwrap_or(0) == finishes);
                    let span = cols.span(d, end, row_top(row), row_top(row) + BAR_H);
                    let rect = Rect::from_min_max(
                        pos2(span.min.x + if starts { 3.0 } else { 0.0 }, span.min.y),
                        pos2(span.max.x - if ends { 3.0 } else { 1.0 }, span.max.y),
                    );
                    let resp = ui.interact(
                        rect,
                        ui.scope_id().with(("month_bar", item.ev.id, item.occ.start.date.to_days(), week, d)),
                        Sense::click_and_drag(),
                    );
                    app.hit.add("event", item.ev.id, rect);
                    // Dragging moves it by days; its ends change its first and its last day.
                    frame.track(ui, &resp, item, rect, Piece { bar: true, starts, ends }, &under);
                    let dragging = frame.drag.is_some();
                    paint_bar(&p, rect, item, &t, twelve, starts, ends, is_selected(app, item), resp.hovered() && !dragging);
                    if frame.fades(item) {
                        p.rect_filled(rect, 3.0, t.surface.linear_multiply(0.55));
                    }
                    event_input(app, resp, item, !dragging);
                    d = end + 1;
                }
            }
            for &(index, d, row) in &layout.chips {
                let Some(item) = item_at(index) else { continue };
                if row >= limit_of(d) || day_at(d).is_none() {
                    continue;
                }
                if let Some(slot) = more.get_mut(d) {
                    *slot = slot.saturating_sub(1);
                }
                let span = cols.span(d, d, row_top(row), row_top(row) + BAR_H);
                let rect = Rect::from_min_max(pos2(span.min.x + 3.0, span.min.y), pos2(span.max.x - 3.0, span.max.y));
                let resp =
                    ui.interact(rect, ui.scope_id().with(("month_chip", item.ev.id, item.occ.start.date.to_days(), week)), Sense::click_and_drag());
                app.hit.add("event", item.ev.id, rect);
                // Dragged to another day it keeps its time of day.
                frame.track(ui, &resp, item, rect, Piece { bar: false, starts: false, ends: false }, &under);
                let dragging = frame.drag.is_some();
                paint_chip(&p, rect, item, &t, twelve, is_selected(app, item), resp.hovered() && !dragging);
                if frame.fades(item) {
                    p.rect_filled(rect, 3.0, t.surface.linear_multiply(0.55));
                }
                event_input(app, resp, item, !dragging);
            }
            for (d, count) in more.iter().enumerate().filter(|(_, n)| **n > 0) {
                let Some(day) = day_at(d) else { continue };
                let rect = if compact {
                    let cell = cols.span(d, d, top + 4.0, top + NUM_H - 2.0);
                    Rect::from_min_max(pos2((cell.max.x - 66.0).max(cell.min.x + 34.0), cell.min.y), pos2(cell.max.x - 3.0, cell.max.y))
                } else {
                    let span = cols.span(d, d, row_top(limit_of(d)), row_top(limit_of(d)) + BAR_H);
                    Rect::from_min_max(pos2(span.min.x + 3.0, span.min.y), pos2(span.max.x - 3.0, span.max.y))
                };
                more_link(app, ui, rect, ("month_more", k * 1000 + week * 7 + d), *count, day);
            }
        }
    }
    // What is typed with days picked out starts a whole-day entry in the first of them, over
    // whatever is there. (A pick that changed this frame is drawn, and typed into, from the next.)
    if let (Some(at), Some(now)) = (entry_at, slot.as_mut().filter(|now| picked.as_ref() == Some(&**now))) {
        let entry = slot_entry(app, ui, at, now, false);
        finish_entry(app, &mut slot, entry);
    }
    keep_slot(ui.ctx(), &was, slot, |s| s.all_day);
}

#[cfg(test)]
mod tests {
    use mailcraft_engine::Session;

    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    fn at(day: Date, h: u8, m: u8) -> DateTime {
        DateTime::new(day, h, m, 0).unwrap()
    }

    #[test]
    fn overlapping_blocks_share_the_width() {
        // Apart: each has the day to itself.
        assert_eq!(pack_columns(&[(540, 600), (600, 660), (700, 730)]), [(0, 1), (0, 1), (0, 1)]);
        // Two at once: side by side.
        assert_eq!(pack_columns(&[(540, 600), (570, 630)]), [(0, 2), (1, 2)]);
        // A chain: the third fits under the first, and the whole cluster is two columns wide.
        assert_eq!(pack_columns(&[(540, 600), (570, 630), (600, 660), (900, 930)]), [(0, 2), (1, 2), (0, 2), (0, 1)]);
        // Three at once, then a fourth that only meets the longest.
        assert_eq!(pack_columns(&[(0, 300), (10, 60), (20, 60), (100, 120)]), [(0, 3), (1, 3), (2, 3), (1, 3)]);
        assert!(pack_columns(&[]).is_empty());
    }

    #[test]
    fn hostile_blocks_pack_without_panicking() {
        // No length, an end before the start, the ends of the number line, out of order.
        let odd = [(600, 600), (600, 10), (i64::MIN, i64::MAX), (i64::MAX, i64::MIN), (5, 6)];
        let packed = pack_columns(&odd);
        assert_eq!(packed.len(), odd.len());
        assert!(packed.iter().all(|(column, columns)| column < columns));
        // Two events without a length at the same minute still sit side by side.
        assert_eq!(pack_columns(&[(600, 600), (600, 600)]), [(0, 2), (1, 2)]);
        // A day with thousands of events at once.
        let many = vec![(540, 600); 5000];
        let packed = pack_columns(&many);
        assert_eq!(packed.last(), Some(&(4999, 5000)));
    }

    #[test]
    fn bars_take_the_first_free_lane() {
        assert_eq!(assign_lanes(&[(0, 2), (1, 3), (3, 4), (4, 6)]), [0, 1, 0, 1]);
        assert_eq!(assign_lanes(&[(0, 6), (0, 0), (1, 1)]), [0, 1, 1]);
        // A span the wrong way round counts as its first day.
        assert_eq!(assign_lanes(&[(3, 1), (3, 3), (4, 4)]), [0, 1, 0]);
        assert_eq!(assign_lanes(&vec![(0, 6); 500]).last(), Some(&499));
    }

    #[test]
    fn range_titles() {
        let (day, mon) = (d(2026, 10, 14), d(2026, 10, 12));
        assert_eq!(range_title(CalView::Day, day, day, day), "October 14, 2026");
        assert_eq!(range_title(CalView::Week, day, d(2026, 10, 11), d(2026, 10, 17)), "October 11 – 17, 2026");
        assert_eq!(range_title(CalView::WorkWeek, day, mon, d(2026, 10, 16)), "October 12 – 16, 2026");
        assert_eq!(range_title(CalView::Week, d(2026, 10, 1), d(2026, 9, 27), d(2026, 10, 3)), "September 27 – October 3, 2026");
        assert_eq!(range_title(CalView::WorkWeek, d(2026, 9, 30), d(2026, 9, 28), d(2026, 10, 4)), "September 28 – October 4, 2026");
        assert_eq!(range_title(CalView::Week, d(2026, 12, 30), d(2026, 12, 27), d(2027, 1, 2)), "December 27, 2026 – January 2, 2027");
        // A work week of one working day.
        assert_eq!(range_title(CalView::WorkWeek, day, day, day), "October 14, 2026");
        // The month is the one the date is in, whatever weeks the grid shows.
        assert_eq!(range_title(CalView::Month, day, d(2026, 9, 27), d(2026, 10, 31)), "October 2026");
        // The ends of the calendar.
        assert_eq!(range_title(CalView::Week, d(9999, 12, 31), d(9999, 12, 26), d(9999, 12, 31)), "December 26 – 31, 9999");
        assert_eq!(range_title(CalView::Month, d(1, 1, 1), d(1, 1, 1), d(1, 2, 3)), "January 1");
    }

    #[test]
    fn minutes_and_points() {
        // The usual scale: half an hour a row, two rows to the hour.
        let scale = Scale::of(30);
        assert_eq!((scale.slot, scale.hour_h, scale.step()), (30, HOUR_H, 15));
        assert_eq!(scale.y(0), 0.0);
        assert_eq!(scale.y(60), HOUR_H);
        assert_eq!(scale.y(DAY_MINUTES), 24.0 * HOUR_H);
        // Outside the day: clamped to it.
        assert_eq!(scale.y(-30), 0.0);
        assert_eq!(scale.y(i64::MAX), 24.0 * HOUR_H);
        for minute in [0, 1, 15, 570, 1439, 1440] {
            assert_eq!(scale.minute(scale.y(minute) + 0.01), minute);
        }
        assert_eq!(scale.minute(HOUR_H * 9.5), 570);
        assert_eq!(scale.minute(-5.0), 0);
        assert_eq!(scale.minute(1.0e12), DAY_MINUTES);
        assert_eq!(scale.minute(f32::NAN), 0);
        assert_eq!(scale.minute(f32::INFINITY), 0);
        assert_eq!(scale.minute(f32::NEG_INFINITY), 0);
        // Every scale has rows of the same height, so a finer one makes a taller day; dragging
        // goes by a quarter of an hour until the rows are finer than that.
        let of = |minutes: u8| (Scale::of(minutes).slot, Scale::of(minutes).hour_h, Scale::of(minutes).step());
        assert_eq!(of(60), (60, ROW_H, 15));
        assert_eq!(of(15), (15, 4.0 * ROW_H, 15));
        assert_eq!(of(10), (10, 6.0 * ROW_H, 10));
        assert_eq!(of(6), (6, 10.0 * ROW_H, 6));
        assert_eq!(of(5), (5, 12.0 * ROW_H, 5));
        for scale in TIME_SCALES.map(Scale::of) {
            assert_eq!(scale.y(scale.slot), ROW_H);
            assert_eq!(scale.minute(scale.y(13 * 60) + 0.01), 13 * 60);
            assert_eq!(snap_down(scale.minute(scale.y(13 * 60) + ROW_H - 1.0), scale.slot), 13 * 60);
        }
        // A scale nobody offers is the usual one.
        for odd in [0, 1, 7, 45, 255] {
            assert_eq!(Scale::of(odd), scale);
        }
        let _ = Scale { slot: 0, hour_h: 0.0 }.minute(10.0);
        assert_eq!(Scale { slot: i64::MAX, hour_h: f32::NAN }.minute(10.0), 0);
    }

    #[test]
    fn snapping() {
        assert_eq!(snap(7, 15), 0);
        assert_eq!(snap(8, 15), 15);
        assert_eq!(snap(22, 15), 15);
        assert_eq!(snap(23, 15), 30);
        assert_eq!(snap(-8, 15), -15);
        assert_eq!(snap(600, 15), 600);
        assert_eq!(snap_down(59, 30), 30);
        assert_eq!(snap_down(60, 30), 60);
        assert_eq!(snap_down(-1, 30), -30);
        // A step of nothing, and the ends of the number line.
        assert_eq!(snap(17, 0), 17);
        assert_eq!(snap_down(17, -5), 17);
        let _ = (snap(i64::MAX, 15), snap(i64::MIN, 15), snap_down(i64::MIN, 30), snap_down(i64::MAX, 30));
    }

    #[test]
    fn dragging_snaps_to_a_quarter_hour_inside_the_day() {
        // A nudge leaves the event where it was, odd minute and all.
        assert_eq!(drag_minute(607, 5, 15), 607);
        assert_eq!(drag_minute(607, -7, 15), 607);
        // A real move lands on the quarter hour.
        assert_eq!(drag_minute(600, 20, 15), 615);
        assert_eq!(drag_minute(607, 30, 15), 630);
        assert_eq!(drag_minute(600, -65, 15), 540);
        // Never out of the day.
        assert_eq!(drag_minute(60, -500, 15), 0);
        assert_eq!(drag_minute(1380, 500, 15), 1425);
        assert_eq!(drag_minute(i64::MAX, i64::MAX, 15), 1425);
        assert_eq!(drag_minute(i64::MIN, i64::MIN, 0), 0);

        let wed = d(2026, 10, 14);
        let origin = at(wed, 11, 0);
        let state = grabbed(origin, at(wed, 12, 0), false, Grip::Move, Under::Time { day: wed, minute: 680 });
        let over = |state: &Drag, day: Date, minute: i64| state.span(Under::Time { day, minute }, 15, 30);
        // Down an hour and a bit, two columns to the right; it stays as long as it was.
        assert_eq!(over(&state, d(2026, 10, 16), 750), Span { start: at(d(2026, 10, 16), 12, 15), end: at(d(2026, 10, 16), 13, 15), all_day: false });
        // Straight across: the time stays.
        assert_eq!(over(&state, d(2026, 10, 11), 682).start, at(d(2026, 10, 11), 11, 0));
        assert_eq!(over(&state, d(9999, 12, 31), i64::MIN).start.date, d(9999, 12, 31));
        assert_eq!(iso_minute(over(&state, d(2026, 10, 16), 750).start), "2026-10-16T12:15");
        // An occurrence of a series goes where it is dragged like anything else. (Dropped on
        // another day, one of a series that keeps its days goes there on its own.)
        let series = Drag { repeats: true, ..state };
        assert_eq!(over(&series, d(2026, 10, 16), 750).start, at(d(2026, 10, 16), 12, 15));
        assert_eq!(over(&series, wed, 682), series.here());
        let every_weekday = Event {
            recurrence: Some(Recurrence {
                by_day: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri],
                ..Recurrence::every(Freq::Daily)
            }),
            ..Default::default()
        };
        let said = |ev: &Event, lands: Date| {
            let mut params = json!({"id": 1});
            alone(&mut params, ev, wed, lands);
            params
        };
        assert_eq!(said(&every_weekday, d(2026, 10, 16)), json!({"id": 1, "scope": "occurrence", "occurrence": "2026-10-14"}));
        // On its own day it is asked about; and so is a series that can go to another day
        // whole, and what doesn't repeat has nothing to ask.
        assert_eq!(said(&every_weekday, wed), json!({"id": 1}));
        let weekly = Event { recurrence: Some(Recurrence::every(Freq::Weekly)), ..Default::default() };
        assert_eq!(said(&weekly, d(2026, 10, 16)), json!({"id": 1}));
        assert_eq!(said(&Event::default(), d(2026, 10, 16)), json!({"id": 1}));
        let mut not_fields = json!([1, 2]);
        alone(&mut not_fields, &every_weekday, wed, d(2026, 10, 16));
        assert_eq!(not_fields, json!([1, 2]));

        assert!(moves_across_days(None));
        assert!(moves_across_days(Some(&Recurrence::every(Freq::Weekly))));
        assert!(moves_across_days(Some(&Recurrence { nth: Some(2), ..Recurrence::every(Freq::Monthly) })));
        assert!(!moves_across_days(Some(&Recurrence::every(Freq::Daily))));
        assert!(!moves_across_days(Some(&Recurrence { by_day: vec![Weekday::Tue], ..Recurrence::every(Freq::Weekly) })));
    }

    #[test]
    fn days_and_last_days() {
        assert_eq!(days_of(d(2026, 10, 11), d(2026, 10, 17), 7).len(), 7);
        assert_eq!(days_of(d(2026, 10, 14), d(2026, 10, 14), 7), [d(2026, 10, 14)]);
        // The wrong way round, far too long, no room at all: still at least one day and never more than the cap.
        assert_eq!(days_of(d(2026, 10, 14), d(2026, 10, 1), 7), [d(2026, 10, 14)]);
        assert_eq!(days_of(d(1, 1, 1), d(9999, 12, 31), 42).len(), 42);
        assert_eq!(days_of(d(2026, 10, 14), d(2026, 10, 20), 0).len(), 1);
        // The calendar ends: the days stop moving but nothing breaks.
        assert_eq!(days_of(d(9999, 12, 30), d(9999, 12, 31), 7), [d(9999, 12, 30), d(9999, 12, 31)]);

        let day = d(2026, 10, 14);
        assert_eq!(last_day(at(day, 9, 0), at(day, 10, 0)), day);
        // Ending at midnight doesn't reach into the next day.
        assert_eq!(last_day(at(day, 0, 0), at(d(2026, 10, 15), 0, 0)), day);
        assert_eq!(last_day(at(day, 22, 0), at(d(2026, 10, 15), 2, 0)), d(2026, 10, 15));
        // No length, and an end before the start.
        assert_eq!(last_day(at(day, 9, 0), at(day, 9, 0)), day);
        assert_eq!(last_day(at(day, 9, 0), at(d(2026, 10, 1), 9, 0)), day);
    }

    #[test]
    fn month_cells_keep_a_row_for_more() {
        assert_eq!(chip_limit(3, 4), 4);
        assert_eq!(chip_limit(4, 4), 4);
        assert_eq!(chip_limit(5, 4), 3);
        assert_eq!(chip_limit(1, 0), 0);
        assert_eq!(chip_limit(0, 0), 0);

        // Five timed events on the Wednesday of the first week, one on the Thursday.
        let mut spans = vec![(3, 3, false); 5];
        spans.push((4, 4, false));
        let week = layout_week(&spans, 0);
        assert_eq!(week.totals, [0, 0, 0, 5, 1, 0, 0]);
        assert_eq!(week.rows, [0, 0, 0, 5, 1, 0, 0]);
        assert_eq!(week.chips.iter().filter(|c| c.1 == 3).map(|c| c.2).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        // With room for three rows, Wednesday shows two and says `+3 more`; Thursday shows its one.
        let limit = chip_limit(week.rows[3], 3);
        assert_eq!(limit, 2);
        assert_eq!(week.totals[3] - week.chips.iter().filter(|c| c.1 == 3 && c.2 < limit).count(), 3);
        assert_eq!(chip_limit(week.rows[4], 3), 3);
    }

    #[test]
    fn month_weeks_put_timed_events_under_the_bars() {
        // A bar from Tuesday to Thursday, a one-day bar on Wednesday, timed events on Wednesday and Friday.
        let spans = [(2, 4, true), (3, 3, true), (3, 3, false), (5, 5, false)];
        let week = layout_week(&spans, 0);
        assert_eq!(week.bars, [(0, 2, 4, 0), (1, 3, 3, 1)]);
        assert_eq!(week.chips, [(2, 3, 2), (3, 5, 0)]);
        assert_eq!(week.totals, [0, 0, 1, 3, 1, 1, 0]);
        assert_eq!(week.rows, [0, 0, 1, 3, 1, 1, 0]);

        // A bar that runs over the weekend is cut at the week's edges.
        let spans = [(5, 9, true), (8, 8, false)];
        assert_eq!(layout_week(&spans, 0).bars, [(0, 5, 6, 0)]);
        let second = layout_week(&spans, 1);
        assert_eq!(second.bars, [(0, 0, 2, 0)]);
        // The timed event takes the row beside the bar's last day... which the bar holds, so the next.
        assert_eq!(second.chips, [(1, 1, 1)]);
        assert!(layout_week(&spans, 2).bars.is_empty());
    }

    #[test]
    fn hostile_months_stay_bounded() {
        // Thousands of events on one day, thousands of bars across the week.
        let mut spans = vec![(1, 1, false); 5000];
        spans.extend(vec![(0, 6, true); 5000]);
        let week = layout_week(&spans, 0);
        assert!(week.bars.len() <= MAX_ROWS && week.chips.len() <= MAX_ROWS);
        assert_eq!(week.totals[1], 10_000);
        // What wasn't laid out forces the `+n more` row.
        assert!(week.rows[1] > MAX_ROWS);
        assert_eq!(chip_limit(week.rows[1], 4), 3);
        // Spans far outside the grid, the wrong way round, and weeks that don't exist.
        let odd = [(i64::MIN, i64::MAX, true), (i64::MAX, i64::MIN, true), (i64::MIN, i64::MIN, false), (9, 2, true)];
        for week in [i64::MIN, -1, 0, 1, 5, i64::MAX] {
            let _ = layout_week(&odd, week);
        }
        assert_eq!(layout_week(&odd, 1).bars, [(0, 0, 6, 0), (3, 2, 2, 1)]);
    }

    #[test]
    fn columns_find_the_day_under_the_pointer() {
        let cols = Cols { left: 100.0, width: 50.0, n: 7 };
        assert_eq!(cols.index_at(100.0), 0);
        assert_eq!(cols.index_at(149.9), 0);
        assert_eq!(cols.index_at(150.0), 1);
        assert_eq!(cols.index_at(449.0), 6);
        // Outside the columns: the nearest one.
        assert_eq!(cols.index_at(-1.0e9), 0);
        assert_eq!(cols.index_at(1.0e9), 6);
        assert_eq!(cols.index_at(f32::NAN), 0);
        assert_eq!(Cols { left: 0.0, width: 0.0, n: 0 }.index_at(10.0), 0);
        assert_eq!(cols.span(1, 2, 0.0, 10.0), Rect::from_min_max(pos2(150.0, 0.0), pos2(250.0, 10.0)));
    }

    #[test]
    fn text_is_cut_on_a_character() {
        assert_eq!(clip_text("héllo", 2), "hé");
        assert_eq!(clip_text("héllo", 50), "héllo");
        assert_eq!(clip_text("", 0), "");
    }

    /// Wednesday 2026-10-14 16:00 UTC, as the screenshots use.
    const NOW: i64 = 1_791_993_600;

    /// An occurrence from `start` to `end` that was grabbed by `grip` where `press` is.
    fn grabbed(start: DateTime, end: DateTime, all_day: bool, grip: Grip, press: Under) -> Drag {
        let bar = all_day || start.minutes_until(end) >= DAY_MINUTES;
        let (target, pane, over, into) = (None, 0, 0, 0);
        Drag { event: 1, start, end, all_day, bar, grip, menu: false, press, repeats: false, target, copy: false, cancelled: false, pane, over, into }
    }

    /// The whole window at 1280×800, driven with real pointer input, a frame every 16 ms.
    struct Rig {
        ctx: egui::Context,
        app: MailApp,
        time: f64,
        /// The modifier keys that are held down.
        mods: egui::Modifiers,
        /// The pointer's shape after the last frame.
        cursor: egui::CursorIcon,
    }

    /// Ctrl, as the keyboard of this machine has it.
    const MAC: bool = cfg!(target_os = "macos");
    /// The key of a shortcut on this system: Ctrl, or Command on a Mac.
    const CTRL: egui::Modifiers = egui::Modifiers { alt: false, ctrl: !MAC, shift: false, mac_cmd: MAC, command: true };
    /// The key that makes a drag a copy on this system: Ctrl, or Option on a Mac ([`copy_key`]).
    const COPY: egui::Modifiers = if MAC { egui::Modifiers::ALT } else { CTRL };

    impl Rig {
        fn new(mailbox: mailcraft_model::Mailbox) -> Rig {
            let mut app = MailApp::new(Session::at(mailbox, NOW, 0), crate::Services::default());
            app.autosave = false;
            app.execute("view.calendar", json!({})).unwrap();
            let mut rig = Rig { ctx: egui::Context::default(), app, time: 0.0, mods: egui::Modifiers::NONE, cursor: egui::CursorIcon::Default };
            rig.frames(4);
            // The sample has reminders that are due, and the window that says so would come up
            // over whatever is under it a moment later.
            let _ = rig.app.execute("ui.dismissReminder", json!({"all": true}));
            rig.frames(2);
            rig
        }

        fn sample() -> Rig {
            Rig::new(mailcraft_engine::sample::mailbox(NOW, 0))
        }

        fn frame(&mut self, mut events: Vec<egui::Event>) {
            self.time += 1.0 / 60.0;
            let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
            // The keyboard says which modifier keys are held, as a window's does.
            events.insert(0, egui::Event::ModifiersChanged(self.mods));
            let input = egui::RawInput { screen_rect: Some(screen), time: Some(self.time), events, ..Default::default() };
            let app = &mut self.app;
            let output = self.ctx.run_ui(input, |ui| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            });
            self.cursor = output.platform_output.cursor_icon;
            output.drop_without_applying_deltas();
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.frame(Vec::new());
            }
        }

        /// Run a command and let the window catch up.
        fn run(&mut self, id: &str, params: Value) {
            self.app.execute(id, params).unwrap();
            self.frames(3);
        }

        fn button(&mut self, pos: egui::Pos2, button: egui::PointerButton, pressed: bool) {
            let modifiers = self.mods;
            self.frame(vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton { pos, button, pressed, modifiers }]);
        }

        fn key(&mut self, key: egui::Key) {
            let modifiers = self.mods;
            self.frame(vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
            self.frame(vec![egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
        }

        /// Press `key` with `mods` held, and let them go again.
        fn chord(&mut self, mods: egui::Modifiers, key: egui::Key) {
            self.mods = mods;
            self.key(key);
            self.mods = egui::Modifiers::NONE;
            self.frames(1);
        }

        /// Click `count` times at `pos`, a moment after whatever came before.
        fn click(&mut self, pos: egui::Pos2, count: usize) {
            self.time += 1.0;
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            for _ in 0..count {
                self.button(pos, egui::PointerButton::Primary, true);
                self.button(pos, egui::PointerButton::Primary, false);
            }
            self.frames(2);
        }

        fn right_click(&mut self, pos: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            self.button(pos, egui::PointerButton::Secondary, true);
            self.button(pos, egui::PointerButton::Secondary, false);
            self.frames(2);
        }

        /// Press at `from` and move to `to` in steps, the button still down.
        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            self.drag_with(egui::PointerButton::Primary, from, to);
        }

        fn drag_with(&mut self, button: egui::PointerButton, from: egui::Pos2, to: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(from)]);
            self.button(from, button, true);
            for i in 1..=10u8 {
                self.frame(vec![egui::Event::PointerMoved(from + (to - from) * (f32::from(i) / 10.0))]);
            }
        }

        fn drop_at(&mut self, pos: egui::Pos2) {
            self.drop_with(egui::PointerButton::Primary, pos);
        }

        fn drop_with(&mut self, button: egui::PointerButton, pos: egui::Pos2) {
            self.button(pos, button, false);
            self.frames(2);
        }

        fn dragging(&self) -> Option<Drag> {
            self.ctx.data(|data| data.get_temp(egui::Id::unique(DRAG_ID)))
        }

        /// When the event that is being dragged would be if it were dropped now.
        fn target(&self) -> Option<(DateTime, DateTime, bool)> {
            self.dragging().and_then(|drag| drag.target).map(|span| (span.start, span.end, span.all_day))
        }

        fn event(&self, title: &str) -> Arc<Event> {
            self.app.session.mailbox.events.values().find(|e| e.title.starts_with(title)).cloned().unwrap_or_else(|| panic!("no event \"{title}\""))
        }

        /// Where the event (its first occurrence in view) is drawn.
        fn at(&self, title: &str) -> Rect {
            self.app.hit.rect("event", self.event(title).id).unwrap_or_else(|| panic!("\"{title}\" isn't drawn"))
        }

        /// Where the thing of `kind` for `day` is drawn (`more`, `miniDay`).
        fn day(&self, kind: &str, day: Date) -> Option<Rect> {
            self.app.hit.rect(kind, u64::try_from(day.to_days()).unwrap())
        }

        fn dialog(&self) -> Option<&'static str> {
            self.app.dialog.as_ref().map(|d| d.name())
        }

        fn selected(&self) -> (Option<Id>, Option<Date>) {
            (self.app.session.sel.event, self.app.session.sel.event_day)
        }
    }

    /// While an account's calendar waits for the owner's say-so, the calendar says so and offers
    /// the way out, instead of looking like a calendar in which nothing happens.
    #[test]
    fn a_calendar_that_is_not_connected_says_so_and_offers_to_connect() {
        let mut rig = Rig::sample();
        rig.run("view.calendar", json!({}));
        let account = rig.app.session.mailbox.accounts.first().unwrap().id;
        assert!(rig.app.hit.rect("calendarConnect", account).is_none());
        let before = rig.at("Design review");

        rig.app.graph_consent_needed.insert(account);
        rig.frames(2);
        let button = rig.app.hit.rect("calendarConnect", account).expect("the line is shown");
        // The calendar is all there, under the line.
        let after = rig.at("Design review");
        assert!(after.min.y > before.min.y && after.min.y > button.max.y, "{before:?} {after:?} {button:?}");
        // The button runs the command (which, for this account, has nothing to sign in with).
        rig.click(button.center(), 1);
        let said = rig.app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
        assert_eq!(said, "This account doesn't sign in with Microsoft");
        assert_eq!(rig.selected(), (None, None), "the click went to the button, not to the calendar under it");

        // Connected: the line goes.
        rig.app.graph_consent_needed.remove(&account);
        rig.frames(2);
        assert!(rig.app.hit.rect("calendarConnect", account).is_none());
        assert_eq!(rig.at("Design review"), before);
    }

    #[test]
    fn every_view_draws_the_sample_calendar() {
        let mut rig = Rig::sample();
        for theme in ["light", "dark"] {
            rig.run("ui.theme", json!({"value": theme}));
            for view in ["view.day", "view.workWeek", "view.week", "view.month"] {
                rig.run(view, json!({}));
                // Today's meeting is on screen in every view, and says where.
                let rect = rig.at("Design review");
                assert!(rect.width() > 20.0 && rect.height() >= MIN_BLOCK_H - 2.0, "{view}: {rect:?}");
            }
        }
        // Next week's two whole days are one bar across both.
        rig.run("view.week", json!({}));
        let column = rig.at("Team stand-up").width();
        rig.run("view.next", json!({}));
        assert!(rig.at("Planning days").width() > 1.8 * column);
        // The calendars in the side pane are there to click, until the pane is hidden.
        let calendar = rig.app.session.mailbox.calendars.first().map(|c| c.id).unwrap();
        assert!(rig.app.hit.rect("calendar", calendar).is_some());
        rig.run("view.folderPane", json!({"value": false}));
        assert!(rig.app.hit.rect("calendar", calendar).is_none());
    }

    #[test]
    fn clicks_select_open_and_start_appointments() {
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let review = rig.event("Design review").id;
        let block = rig.at("Design review");
        rig.click(block.center(), 1);
        assert_eq!(rig.selected(), (Some(review), Some(d(2026, 10, 14))));
        assert_eq!(rig.dialog(), None);
        // Empty grid, two hours further down the same day: the selection goes.
        let empty = pos2(block.center().x, block.max.y + 2.0 * HOUR_H);
        rig.click(empty, 1);
        assert_eq!(rig.selected(), (None, None));
        // A double click on an event opens it, in a window of its own (or in the form here, where
        // there are no such windows); on empty grid it starts a new appointment.
        rig.click(block.center(), 2);
        assert_eq!(rig.app.session.sel.event, Some(review));
        assert!(rig.app.windows.of_event(review).is_some() || rig.dialog() == Some("editEvent"), "{:?}", rig.dialog());
        rig.run("ui.closeDialog", json!({}));
        let _ = rig.app.execute("ui.closeWindow", json!({"all": true}));
        rig.frames(2);
        rig.click(empty, 2);
        assert_eq!((rig.app.session.sel.event, rig.dialog(), rig.app.windows.new_events()), (None, None, 1));
        rig.run("ui.closeWindow", json!({"all": true}));
        // The right button selects what is under it for the menu; nothing has changed yet.
        let lunch = rig.event("Lunch with Dev").id;
        rig.right_click(rig.at("Lunch with Dev").center());
        assert_eq!(rig.app.session.sel.event, Some(lunch));
        assert!(!rig.app.session.dirty);
        rig.key(egui::Key::Escape);
        // An occurrence of a series is selected with its day: Friday's stand-up.
        let friday = rig.at("Q4 roadmap").center().x;
        rig.click(pos2(friday, rig.at("Team stand-up").center().y), 1);
        assert_eq!(rig.selected(), (Some(rig.event("Team stand-up").id), Some(d(2026, 10, 16))));
    }

    #[test]
    fn right_click_on_a_picked_range_opens_a_menu_and_keeps_the_range() {
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let block = rig.at("Design review");
        let x = block.center().x;
        // Two hours of empty grid below the event, picked out by a drag.
        let (from, to) = (pos2(x, block.max.y + 2.0 * HOUR_H), pos2(x, block.max.y + 3.5 * HOUR_H));
        rig.drag(from, to);
        rig.drop_at(to);
        let picked = rig.app.hit.rect("slot", u64::try_from(d(2026, 10, 14).to_days()).unwrap()).expect("the range is picked");
        assert!(picked.height() > 1.5 * HOUR_H, "{picked:?}");
        // The right button inside it opens a menu, and the range stays.
        rig.right_click(picked.center());
        assert!(egui::Popup::is_any_open(&rig.ctx), "no menu opened");
        assert_eq!(rig.app.hit.rect("slot", u64::try_from(d(2026, 10, 14).to_days()).unwrap()), Some(picked));
        rig.key(egui::Key::Escape);
        rig.frames(2);
        assert!(!egui::Popup::is_any_open(&rig.ctx));
    }

    #[test]
    fn the_menu_over_a_range_opens_the_form_for_it() {
        let day = d(2026, 10, 14);
        let slot = Slot::timed(day, 14 * 60, 30);
        for (kind, form) in [("appointment", slot.form_for(Kind::Appointment)), ("meeting", slot.form_for(Kind::Meeting))] {
            assert_eq!((form["new"].as_str(), form["start"].as_str()), (Some(kind), Some("2026-10-14T14:00")));
        }
        let all_day = slot.form_for(Kind::AllDay);
        assert_eq!(
            (all_day["new"].as_str(), all_day["allDay"].as_bool(), all_day["start"].as_str()),
            (Some("appointment"), Some(true), Some("2026-10-14"))
        );
        // A click elsewhere in the picked range keeps it; one outside picks another.
        let mut long = Slot::timed(day, 14 * 60, 30);
        long.stretch_to(16 * 60);
        assert!(long.covers(&Slot::timed(day, 15 * 60, 30)));
        assert!(!long.covers(&Slot::timed(day, 17 * 60, 30)));
        assert!(!long.covers(&Slot::day(day)));
    }

    #[test]
    fn new_appointments_snap_to_the_half_hour() {
        let day = d(2026, 10, 14);
        assert_eq!(slot_at(day, 13 * 60 + 14, 30), (at(day, 13, 0), at(day, 13, 30)));
        assert_eq!(slot_at(day, 13 * 60 + 31, 30), (at(day, 13, 30), at(day, 14, 0)));
        assert_eq!(slot_at(day, 0, 30), (at(day, 0, 0), at(day, 0, 30)));
        // The last slot of the day ends at midnight.
        assert_eq!(slot_at(day, DAY_MINUTES, 30), (at(day, 23, 30), at(d(2026, 10, 15), 0, 0)));
        assert_eq!(slot_at(day, i64::MAX, 30).0, at(day, 23, 30));
        assert_eq!(slot_at(day, i64::MIN, 30).0, at(day, 0, 0));
        assert_eq!(iso_minute(slot_at(day, 9 * 60 + 45, 30).0), "2026-10-14T09:30");
        let _ = slot_at(d(9999, 12, 31), DAY_MINUTES, 30);
        // At another time scale the row is that long.
        assert_eq!(slot_at(day, 13 * 60 + 14, 60), (at(day, 13, 0), at(day, 14, 0)));
        assert_eq!(slot_at(day, 13 * 60 + 14, 5), (at(day, 13, 10), at(day, 13, 15)));
        assert_eq!(slot_at(day, DAY_MINUTES, 6).0, at(day, 23, 54));
        // A row of no length, or longer than the day.
        assert_eq!(slot_at(day, 61, 0), (at(day, 1, 1), at(day, 1, 2)));
        assert_eq!(slot_at(day, 61, i64::MAX), (at(day, 0, 0), at(d(2026, 10, 15), 0, 0)));
    }

    #[test]
    fn dragging_moves_an_event() {
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let review = rig.event("Design review").id;
        let block = rig.at("Design review");
        let column = rig.at("Q4 roadmap").min.x - rig.at("Lunch with Dev").min.x;
        // One column to the right and an hour and a half down: Thursday at 12:30.
        let (from, to) = (block.center(), block.center() + vec2(column, 1.5 * HOUR_H));
        rig.drag(from, to);
        assert_eq!(rig.target(), Some((at(d(2026, 10, 15), 12, 30), at(d(2026, 10, 15), 13, 30), false)));
        // Its ghost is where it would land, saying so.
        let ghost = rig.app.hit.rect("ghost", review).expect("the ghost is drawn");
        assert!(ghost.contains(to) && (ghost.height() - (HOUR_H - 2.0)).abs() < 1.0, "{ghost:?}");
        // Nothing moves until the button is let go.
        assert_eq!(rig.event("Design review").start, at(d(2026, 10, 14), 11, 0));
        rig.drop_at(to);
        let moved = rig.event("Design review");
        assert_eq!((moved.start, moved.end), (at(d(2026, 10, 15), 12, 30), at(d(2026, 10, 15), 13, 30)));
        assert_eq!(rig.app.session.undo_label(), Some("Move Event"));
        assert_eq!(rig.selected(), (Some(review), Some(d(2026, 10, 15))));
        assert_eq!(rig.dragging(), None);
        // It now shares Thursday lunchtime with another event: side by side, half the width each.
        let (a, b) = (rig.at("Design review"), rig.at("Lunch with Dev"));
        assert!(a.width() < 0.6 * block.width() && (a.width() - b.width()).abs() < 1.0 && !a.intersects(b), "{a:?} {b:?}");

        // A nudge is not a move.
        let before = rig.event("Q4 roadmap").start;
        let from = rig.at("Q4 roadmap").center();
        rig.drag(from, from + vec2(3.0, 5.0));
        rig.drop_at(from + vec2(3.0, 5.0));
        assert_eq!(rig.event("Q4 roadmap").start, before);
        // Escape while dragging leaves the event where it was.
        let to = from + vec2(0.0, 2.0 * HOUR_H);
        rig.drag(from, to);
        assert_eq!(rig.target().map(|t| t.0), Some(before.add_minutes(120)));
        rig.key(egui::Key::Escape);
        assert!(rig.target().is_none() && rig.app.hit.rect("ghost", rig.event("Q4 roadmap").id).is_none());
        rig.drop_at(to);
        assert_eq!(rig.event("Q4 roadmap").start, before);
        assert_eq!(rig.dragging(), None);

        // An item that repeats is asked about first: just this one, or the entire series?
        // The series moves whole: an hour later on every day, starting when it started.
        let series = rig.event("Team stand-up");
        let from = rig.at("Team stand-up").center();
        rig.drag(from, from + vec2(0.0, HOUR_H));
        rig.drop_at(from + vec2(0.0, HOUR_H));
        assert_eq!((rig.dialog(), rig.event("Team stand-up").start), (Some("confirm"), series.start), "nothing moves until it is answered");
        assert_eq!(rig.answer(1), ("calendar.move".to_string(), "Move Recurring Item".to_string()));
        let moved = rig.event("Team stand-up");
        assert_eq!(moved.start, series.start.add_minutes(60));
        assert_eq!((moved.duration_minutes(), moved.exceptions.len(), rig.dialog()), (series.duration_minutes(), 0, None));
        // It happens every weekday, and keeps its days: dropped in another day's column it is
        // that day's alone that goes there, and there is nothing to ask. Monday's, to Saturday.
        let saturday = d(2026, 10, 17);
        let from = rig.at("Team stand-up").center();
        let to = pos2(rig.day("allDay", saturday).unwrap().center().x, from.y + 0.5 * HOUR_H);
        rig.drag(from, to);
        assert_eq!(rig.target(), Some((at(saturday, 11, 0), at(saturday, 11, 15), false)));
        rig.drop_at(to);
        let stand_ups = rig.all("Team stand-up");
        assert_eq!((rig.dialog(), stand_ups.len()), (None, 2));
        assert_eq!(stand_ups[0], (series.id, moved.start, moved.end, true));
        assert_eq!((stand_ups[1].1, stand_ups[1].2, stand_ups[1].3), (at(saturday, 11, 0), at(saturday, 11, 15), false));
        assert_eq!(rig.app.session.mailbox.event(series.id).map(|e| e.exceptions.clone()), Some(vec![d(2026, 10, 12)]));
        assert_eq!(rig.selected(), (Some(stand_ups[1].0), Some(saturday)));
        // A weekly one on its own day goes to the day it is dropped on: Wednesdays from now on.
        let weekly = rig.event("1:1 with Priya");
        let from = rig.at("1:1 with Priya").center();
        rig.drag(from, from + vec2(column, 0.0));
        rig.drop_at(from + vec2(column, 0.0));
        rig.answer(1);
        assert_eq!(rig.all("1:1 with Priya"), [(weekly.id, weekly.start.add_days(1), weekly.end.add_days(1), true)]);
        // Just this one: that day's is an event of its own where it was dropped, a day on and
        // half an hour later; the series is as it was, less that day.
        let from = rig.at("1:1 with Priya").center();
        rig.drag(from, from + vec2(column, 0.5 * HOUR_H));
        rig.drop_at(from + vec2(column, 0.5 * HOUR_H));
        assert_eq!(rig.answer(0).0, "calendar.move");
        let (wednesday, thursday) = (d(2026, 10, 14), d(2026, 10, 15));
        let now = rig.all("1:1 with Priya");
        assert_eq!(now.len(), 2, "{now:?}");
        assert_eq!(now[0], (weekly.id, weekly.start.add_days(1), weekly.end.add_days(1), true));
        assert_eq!((now[1].1, now[1].2, now[1].3), (at(thursday, 14, 30), at(thursday, 15, 0), false));
        assert_eq!(rig.app.session.mailbox.event(weekly.id).map(|e| e.exceptions.clone()), Some(vec![wednesday]));
        assert_eq!(rig.selected().0, Some(now[1].0));
    }

    #[test]
    fn the_month_says_more_and_the_side_pane_navigates() {
        let mut rig = Rig::sample();
        // A Tuesday with more on than a month cell has rows for.
        let tuesday = d(2026, 10, 13);
        for hour in 8..18u8 {
            rig.app.execute("calendar.new", json!({"title": format!("Slot {hour}"), "start": iso_minute(at(tuesday, hour, 0))})).unwrap();
        }
        rig.run("view.month", json!({}));
        let more = rig.day("more", tuesday).expect("the 13th says +n more");
        assert!(rig.day("more", d(2026, 10, 14)).is_none());
        rig.click(more.center(), 1);
        assert_eq!((rig.app.session.view.calendar, rig.app.session.view.date), (CalView::Day, tuesday));
        // (Yesterday's ten appointments have their reminders due, and the window that says so
        // comes up over the side pane.)
        rig.run("ui.dismissReminder", json!({"all": true}));
        // A day in the date navigator goes there, keeping the view.
        let day = d(2026, 10, 22);
        rig.click(rig.day("miniDay", day).unwrap().center(), 1);
        assert_eq!((rig.app.session.view.calendar, rig.app.session.view.date), (CalView::Day, day));
        // Its check box hides the calendar, and with it the events.
        let calendar = rig.app.session.mailbox.calendars.first().map(|c| c.id).unwrap();
        let planning = rig.event("Planning days").id;
        assert!(rig.app.hit.rect("event", planning).is_some());
        rig.click(rig.app.hit.rect("calendar", calendar).unwrap().center(), 1);
        assert!(!rig.app.session.mailbox.calendars.iter().any(|c| c.visible));
        assert!(rig.app.hit.rect("event", planning).is_none());
    }

    #[test]
    fn hostile_calendars_draw_without_panicking() {
        let mut mailbox = mailcraft_engine::sample::mailbox(NOW, 0);
        let today = d(2026, 10, 14);
        // Thousands of events at once today, and thousands of whole-day ones.
        for i in 0..2500u32 {
            let _ = mailbox.add_event(Event { title: format!("Flood {i}"), start: at(today, 9, 0), end: at(today, 9, 30), ..Default::default() });
            let _ = mailbox.add_event(Event {
                title: format!("Whole day {i}"),
                all_day: true,
                start: at(today, 0, 0),
                end: at(today.add_days(1), 0, 0),
                ..Default::default()
            });
        }
        // An end before the start, no length, no title, a megabyte of title, text that isn't one line.
        let _ = mailbox.add_event(Event { title: "Backwards".into(), start: at(today, 13, 0), end: at(d(2026, 10, 1), 9, 0), ..Default::default() });
        let _ =
            mailbox.add_event(Event { title: String::new(), start: at(today, 14, 0), end: at(today, 14, 0), cancelled: true, ..Default::default() });
        let _ = mailbox.add_event(Event {
            title: "x\n\u{202e}é".repeat(200_000),
            location: "\n".repeat(5000),
            start: at(today, 15, 0),
            end: at(today, 15, 1),
            ..Default::default()
        });
        // From the first day of the calendar to the last, and a series that runs into its end.
        let _ =
            mailbox.add_event(Event { title: "Forever".into(), start: at(d(1, 1, 1), 0, 0), end: at(d(9999, 12, 31), 23, 59), ..Default::default() });
        let every_day = Recurrence::every(Freq::Daily);
        let _ = mailbox.add_event(Event {
            title: "Last days".into(),
            start: at(d(9999, 12, 1), 23, 0),
            end: at(d(9999, 12, 2), 1, 0),
            recurrence: Some(every_day),
            ..Default::default()
        });
        let mut rig = Rig::new(mailbox);
        // Preferences nobody would set.
        rig.app.session.prefs.work_days.clear();
        rig.app.session.prefs.work_start = 200;
        rig.app.session.prefs.work_end = 3;
        rig.app.session.prefs.twelve_hour = false;
        for date in ["2026-10-14", "9999-12-31", "0001-01-01"] {
            rig.run("view.goToDate", json!({"date": date}));
            for view in ["view.day", "view.workWeek", "view.week", "view.month"] {
                rig.run(view, json!({}));
            }
        }
        // What didn't fit is counted, not drawn: the flooded day says so.
        rig.run("view.goToDate", json!({"date": "2026-10-14"}));
        rig.run("view.week", json!({}));
        assert!(rig.day("more", today).is_some());
        // A drag left over from a view that is gone is forgotten.
        let far = at(d(9999, 12, 31), 23, 59);
        let stale = Drag {
            event: 0,
            target: Some(Span { start: far, end: at(d(1, 1, 1), 0, 0), all_day: true }),
            ..grabbed(at(today, 9, 0), far, false, Grip::End, Under::Time { day: d(1, 1, 1), minute: i64::MAX })
        };
        rig.ctx.data_mut(|data| data.insert_temp(egui::Id::unique(DRAG_ID), stale));
        rig.frames(2);
        assert_eq!(rig.dragging(), None);
        // A question about a drop of an event that is gone is not asked.
        let ask = DropMenu { drag: stale, span: stale.here(), pos: pos2(f32::NAN, -1.0e9) };
        rig.ctx.data_mut(|data| data.insert_temp(egui::Id::unique(DROP_MENU_ID), ask));
        rig.frames(2);
        assert!(rig.ctx.data(|data| data.get_temp::<DropMenu>(egui::Id::unique(DROP_MENU_ID))).is_none());
        // And so is a pick of days that are not in view, or that nothing draws any more.
        rig.ctx.data_mut(|data| data.insert_temp(egui::Id::unique(SLOT_ID), Slot::timed(d(9999, 12, 31), i64::MAX, i64::MAX)));
        rig.frames(3);
        assert_eq!(rig.slot(), None);
        // A pick from the first day of the calendar to the last, with text nobody would type.
        for view in ["view.day", "view.week", "view.month"] {
            let mut odd = Slot::day(d(1, 1, 1));
            odd.stretch_to_day(d(9999, 12, 31));
            odd.text = Some("x\n\u{202e}é".repeat(2_000));
            odd.frame = u64::MAX;
            rig.ctx.data_mut(|data| data.insert_temp(egui::Id::unique(SLOT_ID), odd));
            rig.run(view, json!({}));
        }
    }

    impl Rig {
        /// Type `text`, a character a frame, as a keyboard does.
        fn type_text(&mut self, text: &str) {
            for c in text.chars() {
                self.frame(vec![egui::Event::Text(c.to_string())]);
            }
            self.frames(1);
        }

        /// The stretch of empty calendar that is picked out.
        fn slot(&self) -> Option<Slot> {
            load_slot(&self.ctx)
        }

        /// The stretch that is picked out, as (start, end, whole days, what is typed).
        fn picked(&self) -> Option<(DateTime, DateTime, bool, Option<String>)> {
            self.slot().map(|s| (s.start, s.end, s.all_day, s.text))
        }

        /// The sample calendar in the Week view, with an account whose Microsoft server (a
        /// stand-in) has `events`, already brought down.
        fn served(
            events: Vec<mailcraft_model::remote::RemoteEvent>,
        ) -> (Rig, std::sync::Arc<std::sync::Mutex<crate::calendar_sync::testing::Server>>, Id) {
            let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
            app.autosave = false;
            let server = crate::calendar_sync::testing::serve(&mut app);
            let account = crate::calendar_sync::testing::microsoft_account(&mut app);
            server.lock().unwrap().events = events;
            app.execute("view.calendar", json!({})).unwrap();
            app.execute("view.week", json!({})).unwrap();
            let mut rig = Rig { ctx: egui::Context::default(), app, time: 0.0, mods: egui::Modifiers::NONE, cursor: egui::CursorIcon::Default };
            rig.frames(8);
            (rig, server, account)
        }
    }

    /// A point in the Wednesday column of the sample week, `hours` after the end of the design
    /// review (which ends at noon) and a little into that half hour.
    fn after_noon(rig: &Rig, hours: f32) -> egui::Pos2 {
        let block = rig.at("Design review");
        pos2(block.center().x, block.max.y + hours * HOUR_H + 6.0)
    }

    #[test]
    fn typing_in_a_picked_slot_makes_an_appointment_there() {
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let wednesday = d(2026, 10, 14);
        let at_two = after_noon(&rig, 2.0);
        // A click on empty grid picks the half hour under it out, and shows it.
        rig.click(at_two, 1);
        assert_eq!(rig.picked(), Some((at(wednesday, 14, 0), at(wednesday, 14, 30), false, None)));
        assert_eq!(rig.selected(), (None, None));
        let mark = rig.day("slot", wednesday).expect("the pick is drawn");
        assert!(mark.contains(at_two) && (mark.height() - HOUR_H / 2.0).abs() < 3.0, "{mark:?}");
        // Typing starts an entry right there. Nothing is made yet, and no dialog comes up.
        let before = rig.app.session.mailbox.events.len();
        rig.type_text("Lunch w");
        assert_eq!(rig.slot().and_then(|s| s.text), Some("Lunch w".to_string()));
        assert_eq!((rig.app.session.mailbox.events.len(), rig.dialog()), (before, None));
        rig.type_text("ith Bo");
        // Enter makes the appointment, for the time that was picked, as a new appointment is:
        // busy, with the usual reminder.
        rig.key(egui::Key::Enter);
        rig.frames(2);
        let made = rig.event("Lunch with Bo");
        assert_eq!((made.title.as_str(), made.start, made.end, made.all_day), ("Lunch with Bo", at(wednesday, 14, 0), at(wednesday, 14, 30), false));
        assert_eq!((made.show_as, made.reminder_minutes), (ShowAs::Busy, Some(15)));
        assert_eq!(rig.app.session.mailbox.events.len(), before + 1);
        // It is selected in the slot's place, and it is one step to undo.
        assert_eq!((rig.selected().0, rig.slot()), (Some(made.id), None));
        assert_eq!(rig.app.session.undo_label(), Some("New Appointment"));
        // Typing with nothing picked out goes nowhere.
        rig.type_text("stray");
        assert_eq!(rig.app.session.mailbox.events.len(), before + 1);
        // Enter on a slot nothing was typed into opens a new appointment for it, in a window of
        // its own: saved from there, it is at the time that was picked.
        rig.click(after_noon(&rig, 4.0), 1);
        rig.key(egui::Key::Enter);
        assert_eq!((rig.dialog(), rig.slot(), rig.app.windows.new_events()), (None, None, 1));
        assert_eq!(rig.app.session.mailbox.events.len(), before + 1);
        rig.frames(3);
        rig.type_text("From the window");
        rig.key(egui::Key::Enter);
        rig.frames(2);
        let made = rig.event("From the window");
        assert_eq!((made.start, made.end, rig.app.windows.new_events()), (at(wednesday, 16, 0), at(wednesday, 16, 30), 0));
        // While such a window lies over the calendar, what is typed is the window's.
        rig.click(after_noon(&rig, 3.0), 1);
        rig.app.execute("ui.openWindow", json!({"new": "meeting"})).unwrap();
        rig.frames(3);
        rig.type_text("y");
        assert_eq!(rig.slot().and_then(|s| s.text), None);
        rig.run("ui.closeWindow", json!({"all": true}));
        // With a dialog open, what is typed is the dialog's.
        rig.click(after_noon(&rig, 3.0), 1);
        rig.run("ui.dialog", json!({"name": "goToDate"}));
        rig.type_text("x");
        rig.run("ui.closeDialog", json!({}));
        assert_eq!(rig.slot().and_then(|s| s.text), None);
        assert_eq!(rig.app.session.mailbox.events.len(), before + 2);
    }

    #[test]
    fn a_drag_over_slots_picks_them_all_and_escape_or_a_click_away_end_the_entry() {
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let wednesday = d(2026, 10, 14);
        // Dragged from one o'clock down to past half past two: two hours are picked.
        let (from, to) = (after_noon(&rig, 1.0), after_noon(&rig, 2.5));
        rig.drag(from, to);
        assert_eq!(rig.picked(), Some((at(wednesday, 13, 0), at(wednesday, 15, 0), false, None)));
        rig.drop_at(to);
        assert_eq!(rig.picked(), Some((at(wednesday, 13, 0), at(wednesday, 15, 0), false, None)));
        assert!(rig.day("slot", wednesday).is_some_and(|mark| (mark.height() - 2.0 * HOUR_H).abs() < 3.0));
        // Upwards works the same, from where the drag began.
        rig.drag(to, from);
        rig.drop_at(from);
        assert_eq!(rig.picked().map(|s| (s.0, s.1)), Some((at(wednesday, 13, 0), at(wednesday, 15, 0))));
        // The entry covers what was picked.
        let before = rig.app.session.mailbox.events.len();
        rig.type_text("Workshop");
        rig.key(egui::Key::Enter);
        rig.frames(2);
        let made = rig.event("Workshop");
        assert_eq!((made.start, made.end), (at(wednesday, 13, 0), at(wednesday, 15, 0)));

        // Escape gives up what was typed; the slot stays picked. Once more, and the pick goes.
        let (noon, half_past, later) = (after_noon(&rig, 0.0), after_noon(&rig, 0.5), after_noon(&rig, 3.0));
        rig.click(later, 1);
        rig.type_text("Never mind");
        assert_eq!(rig.slot().and_then(|s| s.text), Some("Never mind".to_string()));
        rig.key(egui::Key::Escape);
        assert_eq!(rig.picked(), Some((at(wednesday, 15, 0), at(wednesday, 15, 30), false, None)));
        assert_eq!(rig.app.session.mailbox.events.len(), before + 1);
        rig.key(egui::Key::Escape);
        assert_eq!(rig.slot(), None);

        // A click elsewhere keeps what was typed: it becomes the appointment, as in Outlook.
        rig.click(later, 1);
        rig.type_text("Call Priya");
        rig.click(noon, 1);
        let made = rig.event("Call Priya");
        assert_eq!((made.start, made.end), (at(wednesday, 15, 0), at(wednesday, 15, 30)));
        // And the click picked the slot it landed on.
        assert_eq!(rig.picked(), Some((at(wednesday, 12, 0), at(wednesday, 12, 30), false, None)));
        // Nothing typed but spaces, and a click elsewhere: nothing is made.
        let count = rig.app.session.mailbox.events.len();
        rig.type_text("  ");
        rig.click(half_past, 1);
        assert_eq!(rig.app.session.mailbox.events.len(), count);
        assert_eq!(rig.picked().map(|s| s.0), Some(at(wednesday, 12, 30)));
        // Picking an event lets go of the slot.
        rig.click(rig.at("Design review").center(), 1);
        rig.frames(2);
        assert_eq!(rig.slot(), None);
        // Another week: the pick doesn't follow.
        rig.click(noon, 1);
        assert!(rig.slot().is_some());
        rig.run("view.next", json!({}));
        rig.frames(2);
        assert_eq!(rig.slot(), None);
    }

    #[test]
    fn days_are_picked_and_typed_into_in_the_month_and_the_all_day_row() {
        let mut rig = Rig::sample();
        rig.run("view.month", json!({}));
        let (tuesday, thursday) = (d(2026, 10, 27), d(2026, 10, 29));
        let cell = |rig: &Rig, day: Date| rig.day("day", day).unwrap_or_else(|| panic!("{} isn't drawn", day.iso()));
        // The bottom of a cell is empty calendar.
        let low = |rig: &Rig, day: Date| pos2(cell(rig, day).center().x, cell(rig, day).max.y - 5.0);
        rig.click(low(&rig, tuesday), 1);
        assert_eq!(rig.picked(), Some((at(tuesday, 0, 0), at(d(2026, 10, 28), 0, 0), true, None)));
        assert!(rig.day("slot", tuesday).is_some());
        rig.type_text("Offsite");
        rig.key(egui::Key::Enter);
        rig.frames(2);
        // A whole-day event, as Outlook makes one: free, and reminding the afternoon before.
        let made = rig.event("Offsite");
        assert_eq!((made.all_day, made.start, made.end), (true, at(tuesday, 0, 0), at(d(2026, 10, 28), 0, 0)));
        assert_eq!((made.show_as, made.reminder_minutes), (ShowAs::Free, Some(18 * 60)));
        assert_eq!((rig.selected().0, rig.slot()), (Some(made.id), None));
        // A drag over days picks them all, and the entry covers them.
        rig.drag(low(&rig, tuesday), low(&rig, thursday));
        rig.drop_at(low(&rig, thursday));
        assert_eq!(rig.picked(), Some((at(tuesday, 0, 0), at(d(2026, 10, 30), 0, 0), true, None)));
        rig.type_text("Conference");
        rig.key(egui::Key::Enter);
        rig.frames(2);
        let made = rig.event("Conference");
        assert_eq!((made.all_day, made.start, made.end), (true, at(tuesday, 0, 0), at(d(2026, 10, 30), 0, 0)));
        // A double click on a day opens a new appointment for it, in a window of its own.
        rig.click(low(&rig, d(2026, 10, 20)), 2);
        assert_eq!((rig.dialog(), rig.app.windows.new_events()), (None, 1));
        rig.run("ui.closeWindow", json!({"all": true}));

        // The all-day row of the week does the same for its days.
        rig.run("view.goToDate", json!({"date": "2026-10-14"}));
        rig.run("view.week", json!({}));
        let wednesday = d(2026, 10, 14);
        let count = rig.app.session.mailbox.events.len();
        let row = rig.day("allDay", wednesday).expect("the row of whole days");
        rig.click(pos2(row.center().x, row.max.y - 3.0), 1);
        assert_eq!(rig.picked(), Some((at(wednesday, 0, 0), at(d(2026, 10, 15), 0, 0), true, None)));
        assert!(rig.day("slot", wednesday).is_some(), "the row shows the pick");
        rig.type_text("Holiday");
        rig.key(egui::Key::Enter);
        rig.frames(2);
        let made = rig.event("Holiday");
        assert_eq!((made.all_day, made.start.date, rig.app.session.mailbox.events.len()), (true, wednesday, count + 1));
        // The hours below have their own pick: one replaces the other.
        let thursday = d(2026, 10, 15);
        let row = rig.day("allDay", thursday).expect("the row of whole days");
        rig.click(pos2(row.center().x, row.max.y - 3.0), 1);
        assert_eq!(rig.picked().map(|s| (s.0, s.2)), Some((at(thursday, 0, 0), true)));
        rig.click(after_noon(&rig, 2.0), 1);
        assert_eq!(rig.picked().map(|s| (s.0, s.2)), Some((at(wednesday, 14, 0), false)));
        rig.click(pos2(row.center().x, row.max.y - 3.0), 1);
        assert_eq!(rig.picked().map(|s| (s.0, s.2)), Some((at(thursday, 0, 0), true)));
    }

    /// What is kept on this computer only says so beside the account's calendar: under the
    /// header, on its block and in the list; and one action moves it to the account's calendar.
    #[test]
    fn what_is_only_on_this_computer_says_so_and_moves_to_the_accounts_calendar() {
        use crate::calendar_sync::testing::event as on_server;
        use crate::dialogs::Dialog;
        let (mut rig, _server, _account) = Rig::served(vec![on_server("e-1", "Dentist", NOW + 18 * 3600)]);
        let mailbox = &rig.app.session.mailbox;
        let own = mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).unwrap();
        let here = mailbox.events.values().filter(|e| e.calendar == own).count();
        let meetings = mailbox.events.values().filter(|e| e.calendar == own && !e.attendees.is_empty()).count();
        assert!(meetings > 0 && here > meetings);
        // Under the header: how many, and the way out. In the list: a line under the calendar.
        let button = rig.app.hit.rect("calendarLocalOnly", here as u64).expect("the line under the header");
        let (row, line) = (rig.app.hit.rect("calendar", own).unwrap(), rig.app.hit.rect("calendarLocalLine", own).expect("the line in the list"));
        assert!(line.min.y >= row.max.y - 1.0 && line.height() >= 20.0, "{row:?} {line:?}");
        // The calendar is all there, under the line.
        assert!(rig.at("Design review").min.y > button.max.y);
        // The calendars of the server have no such line.
        let server_calendar = rig.app.session.mailbox.calendars.iter().find(|c| !c.origin.is_local()).map(|c| c.id).unwrap();
        assert!(rig.app.hit.rect("calendarLocalLine", server_calendar).is_none());

        // The button asks first: it puts things on the server, and meetings would invite people.
        rig.click(button.center(), 1);
        let Some(Dialog::Confirm { command, params, ask }) = rig.app.dialog.take() else { panic!("no question is being asked") };
        assert_eq!(
            (command.as_str(), ask.title.as_str(), ask.action.as_str()),
            ("calendar.moveToAccount", "Move to Your Account's Calendar", "Move All and Send Invitations")
        );
        assert_eq!(ask.more.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), ["Move the Appointments Only"]);
        assert!(ask.text.contains("not in your Outlook calendar") && ask.text.contains("send them the invitation"), "{}", ask.text);
        assert_eq!(rig.app.session.mailbox.events.values().filter(|e| e.calendar == own).count(), here);
        // The appointments only: they are the account's now, and the meetings stay and say so.
        let with = ask.more[0].1.clone();
        crate::dialogs::go_ahead(&mut rig.app, &command, crate::dialogs::answered(&params, &with, None));
        let said = rig.app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
        assert_eq!(said, format!("{} items moved to your account's calendar; {} stay on this computer", here - meetings, meetings));
        rig.frames(3);
        assert_eq!(rig.app.session.mailbox.events.values().filter(|e| e.calendar == own).count(), meetings);
        assert!(rig.app.hit.rect("calendarLocalOnly", meetings as u64).is_some() && rig.app.hit.rect("calendarLocalOnly", here as u64).is_none());
        // With the invitations: nothing is left on this computer, the calendar goes, and so do the lines.
        rig.run("calendar.moveToAccount", json!({"send": true}));
        rig.frames(3);
        assert!(rig.app.session.mailbox.calendar(own).is_none());
        assert!(rig.app.hit.rect("calendarLocalLine", own).is_none() && rig.app.hit.rect("calendarLocalOnly", meetings as u64).is_none());
        assert!(mailcraft_engine::cmd::remote::only_here(&rig.app.session.mailbox).is_empty());
    }

    #[test]
    fn new_items_go_to_the_calendar_whose_block_is_worked_in() {
        use crate::calendar_sync::testing::event as on_server;
        let (mut rig, server, account) = Rig::served(vec![on_server("e-1", "Dentist", NOW + 18 * 3600)]);
        let mailbox = &rig.app.session.mailbox;
        let of_server = mailbox.calendars.iter().find(|c| c.origin.account == account).map(|c| c.id).expect("the server's calendar");
        let own = mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).expect("the mailbox's own");
        // Under the account, simply "Calendar"; the mailbox's own says where it lives.
        let names: Vec<(&str, bool)> = mailbox.calendars.iter().map(|c| (c.name.as_str(), c.origin.is_local())).collect();
        assert_eq!(names, [("On this computer", true), ("Calendar", false)]);
        assert_eq!(calendar_label(&rig.app.session, mailbox.calendar(of_server).unwrap()), "Calendar (avery@mail.example)");
        assert_eq!(calendar_label(&rig.app.session, mailbox.calendar(own).unwrap()), "On this computer");
        // Nothing was picked: the server's is the one being worked in, so what the ribbon makes
        // reaches it.
        let current = |rig: &Rig| mailcraft_engine::cmd::calendar::current_calendar(&rig.app.session);
        assert_eq!(current(&rig), of_server);
        // Both are shown, side by side: the account's own first, then what is kept here.
        let (theirs, ours) = (rig.app.hit.rect("pane", of_server).unwrap(), rig.app.hit.rect("pane", own).unwrap());
        assert!(theirs.max.x <= ours.min.x, "{theirs:?} {ours:?}");
        // What is typed in the server's block is the server's, at once.
        let at_two = after_noon(&rig, 2.0);
        rig.click(pos2(rig.column(of_server, 3, 7), at_two.y), 1);
        assert_eq!(rig.slot().map(|s| s.calendar), Some(of_server));
        rig.type_text("To the server");
        rig.key(egui::Key::Enter);
        rig.frames(6);
        let made = rig.event("To the server");
        assert_eq!((made.calendar, made.origin.account, made.origin.id.as_str()), (of_server, account, "made-1"));
        assert_eq!(server.lock().unwrap().events.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Dentist", "To the server"]);
        assert!(rig.app.session.mailbox.event_changes.is_empty());
        // What is typed in the other block stays on this computer, and that calendar is the one
        // being worked in from then on: the form starts with it.
        rig.click(after_noon(&rig, 3.0), 1);
        assert_eq!((rig.slot().map(|s| s.calendar), rig.app.session.view.in_calendar), (Some(own), own));
        rig.type_text("Stays here");
        rig.key(egui::Key::Enter);
        rig.frames(6);
        let made = rig.event("Stays here");
        assert_eq!((made.calendar, made.origin.is_local()), (own, true));
        assert_eq!(server.lock().unwrap().events.len(), 2);
        let in_form = |rig: &Rig| match crate::forms::Form::open("event", &rig.app, &json!({"title": "x"})) {
            Some(crate::forms::Form::Event(form)) => form.command().1["calendar"].as_u64(),
            _ => None,
        };
        assert_eq!(in_form(&rig), Some(own));
        // A click on a calendar's name in the list makes it the one being worked in; its check
        // box is another matter, and still shows and hides it.
        let (name, check) = (rig.app.hit.rect("calendarName", of_server).unwrap(), rig.app.hit.rect("calendar", of_server).unwrap());
        assert!(name.min.x >= check.max.x, "{name:?} {check:?}");
        rig.click(name.center(), 1);
        assert_eq!((rig.app.session.view.in_calendar, current(&rig), in_form(&rig)), (of_server, of_server, Some(of_server)));
        assert!(rig.app.session.mailbox.calendars.iter().all(|c| c.visible));
        rig.click(check.center(), 1);
        assert_eq!(rig.app.session.mailbox.calendar(of_server).map(|c| c.visible), Some(false));
        // One calendar is left in view: it has no tab, and what is typed there is its own,
        // whichever calendar was worked in before.
        assert!(rig.app.hit.rect("paneTab", own).is_none() && rig.app.hit.rect("pane", own).is_none());
        rig.click(after_noon(&rig, 1.0), 1);
        rig.type_text("Alone");
        rig.key(egui::Key::Enter);
        rig.frames(6);
        assert_eq!((rig.event("Alone").calendar, rig.app.session.view.in_calendar), (own, own));
        rig.click(check.center(), 1);
        assert_eq!(rig.app.session.mailbox.calendar(of_server).map(|c| c.visible), Some(true));
        // The stranded event is put on the server from here: one event, the server's from now on.
        rig.app.run("calendar.edit", json!({"id": made.id, "calendar": of_server})).unwrap();
        rig.frames(6);
        let moved = rig.event("Stays here");
        assert_eq!((moved.id, moved.calendar, moved.origin.id.as_str()), (made.id, of_server, "made-2"));
        assert_eq!(rig.app.session.mailbox.events.values().filter(|e| e.title == "Stays here").count(), 1);
    }

    #[test]
    fn a_dragged_event_goes_to_its_server_and_somebody_elses_meeting_is_asked_about_first() {
        use crate::calendar_sync::testing::event as on_server;
        use mailcraft_model::{Address, Attendee};
        // The owner's own appointment at noon, and a meeting Bo organizes at two.
        let meeting = mailcraft_model::remote::RemoteEvent {
            organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
            attendees: vec![Attendee { address: Address::new("Avery", "avery@mail.example"), ..Default::default() }],
            is_organizer: Some(false),
            ..on_server("e-2", "Board meeting", NOW - 2 * 3600)
        };
        let (mut rig, server, _) = Rig::served(vec![on_server("e-1", "Haircut", NOW - 4 * 3600), meeting]);
        let wednesday = d(2026, 10, 14);
        let server_start = |server: &std::sync::Arc<std::sync::Mutex<crate::calendar_sync::testing::Server>>, id: &str| {
            server.lock().unwrap().events.iter().find(|e| e.id == id).map(|e| e.start).unwrap()
        };

        // The owner's own: dragged an hour down, it moves at once, and the server has it.
        let from = rig.at("Haircut").center();
        rig.drag(from, from + vec2(0.0, HOUR_H));
        rig.drop_at(from + vec2(0.0, HOUR_H));
        assert_eq!(rig.dialog(), None);
        assert_eq!(rig.event("Haircut").start, at(wednesday, 13, 0));
        rig.frames(6);
        assert_eq!(server_start(&server, "e-1"), NOW - 3 * 3600);
        assert!(rig.app.session.mailbox.event_changes.is_empty());
        let sent: Vec<_> = server.lock().unwrap().jobs.iter().flat_map(|job| job.ops.clone()).collect();
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0].kind, sent[0].remote.as_str()), (mailcraft_model::remote::EventChangeKind::Update, "e-1"));
        assert_eq!(sent[0].fields.time.map(|t| (t.start, t.end, t.all_day)), Some((NOW - 3 * 3600, NOW - 2 * 3600, false)));

        // Somebody else's meeting: the drag is asked about first, and nothing has moved.
        let board = rig.event("Board meeting");
        assert!(board.origin.guest);
        let from = rig.at("Board meeting").center();
        rig.drag(from, from + vec2(0.0, HOUR_H));
        rig.drop_at(from + vec2(0.0, HOUR_H));
        assert_eq!(rig.dialog(), Some("confirm"));
        assert_eq!(rig.event("Board meeting").start, at(wednesday, 14, 0));
        assert_eq!(rig.selected(), (Some(board.id), Some(wednesday)));
        let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = rig.app.dialog.take() else { panic!("a question was to be asked") };
        assert_eq!((command.as_str(), ask.title.as_str(), ask.action.as_str()), ("calendar.move", "Change Your Copy of This Meeting?", "OK"));
        assert_eq!(
            ask.text,
            "Bo Marsh organizes this meeting; you are invited to it. What you change is changed in your calendar only. The organizer and the other people invited are not told and still have the meeting as it was, so you could miss it. An update from the organizer can replace your change."
        );
        // Cancel: nothing happens, and nothing goes to the server.
        rig.frames(6);
        assert_eq!((rig.event("Board meeting").start, server_start(&server, "e-2")), (at(wednesday, 14, 0), NOW - 2 * 3600));
        // OK: the copy here moves, and the server has that too.
        let mut confirmed = params.clone();
        confirmed["confirmed"] = json!(true);
        rig.app.run(&command, confirmed).unwrap();
        rig.frames(6);
        assert_eq!((rig.event("Board meeting").start, server_start(&server, "e-2")), (at(wednesday, 15, 0), NOW - 3600));
        assert_eq!(rig.dialog(), None);

        // The same question for a delete, in its own words; and for an edit of what others see.
        let asked = |rig: &mut Rig, command: &str, params: Value| {
            let answer = rig.app.run(command, params).unwrap();
            let question = match rig.app.dialog.take() {
                Some(crate::dialogs::Dialog::Confirm { ask, .. }) => Some((ask.title, ask.action)),
                _ => None,
            };
            (answer.get("pending").and_then(Value::as_str).map(str::to_string), question)
        };
        let (pending, question) = asked(&mut rig, "calendar.delete", json!({"id": board.id}));
        assert_eq!(pending.as_deref(), Some("confirm"));
        assert_eq!(question, Some(("Delete Your Copy of This Meeting?".to_string(), "Delete My Copy".to_string())));
        assert!(rig.app.session.mailbox.event(board.id).is_some());
        let (pending, question) = asked(&mut rig, "calendar.edit", json!({"id": board.id, "title": "Board"}));
        assert_eq!((pending.as_deref(), question.map(|q| q.0)), (Some("confirm"), Some("Change Your Copy of This Meeting?".to_string())));
        // How it shows in the owner's own calendar is nobody else's business: no question.
        let (pending, question) = asked(&mut rig, "calendar.showAs", json!({"id": board.id, "value": "free"}));
        assert_eq!((pending, question), (None, None));
        let (pending, question) = asked(&mut rig, "calendar.edit", json!({"id": board.id, "reminder": 5}));
        assert_eq!((pending, question), (None, None));
        // Nor is the owner's own appointment asked about, or anything a script does.
        let haircut = rig.event("Haircut").id;
        let (pending, _) = asked(&mut rig, "calendar.move", json!({"id": haircut, "start": "2026-10-14T19:00"}));
        assert_eq!(pending, None);
        rig.app.execute("calendar.move", json!({"id": board.id, "start": "2026-10-14T16:00"})).unwrap();
        assert_eq!((rig.dialog(), rig.event("Board meeting").start), (None, at(wednesday, 16, 0)));
        rig.frames(6);
        assert_eq!(server_start(&server, "e-2"), NOW);
    }

    #[test]
    fn edges_and_whole_days_go_where_the_pointer_is() {
        let (wed, thu, fri) = (d(2026, 10, 14), d(2026, 10, 15), d(2026, 10, 16));
        let (start, end) = (at(wed, 11, 0), at(wed, 12, 0));
        let over = |drag: &Drag, under: Under| drag.span(under, 15, 30);
        let time = |day: Date, minute: i64| Under::Time { day, minute };
        // The lower edge, grabbed a few minutes above the end, goes down an hour and a bit.
        let lower = grabbed(start, end, false, Grip::End, time(wed, 716));
        assert_eq!(over(&lower, time(wed, 786)), Span { start, end: at(wed, 13, 15), all_day: false });
        // Over another day it is the same: the end stays on its day.
        assert_eq!(over(&lower, time(fri, 786)).end, at(wed, 13, 15));
        // Never shorter than a step, never past midnight, and a nudge leaves it as it is.
        assert_eq!(over(&lower, time(wed, 0)).end, at(wed, 11, 15));
        assert_eq!(over(&lower, time(wed, i64::MAX)).end, at(thu, 0, 0));
        assert_eq!(over(&lower, time(wed, 720)), lower.here());
        // The upper edge likewise, never less than a step before the end.
        let upper = grabbed(start, end, false, Grip::Start, time(wed, 662));
        assert_eq!(over(&upper, time(wed, 572)), Span { start: at(wed, 9, 30), end, all_day: false });
        assert_eq!(over(&upper, time(wed, 1400)).start, at(wed, 11, 45));
        assert_eq!(over(&upper, time(wed, i64::MIN)).start, at(wed, 0, 0));
        // An event that ends at midnight ends on the day before, and that is where its edge is.
        let late = grabbed(at(wed, 22, 0), at(thu, 0, 0), false, Grip::End, time(wed, 1436));
        assert_eq!(over(&late, time(wed, 1376)).end, at(wed, 23, 0));
        // An edge has nothing to do in the row of whole days.
        assert_eq!(over(&lower, Under::Day(fri)), lower.here());

        // A timed event dropped in the row of whole days takes that day; a series stays in the hours.
        let moved = grabbed(start, end, false, Grip::Move, time(wed, 690));
        assert_eq!(over(&moved, Under::Day(fri)), Span { start: at(fri, 0, 0), end: at(d(2026, 10, 17), 0, 0), all_day: true });
        assert_eq!(over(&Drag { repeats: true, ..moved }, Under::Day(fri)), moved.here());
        // Whole days go by days, from the day they were grabbed on: Wednesday and Thursday,
        // grabbed on the Thursday and let go on the 20th.
        let bar = grabbed(at(wed, 0, 0), at(fri, 0, 0), true, Grip::Move, Under::Day(thu));
        assert_eq!(over(&bar, Under::Day(d(2026, 10, 20))), Span { start: at(d(2026, 10, 19), 0, 0), end: at(d(2026, 10, 21), 0, 0), all_day: true });
        // Into the hours they are half an hour, in the row they land in.
        assert_eq!(over(&bar, time(fri, 13 * 60 + 40)), Span { start: at(fri, 13, 30), end: at(fri, 14, 0), all_day: false });
        assert_eq!(bar.span(time(fri, 13 * 60 + 40), 5, 5), Span { start: at(fri, 13, 40), end: at(fri, 14, 10), all_day: false });
        assert_eq!(over(&Drag { repeats: true, ..bar }, time(fri, 800)), bar.here());
        // The right end is the last day, never before the first; the left end the first, never
        // after the last.
        let right = Drag { grip: Grip::End, ..bar };
        assert_eq!(over(&right, Under::Day(d(2026, 10, 18))), Span { start: at(wed, 0, 0), end: at(d(2026, 10, 19), 0, 0), all_day: true });
        assert_eq!(over(&right, Under::Day(d(2026, 10, 1))), Span { start: at(wed, 0, 0), end: at(thu, 0, 0), all_day: true });
        let left = Drag { grip: Grip::Start, ..bar };
        assert_eq!(over(&left, Under::Day(d(2026, 10, 12))), Span { start: at(d(2026, 10, 12), 0, 0), end: at(fri, 0, 0), all_day: true });
        assert_eq!(over(&left, Under::Day(d(2026, 10, 30))), Span { start: at(thu, 0, 0), end: at(fri, 0, 0), all_day: true });
        // A timed event in the month goes to another day and keeps its time of day.
        let chip = grabbed(start, end, false, Grip::Move, Under::Day(wed));
        let later = d(2026, 10, 27);
        assert_eq!(over(&chip, Under::Day(later)), Span { start: at(later, 11, 0), end: at(later, 12, 0), all_day: false });
        // One that is long rather than whole days keeps its times too, at both ends.
        let long = grabbed(at(wed, 22, 0), at(fri, 2, 0), false, Grip::End, Under::Day(fri));
        assert_eq!(over(&long, Under::Day(d(2026, 10, 18))).end, at(d(2026, 10, 18), 2, 0));
        assert_eq!(over(&long, Under::Day(wed)), long.here(), "it can't end before it starts");

        // Nothing a pointer can do breaks it.
        let ends = [time(d(1, 1, 1), i64::MIN), time(d(9999, 12, 31), i64::MAX), Under::Day(d(1, 1, 1)), Under::Day(d(9999, 12, 31))];
        for under in ends {
            for press in ends {
                for grip in [Grip::Move, Grip::Start, Grip::End] {
                    for drag in [lower, bar, chip, late, long] {
                        for (step, slot) in [(0, 0), (i64::MAX, i64::MAX), (i64::MIN, -3)] {
                            let _ = Drag { grip, press, ..drag }.span(under, step, slot);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_parts_of_an_event_that_are_grabbed() {
        let block = Rect::from_min_max(pos2(100.0, 200.0), pos2(180.0, 246.0));
        let all = Piece { bar: false, starts: true, ends: true };
        let grip = |y: f32, piece: Piece| grip_of(block, pos2(140.0, y), piece);
        assert_eq!((grip(201.0, all), grip(223.0, all), grip(244.5, all)), (Grip::Start, Grip::Move, Grip::End));
        // The piece of an event that goes on tomorrow has no lower edge to drag, and so on.
        assert_eq!(grip(244.5, Piece { ends: false, ..all }), Grip::Move);
        assert_eq!(grip(201.0, Piece { starts: false, ..all }), Grip::Move);
        // However small it is drawn, its middle moves it.
        let small = Rect::from_min_max(pos2(0.0, 0.0), pos2(80.0, 8.0));
        assert_eq!(grip_of(small, pos2(40.0, 4.0), all), Grip::Move);
        assert_eq!(grip_of(Rect::NOTHING, pos2(f32::NAN, f32::NAN), all), Grip::Move);
        // A bar is grabbed by its ends.
        let bar = Piece { bar: true, starts: true, ends: true };
        let grip = |x: f32| grip_of(block, pos2(x, 220.0), bar);
        assert_eq!((grip(102.0), grip(140.0), grip(178.0)), (Grip::Start, Grip::Move, Grip::End));
    }

    #[test]
    fn dragging_an_edge_changes_when_an_event_starts_or_ends() {
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let wednesday = d(2026, 10, 14);
        let review = rig.event("Design review").id;
        let times = |rig: &Rig| rig.app.session.mailbox.event(review).map(|e| (e.start, e.end)).unwrap();
        assert_eq!(times(&rig), (at(wednesday, 11, 0), at(wednesday, 12, 0)));
        let column = rig.at("Q4 roadmap").min.x - rig.at("Lunch with Dev").min.x;
        // Its edges say what they are for; its middle doesn't.
        let block = rig.at("Design review");
        let (upper, lower) = (pos2(block.center().x, block.min.y + 2.0), pos2(block.center().x, block.max.y - 2.0));
        for (pos, shape) in
            [(lower, egui::CursorIcon::ResizeVertical), (block.center(), egui::CursorIcon::Default), (upper, egui::CursorIcon::ResizeVertical)]
        {
            rig.frame(vec![egui::Event::PointerMoved(pos)]);
            rig.frames(2);
            assert_eq!(rig.cursor, shape, "{pos:?}");
        }
        // The lower edge, an hour and a quarter down: the ghost shows the new end, and nothing
        // changes until the button is let go.
        let to = lower + vec2(0.0, 1.25 * HOUR_H);
        rig.drag(lower, to);
        assert_eq!(rig.target(), Some((at(wednesday, 11, 0), at(wednesday, 13, 15), false)));
        let ghost = rig.app.hit.rect("ghost", review).expect("the ghost is drawn");
        assert!((ghost.min.y - block.min.y).abs() < 1.5 && (ghost.height() - (2.25 * HOUR_H - 2.0)).abs() < 1.5, "{ghost:?} {block:?}");
        assert_eq!((times(&rig), rig.cursor), ((at(wednesday, 11, 0), at(wednesday, 12, 0)), egui::CursorIcon::ResizeVertical));
        rig.drop_at(to);
        assert_eq!(times(&rig), (at(wednesday, 11, 0), at(wednesday, 13, 15)));
        assert_eq!((rig.app.session.undo_label(), rig.selected(), rig.dragging()), (Some("Move Event"), (Some(review), Some(wednesday)), None));
        // The upper edge, half an hour up. An edge stays on its day, whatever column the
        // pointer strays into.
        let block = rig.at("Design review");
        let from = pos2(block.center().x, block.min.y + 2.0);
        let to = from + vec2(column, -0.5 * HOUR_H);
        rig.drag(from, to);
        rig.drop_at(to);
        assert_eq!(times(&rig), (at(wednesday, 10, 30), at(wednesday, 13, 15)));
        // Never shorter than a quarter of an hour: the lower edge dragged far above the start.
        let block = rig.at("Design review");
        let from = pos2(block.center().x, block.max.y - 2.0);
        let to = pos2(from.x, block.min.y - 2.0 * HOUR_H);
        rig.drag(from, to);
        assert_eq!(rig.target().map(|t| t.1), Some(at(wednesday, 10, 45)));
        rig.drop_at(to);
        assert_eq!(times(&rig), (at(wednesday, 10, 30), at(wednesday, 10, 45)));
        // A nudge of an edge is not a change, and Escape gives a real one up.
        let block = rig.at("Q4 roadmap");
        let roadmap = rig.event("Q4 roadmap");
        let from = pos2(block.center().x, block.max.y - 2.0);
        rig.drag(from, from + vec2(2.0, 5.0));
        rig.drop_at(from + vec2(2.0, 5.0));
        assert_eq!(rig.event("Q4 roadmap").end, roadmap.end);
        rig.drag(from, from + vec2(0.0, HOUR_H));
        assert_eq!(rig.target().map(|t| t.1), Some(roadmap.end.add_minutes(60)));
        rig.key(egui::Key::Escape);
        rig.drop_at(from + vec2(0.0, HOUR_H));
        assert_eq!((rig.event("Q4 roadmap").end, rig.dragging()), (roadmap.end, None));
        // An item that repeats is asked about first. The entire series: it is longer on every
        // day it happens.
        let standup = rig.event("Team stand-up");
        let block = rig.at("Team stand-up");
        let from = pos2(block.center().x, block.max.y - 2.0);
        rig.drag(from, from + vec2(0.0, 0.5 * HOUR_H));
        rig.drop_at(from + vec2(0.0, 0.5 * HOUR_H));
        assert_eq!((rig.dialog(), rig.event("Team stand-up").end), (Some("confirm"), standup.end));
        assert_eq!(rig.answer(1), ("calendar.move".to_string(), "Move Recurring Item".to_string()));
        let longer = rig.event("Team stand-up");
        assert_eq!((longer.start, longer.duration_minutes()), (standup.start, standup.duration_minutes() + 30));
        assert!(longer.recurrence.is_some() && longer.exceptions.is_empty());
        // Just this one: that day's starts half an hour earlier, as an event of its own; the
        // series keeps its times and skips the day.
        let block = rig.at("Team stand-up");
        let from = pos2(block.center().x, block.min.y + 2.0);
        rig.drag(from, from + vec2(0.0, -0.5 * HOUR_H));
        rig.drop_at(from + vec2(0.0, -0.5 * HOUR_H));
        rig.answer(0);
        let now = rig.all("Team stand-up");
        assert_eq!(now.len(), 2, "{now:?}");
        let series = rig.app.session.mailbox.event(standup.id).cloned().unwrap();
        assert_eq!((series.start, series.end, series.exceptions.len()), (longer.start, longer.end, 1));
        let one = now.iter().find(|e| !e.3).expect("the one that was changed");
        let day = series.exceptions[0];
        assert_eq!((one.1, one.2), (at(day, 9, 0), at(day, 10, 15)));
    }

    #[test]
    fn ctrl_makes_a_drop_a_copy_and_the_right_button_asks() {
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let wednesday = d(2026, 10, 14);
        let review = rig.event("Design review");
        let copies = |rig: &Rig, of: &Event| -> Vec<Arc<Event>> {
            rig.app.session.mailbox.events.values().filter(|e| e.title == of.title && e.id != of.id).cloned().collect()
        };
        let block = rig.at("Design review");
        let (from, to) = (block.center(), block.center() + vec2(0.0, 3.0 * HOUR_H));
        // Dragged three hours down, it would move there; with Ctrl held it would be copied
        // there, and its ghost says so.
        rig.drag(from, to);
        assert!(rig.app.hit.rect("ghost", review.id).is_some() && rig.app.hit.rect("ghostCopy", review.id).is_none());
        rig.mods = COPY;
        rig.frames(2);
        assert!(rig.dragging().is_some_and(|drag| drag.copy));
        assert!(rig.app.hit.rect("ghostCopy", review.id).is_some() && rig.app.hit.rect("ghost", review.id).is_none());
        assert_eq!(rig.cursor, egui::CursorIcon::Copy);
        rig.drop_at(to);
        rig.mods = egui::Modifiers::NONE;
        rig.frames(1);
        // The original is where it was; the copy is an appointment of its own at the new time.
        assert_eq!(rig.app.session.mailbox.event(review.id).map(|e| (e.start, e.end)), Some((review.start, review.end)));
        let made = copies(&rig, &review);
        assert_eq!(made.len(), 1);
        assert_eq!((made[0].start, made[0].end, made[0].location.as_str()), (at(wednesday, 14, 0), at(wednesday, 15, 0), review.location.as_str()));
        assert!(!made[0].is_meeting() && made[0].recurrence.is_none());
        assert_eq!((rig.app.session.undo_label(), rig.selected().0), (Some("Copy Event"), Some(made[0].id)));
        // Ctrl let go before the button: it is a move after all.
        let lunch = rig.event("Lunch with Dev");
        let from = rig.at("Lunch with Dev").center();
        let to = from + vec2(0.0, HOUR_H);
        rig.mods = COPY;
        rig.drag(from, to);
        rig.mods = egui::Modifiers::NONE;
        rig.frames(2);
        rig.drop_at(to);
        assert_eq!(rig.event("Lunch with Dev").start, lunch.start.add_minutes(60));
        assert!(copies(&rig, &lunch).is_empty());

        // The right button: letting go asks what to do, and nothing has happened yet.
        let lunch = rig.event("Lunch with Dev");
        let secondary = egui::PointerButton::Secondary;
        let ask = |rig: &mut Rig| {
            let from = rig.at("Lunch with Dev").center();
            let to = from + vec2(0.0, 2.0 * HOUR_H);
            rig.drag_with(secondary, from, to);
            assert!(rig.dragging().is_some_and(|drag| drag.menu && !drag.copy));
            rig.drop_with(secondary, to);
            let menu: Vec<Rect> = (0..3).map(|n| rig.app.hit.rect("dropMenu", n).expect("the question is asked")).collect();
            assert!(menu[0].max.y <= menu[1].min.y + 0.5 && menu[1].max.y <= menu[2].min.y + 0.5, "{menu:?}");
            // The event's ghost waits where it would go.
            assert!(rig.dragging().is_none());
            menu
        };
        let asked = |rig: &Rig| rig.app.hit.rect("dropMenu", 0).is_some();
        // Cancel.
        let menu = ask(&mut rig);
        assert_eq!((rig.event("Lunch with Dev").start, rig.dialog()), (lunch.start, None));
        rig.click(menu[2].center(), 1);
        assert_eq!((rig.event("Lunch with Dev").start, asked(&rig), copies(&rig, &lunch).len()), (lunch.start, false, 0));
        // Escape, and a click somewhere else, are Cancel too.
        ask(&mut rig);
        rig.key(egui::Key::Escape);
        assert_eq!((rig.event("Lunch with Dev").start, asked(&rig)), (lunch.start, false));
        let menu = ask(&mut rig);
        rig.click(menu[0].left_top() - vec2(60.0, 60.0), 1);
        assert_eq!(
            (rig.app.session.mailbox.event(lunch.id).map(|e| e.start), asked(&rig), copies(&rig, &lunch).len()),
            (Some(lunch.start), false, 0)
        );
        // Copy.
        rig.run("calendar.select", json!({"id": null}));
        let menu = ask(&mut rig);
        rig.click(menu[1].center(), 1);
        let made = copies(&rig, &lunch);
        assert_eq!((made.len(), asked(&rig)), (1, false));
        assert_eq!((made[0].start, rig.app.session.mailbox.event(lunch.id).map(|e| e.start)), (lunch.start.add_minutes(120), Some(lunch.start)));
        // Move.
        rig.run("calendar.delete", json!({"id": made[0].id}));
        let menu = ask(&mut rig);
        rig.click(menu[0].center(), 1);
        assert_eq!((rig.event("Lunch with Dev").start, copies(&rig, &lunch).len(), asked(&rig)), (lunch.start.add_minutes(120), 0, false));
        // The right button without a drag is still the menu of the event, and changes nothing.
        let count = rig.app.session.mailbox.events.len();
        rig.right_click(rig.at("Lunch with Dev").center());
        assert_eq!((rig.app.session.sel.event, asked(&rig), rig.app.session.mailbox.events.len()), (Some(lunch.id), false, count));
    }

    #[test]
    fn events_are_dragged_between_days_in_the_month_and_the_row_of_whole_days() {
        let mut rig = Rig::sample();
        rig.run("view.month", json!({}));
        let cell = |rig: &Rig, day: Date| rig.day("day", day).unwrap_or_else(|| panic!("{} isn't drawn", day.iso()));
        let whole = |rig: &Rig, title: &str| {
            let e = rig.event(title);
            (e.all_day, e.start, e.end)
        };
        // A timed event dragged to another day keeps its time of day.
        let dentist = rig.event("Dentist");
        assert_eq!((dentist.start, dentist.end), (at(d(2026, 10, 17), 8, 30), at(d(2026, 10, 17), 9, 15)));
        let to = cell(&rig, d(2026, 10, 27)).center();
        rig.drag(rig.at("Dentist").center(), to);
        assert_eq!(rig.target(), Some((at(d(2026, 10, 27), 8, 30), at(d(2026, 10, 27), 9, 15), false)));
        assert!(rig.app.hit.rect("ghost", dentist.id).is_some_and(|ghost| cell(&rig, d(2026, 10, 27)).contains(ghost.center())));
        rig.drop_at(to);
        assert_eq!(whole(&rig, "Dentist"), (false, at(d(2026, 10, 27), 8, 30), at(d(2026, 10, 27), 9, 15)));
        assert_eq!((rig.app.session.undo_label(), rig.selected()), (Some("Move Event"), (Some(dentist.id), Some(d(2026, 10, 27)))));
        // With Ctrl it is copied there.
        let to = cell(&rig, d(2026, 10, 30)).center();
        rig.drag(rig.at("Dentist").center(), to);
        rig.mods = COPY;
        rig.frames(2);
        rig.drop_at(to);
        rig.mods = egui::Modifiers::NONE;
        rig.frames(1);
        let copy: Vec<DateTime> =
            rig.app.session.mailbox.events.values().filter(|e| e.title == dentist.title && e.id != dentist.id).map(|e| e.start).collect();
        assert_eq!(copy, [at(d(2026, 10, 30), 8, 30)]);
        // Two whole days, grabbed on the first of them, go by days.
        assert_eq!(whole(&rig, "Planning days"), (true, at(d(2026, 10, 21), 0, 0), at(d(2026, 10, 23), 0, 0)));
        let bar = rig.at("Planning days");
        let day_w = cell(&rig, d(2026, 10, 21)).width();
        assert!(bar.width() > 1.8 * day_w, "{bar:?}");
        let to = cell(&rig, d(2026, 10, 5)).center();
        rig.drag(pos2(bar.min.x + 0.5 * day_w, bar.center().y), to);
        assert_eq!(rig.target(), Some((at(d(2026, 10, 5), 0, 0), at(d(2026, 10, 7), 0, 0), true)));
        rig.drop_at(to);
        assert_eq!(whole(&rig, "Planning days"), (true, at(d(2026, 10, 5), 0, 0), at(d(2026, 10, 7), 0, 0)));
        // Its right end makes it longer, its left end makes it start later; its ends say so.
        let bar = rig.at("Planning days");
        let (left, right) = (pos2(bar.min.x + 2.0, bar.center().y), pos2(bar.max.x - 2.0, bar.center().y));
        rig.frame(vec![egui::Event::PointerMoved(right)]);
        rig.frames(2);
        assert_eq!(rig.cursor, egui::CursorIcon::ResizeHorizontal);
        let to = pos2(cell(&rig, d(2026, 10, 9)).center().x, right.y);
        rig.drag(right, to);
        rig.drop_at(to);
        assert_eq!(whole(&rig, "Planning days"), (true, at(d(2026, 10, 5), 0, 0), at(d(2026, 10, 10), 0, 0)));
        let to = pos2(cell(&rig, d(2026, 10, 8)).center().x, left.y);
        rig.drag(left, to);
        rig.drop_at(to);
        assert_eq!(whole(&rig, "Planning days"), (true, at(d(2026, 10, 8), 0, 0), at(d(2026, 10, 10), 0, 0)));
        assert_eq!(rig.dragging(), None);

        // The week of those days: its row of whole days, and the hours under it.
        rig.run("view.goToDate", json!({"date": "2026-10-08"}));
        rig.run("view.week", json!({}));
        let row = |rig: &Rig, day: Date| rig.day("allDay", day).unwrap_or_else(|| panic!("{} isn't drawn", day.iso()));
        // Along the row it goes by days.
        let bar = rig.at("Planning days");
        let to = pos2(row(&rig, d(2026, 10, 5)).center().x, bar.center().y);
        rig.drag(pos2(bar.min.x + 20.0, bar.center().y), to);
        rig.drop_at(to);
        assert_eq!(whole(&rig, "Planning days"), (true, at(d(2026, 10, 5), 0, 0), at(d(2026, 10, 7), 0, 0)));
        // Down into the hours it is half an hour there, on the day and in the row it lands on:
        // busy time with the usual reminder, where it was free days with the afternoon's.
        let before = rig.event("Planning days");
        let bar = rig.at("Planning days");
        let wednesday = row(&rig, d(2026, 10, 7));
        let to = pos2(wednesday.center().x, wednesday.max.y + 3.0 * HOUR_H + 6.0);
        rig.drag(pos2(bar.min.x + 20.0, bar.center().y), to);
        let (start, end, all_day) = rig.target().expect("it can be dropped there");
        assert_eq!((all_day, start.date, start.minutes_until(end), start.minutes_of_day() % 30), (false, d(2026, 10, 7), 30, 0));
        assert!(
            rig.app
                .hit
                .rect("ghost", before.id)
                .is_some_and(|ghost| (ghost.center().x - to.x).abs() < 80.0 && ghost.contains(pos2(ghost.center().x, to.y)))
        );
        rig.drop_at(to);
        let after = rig.event("Planning days");
        assert_eq!((after.all_day, after.start, after.end), (false, start, end));
        let was_usual = (before.show_as == ShowAs::Free, before.reminder_minutes == Some(18 * 60));
        assert_eq!(after.show_as, if was_usual.0 { ShowAs::Busy } else { before.show_as });
        assert_eq!(after.reminder_minutes, if was_usual.1 { Some(15) } else { before.reminder_minutes });
        assert_eq!(rig.app.session.undo_label(), Some("Edit Event"));
        // Up into the row of whole days it is that day, and free time again.
        let block = rig.at("Planning days");
        let thursday = row(&rig, d(2026, 10, 8));
        let to = pos2(thursday.center().x, thursday.center().y);
        rig.drag(block.center(), to);
        assert_eq!(rig.target(), Some((at(d(2026, 10, 8), 0, 0), at(d(2026, 10, 9), 0, 0), true)));
        rig.drop_at(to);
        let back = rig.event("Planning days");
        assert_eq!((back.all_day, back.start, back.end), (true, at(d(2026, 10, 8), 0, 0), at(d(2026, 10, 9), 0, 0)));
        // (It was busy time as an appointment, which is what an appointment starts as: as whole
        // days it is what whole days start as. The reminder it never had, it still hasn't.)
        assert_eq!((back.show_as, back.reminder_minutes), (ShowAs::Free, after.reminder_minutes));
        // Its right end, along the row, to Saturday.
        let bar = rig.at("Planning days");
        let to = pos2(row(&rig, d(2026, 10, 10)).center().x, bar.center().y);
        rig.drag(pos2(bar.max.x - 2.0, bar.center().y), to);
        rig.drop_at(to);
        assert_eq!(whole(&rig, "Planning days"), (true, at(d(2026, 10, 8), 0, 0), at(d(2026, 10, 11), 0, 0)));
        // Dropped outside the calendar, an event stays where it is.
        let bar = rig.at("Planning days");
        rig.drag(bar.center(), pos2(60.0, 300.0));
        assert_eq!(rig.target(), None);
        rig.drop_at(pos2(60.0, 300.0));
        assert_eq!(whole(&rig, "Planning days"), (true, at(d(2026, 10, 8), 0, 0), at(d(2026, 10, 11), 0, 0)));
    }

    #[test]
    fn an_edge_and_a_copy_reach_the_server_and_somebody_elses_meeting_is_asked_about() {
        use crate::calendar_sync::testing::event as on_server;
        use mailcraft_model::{Address, Attendee};
        let meeting = mailcraft_model::remote::RemoteEvent {
            organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
            attendees: vec![Attendee { address: Address::new("Avery", "avery@mail.example"), ..Default::default() }],
            is_organizer: Some(false),
            ..on_server("e-2", "Board meeting", NOW - 2 * 3600)
        };
        let (mut rig, server, account) = Rig::served(vec![on_server("e-1", "Haircut", NOW - 4 * 3600), meeting]);
        let wednesday = d(2026, 10, 14);
        // The owner's own, made half an hour longer by its lower edge: the server has it at once.
        let haircut = rig.event("Haircut");
        let block = rig.at("Haircut");
        let from = pos2(block.center().x, block.max.y - 2.0);
        rig.drag(from, from + vec2(0.0, 0.5 * HOUR_H));
        rig.drop_at(from + vec2(0.0, 0.5 * HOUR_H));
        assert_eq!((rig.dialog(), rig.event("Haircut").end), (None, haircut.end.add_minutes(30)));
        rig.frames(6);
        let on = |id: &str| server.lock().unwrap().events.iter().find(|e| e.id == id).map(|e| (e.start, e.end)).unwrap();
        assert_eq!(on("e-1"), (NOW - 4 * 3600, NOW - 4 * 3600 + 90 * 60));
        // A copy of it, three hours on, is the server's too. (Not so far down that the hours
        // would scroll along: the tabs over the two calendars take a row of the window.)
        let from = rig.at("Haircut").center();
        rig.drag(from, from + vec2(0.0, 3.0 * HOUR_H));
        rig.mods = COPY;
        rig.frames(2);
        rig.drop_at(from + vec2(0.0, 3.0 * HOUR_H));
        rig.mods = egui::Modifiers::NONE;
        rig.frames(6);
        let copy: Vec<Arc<Event>> = rig.app.session.mailbox.events.values().filter(|e| e.title == "Haircut" && e.id != haircut.id).cloned().collect();
        assert_eq!(copy.len(), 1);
        assert_eq!((copy[0].start, copy[0].origin.account, copy[0].origin.id.is_empty()), (at(wednesday, 15, 0), account, false));
        assert_eq!(server.lock().unwrap().events.iter().filter(|e| e.title == "Haircut").count(), 2);
        assert!(rig.app.session.mailbox.event_changes.is_empty());
        // Somebody else's meeting: an edge is asked about first, like a move, and nothing changed.
        let board = rig.event("Board meeting");
        let block = rig.at("Board meeting");
        let from = pos2(block.center().x, block.max.y - 2.0);
        rig.drag(from, from + vec2(0.0, HOUR_H));
        rig.drop_at(from + vec2(0.0, HOUR_H));
        assert_eq!((rig.dialog(), rig.event("Board meeting").end), (Some("confirm"), board.end));
        let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = rig.app.dialog.take() else { panic!("a question was to be asked") };
        assert_eq!((command.as_str(), ask.title.as_str()), ("calendar.move", "Change Your Copy of This Meeting?"));
        let mut confirmed = params.clone();
        confirmed["confirmed"] = json!(true);
        rig.app.run(&command, confirmed).unwrap();
        rig.frames(6);
        assert_eq!(rig.event("Board meeting").end, board.end.add_minutes(60));
        assert_eq!(on("e-2"), (NOW - 2 * 3600, NOW));
        // A copy of it is the owner's own appointment: nobody is asked, nobody is invited.
        let from = rig.at("Board meeting").center();
        rig.drag(from, from + vec2(0.0, -3.0 * HOUR_H));
        rig.mods = COPY;
        rig.frames(2);
        rig.drop_at(from + vec2(0.0, -3.0 * HOUR_H));
        rig.mods = egui::Modifiers::NONE;
        rig.frames(6);
        assert_eq!(rig.dialog(), None);
        let copy: Vec<Arc<Event>> =
            rig.app.session.mailbox.events.values().filter(|e| e.title == "Board meeting" && e.id != board.id).cloned().collect();
        assert_eq!(copy.len(), 1);
        assert!(!copy[0].is_meeting() && !copy[0].origin.guest && copy[0].organizer.is_none());
    }

    const SHIFT: egui::Modifiers = egui::Modifiers { alt: false, ctrl: false, shift: true, mac_cmd: false, command: false };
    const ALT: egui::Modifiers = egui::Modifiers { alt: true, ctrl: false, shift: false, mac_cmd: false, command: false };
    const ALT_SHIFT: egui::Modifiers = egui::Modifiers { alt: true, ctrl: false, shift: true, mac_cmd: false, command: false };

    impl Rig {
        /// What is picked, without what is typed into it: (start, end, whole days).
        fn pick(&self) -> Option<(DateTime, DateTime, bool)> {
            self.slot().map(|s| (s.start, s.end, s.all_day))
        }

        /// Answer the question that is up with its `way`th way of going ahead (0 is the first
        /// button: of a repeating item, "Just This One"; 1 the next: "The Entire Series"). Says
        /// which command it was about, and the question's title.
        fn answer(&mut self, way: usize) -> (String, String) {
            let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = self.app.dialog.take() else { panic!("no question is asked") };
            let with = if way == 0 { ask.with.clone() } else { ask.more[way - 1].1.clone() };
            crate::dialogs::go_ahead(&mut self.app, &command, crate::dialogs::answered(&params, &with, None));
            self.frames(3);
            (command, ask.title)
        }

        /// Every event called `title`, the earliest first: (id, start, end, whether it repeats).
        fn all(&self, title: &str) -> Vec<(Id, DateTime, DateTime, bool)> {
            let events = self.app.session.mailbox.events.values().filter(|e| e.title == title);
            let mut all: Vec<_> = events.map(|e| (e.id, e.start, e.end, e.recurrence.is_some())).collect();
            all.sort_by_key(|e| (e.1, e.0));
            all
        }

        fn keys(&mut self, key: egui::Key, times: usize) {
            for _ in 0..times {
                self.key(key);
            }
        }

        /// How many things of `kind` for `day` are drawn.
        fn count(&mut self, kind: &str, day: Date) -> usize {
            let id = u64::try_from(day.to_days()).unwrap();
            let ctx = self.ctx.clone();
            let items = crate::control::inspect(&mut self.app, &ctx)["items"].clone();
            items.as_array().unwrap().iter().filter(|item| item["kind"] == kind && item["id"] == id).count()
        }

        fn range(&self) -> (Date, Date) {
            calendar_range(&self.app.session)
        }
    }

    /// An item that repeats is asked about (just this one, or the entire series?) whichever way
    /// the grid changes it, and the answer is what happens.
    #[test]
    fn a_repeating_item_is_asked_about_however_the_grid_changes_it() {
        use egui::Key;
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let standup = rig.event("Team stand-up");
        let days: Vec<Date> = (12..=16u8).map(|n| d(2026, 10, n)).collect();
        let (monday, tuesday, wednesday, thursday, friday) = (days[0], days[1], days[2], days[3], days[4]);
        // Where the stand-up of a day is drawn.
        let height = rig.at("Team stand-up").center().y;
        let spot = |rig: &Rig, day: Date| pos2(rig.day("allDay", day).unwrap().center().x - 20.0, height);
        let series = |rig: &Rig| rig.app.session.mailbox.event(standup.id).cloned();
        let skipped = |rig: &Rig| series(rig).map(|e| e.exceptions.clone()).unwrap_or_default();
        // The stand-ups that are events of their own by now.
        let ones = |rig: &Rig| -> Vec<(DateTime, DateTime)> { rig.all("Team stand-up").into_iter().filter(|e| !e.3).map(|e| (e.1, e.2)).collect() };
        let asked = |rig: &mut Rig, way: usize| {
            assert_eq!(rig.dialog(), Some("confirm"), "the question is asked");
            assert!(series(rig).is_some_and(|e| e.start == standup.start && e.end == standup.end), "nothing happens until it is answered");
            rig.answer(way)
        };

        // Alt with Down, on Monday's: just this one goes a row later.
        rig.click(spot(&rig, monday), 1);
        assert_eq!(rig.selected(), (Some(standup.id), Some(monday)));
        rig.chord(ALT, Key::ArrowDown);
        assert_eq!(asked(&mut rig, 0), ("calendar.move".to_string(), "Move Recurring Item".to_string()));
        assert_eq!((ones(&rig), skipped(&rig)), (vec![(at(monday, 10, 0), at(monday, 10, 15))], vec![monday]));
        // Delete, on Tuesday's: just this one goes.
        rig.click(spot(&rig, tuesday), 1);
        rig.key(Key::Delete);
        assert_eq!(asked(&mut rig, 0), ("calendar.delete".to_string(), "Delete Recurring Item".to_string()));
        assert_eq!((ones(&rig).len(), skipped(&rig)), (1, vec![monday, tuesday]));
        // Enter, a double click and Open, on Wednesday's: it opens once it is said which is meant.
        rig.click(spot(&rig, wednesday), 1);
        rig.key(Key::Enter);
        assert!(rig.app.windows.is_empty());
        assert_eq!(asked(&mut rig, 0), ("ui.openWindow".to_string(), "Open Recurring Item".to_string()));
        assert!(rig.app.windows.of_event(standup.id).is_some() && rig.dialog().is_none());
        rig.run("ui.closeWindow", json!({"all": true}));
        rig.click(spot(&rig, wednesday), 2);
        assert!(rig.app.windows.is_empty());
        assert_eq!(asked(&mut rig, 1).0, "ui.openWindow");
        assert!(rig.app.windows.of_event(standup.id).is_some());
        rig.run("ui.closeWindow", json!({"all": true}));
        rig.click(spot(&rig, wednesday), 1);
        let _ = rig.app.run("calendar.edit", json!({}));
        rig.frames(2);
        assert!(rig.app.windows.is_empty());
        assert_eq!(asked(&mut rig, 0).0, "calendar.edit");
        assert!(rig.app.windows.of_event(standup.id).is_some() && rig.dialog().is_none(), "{:?}", rig.dialog());
        rig.run("ui.closeWindow", json!({"all": true}));
        // Cut and paste, Thursday's to four o'clock that day: just this one.
        rig.click(spot(&rig, thursday), 1);
        rig.chord(CTRL, Key::X);
        rig.click(pos2(spot(&rig, thursday).x, after_noon(&rig, 4.0).y), 1);
        assert_eq!(rig.pick(), Some((at(thursday, 16, 0), at(thursday, 16, 30), false)));
        rig.chord(CTRL, Key::V);
        assert_eq!(asked(&mut rig, 0), ("calendar.move".to_string(), "Move Recurring Item".to_string()));
        assert_eq!(skipped(&rig), [monday, tuesday, thursday]);
        assert_eq!(ones(&rig), [(at(monday, 10, 0), at(monday, 10, 15)), (at(thursday, 16, 0), at(thursday, 16, 15))]);
        // To another day there is nothing to ask: the series keeps its days, so it is that one
        // that goes. Friday's, cut and pasted to one o'clock on Saturday.
        let saturday = d(2026, 10, 17);
        rig.click(spot(&rig, friday), 1);
        rig.chord(CTRL, Key::X);
        rig.click(pos2(rig.day("allDay", saturday).unwrap().center().x, after_noon(&rig, 1.0).y), 1);
        rig.time += 1.0;
        rig.chord(CTRL, Key::V);
        rig.frames(2);
        assert_eq!((rig.dialog(), skipped(&rig)), (None, vec![monday, tuesday, thursday, friday]));
        assert!(ones(&rig).contains(&(at(saturday, 13, 0), at(saturday, 13, 15))));
        // (On to the week after, where every day still has its stand-up.)
        rig.run("view.next", json!({}));
        let (wednesday, friday) = (d(2026, 10, 21), d(2026, 10, 23));
        // A copy is of the occurrence, whichever is meant: nothing is asked.
        let from = spot(&rig, friday);
        rig.drag(from, from + vec2(0.0, 2.0 * HOUR_H));
        rig.mods = COPY;
        rig.frames(2);
        rig.drop_at(from + vec2(0.0, 2.0 * HOUR_H));
        rig.mods = egui::Modifiers::NONE;
        rig.frames(1);
        assert_eq!((rig.dialog(), ones(&rig).len(), skipped(&rig).len()), (None, 4, 4));
        assert!(ones(&rig).contains(&(at(friday, 11, 30), at(friday, 11, 45))));
        // The right button: Move is asked about like any move.
        let from = spot(&rig, wednesday);
        rig.drag_with(egui::PointerButton::Secondary, from, from + vec2(0.0, HOUR_H));
        rig.drop_with(egui::PointerButton::Secondary, from + vec2(0.0, HOUR_H));
        rig.click(rig.app.hit.rect("dropMenu", 0).expect("Move, Copy or Cancel").center(), 1);
        assert_eq!(asked(&mut rig, 0).0, "calendar.move");
        assert_eq!(skipped(&rig).last(), Some(&wednesday));
        assert!(ones(&rig).contains(&(at(wednesday, 10, 30), at(wednesday, 10, 45))));
        // In the month, to another day: a series that keeps its days stays, and that one
        // occurrence goes there on its own, with nothing to ask. Alt with Down likewise, a
        // week on.
        rig.run("view.month", json!({}));
        let next = d(2026, 10, 24);
        let cell = |rig: &Rig, day: Date| rig.day("day", day).unwrap();
        let chip = rig.app.hit.rect("event", standup.id).expect("a stand-up is drawn in the month");
        let on = (0..42).map(|n| d(2026, 9, 27).add_days(n)).find(|day| cell(&rig, *day).contains(chip.center())).expect("the day it is drawn on");
        let to = pos2(cell(&rig, next).center().x + 30.0, cell(&rig, next).max.y - 4.0);
        rig.drag(chip.center(), to);
        rig.drop_at(to);
        assert_eq!(rig.dialog(), None);
        assert!(skipped(&rig).contains(&on) && ones(&rig).contains(&(at(next, 9, 30), at(next, 9, 45))), "{on:?} {:?}", ones(&rig));
        assert!(series(&rig).is_some_and(|e| e.start == standup.start && e.end == standup.end));
        let chip = rig.app.hit.rect("event", standup.id).expect("the next stand-up");
        let on = (0..42).map(|n| d(2026, 9, 27).add_days(n)).find(|day| cell(&rig, *day).contains(chip.center())).expect("the day it is drawn on");
        rig.click(chip.center(), 1);
        assert_eq!(rig.selected(), (Some(standup.id), Some(on)));
        let own = ones(&rig).len();
        rig.chord(ALT, Key::ArrowDown);
        assert_eq!((rig.dialog(), ones(&rig).len()), (None, own + 1));
        assert!(skipped(&rig).contains(&on) && ones(&rig).contains(&(at(on.add_days(7), 9, 30), at(on.add_days(7), 9, 45))));
        // The entire series, with Delete: it is gone, and the ones that were changed on their
        // own stay.
        rig.run("view.week", json!({}));
        rig.click(spot(&rig, friday), 1);
        assert_eq!(rig.selected(), (Some(standup.id), Some(friday)));
        let own = ones(&rig).len();
        rig.key(Key::Delete);
        assert_eq!(rig.dialog(), Some("confirm"));
        assert_eq!(rig.answer(1).0, "calendar.delete");
        assert_eq!((series(&rig), ones(&rig).len()), (None, own));
    }

    #[test]
    fn the_arrow_keys_move_the_pick_and_shift_stretches_it() {
        use egui::Key;
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let (tuesday, wednesday, thursday) = (d(2026, 10, 13), d(2026, 10, 14), d(2026, 10, 15));
        // With nothing picked, an arrow picks the start of today's working day and goes no further.
        assert_eq!(rig.slot(), None);
        rig.key(Key::ArrowDown);
        assert_eq!(rig.pick(), Some((at(wednesday, 8, 0), at(wednesday, 8, 30), false)));
        assert!(rig.day("slot", wednesday).is_some(), "the pick is drawn");
        // Down and up by a row.
        rig.keys(Key::ArrowDown, 2);
        assert_eq!(rig.pick(), Some((at(wednesday, 9, 0), at(wednesday, 9, 30), false)));
        rig.key(Key::ArrowUp);
        assert_eq!(rig.pick(), Some((at(wednesday, 8, 30), at(wednesday, 9, 0), false)));
        // Shift stretches it from where it began: down, back, and up past its start.
        rig.chord(SHIFT, Key::ArrowDown);
        rig.chord(SHIFT, Key::ArrowDown);
        assert_eq!(rig.pick(), Some((at(wednesday, 8, 30), at(wednesday, 10, 0), false)));
        rig.chord(SHIFT, Key::ArrowUp);
        assert_eq!(rig.pick(), Some((at(wednesday, 8, 30), at(wednesday, 9, 30), false)));
        for _ in 0..2 {
            rig.chord(SHIFT, Key::ArrowUp);
        }
        assert_eq!(rig.pick(), Some((at(wednesday, 8, 0), at(wednesday, 9, 0), false)));
        // Left and right: the same rows of the day before and after.
        rig.key(Key::ArrowRight);
        assert_eq!(rig.pick(), Some((at(thursday, 8, 0), at(thursday, 9, 0), false)));
        rig.keys(Key::ArrowLeft, 2);
        assert_eq!(rig.pick(), Some((at(tuesday, 8, 0), at(tuesday, 9, 0), false)));
        // What is typed goes into what the keys picked.
        rig.type_text("Standing order");
        rig.key(Key::Enter);
        rig.frames(2);
        let made = rig.event("Standing order");
        assert_eq!((made.start, made.end), (at(tuesday, 8, 0), at(tuesday, 9, 0)));
        // With an event selected the keys start from it, and let go of it.
        assert_eq!(rig.selected().0, Some(made.id));
        rig.key(Key::ArrowDown);
        assert_eq!((rig.pick(), rig.selected()), (Some((at(tuesday, 8, 30), at(tuesday, 9, 0), false)), (None, None)));

        // Home and End are the ends of the working day; Page Down and Page Up go by a screen,
        // and the hours follow the pick.
        rig.click(after_noon(&rig, 2.0), 1);
        rig.key(Key::Home);
        assert_eq!(rig.pick(), Some((at(wednesday, 8, 0), at(wednesday, 8, 30), false)));
        rig.key(Key::End);
        assert_eq!(rig.pick(), Some((at(wednesday, 16, 30), at(wednesday, 17, 0), false)));
        let row = rig.day("allDay", wednesday).unwrap();
        let in_view = |rig: &Rig| rig.day("slot", wednesday).is_some_and(|mark| mark.min.y >= row.max.y - 1.0 && mark.max.y <= 800.0);
        rig.key(Key::PageDown);
        rig.frames(2);
        let (later, ..) = rig.pick().unwrap();
        assert!(later > at(wednesday, 19, 0) && later.date == wednesday && in_view(&rig), "{later:?} {:?}", rig.day("slot", wednesday));
        rig.keys(Key::PageUp, 3);
        rig.frames(2);
        let (earlier, ..) = rig.pick().unwrap();
        assert!(earlier < at(wednesday, 3, 0) && in_view(&rig), "{earlier:?} {:?}", rig.day("slot", wednesday));
        rig.keys(Key::PageUp, 3);
        assert_eq!(rig.pick(), Some((at(wednesday, 0, 0), at(wednesday, 0, 30), false)));
        // Up from the first row is the row of whole days; down from there, the working day.
        rig.key(Key::ArrowUp);
        assert_eq!(rig.pick(), Some((at(wednesday, 0, 0), at(thursday, 0, 0), true)));
        rig.key(Key::ArrowRight);
        assert_eq!(rig.pick(), Some((at(thursday, 0, 0), at(d(2026, 10, 16), 0, 0), true)));
        rig.chord(SHIFT, Key::ArrowRight);
        assert_eq!(rig.pick(), Some((at(thursday, 0, 0), at(d(2026, 10, 17), 0, 0), true)));
        rig.key(Key::ArrowDown);
        assert_eq!(rig.pick(), Some((at(d(2026, 10, 16), 8, 0), at(d(2026, 10, 16), 8, 30), false)));
        // Past the end of the week, the next week comes into view.
        rig.keys(Key::ArrowRight, 2);
        assert_eq!((rig.pick().map(|p| p.0), rig.range()), (Some(at(d(2026, 10, 18), 8, 0)), (d(2026, 10, 18), d(2026, 10, 24))));
        rig.key(Key::ArrowLeft);
        assert_eq!((rig.pick().map(|p| p.0), rig.range().0), (Some(at(d(2026, 10, 17), 8, 0)), d(2026, 10, 11)));
        // A work week has no weekend (and Saturday's pick went with it): from Friday it is
        // Monday, and back.
        rig.run("view.workWeek", json!({}));
        assert_eq!(rig.slot(), None);
        rig.keys(Key::ArrowRight, 3);
        assert_eq!(rig.pick().map(|p| p.0), Some(at(d(2026, 10, 16), 8, 0)));
        rig.key(Key::ArrowRight);
        assert_eq!((rig.pick().map(|p| p.0), rig.range()), (Some(at(d(2026, 10, 19), 8, 0)), (d(2026, 10, 19), d(2026, 10, 23))));
        rig.key(Key::ArrowLeft);
        assert_eq!((rig.pick().map(|p| p.0), rig.range().0), (Some(at(d(2026, 10, 16), 8, 0)), d(2026, 10, 12)));
        // Escape lets go of the pick; and nothing of all this changed the mailbox but the entry.
        rig.key(Key::Escape);
        assert_eq!((rig.slot(), rig.app.session.undo_label()), (None, Some("New Appointment")));

        // The month: days, by one and by a week; Home and End are the ends of the week.
        rig.run("view.goToDate", json!({"date": "2026-10-14"}));
        rig.run("view.month", json!({}));
        rig.key(Key::ArrowRight);
        assert_eq!(rig.pick(), Some((at(wednesday, 0, 0), at(thursday, 0, 0), true)));
        rig.key(Key::ArrowRight);
        rig.key(Key::ArrowDown);
        assert_eq!(rig.pick().map(|p| p.0), Some(at(d(2026, 10, 22), 0, 0)));
        rig.chord(SHIFT, Key::ArrowRight);
        rig.chord(SHIFT, Key::ArrowDown);
        assert_eq!(rig.pick(), Some((at(d(2026, 10, 22), 0, 0), at(d(2026, 10, 31), 0, 0), true)));
        rig.key(Key::ArrowUp);
        assert_eq!(rig.pick(), Some((at(d(2026, 10, 23), 0, 0), at(d(2026, 10, 24), 0, 0), true)));
        rig.key(Key::Home);
        assert_eq!(rig.pick().map(|p| p.0), Some(at(d(2026, 10, 18), 0, 0)));
        rig.key(Key::End);
        assert_eq!(rig.pick().map(|p| p.0), Some(at(d(2026, 10, 24), 0, 0)));
        // Off the end of the month, the next month comes into view.
        rig.keys(Key::ArrowDown, 2);
        assert_eq!((rig.pick().map(|p| p.0), rig.app.session.view.date), (Some(at(d(2026, 11, 7), 0, 0)), d(2026, 11, 7)));
        rig.key(Key::PageUp);
        rig.frames(2);
        assert_eq!(rig.app.session.view.date.month, 10);
        // The ends of the calendar, and every key there.
        for date in ["9999-12-31", "0001-01-01"] {
            for view in ["view.day", "view.week", "view.month"] {
                rig.run("view.goToDate", json!({"date": date}));
                rig.run(view, json!({}));
                for key in [
                    Key::ArrowDown,
                    Key::ArrowRight,
                    Key::ArrowLeft,
                    Key::ArrowUp,
                    Key::Home,
                    Key::End,
                    Key::PageDown,
                    Key::PageUp,
                    Key::Tab,
                    Key::Enter,
                ] {
                    rig.keys(key, 2);
                    rig.chord(SHIFT, key);
                    rig.chord(ALT, key);
                    rig.chord(ALT_SHIFT, key);
                }
                let _ = rig.app.execute("ui.closeWindow", json!({"all": true}));
            }
        }
    }

    #[test]
    fn tab_goes_from_event_to_event_and_enter_and_delete_act_on_the_one_selected() {
        use egui::Key;
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let (from, to) = rig.range();
        let order: Vec<(Option<Id>, Option<Date>)> =
            occurrences(&rig.app.session, from, to).iter().map(|o| (Some(o.event), Some(o.start.date))).collect();
        assert!(order.len() >= 5, "{order:?}");
        // From nothing: the first event of the week; then the next, and back.
        rig.key(Key::Tab);
        assert_eq!(rig.selected(), order[0]);
        rig.key(Key::Tab);
        assert_eq!(rig.selected(), order[1]);
        rig.chord(SHIFT, Key::Tab);
        assert_eq!(rig.selected(), order[0]);
        // Round the ends.
        rig.chord(SHIFT, Key::Tab);
        assert_eq!(rig.selected(), order[order.len() - 1]);
        rig.key(Key::Tab);
        assert_eq!(rig.selected(), order[0]);
        // The keyboard is still the calendar's: Tab didn't go to a button somewhere.
        assert!(rig.ctx.memory(|m| m.focused().is_none()));
        for (step, next) in order.iter().enumerate().skip(1) {
            rig.key(Key::Tab);
            assert_eq!(rig.selected(), *next, "step {step}");
        }
        // From a time that is picked: the first event from then on, or the last before it.
        let wednesday = d(2026, 10, 14);
        rig.click(after_noon(&rig, 2.0), 1);
        let next = occurrences(&rig.app.session, from, to).into_iter().find(|o| o.start >= at(wednesday, 14, 0)).unwrap();
        rig.key(Key::Tab);
        assert_eq!((rig.selected(), rig.slot()), ((Some(next.event), Some(next.start.date)), None));
        rig.click(after_noon(&rig, 2.0), 1);
        let before = occurrences(&rig.app.session, from, to).into_iter().rfind(|o| o.start < at(wednesday, 14, 0)).unwrap();
        rig.chord(SHIFT, Key::Tab);
        assert_eq!(rig.selected(), (Some(before.event), Some(before.start.date)));
        // Enter opens the selected event in a window of its own.
        rig.click(rig.at("Lunch with Dev").center(), 1);
        let lunch = rig.event("Lunch with Dev").id;
        rig.key(Key::Enter);
        assert!(rig.app.windows.of_event(lunch).is_some() && rig.dialog().is_none());
        // While that window lies over the calendar the keys are the window's.
        rig.frames(2);
        rig.key(Key::ArrowDown);
        assert_eq!((rig.slot(), rig.selected().0), (None, Some(lunch)));
        rig.run("ui.closeWindow", json!({"all": true}));
        // Delete deletes it, in one step that can be undone.
        rig.key(Key::Delete);
        assert!(rig.app.session.mailbox.event(lunch).is_none());
        assert_eq!((rig.app.session.undo_label(), rig.selected()), (Some("Delete"), (None, None)));
        // With a field in use (an entry being typed), Tab is the field's: it ends the entry.
        rig.click(after_noon(&rig, 2.0), 1);
        rig.type_text("Tabbed away");
        rig.key(Key::Tab);
        rig.frames(2);
        assert_eq!(rig.event("Tabbed away").start, at(wednesday, 14, 0));
    }

    #[test]
    fn alt_keys_move_the_selected_event_and_say_how_many_days_to_show() {
        use egui::Key;
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let lunch = rig.event("Lunch with Dev");
        let now = |rig: &Rig| rig.app.session.mailbox.event(lunch.id).map(|e| (e.start, e.end)).unwrap();
        rig.click(rig.at("Lunch with Dev").center(), 1);
        // Alt with Down and Up: a row later, and earlier.
        rig.chord(ALT, Key::ArrowDown);
        assert_eq!(now(&rig), (lunch.start.add_minutes(30), lunch.end.add_minutes(30)));
        assert_eq!((rig.app.session.undo_label(), rig.selected()), (Some("Move Event"), (Some(lunch.id), Some(lunch.start.date))));
        rig.chord(ALT, Key::ArrowUp);
        rig.chord(ALT, Key::ArrowUp);
        assert_eq!(now(&rig), (lunch.start.add_minutes(-30), lunch.end.add_minutes(-30)));
        // With Shift it is its end that goes: longer, and shorter, never to nothing.
        rig.chord(ALT_SHIFT, Key::ArrowDown);
        assert_eq!(now(&rig), (lunch.start.add_minutes(-30), lunch.end));
        for _ in 0..12 {
            rig.chord(ALT_SHIFT, Key::ArrowUp);
        }
        let (start, end) = now(&rig);
        assert!(start == lunch.start.add_minutes(-30) && (15..=30).contains(&start.minutes_until(end)), "{start:?} {end:?}");
        // It stays inside its day.
        for _ in 0..60 {
            rig.chord(ALT, Key::ArrowUp);
        }
        assert_eq!(now(&rig).0, at(lunch.start.date, 0, 0));
        // Nothing selected: nothing moves.
        rig.run("calendar.select", json!({"id": null}));
        let before = rig.app.session.mailbox.clone();
        rig.chord(ALT, Key::ArrowDown);
        rig.chord(ALT_SHIFT, Key::ArrowDown);
        assert!(rig.app.session.mailbox == before);

        // (The hours followed the event up to midnight: back to the working day.)
        rig.run("view.day", json!({}));
        rig.run("view.week", json!({}));
        // Alt with a digit: that many days, from the day that is picked.
        let wednesday = d(2026, 10, 14);
        rig.click(after_noon(&rig, 2.0), 1);
        rig.chord(ALT, Key::Num3);
        rig.frames(2);
        assert_eq!((rig.app.session.view.calendar, rig.range()), (CalView::Day, (wednesday, d(2026, 10, 16))));
        assert!(
            rig.day("allDay", d(2026, 10, 16)).is_some()
                && rig.day("allDay", d(2026, 10, 17)).is_none()
                && rig.day("allDay", d(2026, 10, 13)).is_none()
        );
        // From the selected event's day; and 0 is ten.
        rig.run("view.week", json!({}));
        rig.click(rig.at("Q4 roadmap").center(), 1);
        let friday = rig.event("Q4 roadmap").start.date;
        rig.chord(ALT, Key::Num0);
        rig.frames(2);
        assert_eq!(rig.range(), (friday, friday.add_days(9)));
        assert!(rig.day("allDay", friday.add_days(9)).is_some());
        // Alt with = is the month, with - the week; Forward goes by as many days as are shown.
        rig.chord(ALT, Key::Num2);
        rig.run("view.next", json!({}));
        assert_eq!(rig.range(), (friday.add_days(2), friday.add_days(3)));
        rig.chord(ALT, Key::Equals);
        assert_eq!(rig.app.session.view.calendar, CalView::Month);
        rig.chord(ALT, Key::Minus);
        assert_eq!((rig.app.session.view.calendar, rig.app.session.view.days), (CalView::Week, 0));
        // In the month, Alt with Down and Up are a week later and earlier.
        rig.run("view.goToDate", json!({"date": "2026-10-14"}));
        rig.run("view.month", json!({}));
        let review = rig.event("Design review");
        rig.click(rig.at("Design review").center(), 1);
        rig.chord(ALT, Key::ArrowDown);
        assert_eq!(rig.event("Design review").start, review.start.add_days(7));
        assert_eq!(rig.selected(), (Some(review.id), Some(review.start.date.add_days(7))));
        rig.chord(ALT, Key::ArrowUp);
        rig.chord(ALT, Key::ArrowUp);
        assert_eq!(rig.event("Design review").start, review.start.add_days(-7));
    }

    #[test]
    fn copy_cut_and_paste_put_an_event_where_a_time_is_picked() {
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let wednesday = d(2026, 10, 14);
        let review = rig.event("Design review");
        let copies = |rig: &Rig| -> Vec<(DateTime, DateTime)> {
            let mut all: Vec<_> =
                rig.app.session.mailbox.events.values().filter(|e| e.title == review.title && e.id != review.id).map(|e| (e.start, e.end)).collect();
            all.sort();
            all
        };
        let paste = |rig: &mut Rig, text: &str| {
            rig.frame(vec![egui::Event::Paste(text.to_string())]);
            rig.frames(2);
        };
        // Copy the selected event; pick a time; paste: a copy of it there, as long as it was.
        rig.click(rig.at("Design review").center(), 1);
        rig.frame(vec![egui::Event::Copy]);
        rig.frames(1);
        rig.click(after_noon(&rig, 3.0), 1);
        paste(&mut rig, &review.title);
        assert_eq!(copies(&rig), [(at(wednesday, 15, 0), at(wednesday, 16, 0))]);
        assert_eq!(
            (rig.app.session.undo_label(), rig.app.session.mailbox.event(review.id).map(|e| e.start)),
            (Some("Copy Event"), Some(review.start))
        );
        // And again somewhere else: what was copied stays copied.
        rig.click(after_noon(&rig, 5.0), 1);
        paste(&mut rig, &review.title);
        assert_eq!(copies(&rig).len(), 2);
        // The same Paste arriving twice (as the key, and as the clipboard's text) is one paste.
        rig.click(after_noon(&rig, 1.0), 1);
        rig.chord(CTRL, egui::Key::V);
        rig.frame(vec![egui::Event::Paste(review.title.clone())]);
        rig.frames(2);
        assert_eq!(copies(&rig).len(), 3);
        // With no time picked there is nowhere to paste, and the status bar says so.
        rig.run("calendar.select", json!({"id": null}));
        rig.key(egui::Key::Escape);
        rig.time += 1.0;
        assert_eq!(rig.slot(), None);
        paste(&mut rig, &review.title);
        assert_eq!(copies(&rig).len(), 3);
        assert!(rig.app.status_msg.as_ref().is_some_and(|(said, _)| said.contains("Pick a time")));
        // Something else was copied since: its Paste isn't the calendar's, now or later.
        rig.click(after_noon(&rig, 2.0), 1);
        paste(&mut rig, "some text from elsewhere");
        rig.time += 1.0;
        paste(&mut rig, &review.title);
        assert_eq!(copies(&rig).len(), 3);

        // Cut and paste moves the event itself, once.
        let lunch = rig.event("Lunch with Dev");
        let count = rig.app.session.mailbox.events.len();
        rig.click(rig.at("Lunch with Dev").center(), 1);
        rig.chord(CTRL, egui::Key::X);
        assert_eq!(rig.event("Lunch with Dev").start, lunch.start, "it stays until it is pasted");
        rig.click(after_noon(&rig, 2.0), 1);
        rig.chord(CTRL, egui::Key::V);
        rig.frames(2);
        let moved = rig.event("Lunch with Dev");
        assert_eq!((moved.start, moved.duration_minutes(), moved.id), (at(wednesday, 14, 0), lunch.duration_minutes(), lunch.id));
        assert_eq!(
            (rig.app.session.mailbox.events.len(), rig.app.session.undo_label(), rig.selected()),
            (count, Some("Move Event"), (Some(lunch.id), Some(wednesday)))
        );
        rig.click(after_noon(&rig, 4.0), 1);
        rig.time += 1.0;
        rig.chord(CTRL, egui::Key::V);
        rig.frames(2);
        assert_eq!((rig.event("Lunch with Dev").start, rig.app.session.mailbox.events.len()), (at(wednesday, 14, 0), count));

        // In the month a copy lands on the day that is picked, at its own time of day; whole
        // days land there whole.
        rig.run("view.month", json!({}));
        let cell = |rig: &Rig, day: Date| rig.day("day", day).unwrap();
        let low = |rig: &Rig, day: Date| pos2(cell(rig, day).center().x, cell(rig, day).max.y - 5.0);
        let dentist = rig.event("Dentist");
        rig.click(rig.at("Dentist").center(), 1);
        rig.chord(CTRL, egui::Key::C);
        rig.click(low(&rig, d(2026, 10, 28)), 1);
        rig.time += 1.0;
        rig.chord(CTRL, egui::Key::V);
        rig.frames(2);
        let pasted: Vec<(DateTime, bool)> = rig
            .app
            .session
            .mailbox
            .events
            .values()
            .filter(|e| e.title == dentist.title && e.id != dentist.id)
            .map(|e| (e.start, e.private))
            .collect();
        assert_eq!(pasted, [(at(d(2026, 10, 28), 8, 30), true)]);
        let planning = rig.event("Planning days");
        rig.click(rig.at("Planning days").center(), 1);
        rig.chord(CTRL, egui::Key::C);
        rig.click(low(&rig, d(2026, 10, 5)), 1);
        rig.time += 1.0;
        rig.chord(CTRL, egui::Key::V);
        rig.frames(2);
        let pasted: Vec<(bool, DateTime, DateTime)> = rig
            .app
            .session
            .mailbox
            .events
            .values()
            .filter(|e| e.title == planning.title && e.id != planning.id)
            .map(|e| (e.all_day, e.start, e.end))
            .collect();
        assert_eq!(pasted, [(true, at(d(2026, 10, 5), 0, 0), at(d(2026, 10, 7), 0, 0))]);
        // An event that is gone can't be pasted.
        rig.run("calendar.delete", json!({"id": planning.id}));
        rig.click(low(&rig, d(2026, 10, 6)), 1);
        rig.time += 1.0;
        let count = rig.app.session.mailbox.events.len();
        rig.chord(CTRL, egui::Key::V);
        rig.frames(2);
        assert_eq!(rig.app.session.mailbox.events.len(), count);
    }

    #[test]
    fn days_that_are_dragged_over_and_weeks_of_the_year() {
        let day = |m: u8, n: u8| d(2026, m, n);
        assert_eq!(dragged_days(day(11, 3), day(11, 5)), (day(11, 3), 3));
        assert_eq!(dragged_days(day(11, 5), day(11, 3)), (day(11, 3), 3));
        assert_eq!(dragged_days(day(11, 3), day(11, 3)), (day(11, 3), 1));
        // No more than the Day view shows: the ones nearest to where the drag began.
        assert_eq!(dragged_days(day(11, 1), day(12, 25)), (day(11, 1), 14));
        assert_eq!(dragged_days(day(12, 25), day(11, 1)), (day(12, 12), 14));
        let _ = (dragged_days(d(1, 1, 1), d(9999, 12, 31)), dragged_days(d(9999, 12, 31), d(1, 1, 1)));
        // A week's number is its Monday's, whichever day the week is drawn from.
        assert_eq!((week_number(day(10, 11)), week_number(day(10, 12)), week_number(day(10, 10))), (42, 42, 42));
        assert_eq!((week_number(day(12, 27)), week_number(d(2027, 1, 3)), week_number(d(2025, 12, 28))), (53, 1, 1));
        let _ = (week_number(d(1, 1, 1)), week_number(d(9999, 12, 31)));
    }

    #[test]
    fn the_date_navigator_shows_months_and_takes_clicks_and_drags() {
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let month = |rig: &Rig, m: u8| rig.day("miniMonth", d(2026, m, 1));
        let january = |rig: &Rig| rig.day("miniMonth", d(2027, 1, 1));
        // As many months as the pane has room for over the calendars: at this height, three.
        let (october, november, december) =
            (month(&rig, 10).expect("October"), month(&rig, 11).expect("November"), month(&rig, 12).expect("December"));
        assert!(november.min.y > october.max.y + 5.0 * MINI_ROW_H && december.min.y > november.max.y + 5.0 * MINI_ROW_H, "{october:?} {november:?}");
        assert!(month(&rig, 9).is_none() && january(&rig).is_none());
        // Every day is there once: the days round a month only before the first and after the last.
        for (day, times) in [
            (d(2026, 9, 30), 1),
            (d(2026, 10, 31), 1),
            (d(2026, 11, 1), 1),
            (d(2026, 11, 30), 1),
            (d(2026, 12, 1), 1),
            (d(2026, 12, 31), 1),
            (d(2027, 1, 2), 1),
            (d(2026, 9, 20), 0),
            (d(2027, 1, 20), 0),
        ] {
            assert_eq!(rig.count("miniDay", day), times, "{}", day.iso());
        }
        // Days on which something happens are bold: today's meeting, the whole days next week,
        // and no empty day.
        assert!(rig.day("miniBusy", d(2026, 10, 14)).is_some() && rig.day("miniBusy", d(2026, 10, 22)).is_some());
        let empty =
            (1..=30u8).map(|n| d(2026, 11, n)).find(|day| occurrences(&rig.app.session, *day, *day).is_empty()).expect("a free day in November");
        assert!(rig.day("miniBusy", empty).is_none() && rig.day("miniDay", empty).is_some());
        // The calendars are under the months, and still in the pane without scrolling.
        let calendar = rig.app.session.mailbox.calendars.first().map(|c| c.id).unwrap();
        let listed = rig.app.hit.rect("calendar", calendar).expect("the calendar's check box");
        assert!(listed.min.y > december.max.y + 6.0 * MINI_ROW_H && listed.max.y < 780.0, "{listed:?}");
        // The arrows go a month on, and back.
        rig.click(rig.app.hit.rect("miniStep", 1).unwrap().center(), 1);
        assert!(month(&rig, 10).is_none() && month(&rig, 11).is_some() && month(&rig, 12).is_some() && january(&rig).is_some());
        rig.click(rig.app.hit.rect("miniStep", 0).unwrap().center(), 1);
        assert!(month(&rig, 10).is_some() && month(&rig, 12).is_some() && january(&rig).is_none());
        // A click left of a week shows that week.
        let margin = rig.day("miniWeek", d(2026, 10, 25)).expect("the margin of the last week of October");
        rig.click(margin.center(), 1);
        assert_eq!((rig.app.session.view.calendar, rig.range()), (CalView::Week, (d(2026, 10, 25), d(2026, 10, 31))));
        rig.run("view.day", json!({}));
        rig.click(rig.day("miniWeek", d(2026, 11, 8)).unwrap().center(), 1);
        assert_eq!((rig.app.session.view.calendar, rig.range()), (CalView::Week, (d(2026, 11, 8), d(2026, 11, 14))));
        // A drag over days shows exactly those days, whichever way it went; the months stay put.
        let at_day = |rig: &Rig, day: Date| rig.day("miniDay", day).unwrap_or_else(|| panic!("{} isn't shown", day.iso())).center();
        let (from, to) = (at_day(&rig, d(2026, 11, 3)), at_day(&rig, d(2026, 11, 5)));
        rig.drag(from, to);
        assert_eq!(rig.range(), (d(2026, 11, 8), d(2026, 11, 14)), "nothing changes until the button is let go");
        rig.drop_at(to);
        assert_eq!((rig.app.session.view.calendar, rig.range()), (CalView::Day, (d(2026, 11, 3), d(2026, 11, 5))));
        assert_eq!(range_title(CalView::Day, d(2026, 11, 3), d(2026, 11, 3), d(2026, 11, 5)), "November 3 – 5, 2026");
        assert!(month(&rig, 10).is_some() && month(&rig, 11).is_some());
        let (from, to) = (at_day(&rig, d(2026, 10, 30)), at_day(&rig, d(2026, 10, 27)));
        rig.drag(from, to);
        rig.drop_at(to);
        assert_eq!(rig.range(), (d(2026, 10, 27), d(2026, 10, 30)));
        // Across the months, and no more days than the Day view takes.
        let (from, to) = (at_day(&rig, d(2026, 10, 26)), at_day(&rig, d(2026, 11, 20)));
        rig.drag(from, to);
        rig.drop_at(to);
        assert_eq!(rig.range(), (d(2026, 10, 26), d(2026, 11, 8)));
        assert!(rig.day("allDay", d(2026, 11, 8)).is_some(), "fourteen days are drawn");
        // A click is still a click: that day, in the view as it is.
        rig.click(at_day(&rig, d(2026, 11, 12)), 1);
        assert_eq!((rig.app.session.view.calendar, rig.range()), (CalView::Day, (d(2026, 11, 12), d(2026, 11, 25))));
        rig.run("view.day", json!({}));
        rig.click(at_day(&rig, d(2026, 11, 13)), 1);
        assert_eq!(rig.range(), (d(2026, 11, 13), d(2026, 11, 13)));
        // The calendar goes where the months don't show: they follow.
        rig.run("view.goToDate", json!({"date": "2027-03-10"}));
        assert!(rig.day("miniMonth", d(2027, 3, 1)).is_some() && rig.day("miniMonth", d(2027, 4, 1)).is_some() && month(&rig, 11).is_none());

        // Week numbers, when they are wanted: beside the weeks of the month view too, where a
        // click on one shows that week.
        rig.run("view.goToDate", json!({"date": "2026-10-14"}));
        rig.run("view.month", json!({}));
        let sunday = d(2026, 10, 18);
        let without = rig.day("day", sunday).unwrap();
        assert!(rig.day("weekNumber", sunday).is_none());
        rig.run("view.weekNumbers", json!({"value": true}));
        let (number, with) = (rig.day("weekNumber", sunday).expect("the week's number"), rig.day("day", sunday).unwrap());
        assert!(
            number.max.x <= with.min.x + 0.5 && with.min.x > without.min.x + 20.0 && (number.min.y - with.min.y).abs() < 0.5,
            "{number:?} {with:?}"
        );
        rig.click(number.center(), 1);
        assert_eq!((rig.app.session.view.calendar, rig.range()), (CalView::Week, (sunday, d(2026, 10, 24))));
    }

    #[test]
    fn the_time_scale_sets_the_rows_and_what_a_click_picks() {
        use egui::Key;
        let mut rig = Rig::sample();
        rig.run("view.week", json!({}));
        let wednesday = d(2026, 10, 14);
        let tall = |rig: &Rig| rig.at("Design review").height();
        assert!((tall(&rig) - (HOUR_H - 2.0)).abs() < 1.0);
        // The time bar's menu offers the scales.
        let bar = rig.app.hit.rect("timeBar", 0).expect("the time bar");
        assert!(bar.max.x <= rig.at("Team stand-up").min.x && bar.height() > 300.0, "{bar:?}");
        rig.right_click(bar.center());
        assert!(egui::Popup::is_any_open(&rig.ctx));
        rig.key(Key::Escape);
        rig.frames(2);
        // A quarter of an hour a row: the hour is four rows, a click picks a quarter of an hour.
        rig.run("view.timeScale", json!({"minutes": 15}));
        let block = rig.at("Design review");
        assert!((block.height() - (4.0 * ROW_H - 2.0)).abs() < 1.0, "{block:?}");
        rig.click(pos2(block.center().x, block.min.y - 2.0 * ROW_H + 8.0), 1);
        assert_eq!(rig.pick(), Some((at(wednesday, 10, 30), at(wednesday, 10, 45), false)));
        assert!(rig.day("slot", wednesday).is_some_and(|mark| (mark.height() - ROW_H).abs() < 3.0));
        // The keys go by that row, and what is typed is that long.
        rig.key(Key::ArrowUp);
        rig.chord(SHIFT, Key::ArrowUp);
        assert_eq!(rig.pick(), Some((at(wednesday, 10, 0), at(wednesday, 10, 30), false)));
        rig.type_text("Half an hour");
        rig.key(Key::Enter);
        rig.frames(2);
        assert_eq!(rig.event("Half an hour").duration_minutes(), 30);
        // The grid scrolls to what was picked, by an animation; let it settle, then measure.
        rig.frames(60);
        let block = rig.at("Design review");
        // A drag over the grid picks rows of that scale; a dragged event still goes by a quarter.
        let from = pos2(block.center().x, block.min.y - 10.0 * ROW_H + 8.0);
        rig.drag(from, from + vec2(0.0, 2.0 * ROW_H));
        rig.drop_at(from + vec2(0.0, 2.0 * ROW_H));
        assert_eq!(rig.pick(), Some((at(wednesday, 8, 30), at(wednesday, 9, 15), false)));
        rig.key(Key::Escape);
        let from = rig.at("Design review").center();
        rig.drag(from, from + vec2(0.0, ROW_H));
        assert_eq!(rig.target().map(|t| t.0), Some(at(wednesday, 11, 15)));
        rig.key(Key::Escape);
        rig.drop_at(from);
        // An hour a row: the whole day is 24 rows, and a click picks an hour.
        rig.run("view.timeScale", json!({"minutes": 60}));
        let block = rig.at("Design review");
        assert!((block.height() - (ROW_H - 2.0)).abs() < 1.0, "{block:?}");
        rig.click(pos2(block.center().x, block.max.y + 2.0 * ROW_H + 8.0), 1);
        assert_eq!(rig.pick(), Some((at(wednesday, 14, 0), at(wednesday, 15, 0), false)));
        rig.key(Key::Escape);
        // Five minutes a row: the keys go by five minutes, from the start of the working day,
        // which the hours show.
        rig.run("view.timeScale", json!({"minutes": 5}));
        rig.keys(Key::ArrowDown, 3);
        assert_eq!(rig.pick(), Some((at(wednesday, 8, 10), at(wednesday, 8, 15), false)));
        rig.frames(60);
        assert!(rig.day("slot", wednesday).is_some_and(|mark| mark.min.y > 200.0 && mark.max.y < 800.0 && (mark.height() - ROW_H).abs() < 3.0));
        // Every scale draws every view, in both themes, and so do the other ways to look at it.
        rig.run("view.weekNumbers", json!({"value": true}));
        for theme in ["dark", "light"] {
            rig.run("ui.theme", json!({"value": theme}));
            for minutes in [60, 30, 15, 10, 6, 5] {
                rig.run("view.timeScale", json!({"minutes": minutes}));
                for view in ["view.day", "view.workWeek", "view.week", "view.month"] {
                    rig.run(view, json!({}));
                }
                rig.run("view.days", json!({"count": 14}));
            }
        }
        // A scale nobody could have chosen is the usual one.
        rig.app.session.prefs.time_scale = 0;
        rig.run("view.week", json!({}));
        assert!((tall(&rig) - (HOUR_H - 2.0)).abs() < 1.0);
    }

    #[test]
    fn events_are_in_their_categorys_colour_and_look_as_their_time_shows() {
        let rig = Rig::sample();
        let t = Tokens::get(&rig.ctx);
        let (from, to) = (d(2026, 10, 1), d(2026, 10, 31));
        let items = gather(&rig.app, from, to, &t, &pane_views(&rig.app));
        let item = |title: &str| items.iter().find(|i| i.ev.title.starts_with(title)).unwrap_or_else(|| panic!("no {title}"));
        let mailbox = &rig.app.session.mailbox;
        // A category colours the event; without one it is its calendar's colour.
        assert_eq!(item("Q4 roadmap").color, c32(mailbox.category_color("Red category")));
        let lunch = item("Lunch with Dev");
        assert!(lunch.ev.categories.is_empty());
        assert_eq!(lunch.color, mailbox.calendar(lunch.ev.calendar).map(|c| c32(c.color)).unwrap());
        assert!(item("Q4 roadmap").tags.is_empty(), "the one category is the colour, not also a mark");
        // How the time shows is the edge: out of office is purple whatever the event's colour,
        // tentative hatched, free hollow and outlined, busy solid.
        let of = |show_as: ShowAs| look(&Event { show_as, ..Default::default() }, t.blue, t.blue, &t, false, false);
        let (busy, away) = (of(ShowAs::Busy), of(ShowAs::OutOfOffice));
        assert!(busy.kind == Edge::Solid && busy.edge == t.blue && away.kind == Edge::Solid && away.edge != t.blue && away.fill == busy.fill);
        assert!(away.edge.r() > away.edge.g() && away.edge.b() > away.edge.g(), "purple: {:?}", away.edge);
        assert!(of(ShowAs::Tentative).kind == Edge::Hatched && of(ShowAs::WorkingElsewhere).kind == Edge::Dotted);
        assert!(of(ShowAs::Free).kind == Edge::Hollow && of(ShowAs::Free).outline && !busy.outline);
    }

    #[test]
    fn somebody_elses_meeting_is_answered_from_its_menu() {
        use egui_kittest::kittest::Queryable as _;
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
        app.autosave = false;
        app.auto_sync = false;
        app.execute("view.calendar", json!({})).unwrap();
        app.execute("view.week", json!({})).unwrap();
        // Whose meeting it is, and what was answered so far.
        let event = |app: &MailApp, title: &str| app.session.mailbox.events.values().find(|e| e.title.starts_with(title)).cloned().unwrap();
        assert_eq!(invitation(&app, &event(&app, "Q4 roadmap")), (true, Some(Reply::Accepted)));
        assert_eq!(invitation(&app, &event(&app, "Design review")), (false, None));
        assert_eq!(invitation(&app, &event(&app, "Lunch with Dev")), (false, None));
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(vec2(1280.0, 800.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.run_steps(4);
        let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
        harness.run_steps(3);
        type Window = egui_kittest::Harness<'static, MailApp>;
        let menu_of = |harness: &mut Window, title: &str| {
            let id = event(harness.state(), title).id;
            let pos = harness.state().hit.rect("event", id).unwrap_or_else(|| panic!("{title} isn't drawn")).center();
            harness.event(egui::Event::PointerMoved(pos));
            harness.step();
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
                harness.step();
            }
            harness.run_steps(3);
        };
        let says = |harness: &Window, text: &str| harness.query_all_by_label_contains(text).next().is_some();
        // The owner's own appointment has no answers to give.
        menu_of(&mut harness, "Lunch with Dev");
        assert!(says(&harness, "Open") && says(&harness, "Show As") && !says(&harness, "Decline") && !says(&harness, "Tentative"));
        harness.key_press(egui::Key::Escape);
        harness.run_steps(3);
        // Somebody else's meeting has the three of them; Decline declines, and frees the time.
        menu_of(&mut harness, "Q4 roadmap");
        assert!(says(&harness, "Accept") && says(&harness, "Tentative") && says(&harness, "Decline") && says(&harness, "Open"));
        harness.get_by_label_contains("Decline").click();
        harness.run_steps(3);
        let roadmap = event(harness.state(), "Q4 roadmap");
        assert_eq!((invitation(harness.state(), &roadmap), roadmap.show_as), ((true, Some(Reply::Declined)), ShowAs::Free));
        assert_eq!(harness.state().session.undo_label(), Some("Respond"));
        menu_of(&mut harness, "Q4 roadmap");
        harness.get_by_label_contains("Tentative").click();
        harness.run_steps(3);
        assert_eq!(invitation(harness.state(), &event(harness.state(), "Q4 roadmap")).1, Some(Reply::Tentative));
    }

    // ---- several calendars ----

    impl Rig {
        /// The sample calendar with two more of the mailbox's own beside it, each with something
        /// on this week: Team (a retro on Wednesday at two, a release on Thursday morning) and
        /// Family (a school play on Wednesday at one, a birthday all of Thursday). In the Week
        /// view. The ids of the three: the sample's own, Team's, Family's.
        fn with_calendars() -> (Rig, Id, Id, Id) {
            let mut rig = Rig::sample();
            let main = rig.app.session.mailbox.calendars.first().map(|c| c.id).unwrap();
            let add = |rig: &mut Rig, name: &str, color: &str| {
                rig.app.execute("calendar.addCalendar", json!({"name": name, "color": color})).unwrap()["id"].as_u64().unwrap()
            };
            let (team, family) = (add(&mut rig, "Team", "#2E8B57"), add(&mut rig, "Family", "#D13B3B"));
            for (title, start, end, calendar) in [
                ("Retro", "2026-10-14T14:00", "2026-10-14T15:00", team),
                ("Release", "2026-10-15T09:00", "2026-10-15T10:00", team),
                ("School play", "2026-10-14T13:00", "2026-10-14T14:00", family),
            ] {
                let made = json!({"title": title, "start": start, "end": end, "calendar": calendar, "reminder": null});
                rig.app.execute("calendar.new", made).unwrap();
            }
            let birthday = json!({"title": "Birthday", "start": "2026-10-15", "allDay": true, "calendar": family, "reminder": null});
            rig.app.execute("calendar.new", birthday).unwrap();
            rig.app.execute("calendar.select", json!({"id": null})).unwrap();
            rig.app.session.forget_history();
            rig.run("view.week", json!({}));
            (rig, main, team, family)
        }

        /// The block of a calendar: where its tab strip is, which is as wide as its days.
        fn block(&self, calendar: Id) -> Rect {
            self.app.hit.rect("pane", calendar).unwrap_or_else(|| panic!("calendar {calendar} has no block"))
        }

        /// The middle of the column of day number `day` (of `days` in view) in a calendar's block.
        fn column(&self, calendar: Id, day: usize, days: usize) -> f32 {
            let block = self.block(calendar);
            block.min.x + (day as f32 + 0.5) * block.width() / days as f32
        }

        /// Whether the item called `title` is drawn dimmer, as one of a calendar that lies under
        /// another.
        fn dim(&self, title: &str) -> bool {
            let t = Tokens::get(&self.ctx);
            let (from, to) = self.range();
            let items = gather(&self.app, from, to, &t, &pane_views(&self.app));
            items.iter().find(|item| item.ev.title.starts_with(title)).map(|item| item.dim).unwrap_or_else(|| panic!("no {title} in view"))
        }

        fn said(&self) -> String {
            self.app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
        }

        /// The calendar being worked in.
        fn current(&self) -> Id {
            mailcraft_engine::cmd::calendar::current_calendar(&self.app.session)
        }
    }

    #[test]
    fn blocks_of_calendars_share_the_width() {
        // One block is the columns of days as they ever were.
        let one = Blocks::of(100.0, 700.0, 7, 1);
        assert_eq!((one.count, one.cols(0).left, one.cols(0).width, one.cols(0).n), (1, 100.0, 100.0, 7));
        assert_eq!((one.block_at(-50.0), one.block_at(450.0), one.block_at(5000.0)), (0, 0, 0));
        // Three side by side, with a gap after each but the last.
        let three = Blocks::of(100.0, 3.0 * 70.0 + 2.0 * PANE_GAP, 7, 3);
        assert_eq!((three.count, three.cols(0).width, three.pitch), (3, 10.0, 70.0 + PANE_GAP));
        assert_eq!(three.cols(1).left, 100.0 + 70.0 + PANE_GAP);
        assert_eq!(three.cols(2).right(), 100.0 + 3.0 * 70.0 + 2.0 * PANE_GAP);
        assert_eq!((three.left(), three.cols(7).left), (100.0, three.cols(2).left));
        // A point belongs to the block it is in, the gap after a block to that block, and a
        // point beside them all to the nearest.
        assert_eq!((three.block_at(100.0), three.block_at(169.0), three.block_at(175.0), three.block_at(185.0)), (0, 0, 0, 1));
        assert_eq!((three.block_at(0.0), three.block_at(1.0e9), three.block_at(f32::NAN), three.block_at(f32::NEG_INFINITY)), (0, 2, 0, 0));
        assert_eq!(three.at(190.0).index_at(190.0), 1);
        // Nonsense in: still blocks, never more than there is room for, never none.
        for (width, days, count) in [(f32::NAN, 0, 0), (f32::INFINITY, 7, 500), (-100.0, 14, 3), (0.0, usize::MAX, usize::MAX)] {
            let odd = Blocks::of(0.0, width, days, count);
            assert!((1..=MAX_PANES).contains(&odd.count) && odd.cols(0).n >= 1 && odd.cols(0).width >= 1.0, "{odd:?}");
            let _ = (odd.block_at(3.0), odd.at(f32::NAN).index_at(f32::INFINITY), odd.cols(usize::MAX));
        }
        assert_eq!(Blocks { first: Cols { left: 0.0, width: 0.0, n: 0 }, pitch: 0.0, count: 0 }.block_at(10.0), 0);
        assert_eq!(Blocks { first: Cols { left: 0.0, width: 1.0, n: 1 }, pitch: f32::NAN, count: 3 }.block_at(10.0), 0);
    }

    #[test]
    fn several_calendars_stand_side_by_side_in_every_view() {
        let (mut rig, main, team, family) = Rig::with_calendars();
        let narrow = rig.at("Design review").width();
        for theme in ["light", "dark"] {
            rig.run("ui.theme", json!({"value": theme}));
            for view in ["view.day", "view.workWeek", "view.week", "view.month"] {
                rig.run(view, json!({}));
                // Three blocks, from left to right in the order of the list, as wide as one
                // another, none over another, each with its calendar's tab over it.
                let blocks = [rig.block(main), rig.block(team), rig.block(family)];
                assert!(blocks[0].max.x <= blocks[1].min.x && blocks[1].max.x <= blocks[2].min.x, "{view}: {blocks:?}");
                assert!((blocks[0].width() - blocks[2].width()).abs() < 1.0 && blocks[0].width() > 100.0, "{view}: {blocks:?}");
                for (calendar, block) in [main, team, family].into_iter().zip(blocks) {
                    let tab = rig.app.hit.rect("paneTab", calendar).expect("a tab");
                    assert!(tab.min.x >= block.min.x && tab.max.x <= block.max.x + 0.5 && tab.max.y <= block.max.y, "{view}: {tab:?} {block:?}");
                }
                // Each item is in the block of its calendar.
                for (title, block) in [("Design review", blocks[0]), ("Retro", blocks[1]), ("School play", blocks[2])] {
                    let at = rig.at(title);
                    assert!(
                        at.min.x >= block.min.x - 0.5 && at.max.x <= block.max.x + 0.5 && at.min.y > block.max.y,
                        "{view}: {title} {at:?} {block:?}"
                    );
                }
            }
        }
        // Whole days too: Thursday's birthday is a bar in Family's block.
        rig.run("view.week", json!({}));
        let (bar, block) = (rig.at("Birthday"), rig.block(family));
        assert!(bar.min.x > block.min.x && bar.max.x < block.max.x && (bar.height() - BAR_H).abs() < 1.0, "{bar:?} {block:?}");
        // Two calendars have more room each than three.
        let three = rig.block(main).width();
        rig.run("calendar.showCalendar", json!({"id": family, "value": false}));
        assert!(rig.block(main).width() > 1.4 * three);
        assert!(rig.app.hit.rect("pane", family).is_none() && rig.app.hit.rect("event", rig.event("School play").id).is_none());
        // One calendar is the calendar as it ever was: no tab, and its items as wide as the days.
        rig.run("calendar.showCalendar", json!({"id": team, "value": false}));
        assert!(rig.app.hit.rect("pane", main).is_none() && rig.app.hit.rect("paneTab", main).is_none());
        assert!(rig.at("Design review").width() > 2.5 * narrow);
        // None at all: an empty calendar, and still no tab.
        rig.run("calendar.showCalendar", json!({"id": main, "value": false}));
        assert!(rig.app.hit.rect("paneTab", main).is_none() && rig.app.hit.rect("event", rig.event("Design review").id).is_none());
    }

    #[test]
    fn the_arrow_on_a_tab_lays_a_calendar_over_the_others_and_back() {
        let (mut rig, main, team, family) = Rig::with_calendars();
        let side_by_side = rig.block(main).width();
        // Side by side, the first calendar has nothing to its left to lie on; the others have
        // the arrow.
        assert!(rig.app.hit.rect("paneArrow", main).is_none());
        rig.click(rig.app.hit.rect("paneArrow", team).expect("the arrow").center(), 1);
        // Team lies over the first calendar now, and is in front; Family stands beside them.
        assert_eq!((rig.app.session.view.overlay.clone(), rig.app.session.view.overlay_front), (vec![team, main], team));
        assert_eq!(rig.block(main), rig.block(team));
        assert!(rig.block(family).min.x >= rig.block(main).max.x && rig.block(main).width() > 1.4 * side_by_side);
        assert!(!rig.dim("Retro") && rig.dim("Design review") && !rig.dim("School play"));
        // The items of both are in the one block.
        let block = rig.block(main);
        for title in ["Design review", "Retro"] {
            let at = rig.at(title);
            assert!(at.min.x >= block.min.x && at.max.x <= block.max.x, "{title}: {at:?} {block:?}");
        }
        // Their tabs stand next to each other over it. A click on the other's brings that
        // calendar to the front, and makes it the one being worked in.
        let (first, second) = (rig.app.hit.rect("paneTab", main).unwrap(), rig.app.hit.rect("paneTab", team).unwrap());
        assert!(first.max.x <= second.min.x + 0.5 && second.max.x <= block.max.x + 0.5, "{first:?} {second:?}");
        rig.click(first.center(), 1);
        assert_eq!((rig.app.session.view.overlay_front, rig.app.session.view.in_calendar), (main, main));
        assert!(rig.dim("Retro") && !rig.dim("Design review"));
        // So does a click on an item that lies under: its calendar comes to the front.
        let retro = rig.event("Retro").id;
        rig.click(rig.at("Retro").center(), 1);
        assert_eq!((rig.app.session.view.overlay_front, rig.selected().0, rig.current()), (team, Some(retro), team));
        assert!(!rig.dim("Retro"));
        // In the month they lie over one another just the same.
        rig.run("view.month", json!({}));
        assert_eq!(rig.block(main), rig.block(team));
        assert!(rig.dim("Design review") && rig.at("Retro").max.x <= rig.block(main).max.x);
        rig.run("view.week", json!({}));

        // View › Overlay lays them all over one another; View › Side-by-Side stands them apart.
        rig.app.run("view.overlay", json!({})).unwrap();
        rig.frames(3);
        assert!(rig.block(main) == rig.block(family) && rig.block(main) == rig.block(team));
        assert!(rig.block(main).width() > 2.5 * side_by_side);
        rig.app.run("view.sideBySide", json!({})).unwrap();
        rig.frames(3);
        assert!(rig.block(main).max.x <= rig.block(team).min.x && rig.block(team).max.x <= rig.block(family).min.x);
        assert!(!rig.dim("Design review") && !rig.dim("Retro") && !rig.dim("School play"));
        // The arrow on a tab that lies over the others stands its calendar beside them again.
        rig.click(rig.app.hit.rect("paneArrow", family).unwrap().center(), 1);
        assert_eq!(rig.block(main), rig.block(family));
        rig.click(rig.app.hit.rect("paneArrow", family).unwrap().center(), 1);
        assert!(rig.app.session.view.overlay.is_empty() && rig.block(family).min.x >= rig.block(team).max.x);
        // The cross on a tab hides its calendar. None of this is anything to undo.
        rig.click(rig.app.hit.rect("paneClose", team).unwrap().center(), 1);
        assert_eq!(rig.app.session.mailbox.calendar(team).map(|c| c.visible), Some(false));
        assert!(rig.app.hit.rect("pane", team).is_none());
        // What lies over what is kept between runs, like the rest of the layout.
        rig.run("calendar.showCalendar", json!({"id": team, "value": true}));
        rig.run("view.overlay", json!({"calendar": family}));
        let saved: crate::UiState = serde_json::from_value(serde_json::to_value(rig.app.prefs()).unwrap()).unwrap();
        let mut again = MailApp::new(Session::at(rig.app.session.mailbox.clone(), NOW, 0), crate::Services::default());
        again.apply_prefs(saved);
        assert_eq!((again.session.view.overlay.clone(), again.session.view.overlay_front), (vec![family, main], family));
    }

    #[test]
    fn items_that_lie_under_are_dimmer_and_keep_their_calendars_edge() {
        let rig = Rig::sample();
        let t = Tokens::get(&rig.ctx);
        let ev = Event::default();
        let (red, green) = (t.red, t.green);
        // The tint is the item's own colour (its category's), the edge its calendar's.
        let front = look(&ev, red, green, &t, false, false);
        assert_eq!(front.edge, green);
        assert_eq!(front.fill, look(&ev, red, red, &t, false, false).fill);
        assert_ne!(front.fill, look(&ev, green, green, &t, false, false).fill);
        // Under another calendar: fainter, its text dimmer, still told by the colour of its edge.
        let under = look(&ev, red, green, &t, false, true);
        assert!(under.fill != front.fill && under.ink == t.text_dim && under.edge != front.edge && under.kind == front.kind);
        let nearer = |a: Color32, b: Color32| (0..3).map(|i| (i32::from(a.to_array()[i]) - i32::from(b.to_array()[i])).abs()).sum::<i32>();
        assert!(nearer(under.fill, t.surface) < nearer(front.fill, t.surface), "{:?} {:?}", under.fill, front.fill);
        // Out of office stays purple whatever the calendar.
        let away = look(&Event { show_as: ShowAs::OutOfOffice, ..Default::default() }, red, green, &t, false, false);
        assert!(away.edge != green && away.edge.b() > away.edge.g());
    }

    #[test]
    fn what_is_picked_typed_and_dragged_in_a_block_is_that_calendars() {
        use egui::Key;
        let (mut rig, main, team, family) = Rig::with_calendars();
        let (wednesday, thursday) = (d(2026, 10, 14), d(2026, 10, 15));
        // A point in Wednesday's column of a calendar's block, `hours` after noon.
        let in_block = |rig: &Rig, calendar: Id, hours: f32| pos2(rig.column(calendar, 3, 7), after_noon(rig, hours).y);
        let inside = |rig: &Rig, calendar: Id, r: Rect| r.min.x >= rig.block(calendar).min.x - 0.5 && r.max.x <= rig.block(calendar).max.x + 0.5;

        // A click in Team's block picks the half hour there, and Team is the calendar being
        // worked in from then on.
        assert_eq!(rig.current(), main);
        rig.click(in_block(&rig, team, 1.0), 1);
        assert_eq!(rig.pick(), Some((at(wednesday, 13, 0), at(wednesday, 13, 30), false)));
        assert_eq!((rig.slot().map(|s| s.calendar), rig.current()), (Some(team), team));
        assert!(inside(&rig, team, rig.day("slot", wednesday).unwrap()));
        // What is typed is Team's.
        rig.type_text("Sprint");
        rig.key(Key::Enter);
        let sprint = rig.event("Sprint");
        assert_eq!((sprint.calendar, sprint.start), (team, at(wednesday, 13, 0)));
        assert!(inside(&rig, team, rig.at("Sprint")));
        // The keys move the pick within its block: down a row, on a day.
        rig.click(in_block(&rig, family, 3.0), 1);
        rig.key(Key::ArrowDown);
        rig.key(Key::ArrowRight);
        assert_eq!(rig.slot().map(|s| (s.start, s.calendar)), Some((at(thursday, 15, 30), family)));
        assert!(inside(&rig, family, rig.day("slot", thursday).unwrap()));
        rig.type_text("Dinner");
        rig.key(Key::Enter);
        assert_eq!((rig.event("Dinner").calendar, rig.event("Dinner").start, rig.current()), (family, at(thursday, 15, 30), family));
        // With an event selected, the arrows start from it, in its block.
        rig.click(rig.at("Retro").center(), 1);
        rig.key(Key::ArrowDown);
        assert_eq!(rig.slot().map(|s| (s.start, s.calendar)), Some((at(wednesday, 14, 30), team)));
        // The window a double click opens is for a new appointment in the block's calendar.
        rig.click(in_block(&rig, family, 3.0), 1);
        let form = rig.slot().unwrap().form();
        assert_eq!((form["calendar"].as_u64(), form["start"].as_str()), (Some(family), Some("2026-10-14T15:00")));
        rig.click(in_block(&rig, family, 3.0), 2);
        assert_eq!((rig.app.windows.new_events(), rig.dialog()), (1, None));
        rig.run("ui.closeWindow", json!({"all": true}));

        // An edge that is dragged changes the item in its own calendar; so does a move within
        // its block.
        let retro = rig.at("Retro");
        let from = pos2(retro.center().x, retro.max.y - 2.0);
        rig.drag(from, from + vec2(0.0, 0.5 * HOUR_H));
        assert_eq!(rig.dragging().map(|drag| drag.into), Some(0));
        rig.drop_at(from + vec2(0.0, 0.5 * HOUR_H));
        assert_eq!((rig.event("Retro").end, rig.event("Retro").calendar), (at(wednesday, 15, 30), team));
        let from = rig.at("School play").center();
        rig.drag(from, from + vec2(0.0, HOUR_H));
        assert_eq!(rig.dragging().map(|drag| drag.into), Some(0));
        rig.drop_at(from + vec2(0.0, HOUR_H));
        assert_eq!((rig.event("School play").start, rig.event("School play").calendar), (at(wednesday, 14, 0), family));

        // The row of whole days: a click in Team's block, and what is typed is all of that day
        // in Team's calendar.
        let row = rig.day("allDay", wednesday).unwrap();
        rig.click(pos2(rig.column(team, 3, 7), row.max.y - 4.0), 1);
        assert_eq!(rig.slot().map(|s| (s.all_day, s.start.date, s.calendar)), Some((true, wednesday, team)));
        rig.type_text("Offsite");
        rig.key(Key::Enter);
        let offsite = rig.event("Offsite");
        assert_eq!((offsite.calendar, offsite.all_day, offsite.start.date), (team, true, wednesday));
        assert!(inside(&rig, team, rig.at("Offsite")));
        // And the month: a day in Family's block.
        rig.run("view.month", json!({}));
        let cell = rig.day("day", d(2026, 10, 20)).unwrap();
        rig.click(pos2(rig.column(family, 2, 7), cell.min.y + 8.0), 1);
        assert_eq!(rig.slot().map(|s| (s.all_day, s.start.date, s.calendar)), Some((true, d(2026, 10, 20), family)));
        assert!(inside(&rig, family, rig.day("slot", d(2026, 10, 20)).unwrap()));
        rig.type_text("Fair");
        rig.key(Key::Enter);
        assert_eq!((rig.event("Fair").calendar, rig.event("Fair").start.date), (family, d(2026, 10, 20)));
        assert!(inside(&rig, family, rig.at("Fair")));
        // Laid over one another, what is picked belongs to the calendar in front, whichever
        // that is when the typing is done.
        rig.run("view.week", json!({}));
        rig.run("view.overlay", json!({}));
        rig.run("view.frontCalendar", json!({"calendar": team}));
        rig.click(pos2(rig.column(team, 5, 7), after_noon(&rig, 1.0).y), 1);
        assert_eq!(rig.slot().map(|s| s.calendar), Some(team));
        rig.run("view.frontCalendar", json!({"calendar": main}));
        assert_eq!(rig.slot().map(|s| s.calendar), Some(main));
        rig.type_text("In front");
        rig.key(Key::Enter);
        assert_eq!(rig.event("In front").calendar, main);
    }

    #[test]
    fn a_drop_in_another_calendars_block_takes_the_item_there() {
        let (mut rig, main, team, family) = Rig::with_calendars();
        let (wednesday, thursday) = (d(2026, 10, 14), d(2026, 10, 15));
        // The retro, from Team's block to the same time in the first calendar's. While it is
        // dragged its ghost is in the block it would land in.
        let retro = rig.event("Retro");
        let from = rig.at("Retro").center();
        let to = pos2(rig.column(main, 3, 7), from.y);
        rig.drag(from, to);
        assert_eq!(rig.dragging().map(|drag| (drag.into, drag.target.map(|span| span.start))), Some((main, Some(retro.start))));
        let ghost = rig.app.hit.rect("ghost", retro.id).expect("the ghost");
        assert!(ghost.min.x >= rig.block(main).min.x && ghost.max.x <= rig.block(main).max.x, "{ghost:?}");
        assert_eq!(rig.event("Retro").calendar, team, "nothing moves until the button is let go");
        rig.drop_at(to);
        let moved = rig.event("Retro");
        assert_eq!((moved.id, moved.calendar, moved.start, moved.end), (retro.id, main, retro.start, retro.end));
        assert_eq!((rig.app.session.undo_label(), rig.selected().0, rig.dialog()), (Some("Edit Event"), Some(retro.id), None));
        assert!(rig.at("Retro").max.x <= rig.block(main).max.x + 0.5);
        // To another day and time in another block, in one go: Thursday at noon, in Family's.
        let from = rig.at("Retro").center();
        let to = pos2(rig.column(family, 4, 7), from.y - 2.0 * HOUR_H);
        rig.drag(from, to);
        assert_eq!(rig.target(), Some((at(thursday, 12, 0), at(thursday, 13, 0), false)));
        rig.drop_at(to);
        let moved = rig.event("Retro");
        assert_eq!((moved.calendar, moved.start, moved.end), (family, at(thursday, 12, 0), at(thursday, 13, 0)));
        // With Ctrl held it is a copy that goes there, and the original stays where it is.
        let play = rig.event("School play");
        let from = rig.at("School play").center();
        let to = pos2(rig.column(team, 3, 7), from.y + HOUR_H);
        rig.drag(from, to);
        rig.mods = COPY;
        rig.frames(2);
        assert!(rig.app.hit.rect("ghostCopy", play.id).is_some_and(|ghost| ghost.max.x <= rig.block(team).max.x + 0.5));
        rig.drop_at(to);
        rig.mods = egui::Modifiers::NONE;
        rig.frames(2);
        let plays = rig.all("School play");
        assert_eq!(plays.len(), 2, "{plays:?}");
        assert_eq!((plays[0].0, plays[0].1), (play.id, play.start));
        let copy = rig.app.session.mailbox.event(plays[1].0).cloned().unwrap();
        assert_eq!((copy.calendar, copy.start), (team, at(wednesday, 14, 0)));
        assert_eq!(rig.event("School play").calendar, family);
        // The right button asks, as it does within a block; Move takes it there.
        let release = rig.event("Release");
        let from = rig.at("Release").center();
        let to = pos2(rig.column(main, 4, 7), from.y);
        rig.drag_with(egui::PointerButton::Secondary, from, to);
        rig.drop_with(egui::PointerButton::Secondary, to);
        let ask = rig.app.hit.rect("dropMenu", 0).expect("Move, Copy or Cancel is asked, though the time is the same");
        rig.click(ask.center(), 1);
        assert_eq!((rig.event("Release").calendar, rig.event("Release").start), (main, release.start));

        // An item that repeats stays in its calendar: Monday's stand-up, dropped in Team's block.
        // Why is said, and nothing has changed.
        let standup = rig.event("Team stand-up");
        let from = rig.at("Team stand-up").center();
        let to = pos2(rig.column(team, 1, 7), from.y);
        rig.drag(from, to);
        rig.drop_at(to);
        assert_eq!((rig.dialog(), rig.all("Team stand-up").len()), (None, 1));
        assert_eq!(*rig.event("Team stand-up"), *standup);
        assert!(rig.said().starts_with("An item that repeats"), "{}", rig.said());
        // A copy of that day's can go there.
        rig.drag(from, to);
        rig.mods = COPY;
        rig.frames(2);
        rig.drop_at(to);
        rig.mods = egui::Modifiers::NONE;
        rig.frames(2);
        let all = rig.all("Team stand-up");
        assert_eq!(all.len(), 2, "{all:?}");
        let copy = all.iter().find(|one| one.0 != standup.id).and_then(|one| rig.app.session.mailbox.event(one.0)).cloned().unwrap();
        assert_eq!((copy.calendar, copy.start.date, copy.recurrence.is_some()), (team, d(2026, 10, 12), false));
        // Cut and paste goes by the block the time is picked in, just the same.
        rig.click(rig.at("Release").center(), 1);
        rig.chord(CTRL, egui::Key::X);
        rig.click(pos2(rig.column(team, 5, 7), after_noon(&rig, 1.0).y), 1);
        rig.chord(CTRL, egui::Key::V);
        rig.frames(2);
        let pasted = rig.event("Release");
        assert_eq!((pasted.id, pasted.calendar, pasted.start), (release.id, team, at(d(2026, 10, 16), 13, 0)));
        rig.app.status("");
        rig.click(from, 1);
        assert_eq!(rig.selected(), (Some(standup.id), Some(d(2026, 10, 12))));
        rig.chord(CTRL, egui::Key::X);
        rig.click(pos2(rig.column(family, 5, 7), after_noon(&rig, 2.0).y), 1);
        rig.chord(CTRL, egui::Key::V);
        rig.frames(2);
        assert!(rig.said().starts_with("An item that repeats"), "{}", rig.said());
        assert_eq!(rig.app.session.mailbox.event(standup.id).map(|e| (e.calendar, e.start)), Some((main, standup.start)));
    }

    /// The account's server keeps two calendars, side by side: an appointment dropped from one
    /// into the other is made there and then taken out of the first (the server has no way of
    /// moving one); a meeting stays where it is, and is told why; Ctrl copies.
    #[test]
    fn an_appointment_dropped_in_another_calendar_of_its_account_moves_there() {
        use crate::calendar_sync::testing::event as on_server;
        use mailcraft_model::remote::{CalendarsDownload, EventChangeKind, RemoteCalendar, RemoteEvent};
        let invited =
            |name: &str, email: &str| mailcraft_model::Attendee { address: mailcraft_model::Address::new(name, email), ..Default::default() };
        let meeting = RemoteEvent { attendees: vec![invited("Bo", "bo@example.org")], ..on_server("m-1", "Roadmap", NOW + 20 * 3600) };
        let (mut rig, server, account) = Rig::served(vec![on_server("e-1", "Physio", NOW + 18 * 3600), meeting]);
        let list = CalendarsDownload {
            account,
            email: "avery@mail.example".into(),
            complete: true,
            calendars: vec![
                RemoteCalendar { id: "c-main".into(), name: "Calendar".into(), main: true, ..Default::default() },
                RemoteCalendar { id: "c-side".into(), name: "Side".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        mailcraft_engine::cmd::remote::apply_calendars(&mut rig.app.session, &list).unwrap();
        let of = |rig: &Rig, name: &str| {
            rig.app.session.mailbox.calendars.iter().find(|c| c.origin.account == account && c.origin.id == name).map(|c| c.id).unwrap()
        };
        let (main, side) = (of(&rig, ""), of(&rig, "c-side"));
        rig.run("calendar.showCalendar", json!({"id": side, "value": true}));
        rig.frames(6);
        let jobs = server.lock().unwrap().jobs.len();

        // The appointment, into the other calendar's block, on its own day and at its own time.
        let physio = rig.event("Physio");
        assert_eq!(physio.calendar, main);
        let from = rig.at("Physio").center();
        let to = pos2(rig.column(side, 4, 7), from.y);
        rig.drag(from, to);
        assert_eq!(rig.dragging().map(|drag| drag.into), Some(side));
        rig.drop_at(to);
        rig.frames(10);
        // Here: the same item, in the other calendar. There: one of it, in the other calendar,
        // and no longer in the one it was in.
        let moved = rig.event("Physio");
        assert_eq!((moved.id, moved.calendar, moved.start, rig.dialog()), (physio.id, side, physio.start, None));
        assert_eq!(rig.all("Physio").len(), 1);
        assert!(rig.app.session.mailbox.event_changes.is_empty(), "both steps are through: {:?}", rig.app.session.mailbox.event_changes);
        {
            let there = server.lock().unwrap();
            let copies: Vec<&RemoteEvent> = there.events.iter().filter(|e| e.title == "Physio").collect();
            assert_eq!(copies.len(), 1);
            assert_eq!((copies[0].id.as_str() != "e-1", there.placed.get(&copies[0].id).map(String::as_str)), (true, Some("c-side")));
            assert_eq!(moved.origin.id, copies[0].id);
            // What it was told, in order: make it in the other calendar, and only then delete the original.
            let told: Vec<(EventChangeKind, String, String)> =
                there.jobs.iter().skip(jobs).flat_map(|job| job.ops.iter()).map(|op| (op.kind, op.calendar.clone(), op.remote.clone())).collect();
            assert_eq!(
                told,
                [(EventChangeKind::Create, "c-side".to_string(), String::new()), (EventChangeKind::Delete, String::new(), "e-1".to_string())]
            );
            assert!(there.mail.is_empty(), "nobody is written to about an appointment");
        }

        // A meeting stays where it is: the server couldn't move it without writing to everyone.
        let planning = rig.event("Roadmap");
        let from = rig.at("Roadmap").center();
        let to = pos2(rig.column(side, 4, 7), from.y);
        rig.drag(from, to);
        rig.drop_at(to);
        rig.frames(6);
        // Refused at once: the reason in a message box (#152) and in the status bar; OK (Enter)
        // closes it.
        assert_eq!((rig.event("Roadmap").calendar, rig.dialog()), (main, Some("refused")));
        assert!(rig.said().starts_with("A meeting can't be moved to another calendar"), "{}", rig.said());
        rig.key(egui::Key::Enter);
        rig.frames(2);
        assert_eq!(rig.dialog(), None);
        assert_eq!(*rig.event("Roadmap"), *planning);
        // With Ctrl it is copied there: an appointment of its own, to which nobody is invited.
        rig.drag(from, to);
        rig.mods = COPY;
        rig.frames(2);
        rig.drop_at(to);
        rig.mods = egui::Modifiers::NONE;
        rig.frames(8);
        let all = rig.all("Roadmap");
        assert_eq!(all.len(), 2, "{all:?}");
        let copy = all.iter().find(|one| one.0 != planning.id).and_then(|one| rig.app.session.mailbox.event(one.0)).cloned().unwrap();
        assert!(copy.calendar == side && copy.attendees.is_empty() && !copy.origin.id.is_empty(), "{copy:?}");
        assert!(server.lock().unwrap().mail.is_empty());
    }

    #[test]
    fn a_drop_between_a_servers_calendar_and_one_kept_here() {
        use crate::calendar_sync::testing::event as on_server;
        let (mut rig, server, account) = Rig::served(vec![on_server("e-1", "Physio", NOW + 18 * 3600)]);
        let mailbox = &rig.app.session.mailbox;
        let of_server = mailbox.calendars.iter().find(|c| c.origin.account == account).map(|c| c.id).unwrap();
        let own = mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).unwrap();
        // One of the mailbox's own, dropped in the server's block: it is the server's from then
        // on, which has it at once.
        let lunch = rig.event("Lunch with Dev");
        assert_eq!(lunch.calendar, own);
        let from = rig.at("Lunch with Dev").center();
        let to = pos2(rig.column(of_server, 4, 7), from.y);
        rig.drag(from, to);
        rig.drop_at(to);
        rig.frames(6);
        let moved = rig.event("Lunch with Dev");
        assert_eq!((moved.id, moved.calendar, moved.start, moved.origin.id.as_str()), (lunch.id, of_server, lunch.start, "made-1"));
        assert_eq!(server.lock().unwrap().events.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Physio", "Lunch with Dev"]);
        // What a server keeps can't be taken out of its calendar from here (it would be deleted
        // there): the drop is refused, and the engine's reason is said.
        let physio = rig.event("Physio");
        let from = rig.at("Physio").center();
        let to = pos2(rig.column(own, 4, 7), from.y);
        rig.drag(from, to);
        assert_eq!(rig.dragging().map(|drag| drag.into), Some(own), "the gesture is there to be made");
        rig.drop_at(to);
        rig.frames(6);
        assert_eq!((rig.event("Physio").calendar, rig.dialog()), (of_server, Some("refused")));
        assert!(rig.said().contains("kept by the server") && rig.said().contains("not out of it"), "{}", rig.said());
        rig.key(egui::Key::Enter);
        rig.frames(2);
        assert_eq!(rig.dialog(), None);
        assert_eq!(server.lock().unwrap().events.len(), 2);
        // A copy of it can be kept here, and nothing of that reaches the server.
        rig.drag(from, to);
        rig.mods = COPY;
        rig.frames(2);
        rig.drop_at(to);
        rig.mods = egui::Modifiers::NONE;
        rig.frames(6);
        let all = rig.all("Physio");
        assert_eq!(all.len(), 2);
        let copy = all.iter().find(|one| one.0 != physio.id).and_then(|one| rig.app.session.mailbox.event(one.0)).cloned().unwrap();
        assert_eq!((copy.calendar, copy.origin.is_local(), copy.start), (own, true, physio.start));
        assert_eq!(server.lock().unwrap().events.len(), 2);
        assert!(rig.app.session.mailbox.event_changes.is_empty());
    }

    #[test]
    fn the_list_has_the_calendars_by_group_and_groups_fold_away() {
        use crate::calendar_sync::testing::event as on_server;
        let (mut rig, _server, account) = Rig::served(vec![on_server("e-1", "Dentist", NOW + 18 * 3600)]);
        let download = json!({"account": account, "complete": true, "calendars": [
            {"id": "c-hol", "name": "Holidays", "main": false, "rights": "read", "group": "Other Calendars"},
            {"id": "c-team", "name": "Team", "main": false, "rights": "edit", "group": "Projects", "color": [46, 139, 87]},
            {"id": "c-main", "name": "Calendar", "main": true, "rights": "edit", "group": "My Calendars"},
        ]});
        rig.run("sync.applyCalendars", json!({"download": download}));
        let id_of = |rig: &Rig, name: &str| rig.app.session.mailbox.calendars.iter().find(|c| c.name == name).map(|c| c.id).unwrap();
        let (main, team, holidays, own) = (id_of(&rig, "Calendar"), id_of(&rig, "Team"), id_of(&rig, "Holidays"), id_of(&rig, "On this computer"));
        // Only the account's own calendar is ticked when the list first comes: the others are
        // listed, and shown once the owner ticks them.
        let ticked = |rig: &Rig, id: Id| rig.app.session.mailbox.calendar(id).map(|c| c.visible);
        assert_eq!((ticked(&rig, main), ticked(&rig, team), ticked(&rig, holidays)), (Some(true), Some(false), Some(false)));
        assert!(rig.app.hit.rect("pane", team).is_none() && rig.app.hit.rect("calendar", team).is_some());
        for id in [team, holidays] {
            rig.click(rig.app.hit.rect("calendar", id).unwrap().center(), 1);
        }
        assert_eq!((ticked(&rig, team), ticked(&rig, holidays)), (Some(true), Some(true)));
        // The account's own first, then its other groups, the calendars of other people and
        // places, and last what is kept on this computer.
        let groups = calendar_groups(&rig.app);
        let listed: Vec<(&str, Vec<Id>)> = groups.iter().map(|group| (group.title.as_str(), group.rows.iter().map(|row| row.id).collect())).collect();
        assert_eq!(
            listed,
            [("My Calendars", vec![main]), ("Projects", vec![team]), ("Other Calendars", vec![holidays]), ("On this computer", vec![own])]
        );
        // Each heading over its calendars, down the side pane.
        let heading = |rig: &Rig, n: u64| rig.app.hit.rect("calendarGroup", n).unwrap_or_else(|| panic!("no heading {n}"));
        let row = |rig: &Rig, id: Id| rig.app.hit.rect("calendar", id);
        let mut last = 0.0;
        for (n, calendar) in [main, team, holidays, own].into_iter().enumerate() {
            let (head, under) = (heading(&rig, n as u64), row(&rig, calendar).expect("its row"));
            assert!(head.min.y >= last && under.min.y >= head.max.y, "{n}: {head:?} {under:?}");
            last = under.max.y;
        }
        // Every calendar's check box is in its own colour: the one the server gave it.
        assert_eq!(groups[1].rows[0].color, Color32::from_rgb(46, 139, 87));

        // A click on a heading folds its calendars away: the rows below move up, the calendar
        // itself stays in view, and the next run has it folded still.
        let before = row(&rig, holidays).unwrap();
        rig.click(heading(&rig, 1).center(), 1);
        assert_eq!(rig.app.ui.collapsed_calendar_groups, ["Projects"]);
        assert!(row(&rig, team).is_none() && rig.app.hit.rect("pane", team).is_some());
        assert!(row(&rig, holidays).unwrap().min.y < before.min.y);
        let saved = serde_json::to_value(rig.app.prefs()).unwrap();
        assert_eq!(saved["collapsedCalendarGroups"], json!(["Projects"]));
        let mut again = MailApp::new(Session::at(rig.app.session.mailbox.clone(), NOW, 0), crate::Services::default());
        again.apply_prefs(serde_json::from_value(saved).unwrap());
        assert!(calendar_groups(&again)[1].folded && !calendar_groups(&again)[0].folded);
        // Settings from before there was anything to fold load as they were.
        let old: crate::UiState = serde_json::from_value(json!({"tab": "Home", "folderPaneWidth": 240.0})).unwrap();
        assert!(old.collapsed_calendar_groups.is_empty() && old.folder_pane_width == 240.0);
        // Another click brings them back.
        rig.click(heading(&rig, 1).center(), 1);
        assert!(rig.app.ui.collapsed_calendar_groups.is_empty() && row(&rig, team).is_some());
        // The command, as a script runs it; and what it can't do.
        assert_eq!(rig.app.execute("ui.toggleCalendarGroup", json!({"group": " Other Calendars\n", "value": true})).unwrap()["collapsed"], true);
        assert_eq!(rig.app.ui.collapsed_calendar_groups, ["Other Calendars"]);
        assert_eq!(rig.app.execute("ui.toggleCalendarGroup", json!({"group": "Other Calendars"})).unwrap()["collapsed"], false);
        for odd in [json!({}), json!({"group": ""}), json!({"group": 7}), json!({"group": "\u{0}\r\n"})] {
            assert!(rig.app.execute("ui.toggleCalendarGroup", odd).is_err());
        }
        for n in 0..500 {
            let _ = rig.app.execute("ui.toggleCalendarGroup", json!({"group": format!("Group {n} {}", "x".repeat(5000)), "value": true}));
        }
        assert!(
            rig.app.ui.collapsed_calendar_groups.len() <= MAX_FOLDED
                && rig.app.ui.collapsed_calendar_groups.iter().all(|key| key.len() <= MAX_HEADING)
        );
        rig.app.ui.collapsed_calendar_groups.clear();
        rig.frames(2);

        // A calendar that can only be read says so, and is not the one new items go to.
        assert!(rig.app.hit.rect("calendarReadOnly", holidays).is_some() && rig.app.hit.rect("calendarReadOnly", team).is_none());
        let before = rig.current();
        rig.click(rig.app.hit.rect("calendarName", holidays).unwrap().center(), 1);
        assert_eq!(rig.current(), before);
        assert_eq!(rig.said(), "This calendar can only be read: new items can't go to it");
        // Nor does working in its block make it that; what is typed there is refused, and the
        // engine's reason said.
        rig.click(pos2(rig.column(holidays, 3, 7), after_noon(&rig, 2.0).y), 1);
        assert_eq!((rig.slot().map(|s| s.calendar), rig.current()), (Some(holidays), before));
        let events = rig.app.session.mailbox.events.len();
        rig.type_text("Not here");
        rig.key(egui::Key::Enter);
        assert_eq!(rig.app.session.mailbox.events.len(), events);
        assert!(rig.said().contains("can only be read"), "{}", rig.said());
        // (In a message box too, #152: OK closes it.)
        assert_eq!(rig.dialog(), Some("refused"));
        rig.key(egui::Key::Enter);
        rig.frames(2);
        assert_eq!(rig.dialog(), None);
        // One that can be changed is picked by its name as ever.
        rig.click(rig.app.hit.rect("calendarName", team).unwrap().center(), 1);
        assert_eq!(rig.current(), team);

        // Show Only This Calendar, and a colour of the owner's choosing, which wins over the
        // server's and stays when the server says its own again.
        rig.run("calendar.color", json!({"id": team, "color": "#D9534F"}));
        let red = Color32::from_rgb(0xD9, 0x53, 0x4F);
        assert_eq!(calendar_groups(&rig.app)[1].rows[0].color, red);
        assert_eq!(pane_views(&rig.app).iter().flat_map(|pane| &pane.tabs).find(|tab| tab.id == team).map(|tab| tab.color), Some(red));
        rig.run("sync.applyCalendars", json!({"download": download}));
        assert_eq!(calendar_groups(&rig.app)[1].rows[0].color, red);
        rig.run("calendar.color", json!({"id": team, "color": null}));
        assert_eq!(calendar_groups(&rig.app)[1].rows[0].color, Color32::from_rgb(46, 139, 87));
        rig.run("calendar.showOnly", json!({"id": team}));
        assert!(rig.app.hit.rect("paneTab", team).is_none() && rig.app.session.mailbox.calendars.iter().all(|c| c.visible == (c.id == team)));
    }

    #[test]
    fn the_right_button_on_a_calendar_shows_it_alone_or_gives_it_a_colour() {
        use egui_kittest::kittest::Queryable as _;
        // (The same calendars in a window of the kind that can be asked what its menus say.)
        let rig = Rig::with_calendars().0;
        let team = rig.app.session.mailbox.calendars.iter().find(|c| c.name == "Team").map(|c| c.id).unwrap();
        let mut app = MailApp::new(Session::at(rig.app.session.mailbox.clone(), NOW, 0), crate::Services::default());
        app.autosave = false;
        app.auto_sync = false;
        app.execute("view.calendar", json!({})).unwrap();
        app.execute("view.week", json!({})).unwrap();
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(vec2(1280.0, 800.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.run_steps(4);
        let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
        harness.run_steps(3);
        let name = harness.state().hit.rect("calendarName", team).unwrap().center();
        type Window = egui_kittest::Harness<'static, MailApp>;
        let menu = |harness: &mut Window| {
            harness.event(egui::Event::PointerMoved(name));
            harness.step();
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: name,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.step();
            }
            harness.run_steps(3);
        };
        let says = |harness: &Window, text: &str| harness.query_all_by_label_contains(text).next().is_some();
        menu(&mut harness);
        assert!(says(&harness, "Show Only This Calendar") && says(&harness, "Overlay") && says(&harness, "Colour"));
        // A colour from the palette is the calendar's from then on.
        harness.get_by_label_contains("Colour").click();
        harness.run_steps(4);
        assert!(says(&harness, "Automatic") && says(&harness, "Teal"));
        harness.get_by_label_contains("Teal").click();
        harness.run_steps(4);
        let teal = mailcraft_engine::cmd::calendar_settings::CALENDAR_COLORS.iter().find(|(name, _)| *name == "Teal").map(|(_, rgb)| *rgb);
        assert_eq!(harness.state().session.mailbox.calendar(team).and_then(|c| c.own_color), teal);
        assert_eq!(harness.state().session.undo_label(), Some("Calendar Colour"));
        // Show Only This Calendar hides the others.
        menu(&mut harness);
        harness.get_by_label_contains("Show Only This Calendar").click();
        harness.run_steps(4);
        assert!(harness.state().session.mailbox.calendars.iter().all(|c| c.visible == (c.id == team)));
    }

    #[test]
    fn working_hours_come_from_the_mailbox_once_an_hour() {
        use egui::Key;
        use mailcraft_engine::cmd::calendar_settings::{MailboxHours, work_hours};
        let (mut rig, server, account) = Rig::served(Vec::new());
        let wednesday = d(2026, 10, 14);
        let asked = |server: &std::sync::Arc<std::sync::Mutex<crate::calendar_sync::testing::Server>>| {
            server.lock().unwrap().jobs.iter().filter(|job| job.settings).count()
        };
        let hours = |rig: &Rig| {
            let work = work_hours(&rig.app.session);
            (work.days.len(), work.start, work.end, work.from_mailbox)
        };
        // The first look at the calendar asked for them; the mailbox had none to give, so the
        // local options hold: Monday to Friday, eight to five.
        assert_eq!((asked(&server), hours(&rig)), (1, (5, 480, 1020, false)));
        // Now it has: Sunday to Thursday, seven to three. Not asked again within the hour,
        // however often the calendar is looked at.
        let days = vec![Weekday::Sun, Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu];
        let theirs = MailboxHours { days, start: 420, end: 900, zone: "Pacific Standard Time".into(), ..Default::default() };
        server.lock().unwrap().hours = Some(theirs.clone());
        let t0 = crate::now_ms();
        let mut looks = 0;
        for minute in 1..59 {
            looks += usize::from(rig.app.calendar_tick_at(true, t0 + f64::from(minute) * 60_000.0));
            rig.app.poll_calendar();
        }
        assert!(looks > 50, "the calendar itself was asked for every time: {looks}");
        assert_eq!((asked(&server), hours(&rig)), (1, (5, 480, 1020, false)));
        // After the hour they are asked for, and they are the working hours from then on.
        assert!(rig.app.calendar_tick_at(true, t0 + 61.0 * 60_000.0));
        rig.app.poll_calendar();
        // (They are Pacific time's hours: seven to three there is fourteen to twenty-two by the
        // clock here, which is UTC's in these tests.)
        assert_eq!((asked(&server), hours(&rig)), (2, (5, 840, 1320, true)));
        assert_eq!(
            rig.app.session.prefs.mailbox_hours.as_ref().map(|kept| (kept.email.as_str(), kept.zone.as_str())),
            Some(("avery@mail.example", "Pacific Standard Time"))
        );
        // Home and End go to their ends.
        rig.frames(3);
        rig.click(after_noon(&rig, 1.0), 1);
        rig.key(Key::Home);
        assert_eq!(rig.pick().map(|pick| pick.0), Some(at(wednesday, 14, 0)));
        rig.key(Key::End);
        assert_eq!(rig.pick().map(|pick| pick.0), Some(at(wednesday, 21, 30)));
        // The Work Week is the mailbox's days: Sunday to Thursday, in a week that starts on Sunday.
        rig.run("view.workWeek", json!({}));
        assert_eq!(rig.range(), (d(2026, 10, 11), d(2026, 10, 15)));
        // "Use my own working hours": the local options again, the mailbox's kept for later.
        rig.run("view.workHours", json!({"own": true}));
        assert_eq!((hours(&rig), rig.range()), ((5, 480, 1020, false), (d(2026, 10, 12), d(2026, 10, 16))));
        rig.run("view.week", json!({}));
        rig.click(after_noon(&rig, 1.0), 1);
        rig.key(Key::Home);
        assert_eq!(rig.pick().map(|pick| pick.0), Some(at(wednesday, 8, 0)));
        rig.run("view.workHours", json!({"own": false}));
        assert_eq!(hours(&rig), (5, 840, 1320, true));

        // "Connect" asks for them at the next look, hour or no hour. Hours that are none (no
        // days, an end before the start) are not taken in, and the ones that were kept stay.
        server.lock().unwrap().hours = Some(MailboxHours { days: Vec::new(), start: 900, end: 60, ..Default::default() });
        rig.app.calendar_connected(account);
        assert!(rig.app.calendar_tick_at(true, t0 + 62.0 * 60_000.0));
        rig.app.poll_calendar();
        assert_eq!((asked(&server), hours(&rig)), (3, (5, 840, 1320, true)));
        // A sign-in that doesn't cover the mailbox's settings brings none, and says so quietly.
        server.lock().unwrap().extras_missing = true;
        rig.app.calendar_connected(account);
        assert!(rig.app.calendar_tick_at(true, t0 + 63.0 * 60_000.0));
        rig.app.poll_calendar();
        assert!(rig.app.graph_extras_missing(account));
        assert_eq!((asked(&server), hours(&rig)), (4, (5, 840, 1320, true)));
        // Once they come, the sign-in is known to cover them.
        server.lock().unwrap().extras_missing = false;
        server.lock().unwrap().hours = Some(theirs);
        rig.app.calendar_connected(account);
        assert!(rig.app.calendar_tick_at(true, t0 + 64.0 * 60_000.0));
        rig.app.poll_calendar();
        assert!(!rig.app.graph_extras_missing(account));
    }

    #[test]
    fn a_quiet_line_offers_to_connect_for_the_optional_extras() {
        let (mut rig, _server, account) = Rig::served(Vec::new());
        let own = rig.app.session.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).unwrap();
        assert!(rig.app.hit.rect("calendarExtras", account).is_none());
        // The sign-in covers the calendar and not the mailbox's categories and working hours:
        // one line under the list of calendars, and no bar across the calendar.
        rig.app.calendar.extras_missing.insert(account);
        rig.frames(2);
        let line = rig.app.hit.rect("calendarExtras", account).expect("the line");
        let last_row = rig.app.hit.rect("calendar", own).unwrap();
        assert!(line.min.y >= last_row.max.y && line.max.x < rig.at("Design review").min.x && line.height() <= EXTRAS_H, "{line:?} {last_row:?}");
        assert!(rig.app.hit.rect("calendarConnect", account).is_none());
        let before = rig.at("Design review");
        // Its link has the host sign the owner in once more, for Microsoft Graph.
        let asked: std::sync::Arc<std::sync::Mutex<Vec<(Id, bool, std::sync::mpsc::Sender<Result<mailcraft_model::sync::Tokens, String>>)>>> =
            Default::default();
        let seen = asked.clone();
        rig.app.services.sign_in = Some(Box::new(move |request: crate::SignInRequest| {
            let (tx, rx) = std::sync::mpsc::channel();
            seen.lock().unwrap().push((request.account, request.graph, tx));
            rx
        }));
        rig.click(line.center(), 1);
        assert_eq!(asked.lock().unwrap().iter().map(|(account, graph, _)| (*account, *graph)).collect::<Vec<_>>(), [(account, true)]);
        assert!(rig.app.signing_in());
        assert_eq!(rig.selected(), (None, None));
        let _ = rig.app.execute("ui.cancelSignIn", json!({}));
        // A calendar that can't be read at all says so across the top, and the line keeps quiet.
        rig.app.graph_consent_needed.insert(account);
        rig.frames(2);
        assert!(rig.app.hit.rect("calendarExtras", account).is_none() && rig.app.hit.rect("calendarConnect", account).is_some());
        rig.app.graph_consent_needed.remove(&account);
        rig.frames(2);
        assert_eq!(rig.at("Design review"), before, "the line takes nothing from the calendar");
        // With the extras there, the line goes.
        let hours = mailcraft_engine::cmd::calendar_settings::MailboxHours { days: vec![Weekday::Mon], start: 480, end: 1020, ..Default::default() };
        rig.app.take_in_mailbox_hours(account, &hours);
        rig.frames(2);
        assert!(rig.app.hit.rect("calendarExtras", account).is_none());
    }

    #[test]
    fn hostile_arrangements_of_calendars_draw_without_panicking() {
        let mut rig = Rig::sample();
        // Far more calendars than there are blocks, with names nobody would give them.
        for n in 0..60 {
            let name = if n == 7 { format!("{}\u{202e}{n}", "long ".repeat(20)) } else { format!("Extra {n}") };
            let made = rig.app.execute("calendar.addCalendar", json!({"name": name})).unwrap()["id"].as_u64().unwrap();
            let start = format!("2026-10-14T{:02}:00", 8 + n % 9);
            rig.app.execute("calendar.new", json!({"title": format!("In {n}"), "start": start, "calendar": made, "reminder": null})).unwrap();
        }
        if let Some(calendar) = rig.app.session.mailbox.calendars.last_mut() {
            calendar.name = "x\n\u{202e}é".repeat(50_000);
            calendar.group = "\u{0}\r\n".repeat(1000);
        }
        let all: Vec<Id> = rig.app.session.mailbox.calendars.iter().map(|c| c.id).collect();
        let every_view = |rig: &mut Rig| {
            for view in ["view.day", "view.workWeek", "view.week", "view.month"] {
                rig.run(view, json!({}));
            }
        };
        every_view(&mut rig);
        assert_eq!(pane_views(&rig.app).len(), MAX_PANES);
        // All over one another; over calendars that are not there; with a front that is none.
        rig.run("view.overlay", json!({}));
        every_view(&mut rig);
        rig.app.session.view.overlay = vec![0, u64::MAX, 3, 3, 3, all[2], all[1]];
        rig.app.session.view.overlay_front = u64::MAX;
        every_view(&mut rig);
        // A pick in a calendar that is gone, a drag from a block that isn't there.
        let mut odd = Slot::timed(d(2026, 10, 14), 600, 30).in_calendar(u64::MAX);
        odd.frame = u64::MAX;
        rig.ctx.data_mut(|data| data.insert_temp(egui::Id::unique(SLOT_ID), odd));
        let stale = Drag {
            pane: usize::MAX,
            over: usize::MAX,
            into: u64::MAX,
            ..grabbed(at(d(2026, 10, 14), 9, 0), at(d(2026, 10, 14), 10, 0), false, Grip::Move, Under::Day(d(1, 1, 1)))
        };
        rig.ctx.data_mut(|data| data.insert_temp(egui::Id::unique(DRAG_ID), stale));
        rig.frames(3);
        let review = Drag { event: rig.event("Design review").id, ..stale };
        drop_event(&mut rig.app, &review, stale.here(), false);
        drop_event(&mut rig.app, &review, stale.here(), true);
        // Nothing shown at all; a window too narrow for the blocks it has.
        for id in &all {
            rig.app.execute("calendar.showCalendar", json!({"id": id, "value": false})).unwrap();
        }
        every_view(&mut rig);
        for id in &all {
            rig.app.execute("calendar.showCalendar", json!({"id": id, "value": true})).unwrap();
        }
        rig.app.ui.folder_pane_width = 360.0;
        rig.app.ui.collapsed_calendar_groups = vec!["My Calendars".into(); 5000];
        rig.run("view.days", json!({"count": 14}));
        every_view(&mut rig);
        // Working hours nobody has: from a mailbox, in the settings file.
        rig.app.session.prefs.mailbox_hours = Some(mailcraft_engine::cmd::calendar_settings::MailboxHours {
            email: rig.app.session.mailbox.accounts.first().map(|a| a.email.clone()).unwrap(),
            days: vec![Weekday::Sun; 5000],
            start: u16::MAX,
            end: 0,
            zone: "\u{0}".repeat(100_000),
            offset: Some(i32::MIN),
        });
        every_view(&mut rig);
        rig.app.session.prefs.mailbox_hours = Some(mailcraft_engine::cmd::calendar_settings::MailboxHours {
            email: rig.app.session.mailbox.accounts.first().map(|a| a.email.clone()).unwrap(),
            days: vec![Weekday::Sun],
            start: 1439,
            end: 1440,
            offset: Some(18 * 60),
            ..Default::default()
        });
        every_view(&mut rig);
        rig.click(pos2(700.0, 500.0), 1);
        rig.key(egui::Key::Home);
        rig.key(egui::Key::End);
    }
}
