// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared chat-style reading pane (#1907): chronological message bubbles (incoming on the left,
//! outgoing on the right), date separators, in-conversation search/filter, incremental history
//! loading, attachments, and per-message email actions. Designed to be shared by both
//! Contact-based and Project-based conversation views.

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Filter;
use mailcraft_engine::correspondents::{DEFAULT_CHAT_PAGE, is_outgoing, own_addresses, paginate_latest};
use mailcraft_engine::list::when;
use mailcraft_model::{Address, FlagStatus, FolderKind, Id, Message, format_list_capped};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// Maximum message bodies loaded from disk per frame while rendering a chat timeline.
const MAX_LOADS_PER_FRAME: usize = 4;
/// Collapsed height range for tall HTML bubbles (targeted at up to half the chat viewport).
const MIN_COLLAPSED_H: f32 = 240.0;
const MAX_COLLAPSED_H: f32 = 460.0;

/// Header metadata supplied by the active conversation mode (contact or project).
pub struct ChatHeader<'a> {
    pub key: &'a str,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub avatar_address: Option<&'a Address>,
    pub badge: Option<String>,
    pub total_messages: usize,
    pub unread_count: usize,
}

/// Ephemeral UI state for the shared chat-style reading pane.
#[derive(Default)]
pub struct ChatPaneState {
    pub active_key: String,
    pub shown_limit: usize,
    pub search_open: bool,
    pub search_query: String,
    pub filter: Filter,
    pub expanded: HashSet<Id>,
    pub scroll_to_bottom: bool,
}

impl ChatPaneState {
    pub fn reset_for(&mut self, key: &str) {
        if self.active_key != key {
            self.active_key = key.to_string();
            self.shown_limit = DEFAULT_CHAT_PAGE;
            self.expanded.clear();
            self.scroll_to_bottom = true;
        }
        if self.shown_limit == 0 {
            self.shown_limit = DEFAULT_CHAT_PAGE;
        }
    }
}

/// Render a shared chat-style conversation timeline in `ui`.
///
/// `message_ids` must be sorted chronologically oldest-first. `header_actions` allows the caller
/// (contact view or project view) to inject mode-specific buttons into the top-right header bar.
pub fn show_timeline(app: &mut MailApp, ui: &mut Ui, header: ChatHeader<'_>, message_ids: &[Id], header_actions: impl FnOnce(&mut MailApp, &mut Ui)) {
    app.chat_pane.reset_for(header.key);
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

    // ---- Top Conversation Header ----
    let header_resp = egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin { left: 16, right: 12, top: 10, bottom: 10 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
            let (avatar_rect, _) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::hover());
            if let Some(addr) = header.avatar_address {
                crate::sender_images::contact_avatar(app, ui, avatar_rect.center(), 18.0, addr);
            } else {
                ui.painter().circle_filled(avatar_rect.center(), 18.0, t.accent_soft);
                icons::paint(ui.painter(), Rect::from_center_size(avatar_rect.center(), vec2(18.0, 18.0)), "folder", t.accent_soft_text, t.accent);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                header_actions(app, ui);

                // In-conversation filter menu button.
                let active_filter = app.chat_pane.filter;
                let filter_btn = crate::widgets::icon_button(ui, "filter", tl!("Filter messages in conversation"), 16.0, true);
                egui::Popup::menu(&filter_btn).show(|ui| {
                    ui.set_min_width(150.0);
                    for f in [Filter::All, Filter::Unread, Filter::Attachments, Filter::Flagged] {
                        let mark = if f == active_filter { "✓ " } else { "   " };
                        if ui.button(format!("{mark}{}", tl!(f.label()))).clicked() {
                            app.chat_pane.filter = f;
                            ui.close();
                        }
                    }
                });

                // In-conversation search toggle.
                let search_tip = if app.chat_pane.search_open { tl!("Hide conversation search") } else { tl!("Search in conversation") };
                let search_btn = crate::widgets::icon_button(ui, "search", search_tip, 16.0, true);
                if search_btn.clicked() {
                    app.chat_pane.search_open = !app.chat_pane.search_open;
                    if !app.chat_pane.search_open {
                        app.chat_pane.search_query.clear();
                    }
                }

                // Title + subtitle take the remaining width.
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                        ui.add(egui::Label::new(egui::RichText::new(header.title).font(semibold(15.0)).color(t.text)).truncate());
                        if let Some(badge) = &header.badge {
                            let bw = crate::widgets::text_w(ui, badge, 10.5) + 12.0;
                            let (br, _) = ui.allocate_exact_size(vec2(bw, 18.0), Sense::hover());
                            ui.painter().rect_filled(br, 9.0, t.pill_quiet);
                            ui.painter().text(br.center(), Align2::CENTER_CENTER, badge, regular(10.5), t.text_dim);
                        }
                    });
                    let count_label = tln!(header.total_messages, "{n} message", "{n} messages");
                    let sub = if header.unread_count > 0 {
                        let unread_label = tln!(header.unread_count, "{n} unread", "{n} unread");
                        format!("{} · {count_label} · {unread_label}", header.subtitle)
                    } else if header.subtitle.is_empty() {
                        count_label
                    } else {
                        format!("{} · {count_label}", header.subtitle)
                    };
                    ui.add(egui::Label::new(egui::RichText::new(sub).font(regular(11.5)).color(t.text_dim)).truncate());
                });
            });
        });
    });
    let hr = header_resp.response.rect;
    ui.painter().line_segment([pos2(hr.min.x, hr.max.y), pos2(hr.max.x, hr.max.y)], Stroke::new(1.0, t.border));

    // ---- Optional In-Conversation Search & Filter Strip ----
    if app.chat_pane.search_open || app.chat_pane.filter != Filter::All {
        let strip = egui::Frame::NONE.fill(t.surface_sunken).inner_margin(egui::Margin { left: 16, right: 12, top: 6, bottom: 6 }).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                if app.chat_pane.search_open {
                    ui.add(
                        egui::TextEdit::singleline(&mut app.chat_pane.search_query).hint_text(tl!("Search in conversation…")).desired_width(210.0),
                    );
                    if !app.chat_pane.search_query.is_empty() && ui.button("✕").clicked() {
                        app.chat_pane.search_query.clear();
                    }
                }
                for f in [Filter::All, Filter::Unread, Filter::Attachments, Filter::Flagged] {
                    let on = app.chat_pane.filter == f;
                    let resp = ui.selectable_label(on, tl!(f.label()));
                    if resp.clicked() {
                        app.chat_pane.filter = f;
                    }
                }
            });
        });
        let sr = strip.response.rect;
        ui.painter().line_segment([pos2(sr.min.x, sr.max.y), pos2(sr.max.x, sr.max.y)], Stroke::new(1.0, t.border_faint));
    }

    let ctx = ui.ctx().clone();
    crate::reading::remote::tick(app, &ctx);

    // ---- Scrollable Chronological Feed ----
    let (visible_ids, older_remaining) = paginate_latest(message_ids, app.chat_pane.shown_limit, 0);
    let own = own_addresses(&app.session.mailbox);
    let room = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    let collapsed_max_h = (room.height() * 0.5).clamp(MIN_COLLAPSED_H, MAX_COLLAPSED_H);

    let mut any_bubble_or_control_hit = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(room), |ui| {
        egui::ScrollArea::vertical().id_salt(("chat_timeline", header.key)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 16 }).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 10.0);

                if older_remaining > 0 {
                    ui.vertical_centered(|ui| {
                        let label = tln!(older_remaining, "Load earlier messages ({n} more)", "Load earlier messages ({n} more)");
                        let btn = ui.button(label);
                        if ui.rect_contains_pointer(btn.rect) {
                            any_bubble_or_control_hit = true;
                        }
                        if btn.clicked() {
                            app.chat_pane.shown_limit = app.chat_pane.shown_limit.saturating_add(DEFAULT_CHAT_PAGE);
                        }
                    });
                }

                if visible_ids.is_empty() {
                    ui.add_space(36.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(tl!("No messages match this view.")).font(regular(13.0)).color(t.text_dim));
                    });
                    return;
                }

                let mut last_date = None;
                let mut loads_left = MAX_LOADS_PER_FRAME;
                let avail_w = ui.available_width().max(260.0);

                for &id in visible_ids {
                    if loads_left > 0 && app.session.needs_load_for_display(id) {
                        let _ = app.session.load_for_display(id);
                        loads_left = loads_left.saturating_sub(1);
                    }
                    let Some(m) = app.session.mailbox.messages.get(&id).cloned() else { continue };
                    let local_dt = app.session.local(when(&m));
                    if last_date != Some(local_dt.date) {
                        date_separator(ui, &crate::dates::medium(local_dt.date), &t);
                        last_date = Some(local_dt.date);
                    }
                    let outgoing = is_outgoing(&app.session.mailbox, &m, &own);
                    let focused = app.session.sel.focus() == Some(m.id);
                    let time_str = crate::dates::clock(local_dt, app.session.prefs.twelve_hour);
                    if message_bubble(app, ui, header.key, &m, outgoing, focused, &time_str, avail_w, collapsed_max_h, &t) {
                        any_bubble_or_control_hit = true;
                    }
                }

                if std::mem::take(&mut app.chat_pane.scroll_to_bottom) {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                }
            });
        });
    });

    // Clicking blank space in the right-hand reading pane clears bubble selection.
    let blank_click = !any_bubble_or_control_hit
        && !app.session.sel.messages.is_empty()
        && !egui::Popup::is_any_open(ui.ctx())
        && ui.rect_contains_pointer(room)
        && ui.input(|i| {
            i.pointer.primary_clicked() && i.pointer.press_origin().zip(i.pointer.interact_pos()).is_none_or(|(a, b)| a.distance(b) < 5.0)
        });
    if blank_click {
        let _ = app.run("mail.select", json!({"ids": []}));
    }
}

fn date_separator(ui: &mut Ui, label: &str, t: &Tokens) {
    ui.vertical_centered(|ui| {
        let w = crate::widgets::text_w(ui, label, 11.0) + 18.0;
        let (r, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
        ui.painter().rect(r, CornerRadius::same(11), t.panel, Stroke::new(1.0, t.border_faint), egui::StrokeKind::Inside);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, regular(11.0), t.text_dim);
    });
}

#[allow(clippy::too_many_arguments)]
fn message_bubble(
    app: &mut MailApp,
    ui: &mut Ui,
    conv_key: &str,
    m: &Arc<Message>,
    outgoing: bool,
    focused: bool,
    time_str: &str,
    avail_w: f32,
    collapsed_max_h: f32,
    t: &Tokens,
) -> bool {
    // Keep side breathing room on the opposite edge so bubbles clearly sit left vs. right, while
    // expanding up to content width for rich HTML emails and tables.
    let max_bubble_w = (avail_w - 84.0).max(220.0);
    let card_fill = if outgoing { t.selection_pill } else { t.panel };
    let card_stroke = if !outgoing && !m.read {
        Stroke::new(1.4, t.accent)
    } else if focused {
        Stroke::new(1.3, t.selection_edge)
    } else {
        Stroke::new(1.0, t.border)
    };
    let mut action_clicked = false;
    let bubble_rect = if outgoing {
        let wrap_width = (max_bubble_w - 24.0).max(180.0);
        let resp = ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.set_max_width(avail_w);
            egui::Frame::NONE.fill(card_fill).stroke(card_stroke).corner_radius(10.0).inner_margin(egui::Margin::symmetric(12, 9)).show(ui, |ui| {
                ui.set_max_width(max_bubble_w);
                action_clicked = bubble_contents(app, ui, m, true, time_str, wrap_width, collapsed_max_h, card_fill, t);
            })
        });
        resp.inner.response.rect
    } else {
        let inner_max = (max_bubble_w - 36.0).max(200.0);
        let wrap_width = (inner_max - 24.0).max(170.0);
        let resp = ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
            let (av_rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
            crate::sender_images::avatar(app, ui, av_rect.center(), 14.0, m, &m.from);
            egui::Frame::NONE.fill(card_fill).stroke(card_stroke).corner_radius(10.0).inner_margin(egui::Margin::symmetric(12, 9)).show(ui, |ui| {
                ui.set_max_width(inner_max);
                action_clicked = bubble_contents(app, ui, m, false, time_str, wrap_width, collapsed_max_h, card_fill, t);
            })
        });
        resp.inner.response.rect
    };

    let visible_bubble = bubble_rect.intersect(ui.clip_rect());
    let pointer_in_bubble = visible_bubble.width() > 2.0 && visible_bubble.height() > 2.0 && ui.rect_contains_pointer(visible_bubble);
    if visible_bubble.width() > 2.0 && visible_bubble.height() > 2.0 {
        app.hit.add("chatBubble", m.id, visible_bubble);
    }

    // Clicking anywhere on the visible bubble (outside explicit buttons/links/drag-selection)
    // focuses it and marks unread messages as read when `mark_read_on_open` is enabled.
    let plain_click = !action_clicked
        && !egui::Popup::is_any_open(ui.ctx())
        && pointer_in_bubble
        && ui.input(|i| {
            i.pointer.primary_clicked() && i.pointer.press_origin().zip(i.pointer.interact_pos()).is_none_or(|(a, b)| a.distance(b) < 5.0)
        });
    if plain_click {
        let _ = app.run("mail.selectCorrespondent", json!({"key": conv_key, "messageId": m.id}));
    }
    pointer_in_bubble
}

#[allow(clippy::too_many_arguments)]
fn bubble_contents(
    app: &mut MailApp,
    ui: &mut Ui,
    m: &Arc<Message>,
    outgoing: bool,
    time_str: &str,
    wrap_width: f32,
    collapsed_max_h: f32,
    card_fill: Color32,
    t: &Tokens,
) -> bool {
    let mut action_clicked = false;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 4.0);

        // ---- Sender / Status / Time row ----
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            let sender_label = if outgoing { tl!("You").to_string() } else { m.from.display().to_string() };
            ui.label(egui::RichText::new(sender_label).font(semibold(12.0)).color(t.text));

            if m.draft {
                ui.label(egui::RichText::new(tl!("Draft")).font(regular(10.5)).color(t.orange));
            } else if app.session.mailbox.folder(m.folder).is_some_and(|f| f.kind == FolderKind::Outbox) {
                ui.label(egui::RichText::new(tl!("Outbox")).font(regular(10.5)).color(t.orange));
            }

            ui.label(egui::RichText::new(time_str).font(regular(11.0)).color(t.text_dim));

            if !m.read {
                let (dr, _) = ui.allocate_exact_size(vec2(8.0, 12.0), Sense::hover());
                ui.painter().circle_filled(dr.center(), 3.5, t.accent);
            }
            if m.flag == FlagStatus::Flagged {
                let (fr, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), fr, "flag", t.icon_red, t.icon_red);
            }
        });

        // ---- Subject (always preserved across threads) ----
        let subject = if m.subject.trim().is_empty() { tl!("(no subject)") } else { m.subject.as_str() };
        ui.add(egui::Label::new(egui::RichText::new(subject).font(semibold(13.0)).color(t.text)).wrap());

        // ---- Recipients (To / Cc / Bcc) ----
        recipient_line(ui, tl!("To: {names}"), &m.to, t.text_dim);
        recipient_line(ui, tl!("Cc: {names}"), &m.cc, t.text);
        recipient_line(ui, tl!("Bcc: {names}"), &m.bcc, t.text_dim);

        // ---- HTML / Plain-text / Inline & Remote Images Body ----
        let expanded = app.chat_pane.expanded.contains(&m.id);
        let body = crate::reading::show_chat_body(app, ui, m, wrap_width, collapsed_max_h, expanded, card_fill);
        if body.block_click {
            action_clicked = true;
        }
        if body.has_body {
            if body.truncated || expanded {
                let link_label = if expanded { tl!("Show less") } else { tl!("Show more") };
                if ui.link(egui::RichText::new(link_label).font(regular(11.5))).clicked() {
                    action_clicked = true;
                    if expanded {
                        app.chat_pane.expanded.remove(&m.id);
                    } else {
                        app.chat_pane.expanded.insert(m.id);
                    }
                }
            }
        } else if m.unfetched {
            ui.label(egui::RichText::new(crate::reading::unfetched_said(app, m)).font(regular(11.5)).color(t.text_dim));
        }

        // ---- Attachments (pictures as tiles, other files as chips) ----
        let att_scope = ui.scope(|ui| {
            crate::reading::attachments(app, ui, m, crate::reading::Place::Chat(m.id), t);
        });
        if ui.rect_contains_pointer(att_scope.response.rect) && ui.input(|i| i.pointer.primary_clicked() || i.pointer.secondary_clicked()) {
            action_clicked = true;
        }

        // ---- Per-message Action Bar ----
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
            let add_visible_hit = |app: &mut MailApp, ui: &Ui, kind: &'static str, rect: Rect| {
                let vis = rect.intersect(ui.clip_rect());
                if vis.width() > 1.0 && vis.height() > 1.0 {
                    app.hit.add(kind, m.id, vis);
                }
            };
            let mut action = |icon: &str, label: &str, hit_kind: &'static str, tip: Option<&str>, cmd: &str, params: serde_json::Value| {
                let mut r = crate::widgets::icon_text_button(ui, icon, tl!(label), true);
                if let Some(tip) = tip {
                    r = r.on_hover_text(tip);
                }
                add_visible_hit(app, ui, hit_kind, r.rect);
                if r.clicked() {
                    action_clicked = true;
                    let _ = app.run(cmd, params);
                }
            };
            action("reply", "Reply", "chatReply", None, "mail.reply", json!({"id": m.id}));
            let reply_all_tip = (!m.cc.is_empty() || m.to.len() > 1).then(|| tl!("Reply to sender and all recipients"));
            action("replyAll", "Reply All", "chatReplyAll", reply_all_tip, "mail.replyAll", json!({"id": m.id}));
            action("forward", "Forward", "chatForward", None, "mail.forward", json!({"id": m.id}));
            let read_tip = if m.read { tl!("Mark as Unread") } else { tl!("Mark as Read") };
            let read_btn = crate::widgets::icon_button(ui, if m.read { "unread" } else { "read" }, read_tip, 14.0, true);
            add_visible_hit(app, ui, "chatToggleRead", read_btn.rect);
            if read_btn.clicked() {
                action_clicked = true;
                let _ = app.run("mail.toggleRead", json!({"ids": [m.id]}));
            }
            let open_btn = crate::widgets::icon_button(ui, "popOut", tl!("Open in Window"), 14.0, true);
            add_visible_hit(app, ui, "chatOpen", open_btn.rect);
            if open_btn.clicked() {
                action_clicked = true;
                let _ = app.run("ui.openWindow", json!({"message": m.id}));
            }
            let del_btn = crate::widgets::icon_button(ui, "delete", tl!("Delete"), 14.0, true);
            add_visible_hit(app, ui, "chatDelete", del_btn.rect);
            if del_btn.clicked() {
                action_clicked = true;
                let _ = app.run("mail.delete", json!({"ids": [m.id]}));
            }
        });
    });
    action_clicked
}

fn recipient_line(ui: &mut Ui, template: &str, addrs: &[Address], color: Color32) {
    if addrs.is_empty() {
        return;
    }
    let (mut names, more) = format_list_capped(addrs, 4);
    if more > 0 {
        names.push_str(&format!("; {}", tln!(more, "and {n} more", "and {n} more")));
    }
    let text = crate::i18n::fmt(template, &[("names", &names)]);
    let resp = ui.add(egui::Label::new(egui::RichText::new(text).font(regular(11.0)).color(color)).wrap());
    let (mut full_names, full_more) = format_list_capped(addrs, 30);
    if full_more > 0 {
        full_names.push_str(&format!("; {}", tln!(full_more, "and {n} more", "and {n} more")));
    }
    let full = addrs.iter().take(12).map(|a| a.full()).collect::<Vec<_>>().join(", ");
    let tip = if addrs.len() > 12 { format!("{full}\n({full_names})") } else { full };
    resp.on_hover_text(tip);
}
