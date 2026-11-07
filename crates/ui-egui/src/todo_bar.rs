// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The To-Do Bar (#831): a narrow pane down the right of the window that keeps the calendar, the
//! favourite contacts and the tasks in view whatever module is in front. View › To-Do Bar turns
//! each of its three parts on or off (`view.todoBar`), module by module; they stand one under the
//! other in Outlook's order: the date navigator and the next appointments, the people, the tasks.
//! A title typed in its box is a new task, a double click opens what it lists, and a message
//! dragged out of the list and let go over its tasks becomes a task.

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
    if !parts.any() {
        return;
    }
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
            egui::ScrollArea::vertical().id_salt("todo_bar_scroll").auto_shrink([false, false]).show(ui, |ui| {
                if parts.calendar {
                    calendar_part(app, ui);
                }
                if parts.people {
                    people_part(app, ui);
                }
                if parts.tasks {
                    tasks_part(app, ui);
                }
                ui.add_space(8.0);
            });
        });
    app.ui.todo_bar_width = sane_width(shown.response.rect.width());
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

fn calendar_part(app: &mut MailApp, ui: &mut Ui) {
    heading(app, ui, tl!("Calendar"), "calendar");
    navigator(app, ui);
    ui.add_space(6.0);
    upcoming(app, ui);
}

/// The month to show first, as the arrows left it; this month until they are used.
fn shown_month(ctx: &egui::Context, today: Date) -> Date {
    ctx.data(|d| d.get_temp::<Date>(egui::Id::unique(MONTH_ID))).unwrap_or(today).start_of_month()
}

/// The date navigator: one month, the arrows over it going a month back and on. Today is marked,
/// a day on which something happens is bold, and a click on a day shows it in the calendar.
fn navigator(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let today = app.session.today();
    let week_start = app.session.prefs.week_start;
    let month = shown_month(ui.ctx(), today);
    let width = ui.available_width();
    let pad = 12.0;
    let cell_w = ((width - 2.0 * pad) / 7.0).max(1.0);
    let x0 = ui.cursor().min.x + pad;

    // The month's name, and the arrows.
    let (r, _) = ui.allocate_exact_size(vec2(width, MONTH_TITLE_H), Sense::hover());
    let mut step = 0;
    for (i, (icon, tip, by)) in [("next", "Next Month", 1), ("previous", "Previous Month", -1)].into_iter().enumerate() {
        let b = Rect::from_center_size(pos2(r.max.x - pad - 10.0 - i as f32 * 24.0, r.center().y), vec2(22.0, 22.0));
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
    // happens among them.
    let first = month.start_of_week(week_start);
    let last = first.add_days(41);
    let mut busy = [false; 42];
    for o in occurrences(&app.session, first, last).into_iter().take(2000) {
        let lo = first.days_until(o.start.date).clamp(0, 41);
        let hi = first.days_until(crate::calendar_view::last_day(o.start, o.end)).clamp(-1, 41);
        for day in lo..=hi {
            if let Some(slot) = usize::try_from(day).ok().and_then(|i| busy.get_mut(i)) {
                *slot = true;
            }
        }
    }
    let (grid, _) = ui.allocate_exact_size(vec2(width, 6.0 * WEEK_H), Sense::hover());
    for i in 0..42i64 {
        let day = first.add_days(i);
        let (row, col) = (i / 7, i % 7);
        let cell = Rect::from_min_size(pos2(x0 + col as f32 * cell_w, grid.min.y + row as f32 * WEEK_H), vec2(cell_w, WEEK_H));
        let resp = ui.interact(cell, ui.scope_id().with(("todo_day", i)), Sense::click());
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
        let outside = day.month != month.month || day.year != month.year;
        let ink = if is_today {
            t.on_accent
        } else if outside {
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
    if step != 0 {
        let to = month.add_months(step);
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

fn upcoming(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let list = next_appointments(&app.session);
    if list.is_empty() {
        quiet(ui, tl!("No upcoming appointments"));
        return;
    }
    let today = app.session.today();
    let twelve = app.session.prefs.twelve_hour;
    let mut day_shown: Option<Date> = None;
    for o in list {
        let Some(ev) = app.session.mailbox.event(o.event).cloned() else { continue };
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
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), EVENT_H), Sense::click());
        app.hit.add("todoEvent", ev.id, rect);
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

fn people_part(app: &mut MailApp, ui: &mut Ui) {
    heading(app, ui, tl!("People"), "people");
    let t = Tokens::get(ui.ctx());
    let list = favourites(&app.session.mailbox);
    if list.is_empty() {
        quiet(ui, tl!("Add contacts to Favorites to keep them here"));
        return;
    }
    for id in list {
        let Some(c) = app.session.mailbox.contacts.get(&id).cloned() else { continue };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), PERSON_H), Sense::click());
        app.hit.add("todoContact", id, rect);
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

fn tasks_part(app: &mut MailApp, ui: &mut Ui) {
    heading(app, ui, tl!("Tasks"), "tasks");
    let top = ui.cursor().min.y;
    new_task(app, ui);
    let rows = task_rows(&app.session);
    if rows.is_empty() {
        quiet(ui, tl!("Nothing to do here."));
    }
    let today = app.session.today();
    let mut bucket_shown = None;
    for (bucket, id) in rows {
        if bucket_shown != Some(bucket) {
            bucket_shown = Some(bucket);
            group(ui, tl!(bucket.label()));
        }
        task_row(app, ui, id, today);
    }
    // A message let go over the tasks becomes one: room under them to aim at, too.
    ui.add_space(24.0);
    let area = Rect::from_min_max(pos2(ui.max_rect().min.x, top), pos2(ui.max_rect().max.x, ui.cursor().min.y));
    drop_target(app, ui, area);
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

/// Messages dragged out of the list and let go over the tasks: each becomes a task, the mail
/// staying in front (`tasks.fromMessage` with `show` false). While they are over it, the tasks
/// are outlined and the pointer says they are taken.
fn drop_target(app: &mut MailApp, ui: &mut Ui, area: Rect) {
    let t = Tokens::get(ui.ctx());
    let resp = ui.interact(area, ui.scope_id().with("todo_task_drop"), Sense::hover());
    app.hit.add("todoTaskDrop", 0, area);
    if resp.dnd_hover_payload::<crate::mail_view::MessageDrag>().is_some() {
        crate::drag_pointer::note_folder(ui.ctx(), true);
        ui.painter().rect_stroke(area.shrink(1.0), 4.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    }
    if let Some(drag) = resp.dnd_release_payload::<crate::mail_view::MessageDrag>() {
        for message in drag.0.iter().take(MAX_TASKS) {
            let _ = app.run("tasks.fromMessage", json!({"id": message, "show": false}));
        }
    }
}

#[cfg(test)]
#[path = "todo_bar_tests.rs"]
mod todo_bar_tests;
