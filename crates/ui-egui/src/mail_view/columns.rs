// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list in columns (#344): one geometry for the heading row, local messages and
//! results found on the server, and Outlook's Show Columns dialog to choose them.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::columns::{Column, ColumnSpec, Columns};
use mailcraft_model::{FlagStatus, Importance, Message};
use serde_json::json;

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

const LEFT: f32 = super::READ_STRIP;
const ACTIONS: f32 = 70.0;
const HEADER: f32 = 28.0;
/// How far below or above the heading row a heading is dropped to take its column away, as
/// Outlook does: past the first rows, so a drag along the row that strays a little still moves it.
const DROP_OFF: f32 = 48.0;

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

/// The columns with `column` taken away (never Subject).
fn without(columns: &Columns, column: Column) -> Columns {
    Columns { items: columns.items.iter().filter(|c| c.column != column || column == Column::Subject).cloned().collect(), ..columns.clone() }
}

/// A heading's menu, as Outlook's: Reverse Sort, Remove This Column, and the Show Columns dialog.
fn heading_menu(app: &mut MailApp, ui: &mut Ui, column: Option<Column>) {
    if ui.button(tl!("Reverse Sort")).clicked() {
        let _ = app.run("view.reverseSort", json!({}));
        ui.close();
    }
    if let Some(column) = column {
        let removable = column != Column::Subject;
        if ui.add_enabled(removable, egui::Button::new(tl!("Remove This Column"))).clicked() {
            let _ = app.run("view.columns", json!(without(&app.session.prefs.columns, column)));
            ui.close();
        }
    }
    ui.separator();
    if ui.button(tl!("Add Columns…")).clicked() {
        let _ = app.run("ui.dialog", json!({"name": "showColumns"}));
        ui.close();
    }
}

pub(super) fn header(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER), Sense::click());
    ui.painter().rect_filled(rect, 0.0, t.panel);
    response.context_menu(|ui| heading_menu(app, ui, None));
    let config = app.session.prefs.columns.clone();
    let outgoing = outgoing(app);
    let cells = cells(&config, rect);
    // A heading being dragged: where it would go, or that it would be taken away.
    let mut dragging: Option<(usize, egui::Pos2)> = None;
    let mut dropped: Option<(usize, egui::Pos2)> = None;
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
        let response =
            ui.interact(clickable, ui.scope_id().with(("column_sort", column)), Sense::click_and_drag()).on_hover_text(label(column, outgoing));
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, column.sort().is_some(), label(column, outgoing)));
        app.hit.add("columnHeader", column as u64, clickable);
        if response.hovered() || response.dragged() {
            ui.painter().rect_filled(clickable, 0.0, t.hover);
        }
        let active = column.sort() == Some(app.session.view.sort);
        let title = if active {
            format!("{} {}", label(column, outgoing), if app.session.view.sort_descending { "↓" } else { "↑" })
        } else {
            label(column, outgoing).to_string()
        };
        paint_clipped(ui.painter(), pos2(cell.min.x + 7.0, cell.center().y), cell.width() - 14.0, &title, regular(12.0), t.text);
        ui.painter().line_segment([cell.right_top(), cell.right_bottom()], Stroke::new(1.0, t.border));
        response.context_menu(|ui| heading_menu(app, ui, Some(column)));
        let at = response.interact_pointer_pos();
        if response.dragged()
            && let Some(at) = at
        {
            dragging = Some((i, at));
        }
        if response.drag_stopped()
            && let Some(at) = at
        {
            dropped = Some((i, at));
        }
        if response.clicked()
            && let Some(sort) = column.sort()
        {
            let result = if active { app.run("view.reverseSort", json!({})) } else { app.run("view.sort", json!({"by": sort})) };
            if result.is_ok() {
                app.reveal_selection = true;
            }
        }
    }
    // Outlook's way of arranging columns: a heading dragged along the row goes where it is
    // dropped, and one dragged off the row is taken away (Subject stays).
    let off = |at: egui::Pos2| at.y < rect.min.y - DROP_OFF || at.y > rect.max.y + DROP_OFF;
    if let Some((column, at)) = dragging.and_then(|(i, at)| Some((cells.get(i)?.0, at))) {
        let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("column_drag")));
        if off(at) && column != Column::Subject {
            p.text(at, Align2::CENTER_CENTER, "✕", semibold(22.0), t.red);
        } else {
            let x = match place(&cells, at.x) {
                Some(j) => cells.get(j).map(|c| c.1.min.x).or_else(|| cells.last().map(|c| c.1.max.x)).unwrap_or(rect.min.x + LEFT),
                None => rect.min.x + LEFT,
            };
            p.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], Stroke::new(2.0, t.accent));
            p.text(at + vec2(12.0, 0.0), Align2::LEFT_CENTER, label(column, outgoing), regular(12.0), t.text_dim);
        }
    }
    if let Some((i, column, at)) = dropped.and_then(|(i, at)| Some((i, cells.get(i)?.0, at))) {
        let next = if off(at) {
            without(&config, column)
        } else {
            let mut next = config.clone();
            // Where it goes among the others, once it is out of its own place.
            let to = place(&cells, at.x).unwrap_or(0);
            let to = if to > i { to - 1 } else { to };
            if to != i && i < next.items.len() {
                let spec = next.items.remove(i);
                next.items.insert(to.min(next.items.len()), spec);
            }
            next
        };
        if next != config {
            let _ = app.run("view.columns", json!(next));
        }
    }
}

/// The gap between headings nearest to `x`: 0 before the first, `cells.len()` after the last.
fn place(cells: &[(Column, Rect)], x: f32) -> Option<usize> {
    (!cells.is_empty()).then(|| cells.iter().take_while(|(_, cell)| cell.center().x < x).count())
}

/// Metadata only: no body or attachment is loaded to paint a column.
pub(super) fn paint(app: &mut MailApp, ui: &Ui, m: &Message, rect: Rect, unread: bool, members: usize, server: bool) {
    let t = Tokens::get(ui.ctx());
    let font = if unread { semibold(12.0) } else { regular(12.0) };
    let overdue = super::FollowUp::of(m, &app.session) == super::FollowUp::Overdue;
    let ink = if overdue { t.overdue } else { t.text };
    for (column, cell) in cells(&app.session.prefs.columns, rect) {
        let p = ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
        let mut left = cell.min.x + 7.0;
        let text = match column {
            Column::From => crate::people_view::one_line(&super::who(&app.session, m), 256),
            Column::Subject => {
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

/// Outlook's Show Columns dialog (View › Add Columns): the columns there are on the left, the
/// ones shown in their order on the right, with Add, Remove, Move Up and Move Down between.
/// Nothing changes until OK, which also turns the columns on.
pub struct Chooser {
    shown: Vec<ColumnSpec>,
    /// The column picked in either list.
    available: Option<Column>,
    picked: Option<Column>,
    error: String,
}

pub(crate) const CHOOSER_WIDTH: f32 = 480.0;

impl Chooser {
    pub(crate) fn new(app: &MailApp) -> Chooser {
        let mut columns = app.session.prefs.columns.clone();
        columns.sanitize();
        Chooser { shown: columns.items, available: None, picked: None, error: String::new() }
    }

    /// The dialog's content; `true` when it is done with.
    pub(crate) fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let outgoing = outgoing(app);
        let rest: Vec<Column> = Column::ALL.into_iter().filter(|c| !self.shown.iter().any(|s| s.column == *c)).collect();
        if self.available.is_some_and(|c| !rest.contains(&c)) {
            self.available = None;
        }
        let list = |ui: &mut Ui, title: &str, columns: &[Column], picked: &mut Option<Column>, hit: &'static str, app: &mut MailApp| {
            ui.vertical(|ui| {
                ui.set_width(170.0);
                ui.label(title);
                egui::Frame::new().stroke(Stroke::new(1.0, t.border)).inner_margin(2.0).show(ui, |ui| {
                    ui.set_min_size(vec2(166.0, 210.0));
                    ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                    for &column in columns {
                        let r = ui.add_sized(vec2(166.0, 22.0), egui::Button::selectable(*picked == Some(column), label(column, outgoing)));
                        app.hit.add(hit, column as u64, r.rect);
                        if r.clicked() {
                            *picked = Some(column);
                        }
                    }
                });
            });
        };
        let mut add = false;
        let mut remove = false;
        let mut up = false;
        let mut down = false;
        let shown: Vec<Column> = self.shown.iter().map(|s| s.column).collect();
        let at = self.picked.and_then(|c| shown.iter().position(|s| *s == c));
        ui.horizontal_top(|ui| {
            list(ui, tl!("Available columns:"), &rest, &mut self.available, "columnAvailable", app);
            ui.vertical(|ui| {
                ui.add_space(90.0);
                let r = ui.add_enabled(self.available.is_some(), egui::Button::new(tl!("Add ->")).min_size(vec2(90.0, 24.0)));
                app.hit.add("columnsAdd", 0, r.rect);
                add = r.clicked();
                let r =
                    ui.add_enabled(self.picked.is_some_and(|c| c != Column::Subject), egui::Button::new(tl!("<- Remove")).min_size(vec2(90.0, 24.0)));
                app.hit.add("columnsRemove", 0, r.rect);
                remove = r.clicked();
            });
            ui.vertical(|ui| {
                list(ui, tl!("Show these columns in this order:"), &shown, &mut self.picked, "columnShown", app);
                ui.horizontal(|ui| {
                    let r = ui.add_enabled(at.is_some_and(|i| i > 0), egui::Button::new(tl!("Move Up")));
                    app.hit.add("columnsUp", 0, r.rect);
                    up = r.clicked();
                    let r = ui.add_enabled(at.is_some_and(|i| i + 1 < shown.len()), egui::Button::new(tl!("Move Down")));
                    app.hit.add("columnsDown", 0, r.rect);
                    down = r.clicked();
                });
            });
        });
        // Added below the one picked on the right, else at the end, and picked there.
        if add && let Some(column) = self.available.take() {
            let to = at.map_or(self.shown.len(), |i| i + 1);
            self.shown.insert(to, ColumnSpec::new(column));
            self.picked = Some(column);
        }
        if remove && let Some(i) = at.filter(|_| self.picked != Some(Column::Subject)) {
            let gone = self.shown.remove(i);
            self.available = Some(gone.column);
            self.picked = self.shown.get(i.min(self.shown.len().saturating_sub(1))).map(|s| s.column);
        }
        if up && let Some(i) = at.filter(|i| *i > 0) {
            self.shown.swap(i, i - 1);
        }
        if down && let Some(i) = at.filter(|i| i + 1 < self.shown.len()) {
            self.shown.swap(i, i + 1);
        }
        if !self.error.is_empty() {
            ui.label(egui::RichText::new(self.error.as_str()).color(t.red));
        }
        let (ok, cancel) = crate::dialogs::buttons(ui, tl!("OK"), true);
        if ok {
            match app.run("view.columns", json!({"enabled": true, "items": self.shown})) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        cancel
    }
}

#[cfg(test)]
#[path = "columns_tests.rs"]
mod columns_tests;
