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

/// The one-click unsubscribe on its way, and what came of the last one.
#[derive(Default)]
pub(crate) struct Unsubscribing {
    /// The message, and the list server's answer to come.
    flying: Option<(u64, Receiver<Result<(), String>>)>,
    /// The message, and what came of it.
    said: Option<(u64, Outcome)>,
}

/// The answer of a one-click unsubscribe, once it has come.
fn poll(app: &mut MailApp) {
    let Some((id, rx)) = &app.unsubscribing.flying else { return };
    let id = *id;
    let answer = match rx.try_recv() {
        Ok(answer) => answer,
        Err(std::sync::mpsc::TryRecvError::Empty) => return,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(tl!("the request was lost").to_string()),
    };
    app.unsubscribing.flying = None;
    let said = match answer {
        Ok(()) => Outcome::Accepted,
        Err(why) => Outcome::Failed(why),
    };
    app.unsubscribing.said = Some((id, said));
}

/// Whether the engine would offer `m` a way to unsubscribe (its server vouches for it, and it has
/// no sender warning): the bar and the right-click menu offer it only then.
pub(crate) fn offered(app: &mut MailApp, m: &Message) -> bool {
    app.session.unsubscribe_way(m.id).is_ok()
}

/// The bar above a message from a mailing list: Unsubscribe, how it is going, or what came of it.
pub(super) fn bar(app: &mut MailApp, ui: &mut Ui, m: &Message, place: Place, t: &Tokens) {
    poll(app);
    let flying = app.unsubscribing.flying.as_ref().is_some_and(|(id, _)| *id == m.id);
    let said = app.unsubscribing.said.as_ref().filter(|(id, _)| *id == m.id).map(|(_, said)| said.clone());
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

/// The way this window takes: one click where it has a host to post with, else (the browser
/// build) the same address opened as a page.
fn shown(app: &MailApp, way: Way) -> Way {
    match way {
        Way::OneClick { url } if app.services.unsubscribe.is_none() => Way::Page { url },
        way => way,
    }
}

/// The question asked before `ui.unsubscribe` goes ahead: what will be sent, to whom, or opened.
/// `None` when the message offers nothing (the command then says why).
pub(crate) fn question(app: &MailApp, p: &Value) -> Option<crate::dialogs::Question> {
    let id = p.get("message").and_then(Value::as_u64).or_else(|| app.session.sel.focus())?;
    let m = app.session.mailbox.messages.get(&id)?;
    let host = |url: &str| mailcraft_link::WebLink::parse(url).map_or_else(String::new, |l| l.host().to_string());
    let (text, action) = match shown(app, way(app.session.mailbox.account(m.account), m)?) {
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
    (id == "ui.unsubscribe").then(|| unsubscribe(app, p))
}

fn unsubscribe(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    poll(app);
    let id = match p.get("message") {
        None | Some(Value::Null) => app.session.sel.focus().ok_or_else(|| tl!("Select a message first").to_string())?,
        Some(id) => id.as_u64().ok_or_else(|| "`message` is a message's id".to_string())?,
    };
    if app.unsubscribing.flying.is_some() {
        return Err(tl!("An unsubscribe is already on its way.").to_string());
    }
    let told = app.session.run("mail.unsubscribe", &json!({"id": id})).map_err(|e| e.to_string())?;
    let url = told.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
    // Without a host to post with (the browser build), one click is the same address opened as a page.
    let way = match told.get("way").and_then(Value::as_str) {
        Some("oneClick") if app.services.unsubscribe.is_none() => Some("page"),
        way => way,
    };
    match way {
        Some("oneClick") => {
            let Some(post) = &app.services.unsubscribe else { return Err(tl!("This build can't unsubscribe in one click.").to_string()) };
            app.unsubscribing.flying = Some((id, post(url)));
            app.unsubscribing.said = None;
            Ok(json!({"message": id, "way": "oneClick", "started": true}))
        }
        Some("mail") => {
            app.session.run("mail.unsubscribe", &json!({"id": id, "send": true})).map_err(|e| e.to_string())?;
            app.unsubscribing.said = Some((id, Outcome::Queued));
            Ok(json!({"message": id, "way": "mail", "sent": true}))
        }
        Some("page") => {
            if !mailcraft_html::is_safe_url(&url) {
                return Err(tl!("This list's page isn't one that can be opened.").to_string());
            }
            let Some(ctx) = app.ctx.clone() else { return Err(tl!("The window isn't up yet.").to_string()) };
            ctx.open_url(egui::OpenUrl::new_tab(url.clone()));
            app.unsubscribing.said = Some((id, Outcome::Opened));
            Ok(json!({"message": id, "way": "page", "url": url}))
        }
        _ => Err(tl!("This message doesn't say how to unsubscribe from it.").to_string()),
    }
}

#[cfg(test)]
#[path = "unsubscribe_tests.rs"]
mod tests;
