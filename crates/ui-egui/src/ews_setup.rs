// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Setting up an Exchange server account (#252, E7): what the window asks of the host
//! ([`EwsSetupJob`]) and what the host tells it ([`EwsSetupEvent`]).
//!
//! The window only collects: an address, a password, perhaps the server's address and a user
//! name. The host does the rest on a thread of its own, and nothing is sent anywhere before the
//! owner presses Connect: it asks the DNS who takes the domain's mail, searches for the server
//! (Autodiscover), asks the owner before the sign-in goes to a host outside the domain, and tests
//! the account with one request for the Inbox. What comes back is what the page says, in words:
//! the host never hands the window a sentence a server wrote.
//!
//! The password travels in the job, in memory, and is in no log line. The shapes here have no
//! `Debug` that shows it.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use mailcraft_model::Id;

use crate::MailApp;

/// What the host is asked to do ([`crate::Services::ews_setup`]).
pub struct EwsSetupJob {
    /// The owner's address.
    pub email: String,
    /// The name to sign in with; empty means the address.
    pub username: String,
    /// The password. Empty for a job that only looks at the server's certificate.
    pub password: String,
    /// The server's Exchange Web Services address when the owner typed it (no search is made);
    /// empty to search for it.
    pub url: String,
    /// The SHA-256 fingerprint of the one certificate the owner has just agreed to trust for this
    /// account although the system doesn't; empty for none.
    pub trusted: String,
    /// Only look at the certificate the server at `url` shows: nothing is signed in, so no
    /// password is sent ([`MailApp::review_ews_certificate`]).
    pub certificate_only: bool,
    /// Set when the owner gives up: the host stops at the next safe place.
    pub cancel: Arc<AtomicBool>,
    /// The owner's answers to the questions the host asks ([`EwsSetupEvent::Consent`]): yes or no.
    pub answers: Receiver<bool>,
}

impl std::fmt::Debug for EwsSetupJob {
    /// Which kind of job it is, and nothing of the owner's.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsSetupJob").field("searches", &self.url.is_empty()).field("certificate_only", &self.certificate_only).finish()
    }
}

/// Why the owner is asked before the sign-in goes to a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EwsConsentReason {
    /// The search was sent on to another host.
    Redirect,
    /// The search was asked to start again with an address in another mail domain.
    OtherDomain,
    /// The server's settings name a mail server on another host.
    MailServer,
}

/// A certificate the owner may be asked to trust for this account.
#[derive(Clone, PartialEq, Eq)]
pub struct EwsCertificate {
    /// The address of the server that showed it: the account is added with this address, and the
    /// test is made again against it.
    pub url: String,
    /// The SHA-256 fingerprint, 64 lower-case hexadecimal digits.
    pub fingerprint: String,
    /// Who it was issued to.
    pub subject: String,
    /// Who issued it.
    pub issuer: String,
    /// The first moment it is valid, in seconds since 1970-01-01 UTC.
    pub not_before: i64,
    /// The last moment it is valid, in seconds since 1970-01-01 UTC.
    pub not_after: i64,
    /// It is not the first: the account trusted another one, and this is what the server shows now.
    pub changed: bool,
}

impl std::fmt::Debug for EwsCertificate {
    /// The names and the fingerprint are an organisation's own: not for a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsCertificate").field("changed", &self.changed).finish_non_exhaustive()
    }
}

/// Why a certificate can never be trusted, whatever the owner says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EwsCertificateFault {
    /// Its last day is past.
    Expired,
    /// Its first day has not come.
    NotYetValid,
    /// It is for another server.
    WrongName,
}

/// Why setting the account up came to nothing: each is a sentence the owner can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EwsSetupFailure {
    /// Nobody answered at the usual places.
    Unreachable,
    /// The domain has no automatic setup.
    NoAutodiscover,
    /// A secure connection could not be set up.
    SecureConnection,
    /// The server's answer could not be read.
    Unreadable,
    /// The setup was sent somewhere that is not encrypted.
    InsecureRedirect,
    /// The server or the organisation refused.
    ServerRefused,
    /// The answer has no address for Exchange Web Services.
    NoExchangeAddress,
    /// The server wants a way of signing in that MailCraft doesn't speak yet.
    SignInNotSupported,
    /// The server's automatic setup takes only NTLM, which a typed server address is signed in to with.
    SignInNtlm,
    /// The owner did not agree to a host.
    Declined,
    /// The user name or password was not accepted.
    SignInRefused,
    /// Too many redirects in a row.
    TooManyRedirects,
    /// The organisation has switched Exchange Web Services off, or doesn't allow it.
    NotAllowed,
    /// The server answered, but not as an Exchange server does.
    NotExchange,
    /// The address or the user name can't be used.
    BadSettings,
    /// A certificate nobody may trust.
    Certificate(EwsCertificateFault),
    /// Anything else.
    Other,
}

/// How a setup job ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EwsSetupEnd {
    /// The account works: the Inbox could be had at this address.
    Works {
        /// The address of Exchange Web Services that worked.
        url: String,
        /// How the sign-in went: `basic` or `ntlm`.
        signed_in_with: String,
    },
    /// The mailbox is in Microsoft 365. Nothing was sent there; it is a Microsoft account.
    ExchangeOnline,
    /// The server's certificate needs the owner's say.
    Certificate(EwsCertificate),
    /// A job that only looked at the certificate: nothing needs the owner's say now.
    CertificateFine,
    /// It didn't work, and why.
    Failed(EwsSetupFailure),
}

/// What the host tells the window while a job is under way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EwsSetupEvent {
    /// May the sign-in go to `host`? The window asks the owner and sends the answer.
    Consent {
        /// The host (or mail domain) in question.
        host: String,
        /// Why it is asked.
        reason: EwsConsentReason,
    },
    /// The job is over.
    Finished(EwsSetupEnd),
}

impl MailApp {
    /// What File › Info says of how an Exchange server account's last run signed in, in words;
    /// `None` before any run has.
    pub(crate) fn ews_sign_in_words(&self, account: Id) -> Option<&'static str> {
        match self.ews_signed_in.get(&account).map(String::as_str) {
            Some("ntlm") => Some("NTLM"),
            Some("basic") => Some(tl!("Basic (user name and password over a secure connection)")),
            _ => None,
        }
    }
}

/// File › Info under an Exchange server account's card (#252): where it connects, as whom, how the
/// last run signed in, the certificate it trusts (with Forget), and a certificate that needs the
/// owner's say again (with a way to answer). Only while Exchange server accounts are on: switched
/// off, the card says so and there is nothing to change.
pub(crate) fn info_rows(app: &mut MailApp, ui: &mut egui::Ui, t: &crate::theme::Tokens, a: &mailcraft_model::Account) {
    use crate::forms::kit::shown;
    use crate::theme::regular;
    use serde_json::json;
    let Some(server) = a.ews.as_ref().filter(|_| app.session.ews) else { return };
    // (Close together: they belong to the card above.)
    ui.spacing_mut().item_spacing.y = 2.0;
    let line = |ui: &mut egui::Ui, text: String| {
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            ui.label(egui::RichText::new(text).font(regular(11.5)).color(t.text_dim));
        });
    };
    line(ui, crate::i18n::fmt(tl!("Server: {address}"), &[("address", &shown(server.url.trim(), 160))]));
    let user = if server.username.trim().is_empty() { shown(a.email.trim(), 160) } else { shown(server.username.trim(), 160) };
    line(ui, crate::i18n::fmt(tl!("User name: {name}"), &[("name", &user)]));
    let how = app.ews_sign_in_words(a.id).map_or_else(|| tl!("not known until the first Send/Receive").to_string(), str::to_string);
    line(ui, crate::i18n::fmt(tl!("Signs in with: {how}"), &[("how", &how)]));
    if !server.trusted.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            ui.label(egui::RichText::new(tl!("Trusted certificate (SHA-256 fingerprint):")).font(regular(11.5)).color(t.text_dim));
            let tip = tl!(
                "Stop trusting this certificate for this account. If the server shows it again and this computer doesn't trust it, you are asked."
            );
            if ui.link(tl!("Forget")).on_hover_text(tip).clicked() {
                let _ = app.run("account.forgetEwsCertificate", json!({"id": a.id}));
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            // (A mailbox file can say anything: only 64 hexadecimal digits are set out in pairs.)
            let text = if mailcraft_model::EwsServer::check_fingerprint(&server.trusted).is_ok() {
                crate::account_setup::grouped(&server.trusted)
            } else {
                shown(&server.trusted, 120)
            };
            ui.label(egui::RichText::new(text).monospace().size(11.0).color(t.text_dim));
        });
    }
    if app.ews_certificate.contains_key(&a.id) {
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            ui.label(egui::RichText::new(tl!("The server shows a certificate that needs your say.")).font(regular(11.5)).color(t.orange));
            let tip = tl!("Look at the certificate the server shows now. Nothing is signed in while you decide.");
            if ui.link(tl!("Review certificate…")).on_hover_text(tip).clicked() {
                let _ = app.run("ui.dialog", json!({"name": "account", "id": a.id, "page": "certificate"}));
            }
        });
    }
    ui.add_space(4.0);
}
