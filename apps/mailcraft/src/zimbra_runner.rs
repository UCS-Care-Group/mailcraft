// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of a Zimbra account: `mailcraft-zimbra` runs its requests over [`Transport`],
//! which is one HTTPS exchange of `mailcraft_net::https` at a time, on a thread of the run's own,
//! and the pieces of the run reach the window through a channel.
//!
//! **The credential** is the password the window hands over with the job, with the code of a
//! second step when the owner typed one, or the trusted device's token. They live in memory for
//! the length of the run, are not written anywhere, and are in no log line. What the sign-in brings
//! to be kept (a trusted-device token) goes back to the window through the job's slot, never
//! through a piece.
//!
//! **The certificate.** The system's trust first; a certificate the system doesn't trust is accepted
//! only when it is exactly the one the account trusts (its fingerprint). When it is not accepted the
//! first request does not go, so no password has gone either, and the run's last piece carries the
//! question (`ZimbraMailResult::untrusted`).

use std::future::{Future, ready};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, sync_channel};

use mailcraft_model::sync::{GraphFetchResult, ZimbraMailResult};
use mailcraft_net::{CertificateProblem, NetError};
use mailcraft_ui_egui::zimbra_mail::{ZimbraFetchJob, ZimbraJob};
use mailcraft_zimbra::{Credentials, HttpExchange, HttpRequest, HttpResponse, Secret, TransportError};

/// One HTTPS exchange after another with the server of one account.
pub(crate) struct Transport {
    /// The fingerprint of the certificate the account trusts besides the system's; empty for none.
    trusted: String,
    /// Why the server's certificate was not accepted, when that is what stopped an exchange.
    stopped_at: Mutex<Option<CertificateProblem>>,
}

impl Transport {
    pub(crate) fn new(trusted: &str) -> Self {
        Self { trusted: trusted.trim().to_owned(), stopped_at: Mutex::new(None) }
    }

    /// What the certificate question is, when the run was stopped by one.
    pub(crate) fn problem(&self) -> Option<CertificateProblem> {
        self.stopped_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let fields: Vec<(&str, &str)> = request.fields().collect();
        let ask = mailcraft_net::https::Request {
            method: request.method().as_str(),
            url: request.url(),
            fields: &fields,
            body: request.body(),
            max_response: request.max_response_bytes(),
            trusted: &self.trusted,
        };
        match mailcraft_net::https::send(&ask) {
            Ok(answer) => {
                let mut response = HttpResponse::new(answer.status, answer.body);
                if let Some(location) = &answer.location {
                    response = response.with_field("Location", location);
                }
                if let Some(wait) = &answer.retry_after {
                    response = response.with_field("Retry-After", wait);
                }
                Ok(response)
            }
            Err(error) => {
                let after_write = !request.is_repeatable()
                    && !matches!(error, NetError::Connect { .. } | NetError::Certificate(_) | NetError::Tls { .. } | NetError::Config(_));
                let judged = match error {
                    NetError::Certificate(problem) => {
                        *self.stopped_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(*problem);
                        TransportError::Tls
                    }
                    NetError::Tls { .. } => TransportError::Tls,
                    NetError::Connect { .. } => TransportError::Unreachable,
                    _ => TransportError::Other,
                };
                Err(if after_write { TransportError::Lost } else { judged })
            }
        }
    }
}

impl HttpExchange for Transport {
    fn exchange(&self, request: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        // (The call blocks, on the run's own thread.)
        ready(self.send(request))
    }
}

/// The last piece of a run, with the certificate question in it when a certificate stopped the run.
fn settled(mut piece: ZimbraMailResult, problem: Option<&CertificateProblem>) -> ZimbraMailResult {
    let Some(problem) = problem.filter(|_| !piece.partial) else { return piece };
    piece.whole = false;
    piece.errors.clear();
    if problem.can_be_trusted() {
        piece.untrusted = problem.info().fingerprint.clone();
    } else {
        piece.errors.push(format!("{problem}"));
    }
    piece
}

/// What a job signs in with.
fn credentials(secret: &str, code: Option<&str>, trusted: Option<&(String, String)>) -> Credentials {
    Credentials {
        password: Secret::new(secret),
        code: code.map(Secret::new),
        trusted: trusted.map(|(token, device)| (Secret::new(token), device.clone())),
    }
}

/// Runs a Zimbra run on a thread of its own, handing the pieces to the window as they come.
pub(crate) fn run_mail(job: ZimbraJob, wake: &egui::Context) -> Receiver<ZimbraMailResult> {
    let (tx, rx) = sync_channel(8);
    let wake = wake.clone();
    let spawned = std::thread::Builder::new().name("zimbra send/receive".into()).spawn(move || {
        let ZimbraJob { request, secret, code, trusted, trust, cancel } = job;
        // The address is checked again before a password is sent to it: the mailbox file that
        // carried it is hostile too.
        if let Err(why) = mailcraft_model::ZimbraServer::check_url(request.url.trim()) {
            let mut piece = ZimbraMailResult { account: request.account, ..ZimbraMailResult::default() };
            piece.errors.push(format!("The Zimbra server's address {why}"));
            let _ = tx.send(piece);
            return;
        }
        let transport = Transport::new(&request.trusted);
        let credentials = credentials(&secret, code.as_deref(), trusted.as_ref());
        // The trust the sign-in brought is in the slot before any piece, so that the window finds it
        // with the last one.
        let mut keep = |brought: &mailcraft_zimbra::Trust| {
            if let Some(token) = &brought.token
                && let Ok(mut slot) = trust.lock()
            {
                *slot = Some((token.expose().to_string(), brought.device.clone()));
            }
        };
        crate::block_on::block_on(mailcraft_zimbra::sync_mail(&transport, &request, &credentials, &cancel, &mut keep, &mut |piece| {
            let piece = settled(piece, transport.problem().as_ref());
            if tx.send(piece).is_err() {
                cancel.store(true, Ordering::Relaxed);
            }
            wake.request_repaint();
        }));
    });
    if let Err(e) = spawned {
        log::error!("couldn't start Send/Receive for a Zimbra account: {e}");
    }
    rx
}

/// Fetches messages of a Zimbra account whole on a thread of its own.
pub(crate) fn run_fetch(job: ZimbraFetchJob, wake: &egui::Context) -> Receiver<GraphFetchResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = wake.clone();
    let spawned = std::thread::Builder::new().name("zimbra message download".into()).spawn(move || {
        let ZimbraFetchJob { request, secret, trusted, cancel } = job;
        if let Err(why) = mailcraft_model::ZimbraServer::check_url(request.url.trim()) {
            let mut result = GraphFetchResult { account: request.account, ..GraphFetchResult::default() };
            result.errors.push(format!("The Zimbra server's address {why}"));
            let _ = tx.send(result);
            return;
        }
        let transport = Transport::new(&request.trusted);
        let credentials = credentials(&secret, None, trusted.as_ref());
        let mut result = crate::block_on::block_on(mailcraft_zimbra::fetch_messages(&transport, &request, &credentials, &cancel));
        if let Some(problem) = transport.problem() {
            result.errors.insert(0, format!("Couldn't download the messages from the server: {problem}"));
        }
        let _ = tx.send(result);
        wake.request_repaint();
    });
    if let Err(e) = spawned {
        log::error!("couldn't start a download for a Zimbra account: {e}");
    }
    rx
}
