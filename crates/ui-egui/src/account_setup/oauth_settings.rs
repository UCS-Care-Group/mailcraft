// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The OAuth settings of an account that signs in on its provider's page: every value its sign-in
//! uses, shown, and those that are the account's own, entered, saved and edited.
//!
//! Microsoft's and Google's values are built in ([`OAuthProvider::defaults`]): they are shown as
//! they are used and only the client id, Microsoft's tenant and the redirect port can change. To
//! use other addresses, scopes or a mechanism the account becomes one with its own identity
//! provider, after a question, starting from the values it had: so that what may be sent to
//! Microsoft's and Google's servers is never loosened from here.
//!
//! An own identity provider (Keycloak, Authentik and the like, in front of the owner's own IMAP
//! and SMTP servers) has every value typed, or its addresses found from its issuer (Discover:
//! only the issuer's address goes out, and only when the owner presses it). Its client secret,
//! when it has one, is typed here and kept in the system's credential store alone; the mailbox
//! says only that there is one. Saving goes through `account.add` and `account.edit`.

use mailcraft_model::{OAuthProvider, SaslChoice};

use super::*;

/// What the OAuth settings keep beside the account's [`OAuth`] while the dialog is open.
#[derive(Default)]
pub(super) struct OwnFields {
    /// The redirect port as typed; empty for any free one.
    pub(super) port: String,
    /// A client secret typed here, to keep in place of the one there is; empty for no change.
    secret: Secret,
    /// Its eye was clicked: it is shown.
    reveal: bool,
    /// The owner asked to forget the client secret there is.
    forget_secret: bool,
    /// The identity provider's description, while it is fetched.
    discovering: Option<Receiver<Result<crate::OAuthFound, String>>>,
    /// What Discover found, said in a line; or why it found nothing.
    found: String,
    /// The hosts of the found addresses outside the issuer's own domain, and whether the owner
    /// agreed to them.
    outside: Vec<String>,
    agreed: bool,
    /// The question whether a Microsoft or Google account is to use its own identity provider.
    asking: bool,
    /// The owner said yes: saving sets it up for its own identity provider (`asCustom`).
    pub(super) as_custom: bool,
    /// The identity provider the domain's own document names, when it does: offered.
    pub(super) offered: String,
}

impl OwnFields {
    pub(super) fn of(oauth: Option<&OAuth>) -> OwnFields {
        OwnFields { port: oauth.and_then(|o| o.redirect_port).map(|p| p.to_string()).unwrap_or_default(), ..OwnFields::default() }
    }

    /// Nothing typed yet, with the identity provider the domain's document named offered.
    pub(super) fn offering(issuer: String) -> OwnFields {
        OwnFields { offered: issuer, ..OwnFields::default() }
    }

    /// The client secret typed, if any.
    pub(super) fn typed_secret(&self) -> &str {
        &self.secret.0
    }

    /// Blank what was typed of the secret.
    pub(super) fn wipe(&mut self) {
        self.secret.wipe();
    }
}

/// The redirect port as typed: `Ok(None)` for any free one.
fn port(text: &str) -> Result<Option<u16>, ()> {
    match text.trim() {
        "" => Ok(None),
        digits => digits.parse::<u16>().ok().filter(|p| *p != 0).map(Some).ok_or(()),
    }
}

impl AccountSetup {
    /// Set the account up to sign in with an own identity provider, from nothing (or the issuer
    /// the domain's document offered, when the owner takes it).
    pub(super) fn choose_own_provider(&mut self, issuer: &str) {
        self.forget_password();
        self.oauth = Some(OAuth { issuer: issuer.to_string(), ..OAuth::custom() });
        self.client_id.clear();
        let offered = std::mem::take(&mut self.own.offered);
        self.own = OwnFields { offered, ..OwnFields::default() };
        self.error.clear();
    }

    /// The account's sign-in settings as the fields have them now.
    pub(super) fn oauth_now(&self) -> Option<OAuth> {
        let mut oauth = self.oauth.clone()?;
        oauth.client_id = self.client_id.trim().to_string();
        if oauth.provider == OAuthProvider::Microsoft {
            oauth.tenant = self.tenant.trim().to_string();
        }
        oauth.redirect_port = port(&self.own.port).unwrap_or(oauth.redirect_port);
        if oauth.provider == OAuthProvider::Custom {
            oauth.has_secret = !self.own.typed_secret().is_empty() || (oauth.has_secret && !self.own.forget_secret);
        }
        Some(oauth)
    }

    /// What keeps the OAuth settings from being saved, in the interface's language.
    pub(super) fn oauth_problem(&self) -> Option<String> {
        let oauth = self.oauth_now()?;
        if port(&self.own.port).is_err() {
            return Some(tl!("The redirect port is a number from 1 to 65535, or empty for any free one").to_string());
        }
        if self.own.discovering.is_some() {
            return Some(tl!("Wait for your identity provider's addresses").to_string());
        }
        if !self.own.outside.is_empty() && !self.own.agreed {
            return Some(tl!("Agree to the addresses outside your identity provider's domain, or enter others").to_string());
        }
        let secret = self.own.typed_secret();
        if secret.len() > 1024 || !secret.bytes().all(|b| b.is_ascii_graphic()) {
            return Some(tl!("The client secret can only be letters, digits and punctuation").to_string());
        }
        oauth.check().err().map(|why| why.to_string())
    }

    /// What signing in needs that the settings don't have yet, before a new account's sign-in.
    pub(super) fn own_sign_in_problem(&self) -> Option<String> {
        if let Some(problem) = self.oauth_problem() {
            return Some(problem);
        }
        let oauth = self.oauth_now()?;
        oauth.ready().err().map(|_| tl!("Find your identity provider's addresses with Discover, or enter them").to_string())
    }

    /// The OAuth settings, as `account.add` and `account.edit` take them.
    pub(super) fn oauth_params(&self, m: &mut Map<String, Value>) {
        let Some(oauth) = self.oauth_now() else { return };
        m.insert("redirectPort".into(), json!(oauth.redirect_port));
        if oauth.provider != OAuthProvider::Custom {
            return;
        }
        m.insert("clientId".into(), json!(oauth.client_id));
        if self.own.as_custom {
            m.insert("asCustom".into(), json!(true));
        }
        for (key, value) in [
            ("issuer", &oauth.issuer),
            ("authorizeUrl", &oauth.authorize),
            ("tokenUrl", &oauth.token),
            ("revokeUrl", &oauth.revoke),
            ("scopes", &oauth.scopes),
        ] {
            m.insert(key.into(), json!(value.trim()));
        }
        m.insert("mechanism".into(), json!(oauth.mechanism.code()));
        m.insert("hasSecret".into(), json!(oauth.has_secret));
    }

    /// Keep the client secret typed (or forget the one there is) for `account`, once the
    /// settings that name it are saved (0: the account being set up, until it exists).
    pub(super) fn keep_client_secret(&mut self, app: &mut MailApp, account: Id) {
        if !self.oauth.as_ref().is_some_and(|o| o.provider == OAuthProvider::Custom) {
            return;
        }
        if !self.own.typed_secret().is_empty() {
            let secret = self.own.typed_secret().to_string();
            app.set_client_secret(account, &secret);
            self.own.wipe();
        } else if self.own.forget_secret {
            app.set_client_secret(account, "");
            self.own.forget_secret = false;
        }
    }

    /// Take in what Discover found, once it has.
    pub(super) fn poll_discover(&mut self) {
        let answer = match self.own.discovering.as_ref().map(Receiver::try_recv) {
            Some(Err(TryRecvError::Empty)) | None => return,
            Some(Ok(answer)) => answer,
            Some(Err(TryRecvError::Disconnected)) => Err(tl!("Finding the addresses stopped unexpectedly").to_string()),
        };
        self.own.discovering = None;
        let Some(oauth) = self.oauth.as_mut().filter(|o| o.provider == OAuthProvider::Custom) else { return };
        match answer {
            Ok(found) => {
                (oauth.authorize, oauth.token, oauth.revoke) = (line(&found.authorize, 2048), line(&found.token, 2048), line(&found.revoke, 2048));
                let host = |address: &str| address.split("://").nth(1).and_then(|rest| rest.split(['/', '?', ':']).next()).unwrap_or("").to_string();
                let mut said = fmt(
                    tl!("Found: sign-in at {signIn}, tokens from {token}."),
                    &[("signIn", &shown(&host(&oauth.authorize), 80)), ("token", &shown(&host(&oauth.token), 80))],
                );
                if found.no_refresh {
                    said.push(' ');
                    said.push_str(tl!("It hands out no refresh tokens: you'll be asked to sign in again each time a sign-in runs out."));
                }
                self.own.found = said;
                self.own.outside = found.outside.iter().take(8).map(|h| line(h, 253)).collect();
                self.own.agreed = false;
            }
            Err(why) => self.own.found = fmt(tl!("No addresses found: {reason}"), &[("reason", &shown(&why, 200))]),
        }
    }

    /// The OAuth settings section of a page, for an account that signs in on its provider's page.
    pub(super) fn oauth_section(&mut self, app: &MailApp, ui: &mut Ui, pass: &mut Pass) {
        let t = pass.t;
        let Some(provider) = self.oauth.as_ref().map(|o| o.provider) else { return };
        caption(ui, tl!("OAuth settings"), &t);
        match provider {
            OAuthProvider::Custom => self.own_provider_fields(app, ui, pass),
            built_in => self.built_in_fields(ui, pass, built_in),
        }
        // The servers the token is for: never chosen here, they follow the server settings.
        let servers: Vec<String> = match provider {
            OAuthProvider::Custom => {
                let mut hosts: Vec<String> =
                    [self.incoming.host.trim(), self.outgoing.host.trim()].into_iter().filter(|h| !h.is_empty()).map(|h| shown(h, 80)).collect();
                hosts.dedup();
                hosts
            }
            other => other.defaults("").hosts,
        };
        let servers = if servers.is_empty() { tl!("the incoming and outgoing servers above").to_string() } else { servers.join(", ") };
        note(ui, &fmt(tl!("The sign-in is only sent to: {servers}"), &[("servers", &servers)]), t.text_dim);
    }

    /// Microsoft's or Google's built-in values, shown; the client id, tenant and port, edited.
    fn built_in_fields(&mut self, ui: &mut Ui, pass: &mut Pass, provider: OAuthProvider) {
        let t = pass.t;
        let profile = if provider == OAuthProvider::Google { tl!("Google (built in)") } else { tl!("Microsoft 365 or Outlook.com (built in)") };
        note(ui, profile, t.text);
        let used = provider.defaults(&self.tenant);
        for (label, value) in [
            (tl!("Identity provider"), used.issuer.as_str()),
            (tl!("Sign-in address"), used.authorize.as_str()),
            (tl!("Token address"), used.token.as_str()),
            (tl!("Scopes"), used.scopes.as_str()),
            (tl!("Mechanism"), used.mechanism.code()),
        ] {
            note(ui, &format!("{label}: {}", shown(value, 200)), t.text_dim);
        }
        field(ui, pass, "clientId", tl!("Application (client) ID"), &mut self.client_id, tl!("The one in File › Options"), 200);
        if provider == OAuthProvider::Microsoft {
            field(ui, pass, "tenant", tl!("Tenant"), &mut self.tenant, "common", 64);
        }
        if !self.own.asking {
            if button(ui, tl!("Use your own identity provider instead…"), false, true).clicked() {
                self.own.asking = true;
            }
            return;
        }
        let question = fmt(
            tl!(
                "These settings will no longer be {provider}'s: MailCraft will treat this account as one with your own identity provider, starting from the values above, and you'll sign in again."
            ),
            &[("provider", provider.label())],
        );
        note(ui, &question, t.orange);
        let (mut yes, mut no) = (false, false);
        ui.horizontal(|ui| {
            yes = button(ui, tl!("Use my own identity provider"), true, true).clicked();
            no = button(ui, tl!("Keep the built-in settings"), false, true).clicked();
        });
        if yes {
            let mut oauth = self.oauth_now().unwrap_or_default();
            let used = oauth.effective();
            let extra = used
                .scopes
                .split_ascii_whitespace()
                .filter(|s| !mailcraft_model::oauth::CUSTOM_SCOPE.split(' ').any(|c| c == *s))
                .collect::<Vec<_>>()
                .join(" ");
            oauth = OAuth {
                provider: OAuthProvider::Custom,
                client_id: used.client_id.clone(),
                issuer: used.issuer,
                authorize: used.authorize,
                token: used.token,
                revoke: used.revoke,
                scopes: extra,
                mechanism: used.mechanism,
                redirect_port: oauth.redirect_port,
                ..OAuth::default()
            };
            self.client_id = used.client_id;
            self.oauth = Some(oauth);
            (self.own.as_custom, self.own.asking) = (true, false);
        } else if no {
            self.own.asking = false;
        }
    }

    /// An own identity provider's values, every one entered here.
    fn own_provider_fields(&mut self, app: &MailApp, ui: &mut Ui, pass: &mut Pass) {
        let t = pass.t;
        note(ui, tl!("Your own identity provider"), t.text);
        if !self.own.offered.is_empty() && self.oauth.as_ref().is_some_and(|o| o.issuer.trim().is_empty()) {
            note(ui, &fmt(tl!("Your domain names {issuer} for signing in."), &[("issuer", &shown(&self.own.offered, 120))]), t.text_dim);
            if button(ui, tl!("Use it"), false, true).clicked()
                && let Some(oauth) = self.oauth.as_mut()
            {
                oauth.issuer = self.own.offered.clone();
            }
        }
        let has_secret = self.oauth.as_ref().is_some_and(|o| o.has_secret);
        let Some(oauth) = self.oauth.as_mut() else { return };
        field(ui, pass, "issuer", tl!("Identity provider (issuer)"), &mut oauth.issuer, "https://id.example.org/realms/mail", 2048);
        let can_discover = app.services.oauth_discover.is_some() && !oauth.issuer.trim().is_empty() && self.own.discovering.is_none();
        let mut discover = false;
        ui.horizontal(|ui| {
            discover = button(ui, tl!("Discover"), false, can_discover)
                .on_hover_text(tl!("Find the sign-in and token addresses from the identity provider's address. Only that address is sent."))
                .clicked();
            if self.own.discovering.is_some() {
                ui.spinner();
            }
        });
        if discover && let Some(start) = app.services.oauth_discover.as_ref() {
            self.own.discovering = Some(start(oauth.issuer.trim().to_string()));
            self.own.found.clear();
        }
        if !self.own.found.is_empty() {
            note(ui, &self.own.found, t.text_dim);
        }
        if !self.own.outside.is_empty() {
            let hosts = self.own.outside.join(", ");
            note(ui, &fmt(tl!("These addresses are outside your identity provider's domain: {hosts}"), &[("hosts", &hosts)]), t.orange);
            ui.checkbox(&mut self.own.agreed, tl!("I trust these addresses"));
        }
        let Some(oauth) = self.oauth.as_mut() else { return };
        let before = (oauth.authorize.clone(), oauth.token.clone());
        field(ui, pass, "authorizeUrl", tl!("Sign-in address"), &mut oauth.authorize, "https://…/auth", 2048);
        field(ui, pass, "tokenUrl", tl!("Token address"), &mut oauth.token, "https://…/token", 2048);
        // (Addresses typed by hand are the owner's own: nothing to agree to.)
        if before != (oauth.authorize.clone(), oauth.token.clone()) {
            self.own.outside.clear();
        }
        field(ui, pass, "revokeUrl", tl!("Revocation address (optional)"), &mut oauth.revoke, "https://…/revoke", 2048);
        field(ui, pass, "clientId", tl!("Client ID"), &mut self.client_id, tl!("The one registered for MailCraft"), 200);
        caption(ui, tl!("Client secret (optional)"), &t);
        let hint =
            if has_secret && !self.own.forget_secret { tl!("Kept: type a new one to replace it") } else { tl!("Only if your provider gave one") };
        let resp = secret_field::field(ui, &mut self.own.secret.0, &mut self.own.reveal, app.secret_watch(), FIELD_MARGIN, |edit| {
            edit.id(field_id(("account", "clientSecret"))).char_limit(1024).hint_text(hint.to_owned())
        });
        pass.field(&resp);
        if has_secret {
            ui.checkbox(&mut self.own.forget_secret, tl!("Forget the client secret"));
        }
        note(
            ui,
            tl!("Kept in your system's credential store, never in the mailbox. A desktop program can't really keep it secret from you."),
            t.text_dim,
        );
        let Some(oauth) = self.oauth.as_mut() else { return };
        field(ui, pass, "scopes", tl!("Extra scopes"), &mut oauth.scopes, tl!("Asked for besides openid email offline_access"), 1024);
        field(ui, pass, "redirectPort", tl!("Redirect port"), &mut self.own.port, tl!("Automatic"), 5);
        note(ui, tl!("Register http://127.0.0.1 with any port (or the one above) as the redirect address."), t.text_dim);
        caption(ui, tl!("Mechanism"), &t);
        let label = |choice: SaslChoice| match choice {
            SaslChoice::Auto => tl!("Automatic (OAUTHBEARER, else XOAUTH2)"),
            SaslChoice::OAuthBearer => "OAUTHBEARER",
            SaslChoice::XOAuth2 => "XOAUTH2",
        };
        egui::ComboBox::from_id_salt(field_id(("account", "mechanism"))).selected_text(label(oauth.mechanism)).show_ui(ui, |ui| {
            for choice in [SaslChoice::Auto, SaslChoice::OAuthBearer, SaslChoice::XOAuth2] {
                ui.selectable_value(&mut oauth.mechanism, choice, label(choice));
            }
        });
    }
}
