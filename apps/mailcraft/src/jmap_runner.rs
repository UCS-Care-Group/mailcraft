// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop's side of a JMAP account (M6): `mailcraft-jmap` runs its requests over
//! [`Transport`], which is one HTTPS exchange of `mailcraft_net::https` at a time, on a thread of
//! the job's own, and the pieces of a run reach the window through a channel. It is the host the
//! browser build has in `apps/mailcraft-web/src/jmap.rs`, with sockets in place of `fetch`.
//!
//! **The credential** is the password, or the bearer token when the account says so, that the window
//! hands over with the job. It lives in memory for the length of the job, goes only to the server
//! the session names (the client holds every address of the session to the session's own origin),
//! is not written anywhere and is in no log line. The desktop holds no credential of its own: a job
//! without one is refused as the server would refuse it, and the window asks the owner.
//!
//! **The connection** is `mailcraft_net::https`'s: TLS checked by the system's verifier, plain
//! `http` only to this machine (for tests), no redirect followed (the client follows the session's
//! own on its origin, RFC 8620 §2.2), no cookie, the answer read within the request's own limit and
//! refused unread past it.
//!
//! **Patience.** Each exchange has a time of its own for connecting and the whole answer, by what
//! it is for ([`patience`]): a session little, an API request more, a message downloaded whole the
//! most, so that a large message on a slow line comes down instead of timing out and being asked
//! for again and again.
//!
//! **Waiting.** A server's `Retry-After` given as a date is handed to the client as the seconds until
//! then (the client reads no clock); while this computer's clock is plainly not set yet (before
//! [`CLOCK_SET`], as after a boot that hasn't reached a time server), a date is no answer at all and
//! the client waits as it would without one. A busy server ends the run with how long it asked for,
//! as in the browser; the window waits that long before the next one.
//!
//! **Cancel** is heard before each request: an exchange under way finishes (within its patience)
//! and nothing after it goes. Once heard, it holds for the job ([`Stop`]): the window's Stop flag is
//! shared with the runs after it, which set it back, and a job that has seen it, or whose window no
//! longer listens, stops for good. A job never sets the window's flag itself.

use std::future::{Future, ready};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use mailcraft_jmap::fetch::{failed_fetch, fetch_messages};
use mailcraft_jmap::{
    Connection, Credentials, Error, HttpExchange, HttpRequest, HttpResponse, RequestKind, RetryAfter, TransportError, UnixTime, failed_run, sync_mail,
};
use mailcraft_model::sync::{GraphFetchResult, JmapFetchRequest, JmapMailRequest, JmapMailResult};
use mailcraft_net::NetError;
use mailcraft_ui_egui::jmap_mail::{JmapFetchJob, JmapJob};

/// A job's own stop: the window's flag (Stop, another mailbox opened), which is shared and set back
/// for the next run, latched into one that is the job's alone.
#[derive(Clone)]
pub(crate) struct Stop {
    window: Arc<AtomicBool>,
    mine: Arc<AtomicBool>,
}

impl Stop {
    pub(crate) fn new(window: Arc<AtomicBool>) -> Stop {
        Stop { window, mine: Arc::new(AtomicBool::new(false)) }
    }

    /// Whether the job is to stop; once it is, it stays so.
    pub(crate) fn stopped(&self) -> bool {
        if self.window.load(Ordering::Relaxed) {
            self.mine.store(true, Ordering::Relaxed);
        }
        self.mine.load(Ordering::Relaxed)
    }

    /// Stop this job, and no other (its window no longer listens).
    pub(crate) fn stop(&self) {
        self.mine.store(true, Ordering::Relaxed);
    }

    /// The flag the client reads between its steps: the job's own.
    fn flag(&self) -> &AtomicBool {
        &self.mine
    }
}

/// How long an exchange may take, connecting and the whole answer, by what it is for.
pub(crate) fn patience(kind: RequestKind) -> Duration {
    Duration::from_secs(match kind {
        RequestKind::Session => 30,
        RequestKind::Api => 120,
        // (A message whole: up to the client's 50 MB, at a few hundred KB a second.)
        RequestKind::Download => 300,
    })
}

/// The program's side of each exchange: `mailcraft_net::https`, one connection a request, until the
/// job is called off.
pub(crate) struct Transport {
    stop: Stop,
}

impl Transport {
    pub(crate) fn new(stop: Stop) -> Transport {
        Transport { stop }
    }

    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if self.stop.stopped() {
            return Err(TransportError::Aborted);
        }
        let fields: Vec<(&str, &str)> = request.fields().collect();
        let answer = mailcraft_net::https::send_with(
            &mailcraft_net::https::Request {
                method: request.method().as_str(),
                url: request.url(),
                fields: &fields,
                body: request.body(),
                max_response: request.max_response_bytes(),
                // No certificate the owner trusts beyond the system's: a JMAP account has none.
                trusted: "",
            },
            patience(request.kind()),
        )
        .map_err(|error| judged(&error))?;
        Ok(response(answer, now()))
    }
}

impl HttpExchange for Transport {
    fn exchange(&self, request: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        // (The call blocks, on the job's own thread.)
        ready(self.send(request))
    }
}

/// What a failed exchange is to the client, by its kind: nothing of the error's text goes on.
pub(crate) fn judged(error: &NetError) -> TransportError {
    match error {
        NetError::Certificate(_) | NetError::Tls { .. } => TransportError::Tls,
        NetError::Connect { .. } => TransportError::Unreachable,
        _ if mailcraft_net::https::is_too_slow(error) => TransportError::TimedOut,
        _ if mailcraft_net::https::is_too_long(error) => TransportError::BodyTooLarge,
        _ => TransportError::Other,
    }
}

/// The answer as the client reads it, at `now`: the fields it reads, and a `Retry-After` date as the
/// seconds until then.
pub(crate) fn response(answer: mailcraft_net::https::Response, now: UnixTime) -> HttpResponse {
    let mut response = HttpResponse::new(answer.status, answer.body);
    if let Some(kind) = &answer.content_type {
        response.set_field("Content-Type", kind);
    }
    if let Some(location) = &answer.location {
        response.set_field("Location", location);
    }
    match answer.retry_after.as_deref().and_then(RetryAfter::parse) {
        // A date read against a clock that isn't set yet would be an hour's wait, or none.
        Some(RetryAfter::At(_)) if now < CLOCK_SET => {}
        Some(wait) => response.set_field("Retry-After", &wait.delay_from(now).as_secs().to_string()),
        None => {}
    }
    response
}

/// A time this computer's clock is past once it is set: 2026-01-01T00:00:00Z.
pub(crate) const CLOCK_SET: UnixTime = UnixTime::from_secs(1_767_225_600);

/// The clock, for a `Retry-After` date.
fn now() -> UnixTime {
    UnixTime::from_secs(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs()))
}

/// The credentials a job goes with: the secret as a bearer token, or as the password of the
/// account's address. `None` when it can't be sent (empty, or more than a header field takes).
pub(crate) fn credentials(email: &str, bearer: bool, secret: &str) -> Option<Credentials> {
    if secret.is_empty() {
        return None;
    }
    if bearer { Credentials::bearer(secret).ok() } else { Credentials::basic(email.trim(), secret).ok() }
}

/// Runs a Send/Receive of a JMAP account on a thread of its own, handing the pieces to the window as
/// they come.
pub(crate) fn run_mail(job: JmapJob, wake: &egui::Context) -> Receiver<JmapMailResult> {
    // Through a channel short enough that a busy window holds the run back instead of piling pieces up.
    let (tx, rx) = sync_channel(8);
    let wake = wake.clone();
    let spawned = std::thread::Builder::new().name("jmap send/receive".into()).spawn(move || {
        let JmapJob { request, secret, cancel } = job;
        let stop = Stop::new(cancel);
        let mine = stop.clone();
        run_mail_with(&request, &secret, &stop, &mut |piece| {
            // Nobody is listening any more (the window closed, or opened another mailbox): this
            // job stops, and the window's flag is left to the runs after it.
            if tx.send(piece).is_err() {
                mine.stop();
            }
            wake.request_repaint();
        });
    });
    // A dropped sender reads as "stopped unexpectedly" in the app.
    if let Err(e) = spawned {
        log::error!("couldn't start Send/Receive for a JMAP account: {e}");
    }
    rx
}

/// [`run_mail`] on this thread: every piece through `emit`, the last one not partial.
pub(crate) fn run_mail_with(request: &JmapMailRequest, secret: &str, stop: &Stop, emit: &mut dyn FnMut(JmapMailResult)) {
    let Some(credentials) = credentials(&request.email, request.bearer, secret) else {
        return emit(failed_run(request, &Error::CredentialsRefused));
    };
    crate::block_on::block_on(async {
        match Connection::connect(Transport::new(stop.clone()), &request.session, credentials).await {
            Ok(connection) => sync_mail(&connection, request, stop.flag(), emit).await,
            // Stopped while signing in: as before anything was read.
            Err(Error::Transport(TransportError::Aborted)) if stop.stopped() => emit(stopped(request)),
            Err(error) => emit(failed_run(request, &error)),
        }
    });
}

/// The last piece of a run stopped before it read anything.
fn stopped(request: &JmapMailRequest) -> JmapMailResult {
    JmapMailResult { account: request.account, email: request.email.clone(), session: request.session.clone(), stopped: true, ..Default::default() }
}

/// Fetches messages of a JMAP account whole on a thread of its own.
pub(crate) fn run_fetch(job: JmapFetchJob, wake: &egui::Context) -> Receiver<GraphFetchResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = wake.clone();
    let spawned = std::thread::Builder::new().name("jmap message download".into()).spawn(move || {
        let JmapFetchJob { request, secret, cancel } = job;
        let _ = tx.send(run_fetch_with(&request, &secret, &Stop::new(cancel)));
        wake.request_repaint();
    });
    if let Err(e) = spawned {
        log::error!("couldn't start a download for a JMAP account: {e}");
    }
    rx
}

/// [`run_fetch`] on this thread.
pub(crate) fn run_fetch_with(request: &JmapFetchRequest, secret: &str, stop: &Stop) -> GraphFetchResult {
    let Some(credentials) = credentials(&request.email, request.bearer, secret) else {
        return failed_fetch(request, &Error::CredentialsRefused);
    };
    crate::block_on::block_on(async {
        match Connection::connect(Transport::new(stop.clone()), &request.session, credentials).await {
            Ok(connection) => fetch_messages(&connection, request, stop.flag()).await,
            // Called off while signing in: nothing is read, and nothing is said of it.
            Err(Error::Transport(TransportError::Aborted)) if stop.stopped() => {
                GraphFetchResult { account: request.account, email: request.email.clone(), session: request.session.clone(), ..Default::default() }
            }
            Err(error) => failed_fetch(request, &error),
        }
    })
}

// What follows is not in the runner above, and is built on its pieces ([`Transport`], [`Stop`],
// [`credentials`]): an account's contacts (RFC 9610) and its calendars (JMAP for Calendars), each
// read in one answer on a thread of the job's own and never written; and what a job says when it
// has no credential it could send. Their jobs carry a stop of their own, which is latched as a
// run's is ([`Stop::new`]). What a job of one kind of an account's data does whatever the kind is in
// [`on_thread`] and [`signed_in`].

use mailcraft_jmap::{Redactor, failed_calendar, failed_contacts, sync_calendar, sync_contacts};
use mailcraft_model::sync::{JmapCalendarRequest, JmapCalendarResult, JmapContactsRequest, JmapContactsResult};
use mailcraft_ui_egui::jmap_mail::{JmapCalendarJob, JmapContactsJob, JmapSideJob};

/// Scrubs `error` of the account's secret, address, session URL and any word containing `@`.
pub(crate) fn redacted_line(email: &str, session: &str, secret: &str, error: &dyn std::fmt::Display) -> String {
    let mut redactor = Redactor::new();
    redactor.add(secret);
    if !secret.trim().is_empty() {
        redactor.add(secret.trim());
    }
    redactor.add_personal(email);
    redactor.add_personal(session);
    let cut = redactor.excerpt(&error.to_string());
    cut.as_str().split(' ').map(|word| if word.contains('@') || word.contains("%40") { "[redacted]" } else { word }).collect::<Vec<&str>>().join(" ")
}

/// Why [`credentials`] has none to send for the account, in words free of the secret, the address
/// and the server's.
pub(crate) fn unsendable(email: &str, session: &str, bearer: bool, secret: &str) -> String {
    let why = if bearer { Credentials::bearer(secret).err() } else { Credentials::basic(email.trim(), secret).err() };
    match why {
        Some(why) => redacted_line(email, session, secret, &why),
        None => "there is no password or token to sign in with".to_string(),
    }
}

/// Runs `job`, which reads `what` of a JMAP account, on a thread of its own: `run` with the job's
/// request, its secret and a stop of its own, and the one answer handed back, waking the window.
fn on_thread<Q, R>(job: JmapSideJob<Q>, wake: &egui::Context, what: &'static str, run: fn(&Q, &str, &Stop) -> R) -> Receiver<R>
where
    Q: Send + 'static,
    R: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = wake.clone();
    let spawned = std::thread::Builder::new().name(format!("jmap {what}")).spawn(move || {
        let JmapSideJob { request, secret, cancel } = job;
        let _ = tx.send(run(&request, &secret, &Stop::new(cancel)));
        wake.request_repaint();
    });
    if let Err(e) = spawned {
        log::error!("couldn't start reading the {what} of a JMAP account: {e}");
    }
    rx
}

/// An answer of the kind `$answer` to `$request` that says a line and nothing else, made from the
/// line: the answers of every kind name the account, its address and its session's the same way.
macro_rules! only_a_line {
    ($answer:ident, $request:expr) => {
        |line| $answer {
            account: $request.account,
            email: $request.email.clone(),
            session: $request.session.clone(),
            errors: vec![line],
            ..Default::default()
        }
    };
}

/// Whose data a job reads, as its request says, and the secret it goes with.
struct Whose<'a> {
    email: &'a str,
    session: &'a str,
    bearer: bool,
    secret: &'a str,
}

/// Signs in for a job that reads `what`, and has `read` do the reading over the connection. Without a
/// credential that could be sent nothing is sent, and the answer is `unsent` of a line that says so; a
/// sign-in that fails is `failed`'s to tell.
fn signed_in<R>(
    whose: &Whose<'_>,
    stop: &Stop,
    what: &str,
    unsent: impl FnOnce(String) -> R,
    failed: impl FnOnce(&Error) -> R,
    read: impl AsyncFnOnce(&Connection<Transport>) -> R,
) -> R {
    let Some(credentials) = credentials(whose.email, whose.bearer, whose.secret) else {
        let why = unsendable(whose.email, whose.session, whose.bearer, whose.secret);
        return unsent(format!("Couldn't read the {what} from the server: {why}"));
    };
    crate::block_on::block_on(async {
        match Connection::connect(Transport::new(stop.clone()), whose.session, credentials).await {
            Ok(connection) => read(&connection).await,
            Err(error) => failed(&error),
        }
    })
}

/// Reads the contacts of a JMAP account on a thread of its own.
pub(crate) fn run_contacts(job: JmapContactsJob, wake: &egui::Context) -> Receiver<JmapContactsResult> {
    on_thread(job, wake, "contacts", run_contacts_with)
}

/// [`run_contacts`] on this thread.
pub(crate) fn run_contacts_with(request: &JmapContactsRequest, secret: &str, stop: &Stop) -> JmapContactsResult {
    let whose = Whose { email: &request.email, session: &request.session, bearer: request.bearer, secret };
    let unsent = only_a_line!(JmapContactsResult, request);
    let failed = |error: &Error| failed_contacts(request, error);
    signed_in(&whose, stop, "contacts", unsent, failed, async |connection| sync_contacts(connection, request, stop.flag()).await)
}

/// Reads the calendars of a JMAP account and their events in the stretch asked for, on a thread
/// of its own. The request goes to `mailcraft-jmap` as it is, and nothing of it is logged here.
pub(crate) fn run_calendar(job: JmapCalendarJob, wake: &egui::Context) -> Receiver<JmapCalendarResult> {
    on_thread(job, wake, "calendars", run_calendar_with)
}

/// [`run_calendar`] on this thread.
pub(crate) fn run_calendar_with(request: &JmapCalendarRequest, secret: &str, stop: &Stop) -> JmapCalendarResult {
    let whose = Whose { email: &request.email, session: &request.session, bearer: request.bearer, secret };
    let unsent = only_a_line!(JmapCalendarResult, request);
    let failed = |error: &Error| failed_calendar(request, error);
    signed_in(&whose, stop, "calendars", unsent, failed, async |connection| sync_calendar(connection, request, stop.flag()).await)
}

#[cfg(test)]
#[path = "jmap_runner_tests.rs"]
mod tests;
