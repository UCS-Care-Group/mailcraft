// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar's List view (#845): the appointments of the calendars shown as a table, a series
//! one row, under the groups of View › Arrange By. A click on a heading sorts by it and a second
//! turns the order round; the engine sorts and groups ([`calendar_list`]), this only draws.
//!
//! A row is the whole of its event or series: it is selected with no day, so what is done with
//! it (Open, Delete, Forward) is done to the series and nothing asks about one occurrence. A row
//! of a calendar shared as free/busy doesn't open and has no menu.

use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::ListColumn;
use mailcraft_engine::cmd::calendar_list::{self, Group, Row, Section};
use mailcraft_model::{Freq, Id};
use serde_json::json;

use super::{calendar_label, event_menu, has_keyboard, opens, paint_line};
use crate::theme::{Tokens, c32, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// Height of a row, of a group's heading, and of the column headings.
pub(crate) const LINE_H: f32 = 24.0;
/// Room a cell keeps either side of its text.
const PAD: f32 = 6.0;
/// The least room Subject is left with before other columns give way.
const SUBJECT_W: f32 = 180.0;
/// The scroll that is remembered between frames: where the list was, and how much of it showed.
const SCROLL_ID: &str = "calendar_list_scroll";
/// The last day chosen that the list scrolled to ([`mailcraft_engine::ViewState::list_reveal`]).
const REVEAL_ID: &str = "calendar_list_reveal";

/// A column as the window draws it: the engine's, and the two narrow ones of marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Col {
    /// What it is: an appointment, a meeting or a series.
    Icon,
    Attachment,
    Of(ListColumn),
}

impl Col {
    /// Left to right.
    const ALL: [Col; 9] = [
        Col::Icon,
        Col::Attachment,
        Col::Of(ListColumn::Subject),
        Col::Of(ListColumn::Location),
        Col::Of(ListColumn::Start),
        Col::Of(ListColumn::End),
        Col::Of(ListColumn::Recurrence),
        Col::Of(ListColumn::Categories),
        Col::Of(ListColumn::Calendar),
    ];

    /// The order they give way in when the room is short, first to go first. Subject and Start
    /// always stay.
    const GIVE_WAY: [Col; 7] = [
        Col::Of(ListColumn::Calendar),
        Col::Of(ListColumn::Categories),
        Col::Of(ListColumn::Recurrence),
        Col::Attachment,
        Col::Of(ListColumn::Location),
        Col::Of(ListColumn::End),
        Col::Icon,
    ];

    fn width(self) -> f32 {
        match self {
            Col::Icon => 26.0,
            Col::Attachment => 24.0,
            Col::Of(ListColumn::Subject) => SUBJECT_W,
            Col::Of(ListColumn::Location) => 150.0,
            Col::Of(ListColumn::Start | ListColumn::End) => 170.0,
            Col::Of(ListColumn::Recurrence) => 230.0,
            Col::Of(ListColumn::Categories) => 150.0,
            Col::Of(ListColumn::Calendar) => 130.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Col::Icon => tl!("Icon"),
            Col::Attachment => tl!("Attachment"),
            Col::Of(column) => tl!(column.label()),
        }
    }
}

/// The columns that fit in `width`, each with its left edge and its width from 0: those that give
/// way have gone in their order, and Subject has whatever is left over. Never wider than `width`
/// while more than Subject and Start are left; there is no scrolling sideways.
pub(crate) fn fit(width: f32) -> Vec<(Col, f32, f32)> {
    let width = if width.is_finite() { width.max(0.0) } else { 0.0 };
    let mut shown: Vec<Col> = Col::ALL.to_vec();
    for gone in Col::GIVE_WAY {
        if shown.iter().map(|c| c.width()).sum::<f32>() <= width {
            break;
        }
        shown.retain(|c| *c != gone);
    }
    let fixed: f32 = shown.iter().filter(|c| **c != Col::Of(ListColumn::Subject)).map(|c| c.width()).sum();
    let mut x = 0.0;
    shown
        .into_iter()
        .map(|c| {
            let w = if c == Col::Of(ListColumn::Subject) { (width - fixed).max(40.0) } else { c.width() };
            let cell = (c, x, w);
            x += w;
            cell
        })
        .collect()
}

/// One line of the list: a group's heading, or a row of a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Heading(usize),
    Row(usize, usize),
}

/// The lines in order: each group's heading (none when the list isn't grouped) and its rows.
pub(crate) fn lines(sections: &[Section]) -> Vec<Line> {
    let mut out = Vec::new();
    for (n, section) in sections.iter().enumerate() {
        if section.group != Group::All {
            out.push(Line::Heading(n));
        }
        out.extend((0..section.rows.len()).map(|r| Line::Row(n, r)));
    }
    out
}

/// The line the `index`th row (counted through the groups) is on.
fn line_of_row(lines: &[Line], index: usize) -> Option<usize> {
    lines.iter().enumerate().filter(|(_, line)| matches!(line, Line::Row(..))).nth(index).map(|(at, _)| at)
}

fn row_at<'a>(sections: &'a [Section], line: &Line) -> Option<&'a Row> {
    match line {
        Line::Row(s, r) => sections.get(*s)?.rows.get(*r),
        Line::Heading(_) => None,
    }
}

/// A group's heading in words, with how many it holds.
fn heading(group: &Group, count: usize) -> String {
    let name = match group {
        Group::Category(Some(name)) => crate::people_view::one_line(name, 120),
        Group::Category(None) => tl!("No category").to_string(),
        Group::Recurrence(None) => tl!("(none)").to_string(),
        Group::Recurrence(Some(Freq::Daily)) => tl!("Daily").to_string(),
        Group::Recurrence(Some(Freq::Weekly)) => tl!("Weekly").to_string(),
        Group::Recurrence(Some(Freq::Monthly)) => tl!("Monthly").to_string(),
        Group::Recurrence(Some(Freq::Yearly)) => tl!("Yearly").to_string(),
        Group::All => String::new(),
    };
    format!("{name}: {}", tln!(count, "{n} item", "{n} items"))
}

/// Open a row's series in a window of its own, or in the form here where there are none.
fn open(app: &mut MailApp, id: Id) {
    let _ = app.execute("calendar.select", json!({"id": id, "series": true}));
    if app.run_or_else("ui.openWindow", json!({"event": id, "scope": "series"})).is_err() {
        let _ = app.run("calendar.edit", json!({}));
    }
}

pub(crate) fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let sections = calendar_list::sections(&app.session);
    let lines = lines(&sections);
    // The rows' width, beside the scroll bar where it has a lane of its own.
    let width = (ui.available_width() - ui.spacing().scroll.allocated_width()).max(0.0);
    let cells = fit(width);
    headings(app, ui, &t, &cells);
    keys(app, ui, &sections, &lines);

    // A day chosen (Today, Go to Date, the date navigator, this view itself) scrolls to the first
    // row on or after it.
    let memory = egui::Id::unique(SCROLL_ID);
    let reveal_id = egui::Id::unique(REVEAL_ID);
    let wanted = app.session.view.list_reveal;
    let mut area = egui::ScrollArea::vertical().id_salt("calendar_list").auto_shrink([false, false]);
    if ui.ctx().data(|d| d.get_temp::<u32>(reveal_id)) != Some(wanted) {
        ui.ctx().data_mut(|d| d.insert_temp(reveal_id, wanted));
        let row = calendar_list::first_from(&sections, app.session.view.date);
        let line = row.and_then(|row| line_of_row(&lines, row)).unwrap_or(0);
        // With its group's heading over it when it is the group's first.
        let line = if line > 0 && matches!(lines.get(line - 1), Some(Line::Heading(_))) { line - 1 } else { line };
        area = area.vertical_scroll_offset(line as f32 * LINE_H);
    } else if let Some(offset) = ui.ctx().data(|d| d.get_temp::<f32>(memory.with("to"))) {
        area = area.vertical_scroll_offset(offset);
        ui.ctx().data_mut(|d| d.remove::<f32>(memory.with("to")));
    }
    if lines.is_empty() {
        let at = pos2(ui.max_rect().center().x, ui.cursor().min.y + 40.0);
        ui.painter().text(at, egui::Align2::CENTER_CENTER, tl!("There are no items to show in this view."), regular(12.0), t.text_dim);
    }
    let output = area.show_rows(ui, LINE_H, lines.len(), |ui, range| {
        for at in range {
            let (rect, resp) = ui.allocate_exact_size(vec2(width, LINE_H), Sense::click());
            match lines.get(at) {
                Some(Line::Heading(n)) => {
                    if let Some(section) = sections.get(*n) {
                        ui.painter().rect_filled(rect, 0.0, t.group_header);
                        let text = heading(&section.group, section.rows.len());
                        paint_clipped(ui.painter(), pos2(rect.min.x + PAD, rect.center().y), rect.width() - 2.0 * PAD, &text, semibold(12.0), t.text);
                    }
                }
                Some(line @ Line::Row(..)) => {
                    if let Some(row) = row_at(&sections, line) {
                        row_ui(app, ui, &t, &cells, rect, &resp, row);
                    }
                }
                None => {}
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(memory, (output.state.clamped_offset().y, output.inner_rect.height())));
}

/// The column headings: a click sorts by one, a second turns the order round. The one sorted by
/// has the arrow, pointing up from A or the earliest.
fn headings(app: &mut MailApp, ui: &mut Ui, t: &Tokens, cells: &[(Col, f32, f32)]) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), LINE_H + 4.0), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.panel);
    let (by, descending) = (app.session.view.list_sort, app.session.view.list_descending);
    for (n, &(col, x, w)) in cells.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(rect.min.x + x, rect.min.y), vec2(w, rect.height()));
        let sorts = matches!(col, Col::Of(column) if column.sorts());
        let resp = ui.interact(cell, ui.scope_id().with(("calendar_list_heading", n)), if sorts { Sense::click() } else { Sense::hover() });
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, sorts, col.label()));
        // (By the column, wherever it is now.)
        app.hit.add("listHeading", Col::ALL.iter().position(|c| *c == col).unwrap_or(n) as u64, cell);
        if sorts && resp.hovered() {
            p.rect_filled(cell, 0.0, t.hover);
        }
        let active = col == Col::Of(by);
        let room = (w - 2.0 * PAD - if active { 14.0 } else { 0.0 }).max(0.0);
        let written = match col {
            Col::Icon => {
                icons::paint(&p, Rect::from_center_size(cell.center(), vec2(14.0, 14.0)), "appointment", t.icon, t.accent);
                0.0
            }
            Col::Attachment => {
                icons::paint(&p, Rect::from_center_size(cell.center(), vec2(14.0, 14.0)), "attach", t.icon, t.accent);
                0.0
            }
            Col::Of(_) => paint_clipped(&p, pos2(cell.min.x + PAD, cell.center().y), room, col.label(), regular(12.0), t.text),
        };
        if active {
            crate::mail_view::paint_direction(&p, pos2(cell.min.x + PAD + written + 9.0, cell.center().y), !descending, t.text);
        }
        p.line_segment([cell.right_top(), cell.right_bottom()], Stroke::new(1.0, t.border));
        let resp = resp.on_hover_text(col.label());
        if let (true, Col::Of(column)) = (resp.clicked(), col) {
            let _ = app.run("calendar.listSort", json!({"by": column}));
        }
    }
    p.hline(rect.x_range(), rect.max.y - 0.5, Stroke::new(1.0, t.border));
}

/// One row: its cells, and what a click, a double click and the right button do to it.
fn row_ui(app: &mut MailApp, ui: &Ui, t: &Tokens, cells: &[(Col, f32, f32)], rect: Rect, resp: &egui::Response, row: &Row) {
    let id = row.event;
    let selected = app.session.sel.event == Some(id);
    app.hit.add("listRow", id, rect);
    if selected {
        ui.painter().rect_filled(rect, 0.0, t.selection);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    ui.painter().hline(rect.x_range(), rect.max.y - 0.5, Stroke::new(1.0, t.border_faint));
    let event = app.session.mailbox.events.get(&id).cloned();
    let twelve = app.session.prefs.twelve_hour;
    let when = |at: mailcraft_time::DateTime, whole_day: bool| {
        let day = crate::dates::weekday_medium(at.date);
        if whole_day { day } else { format!("{day} {}", crate::dates::clock(at, twelve)) }
    };
    for &(col, x, w) in cells {
        let cell = Rect::from_min_size(pos2(rect.min.x + x, rect.min.y), vec2(w, rect.height()));
        let p = ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
        let mut left = cell.min.x + PAD;
        let mark = |name: &str| icons::paint(&p, Rect::from_center_size(cell.center(), vec2(14.0, 14.0)), name, t.icon, t.accent);
        let text = match col {
            Col::Icon => {
                let meeting = event.as_ref().is_some_and(|e| e.is_meeting()) && !row.free_busy;
                mark(if row.freq.is_some() {
                    "repeat"
                } else if meeting {
                    "meeting"
                } else {
                    "appointment"
                });
                String::new()
            }
            Col::Attachment => {
                if row.attachment {
                    mark("attach");
                }
                String::new()
            }
            Col::Of(ListColumn::Subject) => {
                let text = if row.free_busy {
                    tl!(row.show_as.label()).to_string()
                } else if row.subject.is_empty() {
                    tl!("(No title)").to_string()
                } else {
                    crate::people_view::one_line(&row.subject, 300)
                };
                let font = regular(12.0);
                paint_line(&p, pos2(left, cell.center().y), (cell.max.x - left - PAD).max(0.0), &text, font, t.text, row.cancelled);
                continue;
            }
            Col::Of(ListColumn::Location) => crate::people_view::one_line(&row.location, 300),
            Col::Of(ListColumn::Start) => when(row.start, row.all_day),
            // A whole day ends at the next midnight: the day it ends on is the one before.
            Col::Of(ListColumn::End) => when(if row.all_day { row.end.add_minutes(-1).max(row.start) } else { row.end }, row.all_day),
            Col::Of(ListColumn::Recurrence) => row.recurrence.clone(),
            Col::Of(ListColumn::Categories) => {
                let account = event.as_ref().map_or(0, |e| app.session.mailbox.event_account(e));
                for name in row.categories.iter().take(4) {
                    let color = c32(app.session.mailbox.category_color_of(account, name));
                    p.rect_filled(Rect::from_center_size(pos2(left + 5.0, cell.center().y), vec2(10.0, 10.0)), 2.0, color);
                    left += 13.0;
                }
                row.categories.iter().map(|c| crate::people_view::one_line(c, 64)).collect::<Vec<_>>().join("; ")
            }
            Col::Of(ListColumn::Calendar) => app.session.mailbox.calendar(row.calendar).map(|c| calendar_label(&app.session, c)).unwrap_or_default(),
        };
        paint_clipped(&p, pos2(left, cell.center().y), (cell.max.x - left - PAD).max(0.0), &text, regular(12.0), t.text);
    }
    if opens(resp) {
        if row.free_busy {
            let _ = app.run("calendar.select", json!({"id": id, "series": true}));
        } else {
            open(app, id);
        }
    } else if resp.clicked() || resp.secondary_clicked() {
        let _ = app.run("calendar.select", json!({"id": id, "series": true}));
    }
    if let (Some(event), false) = (event, row.free_busy) {
        event_menu(app, resp, &event, None);
    }
}

/// The keys of the list, while nothing else has the keyboard: Up and Down go from row to row,
/// Enter opens the one selected; Alt with a digit, = and - leave the list for the grids, as they
/// do from any other view.
fn keys(app: &mut MailApp, ui: &Ui, sections: &[Section], lines: &[Line]) {
    use egui::Key;
    let ctx = ui.ctx().clone();
    if !has_keyboard(app, &ctx) {
        return;
    }
    let rows: Vec<(usize, &Row)> = lines.iter().enumerate().filter_map(|(at, line)| row_at(sections, line).map(|row| (at, row))).collect();
    let current = app.session.sel.event.and_then(|id| rows.iter().position(|(_, row)| row.event == id));
    if let Some((_, row)) = current.and_then(|n| rows.get(n))
        && crate::keys::take_enter_alone(&ctx, ui.layer_id())
        && !row.free_busy
    {
        open(app, row.event);
    }
    let presses: Vec<(Key, egui::Modifiers)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
            .take(16)
            .collect()
    });
    for (key, modifiers) in presses {
        let (alt, command) = (modifiers.alt, modifiers.command);
        let _ = match key {
            _ if alt && !command && !modifiers.shift && super::days_key(key).is_some() => {
                app.run("view.days", json!({"count": super::days_key(key), "start": app.session.view.date.iso()}))
            }
            Key::Equals | Key::Plus if alt && !command => app.run("view.month", json!({})),
            Key::Minus if alt && !command => app.run("view.week", json!({})),
            Key::ArrowUp | Key::ArrowDown if !alt && !command => {
                let next = match (current, key) {
                    (None, _) => 0,
                    (Some(n), Key::ArrowUp) => n.saturating_sub(1),
                    (Some(n), _) => (n + 1).min(rows.len().saturating_sub(1)),
                };
                let Some(&(line, row)) = rows.get(next) else { continue };
                scroll_into_view(&ctx, line);
                app.run("calendar.select", json!({"id": row.event, "series": true}))
            }
            _ => continue,
        };
    }
}

/// Scroll so that `line` shows, by as little as that takes.
fn scroll_into_view(ctx: &egui::Context, line: usize) {
    let memory = egui::Id::unique(SCROLL_ID);
    let Some((offset, height)) = ctx.data(|d| d.get_temp::<(f32, f32)>(memory)) else { return };
    let (top, bottom) = (line as f32 * LINE_H, (line + 1) as f32 * LINE_H);
    let to = if top < offset {
        top
    } else if bottom > offset + height {
        bottom - height
    } else {
        return;
    };
    ctx.data_mut(|d| d.insert_temp(memory.with("to"), to.max(0.0)));
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod list_tests;
