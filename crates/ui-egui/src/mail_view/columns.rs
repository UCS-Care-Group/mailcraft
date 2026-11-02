// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list in columns (#344): one geometry for the heading row, local messages and
//! results found on the server.

use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::columns::{Column, Columns};
use mailcraft_model::{FlagStatus, Importance, Message};
use serde_json::json;

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

const LEFT: f32 = super::READ_STRIP;
const ACTIONS: f32 = 70.0;
const HEADER: f32 = 28.0;

/// A column's heading. Sent Items, Drafts and the Outbox show who mail went to, so their From
/// column is To, as Outlook's is.
fn label(column: Column, outgoing: bool) -> &'static str {
    match column {
        Column::From if outgoing => tl!("To"),
        Column::From => tl!("From"),
        Column::Subject => tl!("Subject"),
        Column::Date => tl!("Date"),
        Column::Size => tl!("Size"),
        Column::Importance => tl!("Importance"),
        Column::Attachments => tl!("Attachments"),
        Column::Flag => tl!("Flag"),
        Column::Categories => tl!("Categories"),
        Column::Folder => tl!("Folder"),
        Column::Unknown => "",
    }
}

/// Whether the folder in view holds mail going out.
fn outgoing(app: &MailApp) -> bool {
    app.session.mailbox.folder(app.session.view.folder).is_some_and(mailcraft_engine::columns::is_outgoing)
}

pub(super) fn width(columns: &Columns) -> f32 {
    LEFT + columns.items.iter().take(Column::ALL.len()).map(|c| f32::from(c.width.clamp(36, 800))).sum::<f32>() + ACTIONS + super::SCROLL_LANE
}

/// An ordered, finite list of cells, identical for header and body.
fn cells(columns: &Columns, rect: Rect) -> Vec<(Column, Rect)> {
    let mut x = rect.min.x + LEFT;
    columns
        .items
        .iter()
        .take(Column::ALL.len())
        .map(|spec| {
            let cell = Rect::from_min_size(pos2(x, rect.min.y), vec2(f32::from(spec.width.clamp(36, 800)), rect.height()));
            x = cell.max.x;
            (spec.column, cell)
        })
        .collect()
}

/// A heading's menu, as Outlook's: Reverse Sort.
fn heading_menu(app: &mut MailApp, ui: &mut Ui) {
    if ui.button(tl!("Reverse Sort")).clicked() {
        let _ = app.run("view.reverseSort", json!({}));
        ui.close();
    }
}

pub(super) fn header(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER), Sense::click());
    ui.painter().rect_filled(rect, 0.0, t.panel);
    response.context_menu(|ui| heading_menu(app, ui));
    let config = app.session.prefs.columns.clone();
    let outgoing = outgoing(app);
    let cells = cells(&config, rect);
    for (i, &(column, cell)) in cells.iter().enumerate() {
        // The resize handle gets its own non-overlapping hit area; dragging it never sorts.
        let handle = Rect::from_min_max(pos2(cell.max.x - 4.0, cell.min.y), pos2(cell.max.x + 4.0, cell.max.y));
        let resize =
            ui.interact(handle, ui.scope_id().with(("column_resize", column)), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        app.hit.add("columnResize", column as u64, handle);
        if resize.dragged() {
            let mut next = config.clone();
            if let Some(spec) = next.items.get_mut(i) {
                // From the width the drag started at, so that a slow drag adds up instead of
                // rounding back to where it was every frame.
                let from = resize.id.with("from");
                if resize.drag_started() {
                    ui.data_mut(|d| d.insert_temp(from, spec.width));
                }
                let start = ui.data(|d| d.get_temp::<u16>(from)).unwrap_or(spec.width);
                let moved = resize.total_drag_delta().unwrap_or_default().x;
                let width = (f32::from(start) + moved).round().clamp(36.0, 800.0) as u16;
                // Only when the width changes: a held, still pointer changes nothing.
                if width != spec.width {
                    spec.width = width;
                    let _ = app.run("view.columns", json!(next));
                }
            }
        }
        let clickable = Rect::from_min_max(pos2(cell.min.x + 4.0, cell.min.y), pos2(cell.max.x - 4.0, cell.max.y));
        let response = ui.interact(clickable, ui.scope_id().with(("column_sort", column)), Sense::click()).on_hover_text(label(column, outgoing));
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, column.sort().is_some(), label(column, outgoing)));
        app.hit.add("columnHeader", column as u64, clickable);
        if response.hovered() {
            ui.painter().rect_filled(clickable, 0.0, t.hover);
        }
        let active = column.sort() == Some(app.session.view.sort);
        // The column sorted by wears the arrow beside the list's arrangement, pointing as it does.
        let room = cell.width() - 14.0 - if active { SORT_MARK } else { 0.0 };
        let written = paint_clipped(ui.painter(), pos2(cell.min.x + 7.0, cell.center().y), room, label(column, outgoing), regular(12.0), t.text);
        if active && let Some(x) = sort_mark_x(cell, written) {
            super::paint_direction(
                ui.painter(),
                pos2(x, cell.center().y),
                super::direction_up(app.session.view.sort, app.session.view.sort_descending),
                t.text,
            );
        }
        ui.painter().line_segment([cell.right_top(), cell.right_bottom()], Stroke::new(1.0, t.border));
        response.context_menu(|ui| heading_menu(app, ui));
        if response.clicked()
            && let Some(sort) = column.sort()
        {
            let result = if active { app.run("view.reverseSort", json!({})) } else { app.run("view.sort", json!({"by": sort})) };
            if result.is_ok() {
                app.reveal_selection = true;
            }
        }
    }
}

/// The room a heading keeps after its name for the arrow of the column sorted by.
const SORT_MARK: f32 = 14.0;

/// Where the arrow goes after a heading's name `written` wide; none where it would leave the cell.
fn sort_mark_x(cell: Rect, written: f32) -> Option<f32> {
    let x = cell.min.x + 7.0 + written + SORT_MARK / 2.0;
    (x + SORT_MARK / 2.0 <= cell.max.x).then_some(x)
}

/// Metadata only: no body or attachment is loaded to paint a column.
pub(super) fn paint(app: &mut MailApp, ui: &Ui, m: &Message, rect: Rect, unread: bool, members: usize, under: bool, server: bool) {
    let t = Tokens::get(ui.ctx());
    let font = if unread { semibold(12.0) } else { regular(12.0) };
    let overdue = super::FollowUp::of(m, &app.session) == super::FollowUp::Overdue;
    let ink = if overdue { t.overdue } else { t.text };
    // A message under its open conversation starts its first text a step further in (#503).
    let mut indent = if under { super::UNDER_INDENT } else { 0.0 };
    for (column, cell) in cells(&app.session.prefs.columns, rect) {
        let p = ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
        let mut left = cell.min.x + 7.0;
        let text = match column {
            Column::From => {
                left += std::mem::take(&mut indent);
                crate::people_view::one_line(&super::who(&app.session, m, under), 256)
            }
            Column::Subject => {
                left += std::mem::take(&mut indent);
                let subject = if m.subject.trim().is_empty() { tl!("(no subject)") } else { &m.subject };
                let subject = crate::people_view::one_line(subject, 512);
                let draft = if m.draft { tl!("[Draft]") } else { "" };
                let count = if members > 1 { format!(" ({members})") } else { String::new() };
                format!("{draft} {subject}{count}").trim_start().to_string()
            }
            Column::Date => crate::dates::list_label(
                mailcraft_engine::list::when(m),
                app.session.now(),
                app.session.offset_at(mailcraft_engine::list::when(m)),
                app.session.prefs.twelve_hour,
            ),
            // As the row says it (#273): the server's size for what isn't here, nothing when not known.
            Column::Size => m.list_size().map(|bytes| mailcraft_model::size_label_in(crate::i18n::current(), bytes.max(1024))).unwrap_or_default(),
            // Each category's colour, then the names.
            Column::Categories => {
                for c in m.categories.iter().take(4) {
                    let color = crate::theme::c32(app.session.mailbox.category_color(c));
                    p.rect_filled(Rect::from_center_size(pos2(left + 5.0, cell.center().y), vec2(10.0, 10.0)), 2.0, color);
                    left += 13.0;
                }
                if !m.categories.is_empty() {
                    left += 3.0;
                }
                m.categories.iter().take(8).map(|c| crate::people_view::one_line(c, 64)).collect::<Vec<_>>().join("; ")
            }
            Column::Folder => {
                app.session.mailbox.folder(m.folder).map(|f| crate::nav::result_folder_label(&app.session.mailbox, f)).unwrap_or_default()
            }
            _ => String::new(),
        };
        let icon = match column {
            Column::Attachments if crate::reading::list_marks(m).1 => Some("attach"),
            Column::Importance if m.importance == Importance::High => Some("highImportance"),
            Column::Importance if m.importance == Importance::Low => Some("lowImportance"),
            Column::Flag if m.flag == FlagStatus::Flagged => Some("flag"),
            Column::Flag if m.flag == FlagStatus::Complete => Some("complete"),
            _ => None,
        };
        if let Some(icon) = icon {
            icons::paint(&p, Rect::from_center_size(cell.center(), vec2(14.0, 14.0)), icon, ink, t.accent);
        }
        // The server says its sender's checks failed, or its name hides another address (#508):
        // the mark the list without columns shows, before the name.
        if column == Column::From && crate::reading::warning_mark(app, m) {
            let at = Rect::from_center_size(pos2(left + 6.0, cell.center().y), vec2(12.0, 12.0));
            icons::paint(&p, at, "warning", t.icon, t.accent);
            if !server {
                app.hit.add("messageWarning", m.id, at);
            }
            left += 16.0;
        }
        if column == Column::Subject {
            let response = if m.answered {
                Some("reply")
            } else if m.forwarded {
                Some("forward")
            } else {
                None
            };
            // A meeting that came by mail wears its mark, as in the list without columns.
            let meeting = crate::reading::list_marks(m).0;
            for mark in [meeting, m.pinned.then_some("pin"), response].into_iter().flatten() {
                icons::paint(&p, Rect::from_center_size(pos2(left + 7.0, cell.center().y), vec2(12.0, 12.0)), mark, t.text_dim, t.accent);
                left += 16.0;
            }
        }
        paint_clipped(&p, pos2(left, cell.center().y), (cell.max.x - left - 7.0).max(0.0), &text, font.clone(), ink);
        // Cell bounds for deterministic UI tests; row interactions retain (id, copy).
        if !server {
            app.hit.add("messageColumn", (m.id.saturating_mul(16)).saturating_add(column as u64), cell);
        }
    }
}

#[cfg(test)]
#[path = "columns_tests.rs"]
mod columns_tests;
