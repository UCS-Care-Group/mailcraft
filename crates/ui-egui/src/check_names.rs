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
use mailcraft_model::Id;
use mailcraft_model::sync::{EwsResolveRequest, EwsResolveResult, MAX_CHECK_NAMES, MAX_RESOLVE_TEXT, one_line};
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

/// A look-up under way, for the message being written that asked, ten names a
/// request until the names are done.
pub(crate) struct Resolving {
    compose: u64,
    opening: u64,
    /// The account whose address list is asked: a message now sent from another lets the answer go.
    account: Id,
    rx: Receiver<EwsResolveResult>,
    cancel: Arc<AtomicBool>,
    /// The password it went with: only that one is forgotten when the server turns it down.
    used: String,
    /// The names not asked yet.
    rest: Vec<String>,
    gathered: Gathered,
}

/// What the requests of one Check Names have brought so far.
#[derive(Default)]
struct Gathered {
    /// `{name as looked up: [person…]}`, as `compose.checkNames` takes it.
    found: Map<String, Value>,
    /// The names the address list has more people for than it listed.
    more: Vec<String>,
    /// The names asked, as they are matched ([`key`]).
    asked: Vec<String>,
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
            && self.services.ews_resolve.is_some()
            && let Some(compose) = self.session.compose_id()
        {
            let names: Vec<String> = unresolved.into_iter().take(MAX_CHECK_NAMES).collect();
            match self.ask_address_list(compose, self.session.openings(), names, Gathered::default()) {
                Ok(()) => self.status(looking()),
                // Why the address list wasn't asked is what there is to say.
                Err((why, _)) => {
                    self.offer_choices(v, &[], None);
                    if !why.is_empty() {
                        self.status(why);
                    }
                }
            }
            return;
        }
        self.offer_choices(v, &[], None);
    }

    /// The next names of `names` are asked for in the address list of the message's account; the
    /// others wait their turn. What it can't be asked for is said, and `gathered` given back.
    fn ask_address_list(&mut self, compose: u64, opening: u64, names: Vec<String>, mut gathered: Gathered) -> Result<(), (String, Gathered)> {
        if self.offline() {
            return Err((crate::send_receive::said(), gathered));
        }
        let request = match ews_resolve_request(&self.session, &names) {
            Ok(request) if !request.texts.is_empty() => request,
            Ok(_) => return Err((String::new(), gathered)),
            Err(e) => return Err((tl!(&e.to_string()).to_string(), gathered)),
        };
        let Some(secret) = self.ews_secret(request.account) else {
            return Err((tl!("The account's password is needed first: press Send/Receive and type it").to_string(), gathered));
        };
        let Some(resolve) = &self.services.ews_resolve else { return Err((String::new(), gathered)) };
        let sent: Vec<String> = request.texts.iter().map(|text| key(text)).collect();
        let rest = names.into_iter().filter(|name| !key(name).is_empty() && !sent.contains(&key(name))).collect();
        let account = request.account;
        let cancel = Arc::new(AtomicBool::new(false));
        let used = secret.clone();
        let rx = resolve(EwsResolveJob { request, secret, cancel: cancel.clone() });
        gathered.asked.extend(sent);
        self.resolving = Some(Resolving { compose, opening, account, rx, cancel, used, rest, gathered });
        Ok(())
    }

    /// Whether names are being looked up for Check Names.
    pub fn resolving_names(&self) -> bool {
        self.resolving.is_some()
    }

    /// What the host found, once it has: the next names are asked for, and once they are all done
    /// (or something went wrong) what came is given to Check Names, and what is left offered or said.
    pub fn poll_resolving(&mut self) {
        let Some(resolving) = &mut self.resolving else { return };
        let result = match resolving.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                EwsResolveResult { error: Some(tl!("Looking the names up stopped unexpectedly").to_string()), ..Default::default() }
            }
        };
        let (compose, opening, account) = (resolving.compose, resolving.opening, resolving.account);
        let (rest, mut gathered, used) =
            (std::mem::take(&mut resolving.rest), std::mem::take(&mut resolving.gathered), std::mem::take(&mut resolving.used));
        self.resolving = None;
        // For a message no longer being written, in a mailbox no longer open, or now sent from
        // another account than the one whose address list answered: let go of.
        let from = ews_resolve_request(&self.session, &[]).ok().map(|request| request.account);
        if self.session.compose_id() != Some(compose) || self.session.openings() != opening || from != Some(account) {
            if self.status_msg.as_ref().is_some_and(|(text, _)| text == looking()) {
                self.status_msg = None;
            }
            return;
        }
        for name in &result.found {
            gathered.found.insert(name.text.clone(), serde_json::to_value(&name.people).unwrap_or(Value::Array(Vec::new())));
            if name.more {
                gathered.more.push(name.text.clone());
            }
        }
        if result.auth_refused {
            self.ews_password_refused_with(account, &used);
        }
        let error = result.error;
        if error.is_none() && !result.auth_refused && !rest.is_empty() {
            match self.ask_address_list(compose, opening, rest, gathered) {
                Ok(()) => return,
                Err((_, back)) => gathered = back,
            }
        }
        let more = gathered.more.clone();
        match self.execute("compose.checkNames", json!({"found": gathered.found, "more": gathered.more})) {
            Ok(v) => {
                self.offer_choices(&v, &more, Some(&gathered.asked));
                if let Some(error) = &error {
                    self.status(tl!(error));
                }
            }
            Err(e) => self.status(tl!(&e)),
        }
    }

    /// The names of `v` with several people are offered for the owner to choose one each; the
    /// rest, nobody's, are said: as not looked up, when the address list was asked (`asked`) and
    /// they weren't among the names asked.
    fn offer_choices(&mut self, v: &Value, more: &[String], asked: Option<&[String]>) {
        let unresolved = strings(&v["unresolved"]);
        let mut names: VecDeque<(String, Vec<Value>)> = VecDeque::new();
        for choice in v["choices"].as_array().into_iter().flatten() {
            let (Some(text), Some(people)) = (choice["text"].as_str(), choice["people"].as_array()) else { continue };
            names.push_back((text.to_string(), people.iter().take(MAX_ROWS).cloned().collect()));
        }
        let left: Vec<&String> = unresolved.iter().filter(|text| !names.iter().any(|(name, _)| name == *text)).collect();
        let (nobody, not_asked): (Vec<&String>, Vec<&String>) =
            left.into_iter().partition(|text| asked.is_none_or(|asked| asked.contains(&key(text))));
        if nobody.is_empty() && not_asked.is_empty() {
            if unresolved.is_empty() {
                self.status(tl!("All names are checked"));
            } else {
                // Every name left has people to choose from: the dialog says so, and "Looking the
                // names up…" no longer stands.
                self.status_msg = None;
            }
        } else {
            // (As typed, so one line and not too long, as the meeting form says them.)
            let said = |list: &[&String]| -> String {
                list.iter().map(|text| text.chars().filter(|c| !c.is_control()).take(80).collect::<String>()).collect::<Vec<_>>().join("; ")
            };
            let mut parts = Vec::new();
            if !nobody.is_empty() {
                parts.push(crate::i18n::fmt(tl!("Nobody matches: {names}"), &[("names", &said(&nobody))]));
            }
            if !not_asked.is_empty() {
                parts.push(crate::i18n::fmt(tl!("Not looked up in the address list: {names}"), &[("names", &said(&not_asked))]));
            }
            self.status(parts.join(" · "));
        }
        if let (false, Some(compose)) = (names.is_empty(), self.session.compose_id())
            && self.dialog.is_none()
        {
            self.set_dialog(Some(Dialog::NameChoice(Box::new(NameChoice {
                compose,
                names,
                selected: 0,
                more: more.to_vec(),
                error: String::new(),
            }))));
        }
    }
}

/// Anchor the painted candidate line without changing the row's click region.
pub(crate) fn candidate_text_start_x(rect: egui::Rect, painted_width: f32, rtl: bool) -> f32 {
    let start = rect.min.x + 10.0;
    if rtl { (rect.max.x - 10.0 - painted_width).max(start) } else { start }
}

/// Keep the existing English paint path; Hebrew right-aligns the same clipped
/// galley inside the same padded candidate row.
fn paint_candidate_line(painter: &egui::Painter, rect: egui::Rect, y: f32, text: &str, font: egui::FontId, color: egui::Color32, rtl: bool) {
    let width = rect.width() - 20.0;
    if !rtl {
        paint_clipped(painter, egui::pos2(rect.min.x + 10.0, y), width, text, font, color);
        return;
    }
    if width <= 4.0 || text.is_empty() {
        return;
    }
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::text::TextFormat { font_id: font, color, ..Default::default() });
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = painter.layout_job(job);
    let size = galley.size();
    painter.galley(egui::pos2(candidate_text_start_x(rect, size.x, true), y - size.y / 2.0), galley, color);
}

/// Right-anchor descriptive dialog text in Hebrew; leave English label layout unchanged.
pub(crate) fn choice_description(ui: &mut Ui, text: impl Into<egui::WidgetText>, rtl: bool) -> egui::Response {
    if rtl {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.add(egui::Label::new(text).wrap())).inner
    } else {
        ui.label(text)
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
        let rtl = crate::i18n::current().code() == "he";
        choice_description(ui, crate::i18n::fmt(tl!("More than one person matches \"{name}\". Choose one:"), &[("name", text)]), rtl);
        if self.more.contains(text) {
            choice_description(
                ui,
                egui::RichText::new(tl!("The address list has more people of that name than it shows: type more of it to find one not listed."))
                    .color(t.text_dim),
                rtl,
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
                paint_candidate_line(ui.painter(), rect, rect.min.y + 11.0, &first_line(person, rtl), regular(13.0), t.text, rtl);
                paint_candidate_line(ui.painter(), rect, rect.min.y + 27.0, &second_line(person, rtl), regular(11.0), t.text_dim, rtl);
                if r.clicked() {
                    self.selected = index;
                }
                if r.double_clicked() {
                    chosen = Some(index);
                }
            }
        });
        if !self.error.is_empty() {
            choice_description(ui, egui::RichText::new(self.error.as_str()).color(t.red), rtl);
        }
        let (ok, cancel) = buttons(ui, tl!("OK"), self.selected < people.len());
        if (ok || enter) && chosen.is_none() {
            chosen = Some(self.selected);
        }
        let Some(person) = chosen.and_then(|index| people.get(index)) else { return cancel };
        let pick = json!({text.clone(): {"name": person["name"], "email": person["email"]}});
        match app.execute("compose.checkNames", json!({"pick": pick})) {
            Ok(v) => {
                // The line was changed meanwhile, and the name is no longer on it: nothing was put in.
                if !v["picked"].as_array().is_some_and(|picked| picked.iter().any(|p| p.as_str() == Some(text.as_str()))) {
                    let name: String = text.chars().filter(|c| !c.is_control()).take(80).collect();
                    app.status(crate::i18n::fmt(tl!("\"{name}\" is no longer on the recipient lines"), &[("name", &name)]));
                }
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
fn first_line(person: &Value, rtl: bool) -> String {
    let (name, email) = (person["name"].as_str().unwrap_or(""), person["email"].as_str().unwrap_or(""));
    if name.is_empty() {
        if rtl && !email.is_empty() {
            // Keep a bare address left-to-right when there is no display name.
            // This changes presentation only; the selected person JSON is unchanged.
            format!("\u{2066}{email}\u{2069}")
        } else {
            email.to_string()
        }
    } else if rtl && email.is_empty() {
        // Directory entries can have a display name without an email address.
        // Do not paint misleading empty angle brackets in the Hebrew candidate row.
        name.to_string()
    } else if rtl && !email.is_empty() {
        // Keep <address@domain> together in its LTR order beside a Hebrew display name.
        // These isolates affect the painted label only, never the chosen recipient JSON.
        format!("{name} \u{2066}<{email}>\u{2069}")
    } else {
        format!("{name} <{email}>")
    }
}

/// What tells them apart: the job title, department and company, what kind of recipient it is
/// when it isn't a person, and where it was found.
fn second_line(person: &Value, rtl: bool) -> String {
    let mut parts: Vec<String> = ["title", "department", "company"].iter().filter_map(|key| person[*key].as_str()).filter(|s| !rtl || !s.trim().is_empty()).map(str::to_string).collect();
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
    // In Hebrew, make every descriptive field a separate bidi unit. Latin job titles,
    // departments and company names can contain punctuation and parentheses: without
    // isolation, those marks may attach to neighboring Hebrew fields or separators.
    // Preserve the previous English/LTR output byte-for-byte.
    if rtl { parts.into_iter().map(|part| format!("\u{2068}{part}\u{2069}")).collect::<Vec<_>>().join(" · ") } else { parts.join(" · ") }
}

/// What the status line says while names are looked up.
fn looking() -> &'static str {
    tl!("Looking the names up in the organisation's address list…")
}

/// A name as it is matched with what was asked: one line, as it was sent, in any letter case.
fn key(text: &str) -> String {
    one_line(text, MAX_RESOLVE_TEXT).to_lowercase()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

#[cfg(test)]
mod hebrew_email_bidi_tests {
    use super::*;

    #[test]
    fn check_names_hebrew_name_keeps_email_brackets_left_to_right() {
        let person = json!({"name": "דנה", "email": "dana@example.org"});
        assert_eq!(first_line(&person, false), "דנה <dana@example.org>");
        assert_eq!(first_line(&person, true), "דנה \u{2066}<dana@example.org>\u{2069}");
        // English keeps the bare address; Hebrew isolates it for LTR reading.
        let bare = json!({"name": "", "email": "dana@example.org"});
        assert_eq!(first_line(&bare, false), "dana@example.org");
        assert_eq!(first_line(&bare, true), "\u{2066}dana@example.org\u{2069}");
        let punctuated = json!({"name": "", "email": "dana+tag@sub.example.org"});
        assert_eq!(
            first_line(&punctuated, true),
            "\u{2066}dana+tag@sub.example.org\u{2069}"
        );
    }

    #[test]
    fn check_names_hebrew_secondary_details_isolate_mixed_script_fields() {
        let person = json!({
            "title": "מנהלת",
            "department": "DevOps (EMEA)",
            "company": "Acme GmbH",
            "from": "contacts",
        });
        let original = person.clone();
        let english = second_line(&person, false);
        let hebrew = second_line(&person, true);
        assert!(english.starts_with("מנהלת · DevOps (EMEA) · Acme GmbH · "));
        assert!(hebrew.contains("\u{2068}DevOps (EMEA)\u{2069}"));
        assert!(hebrew.contains("\u{2068}Acme GmbH\u{2069}"));
        assert_eq!(hebrew.matches('\u{2068}').count(), 4);
        assert_eq!(hebrew.matches('\u{2069}').count(), 4);
        assert_eq!(hebrew.replace('\u{2068}', "").replace('\u{2069}', ""), english);
        assert_eq!(person, original, "presentation must not mutate recipient metadata");
    }

    #[test]
    fn check_names_ltr_and_empty_address_keep_previous_display() {
        let person = json!({"name": "Dana", "email": "dana@example.org"});
        assert_eq!(first_line(&person, false), "Dana <dana@example.org>");
        let empty = json!({"name": "דנה", "email": ""});
        assert_eq!(first_line(&empty, true), "דנה");
        let missing_email = json!({"name": "דנה"});
        assert_eq!(first_line(&missing_email, true), "דנה");
        assert_eq!(first_line(&missing_email, false), "דנה <>");
        assert_eq!(first_line(&empty, false), "דנה <>");
    }

    #[test]
    fn check_names_hebrew_secondary_omits_blank_fields_without_changing_english() {
        let empty = json!({"title": "", "department": " \t ", "company": "", "from": "contacts"});
        let before = empty.clone();
        let hebrew = second_line(&empty, true);
        assert_eq!(hebrew.matches(" · ").count(), 0, "no empty Hebrew bullet separators");
        assert_eq!(hebrew.matches('\u{2068}').count(), 1);
        assert_eq!(hebrew.matches('\u{2069}').count(), 1);
        assert_eq!(second_line(&empty, false).matches(" · ").count(), 3, "English byte path preserved");
        let partial = json!({"title": "", "department": "", "company": "Acme GmbH", "from": "contacts"});
        let summary = second_line(&partial, true);
        assert!(summary.starts_with("\u{2068}Acme GmbH\u{2069} · "));
        assert_eq!(summary.matches(" · ").count(), 1);
        assert_eq!(second_line(&partial, false).matches(" · ").count(), 3);
        assert_eq!(empty, before, "presentation must not mutate directory data");
    }
}
