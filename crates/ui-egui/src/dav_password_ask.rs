// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Asking the owner for the password of an account's calendar servers (#209, D7): for servers
//! that have a password of their own, when an origin the settings name has none in this session
//! and the store gave none either.
//!
//! File › Info says so under the account, with Enter Password…; the dialog has one field for each
//! origin that wants one (calendars and contacts on one server share it). What is typed is never
//! a command's parameter: it is held for its origin ([`MailApp::hold_dav_password`]) and the
//! account's next job tries it at once. It is remembered only once a server took it, and only
//! when the owner lets sign-ins be remembered (`dav_secret_read.rs`).

use egui::Ui;
use mailcraft_model::Id;
use serde_json::json;

use crate::MailApp;
use crate::dialogs::{buttons, window};
use crate::theme::{Tokens, regular};

/// The dialog's state.
pub struct DavPasswordAsk {
    account: Id,
    /// The origins that want a password, each with what is typed and whether its eye is open.
    pub(crate) fields: Vec<(String, String, bool)>,
    /// Some passwords waited: the old one under the same name is still being removed.
    pub(crate) waiting: bool,
}

/// What became of the dialog this frame.
pub enum Outcome {
    Open,
    Close,
}

impl MailApp {
    /// The origins of the account's calendar servers that want a password of their own the owner
    /// hasn't given in this session, nor the store. Empty while the feature is off, or when they
    /// take the mail password.
    pub(crate) fn dav_origins_wanting_password(&self, account: Id) -> Vec<String> {
        if !self.session.dav_enabled {
            return Vec::new();
        }
        let Some(server) = self.session.mailbox.account(account).and_then(|a| a.dav.as_ref()).filter(|d| d.separate_password) else {
            return Vec::new();
        };
        server.home_origins().into_iter().filter(|origin| self.dav_password(account, origin).is_none()).collect()
    }
}

impl DavPasswordAsk {
    pub(crate) fn name(&self) -> &'static str {
        "davPassword"
    }

    /// For the account `account`, when its servers want a password. `None` otherwise.
    pub(crate) fn open(app: &MailApp, account: Id) -> Option<Self> {
        let origins = app.dav_origins_wanting_password(account);
        if origins.is_empty() {
            return None;
        }
        Some(Self { account, fields: origins.into_iter().map(|origin| (origin, String::new(), false)).collect(), waiting: false })
    }

    /// Hold what was typed, and have the account's servers tried with it now. A field that
    /// couldn't be held (the old password under its name is still being removed) keeps what was
    /// typed, and the others are emptied. Whether every one was held.
    pub(crate) fn give(&mut self, app: &mut MailApp) -> bool {
        let mut given = false;
        let mut all = true;
        for (origin, password, _) in &mut self.fields {
            if password.is_empty() {
                continue;
            }
            if app.hold_dav_password(self.account, origin, password) {
                given = true;
                password.clear();
            } else {
                all = false;
            }
        }
        if given {
            app.dav.trouble.remove(&self.account);
            app.dav.next = 0.0;
        }
        self.waiting = !all;
        all
    }
}

/// The host of an origin, as it is shown (`sync.example.org`, with its port when it has one).
fn host(origin: &str) -> &str {
    origin.strip_prefix("https://").unwrap_or(origin)
}

pub fn show(ctx: &egui::Context, d: &mut DavPasswordAsk, app: &mut MailApp, t: &Tokens, enter: bool) -> Outcome {
    let watch = app.secret_watch();
    let who = app.session.mailbox.account(d.account).map(|a| a.email.trim().to_string()).unwrap_or_default();
    let remember = app.ui.remember_sign_in;
    let (mut ok, mut cancel) = (false, false);
    let open = window(ctx, tl!("Calendar Server Password"), 420.0, |ui| {
        ui.label(crate::i18n::fmt(tl!("The calendars and contacts of {account} have a password of their own."), &[("account", &who)]));
        let room = egui::Margin::from(ui.spacing().button_padding);
        for (origin, password, shown) in &mut d.fields {
            ui.label(crate::i18n::fmt(tl!("Password for {server}"), &[("server", &crate::forms::kit::shown(host(origin), 80))]));
            let resp = crate::secret_field::field(ui, password, shown, watch, room, |edit| edit);
            crate::text_menu::hold(&resp);
        }
        let said = if remember {
            tl!("Once the server has taken it, MailCraft keeps it in your system's credential store, as File › Options lets it.")
        } else {
            tl!("MailCraft keeps it in memory until you close the app. It is never written to disk.")
        };
        ui.label(egui::RichText::new(said).color(t.text_dim).size(11.0));
        if d.waiting {
            ui.label(
                egui::RichText::new(tl!("MailCraft is still removing the old password for this server. Try OK again in a moment.")).color(t.red),
            );
        }
        let ready = d.fields.iter().any(|(_, password, _)| !password.is_empty());
        (ok, cancel) = buttons(ui, tl!("OK"), ready);
        ok |= enter && ready;
    });
    if ok && d.give(app) {
        return Outcome::Close;
    }
    if !open || cancel {
        return Outcome::Close;
    }
    Outcome::Open
}

/// The account's line in File › Info when its calendar servers want a password.
pub(crate) fn info_row(app: &mut MailApp, ui: &mut Ui, t: &Tokens, account: Id) {
    // One the store may still hold isn't wanted yet: it hasn't been read (the app started offline),
    // or the read is out.
    if app.dav_origins_wanting_password(account).iter().all(|origin| app.dav_secret_may_come(account, origin)) {
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(36.0);
        ui.label(egui::RichText::new(tl!("The calendar server needs its password.")).font(regular(11.5)).color(t.orange));
        let tip = tl!("Give the password of this account's calendars and contacts. Nothing is sent until the server asks for it.");
        if ui.link(tl!("Enter Password…")).on_hover_text(tip).clicked() {
            let _ = app.run("ui.dialog", json!({"name": "davPassword", "id": account}));
        }
    });
    ui.add_space(4.0);
}
