// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folder › Clean Up › Senders (#1207, part 3): the senders of the folder in view, most messages
//! first, as the engine counts them (`mail.senders`): how many messages, how many unread, the
//! newest one's date, how each can be unsubscribed from, and whether it has a sender warning. The
//! owner ticks several, then Unsubscribe (`ui.unsubscribeMany` on each one's newest message, its
//! own question first), Delete All (`mail.delete`, to Deleted Items, undoable) or Move All (their
//! messages selected, then Move Items). Nothing happens on a tick: only on a button, and each
//! button closes the dialog first, so that the question or dialog it leads to is the one shown.

use std::collections::{BTreeSet, HashSet};

use egui::{RichText, Ui, vec2};
use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::button_row;
use crate::people_view::one_line;
use crate::reading::unsubscribe as unsub;
use crate::theme::Tokens;
use crate::widgets::primary_button;

pub const WIDTH: f32 = 720.0;

/// A sender, as `mail.senders` answers it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub sender: String,
    pub name: String,
    pub messages: u64,
    pub unread: u64,
    pub last: i64,
    pub newest: Id,
    /// `oneClick`, `mail` or `page`; `None` when its newest message offers nothing.
    pub way: Option<String>,
    /// Why it offers nothing.
    pub why: String,
    pub warning: bool,
}

pub struct Senders {
    pub(crate) folder: Id,
    pub(crate) rows: Vec<Row>,
    /// Senders past those listed.
    pub(crate) more: u64,
    /// The senders ticked, by address.
    pub(crate) ticked: BTreeSet<String>,
}

impl Senders {
    /// The senders of `folder` (else the folder in view); `None` when the engine refuses.
    pub fn open(app: &mut MailApp, params: &Value) -> Option<Senders> {
        let asked = match params.get("folder") {
            Some(folder) if !folder.is_null() => json!({"folder": folder}),
            _ => json!({}),
        };
        let answer = app.session.run("mail.senders", &asked).ok()?;
        Some(Senders::from_answer(&answer))
    }

    pub(crate) fn from_answer(answer: &Value) -> Senders {
        let rows = answer
            .get("senders")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
                let number = |k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0);
                Some(Row {
                    sender: r.get("sender").and_then(Value::as_str)?.to_string(),
                    name: text("name"),
                    messages: number("messages"),
                    unread: number("unread"),
                    last: r.get("last").and_then(Value::as_i64).unwrap_or(0),
                    newest: r.get("newest").and_then(Value::as_u64)?,
                    way: r.get("way").and_then(Value::as_str).map(str::to_string),
                    why: text("why"),
                    warning: r.get("warning").and_then(Value::as_bool) == Some(true),
                })
            })
            .collect();
        Senders {
            folder: answer.get("folder").and_then(Value::as_u64).unwrap_or(0),
            rows,
            more: answer.get("more").and_then(Value::as_u64).unwrap_or(0),
            ticked: BTreeSet::new(),
        }
    }

    fn ticked_rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| self.ticked.contains(&r.sender))
    }

    /// The newest message of each ticked sender that offers a way, and isn't unsubscribed from
    /// already: what Unsubscribe goes ahead with.
    pub(crate) fn to_unsubscribe(&self, app: &MailApp) -> Vec<Id> {
        self.ticked_rows().filter(|r| r.way.is_some() && unsub::outcome(app, r.newest).is_none()).map(|r| r.newest).collect()
    }

    /// Every message in the folder of the ticked senders: what Delete All and Move All act on.
    pub(crate) fn their_messages(&self, app: &MailApp) -> Vec<Id> {
        let senders: HashSet<String> = self.ticked.iter().cloned().collect();
        mailcraft_engine::cmd::unsubscribe::of_senders(&app.session.mailbox, self.folder, &senders)
    }

    /// The dialog's contents. Returns what to run once it has closed (`None` while it stays open),
    /// or `Some(vec![])` for Close.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> Option<Vec<(String, Value)>> {
        let t = Tokens::get(ui.ctx());
        let folder = app.session.mailbox.folder(self.folder).map(|f| crate::nav::folder_name(f).to_string()).unwrap_or_default();
        ui.label(RichText::new(crate::i18n::fmt(tl!("Senders in {folder}"), &[("folder", &one_line(&folder, 60))])).strong());
        ui.label(
            RichText::new(tl!("Tick senders to unsubscribe from their lists, or to delete or move all their messages in this folder."))
                .color(t.text_dim),
        );
        if self.rows.is_empty() {
            ui.label(tl!("No messages in this folder."));
            let mut ok = enter;
            button_row(ui, |ui| ok |= primary_button(ui, tl!("OK"), true).clicked());
            return ok.then(Vec::new);
        }
        self.table(app, ui, &t);
        if self.more > 0 {
            ui.label(
                RichText::new(tln!(self.more, "and {n} more sender, with fewer messages", "and {n} more senders, with fewer messages"))
                    .color(t.text_dim),
            );
        }
        // The one-click requests on their way, one at a time, which can be stopped.
        let pending = unsub::pending(app);
        if pending > 0 {
            ui.horizontal(|ui| {
                ui.label(tln!(pending as u64, "Unsubscribing: {n} list left.", "Unsubscribing: {n} lists left."));
                if ui.link(tl!("Stop")).on_hover_text(tl!("The lists not yet asked aren't asked; the one being asked still is.")).clicked() {
                    unsub::stop(app);
                }
            });
        }
        let unsubscribe = self.to_unsubscribe(app);
        let any = !self.ticked.is_empty();
        let mut then = None;
        ui.add_space(4.0);
        button_row(ui, |ui| {
            if ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0))).clicked() {
                then = Some(Vec::new());
            }
            if ui.add_enabled(any, egui::Button::new(tl!("Move All…")).min_size(vec2(72.0, 26.0))).clicked() {
                let ids = self.their_messages(app);
                then = Some(vec![("mail.select".to_string(), json!({"ids": ids})), ("ui.dialog".to_string(), json!({"name": "moveItems"}))]);
            }
            if ui.add_enabled(any, egui::Button::new(tl!("Delete All")).min_size(vec2(72.0, 26.0))).clicked() {
                then = Some(vec![("mail.delete".to_string(), json!({"ids": self.their_messages(app)}))]);
            }
            let tip = tl!("Each ticked sender's newest message says how: MailCraft asks first.");
            if primary_button(ui, tl!("Unsubscribe"), !unsubscribe.is_empty()).on_hover_text(tip).clicked() && !unsubscribe.is_empty() {
                then = Some(vec![("ui.unsubscribeMany".to_string(), json!({"ids": unsubscribe}))]);
            }
        });
        then
    }

    fn table(&mut self, app: &MailApp, ui: &mut Ui, t: &Tokens) {
        let (now, twelve) = (app.session.now(), app.session.prefs.twelve_hour);
        egui::ScrollArea::vertical().id_salt("senders_rows").max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("senders_grid").num_columns(6).striped(true).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                let mut all = !self.rows.is_empty() && self.rows.iter().all(|r| self.ticked.contains(&r.sender));
                if ui.add(egui::Checkbox::without_text(&mut all)).on_hover_text(tl!("Tick every sender listed")).changed() {
                    self.ticked = if all { self.rows.iter().map(|r| r.sender.clone()).collect() } else { BTreeSet::new() };
                }
                for head in [tl!("Sender"), tl!("Messages"), tl!("Unread"), tl!("Last received"), tl!("How to unsubscribe")] {
                    ui.label(RichText::new(head).strong());
                }
                ui.end_row();
                for r in &self.rows {
                    let mut on = self.ticked.contains(&r.sender);
                    if ui.add(egui::Checkbox::without_text(&mut on)).changed() {
                        if on {
                            self.ticked.insert(r.sender.clone());
                        } else {
                            self.ticked.remove(&r.sender);
                        }
                    }
                    ui.vertical(|ui| {
                        // On one line each, however long: the column is as wide as they are.
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        ui.set_min_width(240.0);
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        let name = if r.name.trim().is_empty() { r.sender.clone() } else { r.name.clone() };
                        ui.horizontal(|ui| {
                            if r.warning {
                                let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                                crate::icons::paint(ui.painter(), rect, "warning", t.text, t.orange);
                            }
                            ui.label(one_line(&name, 50));
                        });
                        ui.label(RichText::new(one_line(&r.sender, 60)).color(t.text_dim).small());
                    });
                    ui.label(r.messages.to_string());
                    let share = r.unread.saturating_mul(100).checked_div(r.messages).unwrap_or(0);
                    ui.label(format!("{share} %"));
                    ui.label(crate::dates::list_label(r.last, now, app.session.offset_at(r.last), twelve));
                    let (said, tip) = way_text(app, r);
                    let resp = ui.label(RichText::new(said).color(if r.way.is_some() { t.text } else { t.text_dim }));
                    if !tip.is_empty() {
                        resp.on_hover_text(tip);
                    }
                    ui.end_row();
                }
            });
        });
    }
}

/// How a sender's row says it can be unsubscribed from, or what came of it, and why not.
fn way_text(app: &MailApp, r: &Row) -> (String, String) {
    match unsub::outcome(app, r.newest) {
        Some(unsub::Outcome::Accepted) => return (tl!("Unsubscribed").to_string(), String::new()),
        Some(unsub::Outcome::Queued) => return (tl!("Request in your Outbox").to_string(), String::new()),
        Some(unsub::Outcome::Opened) => return (tl!("Page opened").to_string(), String::new()),
        Some(unsub::Outcome::Failed(why)) => return (tl!("Not unsubscribed").to_string(), why),
        None => {}
    }
    match r.way.as_deref() {
        Some("oneClick") if app.services.unsubscribe.is_some() => (tl!("In one click").to_string(), String::new()),
        Some("oneClick" | "page") => (tl!("On its own page").to_string(), tl!("Open one of its messages to unsubscribe there.").to_string()),
        Some("mail") => (tl!("By mail").to_string(), String::new()),
        _ => ("—".to_string(), r.why.clone()),
    }
}

#[cfg(test)]
#[path = "senders_tests.rs"]
mod tests;
