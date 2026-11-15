// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The All Bookmarks window: a non-modal overview of every bookmark across the mailbox (#951).

use egui::{Align, Color32, Key, LayerId, Order, RichText, Sense, Stroke, pos2, vec2};
use mailcraft_engine::Session;
use mailcraft_model::bookmark::clean_text;
use mailcraft_model::{Bookmark, Id, Mailbox};
use mailcraft_time::Timestamp;
use serde_json::json;

use crate::MailApp;
use crate::theme::{Tokens, regular, semibold};

#[cfg(test)]
#[path = "bookmarks_tests.rs"]
mod bookmarks_tests;

const ROW_H: f32 = 42.0;
const LIST_H: f32 = 210.0;
const DEFAULT_SIZE: egui::Vec2 = vec2(600.0, 460.0);
const MIN_SIZE: egui::Vec2 = vec2(440.0, 340.0);

#[derive(Clone, Debug)]
struct Row {
    id: Id,
    name: String,
    subject: String,
    sender: String,
    quote: String,
    created: Timestamp,
}

fn clean_one_line(s: &str, cap: usize) -> String {
    let stripped = clean_text(s, cap);
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Row {
    fn from_bookmark(mailbox: &Mailbox, bm: &Bookmark) -> Self {
        let (subject, sender) = match mailbox.message(bm.message) {
            Some(msg) => (clean_one_line(&msg.subject, 200), clean_one_line(msg.from.display(), 160)),
            None => (String::new(), String::new()),
        };
        Self { id: bm.id, name: bm.name.clone(), subject, sender, quote: bm.quote.clone(), created: bm.created }
    }

    fn display_title(&self) -> String {
        if !self.name.is_empty() {
            self.name.clone()
        } else if !self.quote.is_empty() {
            clean_text(&self.quote, 120)
        } else {
            let num = self.id.to_string();
            crate::i18n::fmt(tl!("Bookmark #{n}"), &[("n", &num)])
        }
    }
}

/// Persistent UI state for the non-modal All Bookmarks window.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) open: bool,
    pub(crate) filter: String,
    pub(crate) selected: Option<Id>,
    confirm_remove: Option<Id>,
    focus_filter: bool,
    scroll_to_selected: bool,
    cached_valid: bool,
    cached_rev: u64,
    cached_total: usize,
    cached_filter: String,
    cached_rows: Vec<Row>,
}

impl State {
    pub(crate) fn open_window(&mut self) {
        self.open = true;
        self.focus_filter = true;
        self.confirm_remove = None;
        self.cached_valid = false;
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.confirm_remove = None;
    }

    fn ensure_cache(&mut self, session: &Session) {
        let rev = session.rev();
        let total = session.mailbox.bookmarks().len();
        if self.cached_valid && self.cached_rev == rev && self.cached_total == total && self.cached_filter == self.filter {
            self.fix_selection();
            return;
        }
        let query = self.filter.trim().to_lowercase();
        let all = session.mailbox.all_bookmarks();
        let mut rows = Vec::with_capacity(all.len());
        for bm in &all {
            let row = Row::from_bookmark(&session.mailbox, bm);
            if query.is_empty()
                || row.name.to_lowercase().contains(&query)
                || row.quote.to_lowercase().contains(&query)
                || row.subject.to_lowercase().contains(&query)
            {
                rows.push(row);
            }
        }
        self.cached_rows = rows;
        self.cached_rev = rev;
        self.cached_total = total;
        self.cached_filter = self.filter.clone();
        self.cached_valid = true;
        self.fix_selection();
    }

    fn fix_selection(&mut self) {
        let keep = self.selected.is_some_and(|sel| self.cached_rows.iter().any(|r| r.id == sel));
        if !keep {
            self.selected = self.cached_rows.first().map(|r| r.id);
            self.confirm_remove = None;
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.cached_rows.is_empty() {
            self.selected = None;
            return;
        }
        let cur = self.selected.and_then(|sel| self.cached_rows.iter().position(|r| r.id == sel));
        let next = match cur {
            None => 0,
            Some(i) => i.saturating_add_signed(delta).min(self.cached_rows.len().saturating_sub(1)),
        };
        if let Some(row) = self.cached_rows.get(next)
            && self.selected != Some(row.id)
        {
            self.selected = Some(row.id);
            self.confirm_remove = None;
            self.scroll_to_selected = true;
        }
    }

    fn selected_row(&self) -> Option<&Row> {
        let sel = self.selected?;
        self.cached_rows.iter().find(|r| r.id == sel)
    }
}

/// The egui layer of the All Bookmarks window.
pub(crate) fn layer() -> LayerId {
    LayerId::new(Order::Middle, egui::Id::unique("bookmarks_window"))
}

fn filter_id() -> egui::Id {
    egui::Id::unique("bookmarks_filter")
}

fn non_jump_focused(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::unique("bookmarks_non_jump_focused")).unwrap_or(false))
}

fn track_non_jump_focus(ctx: &egui::Context, any_focused: bool) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("bookmarks_non_jump_focused"), any_focused));
}

fn paint_single_line_galley(p: &egui::Painter, text: &str, size: f32, color: Color32, max_width: f32, bold: bool) -> std::sync::Arc<egui::Galley> {
    let font_id = if bold { semibold(size) } else { regular(size) };
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::text::TextFormat { font_id, color, ..Default::default() });
    job.wrap = egui::text::TextWrapping { max_width: max_width.max(8.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    p.layout_job(job)
}

/// Keyboard handling for the non-modal All Bookmarks window: runs before
/// [`crate::keys::global_shortcuts`] so Escape, Enter, and Up/Down act on this window rather
/// than the message list underneath.
pub(crate) fn keys(app: &mut MailApp, ctx: &egui::Context) {
    if !app.bookmarks.open || app.ui.backstage || app.dialog.is_some() || !app.asking.is_empty() {
        return;
    }
    if egui::Popup::is_any_open(ctx) {
        return;
    }
    app.bookmarks.ensure_cache(&app.session);

    if crate::keys::take_escape_any(ctx, layer()) {
        app.bookmarks.close();
        return;
    }
    if crate::keys::active(ctx) != layer() {
        return;
    }

    let mut moved = false;
    let events = ctx.input(|i| i.events.clone());
    for e in events {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { continue };
        if modifiers.any() {
            continue;
        }
        match key {
            Key::ArrowDown => {
                app.bookmarks.move_selection(1);
                moved = true;
            }
            Key::ArrowUp => {
                app.bookmarks.move_selection(-1);
                moved = true;
            }
            _ => {}
        }
    }
    if moved {
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key {
                        key: Key::ArrowDown | Key::ArrowUp,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.is_none()
                )
            });
        });
    }

    if !non_jump_focused(ctx)
        && let Some(sel_id) = app.bookmarks.selected
        && crate::keys::take_enter_out(ctx, layer())
    {
        let _ = app.run("bookmark.jump", json!({"id": sel_id}));
    }
}

/// Draws the non-modal All Bookmarks window when open.
pub(crate) fn show(app: &mut MailApp, ctx: &egui::Context) {
    if !app.bookmarks.open || app.ui.backstage {
        return;
    }
    app.bookmarks.ensure_cache(&app.session);

    let t = Tokens::get(ctx);
    let mut still_open = app.bookmarks.open;
    let mut jump_id: Option<Id> = None;
    let mut remove_id: Option<Id> = None;
    let mut do_close = false;
    let mut any_non_jump_focused = false;

    egui::Window::new(tl!("All Bookmarks"))
        .id(layer().id)
        .order(Order::Middle)
        .collapsible(false)
        .resizable(true)
        .default_size(DEFAULT_SIZE)
        .min_size(MIN_SIZE)
        .open(&mut still_open)
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.surface).stroke(Stroke::new(1.0, t.border_strong)).inner_margin(12.0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);

            // Filter bar
            ui.horizontal(|ui| {
                let clear_w = if app.bookmarks.filter.is_empty() { 0.0 } else { 26.0 };
                let width = (ui.available_width() - clear_w).max(120.0);
                let resp = crate::text_menu::field(ui.add_sized(
                    vec2(width, 26.0),
                    egui::TextEdit::singleline(&mut app.bookmarks.filter).id(filter_id()).hint_text(tl!("Filter bookmarks")),
                ));
                if app.bookmarks.focus_filter {
                    resp.request_focus();
                    app.bookmarks.focus_filter = false;
                }
                if resp.changed() {
                    app.bookmarks.confirm_remove = None;
                }
                let has_text = !app.bookmarks.filter.is_empty();
                if has_text && crate::widgets::icon_button(ui, "close", tl!("Clear filter"), 12.0, true).clicked() {
                    app.bookmarks.filter.clear();
                    app.bookmarks.confirm_remove = None;
                }
            });

            // Re-sync cache if filter changed this frame before painting rows
            app.bookmarks.ensure_cache(&app.session);

            // Virtualized list of all matching bookmarks
            let total_in_mailbox = app.bookmarks.cached_total;
            let row_count = app.bookmarks.cached_rows.len();
            let list_frame =
                egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).corner_radius(4).inner_margin(egui::Margin::symmetric(0, 2));

            list_frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                if total_in_mailbox == 0 {
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), LIST_H),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            ui.label(RichText::new(tl!("No bookmarks yet.")).font(regular(12.5)).color(t.text_dim));
                        },
                    );
                } else if row_count == 0 {
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), LIST_H),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            ui.label(RichText::new(tl!("No bookmarks match.")).font(regular(12.5)).color(t.text_dim));
                        },
                    );
                } else {
                    let now = app.session.now();
                    let twelve = app.session.prefs.twelve_hour;
                    let selected_id = app.bookmarks.selected;
                    let selected_idx = selected_id.and_then(|sel| app.bookmarks.cached_rows.iter().position(|r| r.id == sel));
                    let mut clicked_id: Option<Id> = None;
                    let mut scroll_flag = app.bookmarks.scroll_to_selected;

                    let style = ui.style_mut();
                    style.spacing.scroll = egui::style::ScrollStyle { bar_width: 8.0, ..egui::style::ScrollStyle::solid() };
                    style.spacing.item_spacing.y = 0.0;

                    egui::ScrollArea::vertical()
                        .id_salt("bookmarks_scroll_list")
                        .min_scrolled_height(LIST_H)
                        .max_height(LIST_H)
                        .auto_shrink([false, false])
                        .show_rows(ui, ROW_H, row_count, |ui, range| {
                            if scroll_flag && let Some(idx) = selected_idx {
                                let list_top_y = ui.cursor().min.y - (range.start as f32) * ROW_H;
                                let target = egui::Rect::from_min_size(
                                    pos2(ui.cursor().min.x, list_top_y + (idx as f32) * ROW_H),
                                    vec2(ui.available_width().max(120.0), ROW_H),
                                );
                                ui.scroll_to_rect(target, None);
                                scroll_flag = false;
                            }
                            let rows_slice = app.bookmarks.cached_rows.get(range).unwrap_or(&[]);
                            for row in rows_slice {
                                let title = row.display_title();
                                let date_str = crate::dates::list_label(row.created, now, app.session.offset_at(row.created), twelve);
                                let subject_str = if row.subject.is_empty() { tl!("(no subject)").to_string() } else { row.subject.clone() };
                                let is_selected = selected_id == Some(row.id);

                                let w = ui.available_width().max(120.0);
                                let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
                                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, is_selected, &title));

                                if is_selected {
                                    ui.painter().rect_filled(rect, 0.0, t.selection);
                                    let stripe = egui::Rect::from_min_size(rect.min, vec2(3.0, rect.height()));
                                    ui.painter().rect_filled(stripe, 0.0, t.accent);
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(rect, 0.0, t.hover);
                                }

                                let pad_x = 10.0;
                                let inner_left = rect.min.x + pad_x;
                                let inner_right = (rect.max.x - pad_x).max(inner_left + 16.0);
                                let inner_w = inner_right - inner_left;

                                // Date right-aligned on line 1
                                let date_galley = paint_single_line_galley(ui.painter(), &date_str, 11.0, t.text_dim, inner_w * 0.4, false);
                                let date_w = date_galley.size().x;
                                ui.painter().galley(pos2(inner_right - date_w, rect.min.y + 6.0), date_galley, Color32::TRANSPARENT);

                                // Bookmark name left-aligned on line 1, clipped before date
                                let title_w = (inner_w - date_w - 8.0).max(20.0);
                                let title_galley = paint_single_line_galley(ui.painter(), &title, 12.5, t.text, title_w, true);
                                ui.painter().galley(pos2(inner_left, rect.min.y + 5.0), title_galley, Color32::TRANSPARENT);

                                // Dimmed cleaned subject left-aligned with ellipsis on line 2
                                let subj_galley = paint_single_line_galley(ui.painter(), &subject_str, 11.5, t.text_dim, inner_w, false);
                                ui.painter().galley(pos2(inner_left, rect.min.y + 23.0), subj_galley, Color32::TRANSPARENT);

                                // Subtle row hairline
                                let line_y = rect.max.y - 0.5;
                                ui.painter().line_segment([pos2(rect.min.x, line_y), pos2(rect.max.x, line_y)], Stroke::new(0.5, t.border));

                                if resp.clicked() {
                                    clicked_id = Some(row.id);
                                }
                                if resp.double_clicked() {
                                    clicked_id = Some(row.id);
                                    jump_id = Some(row.id);
                                }
                            }
                        });
                    app.bookmarks.scroll_to_selected = scroll_flag;
                    if let Some(id) = clicked_id
                        && app.bookmarks.selected != Some(id)
                    {
                        app.bookmarks.selected = Some(id);
                        app.bookmarks.confirm_remove = None;
                    }
                }
            });

            // Selected bookmark detail pane
            if let Some(row) = app.bookmarks.selected_row().cloned() {
                let subj = if row.subject.is_empty() { tl!("(no subject)").to_string() } else { row.subject.clone() };
                ui.add(egui::Label::new(RichText::new(subj).font(semibold(12.5)).color(t.text)).wrap().selectable(true));
                if !row.sender.is_empty() {
                    let from = crate::i18n::fmt(tl!("From: {sender}"), &[("sender", &row.sender)]);
                    ui.add(egui::Label::new(RichText::new(from).font(regular(11.5)).color(t.text_dim)).wrap().selectable(true));
                }

                let quote_frame = egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).corner_radius(4).inner_margin(8.0);
                quote_frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .id_salt(("bookmark_quote_detail", row.id))
                        .min_scrolled_height(64.0)
                        .max_height(104.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if row.quote.is_empty() {
                                ui.label(RichText::new(tl!("No saved quote.")).font(regular(12.0)).color(t.text_dim));
                            } else {
                                ui.add(egui::Label::new(RichText::new(&row.quote).font(regular(12.5)).color(t.text)).wrap().selectable(true));
                            }
                        });
                });

                // Footer action buttons
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(tl!("Go to message")).min_size(vec2(108.0, 26.0))).clicked() {
                        jump_id = Some(row.id);
                    }
                    if app.bookmarks.confirm_remove == Some(row.id) {
                        let confirm = ui.add(egui::Button::new(tl!("Confirm Remove")).min_size(vec2(114.0, 26.0)));
                        any_non_jump_focused |= confirm.has_focus();
                        if confirm.clicked() {
                            remove_id = Some(row.id);
                        }
                        let cancel = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0)));
                        any_non_jump_focused |= cancel.has_focus();
                        if cancel.clicked() {
                            app.bookmarks.confirm_remove = None;
                        }
                    } else {
                        let rem = ui.add(egui::Button::new(tl!("Remove")).min_size(vec2(80.0, 26.0)));
                        any_non_jump_focused |= rem.has_focus();
                        if rem.clicked() {
                            app.bookmarks.confirm_remove = Some(row.id);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let close = ui.add(egui::Button::new(tl!("Close")).min_size(vec2(76.0, 26.0)));
                        any_non_jump_focused |= close.has_focus();
                        if close.clicked() {
                            do_close = true;
                        }
                    });
                });
            } else {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let close = ui.add(egui::Button::new(tl!("Close")).min_size(vec2(76.0, 26.0)));
                        any_non_jump_focused |= close.has_focus();
                        if close.clicked() {
                            do_close = true;
                        }
                    });
                });
            }
        });

    track_non_jump_focus(ctx, any_non_jump_focused);

    if !still_open || do_close {
        app.bookmarks.close();
    }
    if let Some(id) = jump_id {
        let _ = app.run("bookmark.jump", json!({"id": id}));
    }
    if let Some(id) = remove_id {
        let _ = app.run("bookmark.remove", json!({"id": id}));
        app.bookmarks.confirm_remove = None;
        app.bookmarks.cached_valid = false;
    }
}
