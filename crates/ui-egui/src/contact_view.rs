// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contact-based conversation view (#1907): simplified list of correspondents (one row per person,
//! sorted by latest communication time, with unread count badges) paired with the shared chat-like
//! reading pane and a modal dialog for merging/splitting correspondent email addresses.

use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Filter;
use mailcraft_engine::correspondents::{CorrespondentSummary, messages_for, summaries};
use mailcraft_model::Address;
use mailcraft_model::correspondents::normalize_address;
use serde_json::json;

use crate::chat_view::{ChatHeader, show_timeline};
use crate::dialogs::button_row;
use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::widgets::{mi, mi_check, overlay_scroll_style, plain_button, primary_button};
use crate::{MailApp, icons};

const HEADER_H: f32 = 48.0;
const ROW_H: f32 = 68.0;
const AVATAR_R: f32 = 18.0;
pub const MERGE_DIALOG_WIDTH: f32 = 480.0;

/// Dialog state for merging multiple email addresses into one correspondent conversation.
pub struct MergeDialog {
    pub primary_key: String,
    pub existing_addresses: Vec<String>,
    pub manual_addresses: std::collections::HashSet<String>,
    pub candidates: Vec<(String, String)>,
    pub selected_candidate: Option<String>,
    pub custom_address: String,
    pub custom_name: String,
    pub error: String,
}

impl MergeDialog {
    pub fn open(app: &MailApp, key: &str, name: &str, addresses: &[String]) -> Self {
        let all = summaries(app_session(app), app.session.view.contact_all_accounts, Filter::All, "");
        let existing_set: std::collections::HashSet<&str> = addresses.iter().map(String::as_str).collect();
        let candidates: Vec<(String, String)> = all.into_iter().filter(|c| !existing_set.contains(c.key.as_str())).map(|c| (c.key, c.name)).collect();
        let selected_candidate = candidates.first().map(|(k, _)| k.clone());
        MergeDialog {
            primary_key: key.to_string(),
            existing_addresses: addresses.to_vec(),
            manual_addresses: manual_address_set(&app.session.mailbox),
            candidates,
            selected_candidate,
            custom_address: String::new(),
            custom_name: name.to_string(),
            error: String::new(),
        }
    }

    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut done = false;
        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);

        ui.label(
            egui::RichText::new(tl!("Combine multiple email addresses belonging to the same person into one conversation."))
                .font(regular(12.5))
                .color(t.text_dim),
        );

        // Display name field.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Display name:")).font(regular(12.5)).color(t.text));
            ui.add(egui::TextEdit::singleline(&mut self.custom_name).desired_width(280.0));
        });

        // Existing addresses in this correspondent group.
        ui.label(egui::RichText::new(tl!("Current addresses in this conversation:")).font(semibold(12.5)).color(t.text));
        let can_unmerge = self.existing_addresses.len() > 1;
        let mut to_unmerge: Option<String> = None;
        egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).corner_radius(6.0).inner_margin(egui::Margin::symmetric(10, 8)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                for addr in &self.existing_addresses {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(addr).font(regular(12.0)).color(t.text));
                        if can_unmerge && self.manual_addresses.contains(addr) {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(tl!("Remove")).clicked() {
                                    to_unmerge = Some(addr.clone());
                                }
                            });
                        }
                    });
                }
            },
        );
        if let Some(addr) = to_unmerge {
            match app.execute("mail.unmergeCorrespondent", json!({"address": addr})) {
                Err(err) => self.error = app.refusal_said(&err),
                Ok(val) => {
                    if val.get("changed").and_then(|v| v.as_bool()).unwrap_or(false) {
                        self.existing_addresses.retain(|a| a != &addr);
                        self.manual_addresses = manual_address_set(&app.session.mailbox);
                        if let Some(first) = self.existing_addresses.first() {
                            self.primary_key = first.clone();
                        }
                        self.error.clear();
                    } else {
                        self.error = tl!("This address is linked by a Contact card rather than a manual merge.").to_string();
                    }
                }
            }
        }

        // Pick an existing correspondent to merge with.
        if !self.candidates.is_empty() {
            ui.label(egui::RichText::new(tl!("Merge with another correspondent:")).font(semibold(12.5)).color(t.text));
            let current_label = self
                .selected_candidate
                .as_deref()
                .and_then(|k| self.candidates.iter().find(|(ck, _)| ck == k))
                .map(|(k, n)| if n == k { k.clone() } else { format!("{n} <{k}>") })
                .unwrap_or_else(|| tl!("Select correspondent…").to_string());
            egui::ComboBox::from_id_salt("merge_correspondent_picker").selected_text(current_label).width(ui.available_width()).show_ui(ui, |ui| {
                if ui.selectable_label(self.selected_candidate.is_none(), tl!("(Enter address manually below)")).clicked() {
                    self.selected_candidate = None;
                }
                for (k, n) in &self.candidates {
                    let text = if n == k { k.clone() } else { format!("{n} <{k}>") };
                    if ui.selectable_label(self.selected_candidate.as_deref() == Some(k.as_str()), text).clicked() {
                        self.selected_candidate = Some(k.clone());
                    }
                }
            });
        }

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Or add email address:")).font(regular(12.5)).color(t.text));
            ui.add(egui::TextEdit::singleline(&mut self.custom_address).hint_text("name@example.com").desired_width(240.0));
        });

        if !self.error.is_empty() {
            ui.label(egui::RichText::new(&self.error).font(regular(12.0)).color(t.red));
        }

        let extra = if !self.custom_address.trim().is_empty() { normalize_address(&self.custom_address) } else { self.selected_candidate.clone() };
        let can_merge = extra.as_ref().is_some_and(|e| !self.existing_addresses.contains(e));

        button_row(ui, |ui| {
            if plain_button(ui, tl!("Cancel")).clicked() {
                done = true;
            }
            if (primary_button(ui, tl!("Merge"), can_merge).clicked() || (enter && can_merge))
                && let Some(other) = extra
            {
                let mut addresses = self.existing_addresses.clone();
                addresses.push(other);
                let name = self.custom_name.trim();
                let params = if name.is_empty() { json!({"addresses": addresses}) } else { json!({"addresses": addresses, "name": name}) };
                match app.execute("mail.mergeCorrespondents", params) {
                    Ok(_) => done = true,
                    Err(err) => self.error = app.refusal_said(&err),
                }
            }
        });
        done
    }
}

fn manual_address_set(mb: &mailcraft_model::Mailbox) -> std::collections::HashSet<String> {
    mb.correspondent_aliases.iter().flat_map(|g| g.addresses.iter().filter_map(|a| normalize_address(a))).collect()
}

fn app_session(app: &MailApp) -> &mailcraft_engine::Session {
    &app.session
}

/// Render the Contact-based Conversation List panel.
pub fn show_list(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

    let all_accounts = app.session.view.contact_all_accounts;
    let list = summaries(&app.session, all_accounts, app.session.view.filter, &app.session.view.search);

    // Ensure a valid correspondent and aligned message selection when available; clear stale selection.
    if list.is_empty() {
        app.session.view.selected_correspondent = None;
        app.session.view.selected_correspondent_all_accounts = None;
        app.session.sel.clear_messages();
    } else {
        let valid = app.session.view.selected_correspondent.as_deref().is_some_and(|k| list.iter().any(|c| c.key == k));
        if !valid {
            let next_key = list[0].key.clone();
            let (_, ids) = messages_for(&app.session, &next_key, all_accounts, Filter::All, "");
            app.session.view.selected_correspondent = Some(next_key);
            app.session.view.selected_correspondent_all_accounts = Some(all_accounts);
            if app.session.sel.focus().is_none_or(|fid| !ids.contains(&fid)) {
                if let Some(&latest) = ids.last() {
                    app.session.sel.messages = vec![latest];
                    app.session.sel.whole.clear();
                    app.session.sel.focus_row = None;
                } else {
                    app.session.sel.clear_messages();
                }
            }
        }
    }

    list_header(app, ui, list.len());

    if !app.session.view.search.trim().is_empty() {
        let count_text = tln!(list.len(), "{n} conversation matching search", "{n} conversations matching search");
        let (sr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
        ui.painter().rect_filled(sr.shrink2(vec2(6.0, 2.0)), 4.0, t.selection);
        let close = Rect::from_center_size(pos2(sr.max.x - 20.0, sr.center().y), vec2(20.0, 20.0));
        paint_clipped(ui.painter(), pos2(sr.min.x + 14.0, sr.center().y), close.min.x - sr.min.x - 20.0, &count_text, semibold(11.5), t.text);
        let x = ui.interact(close, ui.scope_id().with("contact_search_close"), Sense::click()).on_hover_text(tl!("Clear search"));
        if x.hovered() {
            ui.painter().rect_filled(close, 4.0, t.hover);
        }
        let c = close.center();
        for (a, b) in [(vec2(-4.0, -4.0), vec2(4.0, 4.0)), (vec2(-4.0, 4.0), vec2(4.0, -4.0))] {
            ui.painter().line_segment([c + a, c + b], Stroke::new(1.3, t.text_dim));
        }
        if x.clicked() {
            app.search_text.clear();
            let _ = app.run("view.clearSearch", json!({}));
        }
    }

    if list.is_empty() {
        let r = ui.available_rect_before_wrap();
        ui.expand_to_include_rect(r);
        let msg = if !app.session.view.search.trim().is_empty() {
            tl!("Nothing matches the search.")
        } else if app.session.view.filter != Filter::All {
            tl!("Nothing matches this filter.")
        } else {
            tl!("No conversations yet.")
        };
        ui.painter().text(pos2(r.center().x, r.min.y + 60.0), Align2::CENTER_CENTER, msg, regular(12.5), t.text_dim);
        return;
    }

    let selected_key = app.session.view.selected_correspondent.clone();
    let plain = ui.style().clone();
    if app.overlay_scroll_bars {
        overlay_scroll_style(ui, &t);
    }
    egui::ScrollArea::vertical().id_salt("contact_conversation_list").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        ui.set_style(plain);
        let total_h = ROW_H * list.len() as f32;
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h), Sense::hover());
        let first = ((viewport.min.y / ROW_H).floor().max(0.0) as usize).min(list.len());
        let last = (((viewport.max.y / ROW_H).ceil().max(0.0) as usize) + 1).min(list.len());

        for (idx, item) in list.iter().enumerate().take(last).skip(first) {
            let rect = Rect::from_min_size(pos2(area.min.x, area.min.y + idx as f32 * ROW_H), vec2(area.width(), ROW_H));
            let is_selected = selected_key.as_deref() == Some(item.key.as_str());
            correspondent_row(app, ui, item, rect, is_selected, &t);
        }
    });
}

fn list_header(app: &mut MailApp, ui: &mut Ui, count: usize) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    ui.painter().line_segment([pos2(r.min.x, r.max.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, t.border_faint));

    // Menu button on the right.
    let menu_rect = Rect::from_center_size(pos2(r.max.x - 20.0, r.center().y), vec2(26.0, 26.0));
    let menu_resp = ui.interact(menu_rect, ui.scope_id().with("contact_list_menu"), Sense::click()).on_hover_text(tl!("Conversation options"));
    if menu_resp.hovered() {
        ui.painter().rect_filled(menu_rect, 4.0, t.hover);
    }
    icons::paint(ui.painter(), Rect::from_center_size(menu_rect.center(), vec2(16.0, 16.0)), "more", t.icon, t.accent);

    let all_accounts = app.session.view.contact_all_accounts;
    let desc = app.session.view.sort_descending;
    let filter = app.session.view.filter;
    egui::Popup::menu(&menu_resp).show(|ui| {
        ui.set_min_width(185.0);
        if app.session.mailbox.accounts.len() > 1 {
            mi_check(ui, app, "All Accounts", all_accounts, "view.contactConversations", json!({"allAccounts": !all_accounts}));
            ui.separator();
        }
        mi(ui, app, "Reverse Sort", "view.reverseSort", json!({}));
        let sort_note = if desc { tl!("Newest on top") } else { tl!("Oldest on top") };
        ui.label(egui::RichText::new(format!("   {sort_note}")).font(regular(11.0)).color(t.text_dim));
        ui.separator();
        mi_check(ui, app, "Group by Address", true, "view.contactConversations", json!({"value": false}));
        mi_check(ui, app, "Show as Conversations", false, "view.conversations", json!({"value": true}));
        ui.separator();
        for f in Filter::ALL {
            mi_check(ui, app, f.label(), f == filter, "view.filter", json!({"value": f}));
        }
    });

    // Left: All / Unread tabs + count.
    let mut x = r.min.x + 16.0;
    for f in [Filter::All, Filter::Unread] {
        let on = filter == f;
        let label = tl!(f.label());
        let w = crate::widgets::text_w(ui, label, 14.5) + 4.0;
        let tab = Rect::from_min_max(pos2(x, r.min.y + 12.0), pos2(x + w, r.max.y));
        let resp = ui.interact(tab, ui.scope_id().with(("contact_tab", f)), Sense::click());
        paint_clipped(ui.painter(), pos2(x + 2.0, r.center().y + 2.0), w, label, regular(14.5), if on { t.accent_text } else { t.text });
        let under = Rect::from_min_max(pos2(tab.min.x, r.max.y - 2.0), tab.max);
        if on {
            ui.painter().rect_filled(under, 0.0, t.accent_text);
        } else if resp.hovered() {
            ui.painter().rect_filled(under, 0.0, t.border_strong);
        }
        if resp.clicked() && !on {
            let _ = app.run("view.filter", json!({"value": f}));
        }
        x = tab.max.x + 14.0;
    }

    let badge = tln!(count, "{n} contact", "{n} contacts");
    let badge_w = crate::widgets::text_w(ui, &badge, 11.5);
    let max_badge_room = (menu_rect.min.x - x - 14.0).max(0.0);
    if max_badge_room >= 24.0 {
        let shown_w = badge_w.min(max_badge_room);
        paint_clipped(ui.painter(), pos2(menu_rect.min.x - 8.0 - shown_w, r.center().y + 2.0), shown_w, &badge, regular(11.5), t.text_dim);
    }
}

fn correspondent_row(app: &mut MailApp, ui: &mut Ui, item: &CorrespondentSummary, rect: Rect, selected: bool, t: &Tokens) {
    let resp = ui.interact(rect, ui.make_persistent_id(("correspondent_row", &item.key)), Sense::click());
    app.hit.add("correspondentRow", crate::ribbon::hit_id(&item.key), rect);

    let bg = if selected {
        t.selection
    } else if resp.hovered() {
        t.hover_subtle
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect.shrink2(vec2(2.0, 1.0)), 4.0, bg);
    }
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_size(pos2(rect.min.x + 2.0, rect.min.y + 8.0), vec2(3.0, rect.height() - 16.0)),
            1.5,
            t.selection_edge,
        );
    }
    ui.painter().line_segment([pos2(rect.min.x + 62.0, rect.max.y), pos2(rect.max.x - 10.0, rect.max.y)], Stroke::new(1.0, t.border_faint));

    // Avatar.
    let av_center = pos2(rect.min.x + 32.0, rect.center().y);
    let av_addr = Address::new(&item.name, &item.primary_email);
    crate::sender_images::contact_avatar(app, ui, av_center, AVATAR_R, &av_addr);

    // Top row: Name (+ merged pill) on left, timestamp on right.
    let text_x = rect.min.x + 60.0;
    let time_text =
        crate::dates::list_label(item.latest_when, app.session.now(), app.session.offset_at(item.latest_when), app.session.prefs.twelve_hour);
    let time_w = crate::widgets::text_w(ui, &time_text, 11.0);
    let time_color = if item.unread_count > 0 { t.accent_text } else { t.text_dim };
    ui.painter().text(pos2(rect.max.x - 12.0, rect.min.y + 22.0), Align2::RIGHT_CENTER, &time_text, regular(11.0), time_color);

    let name_font = if item.unread_count > 0 { semibold(14.0) } else { regular(14.0) };
    let merged_w = if item.addresses.len() > 1 { 26.0 } else { 0.0 };
    let name_room = (rect.max.x - 18.0 - time_w - text_x - merged_w).max(40.0);
    paint_clipped(ui.painter(), pos2(text_x, rect.min.y + 22.0), name_room, &item.name, name_font, t.text);

    if item.addresses.len() > 1 {
        let name_actual = crate::widgets::text_w(ui, &item.name, 14.0).min(name_room);
        let pill = Rect::from_min_size(pos2(text_x + name_actual + 6.0, rect.min.y + 14.0), vec2(20.0, 16.0));
        ui.painter().rect_filled(pill, 8.0, t.pill_quiet);
        ui.painter().text(pill.center(), Align2::CENTER_CENTER, item.addresses.len().to_string(), regular(10.0), t.text_dim);
    }

    // Bottom row: latest snippet (subject + preview, "You: " if outgoing) + unread badge pill on right.
    let badge_w = if item.unread_count > 0 {
        let badge = if item.unread_count > 99 { "99+".to_string() } else { item.unread_count.to_string() };
        let bw = (crate::widgets::text_w(ui, &badge, 10.5) + 12.0).max(20.0);
        let br = Rect::from_min_size(pos2(rect.max.x - 12.0 - bw, rect.min.y + 37.0), vec2(bw, 18.0));
        ui.painter().rect_filled(br, 9.0, t.accent);
        ui.painter().text(br.center(), Align2::CENTER_CENTER, &badge, semibold(10.5), t.on_accent);
        bw + 8.0
    } else {
        0.0
    };

    let subj = if item.latest_subject.trim().is_empty() { tl!("(no subject)") } else { item.latest_subject.trim() };
    let snippet = if item.latest_outgoing {
        if item.latest_preview.is_empty() { format!("{}: {subj}", tl!("You")) } else { format!("{}: {subj} — {}", tl!("You"), item.latest_preview) }
    } else if item.latest_preview.is_empty() {
        subj.to_string()
    } else {
        format!("{subj} — {}", item.latest_preview)
    };
    let snippet_room = (rect.max.x - 14.0 - badge_w - text_x).max(40.0);
    paint_clipped(ui.painter(), pos2(text_x, rect.min.y + 46.0), snippet_room, &snippet, regular(12.0), t.text_dim);

    if resp.clicked() {
        let _ = app.run("mail.selectCorrespondent", json!({"key": item.key}));
        app.chat_pane.scroll_to_bottom = true;
    }

    let key = item.key.clone();
    let name = item.name.clone();
    let primary_email = item.primary_email.clone();
    let addresses = item.addresses.clone();
    let manually_merged = item.manually_merged;
    resp.context_menu(|ui| {
        ui.set_min_width(190.0);
        if ui.button(tl!("New Email to Contact")).clicked() {
            let to = Address::new(&name, &primary_email).full();
            let _ = app.run("mail.new", json!({"to": to}));
            ui.close();
        }
        if ui.button(tl!("Merge with Another Address…")).clicked() {
            app.dialog = Some(crate::dialogs::Dialog::MergeCorrespondents(Box::new(MergeDialog::open(app, &key, &name, &addresses))));
            ui.close();
        }
        if manually_merged && addresses.len() > 1 {
            ui.menu_button(tl!("Split Merged Address"), |ui| {
                for addr in &addresses {
                    if ui.button(addr).clicked() {
                        let _ = app.run("mail.unmergeCorrespondent", json!({"address": addr}));
                        ui.close();
                    }
                }
            });
        }
        let (_, all_ids) = messages_for(&app.session, &key, app.session.view.contact_all_accounts, Filter::Unread, "");
        if !all_ids.is_empty() && ui.button(tl!("Mark All as Read")).clicked() {
            let _ = app.run("mail.markRead", json!({"ids": all_ids}));
            ui.close();
        }
    });
}

/// Render the Contact-based Chat Reading Pane in `CentralPanel`.
pub fn show_reading(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let key = app.session.view.selected_correspondent.clone().or_else(|| {
        summaries(&app.session, app.session.view.contact_all_accounts, app.session.view.filter, &app.session.view.search)
            .first()
            .map(|c| c.key.clone())
    });

    let Some(key) = key else {
        let r = ui.available_rect_before_wrap();
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(r.center().x, r.min.y + r.height() * 0.38), vec2(56.0, 56.0)),
            "people",
            t.border_strong,
            t.border_strong,
        );
        ui.painter().text(
            pos2(r.center().x, r.min.y + r.height() * 0.38 + 50.0),
            Align2::CENTER_CENTER,
            tl!("Select a correspondent to read"),
            regular(13.0),
            t.text_dim,
        );
        return;
    };

    let all_accounts = app.session.view.contact_all_accounts;
    let in_filter = app.chat_pane.filter;
    let in_query = app.chat_pane.search_query.clone();
    let (summary, message_ids) = messages_for(&app.session, &key, all_accounts, in_filter, &in_query);
    let Some(summary) = summary else {
        let r = ui.available_rect_before_wrap();
        ui.painter().text(
            pos2(r.center().x, r.min.y + r.height() * 0.4),
            Align2::CENTER_CENTER,
            tl!("No messages in this conversation"),
            regular(13.0),
            t.text_dim,
        );
        return;
    };

    let avatar_addr = Address::new(&summary.name, &summary.primary_email);
    let subtitle = if summary.addresses.len() > 1 { summary.addresses.join(", ") } else { summary.primary_email.clone() };
    let badge = (summary.addresses.len() > 1).then(|| tln!(summary.addresses.len(), "{n} address", "{n} addresses"));

    let header = ChatHeader {
        key: &summary.key,
        title: &summary.name,
        subtitle: &subtitle,
        avatar_address: Some(&avatar_addr),
        badge,
        total_messages: summary.message_count,
        unread_count: summary.unread_count,
    };

    let (_, unread_ids) = messages_for(&app.session, &summary.key, all_accounts, Filter::Unread, "");
    let sum_key = summary.key.clone();
    let sum_name = summary.name.clone();
    let sum_email = summary.primary_email.clone();
    let sum_addrs = summary.addresses.clone();

    show_timeline(app, ui, header, &message_ids, move |app, ui| {
        if !unread_ids.is_empty() {
            let mark_btn = crate::widgets::icon_text_button(ui, "markAllRead", tl!("Mark All Read"), true);
            app.hit.add("contactMarkAllRead", 0, mark_btn.rect);
            if mark_btn.clicked() {
                let _ = app.run("mail.markRead", json!({"ids": unread_ids}));
            }
        }
        let merge_btn = crate::widgets::icon_button(ui, "mergeContacts", tl!("Merge addresses…"), 16.0, true);
        app.hit.add("contactMerge", 0, merge_btn.rect);
        if merge_btn.clicked() {
            app.dialog = Some(crate::dialogs::Dialog::MergeCorrespondents(Box::new(MergeDialog::open(app, &sum_key, &sum_name, &sum_addrs))));
        }
        let new_btn = crate::widgets::icon_button(ui, "newMail", tl!("New email to contact"), 16.0, true);
        app.hit.add("contactNewMail", 0, new_btn.rect);
        if new_btn.clicked() {
            let to = Address::new(&sum_name, &sum_email).full();
            let _ = app.run("mail.new", json!({"to": to}));
        }
    });
}
