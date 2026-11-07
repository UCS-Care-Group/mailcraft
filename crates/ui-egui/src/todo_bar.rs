// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The To-Do Bar (#831): a narrow pane down the right of the window that keeps the calendar, the
//! favourite contacts and the tasks in view whatever module is in front. View › To-Do Bar turns
//! each of its three parts on or off (`view.todoBar`), module by module; they stand one under the
//! other in Outlook's order: the date navigator and the next appointments, the people, the tasks.
//! A title typed in its box is a new task, a double click opens what it lists, and a message
//! dragged out of the list and let go over its tasks becomes a task.

use std::sync::Arc;

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::TaskFilter;
use mailcraft_engine::cmd::calendar::occurrences;
use mailcraft_engine::cmd::tasks;
use mailcraft_model::{Contact, Id, Occurrence};
use mailcraft_time::{Date, Weekday};
use serde_json::json;

use crate::people_view::one_line;
use crate::tasks_view::{Bucket, due_text, paint_check, paint_title, shown_title};
use crate::theme::{Tokens, c32, paint_clipped, regular, semibold};
use crate::{MailApp, icons, widgets};

/// The bar's width as a new install has it, and the least and most it can be dragged to.
pub const DEFAULT_WIDTH: f32 = 280.0;
pub const MIN_WIDTH: f32 = 200.0;
pub const MAX_WIDTH: f32 = 480.0;
/// What the bar leaves for the module beside it, on a narrow window.
const LEAVES: f32 = 520.0;

/// A part's heading.
const HEAD_H: f32 = 30.0;
/// The date navigator's month name, its weekday initials and a week of days.
const MONTH_TITLE_H: f32 = 28.0;
const WEEKDAYS_H: f32 = 18.0;
const WEEK_H: f32 = 22.0;
/// An appointment, a contact, a task, and a day's or a due date's heading over them.
const EVENT_H: f32 = 40.0;
const PERSON_H: f32 = 36.0;
const TASK_H: f32 = 32.0;
const GROUP_H: f32 = 24.0;
/// The box a new task is typed in.
const ADD_H: f32 = 38.0;

/// How far ahead the next appointments are looked for, and how many of them are listed.
const AHEAD_DAYS: i64 = 7;
const MAX_EVENTS: usize = 6;
/// The most favourite contacts and tasks listed (the Tasks module lists them all).
const MAX_PEOPLE: usize = 50;
const MAX_TASKS: usize = 200;
/// Longest title the box takes.
const TITLE_MAX: usize = 500;

/// Where the bar's own memory lives in egui's: the month the navigator shows, and what is typed.
const MONTH_ID: &str = "todo_bar_month";
const NEW_TASK_ID: &str = "todo_bar_new_task";

/// What each part lists is worked out when the mailbox, the day or what it depends on changes,
/// not at every frame: a large mailbox's events, contacts and tasks are not gone through again
/// while the owner reads mail beside the bar. Each is kept under these names with what it was
/// worked out from.
const BUSY_CACHE: &str = "todo_bar_busy";
const NEXT_CACHE: &str = "todo_bar_next";
const PEOPLE_CACHE: &str = "todo_bar_people";
const TASKS_CACHE: &str = "todo_bar_tasks";
/// Where the bar says, each frame, whether its tasks take what is dragged onto them.
const TAKES_ID: &str = "todo_bar_takes";
/// Where it says in which frame something dragged was last over its tasks.
const OVER_ID: &str = "todo_bar_over";

/// The most months the navigator stacks when the bar is tall enough (Outlook shows more than one
/// when there is room), and the gap between two.
pub(crate) const MAX_MONTHS: u8 = 3;
const MONTH_GAP: f32 = 8.0;
/// One month of the navigator: its name, the weekdays' initials and six weeks.
const MONTH_H: f32 = MONTH_TITLE_H + WEEKDAYS_H + 6.0 * WEEK_H;
/// Under the navigator, before the appointments.
const AFTER_NAV: f32 = 6.0;
/// A line of quiet text, for a part with nothing in it.
const QUIET_H: f32 = 28.0;
/// The tasks take what is left of the bar, but never less than room for two under a heading.
const TASKS_LEAST: f32 = GROUP_H + 2.0 * TASK_H;
/// Above the tasks: their heading and the box a new one is typed in.
const TASKS_HEAD: f32 = HEAD_H + ADD_H;
/// The favourites shown before their list scrolls, when the tasks are under them; and the least
/// they keep when they are the last part, which takes what is left.
pub(crate) const PEOPLE_MOST: f32 = 6.0 * PERSON_H;
const PEOPLE_LEAST: f32 = 2.0 * PERSON_H;
/// Room under the last part.
const FOOT: f32 = 8.0;

/// An appointment dragged out of the calendar (or the bar's own list of the next ones), for the
/// tasks of the To-Do Bar: let go over them, it becomes a task (`tasks.fromEvent`). The calendar
/// moves it as before wherever else it is let go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EventDrag {
    pub event: Id,
    /// The occurrence's day, for one of a series.
    pub day: Date,
}

/// Contacts dragged out of the People list (or the bar's favourites), for the tasks of the To-Do
/// Bar: each becomes a task (`tasks.fromContact`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ContactDrag(pub Vec<Id>);

/// Whether the To-Do Bar's tasks were on screen this frame, to take an appointment or a contact
/// let go over them: only then does dragging one out of its list carry it. (The bar is laid out
/// before the module, so the module's rows know.)
pub(crate) fn takes_drops(ctx: &egui::Context) -> bool {
    let frame = ctx.cumulative_frame_nr();
    ctx.data(|d| d.get_temp::<(u64, bool)>(egui::Id::unique(TAKES_ID))).is_some_and(|(at, takes)| takes && at == frame)
}

/// Whether something dragged is over the To-Do Bar's tasks this frame: the calendar then neither
/// shows where its appointment would go nor moves it there when it is let go.
pub(crate) fn over_tasks(ctx: &egui::Context) -> bool {
    let frame = ctx.cumulative_frame_nr();
    ctx.data(|d| d.get_temp::<u64>(egui::Id::unique(OVER_ID))) == Some(frame)
}

/// How the bar's height is shared out: how many months the navigator stacks and how many of the
/// next appointments are listed under it, the height the favourites' list gets (it scrolls inside
/// that when it is longer), and whether it all fits. When it doesn't, even with one month and
/// one appointment, the whole bar scrolls instead, with everything at its own height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Plan {
    pub months: u8,
    pub events: usize,
    pub people: f32,
    pub fits: bool,
}

/// Share out `height` between the parts that are on, as Outlook does: the calendar and the
/// favourites take their own height (the favourites at most [`PEOPLE_MOST`] when the tasks are
/// under them), and the last part takes what is left, the tasks never less than
/// [`TASKS_LEAST`]. `agenda[n]` is the height of the next appointments with the first `n` of
/// them listed (`agenda[0]` alone: there are none), and `people` the favourites' own height.
/// The appointments get room before a second month does.
pub(crate) fn plan(height: f32, parts: mailcraft_engine::TodoParts, agenda: &[f32], people: f32) -> Plan {
    let all = agenda.len().saturating_sub(1);
    let least = all.min(1);
    let calendar = |months: u8, events: usize| {
        if !parts.calendar {
            return 0.0;
        }
        let months = f32::from(months.max(1));
        HEAD_H + months * MONTH_H + (months - 1.0) * MONTH_GAP + AFTER_NAV + agenda.get(events).copied().unwrap_or(QUIET_H)
    };
    // The favourites at their own height, capped over the tasks; as the last part, the least.
    let people_part = match (parts.people, parts.tasks) {
        (false, _) => 0.0,
        (true, true) => HEAD_H + people.min(PEOPLE_MOST),
        (true, false) => HEAD_H + people.min(PEOPLE_LEAST),
    };
    let tasks_part = if parts.tasks { TASKS_HEAD + TASKS_LEAST } else { 0.0 };
    let need = |months: u8, events: usize| calendar(months, events) + people_part + tasks_part + FOOT;
    let height = if height.is_finite() { height } else { 0.0 };
    if need(1, least) > height {
        return Plan { months: 1, events: all, people, fits: false };
    }
    let events = (least..=all).rev().find(|e| need(1, *e) <= height).unwrap_or(least);
    let months = (1..=MAX_MONTHS).rev().find(|m| need(*m, events) <= height).unwrap_or(1);
    let people = match (parts.people, parts.tasks) {
        (false, _) => 0.0,
        (true, true) => people.min(PEOPLE_MOST),
        // The last part: what is left.
        (true, false) => (height - calendar(months, events) - HEAD_H - FOOT).max(people.min(PEOPLE_LEAST)),
    };
    Plan { months, events, people, fits: true }
}

/// The height of the next appointments with the first `n` of `list` listed, for each `n`: a
/// day's heading over each day's first, and a quiet line for none.
pub(crate) fn agenda_heights(list: &[Occurrence], today: Date) -> Vec<f32> {
    let mut out = vec![if list.is_empty() { QUIET_H } else { 0.0 }];
    let mut day_shown: Option<Date> = None;
    let mut h = 0.0;
    for o in list {
        let day = o.start.date.max(today);
        if day_shown != Some(day) {
            day_shown = Some(day);
            h += GROUP_H;
        }
        h += EVENT_H;
        out.push(h);
    }
    out
}

/// The favourites' own height: a row each, or the quiet line for none.
fn people_height(count: usize) -> f32 {
    if count == 0 { QUIET_H } else { count as f32 * PERSON_H }
}

/// `make()` as kept under `slot` for `key`, worked out again only when `key` is another.
fn cached<K, V>(ctx: &egui::Context, slot: &'static str, key: K, make: impl FnOnce() -> V) -> Arc<V>
where
    K: Clone + PartialEq + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    let id = egui::Id::unique(slot);
    if let Some((_, kept)) = ctx.data(|d| d.get_temp::<(K, Arc<V>)>(id)).filter(|(at, _)| *at == key) {
        return kept;
    }
    let made = Arc::new(make());
    ctx.data_mut(|d| d.insert_temp(id, (key, made.clone())));
    made
}

/// The days of the six weeks from `first` on which something happens, as the navigator marks
/// them.
pub(crate) fn busy_days(s: &mailcraft_engine::Session, first: Date) -> [bool; 42] {
    let last = first.add_days(41);
    let mut busy = [false; 42];
    for o in occurrences(s, first, last).into_iter().take(2000) {
        let lo = first.days_until(o.start.date).clamp(0, 41);
        let hi = first.days_until(crate::calendar_view::last_day(o.start, o.end)).clamp(-1, 41);
        for day in lo..=hi {
            if let Some(slot) = usize::try_from(day).ok().and_then(|i| busy.get_mut(i)) {
                *slot = true;
            }
        }
    }
    busy
}

/// [`busy_days`], kept until the mailbox changes.
#[cfg(test)]
pub(crate) fn busy_days_cached(ctx: &egui::Context, s: &mailcraft_engine::Session, first: Date) -> Arc<[bool; 42]> {
    busy_month_cached(ctx, s, first, 0)
}

/// [`busy_days`] for the `k`th month the navigator stacks, each kept in a place of its own so
/// that two or three months don't work each other's out again at every frame.
fn busy_month_cached(ctx: &egui::Context, s: &mailcraft_engine::Session, first: Date, k: u8) -> Arc<[bool; 42]> {
    let slot = match k {
        0 => BUSY_CACHE,
        1 => "todo_bar_busy_1",
        _ => "todo_bar_busy_2",
    };
    cached(ctx, slot, (s.rev(), s.mailbox.events.len(), first), || busy_days(s, first))
}

/// [`next_appointments`], kept until the mailbox changes or the clock reaches another minute (an
/// appointment that is over leaves the list).
pub(crate) fn next_appointments_cached(ctx: &egui::Context, s: &mailcraft_engine::Session) -> Arc<Vec<Occurrence>> {
    let mut now = s.local(s.now());
    now.second = 0;
    cached(ctx, NEXT_CACHE, (s.rev(), s.mailbox.events.len(), now), || next_appointments(s))
}

/// [`favourites`], kept until the mailbox changes.
pub(crate) fn favourites_cached(ctx: &egui::Context, s: &mailcraft_engine::Session) -> Arc<Vec<Id>> {
    cached(ctx, PEOPLE_CACHE, (s.rev(), s.mailbox.contacts.len()), || favourites(&s.mailbox))
}

/// [`task_rows`], kept until the mailbox, the day, the first day of the week or the account the
/// Tasks module shows changes.
pub(crate) fn task_rows_cached(ctx: &egui::Context, s: &mailcraft_engine::Session) -> Arc<Vec<(Bucket, Id)>> {
    let key = (s.rev(), s.mailbox.tasks.len(), s.today(), s.prefs.week_start, tasks::shown_account(s));
    cached(ctx, TASKS_CACHE, key, || task_rows(s))
}

/// A width read back from the settings, made one the bar can have.
pub fn sane_width(width: f32) -> f32 {
    if width.is_finite() { width.clamp(MIN_WIDTH, MAX_WIDTH) } else { DEFAULT_WIDTH }
}

/// The most the bar may take of a window `available` wide: never so much that the module beside
/// it has less than [`LEAVES`], and never less than [`MIN_WIDTH`].
pub(crate) fn widest(available: f32) -> f32 {
    if available.is_finite() { (available - LEAVES).clamp(MIN_WIDTH, MAX_WIDTH) } else { MIN_WIDTH }
}

/// The bar, at the right of the window, when the module in front has one part of it on at least.
/// Laid out before the module itself, which takes what is left.
pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let parts = app.session.view.todo_bar.of(app.session.view.module);
    let frame = ui.ctx().cumulative_frame_nr();
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(TAKES_ID), (frame, parts.tasks)));
    if !parts.any() {
        return;
    }
    drag_ghost(ui);
    let t = Tokens::get(ui.ctx());
    let max = widest(ui.available_width());
    let shown = egui::Panel::right("todo_bar")
        .default_size(sane_width(app.ui.todo_bar_width).min(max))
        .size_range(MIN_WIDTH..=max)
        .resizable(true)
        .show_separator_line(t.pane_rule)
        .frame(egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)))
        .show(ui, |ui| {
            app.hit.add("todoBar", 0, ui.max_rect());
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            // The calendar and the favourites at their own height, the tasks under them taking
            // what is left and scrolling inside it, as in Outlook; the whole bar scrolls only
            // when even that leaves the tasks too little.
            let today = app.session.today();
            let agenda = if parts.calendar { agenda_heights(&next_appointments_cached(ui.ctx(), &app.session), today) } else { Vec::new() };
            let people = if parts.people { people_height(favourites_cached(ui.ctx(), &app.session).len()) } else { 0.0 };
            let plan = plan(ui.available_height(), parts, &agenda, people);
            if plan.fits {
                bar_parts(app, ui, parts, plan);
            } else {
                egui::ScrollArea::vertical().id_salt("todo_bar_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    bar_parts(app, ui, parts, plan);
                    ui.add_space(FOOT);
                });
            }
        });
    app.ui.todo_bar_width = sane_width(shown.response.rect.width());
}

/// The parts that are on, one under the other, as `plan` shares the height out.
fn bar_parts(app: &mut MailApp, ui: &mut Ui, parts: mailcraft_engine::TodoParts, plan: Plan) {
    if parts.calendar {
        calendar_part(app, ui, plan);
    }
    if parts.people {
        people_part(app, ui, plan.fits.then_some(plan.people));
    }
    if parts.tasks {
        tasks_part(app, ui, plan.fits);
    }
}

/// While contacts are dragged: a small label by the pointer saying how many, as the message list
/// has for messages. (A dragged appointment has the calendar's own ghost.)
fn drag_ghost(ui: &Ui) {
    let Some(drag) = egui::DragAndDrop::payload::<ContactDrag>(ui.ctx()) else { return };
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let text = tln!(drag.0.len(), "{n} contact", "{n} contacts");
    let t = Tokens::get(ui.ctx());
    let area = egui::Area::new(egui::Id::unique("todo_drag_ghost")).order(egui::Order::Tooltip).fixed_pos(pos + vec2(14.0, 16.0)).interactable(false);
    area.show(ui.ctx(), |ui| {
        let frame =
            egui::Frame::NONE.fill(t.menu).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(4.0).inner_margin(egui::Margin::symmetric(8, 4));
        frame.show(ui, |ui| {
            ui.label(egui::RichText::new(text).font(regular(12.0)).color(t.text));
        });
    });
}

/// A part's heading: its name, and a button that takes the part off the bar.
fn heading(app: &mut MailApp, ui: &mut Ui, title: &str, part: &'static str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEAD_H), Sense::hover());
    ui.painter().line_segment([pos2(r.min.x, r.min.y + 0.5), pos2(r.max.x, r.min.y + 0.5)], Stroke::new(1.0, t.border));
    paint_clipped(ui.painter(), pos2(r.min.x + 12.0, r.center().y), (r.width() - 48.0).max(0.0), title, semibold(13.0), t.text);
    let close = Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(22.0, 22.0));
    let resp = ui.interact(close, ui.scope_id().with(("todo_close", part)), Sense::click());
    app.hit.add(close_kind(part), 0, close);
    if resp.hovered() {
        ui.painter().rect_filled(close, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    icons::paint(ui.painter(), Rect::from_center_size(close.center(), vec2(10.0, 10.0)), "close", t.icon, t.accent);
    let tip = crate::i18n::fmt(tl!("Remove {part} from the To-Do Bar"), &[("part", title)]);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip.clone()));
    if resp.on_hover_text(tip.clone()).clicked() {
        let _ = app.run("view.todoBar", json!({"part": part, "value": false}));
    }
}

/// What the hit map calls a part's close button.
fn close_kind(part: &str) -> &'static str {
    match part {
        "calendar" => "todoCloseCalendar",
        "people" => "todoClosePeople",
        _ => "todoCloseTasks",
    }
}

/// A line of quiet text, for a part with nothing in it.
fn quiet(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    paint_clipped(ui.painter(), pos2(r.min.x + 14.0, r.center().y), (r.width() - 28.0).max(0.0), text, regular(12.0), t.text_dim);
}

/// A small heading over a group of rows (a day, a due date).
fn group(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), GROUP_H), Sense::hover());
    paint_clipped(ui.painter(), pos2(r.min.x + 12.0, r.center().y + 2.0), (r.width() - 24.0).max(0.0), text, semibold(11.5), t.text_dim);
}

// ---- calendar ----

fn calendar_part(app: &mut MailApp, ui: &mut Ui, plan: Plan) {
    heading(app, ui, tl!("Calendar"), "calendar");
    navigator(app, ui, plan.months);
    ui.add_space(AFTER_NAV);
    upcoming(app, ui, plan.events);
}

/// The month to show first, as the arrows left it; this month until they are used.
fn shown_month(ctx: &egui::Context, today: Date) -> Date {
    ctx.data(|d| d.get_temp::<Date>(egui::Id::unique(MONTH_ID))).unwrap_or(today).start_of_month()
}

/// The date navigator: `months` months one under the other (Outlook stacks more when the bar is
/// tall), Previous Month over the first and Next Month over the last. Today is marked, a day on
/// which something happens is bold, and a click on a day shows it in the calendar. The days of
/// the month before the first and after the last fill their weeks, dimmed; between two months a
/// day is drawn once, in its own month.
fn navigator(app: &mut MailApp, ui: &mut Ui, months: u8) {
    let t = Tokens::get(ui.ctx());
    let today = app.session.today();
    let week_start = app.session.prefs.week_start;
    let months = months.clamp(1, MAX_MONTHS);
    let shown = shown_month(ui.ctx(), today);
    let width = ui.available_width();
    let pad = 12.0;
    let cell_w = ((width - 2.0 * pad) / 7.0).max(1.0);
    let x0 = ui.cursor().min.x + pad;

    let mut step = 0;
    for k in 0..months {
        let month = shown.add_months(i32::from(k));
        if k > 0 {
            ui.add_space(MONTH_GAP);
        }
        // The month's name, and the arrows: back over the first month, on over the last.
        let (r, _) = ui.allocate_exact_size(vec2(width, MONTH_TITLE_H), Sense::hover());
        app.hit.add("todoMonth", u64::from(k), r);
        for (slot, (icon, tip, by)) in [("next", "Next Month", 1), ("previous", "Previous Month", -1)].into_iter().enumerate() {
            if (by > 0 && k + 1 != months) || (by < 0 && k != 0) {
                continue;
            }
            let b = Rect::from_center_size(pos2(r.max.x - pad - 10.0 - slot as f32 * 24.0, r.center().y), vec2(22.0, 22.0));
            let resp = ui.interact(b, ui.scope_id().with(("todo_month_step", by)), Sense::click());
            app.hit.add("todoMonthStep", u64::from(by > 0), b);
            if resp.hovered() {
                ui.painter().rect_filled(b, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
            }
            icons::paint(ui.painter(), Rect::from_center_size(b.center(), vec2(11.0, 11.0)), icon, t.icon, t.accent);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!(tip)));
            if resp.on_hover_text(tl!(tip)).clicked() {
                step = by;
            }
        }
        paint_clipped(
            ui.painter(),
            pos2(x0 + 2.0, r.center().y),
            (width - 2.0 * pad - 56.0).max(0.0),
            &crate::dates::month_year(month),
            semibold(12.5),
            t.text,
        );

        // The weekdays' initials.
        let (r, _) = ui.allocate_exact_size(vec2(width, WEEKDAYS_H), Sense::hover());
        for i in 0..7u8 {
            let weekday = Weekday::from_index(i64::from(week_start.index()) + i64::from(i));
            let initial: String = tl!(weekday.short()).chars().take(1).collect();
            ui.painter().text(pos2(x0 + (f32::from(i) + 0.5) * cell_w, r.center().y), Align2::CENTER_CENTER, initial, regular(10.5), t.text_dim);
        }

        // Six weeks from the start of the week the month begins in; the days on which something
        // happens among them (each month's kept apart).
        let first = month.start_of_week(week_start);
        let busy = busy_month_cached(ui.ctx(), &app.session, first, k);
        let (grid, _) = ui.allocate_exact_size(vec2(width, 6.0 * WEEK_H), Sense::hover());
        for i in 0..42i64 {
            let day = first.add_days(i);
            let before = day < month;
            let after = (day.year, day.month) > (month.year, month.month);
            // A day of the month before or after shows only at the ends of the navigator.
            if (before && k > 0) || (after && k + 1 < months) {
                continue;
            }
            let (row, col) = (i / 7, i % 7);
            let cell = Rect::from_min_size(pos2(x0 + col as f32 * cell_w, grid.min.y + row as f32 * WEEK_H), vec2(cell_w, WEEK_H));
            let resp = ui.interact(cell, ui.scope_id().with(("todo_day", k, i)), Sense::click());
            if let Ok(id) = u64::try_from(day.to_days()) {
                app.hit.add("todoDay", id, cell);
            }
            let radius = (WEEK_H / 2.0 - 1.0).min(cell_w / 2.0);
            let is_today = day == today;
            if is_today {
                ui.painter().circle_filled(cell.center(), radius, t.accent);
            } else if resp.hovered() {
                ui.painter().circle_filled(cell.center(), radius, t.hover);
            }
            let ink = if is_today {
                t.on_accent
            } else if before || after {
                t.text_disabled
            } else {
                t.text
            };
            let bold = busy.get(usize::try_from(i).unwrap_or(0)).copied().unwrap_or(false);
            let font = if is_today || bold { semibold(11.0) } else { regular(11.0) };
            ui.painter().text(cell.center(), Align2::CENTER_CENTER, day.day.to_string(), font, ink);
            let name = crate::dates::long(day);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
            if resp.clicked() {
                let _ = app.run("view.goToDate", json!({"date": day.iso()}));
            }
        }
    }
    if step != 0 {
        let to = shown.add_months(step);
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(MONTH_ID), to));
    }
}

/// The next appointments: those not over yet, from now to a week on, by the day they are on.
pub(crate) fn next_appointments(s: &mailcraft_engine::Session) -> Vec<Occurrence> {
    let now = s.local(s.now());
    let today = s.today();
    let mut list: Vec<Occurrence> = occurrences(s, today, today.add_days(AHEAD_DAYS - 1)).into_iter().filter(|o| o.end > now).collect();
    list.sort_by(|a, b| a.start.cmp(&b.start).then(a.event.cmp(&b.event)));
    list.truncate(MAX_EVENTS);
    list
}

/// The first `count` of the next appointments (as many as the bar has room for).
fn upcoming(app: &mut MailApp, ui: &mut Ui, count: usize) {
    let t = Tokens::get(ui.ctx());
    let list = next_appointments_cached(ui.ctx(), &app.session);
    if list.is_empty() {
        quiet(ui, tl!("No upcoming appointments"));
        return;
    }
    let today = app.session.today();
    let twelve = app.session.prefs.twelve_hour;
    let mut day_shown: Option<Date> = None;
    let takes = takes_drops(ui.ctx());
    for o in list.iter().take(count.max(1)).copied() {
        // The shared pointer, not a copy of the event.
        let Some(ev) = app.session.mailbox.events.get(&o.event).cloned() else { continue };
        let day = o.start.date.max(today);
        if day_shown != Some(day) {
            day_shown = Some(day);
            let label = match today.days_until(day) {
                0 => tl!("Today").to_string(),
                1 => tl!("Tomorrow").to_string(),
                _ => crate::dates::weekday_month_day(day),
            };
            group(ui, &label);
        }
        // Dragged down onto the tasks, it becomes one.
        let sense = if takes { Sense::click_and_drag() } else { Sense::click() };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), EVENT_H), sense);
        app.hit.add("todoEvent", ev.id, rect);
        if takes {
            resp.dnd_set_drag_payload(EventDrag { event: ev.id, day: o.day });
        }
        let selected = app.session.sel.event == Some(ev.id);
        if selected {
            ui.painter().rect_filled(rect, 0.0, t.selection);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 0.0, t.hover_subtle);
        }
        let colour = app.session.mailbox.calendar(ev.calendar).map_or(t.accent, |c| c32(c.shown_color()));
        let bar = Rect::from_min_size(pos2(rect.min.x + 12.0, rect.min.y + 6.0), vec2(4.0, EVENT_H - 12.0));
        ui.painter().rect_filled(bar, 2.0, colour);
        let left = rect.min.x + 24.0;
        let room = (rect.max.x - left - 10.0).max(0.0);
        let title = one_line(&ev.title, 200);
        let title = if title.is_empty() { tl!("(no title)").to_string() } else { title };
        paint_clipped(ui.painter(), pos2(left, rect.min.y + 13.0), room, &title, regular(12.5), t.text);
        let when = if ev.all_day {
            tl!("All day").to_string()
        } else {
            format!("{} – {}", crate::dates::clock(o.start, twelve), crate::dates::clock(o.end, twelve))
        };
        let place = one_line(&ev.location, 200);
        let second = if place.is_empty() { when } else { format!("{when} · {place}") };
        paint_clipped(ui.painter(), pos2(left, rect.min.y + 29.0), room, &second, regular(11.0), t.text_dim);
        let said = format!("{title}, {second}");
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, said.clone()));
        let at = o.day.iso();
        if resp.double_clicked() {
            // As in the calendar: a window of its own, or the form where there are none.
            let _ = app.execute("calendar.select", json!({"id": ev.id, "day": at}));
            if app.run_or_else("ui.openWindow", json!({"event": ev.id})).is_err() {
                let _ = app.run("calendar.edit", json!({}));
            }
        } else if resp.clicked() {
            let _ = app.run("calendar.select", json!({"id": ev.id, "day": at}));
        }
    }
}

// ---- people ----

/// The favourite contacts, by the name they are filed as.
pub(crate) fn favourites(mailbox: &mailcraft_model::Mailbox) -> Vec<Id> {
    let mut list: Vec<&Contact> = mailbox.contacts.values().map(|c| c.as_ref()).filter(|c| c.favorite).collect();
    list.sort_by_cached_key(|c| (mailcraft_model::words::fold(&c.file_as()), c.id));
    list.into_iter().take(MAX_PEOPLE).map(|c| c.id).collect()
}

/// The favourites, in `room` when it is given: a longer list scrolls inside it.
fn people_part(app: &mut MailApp, ui: &mut Ui, room: Option<f32>) {
    heading(app, ui, tl!("People"), "people");
    let list = favourites_cached(ui.ctx(), &app.session);
    if list.is_empty() {
        quiet(ui, tl!("Add contacts to Favorites to keep them here"));
        return;
    }
    match room {
        Some(room) if people_height(list.len()) > room + 0.5 => {
            let top = ui.cursor().min.y;
            egui::ScrollArea::vertical().id_salt("todo_bar_people").max_height(room).auto_shrink([false, true]).show(ui, |ui| {
                people_rows(app, ui, &list);
            });
            // (The scroll area takes the room it was given, whatever egui's spacing says.)
            let used = ui.cursor().min.y - top;
            if used < room {
                ui.add_space(room - used);
            }
        }
        Some(room) => {
            let top = ui.cursor().min.y;
            people_rows(app, ui, &list);
            // As the last part, the favourites take what is left of the bar.
            let used = ui.cursor().min.y - top;
            if used < room {
                ui.add_space(room - used);
            }
        }
        None => people_rows(app, ui, &list),
    }
}

fn people_rows(app: &mut MailApp, ui: &mut Ui, list: &[Id]) {
    let t = Tokens::get(ui.ctx());
    let takes = takes_drops(ui.ctx());
    for id in list.iter().copied() {
        let Some(c) = app.session.mailbox.contacts.get(&id).cloned() else { continue };
        // Dragged down onto the tasks, a favourite becomes one.
        let sense = if takes { Sense::click_and_drag() } else { Sense::click() };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), PERSON_H), sense);
        app.hit.add("todoContact", id, rect);
        if takes {
            resp.dnd_set_drag_payload(ContactDrag(vec![id]));
        }
        if app.session.sel.contact == Some(id) {
            ui.painter().rect_filled(rect, 0.0, t.selection);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 0.0, t.hover_subtle);
        }
        widgets::avatar(ui, pos2(rect.min.x + 26.0, rect.center().y), 13.0, &crate::people_view::avatar_address(&c));
        let left = rect.min.x + 48.0;
        let name = crate::people_view::shown_name(&c);
        paint_clipped(ui.painter(), pos2(left, rect.center().y), (rect.max.x - left - 10.0).max(0.0), &name, regular(12.5), t.text);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
        if resp.double_clicked() {
            let _ = app.execute("people.select", json!({"id": id}));
            let _ = app.run("people.edit", json!({}));
        } else if resp.clicked() {
            let _ = app.run("people.select", json!({"id": id}));
        }
    }
}

// ---- tasks ----

/// The open tasks, arranged by due date: each with the heading it is under (Overdue, Today…), in
/// the order the Tasks module lists them.
pub(crate) fn task_rows(s: &mailcraft_engine::Session) -> Vec<(Bucket, Id)> {
    let today = s.today();
    let week_start = s.prefs.week_start;
    let mut rows: Vec<(Bucket, Id)> = tasks::sorted(s, TaskFilter::Active)
        .into_iter()
        .filter_map(|id| s.mailbox.task(id).map(|task| (Bucket::of(task.due, task.is_complete(), today, week_start), id)))
        .take(MAX_TASKS)
        .collect();
    // (Sorted by due date already; the stable sort keeps that order under each heading.)
    rows.sort_by_key(|(bucket, _)| *bucket);
    rows
}

/// The tasks: their heading and the box a new one is typed in, then the tasks themselves. With
/// `fill` they take the rest of the bar and scroll inside it (Outlook's way); else they are at
/// their own height in a bar that scrolls as a whole.
fn tasks_part(app: &mut MailApp, ui: &mut Ui, fill: bool) {
    heading(app, ui, tl!("Tasks"), "tasks");
    let top = ui.cursor().min.y;
    new_task(app, ui);
    if fill {
        let bottom = ui.max_rect().max.y;
        egui::ScrollArea::vertical().id_salt("todo_bar_tasks").auto_shrink([false, false]).max_height((bottom - ui.cursor().min.y).max(0.0)).show(
            ui,
            |ui| {
                task_list(app, ui);
                ui.add_space(FOOT);
            },
        );
        // Everything from the box down to the foot of the bar takes what is let go over it.
        let area = Rect::from_min_max(pos2(ui.max_rect().min.x, top), pos2(ui.max_rect().max.x, bottom));
        drop_target(app, ui, area);
        return;
    }
    task_list(app, ui);
    // A message let go over the tasks becomes one: room under them to aim at, too.
    ui.add_space(24.0);
    let area = Rect::from_min_max(pos2(ui.max_rect().min.x, top), pos2(ui.max_rect().max.x, ui.cursor().min.y));
    drop_target(app, ui, area);
}

/// The open tasks under their due dates' headings.
fn task_list(app: &mut MailApp, ui: &mut Ui) {
    let rows = task_rows_cached(ui.ctx(), &app.session);
    if rows.is_empty() {
        quiet(ui, tl!("Nothing to do here."));
    }
    let today = app.session.today();
    let mut bucket_shown = None;
    for (bucket, id) in rows.iter().copied() {
        if bucket_shown != Some(bucket) {
            bucket_shown = Some(bucket);
            group(ui, tl!(bucket.label()));
        }
        task_row(app, ui, id, today);
    }
}

/// The box a task is typed in: Enter makes it (`tasks.new`) and leaves the box ready for the next.
fn new_task(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ADD_H), Sense::hover());
    let field = Rect::from_min_max(pos2(r.min.x + 10.0, r.min.y + 4.0), pos2(r.max.x - 10.0, r.max.y - 6.0));
    app.hit.add("todoNewTask", 0, field);
    let id = egui::Id::unique(NEW_TASK_ID);
    let focused = ui.memory(|m| m.has_focus(id));
    ui.painter().rect(
        field,
        4.0,
        if focused { t.input } else { t.surface },
        Stroke::new(1.0, if focused { t.accent } else { t.border }),
        egui::StrokeKind::Inside,
    );
    let edit_rect = Rect::from_min_max(pos2(field.min.x + 8.0, field.min.y + 1.0), pos2(field.max.x - 6.0, field.max.y - 1.0));
    if edit_rect.width() < 24.0 || edit_rect.height() < 12.0 {
        return;
    }
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default();
    let edit = egui::TextEdit::singleline(&mut text)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(tl!("Type a new task"))
        .font(regular(12.5))
        .char_limit(TITLE_MAX)
        .desired_width(edit_rect.width())
        .min_size(vec2(0.0, edit_rect.height()))
        .align(Align2::LEFT_CENTER);
    let resp = crate::text_menu::field(ui.new_child(egui::UiBuilder::new().max_rect(edit_rect)).add(edit));
    let (enter, escape) = (crate::keys::enter(ui.ctx(), ui.layer_id()), crate::keys::escape(ui.ctx(), ui.layer_id()));
    if resp.lost_focus() && enter {
        let title = text.trim().to_string();
        if title.is_empty() || app.run("tasks.new", json!({"title": title})).is_ok() {
            text.clear();
        }
        resp.request_focus();
    } else if resp.lost_focus() && escape {
        text.clear();
    }
    ui.data_mut(|d| d.insert_temp(id, text));
}

fn task_row(app: &mut MailApp, ui: &mut Ui, id: Id, today: Date) {
    let t = Tokens::get(ui.ctx());
    let Some(task) = app.session.mailbox.tasks.get(&id).cloned() else { return };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), TASK_H), Sense::click());
    app.hit.add("todoTask", id, rect);
    let check = Rect::from_center_size(pos2(rect.min.x + 22.0, rect.center().y), vec2(26.0, TASK_H.min(26.0)));
    let check_resp = ui.interact(check, ui.scope_id().with(("todo_task_check", id)), Sense::click());
    app.hit.add("todoTaskCheck", id, check);
    let p = ui.painter();
    if app.session.sel.task == Some(id) {
        p.rect_filled(rect, 0.0, t.selection);
    } else if resp.hovered() || check_resp.hovered() {
        p.rect_filled(rect, 0.0, t.hover_subtle);
    }
    let done = task.is_complete();
    paint_check(p, check.center(), done, check_resp.hovered(), &t);
    let mut right = rect.max.x - 10.0;
    let due = due_text(&task, today);
    if !due.is_empty() {
        let color = if task.is_overdue(today) { t.red } else { t.text_dim };
        right -= p.text(pos2(right, rect.center().y), Align2::RIGHT_CENTER, &due, regular(11.0), color).width() + 8.0;
    }
    if task.message.is_some() {
        icons::paint(p, Rect::from_center_size(pos2(right - 6.0, rect.center().y), vec2(11.0, 11.0)), "mail", t.text_dim, t.accent);
        right -= 16.0;
    }
    let left = rect.min.x + 40.0;
    let title = shown_title(&task);
    paint_title(p, pos2(left, rect.center().y), (right - left).max(0.0), &title, regular(12.5), if done { t.text_dim } else { t.text }, done);
    let said = if due.is_empty() { title.clone() } else { format!("{title}, {due}") };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, said.clone()));
    let tick = if done { tl!("Mark Incomplete") } else { tl!("Mark Complete") };
    check_resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, done, tick));
    if check_resp.on_hover_text(tick).clicked() {
        let _ = app.run("tasks.complete", json!({"id": id}));
    } else if resp.double_clicked() {
        let _ = app.execute("tasks.select", json!({"id": id}));
        let _ = app.run("tasks.edit", json!({}));
    } else if resp.clicked() {
        let _ = app.run("tasks.select", json!({"id": id}));
    }
}

/// Messages dragged out of the list, appointments out of the calendar and contacts out of People
/// (or the bar's own lists), let go over the tasks: each becomes a task, the module in front
/// staying there (`tasks.fromMessage`, `tasks.fromEvent`, `tasks.fromContact` with `show`
/// false). While they are over it, the tasks are outlined and the pointer says they are taken.
fn drop_target(app: &mut MailApp, ui: &mut Ui, area: Rect) {
    let t = Tokens::get(ui.ctx());
    let _ = ui.interact(area, ui.scope_id().with("todo_task_drop"), Sense::hover());
    app.hit.add("todoTaskDrop", 0, area);
    // (Under the pointer by its place, not by the widget egui finds there: the row being dragged,
    // or the tasks' list, may be what it finds.)
    let ctx = ui.ctx().clone();
    if !ui.rect_contains_pointer(area) {
        return;
    }
    let held = egui::DragAndDrop::has_payload_of_type::<crate::mail_view::MessageDrag>(&ctx)
        || egui::DragAndDrop::has_payload_of_type::<EventDrag>(&ctx)
        || egui::DragAndDrop::has_payload_of_type::<ContactDrag>(&ctx);
    if !held {
        return;
    }
    crate::drag_pointer::note_folder(&ctx, true);
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(OVER_ID), frame));
    ui.painter().rect_stroke(area.shrink(1.0), 4.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    if !ctx.input(|i| i.pointer.any_released()) {
        return;
    }
    // (Taking a payload of another type than it holds would drop it: each is asked for by its own.)
    if let Some(drag) = egui::DragAndDrop::payload::<crate::mail_view::MessageDrag>(&ctx) {
        egui::DragAndDrop::clear_payload(&ctx);
        for message in drag.0.iter().take(MAX_TASKS) {
            let _ = app.run("tasks.fromMessage", json!({"id": message, "show": false}));
        }
    } else if let Some(drag) = egui::DragAndDrop::payload::<EventDrag>(&ctx) {
        egui::DragAndDrop::clear_payload(&ctx);
        let _ = app.run("tasks.fromEvent", json!({"id": drag.event, "day": drag.day.iso(), "show": false}));
    } else if let Some(drag) = egui::DragAndDrop::payload::<ContactDrag>(&ctx) {
        egui::DragAndDrop::clear_payload(&ctx);
        for contact in drag.0.iter().take(MAX_TASKS) {
            let _ = app.run("tasks.fromContact", json!({"id": contact, "show": false}));
        }
    }
}

#[cfg(test)]
#[path = "todo_bar_more_tests.rs"]
mod todo_bar_more_tests;
#[cfg(test)]
#[path = "todo_bar_tests.rs"]
mod todo_bar_tests;
