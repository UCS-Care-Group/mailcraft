// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversation of the selected message, stacked in the reading pane (#1778): in conversation
//! view with View › Conversation Settings › Show Whole Conversation in Reading Pane on (Gmail's
//! way, beyond Outlook's, so off by default: `view.conversationSettings`), a message that is one of several of its conversation ([`Session::conversation`]: its
//! account's messages of the same topic linked by their headers or their server's conversation
//! id, oldest first, but for those in Deleted Items and Junk E-mail) is read with the rest of its
//! conversation in one column that scrolls, under the conversation's subject and actions.
//!
//! The selected message is shown whole, at [`Place::Pane`], as it is when it is read alone; so is
//! each one not read yet, and each one the owner opened, each at a [`Place::Thread`] of its own,
//! with a selection and a layout of its own. Every other is one row (who sent it, when, the
//! beginning of its text) that opens it when clicked; a click on the line of who sent an open one
//! and when closes it again. Expand All and Collapse All open and close them all, but the
//! selected one. Selecting a message reads its whole conversation with it, when File › Options
//! marks what is opened as read (`mail.select`); opening or closing a
//! message of the column changes nothing of the mailbox, nor what is selected.
//!
//! The earlier messages a message quotes under its own text (the conversation, which is all here
//! already) are left out of it, with a line under its text that shows them, and hides them again
//! ([`mailcraft_html::history`]).
//!
//! [`Session::conversation`]: mailcraft_engine::Session::conversation

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Weak};

use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::{Id, Message};
use serde_json::json;

use super::{ACTIONS, Place, react, select, view};
use crate::MailApp;
use crate::theme::{Tokens, paint_clipped, regular, semibold};

/// Most messages of one conversation stacked: the selected one and the newest. A row says how
/// many older ones there are.
pub(crate) const MAX_STACKED: usize = 200;
/// Most messages shown whole at once besides the selected one: those the owner opened last
/// first, then the newest of those not read yet. The rest are rows. Fewer than the layouts kept
/// for the column ([`super::view::KEPT_IN_STACK`]), so that each is laid out once.
pub(crate) const MAX_OPEN: usize = 48;
/// A closed message's row.
pub(crate) const ROW_HEIGHT: f32 = 54.0;
/// Most characters of a closed message's text its row shows.
const PREVIEW_CHARS: usize = 200;

/// What is open in the column, for the message it was shown for. Kept in egui's memory under
/// one key: another selection starts again.
#[derive(Clone, Default)]
struct Open {
    focus: Id,
    /// The frame it was last shown in: a selection that comes back to the same message after
    /// another starts again.
    frame: u64,
    /// Opened by the owner (a row clicked, Expand All) or read by selecting, the last last.
    opened: Vec<Id>,
    /// [`Session::read_on_open_count`](mailcraft_engine::Session::read_on_open_count) as last
    /// taken in: the messages a newer selection read are opened once.
    read_count: u64,
    /// Closed by the owner (its header clicked, Collapse All), though not read yet or selected.
    closed: BTreeSet<Id>,
    /// Messages whose quoted earlier messages the owner asked to see.
    quoted: BTreeSet<Id>,
    /// The column was scrolled to the selected message once.
    scrolled: bool,
    /// The beginnings of the closed messages' text, by the message they were made of.
    previews: HashMap<Id, (Weak<Message>, Arc<str>)>,
}

fn key() -> egui::Id {
    egui::Id::unique("mailcraft-conversation-stack")
}

/// The conversation the reading pane stacks for the selected message `focus`: in conversation
/// view with Conversation Settings › Show Whole Conversation in Reading Pane on (Gmail's way, not
/// Outlook's, so off by default), when it is one of more than one. `None` reads it alone.
pub(crate) fn stacked(app: &mut MailApp, focus: Id) -> Option<Arc<Vec<Id>>> {
    if !(app.session.view.conversations && app.session.view.whole_conversation) {
        return None;
    }
    let thread = app.session.conversation(focus);
    (thread.len() > 1 && thread.contains(&focus)).then_some(thread)
}

impl Open {
    /// `id` is opened (last), and no longer closed.
    fn open(&mut self, id: Id) {
        self.closed.remove(&id);
        self.opened.retain(|x| *x != id);
        self.opened.push(id);
        if self.opened.len() > MAX_STACKED {
            self.opened.remove(0);
        }
    }

    /// `id` is closed.
    fn close(&mut self, id: Id) {
        self.opened.retain(|x| *x != id);
        self.closed.insert(id);
    }
}

/// Whether `m` would be shown whole in the column of `focus`, room aside ([`shown_whole`]).
fn is_open(open: &Open, m: &Message, focus: Id) -> bool {
    !open.closed.contains(&m.id) && (m.id == focus || open.opened.contains(&m.id) || !m.read)
}

/// The messages of `thread` the column lists: the newest [`MAX_STACKED`], or, when the selected
/// one is older than those, it and the newest but one; oldest first.
pub(crate) fn listed(thread: &[Id], focus: Id) -> Vec<Id> {
    let newest = |n: usize| thread.get(thread.len().saturating_sub(n)..).unwrap_or(thread);
    if thread.len() <= MAX_STACKED || newest(MAX_STACKED).contains(&focus) {
        return newest(MAX_STACKED).to_vec();
    }
    let mut ids = vec![focus];
    ids.extend_from_slice(newest(MAX_STACKED - 1));
    ids
}

/// Of the messages listed, those shown whole: the selected one, and at most [`MAX_OPEN`] others
/// that would be open, those the owner opened last first, then the newest.
fn shown_whole(app: &MailApp, open: &Open, listed: &[Id], focus: Id) -> HashSet<Id> {
    let would = |id: &Id| *id != focus && app.session.mailbox.message(*id).is_some_and(|m| is_open(open, m, focus));
    let mut others: HashSet<Id> = HashSet::new();
    let by_owner = open.opened.iter().rev().filter(|id| listed.contains(id) && would(id));
    let newest = listed.iter().rev().filter(|id| would(id));
    for id in by_owner.chain(newest) {
        if others.len() >= MAX_OPEN {
            break;
        }
        others.insert(*id);
    }
    // The selected one, unless the owner closed it.
    if !open.closed.contains(&focus) {
        others.insert(focus);
    }
    others
}

/// The column: the conversation's subject and actions, then its messages, oldest first.
pub(crate) fn show(app: &mut MailApp, ui: &mut Ui, focus: &Arc<Message>, thread: &[Id]) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let mut open = ui.data(|d| d.get_temp::<Open>(key())).unwrap_or_default();
    let frame = ui.ctx().cumulative_frame_nr();
    if open.focus != focus.id || open.frame.saturating_add(1) < frame {
        // Those selecting it just read with it weren't read until then: they are open, as the
        // ones not read yet are.
        let opened = app.session.read_on_open().iter().copied().filter(|id| thread.contains(id)).collect();
        open = Open { focus: focus.id, opened, read_count: app.session.read_on_open_count(), ..Open::default() };
    }
    // The same message selected again, reading others with it: those are opened, once, and what
    // the owner closed since stays closed.
    if open.read_count != app.session.read_on_open_count() {
        open.read_count = app.session.read_on_open_count();
        for id in app.session.read_on_open().iter().copied().filter(|id| thread.contains(id)).collect::<Vec<_>>() {
            open.open(id);
        }
    }
    open.frame = frame;
    let listed = listed(thread, focus.id);
    title(app, ui, focus, &listed, thread.len(), &t, &mut open);
    let older = thread.len().saturating_sub(listed.len());
    let whole = shown_whole(app, &open, &listed, focus.id);
    let room = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    ui.scope_builder(egui::UiBuilder::new().max_rect(room), |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(("conversation", thread.first().copied().unwrap_or_default())).show(
            ui,
            |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.set_width(ui.available_width());
                if older > 0 {
                    let said =
                        tln!(older, "{n} older message of this conversation isn't shown", "{n} older messages of this conversation aren't shown");
                    egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 20, top: 8, bottom: 8 }).show(ui, |ui| {
                        ui.label(egui::RichText::new(said).font(regular(11.5)).color(t.text_dim));
                    });
                    rule(ui, &t);
                }
                for id in &listed {
                    let Some(m) = app.session.mailbox.messages.get(id).cloned() else { continue };
                    if whole.contains(&m.id) {
                        let place = if m.id == focus.id { Place::Pane } else { Place::Thread(m.id) };
                        let top = ui.cursor().min.y;
                        if card(app, ui, &m, place, &mut open) {
                            open.close(m.id);
                        }
                        let shown = Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().min.y.max(top + 1.0));
                        if place != Place::Pane {
                            app.hit.add("conversationCard", m.id, shown.intersect(ui.clip_rect()));
                        }
                        if m.id == focus.id && !open.scrolled {
                            ui.scroll_to_rect(shown.with_max_y(top + 1.0), Some(egui::Align::TOP));
                            open.scrolled = true;
                        }
                    } else if row(app, ui, &m, &t, &mut open).clicked() {
                        open.open(m.id);
                    }
                    rule(ui, &t);
                }
            },
        );
    });
    ui.data_mut(|d| d.insert_temp(key(), open));
}

/// The conversation's subject, how many messages it has, Expand All and Collapse All, and the
/// actions on the selected message.
fn title(app: &mut MailApp, ui: &mut Ui, focus: &Arc<Message>, listed: &[Id], total: usize, t: &Tokens, open: &mut Open) {
    egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 12, top: 12, bottom: 8 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        ui.horizontal(|ui| {
            let topic = mailcraft_model::normalize_subject(&focus.subject);
            let subject = if topic.trim().is_empty() { tl!("(no subject)").to_string() } else { topic };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                for (icon, tip, id) in ACTIONS.iter().rev() {
                    let sc = crate::widgets::shortcut_text(app, id);
                    if crate::widgets::icon_button(ui, icon, &format!("{} ({sc})", tl!(tip)), 17.0, true).clicked() {
                        let _ = app.run(id, json!({}));
                    }
                    // React sits between Forward and Delete (#1544).
                    if *id == "mail.delete" {
                        ui.push_id(("react", focus.id), |ui| react::button(app, ui, focus));
                    }
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(subject).font(semibold(17.0)).color(t.text)).wrap().selectable(false));
                });
            });
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
            let count = tln!(total, "{n} message in this conversation", "{n} messages in this conversation");
            ui.label(egui::RichText::new(count).font(regular(11.5)).color(t.text_dim));
            let expand = ui.link(egui::RichText::new(tl!("Expand All")).font(regular(11.5)));
            app.hit.add("conversationExpandAll", 0, expand.rect);
            if expand.clicked() {
                // The newest last: those are shown whole first when there are more than room.
                open.closed.clear();
                for id in listed {
                    open.open(*id);
                }
            }
            let collapse = ui.link(egui::RichText::new(tl!("Collapse All")).font(regular(11.5)));
            app.hit.add("conversationCollapseAll", 0, collapse.rect);
            if collapse.clicked() {
                open.opened.clear();
                open.closed = listed.iter().copied().filter(|id| *id != focus.id).collect();
            }
        });
    });
    rule(ui, t);
}

/// A message shown whole in the column: its header (without the subject, said above), the bars
/// under it, and its body, which scrolls sideways when a table in it is wider than the pane,
/// without the earlier messages it quotes unless the owner asked for them. Whether the owner
/// clicked its header to close it.
fn card(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, place: Place, open: &mut Open) -> bool {
    let (access, close) = super::above_body(app, ui, m, place, true);
    let whole = open.quoted.contains(&m.id);
    view::page(app, place, ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::horizontal().auto_shrink([false, true]).id_salt(("body", place, m.id)).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 20, top: 12, bottom: 12 }).show(ui, |ui| {
                // Each body of the column in a scope of its own: what it senses is its own.
                ui.push_id(("stackedBody", m.id), |ui| view::show_in(app, ui, m, place, access, true, !whole));
            });
        });
    });
    // The earlier messages it quotes, to show or hide.
    if view::has_history(ui.ctx(), place, m.id) {
        let t = Tokens::get(ui.ctx());
        egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 20, top: 0, bottom: 12 }).show(ui, |ui| {
            let said = if whole { tl!("Hide quoted text") } else { tl!("··· Show quoted text") };
            let link = ui.link(egui::RichText::new(said).font(regular(11.5)).color(t.text_dim));
            app.hit.add("conversationQuoted", m.id, link.rect);
            if link.clicked() && !open.quoted.remove(&m.id) {
                open.quoted.insert(m.id);
            }
        });
    }
    select::settle(app, ui, place);
    close
}

/// A closed message: who sent it, when, and the beginning of its text, on a row that opens it.
fn row(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, t: &Tokens, open: &mut Open) -> egui::Response {
    let (r, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    app.hit.add("conversationRow", m.id, r.intersect(ui.clip_rect()));
    if !ui.is_rect_visible(r) {
        return response;
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter().clone();
    if response.hovered() {
        p.rect_filled(r, 0.0, t.hover);
    }
    let left = r.min.x + 20.0;
    if !m.read {
        p.circle_filled(pos2(r.min.x + 9.0, r.min.y + 18.0), 3.5, t.accent);
    }
    crate::sender_images::avatar(app, ui, pos2(left + 15.0, r.center().y), 15.0, m, &m.from);
    let text_left = left + 40.0;
    let when = crate::dates::list_label(m.date, app.session.now(), app.session.offset_at(m.date), app.session.prefs.twelve_hour);
    let date = p.layout_no_wrap(when, regular(11.5), t.text_dim);
    let date_left = r.max.x - 12.0 - date.size().x;
    p.galley(pos2(date_left, r.min.y + 18.0 - date.size().y / 2.0), date, t.text_dim);
    let name_font = if m.read { regular(13.0) } else { semibold(13.0) };
    paint_clipped(&p, pos2(text_left, r.min.y + 18.0), (date_left - 12.0 - text_left).max(0.0), m.from.display(), name_font, t.text);
    let preview = preview(open, m);
    paint_clipped(&p, pos2(text_left, r.min.y + 37.0), (r.max.x - 12.0 - text_left).max(0.0), &preview, regular(11.5), t.text_dim);
    response
}

/// The beginning of `m`'s text, made once while it stays the same message.
fn preview(open: &mut Open, m: &Arc<Message>) -> Arc<str> {
    if let Some((of, line)) = open.previews.get(&m.id)
        && std::ptr::eq(of.as_ptr(), Arc::as_ptr(m))
    {
        return line.clone();
    }
    let line: Arc<str> = mailcraft_engine::list::preview(m, PREVIEW_CHARS).into();
    open.previews.insert(m.id, (Arc::downgrade(m), line.clone()));
    line
}

/// The line between two parts of the column.
fn rule(ui: &mut Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment([pos2(r.min.x, r.center().y), pos2(r.max.x, r.center().y)], Stroke::new(1.0, t.border));
}
