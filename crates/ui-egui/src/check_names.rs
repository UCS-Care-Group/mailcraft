// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check Names with the organisation's address list (#1262). When the owner presses Check Names
//! (Ctrl+K) in a message written from an Exchange server account, the names the contacts don't
//! settle are looked up in the account's address list by the host ([`crate::Services::ews_resolve`]),
//! with the account's own server, sign-in and certificate. Only the names as typed are sent; who
//! was found is kept in the window while the owner chooses, and nowhere else.
//!
//! What came back goes to `compose.checkNames` as its `found`: one person for a name is put in, and
//! for a name with several the owner chooses in [`NameChoice`], which runs the command again with
//! `pick`. A script or an agent running `compose.checkNames` has nothing looked up and no question
//! asked: it gets `unresolved` and `choices` and does as it likes with them.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::Ui;
use mailcraft_engine::cmd::ews_mail::ews_resolve_request;
use mailcraft_model::sync::{EwsResolveRequest, EwsResolveResult};
use serde_json::{Map, Value, json};

use crate::MailApp;
use crate::dialogs::{Dialog, buttons};
use crate::theme::{paint_clipped, regular};

/// The dialog's width.
pub const WIDTH: f32 = 520.0;
/// Most rows a choice lists.
const MAX_ROWS: usize = 100;

/// What the host is asked to look up ([`crate::Services::ews_resolve`]).
pub struct EwsResolveJob {
    pub request: EwsResolveRequest,
    /// As for a run ([`crate::ews_mail::EwsJob::secret`]).
    pub secret: String,
    /// Set when the answer is no longer wanted: it stops before its next request.
    pub cancel: Arc<AtomicBool>,
}

/// A look-up under way, for the message being written that asked.
pub(crate) struct Resolving {
    compose: u64,
    opening: u64,
    rx: Receiver<EwsResolveResult>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Resolving {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The names with several people, for the owner to choose one each, the first on screen.
#[derive(Debug)]
pub struct NameChoice {
    /// The message being written they are for.
    compose: u64,
    /// `(name as typed, people)`, as `compose.checkNames` answered them.
    pub(crate) names: VecDeque<(String, Vec<Value>)>,
    pub(crate) selected: usize,
    /// What the address list had more of than it listed.
    pub(crate) more: Vec<String>,
    error: String,
}

impl MailApp {
    /// Check Names was pressed and has answered `v`: the names it left are looked up in the
    /// address list when the message's account has one and a host can, else what is left is said.
    pub(crate) fn check_names_pressed(&mut self, v: &Value) {
        let unresolved = strings(&v["unresolved"]);
        if unresolved.is_empty() {
            self.status(tl!("All names are checked"));
            return;
        }
        if v["directory"] == json!(true)
            && let Some(compose) = self.session.compose_id()
            && let Ok(request) = ews_resolve_request(&self.session, &unresolved)
            && let Some(secret) = self.ews_secret(request.account)
            && let Some(resolve) = &self.services.ews_resolve
        {
            let cancel = Arc::new(AtomicBool::new(false));
            let rx = resolve(EwsResolveJob { request, secret, cancel: cancel.clone() });
            self.resolving = Some(Resolving { compose, opening: self.session.openings(), rx, cancel });
            self.status(tl!("Looking the names up in the organisation's address list…"));
            return;
        }
        self.offer_choices(v, &[]);
    }

    /// Whether names are being looked up for Check Names.
    pub fn resolving_names(&self) -> bool {
        self.resolving.is_some()
    }

    /// What the host found, once it has: given to Check Names, and what is left offered or said.
    pub fn poll_resolving(&mut self) {
        let Some(resolving) = &self.resolving else { return };
        let result = match resolving.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                EwsResolveResult { error: Some(tl!("Looking the names up stopped unexpectedly").to_string()), ..Default::default() }
            }
        };
        let (compose, opening) = (resolving.compose, resolving.opening);
        self.resolving = None;
        // For a message no longer being written, or in a mailbox no longer open: let go of.
        if self.session.compose_id() != Some(compose) || self.session.openings() != opening {
            return;
        }
        let mut found = Map::new();
        let mut more = Vec::new();
        for name in &result.found {
            found.insert(name.text.clone(), serde_json::to_value(&name.people).unwrap_or(Value::Array(Vec::new())));
            if name.more {
                more.push(name.text.clone());
            }
        }
        match self.execute("compose.checkNames", json!({"found": found})) {
            Ok(v) => {
                self.offer_choices(&v, &more);
                if let Some(error) = &result.error {
                    self.status(tl!(error));
                }
            }
            Err(e) => self.status(tl!(&e)),
        }
    }

    /// The names of `v` with several people are offered for the owner to choose one each; the
    /// rest, nobody's, are said.
    fn offer_choices(&mut self, v: &Value, more: &[String]) {
        let unresolved = strings(&v["unresolved"]);
        let mut names: VecDeque<(String, Vec<Value>)> = VecDeque::new();
        for choice in v["choices"].as_array().into_iter().flatten() {
            let (Some(text), Some(people)) = (choice["text"].as_str(), choice["people"].as_array()) else { continue };
            names.push_back((text.to_string(), people.iter().take(MAX_ROWS).cloned().collect()));
        }
        let nobody: Vec<&String> = unresolved.iter().filter(|text| !names.iter().any(|(name, _)| name == *text)).collect();
        if nobody.is_empty() {
            if unresolved.is_empty() {
                self.status(tl!("All names are checked"));
            } else {
                // Every name left has people to choose from: the dialog says so, and "Looking the
                // names up…" no longer stands.
                self.status_msg = None;
            }
        } else {
            // (As typed, so one line and not too long, as the meeting form says them.)
            let list: Vec<String> = nobody.iter().map(|text| text.chars().filter(|c| !c.is_control()).take(80).collect()).collect();
            self.status(crate::i18n::fmt(tl!("Nobody matches: {names}"), &[("names", &list.join("; "))]));
        }
        if let (false, Some(compose)) = (names.is_empty(), self.session.compose_id())
            && self.dialog.is_none()
        {
            self.dialog = Some(Dialog::NameChoice(Box::new(NameChoice { compose, names, selected: 0, more: more.to_vec(), error: String::new() })));
        }
    }
}

impl NameChoice {
    /// The message being written it is for.
    pub(crate) fn compose(&self) -> u64 {
        self.compose
    }

    /// Draws it; true once every name is chosen for, or it is cancelled.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        // The message it was for has gone, or isn't the one being written.
        if app.session.compose_id() != Some(self.compose) {
            return true;
        }
        let Some((text, people)) = self.names.front() else { return true };
        let t = crate::theme::Tokens::get(ui.ctx());
        ui.label(crate::i18n::fmt(tl!("More than one person matches \"{name}\". Choose one:"), &[("name", text)]));
        if self.more.contains(text) {
            ui.label(
                egui::RichText::new(tl!("The address list has more people of that name than it shows: type more of it to find one not listed."))
                    .color(t.text_dim),
            );
        }
        ui.add_space(4.0);
        let mut chosen = None;
        egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 1.0);
            for (index, person) in people.iter().enumerate() {
                let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.0), egui::Sense::click());
                if self.selected == index {
                    ui.painter().rect_filled(rect, 4.0, t.selection);
                } else if r.hovered() {
                    ui.painter().rect_filled(rect, 4.0, t.hover);
                }
                let (x, w) = (rect.min.x + 10.0, rect.width() - 20.0);
                paint_clipped(ui.painter(), egui::pos2(x, rect.min.y + 11.0), w, &first_line(person), regular(13.0), t.text);
                paint_clipped(ui.painter(), egui::pos2(x, rect.min.y + 27.0), w, &second_line(person), regular(11.0), t.text_dim);
                if r.clicked() {
                    self.selected = index;
                }
                if r.double_clicked() {
                    chosen = Some(index);
                }
            }
        });
        if !self.error.is_empty() {
            ui.label(egui::RichText::new(self.error.as_str()).color(t.red));
        }
        let (ok, cancel) = buttons(ui, tl!("OK"), self.selected < people.len());
        if (ok || enter) && chosen.is_none() {
            chosen = Some(self.selected);
        }
        let Some(person) = chosen.and_then(|index| people.get(index)) else { return cancel };
        let pick = json!({text.clone(): {"name": person["name"], "email": person["email"]}});
        match app.execute("compose.checkNames", json!({"pick": pick})) {
            Ok(_) => {
                self.names.pop_front();
                self.selected = 0;
                self.error.clear();
                self.names.is_empty()
            }
            Err(e) => {
                self.error = tl!(&e).to_string();
                false
            }
        }
    }
}

/// The name and address of a person of a choice.
fn first_line(person: &Value) -> String {
    let (name, email) = (person["name"].as_str().unwrap_or(""), person["email"].as_str().unwrap_or(""));
    if name.is_empty() { email.to_string() } else { format!("{name} <{email}>") }
}

/// What tells them apart: the job title, department and company, what kind of recipient it is
/// when it isn't a person, and where it was found.
fn second_line(person: &Value) -> String {
    let mut parts: Vec<String> = ["title", "department", "company"].iter().filter_map(|key| person[*key].as_str()).map(str::to_string).collect();
    match person["kind"].as_str() {
        Some("list") => parts.push(tl!("Distribution list").to_string()),
        Some("group") => parts.push(tl!("Group").to_string()),
        Some("publicFolder") => parts.push(tl!("Public folder").to_string()),
        Some("contact") => parts.push(tl!("Outside the organisation").to_string()),
        _ => {}
    }
    parts.push(match person["from"].as_str() {
        Some("contacts") => tl!("Contacts").to_string(),
        _ => tl!("Address list").to_string(),
    });
    parts.join(" · ")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}
