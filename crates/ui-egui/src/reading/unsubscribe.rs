// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Unsubscribe from one message (#1207): the bar above a message from a mailing list, and
//! `ui.unsubscribe`, which does what the engine can't. One click goes to the list's server through
//! the host ([`crate::Services::unsubscribe`], `mailcraft_net::pictures::unsubscribe`); a web page
//! opens in the owner's browser; by mail is the engine's `mail.unsubscribe` with `"send": true`.
//! Each only on the owner's click, after the question of `dialogs::confirmation`.
//!
//! The bar is offered only when the engine would answer (`mailcraft_engine::cmd` refuses mail its
//! server doesn't vouch for, and mail with a sender warning): a message that can't be
//! unsubscribed from shows nothing.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Receiver;

use egui::{Sense, Ui, vec2};
use mailcraft_model::Message;
use mailcraft_model::unsubscribe::{Way, way};
use serde_json::{Value, json};

use super::Place;
use super::remote::notice;

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// What came of unsubscribing from a message: each way says only what it knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// One click: the list's server took the request.
    Accepted,
    /// By mail: the request is in the Outbox, not yet sent, let alone read.
    Queued,
    /// A web page: it is open in the browser, where the owner unsubscribes.
    Opened,
    /// One click: the list's server didn't take it, or the request was lost; why.
    Failed(String),
}

/// The most outcomes kept (one a message), and the most one-click requests waiting their turn.
const MAX_KEPT: usize = 1_000;

/// The one-click unsubscribes on their way, one at a time, and what came of each.
#[derive(Default)]
pub(crate) struct Unsubscribing {
    /// The message, and the list server's answer to come.
    flying: Option<(u64, Receiver<Result<(), String>>)>,
    /// The one-click requests of a selection waiting their turn: the message, the address.
    waiting: VecDeque<(u64, String)>,
    /// What came of unsubscribing, by message.
    said: HashMap<u64, Outcome>,
}

impl Unsubscribing {
    /// Whether a request for `id` is on its way or waiting its turn.
    fn busy_with(&self, id: u64) -> bool {
        self.flying.as_ref().is_some_and(|(m, _)| *m == id) || self.waiting.iter().any(|(m, _)| *m == id)
    }

    fn busy(&self) -> bool {
        self.flying.is_some() || !self.waiting.is_empty()
    }

    fn say(&mut self, id: u64, outcome: Outcome) {
        if self.said.len() >= MAX_KEPT && !self.said.contains_key(&id) {
            self.said.clear();
        }
        self.said.insert(id, outcome);
    }
}

/// The answer of the one-click unsubscribe on its way, once it has come; then the next waiting.
pub(crate) fn poll(app: &mut MailApp) {
    if let Some((id, rx)) = &app.unsubscribing.flying {
        let id = *id;
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(tl!("the request was lost").to_string()),
        };
        app.unsubscribing.flying = None;
        app.unsubscribing.say(id, if let Err(why) = answer { Outcome::Failed(why) } else { Outcome::Accepted });
    }
    // One at a time: each list's server is asked once, and none waits on another's slowness long.
    if let Some((id, url)) = app.unsubscribing.waiting.pop_front() {
        match &app.services.unsubscribe {
            Some(post) => app.unsubscribing.flying = Some((id, post(url))),
            None => app.unsubscribing.say(id, Outcome::Failed(tl!("this build can't unsubscribe in one click").to_string())),
        }
    }
}

/// Whether the engine would offer `m` a way to unsubscribe (its server vouches for it, and it has
/// no sender warning): the bar and the right-click menu offer it only then.
pub(crate) fn offered(app: &mut MailApp, m: &Message) -> bool {
    way(m).is_some()
        && mailcraft_model::auth_results::vouched(app.session.mailbox.account(m.account), m)
        && mailcraft_model::sender_warnings::banner_level(&app.session.sender_warnings(m, true)).is_none()
}

/// The bar above a message from a mailing list: Unsubscribe, how it is going, or what came of it.
pub(super) fn bar(app: &mut MailApp, ui: &mut Ui, m: &Message, place: Place, t: &Tokens) {
    poll(app);
    let flying = app.unsubscribing.busy_with(m.id);
    let said = app.unsubscribing.said.get(&m.id).cloned();
    if !flying && said.is_none() && !offered(app, m) {
        return;
    }
    let (hue, icon, lead, rest) = match (&said, flying) {
        (_, true) => (t.blue, "info", tl!("Unsubscribing…").to_string(), String::new()),
        (Some(Outcome::Accepted), _) => (
            t.green,
            "info",
            tl!("Unsubscribed.").to_string(),
            tl!("The list's server took the request. Mail from it may take a few days to stop.").to_string(),
        ),
        (Some(Outcome::Queued), _) => (
            t.blue,
            "info",
            tl!("Request in your Outbox.").to_string(),
            tl!("It goes with the next Send/Receive; the list then takes you off it.").to_string(),
        ),
        (Some(Outcome::Opened), _) => {
            (t.blue, "info", tl!("Page opened.").to_string(), tl!("Unsubscribe on the list's page in your browser.").to_string())
        }
        (Some(Outcome::Failed(why)), _) => (
            t.orange,
            "highImportance",
            tl!("Not unsubscribed.").to_string(),
            crate::i18n::fmt(tl!("The list's server didn't take the request: {why}."), &[("why", why)]),
        ),
        (None, false) => (t.blue, "info", tl!("From a mailing list.").to_string(), String::new()),
    };
    notice(hue, t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(1.0), icon, t.text, hue);
            ui.add(egui::Label::new(egui::RichText::new(lead).font(semibold(11.5)).color(t.text)).wrap());
            if !rest.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(rest).font(regular(11.5)).color(t.text_dim)).wrap());
            }
            // Unsubscribe, or after a failure Try again (asked first all the same).
            let failed = matches!(said, Some(Outcome::Failed(_)));
            if flying || said.is_some() && !failed {
                return;
            }
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            let label = if failed { tl!("Try again") } else { tl!("Unsubscribe") };
            let link = ui
                .add(egui::Link::new(egui::RichText::new(label).font(semibold(11.5))))
                .on_hover_text(tl!("Ask the list to stop sending you its mail. MailCraft asks first and says how."));
            if place == Place::Pane {
                app.hit.add("unsubscribe", m.id, link.rect);
            }
            if link.clicked() {
                let _ = app.run("ui.unsubscribe", json!({"message": m.id}));
            }
        });
    });
}

/// The question asked before `ui.unsubscribe` goes ahead: what will be sent, to whom, or opened.
/// `None` when the message offers nothing (the command then says why).
pub(crate) fn question(app: &MailApp, p: &Value) -> Option<crate::dialogs::Question> {
    let id = p.get("message").and_then(Value::as_u64).or_else(|| app.session.sel.focus())?;
    let m = app.session.mailbox.messages.get(&id)?;
    let host = |url: &str| mailcraft_link::WebLink::parse(url).map_or_else(String::new, |l| l.host().to_string());
    let (text, action) = match way(m)? {
        Way::OneClick { url } => (
            crate::i18n::fmt(
                tl!(
                    "MailCraft sends {host} the address this list gave for unsubscribing, which may identify you to it, and nothing else: no cookies, no sign-in. Like any connection, it also shows your internet address."
                ),
                &[("host", &host(&url))],
            ),
            tl!("Unsubscribe"),
        ),
        Way::Mail { to, .. } => {
            let from = app.session.mailbox.account(m.account).map_or_else(String::new, |a| a.email.clone());
            (
                crate::i18n::fmt(
                    tl!("A message asking to unsubscribe goes from {from} to {to}, through your Outbox."),
                    &[("from", &from), ("to", &to)],
                ),
                tl!("Send"),
            )
        }
        Way::Page { url } => (
            crate::i18n::fmt(
                tl!(
                    "This list unsubscribes on its own page, at {host}. It opens in your browser: visiting it tells the list that this address is read."
                ),
                &[("host", &host(&url))],
            ),
            tl!("Open the page"),
        ),
    };
    Some(crate::dialogs::Question::new(tl!("Unsubscribe"), text, action, Value::Null))
}

/// `ui.unsubscribe`: the window commands of this module (`UI_COMMANDS` lists them).
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    match id {
        "ui.unsubscribe" => Some(unsubscribe(app, p)),
        "ui.unsubscribeMany" => Some(many(app, p)),
        _ => None,
    }
}

/// The messages a `ui.unsubscribeMany` is about: `ids`, else the selected ones.
fn chosen(app: &MailApp, p: &Value) -> Vec<u64> {
    match p.get("ids").and_then(Value::as_array) {
        Some(ids) => ids.iter().filter_map(Value::as_u64).collect(),
        None => app.session.sel.messages.clone(),
    }
}

/// The question asked before `ui.unsubscribeMany`: how many lists, which way each goes, and
/// that a web page is opened one message at a time. `None` when none of them says how.
pub(crate) fn question_many(app: &MailApp, p: &Value) -> Option<crate::dialogs::Question> {
    let mut seen = std::collections::HashSet::new();
    let (mut click, mut mail, mut pages) = (0usize, 0usize, 0usize);
    for id in chosen(app, p) {
        let Some(m) = app.session.mailbox.messages.get(&id) else { continue };
        if !seen.insert(m.from.email.trim().to_lowercase()) {
            continue;
        }
        match way(m) {
            Some(Way::OneClick { .. }) => click += 1,
            Some(Way::Mail { .. }) => mail += 1,
            Some(Way::Page { .. }) => pages += 1,
            None => {}
        }
    }
    if click + mail + pages == 0 {
        return None;
    }
    let n = |count: usize| count.to_string();
    let text = crate::i18n::fmt(
        tl!(
            "Unsubscribe from {lists} lists: {click} in one click (each list's server is sent the address it gave, nothing else, one at a time), {mail} by a message from your account through your Outbox. {pages} unsubscribe on their own page: open each of those messages to do it there. Mail your server doesn't vouch for, or with a warning about its sender, is left out."
        ),
        &[("lists", &n(click + mail + pages)), ("click", &n(click)), ("mail", &n(mail)), ("pages", &n(pages))],
    );
    Some(crate::dialogs::Question::new(tl!("Unsubscribe"), text, tl!("Unsubscribe"), Value::Null))
}

/// `ui.unsubscribeMany`: each sender of the messages once, as the engine says
/// (`mail.unsubscribeMany`): the requests by mail to the Outbox at once, one click each in turn,
/// pages left to their messages' bars. What came of it is said in the status bar, and on each
/// message's bar.
fn many(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    poll(app);
    if app.unsubscribing.busy() {
        return Err(tl!("An unsubscribe is already on its way.").to_string());
    }
    let ids = chosen(app, p);
    if ids.is_empty() {
        return Err(tl!("Select a message first").to_string());
    }
    let told = app.session.run("mail.unsubscribeMany", &json!({"ids": ids, "send": true})).map_err(|e| e.to_string())?;
    let (mut click, mut mail, mut pages) = (0usize, 0usize, 0usize);
    for sender in told.get("senders").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = sender.get("message").and_then(Value::as_u64) else { continue };
        match sender.get("way").and_then(Value::as_str) {
            Some("oneClick") if app.unsubscribing.waiting.len() < MAX_KEPT => {
                let url = sender.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
                app.unsubscribing.said.remove(&id);
                app.unsubscribing.waiting.push_back((id, url));
                click += 1;
            }
            Some("mail") => {
                app.unsubscribing.say(id, Outcome::Queued);
                mail += 1;
            }
            Some("page") => pages += 1,
            _ => {}
        }
    }
    let left_out = told.get("leftOut").and_then(Value::as_array).map_or(0, Vec::len);
    poll(app);
    let n = |count: usize| count.to_string();
    app.status(crate::i18n::fmt(
        tl!("Unsubscribing: {click} in one click, {mail} by mail, {pages} to do on their own page, {left} left out."),
        &[("click", &n(click)), ("mail", &n(mail)), ("pages", &n(pages)), ("left", &n(left_out))],
    ));
    Ok(json!({"oneClick": click, "mail": mail, "pages": pages, "leftOut": left_out}))
}

fn unsubscribe(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    poll(app);
    let id = match p.get("message") {
        None | Some(Value::Null) => app.session.sel.focus().ok_or_else(|| tl!("Select a message first").to_string())?,
        Some(id) => id.as_u64().ok_or_else(|| "`message` is a message's id".to_string())?,
    };
    if app.unsubscribing.busy() {
        return Err(tl!("An unsubscribe is already on its way.").to_string());
    }
    let told = app.session.run("mail.unsubscribe", &json!({"id": id})).map_err(|e| e.to_string())?;
    let url = told.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
    match told.get("way").and_then(Value::as_str) {
        Some("oneClick") => {
            let Some(post) = &app.services.unsubscribe else { return Err(tl!("This build can't unsubscribe in one click.").to_string()) };
            app.unsubscribing.flying = Some((id, post(url)));
            app.unsubscribing.said.remove(&id);
            Ok(json!({"message": id, "way": "oneClick", "started": true}))
        }
        Some("mail") => {
            app.session.run("mail.unsubscribe", &json!({"id": id, "send": true})).map_err(|e| e.to_string())?;
            app.unsubscribing.say(id, Outcome::Queued);
            Ok(json!({"message": id, "way": "mail", "sent": true}))
        }
        Some("page") => {
            if !mailcraft_html::is_safe_url(&url) {
                return Err(tl!("This list's page isn't one that can be opened.").to_string());
            }
            let Some(ctx) = app.ctx.clone() else { return Err(tl!("The window isn't up yet.").to_string()) };
            ctx.open_url(egui::OpenUrl::new_tab(url.clone()));
            app.unsubscribing.say(id, Outcome::Opened);
            Ok(json!({"message": id, "way": "page", "url": url}))
        }
        _ => Err(tl!("This message doesn't say how to unsubscribe from it.").to_string()),
    }
}

#[cfg(test)]
#[path = "unsubscribe_tests.rs"]
mod tests;
