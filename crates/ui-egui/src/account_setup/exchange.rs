// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Exchange server account of Add Account (#252, E7): an explicit choice, offered only while
//! Exchange server accounts are being tested (`Session::ews`). There is no detection from an
//! address.
//!
//! The owner gives an address and a password, and may open Server settings to type the server's
//! address and the name to sign in with. **Nothing is sent until Connect.** Then the host
//! ([`crate::Services::ews_setup`]) does the work on a thread of its own and the wizard shows
//! what it asks and what comes of it:
//!
//! - a question, when the search would send the sign-in to a host outside the owner's domain
//!   (`Page::ExchangeConsent`): no password goes to that host before a yes;
//! - the server's certificate, when the system doesn't trust it (`Page::ExchangeCertificate`): its
//!   fingerprint, who it was issued to and by, and its dates, and a box to trust this one
//!   certificate for this account. A certificate that is expired, not valid yet or for another
//!   name is never offered: the page says why;
//! - a mailbox in Microsoft 365 (`Page::ExchangeOnline`): it is a Microsoft account, signed in on
//!   Microsoft's own page; no password was sent there;
//! - a sentence the owner can act on when it didn't work, with Server settings open.
//!
//! The account is added by `account.add` with `kind: "exchange"` once its Inbox could be had.
//! A certificate that changed later is asked about again from File › Info (`review`): the host
//! looks at the certificate alone, so no password is sent while the owner decides.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use mailcraft_model::EwsServer;

use super::*;
use crate::ews_setup::{EwsCertificate, EwsCertificateFault, EwsConsentReason, EwsSetupEnd, EwsSetupEvent, EwsSetupFailure, EwsSetupJob};

/// The most characters the server's address takes.
const URL: usize = EwsServer::MAX_URL;

/// What the Exchange server pages keep.
#[derive(Default)]
pub(super) struct Exchange {
    /// The server's address as typed in Server settings; empty to search for it.
    pub(super) url: String,
    /// What the host tells while its job is under way.
    events: Option<Receiver<EwsSetupEvent>>,
    /// The owner's answers to the host's questions.
    answers: Option<Sender<bool>>,
    /// Set when the owner gives up.
    cancel: Arc<AtomicBool>,
    /// The host the owner is asked about, and why.
    consent: Option<(String, EwsConsentReason)>,
    /// The certificate the owner is asked about.
    certificate: Option<EwsCertificate>,
    /// The box "Trust this certificate for this account" is ticked.
    ticked: bool,
    /// The fingerprint the owner has agreed to; sent with the next test and kept on the account.
    pub(super) trusted: String,
    /// The account whose certificate is being looked at again (from File › Info); 0 when the
    /// wizard is adding one.
    pub(super) review: Id,
    /// The look at that certificate has been handed to the host.
    started: bool,
}

impl Exchange {
    /// Whether the host owes an answer (frames keep coming until it is in), or a look at a
    /// certificate is still to be started.
    pub(super) fn pending(&self) -> bool {
        self.events.is_some() || (self.review != 0 && !self.started)
    }
}

impl Drop for Exchange {
    /// However the dialog goes, the host's job is called off.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The 64 hexadecimal digits of a fingerprint in pairs, `ab:cd:…`, in two lines of sixteen pairs.
pub(crate) fn grouped(fingerprint: &str) -> String {
    let pairs: Vec<&str> = (0..fingerprint.len() / 2).filter_map(|i| fingerprint.get(i * 2..i * 2 + 2)).collect();
    pairs.chunks(16).map(|line| line.join(":")).collect::<Vec<_>>().join("\n")
}

/// A moment as a date for the certificate page.
fn day_of(timestamp: i64) -> String {
    crate::dates::medium(mailcraft_time::DateTime::from_timestamp(timestamp, 0).date)
}

/// The words for a setup that didn't work: what happened and what to do.
pub(super) fn failure_words(failure: EwsSetupFailure) -> String {
    match failure {
        EwsSetupFailure::Unreachable => {
            tl!("No server answered at the usual places for this address. Check the network connection, or type the server's address in Server settings.")
        }
        EwsSetupFailure::NoAutodiscover => tl!("This domain's servers have no automatic setup. Type the server's address in Server settings."),
        EwsSetupFailure::SecureConnection => {
            tl!("A secure connection to the server couldn't be set up. Type the server's address in Server settings.")
        }
        EwsSetupFailure::Unreadable => tl!("The server's answer for automatic setup couldn't be read. Type the server's address in Server settings."),
        EwsSetupFailure::InsecureRedirect => {
            tl!("The server sent the setup somewhere that isn't encrypted, which MailCraft never follows. Type the server's address in Server settings.")
        }
        EwsSetupFailure::ServerRefused => {
            tl!("The server or the organisation wouldn't give its settings. Ask your administrator for the server's address and type it in Server settings.")
        }
        EwsSetupFailure::NoExchangeAddress => {
            tl!("The server answered without the address of Exchange Web Services. Ask your administrator for it and type it in Server settings.")
        }
        EwsSetupFailure::SignInNotSupported => {
            tl!("MailCraft can't sign in to this server this way yet (it wants Kerberos). Ask your administrator whether it can take a user name and password over an encrypted connection.")
        }
        EwsSetupFailure::SignInNtlm => tl!("Automatic setup can't sign in to this server. Type its address in Server settings and MailCraft will sign in with NTLM."),
        EwsSetupFailure::Declined => tl!("You didn't agree to send your sign-in to the server the setup led to. Type the server's address in Server settings."),
        EwsSetupFailure::SignInRefused => {
            tl!("The server didn't accept the user name and password. Check them, or type your user name in Server settings (DOMAIN\\user or you@example.org).")
        }
        EwsSetupFailure::TooManyRedirects => tl!("The setup was sent from one server to another too many times. Type the server's address in Server settings."),
        EwsSetupFailure::NotAllowed => {
            tl!("Your organisation doesn't allow Exchange Web Services for this account. Ask your administrator.")
        }
        EwsSetupFailure::NotExchange => {
            tl!("The server answered, but not as an Exchange server does. Check its address in Server settings.")
        }
        EwsSetupFailure::BadSettings => tl!("The server's address or the user name can't be used. Check them in Server settings."),
        EwsSetupFailure::Certificate(EwsCertificateFault::Expired) => {
            tl!("The server's certificate has expired, so MailCraft won't connect to it. Ask your administrator to renew it.")
        }
        EwsSetupFailure::Certificate(EwsCertificateFault::NotYetValid) => {
            tl!("The server's certificate isn't valid yet, so MailCraft won't connect to it. Check this computer's date and time, or ask your administrator.")
        }
        EwsSetupFailure::Certificate(EwsCertificateFault::WrongName) => {
            tl!("The server's certificate is for another server's name, so MailCraft won't connect to it. Check the address in Server settings.")
        }
        EwsSetupFailure::Other => tl!("The account couldn't be set up. Check the details, or type the server's address in Server settings."),
    }
    .to_string()
}

impl AccountSetup {
    // ---- starting ----

    /// The Exchange server choice was picked: the page that asks for the password and, if the owner
    /// opens them, the server's address and the user name.
    pub(super) fn exchange_page_from(&mut self) {
        self.picked = true;
        self.exchange = Exchange::default();
        self.oauth = None;
        self.service.clear();
        self.note.clear();
        self.settings_open = false;
        self.go(Page::Exchange);
    }

    /// What keeps the page's fields from being used, in the interface language; nothing when they can.
    fn exchange_problem(&self) -> Option<String> {
        if address(&self.email).is_none() {
            return Some(self.not_an_address());
        }
        let url = self.exchange.url.trim();
        // (A Microsoft host is handed over, not refused as a bad address: see `exchange_connect`.)
        if !url.is_empty() && EwsServer::check_url(url).is_err() && !self.typed_microsoft() {
            return Some(
                tl!("The server's address must begin with https:// and name the server (https://mail.example.org/EWS/Exchange.asmx)").to_string(),
            );
        }
        if EwsServer::check_username(self.username.trim()).is_err() {
            return Some(tl!("The user name is too long or has a character in it that a name can't have").to_string());
        }
        None
    }

    /// Whether the address typed in Server settings is on one of Microsoft's mail hosts.
    fn typed_microsoft(&self) -> bool {
        let server = EwsServer { url: self.exchange.url.trim().to_string(), ..Default::default() };
        server.host().is_some_and(mailcraft_model::is_exchange_online)
    }

    /// Connect: have the host search for the server (or use the one typed) and test the account.
    fn exchange_connect(&mut self, app: &mut MailApp) {
        if let Some(problem) = self.exchange_problem() {
            self.error = problem;
            self.settings_open = true;
            return;
        }
        // Microsoft's own mail is a Microsoft account: nothing is sent, and the password goes.
        if self.typed_microsoft() {
            self.exchange_finished(app, EwsSetupEnd::ExchangeOnline);
            return;
        }
        self.exchange.trusted.clear();
        let url = self.exchange.url.trim().to_string();
        self.exchange_run(app, url, String::new(), false);
    }

    /// Give the host a job and wait for it on the checking page.
    fn exchange_run(&mut self, app: &mut MailApp, url: String, trusted: String, certificate_only: bool) {
        let Some(start) = &app.services.ews_setup else {
            self.error = tl!("This build can't connect to mail servers. Use the desktop app to add an Exchange server account.").to_string();
            return;
        };
        let (tx, answers) = channel();
        self.exchange.cancel.store(true, Ordering::Relaxed);
        self.exchange.cancel = Arc::new(AtomicBool::new(false));
        let job = EwsSetupJob {
            email: self.email.trim().to_string(),
            username: self.username.trim().to_string(),
            password: if certificate_only { String::new() } else { self.password.0.clone() },
            url,
            trusted,
            certificate_only,
            cancel: self.exchange.cancel.clone(),
            answers,
        };
        // The field goes out of view; what it kept between frames goes with it.
        if let Some(ctx) = &self.ctx {
            forget_field(ctx);
        }
        self.exchange.events = Some(start(job));
        self.exchange.answers = Some(tx);
        self.go(Page::ExchangeChecking);
    }

    /// File › Info asks again about a certificate of this account's server that changed: look at
    /// the certificate alone (no sign-in is sent) and ask.
    pub(super) fn review_certificate(app: &MailApp, id: Id) -> Option<AccountSetup> {
        let a = app.session.mailbox.account(id).filter(|a| a.kind == AccountKind::Exchange && !a.email.is_empty())?;
        let mut w = AccountSetup::blank(Page::ExchangeChecking);
        w.email = line(&a.email, LINE);
        w.exchange.review = id;
        Some(w)
    }

    /// The look at an account's certificate is handed to the host on the dialog's first frame.
    fn start_review(&mut self, app: &mut MailApp) {
        if self.exchange.review == 0 || self.exchange.started {
            return;
        }
        self.exchange.started = true;
        let Some(server) = app.session.mailbox.account(self.exchange.review).and_then(|a| a.ews.clone()) else {
            self.done = true;
            return;
        };
        self.exchange_run(app, server.url, server.trusted, true);
        if self.exchange.events.is_none() {
            // This build has no host to look: nothing to ask.
            self.go_keeping_error(Page::ExchangeCertificate);
        }
    }

    // ---- what the owner asked ----

    /// The owner's action on an Exchange page; `None` when the page in view isn't one.
    pub(super) fn act_exchange(&mut self, app: &mut MailApp, action: Action) -> Option<bool> {
        match (self.page, action) {
            (Page::Exchange, Action::Go) => self.exchange_connect(app),
            (Page::Exchange, Action::Back) => {
                self.forget_password();
                self.go(Page::Choose);
            }
            (Page::ExchangeChecking, Action::Back) => {
                self.exchange_stop();
                if self.exchange.review != 0 {
                    return Some(true);
                }
                self.go(Page::Exchange);
            }
            (Page::ExchangeConsent, Action::Go) => self.exchange_answer(true),
            (Page::ExchangeConsent, Action::Back) => self.exchange_answer(false),
            (Page::ExchangeCertificate, Action::Go) => return Some(self.exchange_trust(app)),
            (Page::ExchangeCertificate, Action::Back) => {
                if self.exchange.review != 0 {
                    return Some(true);
                }
                self.go(Page::Exchange);
            }
            (Page::ExchangeOnline, Action::Go) => {
                // Microsoft's own page, with the address filled in: nothing is typed here.
                self.exchange = Exchange::default();
                self.forget_password();
                self.pick(app, Kind::Microsoft365);
            }
            (Page::ExchangeOnline, Action::Back) => self.go(Page::Exchange),
            (Page::Exchange | Page::ExchangeChecking | Page::ExchangeConsent | Page::ExchangeCertificate | Page::ExchangeOnline, _) => {}
            _ => return None,
        }
        Some(self.done)
    }

    /// Call the host's job off and drop what it would bring.
    pub(super) fn exchange_stop(&mut self) {
        self.exchange.cancel.store(true, Ordering::Relaxed);
        (self.exchange.events, self.exchange.answers, self.exchange.consent) = (None, None, None);
    }

    /// The owner's say on a host the sign-in would go to.
    fn exchange_answer(&mut self, yes: bool) {
        if let Some(answers) = &self.exchange.answers {
            let _ = answers.send(yes);
        }
        self.exchange.consent = None;
        self.go(Page::ExchangeChecking);
    }

    /// "Trust this certificate for this account": only with the box ticked. In a wizard that is
    /// adding the account, the test is made again against the same server with the fingerprint; for
    /// an account that exists, the fingerprint is kept on it. `true` when the dialog is done.
    fn exchange_trust(&mut self, app: &mut MailApp) -> bool {
        let Some(certificate) = self.exchange.certificate.clone().filter(|_| self.exchange.ticked) else { return false };
        // (An expired certificate never gets here: the host does not offer it.)
        if EwsServer::check_fingerprint(&certificate.fingerprint).is_err() || certificate.fingerprint.is_empty() {
            return false;
        }
        if self.exchange.review != 0 {
            let id = self.exchange.review;
            let kept = app.execute("account.edit", json!({"id": id, "ews": {"trusted": certificate.fingerprint}}));
            match kept {
                Ok(_) => {
                    app.ews_certificate.remove(&id);
                    app.jmap_refused.remove(&id);
                    // The run that stopped at the certificate goes on at once.
                    let _ = app.execute("ui.sync", json!({"account": id}));
                    self.done = true;
                }
                Err(e) => self.error = e,
            }
            return self.done;
        }
        self.exchange.trusted = certificate.fingerprint.clone();
        self.exchange_run(app, certificate.url, certificate.fingerprint, false);
        false
    }

    // ---- what the host answered ----

    /// Take in what the host's job brought since the last frame.
    pub(super) fn poll_exchange(&mut self, app: &mut MailApp) {
        self.start_review(app);
        for _ in 0..8 {
            let event = match self.exchange.events.as_ref().map(Receiver::try_recv) {
                Some(Ok(event)) => event,
                Some(Err(TryRecvError::Empty)) | None => return,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.exchange_stop();
                    self.error = tl!("Setting up the account stopped unexpectedly").to_string();
                    self.go_back_from_exchange();
                    return;
                }
            };
            match event {
                EwsSetupEvent::Consent { host, reason } => {
                    self.exchange.consent = Some((line(&host, HOST), reason));
                    self.go(Page::ExchangeConsent);
                }
                EwsSetupEvent::Finished(end) => {
                    (self.exchange.events, self.exchange.answers) = (None, None);
                    self.exchange_finished(app, end);
                    return;
                }
            }
        }
    }

    /// Back to the page the job was started from.
    fn go_back_from_exchange(&mut self) {
        if self.exchange.review != 0 {
            self.done = true;
        } else {
            self.go_keeping_error(Page::Exchange);
        }
    }

    /// Go to `page`, with the error that was said kept.
    fn go_keeping_error(&mut self, page: Page) {
        let error = std::mem::take(&mut self.error);
        self.go(page);
        self.error = error;
    }

    fn exchange_finished(&mut self, app: &mut MailApp, end: EwsSetupEnd) {
        match end {
            EwsSetupEnd::Works { url, signed_in_with } => self.add_exchange(app, &url, &signed_in_with),
            EwsSetupEnd::ExchangeOnline => {
                // Nothing was sent to Microsoft; nothing typed stays either.
                self.forget_password();
                self.go(Page::ExchangeOnline);
            }
            EwsSetupEnd::Certificate(certificate) => {
                self.exchange.certificate = Some(certificate);
                self.exchange.ticked = false;
                self.go(Page::ExchangeCertificate);
            }
            EwsSetupEnd::CertificateFine => {
                // Nothing needs the owner's say now (the system trusts it, or the account does).
                if self.exchange.review != 0 {
                    let id = self.exchange.review;
                    app.ews_certificate.remove(&id);
                    app.jmap_refused.remove(&id);
                    let _ = app.execute("ui.sync", json!({"account": id}));
                }
                self.done = true;
            }
            EwsSetupEnd::Failed(why) => {
                self.error = failure_words(why);
                // A certificate nobody may trust: the page that said so is the settings'.
                self.settings_open = true;
                if self.exchange.review != 0 {
                    self.exchange.certificate = None;
                    self.go_review_failed();
                } else {
                    self.go_keeping_error(Page::Exchange);
                }
            }
        }
    }

    /// A review that came to nothing the owner may decide: the words are shown on the certificate
    /// page, without a box to tick.
    fn go_review_failed(&mut self) {
        self.go_keeping_error(Page::ExchangeCertificate);
    }

    /// The account works: add it, keep its password for this session and, where the owner lets it
    /// be, with the operating system; then its first Send/Receive.
    fn add_exchange(&mut self, app: &mut MailApp, url: &str, signed_in_with: &str) {
        let Some((email, _)) = address(&self.email) else {
            self.error = self.not_an_address();
            return self.go_keeping_error(Page::Exchange);
        };
        let email = email.to_string();
        let mut ews = object(json!({"url": url}));
        if !self.username.trim().is_empty() {
            ews.insert("username".into(), json!(self.username.trim()));
        }
        if !self.exchange.trusted.is_empty() {
            ews.insert("trusted".into(), json!(self.exchange.trusted));
        }
        let added = app.execute("account.add", json!({"email": email, "displayName": self.name.trim(), "kind": "exchange", "ews": ews}));
        match added.map(|v| v.get("id").and_then(Value::as_u64)) {
            Ok(Some(id)) => {
                let password = self.password.take();
                if !password.is_empty() {
                    app.remember("password", &email, &password);
                    app.adopt_password(id, password);
                }
                if matches!(signed_in_with, "basic" | "ntlm") {
                    app.ews_signed_in.insert(id, signed_in_with.to_string());
                }
                self.finish(app, id);
            }
            Ok(None) => {
                self.error = tl!("The account couldn't be added").to_string();
                self.go_keeping_error(Page::Exchange);
            }
            Err(e) => {
                self.error = e;
                self.go_keeping_error(Page::Exchange);
            }
        }
    }

    // ---- the pages ----

    /// The address, the password, and Server settings.
    pub(super) fn exchange_form(&mut self, app: &MailApp, ui: &mut Ui, pass: &mut Pass) -> Option<Action> {
        let t = pass.t;
        page(ui, &t, |ui| {
            heading(ui, tl!("Exchange server account"), &t);
            note(ui, &shown(self.email.trim(), 120), t.text_dim);
            note(
                ui,
                tl!(
                    "For a mail server your organisation runs itself. MailCraft looks for the server from your address, or you can type its address in Server settings. Nothing is sent until you press Connect."
                ),
                t.text,
            );
            self.password_field(ui, pass, app.secret_watch(), "");
            note(ui, kept_where(app), t.text_dim);
            ui.add_space(2.0);
            disclosure(ui, tl!("Server settings"), &mut self.settings_open, &t);
            if self.settings_open {
                indented(ui, |ui| {
                    if field(
                        ui,
                        pass,
                        "ewsUrl",
                        tl!("Server address (Exchange Web Services)"),
                        &mut self.exchange.url,
                        "https://mail.example.org/EWS/Exchange.asmx",
                        URL,
                    )
                    .changed()
                    {
                        self.error.clear();
                    }
                    note(ui, tl!("Leave it empty to look for the server from your address. It must begin with https://."), t.text_dim);
                    field(
                        ui,
                        pass,
                        "username",
                        tl!("User name"),
                        &mut self.username,
                        tl!("DOMAIN\\user or you@example.org; empty for your address"),
                        LINE,
                    );
                });
            }
        });
        problem(ui, &self.error);
        let ready = !self.password.0.is_empty();
        let (mut go, mut back) = (false, false);
        strip(ui, self.page, |ui| {
            go = button(ui, tl!("Connect"), true, ready).clicked();
            back = button(ui, tl!("Back"), false, true).clicked();
        });
        if back { Some(Action::Back) } else { (ready && (go || pass.submit)).then_some(Action::Go) }
    }

    /// The host is searching for the server and trying the account.
    pub(super) fn exchange_checking(&mut self, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let (text, detail) = if self.exchange.review != 0 {
            (tl!("Looking at the server's certificate…"), String::new())
        } else if self.exchange.url.trim().is_empty() {
            (tl!("Looking for your Exchange server…"), shown(self.email.trim(), 120))
        } else {
            (tl!("Checking the account…"), shown(self.email.trim(), 120))
        };
        waiting_page(ui, pass, self.page, text, &detail)
    }

    /// The question before the sign-in goes to a host outside the owner's domain.
    pub(super) fn exchange_consent(&mut self, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        let (host, reason) = self.exchange.consent.clone().unwrap_or((String::new(), EwsConsentReason::Redirect));
        let host = shown(&host, 120);
        let why = match reason {
            EwsConsentReason::Redirect => {
                tl!("The search for your Exchange server was sent on to {host}, which isn't in your address's own domain.")
            }
            EwsConsentReason::OtherDomain => {
                tl!("The search for your Exchange server asks to start again with an address in another domain, {host}.")
            }
            EwsConsentReason::MailServer => {
                tl!("The settings the server gave name {host}, which isn't in your address's own domain, as the mail server.")
            }
        };
        page(ui, &t, |ui| {
            heading(ui, tl!("Send your sign-in to another server?"), &t);
            ui.label(RichText::new(fmt(why, &[("host", &host)])).font(medium(13.0)).color(t.text));
            note(
                ui,
                &fmt(
                    tl!("If you go on, your user name and password are sent to {host}. Go on only if you know it is your organisation's."),
                    &[("host", &host)],
                ),
                t.text_dim,
            );
            note(ui, tl!("If you don't, nothing is sent to it, and the search goes on elsewhere or ends."), t.text_dim);
        });
        let (mut yes, mut no) = (false, false);
        strip(ui, self.page, |ui| {
            yes = button(ui, tl!("Send my sign-in"), true, true).clicked();
            no = button(ui, tl!("Don't send"), false, true).clicked();
        });
        if no { Some(Action::Back) } else { (yes || pass.submit).then_some(Action::Go) }
    }

    /// The certificate the system doesn't trust: what it is, and the box that trusts it for this
    /// account. A review that came to nothing the owner may decide says why instead.
    pub(super) fn exchange_certificate(&mut self, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        let Some(certificate) = self.exchange.certificate.clone() else {
            // Nothing to decide: the words, and Close.
            page(ui, &t, |ui| {
                heading(ui, tl!("The server's certificate"), &t);
            });
            problem(ui, &self.error);
            let mut close = false;
            strip(ui, self.page, |ui| close = button(ui, tl!("Close"), true, true).clicked());
            return if close || pass.submit { Some(Action::Back) } else { None };
        };
        let host = EwsServer { url: certificate.url.clone(), ..Default::default() }.host().map(str::to_string).unwrap_or_default();
        let host = shown(&host, 120);
        page(ui, &t, |ui| {
            heading(ui, if certificate.changed { tl!("The server's certificate has changed") } else { tl!("Trust this server's certificate?") }, &t);
            let says = if certificate.changed {
                tl!(
                    "The certificate the server at {host} shows is not the one you trusted for this account, and this computer doesn't trust it. Trust it only if your administrator has changed it."
                )
            } else {
                tl!(
                    "This computer doesn't trust the certificate the server at {host} shows. If this is your organisation's server, you can trust this one certificate for this account."
                )
            };
            note(ui, &fmt(says, &[("host", &host)]), t.text);
            ui.add_space(4.0);
            let row = |ui: &mut Ui, label: &str, value: &str, mono: bool| {
                caption(ui, label, &t);
                let text = RichText::new(value).color(t.text);
                ui.label(if mono { text.monospace() } else { text });
            };
            row(ui, tl!("Issued to"), &shown(&certificate.subject, 160), false);
            row(ui, tl!("Issued by"), &shown(&certificate.issuer, 160), false);
            row(
                ui,
                tl!("Valid"),
                &fmt(tl!("from {from} to {to}"), &[("from", &day_of(certificate.not_before)), ("to", &day_of(certificate.not_after))]),
                false,
            );
            row(ui, tl!("SHA-256 fingerprint"), &grouped(&certificate.fingerprint), true);
            ui.add_space(6.0);
            ui.checkbox(&mut self.exchange.ticked, tl!("Trust this certificate for this account"));
            note(
                ui,
                tl!("Only this certificate is trusted, and only for this account. If the server shows another one later, you are asked again."),
                t.text_dim,
            );
        });
        problem(ui, &self.error);
        let ready = self.exchange.ticked;
        let (mut go, mut back) = (false, false);
        strip(ui, self.page, |ui| {
            go = button(ui, tl!("Continue"), true, ready).clicked();
            back = button(ui, tl!("Back"), false, true).clicked();
        });
        if back { Some(Action::Back) } else { (ready && (go || pass.submit)).then_some(Action::Go) }
    }

    /// The mailbox is in Microsoft 365: it is a Microsoft account.
    pub(super) fn exchange_online(&mut self, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        page(ui, &t, |ui| {
            heading(ui, tl!("This mailbox is in Microsoft 365"), &t);
            note(ui, &shown(self.email.trim(), 120), t.text_dim);
            note(
                ui,
                tl!(
                    "Microsoft 365 mail is added as a Microsoft account: you sign in on Microsoft's own page in your browser, and MailCraft never sees your password."
                ),
                t.text,
            );
            note(ui, tl!("Nothing was sent to Microsoft, and your password isn't kept."), t.text_dim);
        });
        let (mut go, mut back) = (false, false);
        strip(ui, self.page, |ui| {
            go = button(ui, tl!("Add as a Microsoft account"), true, true).clicked();
            back = button(ui, tl!("Back"), false, true).clicked();
        });
        if back { Some(Action::Back) } else { (go || pass.submit).then_some(Action::Go) }
    }

    /// The part of Account Settings for an Exchange server account: where it connects and as whom
    /// (read, not changed: another server or name is another mailbox, which is added again).
    pub(super) fn exchange_settings(&mut self, ui: &mut Ui, t: &Tokens, server: Option<&EwsServer>) {
        let host = server.and_then(EwsServer::host).unwrap_or_default().to_string();
        caption(ui, tl!("Exchange server"), t);
        ui.label(RichText::new(shown(&host, 120)).color(t.text));
        if let Some(server) = server.filter(|s| !s.username.trim().is_empty()) {
            caption(ui, tl!("User name"), t);
            ui.label(RichText::new(shown(server.username.trim(), 120)).color(t.text));
        }
        note(ui, tl!("To use another server or user name, remove the account and add it again."), t.text_dim);
    }
}
