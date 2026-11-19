// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Zimbra account of Add Account and of Account Settings: chosen by the owner from its tile
//! (only while Zimbra accounts are being tested, `MAILCRAFT_ZIMBRA=1`).
//!
//! A Zimbra account has no mail servers: no IMAP, no SMTP. It is the address of the server's web
//! client, the name it signs in with (the address, unless the server knows the owner by another)
//! and the password. MailCraft speaks to it through Zimbra's own web service over HTTPS: its SOAP
//! API (in JSON) and its REST addresses for whole messages, as Zimbra Collaboration 8.8 and later
//! have them. **Nothing is sent until Connect**: the account is added then, and its first
//! Send/Receive signs in; an account with a second step asks for the code of the authenticator app
//! at that point.

use mailcraft_model::ZimbraServer;

use super::*;

/// The most characters the server's address takes.
const URL: usize = ZimbraServer::MAX_URL;

/// What the Zimbra page keeps.
pub(super) struct Zimbra {
    /// The address of the server's web client, as typed.
    pub(super) url: String,
    /// Its calendars and contacts over CalDAV/CardDAV at the same address are wanted, offered only
    /// while that way of keeping them is being tested (`MAILCRAFT_DAV=1`). On while offered.
    pub(super) calendar: bool,
    pub(super) contacts: bool,
}

impl Default for Zimbra {
    fn default() -> Self {
        Self { url: String::new(), calendar: true, contacts: true }
    }
}

/// The address as it is typed, made one MailCraft takes: `https://` in front of a bare host name.
fn typed_url(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() || text.contains("://") { text.to_string() } else { format!("https://{text}") }
}

/// What the page says about the way MailCraft connects, and what the server must be.
pub(super) fn technology_note(ui: &mut Ui, t: &Tokens) {
    note(
        ui,
        tl!(
            "MailCraft connects to Zimbra through its own web service over HTTPS: the SOAP API (in JSON) and REST, not IMAP or SMTP. It needs Zimbra Collaboration 8.8 or later."
        ),
        t.text_dim,
    );
}

impl AccountSetup {
    /// Off to the Zimbra page, with the address a probe found when there is one, else the
    /// domain's `mail.` host as the guess.
    pub(super) fn zimbra_page_from(&mut self) {
        if self.zimbra.url.trim().is_empty() {
            self.zimbra.url = self
                .zimbra_found
                .first()
                .map(|host| format!("https://{host}"))
                .or_else(|| address(&self.email).map(|(_, domain)| format!("https://mail.{}", domain.to_ascii_lowercase())))
                .unwrap_or_default();
        }
        self.settings_open = true;
        self.go(Page::Zimbra);
    }

    /// Why the Zimbra page can't be connected yet, in the interface language; nothing when it can.
    fn zimbra_problem(&self) -> Option<String> {
        let url = typed_url(&self.zimbra.url);
        if url.is_empty() {
            return Some(tl!("Type the address of your Zimbra server, as your browser opens its web client").to_string());
        }
        if let Err(why) = ZimbraServer::check_url(&url) {
            return Some(fmt(tl!("The server's address {why}"), &[("why", tl!(why))]));
        }
        if let Err(why) = ZimbraServer::check_username(self.username.trim()) {
            return Some(fmt(tl!("The user name {why}"), &[("why", tl!(why))]));
        }
        None
    }

    /// The principal the server keeps the account's calendars and contacts under
    /// (`/dav/<address>` on the web client's origin): `None` when either can't be made.
    fn dav_home(&self) -> Option<String> {
        let (email, _) = address(&self.email)?;
        let origin = typed_url(&self.zimbra.url).trim_end_matches('/').to_string();
        (!email.is_empty() && ZimbraServer::check_url(&origin).is_ok()).then(|| format!("{origin}/dav/{email}"))
    }

    /// `account.add` for the Zimbra account the page describes.
    pub(super) fn zimbra_command(&self) -> Option<(&'static str, Value)> {
        let (email, _) = address(&self.email)?;
        self.zimbra_problem().is_none().then(|| {
            let server = json!({"url": typed_url(&self.zimbra.url), "username": self.username.trim()});
            ("account.add", json!({"email": email, "displayName": self.name.trim(), "kind": "zimbra", "zimbra": server}))
        })
    }

    /// Connect on the Zimbra page: add the account, keep the password for this session, and start
    /// its first Send/Receive, which signs in.
    pub(super) fn zimbra_connect(&mut self, app: &mut MailApp) {
        if let Some(problem) = self.zimbra_problem() {
            self.error = problem;
            return;
        }
        let added = match self.zimbra_command() {
            Some((command, params)) => app.execute(command, params),
            None => Err(self.not_an_address()),
        };
        match added.map(|v| v.get("id").and_then(Value::as_u64)) {
            Ok(Some(id)) => {
                let password = self.password.take();
                if !password.is_empty() {
                    app.adopt_password(id, password);
                }
                // Calendars and contacts over DAV at the server's own address, asked for on the
                // page (`/dav/<address>` is Zimbra's principal): kept with the account before its
                // first run, so the run can keep to them too.
                if app.session.dav_enabled
                    && (self.zimbra.calendar || self.zimbra.contacts)
                    && let Some(home) = self.dav_home()
                {
                    let homes = |on: bool| if on { vec![home.clone()] } else { Vec::new() };
                    let _ = app.execute(
                        "account.davSet",
                        json!({"id": id, "calendarHomes": homes(self.zimbra.calendar), "contactHomes": homes(self.zimbra.contacts)}),
                    );
                }
                self.finish(app, id);
            }
            Ok(None) => self.error = tl!("The account couldn't be added").to_string(),
            Err(e) => self.error = e,
        }
    }

    /// The Zimbra page: the password, and the server's address and the user name.
    pub(super) fn zimbra_form(&mut self, app: &MailApp, ui: &mut Ui, pass: &mut Pass) -> Option<Action> {
        let t = pass.t;
        page(ui, &t, |ui| {
            heading(ui, tl!("Zimbra account"), &t);
            note(ui, &shown(self.email.trim(), 120), t.text_dim);
            technology_note(ui, &t);
            self.password_field(ui, pass, app.secret_watch(), "");
            note(ui, kept_where(app), t.text_dim);
            note(
                ui,
                tl!("If your account signs in with a second step, MailCraft asks for the code of your authenticator app when it first connects."),
                t.text_dim,
            );
            self.zimbra_fields(ui, pass);
            // The same address serves calendars and contacts over DAV, while that way of keeping
            // them is being tested: asked for on the page they belong to, not in the mail's.
            if app.session.dav_enabled {
                ui.checkbox(&mut self.zimbra.calendar, tl!("Use this server's calendar (CalDAV)"));
                ui.checkbox(&mut self.zimbra.contacts, tl!("Use this server's contacts (CardDAV)"));
            }
        });
        problem(ui, &self.error);
        let ready = !self.password.0.is_empty() && self.zimbra_problem().is_none();
        let (mut go, mut back) = (false, false);
        strip(ui, self.page, |ui| {
            go = button(ui, tl!("Connect"), true, ready).clicked();
            back = button(ui, tl!("Back"), false, true).clicked();
        });
        if back { Some(Action::Back) } else { (ready && (go || pass.submit)).then_some(Action::Go) }
    }

    /// The server's address and the name to sign in with, on the Zimbra page and in Account
    /// Settings.
    pub(super) fn zimbra_fields(&mut self, ui: &mut Ui, pass: &mut Pass) {
        let t = pass.t;
        if field(ui, pass, "zimbraUrl", tl!("Zimbra server address (web client, HTTPS)"), &mut self.zimbra.url, "https://mail.example.org", URL)
            .changed()
        {
            self.error.clear();
        }
        note(ui, tl!("The address your browser opens Zimbra's web client at. It must begin with https://."), t.text_dim);
        field(ui, pass, "username", tl!("User name"), &mut self.username, tl!("Usually the whole address; empty for your address"), LINE);
    }

    /// The fields Account Settings changes of a Zimbra account, as `account.edit` takes them.
    pub(super) fn zimbra_fields_map(&self) -> Map<String, Value> {
        object(json!({
            "name": self.account_name.trim(),
            "displayName": self.name.trim(),
            "zimbra": {"url": typed_url(&self.zimbra.url), "username": self.username.trim()},
        }))
    }

    /// Why a Zimbra account's settings can't be saved, when they can't.
    pub(super) fn zimbra_settings_problem(&self) -> Option<String> {
        self.zimbra_problem()
    }
}

#[cfg(test)]
#[path = "zimbra_tests.rs"]
mod tests;
