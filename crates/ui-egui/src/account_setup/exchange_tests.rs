// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Account for an Exchange server account (#252, E7): an explicit choice behind the switch,
//! nothing sent before Connect, the host's search with the owner's questions, the certificate
//! question, a mailbox in Microsoft 365, the password remembered, and File › Info. The host is a
//! stand-in that records the jobs it is given and says what the test tells it to; nothing here
//! leaves the machine, and every name, address and fingerprint is invented.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use serde_json::{Value, json};

use super::*;
use crate::dialogs::Dialog;
use crate::ews_setup::{EwsCertificate, EwsCertificateFault, EwsConsentReason, EwsSetupEnd, EwsSetupEvent, EwsSetupFailure, EwsSetupJob};
use crate::forms::Form;
use crate::{SecretStore, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
/// What the tests type where a password goes.
const TYPED: &str = "not-a-real-password";
const ME: &str = "ada@example.org";
const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";
const TILE: &str = "Exchange server (your organisation's own)";

/// What one job asked of the host.
struct Asked {
    email: String,
    username: String,
    password: String,
    url: String,
    trusted: String,
    certificate_only: bool,
    cancel: Arc<AtomicBool>,
    answers: Receiver<bool>,
}

/// A host that sets Exchange server accounts up: it records each job and says what it is told to.
#[derive(Clone, Default)]
struct Host {
    jobs: Arc<Mutex<Vec<Asked>>>,
    lines: Arc<Mutex<Vec<Sender<EwsSetupEvent>>>>,
    /// The job in the same breath it came: the events to answer with at once.
    at_once: Arc<Mutex<Vec<EwsSetupEvent>>>,
}

impl Host {
    fn install(&self, app: &mut MailApp) {
        let host = self.clone();
        app.services.ews_setup = Some(Box::new(move |job: EwsSetupJob| {
            host.jobs.lock().unwrap().push(Asked {
                email: job.email,
                username: job.username,
                password: job.password,
                url: job.url,
                trusted: job.trusted,
                certificate_only: job.certificate_only,
                cancel: job.cancel,
                answers: job.answers,
            });
            let (tx, rx) = mpsc::channel();
            for event in host.at_once.lock().unwrap().drain(..) {
                tx.send(event).unwrap();
            }
            host.lines.lock().unwrap().push(tx);
            rx
        }));
    }

    /// Tell the job in hand something.
    fn say(&self, event: EwsSetupEvent) {
        self.lines.lock().unwrap().last().unwrap().send(event).unwrap();
    }

    fn end(&self, end: EwsSetupEnd) {
        self.say(EwsSetupEvent::Finished(end));
    }

    fn jobs(&self) -> usize {
        self.jobs.lock().unwrap().len()
    }

    fn last<R>(&self, read: impl FnOnce(&Asked) -> R) -> R {
        read(self.jobs.lock().unwrap().last().unwrap())
    }
}

#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<String, String>>>);

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

fn app(ews: bool) -> MailApp {
    let mut session = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    session.ews = ews;
    let mut app = MailApp::new(session, Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    app
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(6);
    harness
}

fn wizard<'a>(h: &'a Harness<'static, MailApp>) -> Option<&'a AccountSetup> {
    match h.state().dialog.as_ref() {
        Some(Dialog::Form(Form::Account(w))) => Some(w),
        _ => None,
    }
}

fn wizard_mut<'a>(h: &'a mut Harness<'static, MailApp>) -> &'a mut AccountSetup {
    match h.state_mut().dialog.as_mut() {
        Some(Dialog::Form(Form::Account(w))) => w,
        _ => panic!("the wizard isn't open"),
    }
}

fn page(h: &Harness<'static, MailApp>) -> Option<Page> {
    wizard(h).map(|w| w.page)
}

fn click(h: &mut Harness<'static, MailApp>, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run_steps(5);
}

fn says(h: &Harness<'static, MailApp>, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
}

fn type_text(h: &mut Harness<'static, MailApp>, text: &str) {
    h.event(egui::Event::Text(text.to_string()));
    h.run_steps(3);
}

/// Open Add Account, type the address, pick Exchange server by hand and type the password.
fn to_the_form(app: MailApp) -> Harness<'static, MailApp> {
    let mut app = app;
    app.execute("ui.dialog", json!({"name": "account"})).unwrap();
    let mut h = window(app);
    type_text(&mut h, ME);
    click(&mut h, "Advanced options");
    h.get_by_role_and_label(Role::CheckBox, "Let me set up my account manually").click();
    h.run_steps(3);
    click(&mut h, "Connect");
    assert_eq!(page(&h), Some(Page::Choose));
    click(&mut h, TILE);
    assert_eq!(page(&h), Some(Page::Exchange));
    h.get_by_role(Role::PasswordInput).focus();
    h.run_steps(2);
    type_text(&mut h, TYPED);
    h
}

fn works(url: &str, with: &str) -> EwsSetupEnd {
    EwsSetupEnd::Works { url: url.into(), signed_in_with: with.into() }
}

fn certificate(changed: bool) -> EwsCertificate {
    EwsCertificate {
        url: URL.into(),
        fingerprint: "ab".repeat(32),
        subject: "mail.example.org, An Invented Organisation".into(),
        issuer: "An Invented CA".into(),
        not_before: 1_700_000_000,
        not_after: 1_900_000_000,
        changed,
    }
}

fn account(h: &Harness<'static, MailApp>) -> mailcraft_model::Account {
    h.state().session.mailbox.accounts.iter().find(|a| a.email == ME).cloned().expect("the account was added")
}

#[test]
fn exchange_server_is_offered_only_while_the_switch_is_on_and_only_by_choice() {
    for (ews, offered) in [(false, false), (true, true)] {
        let mut a = app(ews);
        Host::default().install(&mut a);
        a.execute("ui.dialog", json!({"name": "account"})).unwrap();
        let mut h = window(a);
        type_text(&mut h, ME);
        // Not detected from the address: Connect goes the way it always went.
        click(&mut h, "Advanced options");
        h.get_by_role_and_label(Role::CheckBox, "Let me set up my account manually").click();
        h.run_steps(3);
        click(&mut h, "Connect");
        assert_eq!(page(&h), Some(Page::Choose));
        assert_eq!(h.query_by_role_and_label(Role::Button, TILE).is_some(), offered, "switch {ews}");
        assert!(h.query_by_role_and_label(Role::Button, "Other (IMAP)").is_some());
    }
    assert_eq!(Kind::tiles(false).len(), 6);
    assert_eq!(Kind::tiles(true).last(), Some(&Kind::Exchange));
    // An ordinary Connect, with the switch on, doesn't pick it.
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    a.execute("ui.dialog", json!({"name": "account"})).unwrap();
    let mut h = window(a);
    type_text(&mut h, "ada@exchange-looking.example");
    click(&mut h, "Connect");
    assert_ne!(page(&h), Some(Page::Exchange));
    assert_eq!(host.jobs(), 0);
}

#[test]
fn nothing_is_sent_until_connect_and_the_server_is_found_from_the_address() {
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    assert_eq!(host.jobs(), 0, "typing and choosing send nothing");
    assert!(says(&h, "Server settings") && !says(&h, "Server address (Exchange Web Services)"));
    click(&mut h, "Connect");
    assert_eq!(page(&h), Some(Page::ExchangeChecking));
    assert_eq!(host.jobs(), 1);
    host.last(|job| {
        assert_eq!((job.email.as_str(), job.password.as_str()), (ME, TYPED));
        assert_eq!((job.url.as_str(), job.username.as_str(), job.trusted.as_str(), job.certificate_only), ("", "", "", false));
    });
    assert!(says(&h, "Looking for your Exchange server"));
    // Found, and the Inbox could be had: the account is added.
    host.end(works(URL, "basic"));
    h.run_steps(5);
    let ada = account(&h);
    assert_eq!(ada.kind, AccountKind::Exchange);
    let server = ada.ews.expect("it has a server");
    assert_eq!((server.url.as_str(), server.username.as_str(), server.trusted.as_str()), (URL, "", ""));
    assert!(ada.incoming.is_none() && ada.outgoing.is_none() && ada.oauth.is_none());
    assert_eq!(page(&h), Some(Page::Done));
    assert_eq!(h.state().ews_signed_in.get(&ada.id).map(String::as_str), Some("basic"));
    // The wizard keeps no password.
    assert!(wizard(&h).is_some_and(|w| w.password.0.is_empty()));
}

#[test]
fn a_server_typed_in_server_settings_skips_the_search() {
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Server settings");
    assert!(says(&h, "Server address (Exchange Web Services)"));
    {
        let w = wizard_mut(&mut h);
        w.exchange.url = format!(" {URL} ");
        w.username = "CORP\\ada".into();
    }
    click(&mut h, "Connect");
    host.last(|job| assert_eq!((job.url.as_str(), job.username.as_str()), (URL, "CORP\\ada")));
    assert!(says(&h, "Checking the account"));
    host.end(works(URL, "ntlm"));
    h.run_steps(5);
    let server = account(&h).ews.unwrap();
    assert_eq!((server.url.as_str(), server.username.as_str()), (URL, "CORP\\ada"));
    assert_eq!(h.state().ews_signed_in.values().next().map(String::as_str), Some("ntlm"));
}

#[test]
fn an_address_that_is_not_https_is_refused_before_anything_is_sent() {
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Server settings");
    for bad in ["http://mail.example.org/EWS/Exchange.asmx", "mail.example.org", "https://ada:pw@mail.example.org/EWS/Exchange.asmx", "javascript:1"]
    {
        wizard_mut(&mut h).exchange.url = bad.into();
        click(&mut h, "Connect");
        assert_eq!(page(&h), Some(Page::Exchange), "{bad}");
        assert!(says(&h, "must begin with https://"));
    }
    assert_eq!(host.jobs(), 0);
}

#[test]
fn the_owner_is_asked_before_the_sign_in_goes_to_another_host() {
    for yes in [true, false] {
        let mut a = app(true);
        let host = Host::default();
        host.install(&mut a);
        let mut h = to_the_form(a);
        click(&mut h, "Connect");
        host.say(EwsSetupEvent::Consent { host: "autodiscover.other.example".into(), reason: EwsConsentReason::Redirect });
        h.run_steps(5);
        assert_eq!(page(&h), Some(Page::ExchangeConsent));
        assert!(says(&h, "autodiscover.other.example") && says(&h, "isn't in your address's own domain"));
        assert!(says(&h, "your user name and password are sent to autodiscover.other.example"));
        // Nothing has been answered: nothing goes there.
        assert!(host.last(|job| job.answers.try_recv().is_err()));
        click(&mut h, if yes { "Send my sign-in" } else { "Don't send" });
        assert_eq!(host.last(|job| job.answers.try_recv().ok()), Some(yes));
        assert_eq!(page(&h), Some(Page::ExchangeChecking));
        if !yes {
            host.end(EwsSetupEnd::Failed(EwsSetupFailure::Declined));
            h.run_steps(5);
            assert_eq!(page(&h), Some(Page::Exchange));
            assert!(says(&h, "You didn't agree to send your sign-in") && says(&h, "Server settings"));
            assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != ME));
        }
    }
}

#[test]
fn the_other_reasons_for_asking_are_said_in_plain_words() {
    for (reason, said) in [
        (EwsConsentReason::OtherDomain, "asks to start again with an address in another domain, mail.other.example"),
        (EwsConsentReason::MailServer, "name mail.other.example, which isn't in your address's own domain, as the mail server"),
    ] {
        let mut a = app(true);
        let host = Host::default();
        host.install(&mut a);
        let mut h = to_the_form(a);
        click(&mut h, "Connect");
        host.say(EwsSetupEvent::Consent { host: "mail.other.example".into(), reason });
        h.run_steps(5);
        assert!(says(&h, said), "{said}");
    }
}

#[test]
fn a_mailbox_in_microsoft_365_is_offered_as_a_microsoft_account_and_no_password_goes_anywhere() {
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    host.end(EwsSetupEnd::ExchangeOnline);
    h.run_steps(5);
    assert_eq!(page(&h), Some(Page::ExchangeOnline));
    assert!(says(&h, "This mailbox is in Microsoft 365") && says(&h, "Nothing was sent to Microsoft"));
    // The password is gone from the wizard, and no account was made.
    assert!(wizard(&h).is_some_and(|w| w.password.0.is_empty()));
    assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != ME));
    click(&mut h, "Add as a Microsoft account");
    // The Microsoft sign-in, with the address filled in (this build has no browser to open).
    let w = wizard(&h).unwrap();
    assert!(matches!(w.page, Page::SignIn | Page::ApplicationId), "{:?}", w.page);
    assert_eq!(w.email, ME);
    assert!(w.oauth.as_ref().is_some_and(|o| o.provider == OAuthProvider::Microsoft));
    assert_eq!(host.jobs(), 1, "no second job, and so no second chance for a password to go");
}

#[test]
fn a_failure_is_a_sentence_to_act_on_with_server_settings_open() {
    for failure in [
        EwsSetupFailure::Unreachable,
        EwsSetupFailure::NoAutodiscover,
        EwsSetupFailure::SignInRefused,
        EwsSetupFailure::SignInNotSupported,
        EwsSetupFailure::NotAllowed,
        EwsSetupFailure::NotExchange,
        EwsSetupFailure::Other,
    ] {
        let mut a = app(true);
        let host = Host::default();
        host.install(&mut a);
        let mut h = to_the_form(a);
        click(&mut h, "Connect");
        host.end(EwsSetupEnd::Failed(failure));
        h.run_steps(5);
        assert_eq!(page(&h), Some(Page::Exchange), "{failure:?}");
        assert!(wizard(&h).is_some_and(|w| w.settings_open && !w.error.is_empty()), "{failure:?}");
        assert!(says(&h, "Server address (Exchange Web Services)"));
        // The password is still there to try again; nothing was added.
        assert!(wizard(&h).is_some_and(|w| w.password.0 == TYPED));
        assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != ME));
        // The words are MailCraft's own, whatever the server said.
        let error = wizard(&h).unwrap().error.clone();
        assert!(!error.contains("WWW-Authenticate") && !error.contains("NTLM ") && error.ends_with('.'), "{error}");
    }
}

#[test]
fn a_host_that_stops_unexpectedly_is_told_and_nothing_is_added() {
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    host.lines.lock().unwrap().clear();
    h.run_steps(5);
    assert_eq!(page(&h), Some(Page::Exchange));
    assert!(says(&h, "Setting up the account stopped unexpectedly"));
}

#[test]
fn a_certificate_the_system_does_not_trust_is_shown_and_trusted_only_by_the_owners_tick() {
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    host.end(EwsSetupEnd::Certificate(certificate(false)));
    h.run_steps(5);
    assert_eq!(page(&h), Some(Page::ExchangeCertificate));
    for shown in ["Trust this server's certificate?", "mail.example.org, An Invented Organisation", "An Invented CA", "2023", "2030", "ab:ab:ab:ab"] {
        assert!(says(&h, shown), "{shown}");
    }
    // The whole fingerprint, in two lines of sixteen pairs.
    assert!(says(&h, &grouped(&"ab".repeat(32))));
    // Not trusted until the box is ticked.
    assert_eq!(h.state().session.mailbox.accounts.iter().filter(|a| a.email == ME).count(), 0);
    h.get_by_role_and_label(Role::Button, "Continue").click();
    h.run_steps(5);
    assert_eq!((page(&h), host.jobs()), (Some(Page::ExchangeCertificate), 1), "Continue waits for the tick");
    h.get_by_role_and_label(Role::CheckBox, "Trust this certificate for this account").click();
    h.run_steps(3);
    click(&mut h, "Continue");
    // The test is made again at the same address, with the fingerprint, and the password.
    assert_eq!((page(&h), host.jobs()), (Some(Page::ExchangeChecking), 2));
    host.last(|job| {
        assert_eq!((job.url.as_str(), job.trusted.as_str(), job.password.as_str()), (URL, "ab".repeat(32).as_str(), TYPED));
    });
    host.end(works(URL, "basic"));
    h.run_steps(5);
    // Only now is the fingerprint kept on the account.
    assert_eq!(account(&h).ews.unwrap().trusted, "ab".repeat(32));
}

#[test]
fn going_back_from_the_certificate_trusts_nothing() {
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    host.end(EwsSetupEnd::Certificate(certificate(false)));
    h.run_steps(5);
    h.get_by_role_and_label(Role::CheckBox, "Trust this certificate for this account").click();
    h.run_steps(3);
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::Exchange));
    assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != ME));
    assert!(wizard(&h).is_some_and(|w| w.exchange.trusted.is_empty()));
}

#[test]
fn an_expired_certificate_is_never_offered_for_trust() {
    for (fault, said) in [
        (EwsCertificateFault::Expired, "has expired"),
        (EwsCertificateFault::NotYetValid, "isn't valid yet"),
        (EwsCertificateFault::WrongName, "is for another server's name"),
    ] {
        let mut a = app(true);
        let host = Host::default();
        host.install(&mut a);
        let mut h = to_the_form(a);
        click(&mut h, "Connect");
        host.end(EwsSetupEnd::Failed(EwsSetupFailure::Certificate(fault)));
        h.run_steps(5);
        assert_eq!(page(&h), Some(Page::Exchange));
        assert!(says(&h, said) && !says(&h, "Trust this certificate"), "{said}");
        assert!(h.query_by_role_and_label(Role::CheckBox, "Trust this certificate for this account").is_none());
        assert!(wizard(&h).is_some_and(|w| w.exchange.trusted.is_empty()));
    }
}

/// An Exchange server account that trusts a certificate, in the window.
fn trusting(host: &Host) -> (MailApp, Id) {
    let mut a = app(true);
    host.install(&mut a);
    let server = mailcraft_model::EwsServer { url: URL.into(), trusted: "ab".repeat(32), ..Default::default() };
    let id = a.session.mailbox.add_account(Account { email: ME.into(), kind: AccountKind::Exchange, ews: Some(server), ..Default::default() });
    (a, id)
}

#[test]
fn a_certificate_that_changed_is_asked_about_again_from_file_info_without_signing_in() {
    let host = Host::default();
    let (mut a, id) = trusting(&host);
    a.ews_certificate.insert(id, "cd".repeat(32));
    a.passwords.insert(id, TYPED.into());
    a.execute("ui.dialog", json!({"name": "account", "id": id, "page": "certificate"})).unwrap();
    let mut h = window(a);
    // The host is asked to look at the certificate alone: no password in the job.
    assert_eq!(host.jobs(), 1);
    host.last(|job| {
        assert!(job.certificate_only && job.password.is_empty());
        assert_eq!((job.url.as_str(), job.trusted.as_str(), job.email.as_str()), (URL, "ab".repeat(32).as_str(), ME));
    });
    assert!(says(&h, "Looking at the server's certificate"));
    let new = EwsCertificate { fingerprint: "cd".repeat(32), ..certificate(true) };
    host.end(EwsSetupEnd::Certificate(new));
    h.run_steps(5);
    assert!(says(&h, "The server's certificate has changed") && says(&h, "cd:cd:cd:cd"));
    // Still trusting the old one until the owner ticks.
    assert_eq!(h.state().session.mailbox.account(id).and_then(|a| a.ews.as_ref()).map(|e| e.trusted.clone()), Some("ab".repeat(32)));
    h.get_by_role_and_label(Role::CheckBox, "Trust this certificate for this account").click();
    h.run_steps(3);
    click(&mut h, "Continue");
    assert!(wizard(&h).is_none_or(|w| w.finished()), "the dialog is done");
    let s = h.state();
    assert_eq!(s.session.mailbox.account(id).and_then(|a| a.ews.as_ref()).map(|e| e.trusted.clone()), Some("cd".repeat(32)));
    assert!(!s.ews_certificate.contains_key(&id) && !s.jmap_refused.contains(&id));
    assert_eq!(host.jobs(), 1, "no second job: the run goes on through the account's own Send/Receive");
}

#[test]
fn a_certificate_that_is_fine_again_needs_no_say_and_one_that_cannot_be_trusted_says_why() {
    let host = Host::default();
    let (mut a, id) = trusting(&host);
    a.ews_certificate.insert(id, "cd".repeat(32));
    a.execute("ui.dialog", json!({"name": "account", "id": id, "page": "certificate"})).unwrap();
    let mut h = window(a);
    host.end(EwsSetupEnd::CertificateFine);
    h.run_steps(5);
    assert!(wizard(&h).is_none_or(|w| w.finished()));
    assert!(!h.state().ews_certificate.contains_key(&id));
    // Expired: said, and no box.
    let host = Host::default();
    let (mut a, id) = trusting(&host);
    a.execute("ui.dialog", json!({"name": "account", "id": id, "page": "certificate"})).unwrap();
    let mut h = window(a);
    host.end(EwsSetupEnd::Failed(EwsSetupFailure::Certificate(EwsCertificateFault::Expired)));
    h.run_steps(5);
    assert!(says(&h, "has expired") && h.query_by_role_and_label(Role::CheckBox, "Trust this certificate for this account").is_none());
    click(&mut h, "Close");
    assert!(wizard(&h).is_none_or(|w| w.finished()));
}

#[test]
fn giving_up_calls_the_job_off_and_nothing_is_added() {
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    let cancel = host.last(|job| job.cancel.clone());
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
    click(&mut h, "Cancel");
    assert_eq!(page(&h), Some(Page::Exchange));
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != ME));
    // Escape closes the dialog and calls a job off too.
    click(&mut h, "Connect");
    let cancel = host.last(|job| job.cancel.clone());
    h.key_press(egui::Key::Escape);
    h.run_steps(5);
    assert!(wizard(&h).is_none() && cancel.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn the_password_is_remembered_with_the_system_when_the_owner_lets_it_be_and_read_back() {
    let store = Store::default();
    let mut a = app(true);
    a.services.secrets = Some(Arc::new(store.clone()));
    let host = Host::default();
    host.install(&mut a);
    // The first Send/Receive is a stand-in that never finishes, and says what it was given.
    let secrets: Arc<Mutex<Vec<String>>> = Default::default();
    let seen = secrets.clone();
    let held: Arc<Mutex<Vec<mpsc::Sender<mailcraft_model::sync::EwsMailResult>>>> = Default::default();
    let keep = held.clone();
    a.services.ews = Some(Box::new(move |job| {
        seen.lock().unwrap().push(job.secret.clone());
        let (tx, rx) = mpsc::channel();
        keep.lock().unwrap().push(tx);
        rx
    }));
    let mut h = to_the_form(a);
    click(&mut h, "Server settings");
    wizard_mut(&mut h).username = "CORP\\ada".into();
    click(&mut h, "Connect");
    host.end(works(URL, "basic"));
    h.run_steps(5);
    let id = account(&h).id;
    // In the system's store under the name that holds the server and the user name ...
    let key = MailApp::secret_key("password", &account(&h));
    assert_eq!(key, format!("password:{ME}|ews={URL}|user=corp\\ada"));
    assert_eq!(store.get(&key).as_deref(), Some(TYPED));
    // ... and used by the first Send/Receive, which is under way.
    assert_eq!(secrets.lock().unwrap().as_slice(), [TYPED]);
    // Never in the mailbox, the window's settings, or what a command was given.
    let mailbox = serde_json::to_string(&h.state().session.mailbox).unwrap();
    assert!(!mailbox.contains(TYPED));
    assert!(!serde_json::to_string(&h.state().ui).unwrap().contains(TYPED));
    // A later start reads it back: nothing is typed.
    let mut b = app(true);
    b.services.secrets = Some(Arc::new(store.clone()));
    b.session.mailbox = h.state().session.mailbox.clone();
    b.services.ews = Some(Box::new(|_| mpsc::channel().1));
    assert!(!b.can_sync_quietly(id));
    crate::secret_read_tests::settle(&mut b);
    assert_eq!(b.ews_secret(id).as_deref(), Some(TYPED));
    assert!(b.can_sync_quietly(id));
    // The switch in File › Options off: nothing is kept.
    let store = Store::default();
    let mut a = app(true);
    a.services.secrets = Some(Arc::new(store.clone()));
    a.ui.remember_sign_in = false;
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    host.end(works(URL, "basic"));
    h.run_steps(5);
    assert!(store.0.lock().unwrap().is_empty());
    assert_eq!(h.state().passwords.get(&account(&h).id).map(String::as_str), Some(TYPED), "kept in memory for the session");
}

#[test]
fn file_info_shows_the_server_the_user_name_the_sign_in_and_the_trusted_certificate_with_forget() {
    let host = Host::default();
    let (mut a, id) = trusting(&host);
    a.session.mailbox.account_mut(id).unwrap().ews.as_mut().unwrap().username = "CORP\\ada".into();
    a.ews_signed_in.insert(id, "ntlm".into());
    a.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    let mut h = window(a);
    // (The card's own line is painted, not a label: read it as the card does.)
    let card = crate::backstage::account_detail(h.state(), h.state().session.mailbox.account(id).unwrap());
    assert_eq!(card, "Exchange server mail.example.org");
    for shown in [
        &format!("Server: {URL}"),
        "User name: CORP\\ada",
        "Signs in with: NTLM",
        "Trusted certificate (SHA-256 fingerprint):",
        &grouped(&"ab".repeat(32)),
    ] {
        assert!(says(&h, shown), "{shown}");
    }
    // "not kept in step yet: this part is not built" is gone.
    assert!(!says(&h, "this part is not built"));
    h.get_by_role_and_label(Role::Link, "Forget").click();
    h.run_steps(5);
    assert_eq!(h.state().session.mailbox.account(id).and_then(|a| a.ews.as_ref()).map(|e| e.trusted.clone()), Some(String::new()));
    assert!(!says(&h, "Trusted certificate (SHA-256 fingerprint):"));
}

#[test]
fn file_info_before_any_run_and_with_a_certificate_waiting() {
    let host = Host::default();
    let (mut a, id) = trusting(&host);
    a.session.mailbox.account_mut(id).unwrap().ews.as_mut().unwrap().trusted.clear();
    a.ews_certificate.insert(id, "cd".repeat(32));
    a.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    let mut h = window(a);
    assert!(says(&h, "User name: ada@example.org") && says(&h, "Signs in with: not known until the first Send/Receive"));
    let card = crate::backstage::account_detail(h.state(), h.state().session.mailbox.account(id).unwrap());
    assert_eq!(card, "Exchange server mail.example.org · the server's certificate needs your say");
    assert!(says(&h, "The server shows a certificate that needs your say."));
    assert!(!says(&h, "Trusted certificate"));
    h.get_by_role_and_label(Role::Link, "Review certificate…").click();
    h.run_steps(5);
    assert_eq!(page(&h), Some(Page::ExchangeChecking));
    assert!(host.last(|job| job.certificate_only));
}

#[test]
fn forgetting_the_certificate_is_a_command_that_agents_get_right_too() {
    let host = Host::default();
    let (mut a, id) = trusting(&host);
    let said = a.execute("account.forgetEwsCertificate", json!({"id": id})).unwrap();
    assert_eq!(said, json!({"id": id, "forgotten": true}));
    assert_eq!(a.session.mailbox.account(id).and_then(|a| a.ews.as_ref()).map(|e| e.trusted.clone()), Some(String::new()));
    // Nothing to forget the second time; and no other kind of account has one.
    assert_eq!(a.execute("account.forgetEwsCertificate", json!({"id": id})).unwrap()["forgotten"], json!(false));
    let imap = a.execute("account.add", json!({"email": "bo@imap.example", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].clone();
    assert!(a.execute("account.forgetEwsCertificate", json!({"id": imap})).is_err());
    for junk in [json!({}), json!({"id": "x"}), json!({"id": 999_999}), json!({"id": -1}), Value::Null] {
        assert!(a.execute("account.forgetEwsCertificate", junk).is_err());
    }
    // Switched off, it refuses.
    let mut off = app(false);
    assert!(off.execute("account.forgetEwsCertificate", json!({"id": 1})).is_err());
}

#[test]
fn account_settings_of_an_exchange_account_show_where_it_connects_and_change_nothing_else() {
    let host = Host::default();
    let (mut a, id) = trusting(&host);
    a.session.mailbox.account_mut(id).unwrap().ews.as_mut().unwrap().username = "CORP\\ada".into();
    a.execute("ui.dialog", json!({"name": "account", "id": id})).unwrap();
    let mut h = window(a);
    assert_eq!(page(&h), Some(Page::Settings));
    assert!(says(&h, "Signs in to an Exchange server with a user name and password") && says(&h, "mail.example.org") && says(&h, "CORP\\ada"));
    assert!(says(&h, "remove the account and add it again"));
    // No IMAP or SMTP rows.
    assert!(!says(&h, "Incoming mail (IMAP)") && !says(&h, "Outgoing mail (SMTP)"));
    // Saving with nothing changed changes nothing.
    let before = h.state().session.mailbox.account(id).cloned();
    click(&mut h, "Save");
    assert_eq!(h.state().session.mailbox.account(id).cloned(), before);
}

#[test]
fn a_build_without_a_host_says_so_and_adds_nothing() {
    let a = app(true);
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    assert_eq!(page(&h), Some(Page::Exchange));
    assert!(says(&h, "This build can't connect to mail servers"));
    assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != ME));
}

#[test]
fn the_grouped_fingerprint_is_two_lines_of_sixteen_pairs() {
    let grouped = grouped(&format!("{}{}", "ab".repeat(16), "cd".repeat(16)));
    let lines: Vec<&str> = grouped.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].split(':').all(|pair| pair == "ab") && lines[0].split(':').count() == 16);
    assert!(lines[1].split(':').all(|pair| pair == "cd"));
    // Anything else is cut at pairs and never panics.
    for odd in ["", "a", "abc", "é", "日本語日本語"] {
        let _ = super::exchange::grouped(odd);
    }
}

#[test]
fn every_exchange_page_lays_out_in_both_themes() {
    for dark in [false, true] {
        for step in 0..5 {
            let mut a = app(true);
            let host = Host::default();
            host.install(&mut a);
            let mut h = to_the_form(a);
            if dark {
                h.ctx.set_visuals(egui::Visuals::dark());
            }
            click(&mut h, "Connect");
            match step {
                0 => {}
                1 => host.say(EwsSetupEvent::Consent { host: "autodiscover.other.example".into(), reason: EwsConsentReason::Redirect }),
                2 => host.end(EwsSetupEnd::Certificate(certificate(false))),
                3 => host.end(EwsSetupEnd::ExchangeOnline),
                _ => host.end(EwsSetupEnd::Failed(EwsSetupFailure::Unreachable)),
            }
            h.run_steps(5);
            assert!(wizard(&h).is_some());
        }
    }
}

#[test]
fn the_dialog_opens_on_the_exchange_page_only_with_the_switch_and_an_address() {
    let a = app(true);
    let w = AccountSetup::open(&a, &json!({"email": ME, "page": "exchange"})).unwrap();
    assert_eq!((w.page, w.email.as_str()), (Page::Exchange, ME));
    // No address, or the switch off: the dialog opens as it always does.
    assert_eq!(AccountSetup::open(&a, &json!({"page": "exchange"})).unwrap().page, Page::Address);
    assert_eq!(AccountSetup::open(&a, &json!({"email": "nobody", "page": "exchange"})).unwrap().page, Page::Address);
    assert_eq!(AccountSetup::open(&app(false), &json!({"email": ME, "page": "exchange"})).unwrap().page, Page::Address);
}

/// The wizard in hand told `action`, as the window does it (the dialog taken out of the app for the call).
fn act(h: &mut Harness<'static, MailApp>, action: Action) {
    let mut dialog = h.state_mut().dialog.take();
    if let Some(Dialog::Form(Form::Account(w))) = dialog.as_mut() {
        w.act_exchange(h.state_mut(), action);
    }
    h.state_mut().dialog = dialog;
    h.run_steps(3);
}

#[test]
fn go_on_the_certificate_page_without_the_tick_trusts_nothing_and_sends_nothing() {
    // Adding an account.
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    host.end(EwsSetupEnd::Certificate(certificate(false)));
    h.run_steps(5);
    assert_eq!(page(&h), Some(Page::ExchangeCertificate));
    act(&mut h, Action::Go);
    assert_eq!((page(&h), host.jobs()), (Some(Page::ExchangeCertificate), 1), "no second test, so no password sent");
    assert!(wizard(&h).is_some_and(|w| w.exchange.trusted.is_empty()));
    assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != ME));
    // An account that exists: its trusted fingerprint is as it was.
    let host = Host::default();
    let (mut a, id) = trusting(&host);
    a.ews_certificate.insert(id, "cd".repeat(32));
    a.execute("ui.dialog", json!({"name": "account", "id": id, "page": "certificate"})).unwrap();
    let mut h = window(a);
    host.end(EwsSetupEnd::Certificate(EwsCertificate { fingerprint: "cd".repeat(32), ..certificate(true) }));
    h.run_steps(5);
    act(&mut h, Action::Go);
    let kept = h.state().session.mailbox.account(id).and_then(|a| a.ews.as_ref()).map(|e| e.trusted.clone());
    assert_eq!(kept, Some("ab".repeat(32)));
    assert!(h.state().ews_certificate.contains_key(&id), "still asked about");
}

#[test]
fn removing_the_account_deletes_its_entry_in_the_store_and_a_new_server_forgets_the_old_one() {
    let store = Store::default();
    let mut a = app(true);
    a.services.secrets = Some(Arc::new(store.clone()));
    let host = Host::default();
    host.install(&mut a);
    a.services.ews = Some(Box::new(|_| mpsc::channel().1));
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    host.end(works(URL, "basic"));
    h.run_steps(5);
    let acct = account(&h);
    let key = MailApp::secret_key("password", &acct);
    assert_eq!(store.get(&key).as_deref(), Some(TYPED));
    // A new server address: the password kept for the old one is gone.
    let moved = "https://exch02.example.org/EWS/Exchange.asmx";
    h.state_mut().execute("account.edit", json!({"id": acct.id, "ews": {"url": moved}})).unwrap();
    assert_eq!(store.get(&key), None, "the old server's entry is deleted");
    assert!(store.0.lock().unwrap().values().all(|v| v != TYPED) || !h.state().passwords.contains_key(&acct.id));
    // Another user name: likewise, and removing the account leaves nothing of it.
    store.set(&MailApp::secret_key("password", &account(&h)), TYPED).unwrap();
    let before = MailApp::secret_key("password", &account(&h));
    h.state_mut().execute("account.edit", json!({"id": acct.id, "ews": {"username": "CORP\\ada"}})).unwrap();
    assert_eq!(store.get(&before), None, "a new user name forgets the old entry");
    let current = MailApp::secret_key("password", &account(&h));
    store.set(&current, TYPED).unwrap();
    h.state_mut().execute("account.remove", json!({"id": acct.id})).unwrap();
    assert!(store.0.lock().unwrap().is_empty(), "{:?}", store.0.lock().unwrap().keys().collect::<Vec<_>>());
}

#[test]
fn the_sign_in_failures_that_differ_say_so_in_plain_words() {
    // NTLM only: Server settings is the way, and NTLM is what will be used.
    let ntlm = exchange::failure_words(EwsSetupFailure::SignInNtlm);
    assert!(ntlm.contains("Server settings") && ntlm.contains("NTLM") && !ntlm.contains("can't do yet"), "{ntlm}");
    // Kerberos: no suggestion that typing the address would help.
    let kerberos = exchange::failure_words(EwsSetupFailure::SignInNotSupported);
    assert!(kerberos.contains("Kerberos") && !kerberos.contains("Server settings") && !kerberos.contains("Type"), "{kerberos}");
    // The window opens Server settings for the first.
    let mut a = app(true);
    let host = Host::default();
    host.install(&mut a);
    let mut h = to_the_form(a);
    click(&mut h, "Connect");
    host.end(EwsSetupEnd::Failed(EwsSetupFailure::SignInNtlm));
    h.run_steps(5);
    assert!(wizard(&h).is_some_and(|w| w.settings_open));
    assert!(says(&h, "Automatic setup can't sign in to this server."));
}

#[test]
fn a_server_typed_on_a_microsoft_host_is_handed_over_with_nothing_sent() {
    for url in ["https://outlook.office365.com/EWS/Exchange.asmx", "https://outlook.office.com/EWS/Exchange.asmx"] {
        let mut a = app(true);
        let host = Host::default();
        host.install(&mut a);
        let mut h = to_the_form(a);
        click(&mut h, "Server settings");
        wizard_mut(&mut h).exchange.url = url.into();
        click(&mut h, "Connect");
        h.run_steps(5);
        assert_eq!(page(&h), Some(Page::ExchangeOnline), "{url}");
        assert_eq!(host.jobs(), 0, "no job, so no sign-in went to {url}");
        assert!(says(&h, "This mailbox is in Microsoft 365"));
        assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != ME));
        assert!(wizard(&h).is_some_and(|w| w.password.0.is_empty()), "the password is wiped");
    }
}
