// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of setting up an Exchange server account (#252, E7), on a thread of its own,
//! once the owner has pressed Connect and not before.
//!
//! **What goes where.** The address, then the password, only to the server Autodiscover found or
//! the owner named, and only in answer to that server's own challenge during Autodiscover (the
//! search never sends a credential unasked: `mailcraft_ews::Autodiscover`). The DNS lookups (the
//! domain's mail exchangers, the `_autodiscover._tcp` SRV record) go over DNS-over-HTTPS with the
//! domain and nothing else. A host outside the owner's domain is asked about first, naming it
//! (`EwsSetupEvent::Consent`), and no password goes there before a yes. A mailbox in Microsoft 365
//! is a Microsoft account: the search ends with that said, and nothing is sent there.
//!
//! **The test** of the account is one `GetFolder` of the Inbox (`Client::check`) through the same
//! transport a run uses ([`Transport`]: NTLM or Basic from the server's own offer, the certificate
//! judged by the system first). A certificate the system doesn't trust ends the test before any
//! credential is sent, with the question for the owner; one that is expired, not yet valid or for
//! another name is told in words and never offered. A typed server address skips Autodiscover.
//!
//! The window is told in kinds, never in words a server wrote ([`EwsSetupFailure`]).

use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use mailcraft_ews::autodiscover::{ConsentReason, Failure, Outcome, Step};
use mailcraft_ews::{Autodiscover, Error, HttpRequest, HttpResponse, PolicyReason, ResponseCode, Secret, TransportError};
use mailcraft_net::{CertificateFault, CertificateProblem, NetError};
use mailcraft_ui_egui::ews_setup::{EwsCertificate, EwsCertificateFault, EwsConsentReason, EwsSetupEnd, EwsSetupEvent, EwsSetupFailure, EwsSetupJob};

use crate::ews_runner::{AuthProblem, Transport, client, ntlm_key};

/// The most rounds of the search a job drives: every one is a request, a lookup or a question,
/// and each is bounded by the machine itself.
const ROUNDS: usize = 64;

/// Who is signing in.
pub(crate) struct Who {
    pub(crate) email: String,
    pub(crate) username: String,
    pub(crate) password: String,
}

/// How the test of the account came out at one address.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Probed {
    /// The Inbox could be had; signed in with `basic` or `ntlm`.
    Works(&'static str),
    /// The system doesn't trust the certificate: nothing was sent.
    Certificate(CertificateProblem),
    /// It did not work.
    Failed(EwsSetupFailure),
}

/// What setting up reaches the world with: the desktop's is [`Real`], the tests give a stand-in.
pub(crate) trait World {
    /// One Autodiscover request, exactly as the machine made it (no redirect followed).
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError>;
    /// The domain's mail exchangers; empty when they can't be had.
    fn mail_exchangers(&self, domain: &str) -> Vec<String>;
    /// The hosts and ports of the domain's `_autodiscover._tcp` SRV record; `None` when the lookup failed.
    fn srv(&self, name: &str) -> Option<Vec<(String, u16)>>;
    /// Test the account at `url`: one request for the Inbox.
    fn probe(&self, url: &str, who: &Who, trusted: &str) -> Probed;
    /// Look at the certificate of the server at `url`, signing nobody in.
    fn look(&self, url: &str, trusted: &str) -> Probed;
}

/// The desktop's way into the world.
pub(crate) struct Real;

/// The window's kind for a failed exchange of the search.
fn transport_failure(error: NetError) -> TransportError {
    match error {
        NetError::Certificate(_) | NetError::Tls { .. } => TransportError::Tls,
        NetError::Connect { .. } => TransportError::Unreachable,
        _ => TransportError::Other,
    }
}

impl World for Real {
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        // The request is the machine's: its `Authorization` is there only where the machine put it.
        let fields: Vec<(&str, &str)> = request.fields().collect();
        let ask = mailcraft_net::https::Request {
            method: request.method().as_str(),
            url: request.url(),
            fields: &fields,
            body: request.body(),
            max_response: request.max_response_bytes(),
            trusted: "",
        };
        mailcraft_net::https::send(&ask).map(Transport::response).map_err(transport_failure)
    }

    fn mail_exchangers(&self, domain: &str) -> Vec<String> {
        mailcraft_net::lookup_mx(domain).unwrap_or_default()
    }

    fn srv(&self, name: &str) -> Option<Vec<(String, u16)>> {
        // (The machine asks for `_autodiscover._tcp.<domain>`; the resolver adds the service itself.)
        let domain = name.strip_prefix("_autodiscover._tcp.")?;
        mailcraft_net::lookup_autodiscover(domain).ok().map(|targets| targets.into_iter().map(|t| (t.host, t.port)).collect())
    }

    fn probe(&self, url: &str, who: &Who, trusted: &str) -> Probed {
        let Ok(client) = client(url, &who.username, &who.email, &who.password) else { return Probed::Failed(EwsSetupFailure::BadSettings) };
        let transport = Transport::new(trusted).with_ntlm(ntlm_key(&who.username, &who.email, &who.password));
        match crate::block_on::block_on(client.check(&transport)) {
            Ok(_) => Probed::Works(transport.scheme_name().unwrap_or("basic")),
            Err(error) => judged(&transport, &error),
        }
    }

    fn look(&self, url: &str, trusted: &str) -> Probed {
        let ask = mailcraft_net::https::Request { method: "GET", url, fields: &[], body: &[], max_response: 16 * 1024, trusted };
        match mailcraft_net::https::send(&ask) {
            Ok(_) => Probed::Works("basic"),
            Err(NetError::Certificate(problem)) => Probed::Certificate(*problem),
            Err(NetError::Config(_)) => Probed::Failed(EwsSetupFailure::BadSettings),
            Err(NetError::Tls { .. }) => Probed::Failed(EwsSetupFailure::SecureConnection),
            Err(_) => Probed::Failed(EwsSetupFailure::Unreachable),
        }
    }
}

/// What a failed test was, from what the transport kept and what the client said.
pub(crate) fn judged(transport: &Transport, error: &Error) -> Probed {
    // A certificate that wasn't accepted comes first: it stopped the exchange before anything went.
    if let Some(problem) = transport.problem() {
        return Probed::Certificate(problem);
    }
    if let Some(problem) = transport.auth_problem() {
        return Probed::Failed(match problem {
            AuthProblem::NegotiateOnly | AuthProblem::NothingSupported => EwsSetupFailure::SignInNotSupported,
            AuthProblem::NtlmFailed => EwsSetupFailure::SignInRefused,
        });
    }
    Probed::Failed(match error {
        Error::Unauthorized => EwsSetupFailure::SignInRefused,
        Error::Blocked(PolicyReason::Disabled | PolicyReason::Blocked | PolicyReason::Other) | Error::Forbidden => EwsSetupFailure::NotAllowed,
        Error::Server(ResponseCode::AccessDenied) => EwsSetupFailure::NotAllowed,
        Error::Endpoint(_) => EwsSetupFailure::BadSettings,
        Error::Transport(TransportError::Unreachable) => EwsSetupFailure::Unreachable,
        Error::Transport(TransportError::Tls) => EwsSetupFailure::SecureConnection,
        Error::Malformed | Error::Unexpected | Error::TooLarge | Error::Status(_) => EwsSetupFailure::NotExchange,
        _ => EwsSetupFailure::Other,
    })
}

/// The window's name for a way the search failed.
fn failure_of(failure: Failure) -> EwsSetupFailure {
    match failure {
        Failure::Unreachable => EwsSetupFailure::Unreachable,
        Failure::NoAutodiscover => EwsSetupFailure::NoAutodiscover,
        Failure::SecureConnection => EwsSetupFailure::SecureConnection,
        Failure::Unreadable => EwsSetupFailure::Unreadable,
        Failure::InsecureRedirect => EwsSetupFailure::InsecureRedirect,
        Failure::ServerRefused => EwsSetupFailure::ServerRefused,
        Failure::NoExchangeAddress => EwsSetupFailure::NoExchangeAddress,
        Failure::SignInNotSupported => EwsSetupFailure::SignInNotSupported,
        Failure::SignInNtlmOnly => EwsSetupFailure::SignInNtlm,
        Failure::Declined => EwsSetupFailure::Declined,
        Failure::SignInRefused => EwsSetupFailure::SignInRefused,
        Failure::TooManyRedirects => EwsSetupFailure::TooManyRedirects,
        _ => EwsSetupFailure::Other,
    }
}

fn reason_of(reason: ConsentReason) -> EwsConsentReason {
    match reason {
        ConsentReason::Redirect => EwsConsentReason::Redirect,
        ConsentReason::OtherDomain => EwsConsentReason::OtherDomain,
        ConsentReason::MailServer => EwsConsentReason::MailServer,
    }
}

/// What the owner is asked, or told, about a certificate the test stopped at.
fn certificate_end(url: &str, problem: CertificateProblem) -> EwsSetupEnd {
    let changed = matches!(problem, CertificateProblem::Changed(_));
    match problem {
        CertificateProblem::Unacceptable(fault, _) => EwsSetupEnd::Failed(EwsSetupFailure::Certificate(match fault {
            CertificateFault::Expired => EwsCertificateFault::Expired,
            CertificateFault::NotYetValid => EwsCertificateFault::NotYetValid,
            CertificateFault::WrongName => EwsCertificateFault::WrongName,
        })),
        CertificateProblem::NotTrusted(info) | CertificateProblem::Changed(info) => EwsSetupEnd::Certificate(EwsCertificate {
            url: url.to_owned(),
            fingerprint: info.fingerprint,
            subject: info.subject,
            issuer: info.issuer,
            not_before: info.not_before,
            not_after: info.not_after,
            changed,
        }),
    }
}

/// Waits for the owner's answer; `None` when the owner gave up.
fn answer(job: &EwsSetupJob) -> Option<bool> {
    loop {
        match job.answers.recv_timeout(Duration::from_millis(100)) {
            Ok(yes) => return Some(yes),
            Err(RecvTimeoutError::Timeout) if job.cancel.load(Ordering::Relaxed) => return None,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// Test the account at each address in turn, the first that answers deciding.
fn test(world: &dyn World, urls: &[String], who: &Who, trusted: &str) -> EwsSetupEnd {
    let mut last = EwsSetupFailure::Unreachable;
    for url in urls {
        match world.probe(url, who, trusted) {
            Probed::Works(with) => return EwsSetupEnd::Works { url: url.clone(), signed_in_with: with.to_owned() },
            Probed::Certificate(problem) => return certificate_end(url, problem),
            // (Only a server that wasn't there makes the next address worth a try.)
            Probed::Failed(EwsSetupFailure::Unreachable) => last = EwsSetupFailure::Unreachable,
            Probed::Failed(failure) => return EwsSetupEnd::Failed(failure),
        }
    }
    EwsSetupEnd::Failed(last)
}

/// Whether `url` is on one of Microsoft's mail hosts (Exchange Online, Outlook.com).
pub(crate) fn on_microsoft(url: &str) -> bool {
    let server = mailcraft_model::EwsServer { url: url.trim().to_owned(), ..Default::default() };
    server.host().is_some_and(mailcraft_model::is_exchange_online)
}

/// The job, start to end; `emit` is told of the owner's questions as they come.
fn decide(world: &dyn World, job: &EwsSetupJob, emit: &mut dyn FnMut(EwsSetupEvent)) -> EwsSetupEnd {
    let who = Who { email: job.email.clone(), username: job.username.clone(), password: job.password.clone() };
    if job.certificate_only {
        if on_microsoft(&job.url) {
            return EwsSetupEnd::ExchangeOnline;
        }
        return match world.look(&job.url, &job.trusted) {
            Probed::Certificate(problem) => certificate_end(&job.url, problem),
            Probed::Works(_) => EwsSetupEnd::CertificateFine,
            Probed::Failed(failure) => EwsSetupEnd::Failed(failure),
        };
    }
    // The owner named the server: no search. A Microsoft host is never signed in to as an Exchange
    // server: that mailbox is a Microsoft account, and nothing is sent.
    if !job.url.trim().is_empty() {
        if on_microsoft(&job.url) {
            return EwsSetupEnd::ExchangeOnline;
        }
        return test(world, &[job.url.trim().to_owned()], &who, &job.trusted);
    }
    let name = if who.username.trim().is_empty() { who.email.trim() } else { who.username.trim() };
    let Ok(secret) = Secret::basic(name, &who.password) else { return EwsSetupEnd::Failed(EwsSetupFailure::BadSettings) };
    let Ok(mut search) = Autodiscover::new(&who.email, secret) else { return EwsSetupEnd::Failed(EwsSetupFailure::BadSettings) };
    // Whose mail the domain's exchangers say it is, before anything is sent to the domain.
    if let Some((_, domain)) = who.email.trim().rsplit_once('@') {
        search.note_mail_exchangers(&world.mail_exchangers(&domain.trim().to_ascii_lowercase()));
    }
    let outcome = 'search: {
        for _ in 0..ROUNDS {
            if job.cancel.load(Ordering::Relaxed) {
                return EwsSetupEnd::Failed(EwsSetupFailure::Other);
            }
            match search.step() {
                Step::Request(request) => search.response(world.exchange(&request)),
                Step::LookupSrv(name) => search.srv(world.srv(name.as_str())),
                Step::NeedsYes(consent) => {
                    emit(EwsSetupEvent::Consent { host: consent.host().to_owned(), reason: reason_of(consent.reason()) });
                    match answer(job) {
                        Some(true) => search.agree(),
                        Some(false) => search.decline(),
                        None => return EwsSetupEnd::Failed(EwsSetupFailure::Declined),
                    }
                }
                Step::Done(outcome) => break 'search outcome,
            }
        }
        Outcome::Failed(Failure::TooManyRedirects)
    };
    match outcome {
        Outcome::Found(found) => {
            let urls: Vec<String> = found.urls().into_iter().map(str::to_owned).collect();
            test(world, &urls, &who, &job.trusted)
        }
        Outcome::ExchangeOnline => EwsSetupEnd::ExchangeOnline,
        Outcome::Failed(failure) => EwsSetupEnd::Failed(failure_of(failure)),
    }
}

/// Runs a setup job to its end, telling `emit` of the owner's questions and, last, how it ended
/// (nothing, when the owner has given up).
pub(crate) fn setup_with(world: &dyn World, job: &EwsSetupJob, emit: &mut dyn FnMut(EwsSetupEvent)) {
    let end = decide(world, job, emit);
    if !job.cancel.load(Ordering::Relaxed) {
        emit(EwsSetupEvent::Finished(end));
    }
}

/// Runs a setup job on a thread of its own, waking the window for each thing it tells.
pub(crate) fn run_setup(job: EwsSetupJob, wake: &egui::Context) -> Receiver<EwsSetupEvent> {
    let (tx, rx) = channel();
    let wake = wake.clone();
    let spawned = std::thread::Builder::new().name("exchange account setup".into()).spawn(move || {
        setup_with(&Real, &job, &mut |event| {
            // Nobody is listening any more (the dialog closed): stop.
            if tx.send(event).is_err() {
                job.cancel.store(true, Ordering::Relaxed);
            }
            wake.request_repaint();
        });
    });
    // A dropped sender reads as "stopped unexpectedly" in the window.
    if let Err(e) = spawned {
        log::error!("couldn't start setting up an Exchange server account: {e}");
    }
    rx
}
