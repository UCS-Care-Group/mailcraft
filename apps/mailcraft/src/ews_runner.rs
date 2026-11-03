// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of an Exchange server account (#252, E6): `mailcraft-ews` runs its requests over
//! [`Transport`], which is one HTTPS exchange of `mailcraft_net::https` at a time, on a thread of
//! the run's own, and the pieces of the run reach the window through a channel.
//!
//! **The credential** is the account's user name (the settings', else its address) and the password
//! the window hands over with the job. It lives in memory for the length of the run, is not written
//! anywhere, and is in no log line.
//!
//! **Signing in** is chosen from what the server offers (`mailcraft_ews::auth::choose`, NTLM before
//! Basic). The first request of a run goes with no credential at all; the server's 401 says how to
//! sign in. Basic then sends the credential with each request, over TLS. NTLM (MS-NLMP) runs over
//! one held connection (`mailcraft_net::https::Connection`): the three legs, then the run's
//! requests on the same connection. A server that offers Kerberos (Negotiate) alone is told in
//! words ([`AuthProblem`]) and nothing is retried. A challenge the server sent is never shown or
//! logged.
//!
//! **A connection that closes** is opened again and signed in again, but only for a request that
//! is the same however often it is made (a read, a set of a property): a request that makes or
//! sends something (`HttpRequest::is_repeatable` is false) whose answer was lost after it went out
//! is never sent again ([`TransportError::Lost`]): a send is held, and the rest is checked
//! against the server by the next run.
//!
//! **The certificate.** The system's trust first; a certificate the system doesn't trust is accepted
//! only when it is exactly the one the account trusts (its fingerprint). When it is not accepted the
//! first request does not go, so no password has gone either, and `mailcraft_net::NetError::Certificate`
//! comes out of `https::send`. It is *not* a plain TLS error here: the transport keeps what it says
//! ([`Transport::problem`]) and the run's last piece carries the question (`EwsMailResult::untrusted`
//! is the fingerprint of a certificate the owner may be asked to trust, or that has changed since
//! they did), so that the window stops and the account says it needs the certificate question. A
//! certificate nobody may trust (expired, for another name) is told in words and never offered.
//!
//! **Waiting.** A busy server asks the run to wait; the thread sleeps in small steps, so that Cancel
//! is heard, and gives up only when the run is called off.

use std::future::{Future, ready};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_ews::{AuthError, Client, HttpExchange, HttpRequest, HttpResponse, NtlmCredentials, Scheme, Secret, TransportError, auth, ntlm};
use mailcraft_model::sync::{EwsFetchRequest, EwsMailRequest, EwsMailResult, GraphFetchResult};
use mailcraft_net::https::Connection;
use mailcraft_net::{CertificateProblem, NetError};
use mailcraft_ui_egui::ews_mail::{EwsFetchJob, EwsJob};

/// What the run says when the secure connection could not be had for a reason the owner can't
/// answer (an expired certificate, one for another server): in words with nothing of the certificate.
const NO_SECURE_LINE: &str = "Couldn't set up a secure connection with the server";

/// Why the sign-in could not be had, in words for the run's last line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthProblem {
    /// The server offers Kerberos (Negotiate) and nothing MailCraft speaks.
    NegotiateOnly,
    /// The server offers no way of signing in that MailCraft speaks.
    NothingSupported,
    /// The NTLM sign-in could not be completed (the server's challenge was not one, or the user
    /// name or the password cannot be sent that way).
    NtlmFailed,
}

impl AuthProblem {
    /// The line the run ends with. Names neither the server nor the user.
    pub(crate) fn words(self) -> &'static str {
        match self {
            Self::NegotiateOnly => "The server only signs in with Kerberos (Negotiate), which MailCraft does not speak yet",
            Self::NothingSupported => "The server offers no way of signing in that MailCraft speaks (NTLM or Basic)",
            Self::NtlmFailed => "The NTLM sign-in with the server could not be completed",
        }
    }
}

/// How the run signs in, once the server has said.
#[derive(Default)]
struct SignIn {
    scheme: Option<Scheme>,
    /// The held connection of an NTLM run, signed in.
    connection: Option<Connection>,
}

/// One HTTPS exchange after another with the server of one account.
pub(crate) struct Transport {
    /// The fingerprint of the certificate the account trusts besides the system's; empty for none.
    trusted: String,
    /// Why the server's certificate was not accepted, when that is what stopped an exchange.
    stopped_at: Mutex<Option<CertificateProblem>>,
    /// The key NTLM signs in with, when the user name and password can be sent that way.
    ntlm: Option<NtlmCredentials>,
    sign_in: Mutex<SignIn>,
    /// Why the sign-in could not be had, when that is what stopped the run.
    auth_problem: Mutex<Option<AuthProblem>>,
}

/// The host of an address (`mail.example.org` of `https://mail.example.org:444/EWS/…`), for the
/// target name of an NTLM sign-in.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    match authority.strip_prefix('[') {
        Some(inside) => inside.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    }
}

impl Transport {
    pub(crate) fn new(trusted: &str) -> Self {
        Self {
            trusted: trusted.trim().to_owned(),
            stopped_at: Mutex::new(None),
            ntlm: None,
            sign_in: Mutex::new(SignIn::default()),
            auth_problem: Mutex::new(None),
        }
    }

    /// With the key NTLM signs in with, for a server that offers it.
    #[must_use]
    pub(crate) fn with_ntlm(mut self, credentials: Option<NtlmCredentials>) -> Self {
        self.ntlm = credentials;
        self
    }

    /// What the certificate question is, when the run was stopped by one.
    pub(crate) fn problem(&self) -> Option<CertificateProblem> {
        self.stopped_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// How the run signed in, `basic` or `ntlm`, once the server has said; `None` before.
    pub(crate) fn scheme_name(&self) -> Option<&'static str> {
        match self.sign_in.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).scheme {
            Some(Scheme::Ntlm) => Some("ntlm"),
            Some(Scheme::Basic) => Some("basic"),
            None => None,
        }
    }

    /// Why the sign-in could not be had, when that stopped the run.
    pub(crate) fn auth_problem(&self) -> Option<AuthProblem> {
        *self.auth_problem.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn note(&self, problem: AuthProblem) {
        *self.auth_problem.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(problem);
    }

    /// The fields of `request` to send: its own, with the `Authorization` given (none: no credential).
    fn fields<'r>(request: &'r HttpRequest, authorization: Option<&'r str>) -> Vec<(&'r str, &'r str)> {
        let mut fields: Vec<(&str, &str)> = request.fields().filter(|(name, _)| !name.eq_ignore_ascii_case("authorization")).collect();
        if let Some(value) = authorization {
            fields.push(("Authorization", value));
        }
        fields
    }

    pub(crate) fn response(answer: mailcraft_net::https::Response) -> HttpResponse {
        let mut response = HttpResponse::new(answer.status, answer.body);
        if let Some(location) = &answer.location {
            response = response.with_field("Location", location);
        }
        for challenge in &answer.challenges {
            response = response.with_field("WWW-Authenticate", challenge);
        }
        if let Some(wait) = &answer.retry_after {
            response = response.with_field("Retry-After", wait);
        }
        if let Some(why) = &answer.policy_reason {
            response = response.with_field("X-EWS-Policy-Reason", why);
        }
        response
    }

    /// One request on a connection of its own (no sign-in held): with the request's own credential
    /// (Basic), or with none.
    fn alone(&self, request: &HttpRequest, with_credential: bool) -> Result<mailcraft_net::https::Response, TransportError> {
        self.alone_with(request, with_credential, request.body())
    }

    /// [`Transport::alone`] with `body` in place of the request's own.
    fn alone_with(&self, request: &HttpRequest, with_credential: bool, body: &[u8]) -> Result<mailcraft_net::https::Response, TransportError> {
        let authorization = with_credential.then(|| request.field("authorization")).flatten();
        let fields = Self::fields(request, authorization);
        let ask = mailcraft_net::https::Request {
            method: request.method().as_str(),
            url: request.url(),
            fields: &fields,
            body,
            max_response: request.max_response_bytes(),
            trusted: &self.trusted,
        };
        mailcraft_net::https::send(&ask).map_err(|error| self.judged_for(error, request))
    }

    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let mut state = self.sign_in.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.scheme {
            Some(Scheme::Basic) => self.alone(request, true).map(Self::response),
            Some(Scheme::Ntlm) => self.over_ntlm(&mut state, request),
            None => {
                // The first request goes with no credential: the server says how to sign in. It goes
                // with no body (a message being sent is not uploaded to be turned away, and then
                // uploaded again): its answer is the probe's, and the request itself is made
                // only once the way to sign in is known.
                let answer = self.alone_with(request, false, &[])?;
                if answer.status != 401 {
                    state.scheme = Some(Scheme::Basic);
                    return self.alone(request, true).map(Self::response);
                }
                match auth::choose(&answer.challenges) {
                    Ok(Scheme::Basic) => {
                        state.scheme = Some(Scheme::Basic);
                        self.alone(request, true).map(Self::response)
                    }
                    Ok(Scheme::Ntlm) => {
                        state.scheme = Some(Scheme::Ntlm);
                        self.over_ntlm(&mut state, request)
                    }
                    Err(error) => {
                        self.note(match error {
                            AuthError::NegotiateOnly => AuthProblem::NegotiateOnly,
                            AuthError::NothingSupported => AuthProblem::NothingSupported,
                        });
                        // (Not a password the server turned down: no challenge goes with the answer.)
                        Ok(HttpResponse::new(401, Vec::new()))
                    }
                }
            }
        }
    }

    /// A request on the held connection, opened and signed in when there is none. A connection that
    /// has closed is opened again, for a request that may be made again; one whose request may have
    /// gone out and is not to be repeated is [`TransportError::Lost`].
    fn over_ntlm(&self, state: &mut SignIn, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let Some(credentials) = &self.ntlm else {
            self.note(AuthProblem::NtlmFailed);
            return Ok(HttpResponse::new(401, Vec::new()));
        };
        for attempt in 0..2 {
            let fresh = !state.connection.as_ref().is_some_and(Connection::is_open);
            let mut third: Option<Secret> = None;
            if fresh {
                state.connection = None;
                let mut connection = Connection::open(request.url(), &self.trusted).map_err(|e| self.judged_for(e, request))?;
                let hash = connection.end_point_hash();
                let context = ntlm::Context {
                    client_challenge: mailcraft_net::https::client_challenge().map_err(|_| TransportError::Other)?,
                    unix_seconds: u64::try_from(mailcraft_time::now()).unwrap_or(0),
                    host: host_of(request.url()),
                    channel_binding: hash.as_deref(),
                };
                let signed = ntlm::sign_in(credentials, &context, |negotiate| {
                    let fields = [("Authorization", negotiate.expose()), ("Content-Type", "text/xml; charset=utf-8")];
                    let ask = mailcraft_net::https::Request {
                        method: "POST",
                        url: request.url(),
                        fields: &fields,
                        body: &[],
                        max_response: request.max_response_bytes().min(64 * 1024),
                        trusted: &self.trusted,
                    };
                    connection.send(&ask).map(|answer| ntlm::Leg { status: answer.status, challenges: answer.challenges })
                });
                match signed {
                    Ok(secret) => third = Some(secret),
                    // (The connection failed before the request went: nothing was done.)
                    Err(ntlm::SignInError::Exchange(error)) => return Err(self.judged_for(error, request)),
                    // The server answered something else than a challenge: that answer is the run's.
                    Err(ntlm::SignInError::Unexpected(status)) => return Ok(HttpResponse::new(status, Vec::new())),
                    Err(ntlm::SignInError::Ntlm(_)) => {
                        self.note(AuthProblem::NtlmFailed);
                        return Ok(HttpResponse::new(401, Vec::new()));
                    }
                }
                state.connection = Some(connection);
            }
            let Some(connection) = state.connection.as_mut() else { return Err(TransportError::Other) };
            let fields = Self::fields(request, third.as_ref().map(Secret::expose));
            let ask = mailcraft_net::https::Request {
                method: request.method().as_str(),
                url: request.url(),
                fields: &fields,
                body: request.body(),
                max_response: request.max_response_bytes(),
                trusted: &self.trusted,
            };
            match connection.send(&ask) {
                Ok(answer) => return Ok(Self::response(answer)),
                Err(error) if mailcraft_net::https::is_closed(&error) => {
                    state.connection = None;
                    if !request.is_repeatable() {
                        // (It may have gone out before the connection closed.)
                        return Err(TransportError::Lost);
                    }
                    if fresh || attempt > 0 {
                        return Err(TransportError::Unreachable);
                    }
                    // A connection the server closed while it was idle: a new one, signed in again.
                }
                Err(error) => {
                    state.connection = None;
                    return Err(self.judged_for(error, request));
                }
            }
        }
        Err(TransportError::Unreachable)
    }

    /// What a failed exchange is to the run, for `request`: a request that is never repeated and may
    /// have gone out is lost, not a failure to connect.
    fn judged_for(&self, error: NetError, request: &HttpRequest) -> TransportError {
        // (Not reached the server, or not past the handshake: nothing of the request went out.)
        let after_write = !request.is_repeatable()
            && !matches!(error, NetError::Connect { .. } | NetError::Certificate(_) | NetError::Tls { .. } | NetError::Config(_));
        let judged = self.judged(error);
        if after_write { TransportError::Lost } else { judged }
    }

    /// What a failed exchange is to the run. A certificate that is not accepted is kept apart (never
    /// a plain TLS error to the run's machine: it ends the run at once and asks); the rest is
    /// told by its kind.
    fn judged(&self, error: NetError) -> TransportError {
        match error {
            NetError::Certificate(problem) => {
                *self.stopped_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(*problem);
                TransportError::Tls
            }
            NetError::Tls { .. } => TransportError::Tls,
            NetError::Connect { .. } => TransportError::Unreachable,
            _ => TransportError::Other,
        }
    }
}

impl HttpExchange for Transport {
    fn exchange(&self, request: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        // (The call blocks, on the run's own thread.)
        ready(self.send(request))
    }
}

/// The last piece of a run, with the certificate question in it when a certificate stopped the run:
/// the fingerprint of one the owner may be asked about, else the words that it can't be trusted. The
/// line the run wrote for the secure connection that failed is replaced, not added to.
pub(crate) fn settled(mut piece: EwsMailResult, problem: Option<&CertificateProblem>, auth: Option<AuthProblem>) -> EwsMailResult {
    // A sign-in that could not be had is told in words, and is no password to ask for again.
    if let Some(auth) = auth.filter(|_| !piece.partial) {
        piece.auth_refused = false;
        piece.whole = false;
        if !piece.errors.iter().any(|line| line == auth.words()) {
            piece.errors.insert(0, auth.words().to_owned());
        }
        return piece;
    }
    let Some(problem) = problem.filter(|_| !piece.partial) else { return piece };
    piece.errors.retain(|line| line != NO_SECURE_LINE);
    piece.whole = false;
    if problem.can_be_trusted() {
        piece.untrusted = problem.info().fingerprint.clone();
    } else {
        piece.errors.insert(0, format!("{problem}"));
    }
    piece
}

/// The same for a batch fetched: no question can be asked from there, so it is told in words.
pub(crate) fn settled_fetch(mut result: GraphFetchResult, problem: Option<&CertificateProblem>, auth: Option<AuthProblem>) -> GraphFetchResult {
    if let Some(auth) = auth {
        result.errors.insert(0, format!("Couldn't download the messages from the server: {}", auth.words()));
        return result;
    }
    if let Some(problem) = problem {
        result.errors.retain(|line| !line.ends_with(NO_SECURE_LINE));
        result.errors.insert(0, format!("Couldn't download the messages from the server: {problem}"));
    }
    result
}

/// The Basic credential of `username` (the address when it is empty) and `password`, as a client for
/// `url`. Why not: words that name neither.
pub(crate) fn client(url: &str, username: &str, email: &str, password: &str) -> Result<Client, &'static str> {
    // A mailbox file is hostile too: an account that names a Microsoft host is never signed in to
    // here, whatever the file says (its mailbox is a Microsoft account).
    if crate::ews_setup::on_microsoft(url) {
        return Err("This server is Microsoft's: add the mailbox as a Microsoft account instead");
    }
    let name = if username.trim().is_empty() { email.trim() } else { username.trim() };
    let secret = Secret::basic(name, password).map_err(|_| "The user name or the password can't be used to sign in")?;
    Client::new(url, secret).map_err(|_| "The server's address can't be used")
}

/// The NTLM key for the same user name and password, when they can be sent that way.
pub(crate) fn ntlm_key(username: &str, email: &str, password: &str) -> Option<NtlmCredentials> {
    let name = if username.trim().is_empty() { email.trim() } else { username.trim() };
    NtlmCredentials::new(name, password).ok()
}

/// Sleeps `ms` milliseconds in small steps; false when the run was called off meanwhile.
fn wait(cancel: &AtomicBool, ms: u32) -> bool {
    let mut left = u64::from(ms);
    while left > 0 {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let step = left.min(100);
        std::thread::sleep(Duration::from_millis(step));
        left = left.saturating_sub(step);
    }
    !cancel.load(Ordering::Relaxed)
}

/// Runs an Exchange run on a thread of its own, handing the pieces to the window as they come.
pub(crate) fn run_mail(job: EwsJob, wake: &egui::Context) -> Receiver<EwsMailResult> {
    // Through a channel short enough that a busy window holds the run back instead of piling pieces up.
    let (tx, rx) = sync_channel(8);
    let wake = wake.clone();
    let spawned = std::thread::Builder::new().name("exchange send/receive".into()).spawn(move || {
        let EwsJob { request, secret, cancel } = job;
        run_mail_with(&request, &secret, &cancel, &mut |piece| {
            // Nobody is listening any more (the window closed): stop.
            if tx.send(piece).is_err() {
                cancel.store(true, Ordering::Relaxed);
            }
            wake.request_repaint();
        });
    });
    // A dropped sender reads as "stopped unexpectedly" in the app.
    if let Err(e) = spawned {
        log::error!("couldn't start Send/Receive for an Exchange server account: {e}");
    }
    rx
}

/// [`run_mail`] on this thread: every piece through `emit`.
pub(crate) fn run_mail_with(request: &EwsMailRequest, password: &str, cancel: &AtomicBool, emit: &mut dyn FnMut(EwsMailResult)) {
    let client = match client(&request.url, &request.username, &request.email, password) {
        Ok(client) => client,
        Err(why) => return emit(mailcraft_ews::failed_run(request, why)),
    };
    let transport = Transport::new(&request.trusted).with_ntlm(ntlm_key(&request.username, &request.email, password));
    let mut pause = |ms: u32| wait(cancel, ms);
    crate::block_on::block_on(mailcraft_ews::sync_mail(&client, &transport, request, cancel, &mut pause, &mut |piece| {
        let mut piece = settled(piece, transport.problem().as_ref(), transport.auth_problem());
        // How it signed in: File › Info says so.
        if let Some(scheme) = transport.scheme_name() {
            scheme.clone_into(&mut piece.signed_in_with);
        }
        emit(piece);
    }));
}

/// Fetches messages of an Exchange server account whole on a thread of its own.
pub(crate) fn run_fetch(job: EwsFetchJob, wake: &egui::Context) -> Receiver<GraphFetchResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = wake.clone();
    let spawned = std::thread::Builder::new().name("exchange message download".into()).spawn(move || {
        let EwsFetchJob { request, secret, cancel } = job;
        let _ = tx.send(run_fetch_with(&request, &secret, &cancel));
        wake.request_repaint();
    });
    if let Err(e) = spawned {
        log::error!("couldn't start a download for an Exchange server account: {e}");
    }
    rx
}

/// [`run_fetch`] on this thread.
pub(crate) fn run_fetch_with(request: &EwsFetchRequest, password: &str, cancel: &Arc<AtomicBool>) -> GraphFetchResult {
    let client = match client(&request.url, &request.username, &request.email, password) {
        Ok(client) => client,
        Err(why) => {
            return GraphFetchResult { account: request.account, email: request.email.clone(), errors: vec![why.to_owned()], ..Default::default() };
        }
    };
    let transport = Transport::new(&request.trusted).with_ntlm(ntlm_key(&request.username, &request.email, password));
    let mut pause = |ms: u32| wait(cancel, ms);
    let result = crate::block_on::block_on(mailcraft_ews::fetch_messages(&client, &transport, request, cancel, &mut pause));
    settled_fetch(result, transport.problem().as_ref(), transport.auth_problem())
}

#[cfg(test)]
#[path = "ews_runner_tests.rs"]
mod tests;
