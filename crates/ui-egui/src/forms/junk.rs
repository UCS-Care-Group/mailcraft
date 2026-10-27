// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Junk E-mail Options (#504): the Safe Senders and Blocked Senders lists, a page each, with
//! the list, Add and Remove. Every change is a command (`junk.add`, `junk.remove`). The lists are
//! each account's (#516): with more than one account, a drop-down says whose are in view.

use egui::{Align2, CornerRadius, Sense, Stroke, StrokeKind, Ui, vec2};
use mailcraft_engine::Session;
use mailcraft_model::{Id, JunkList};
use serde_json::{Value, json};

use super::kit::{ROW_H, body, close_button, error, field_id, shown};
use crate::MailApp;
use crate::theme::{Tokens, regular};

/// The list's height in the dialog, in points.
const LIST_H: f32 = 220.0;
/// The form's width ([`super::Form::width`]) and its buttons', in points.
pub const WIDTH: f32 = 520.0;
const BUTTON_W: f32 = 88.0;

/// The Junk E-mail Options dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JunkForm {
    /// The page in view.
    pub page: JunkList,
    /// The account whose lists are in view (0 when the mailbox has none).
    pub account: Id,
    /// The entry selected on the page in view.
    pub selected: Option<String>,
    /// What is typed to be added.
    pub new_entry: String,
    error: String,
    focus: bool,
}

impl Default for JunkForm {
    fn default() -> Self {
        JunkForm { page: JunkList::Safe, account: 0, selected: None, new_entry: String::new(), error: String::new(), focus: true }
    }
}

/// What a page says it is for.
fn about(page: JunkList) -> &'static str {
    match page {
        JunkList::Safe => {
            "Email from addresses or domain names on your Safe Senders list will never be treated as junk email. A domain (@example.org) is that domain only, not its subdomains."
        }
        JunkList::Blocked => {
            "Email from addresses or domain names on your Blocked Senders list will go to Junk Email as it arrives. A domain (@example.org) is that domain only, not its subdomains."
        }
    }
}

fn title(page: JunkList) -> &'static str {
    match page {
        JunkList::Safe => "Safe Senders",
        JunkList::Blocked => "Blocked Senders",
    }
}

impl JunkForm {
    /// The dialog, on `params.list`'s page (Safe Senders when it doesn't say).
    /// The account is `params.account`, else the selected message's, else the first.
    pub fn new(s: &Session, params: &Value) -> JunkForm {
        let page = params.get("list").and_then(Value::as_str).and_then(JunkList::parse).unwrap_or(JunkList::Safe);
        let own = |id: Id| s.mailbox.account(id).filter(|a| a.shared_of.is_none()).map(|a| a.id);
        let account = params
            .get("account")
            .and_then(Value::as_u64)
            .and_then(own)
            .or_else(|| s.sel.messages.last().and_then(|m| s.mailbox.message(*m)).and_then(|m| own(s.mailbox.junk_owner(m.account))))
            .or_else(|| s.mailbox.default_account().map(|a| a.id))
            .unwrap_or(0);
        JunkForm { page, account, ..Default::default() }
    }

    /// The command that adds what was typed to the page in view.
    pub fn add_command(&self) -> (&'static str, Value) {
        ("junk.add", json!({"account": self.account, "list": self.page.as_str(), "entry": self.new_entry}))
    }

    /// The command that removes the selected entry, when one is.
    pub fn remove_command(&self) -> Option<(&'static str, Value)> {
        self.selected.as_ref().map(|e| ("junk.remove", json!({"account": self.account, "list": self.page.as_str(), "entry": e})))
    }

    /// The pages as tabs side by side.
    fn tabs(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let pages = [JunkList::Safe, JunkList::Blocked];
            for (i, page) in pages.into_iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(140.0, ROW_H + 2.0), Sense::click());
                let on = self.page == page;
                let (left, right) = (if i == 0 { 4 } else { 0 }, if i + 1 == pages.len() { 4 } else { 0 });
                let corners = CornerRadius { nw: left, sw: left, ne: right, se: right };
                let fill = if on {
                    t.checked
                } else if resp.hovered() {
                    t.hover
                } else {
                    t.input
                };
                ui.painter().rect(r, corners, fill, Stroke::new(1.0, if on { t.accent } else { t.input_border }), StrokeKind::Inside);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, tl!(title(page)), regular(12.5), if on { t.text } else { t.text_dim });
                if resp.clicked() && !on {
                    self.page = page;
                    self.selected = None;
                    self.error.clear();
                    self.focus = true;
                }
            }
        });
    }

    /// Lay out the form. Returns `true` when the dialog should close.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let enter = crate::keys::enter(ui.ctx(), ui.layer_id());
        let entries: Vec<String> = app.session.mailbox.junk_accounts.get(&self.account).map(|l| l.list(self.page).to_vec()).unwrap_or_default();
        let accounts: Vec<(Id, String)> = app
            .session
            .mailbox
            .accounts
            .iter()
            .filter(|a| a.shared_of.is_none())
            .map(|a| (a.id, shown(if a.email.is_empty() { &a.name } else { &a.email }, 60)))
            .collect();
        if self.selected.as_ref().is_some_and(|e| !entries.contains(e)) {
            self.selected = None;
        }
        let mut run: Option<(&'static str, Value)> = None;
        let mut adding = false;
        body(ui, |ui| {
            // The list and the field take what the buttons beside them leave of the form's width.
            let width = (ui.available_width().min(WIDTH) - BUTTON_W - 12.0).max(120.0);
            if accounts.len() > 1 {
                ui.horizontal(|ui| {
                    ui.label(tl!("Account"));
                    let now = accounts.iter().find(|(id, _)| *id == self.account).map(|(_, n)| n.clone()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("junk_account").selected_text(now).width(280.0).show_ui(ui, |ui| {
                        for (id, name) in &accounts {
                            if ui.selectable_label(self.account == *id, name).clicked() && self.account != *id {
                                self.account = *id;
                                self.selected = None;
                                self.error.clear();
                            }
                        }
                    });
                });
                ui.add_space(6.0);
            }
            self.tabs(ui);
            ui.add_space(8.0);
            ui.label(egui::RichText::new(tl!(about(self.page))).color(t.text_dim));
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                egui::Frame::NONE.fill(t.input).stroke(Stroke::new(1.0, t.input_border)).inner_margin(4.0).corner_radius(3.0).show(ui, |ui| {
                    ui.set_width(width - 8.0);
                    ui.set_height(LIST_H);
                    egui::ScrollArea::vertical().id_salt(("junk_list", self.page.as_str())).max_height(LIST_H).auto_shrink([false, false]).show(
                        ui,
                        |ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                if entries.is_empty() {
                                    ui.label(egui::RichText::new(tl!("No entries yet")).color(t.text_dim));
                                }
                                for e in &entries {
                                    let on = self.selected.as_ref() == Some(e);
                                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H - 2.0), Sense::click());
                                    let fill = if on {
                                        t.selection
                                    } else if resp.hovered() {
                                        t.hover
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };
                                    ui.painter().rect_filled(r, 2.0, fill);
                                    let text = shown(e, 120);
                                    ui.painter().with_clip_rect(r).text(
                                        r.left_center() + vec2(6.0, 0.0),
                                        Align2::LEFT_CENTER,
                                        text,
                                        regular(13.0),
                                        t.text,
                                    );
                                    if resp.clicked() {
                                        self.selected = Some(e.clone());
                                    }
                                }
                            });
                        },
                    );
                });
                ui.vertical(|ui| {
                    let chosen = self.selected.is_some();
                    if ui.add_enabled(chosen, egui::Button::new(tl!("Remove")).min_size(vec2(BUTTON_W, 24.0))).clicked() {
                        run = self.remove_command();
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.new_entry)
                        .id(field_id(("junk_new", self.page.as_str())))
                        .desired_width(width)
                        .hint_text(tl!("someone@example.org or @example.org")),
                );
                if std::mem::take(&mut self.focus) {
                    resp.request_focus();
                }
                let typed = !self.new_entry.trim().is_empty();
                let add = ui.add_enabled(typed, egui::Button::new(tl!("Add")).min_size(vec2(BUTTON_W, 24.0))).clicked();
                if typed && (add || (enter && resp.lost_focus())) {
                    run = Some(self.add_command());
                    adding = true;
                }
            });
        });
        if let Some((command, params)) = run {
            match app.execute(command, params) {
                Ok(_) => {
                    self.error.clear();
                    if adding {
                        self.new_entry.clear();
                        self.focus = true;
                    } else {
                        self.selected = None;
                    }
                }
                Err(e) => self.error = e,
            }
        }
        error(ui, &self.error);
        close_button(ui)
    }
}
