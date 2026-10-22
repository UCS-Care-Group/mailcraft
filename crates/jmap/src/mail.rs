// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A JMAP account's mailboxes and the newest messages in them (RFC 8620, RFC 8621), read for the engine.
//!
//! [`sync_mail`] takes the request the engine worked out ([`JmapMailRequest`]) and hands back what it
//! read in pieces ([`JmapMailResult`]), each to be taken in as it comes. Nothing here keeps anything
//! between runs: what the engine already has comes with the request.
//!
//! **What is asked of the server**, in order, each in one request:
//!
//! 1. `Mailbox/get`, always: the mailboxes (their roles say which is the Inbox, which a run that reads
//!    only the Inbox and the mailboxes on screen needs as much as one that reads them all). It is the
//!    folder list of the run when the request asks for one ([`JmapMailRequest::list_folders`]).
//! 2. For each mailbox read, the Inbox first: one request of two calls, `Email/query` (the mailbox, newest
//!    first, [`JmapMailRequest::window`] at most, with the total) and `Email/get` of the ids it found by a
//!    back-reference, for their keywords and the mailboxes they lie in. (A server that takes one call to a
//!    request is asked in two requests, the query and then the get.)
//! 3. For the emails the window lists that the engine has not got (`JmapKnown`), `Email/get` with what
//!    a message list shows, 200 emails to a request. The ones it has come with their keywords alone, from
//!    step 2, which is all the engine takes of them.
//!
//! **A window** is the mailbox's newest messages. One that holds all of them is a complete listing
//! ([`MessagePage::afresh`]) and the engine makes the mailbox the same as the server's, which removes
//! what the window lacks: so it says so only when it can be shown. The server gave a total, and it is the
//! number of emails listed; the list began at the newest; every email the list names is in the page, or
//! the server says it is gone (not all of them, nor all of a call for them: that is a server that could not
//! open the mailbox); a window that names none is complete only if the list of mailboxes says the mailbox
//! holds none, as a number; none was left out because another mailbox lists it, and none could not be read
//! whole.
//! Anything less is a window read in part, which adds and updates and removes nothing. An email in several
//! mailboxes is listed by the window of the first of them (of the mailboxes the server lists) and by no other,
//! so that two folders never take it from one another: the Inbox, then the special mailboxes in the order the
//! folder pane lists them, then the one whose id is the smallest. So an email that was moved, out of a
//! mailbox the run read completely and into one it did not read, is taken out of the first at once and
//! comes in again, as a new message, when the second is read and its window reaches it: only a server that
//! says what changed (`Email/changes`) tells a move from a removal, and this does not ask it. (Where a
//! server's ids survive a move, a run that reads both mailboxes keeps it as the same message; where they
//! name the place, it is a removal and an arrival.) The header fields are read from the raw `headers`,
//! whatever else the server serves.
//!
//! **Pieces**, in this order: the folder list (when asked for), then one piece for each window read, then
//! the last, which says how the run ended: `partial` is false only there, `whole` is true when nothing
//! went wrong and the run was not stopped, and the problems are in `errors`. [`JmapMailResult::stopped`]:
//! the `cancel` flag was set, which is looked at before every request.
//!
//! **When something goes wrong.** The server refusing the credentials or the access (401, 403) ends the run
//! with `auth_refused`; one that is busy (429, 503) ends it with `busy_for`, as many seconds as it asked, or
//! 30. An exchange the program called off when the owner stopped the run is the run stopped.
//! A server that cannot be reached, or whose request fails as a whole (an HTTP error, an answer that is no
//! JMAP), or says it is unavailable, is a line in `errors` and the run goes on with the next mailbox, until
//! three in a row have failed that way: then it ends. A mailbox the server answers an error for, or answers
//! without what was asked, is a line and the run goes on, however many there are. When
//! the emails of a mailbox can not be had because the server answers an error to the call, or its answer
//! is too large, the call is halved until the email that breaks it is found; that one is not read, the rest
//! are. (A call that got no answer at all is halved a few times only: it may be too much for the server, or
//! the server may be gone.) Emails that a call for them leaves out of its answer without saying they are
//! gone (or says all of them are), are a line too: a run that lost one is not whole. A list of mailboxes
//! with no mailbox in it is a line as well: an account always has an Inbox. The lines written here name no
//! mailbox, and what a server says in one is scrubbed of the session's address, the account's, the
//! credentials and any word that holds an `@`. A window that a stop (cancel, a busy server, a refused
//! sign-in) cuts between its requests is not handed on. What the request asks that is not done here yet
//! (`changes`, `sends`) gets no answer, so the engine asks again.
//!
//! **What comes back is hostile.** Counts, ids, names, previews, addresses and header fields are cut to
//! sizes; an answer is held to the size its request can need; an entry that cannot be read is passed over;
//! nothing is indexed, unwrapped or recursed into.

mod folders;
mod message;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::FolderKind;
use mailcraft_model::sync::{FolderListing, GraphMessage, GraphProgress, JmapMailRequest, JmapMailResult, JmapScope, MAX_JMAP_WINDOW, MessagePage};
use serde_json::{Map, Value, json};

use self::message::{Marks, claims};
use crate::connection::{busy_seconds, unusable};
use crate::{
    ApiRequest, ApiResponse, BuildError, CAPABILITY_MAIL, CallFailure, CallId, Connection, Error, Failure, HttpExchange, Id, Redactor,
    TransportError, Unusable,
};

/// Emails asked for whole in one request.
const MAX_CHUNK: usize = 200;
/// Most mailboxes a request may name besides the Inbox.
const MAX_ALSO: usize = 64;
/// Most mailboxes one run reads (the first of them, in the order of the run).
const MAX_MAILBOXES: usize = 1_000;
/// Mailboxes that fail one after the other, because the server can not be had, before the run gives up on
/// the rest.
const MAX_FAILURES: usize = 3;
/// Most times in a run that a call for emails is halved to find the one that breaks it (finding one in 200
/// takes 8): past that the server is taken to be failing, and what is left of the mailbox waits.
const MAX_SPLITS: usize = 32;
/// Likewise for a call the server did not answer at all (a time-out, a gateway that gave up): too much for it
/// perhaps, perhaps it is gone; so only a few times, or a run against a server that is gone would wait out
/// a time-out for each.
const MAX_DOWN_SPLITS: usize = 4;
/// How long a busy server is left alone when it does not say, in seconds.
const BUSY_FOR: u32 = 30;
/// An account id this long or longer is hidden in what a server says (a shorter one would garble it).
const SECRET_ID: usize = 8;
/// The most an answer may be, in bytes, by what is asked: what is parsed out of an answer takes many times
/// its size in memory, so a request is given no more than it can need. A list of mailboxes (10,000 at the
/// most), the ids and keywords of a window (500), and the header fields of 200 emails.
const LIST_BYTES: usize = 4 << 20;
const WINDOW_BYTES: usize = 2 << 20;
const DETAILS_BYTES: usize = 8 << 20;

/// What the mailboxes' list asks for, and what an email is read for in the window and for a message list.
const MAILBOX_PROPERTIES: [&str; 6] = ["id", "name", "parentId", "role", "totalEmails", "unreadEmails"];
const WINDOW_PROPERTIES: [&str; 3] = ["id", "keywords", "mailboxIds"];
const EMAIL_PROPERTIES: [&str; 9] = ["id", "blobId", "keywords", "mailboxIds", "size", "receivedAt", "hasAttachment", "preview", "headers"];
/// The special mailboxes after the Inbox, in the order the folder pane lists them.
const SPECIAL: [FolderKind; 5] = [FolderKind::Drafts, FolderKind::Sent, FolderKind::Deleted, FolderKind::Archive, FolderKind::Junk];

/// Read the folders and the newest messages of a JMAP account as `request` says, and hand them to `emit`
/// piece by piece (see the module documentation): the folder list when asked for, a piece for each window,
/// and a last piece (the only one that is not `partial`) that says how the run ended. `connection` is signed
/// in already ([`Connection::connect`]); a caller whose sign-in failed ends the run with [`failed_run`]. `cancel`
/// is read before every request: set, the run ends there, with a last piece that says `stopped`.
///
/// The future need not be `Send`, and `emit` is called from the task that polls it, so a caller on one
/// thread (the browser's) may hand pieces to the window with a plain channel. Nothing here waits: a busy
/// server ends the run with the seconds it asked for, and it is the caller's to ask again after them.
pub async fn sync_mail<H: HttpExchange>(
    connection: &Connection<H>,
    request: &JmapMailRequest,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(JmapMailResult),
) {
    let run = Run::new(connection, request, cancel);
    let mut out = Outcome::default();
    if let Err(stop) = run.all(&mut out, emit).await {
        match stop {
            Stop::Cancelled => out.stopped = true,
            Stop::Auth => out.auth_refused = true,
            Stop::Busy(seconds, why) => (out.busy_for, out.halted) = (Some(seconds), Some(why)),
            Stop::Down(error) | Stop::Failed(error) => out.halted = Some(run.said(&error)),
        }
    }
    log::debug!("jmap mail: the run is over (clean: {}, stopped: {}, busy: {})", out.clean(), out.stopped, out.busy_for.is_some());
    emit(run.piece(JmapMailResult {
        errors: out.lines(),
        busy_for: out.busy_for,
        whole: out.clean(),
        stopped: out.stopped,
        auth_refused: out.auth_refused,
        ..Default::default()
    }));
}

/// The end of a run that could not begin: signing in to the server ([`Connection::connect`]) failed with
/// `error`. What [`sync_mail`] would have said of the same trouble: the sign-in refused, the server busy, or a
/// line.
pub fn failed_run(request: &JmapMailRequest, error: &Error) -> JmapMailResult {
    let secrets = secrets_of(&request.email, &request.session, &Redactor::new(), "");
    let mut result = JmapMailResult { account: request.account, email: request.email.clone(), ..Default::default() };
    match error {
        Error::CredentialsRefused | Error::Forbidden { .. } => result.auth_refused = true,
        Error::Busy { .. } => {
            result.busy_for = Some(busy_seconds(error).unwrap_or(BUSY_FOR));
            result.errors.push(format!("Couldn't reach the server: {}", said(&secrets, error)));
        }
        other => result.errors.push(format!("Couldn't reach the server: {}", said(&secrets, other))),
    }
    result
}

/// Why a step cannot go on.
enum Stop {
    /// The owner stopped the run.
    Cancelled,
    /// The credentials, or the access, were refused: nothing more is asked.
    Auth,
    /// The server is busy: for this many seconds, and what it said.
    Busy(u32, String),
    /// The server can not be reached, or answers nonsense, or says it is unavailable: the next mailbox is
    /// tried, but not after three of these in a row.
    Down(Failure),
    /// The server answered, and this step failed (an error for a mailbox, a call that has no answer, an
    /// answer that is too large, a request too large for the server): the next mailbox is tried.
    Failed(Failure),
}

/// A request that could not be made is this step's trouble.
impl From<BuildError> for Stop {
    fn from(error: BuildError) -> Stop {
        Stop::Failed(error.into())
    }
}

/// What went wrong in a run, to be said in a few lines at its end.
#[derive(Default)]
struct Outcome {
    /// Why the mailboxes could not be listed.
    listing: Option<String>,
    /// A request failed in a way that ended the run (the server was busy, or could not be had).
    halted: Option<String>,
    /// How many mailboxes could not be read, and why the first could not.
    windows: (usize, Option<String>),
    /// How many emails could not be read whole, and why the first could not.
    details: (usize, Option<String>),
    /// Mailboxes that were left for the next run: three failed in a row, or there were too many.
    gave_up: bool,
    left_out: usize,
    /// How many times a call for emails was halved in this run.
    splits: usize,
    busy_for: Option<u32>,
    auth_refused: bool,
    stopped: bool,
}

impl Outcome {
    /// Nothing went wrong, and the run was not stopped: every mailbox it meant to read was read.
    fn clean(&self) -> bool {
        self.listing.is_none()
            && self.halted.is_none()
            && self.windows.0 == 0
            && self.details.0 == 0
            && !self.gave_up
            && self.left_out == 0
            && self.busy_for.is_none()
            && !self.auth_refused
            && !self.stopped
    }

    fn lines(&self) -> Vec<String> {
        let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
        let mut lines = Vec::new();
        if let Some(why) = &self.listing {
            lines.push(format!("Couldn't list the mailboxes on the server: {why}"));
        }
        if let Some(why) = &self.halted {
            lines.push(format!("Couldn't bring the mail down from the server: {why}"));
        }
        if let (n @ 1.., Some(why)) = (self.windows.0, &self.windows.1) {
            lines.push(format!("Couldn't read the mail in {}: {why}", plural(n, "mailbox", "mailboxes")));
        }
        if let (n @ 1.., Some(why)) = (self.details.0, &self.details.1) {
            lines.push(format!("Couldn't read {} in full: {why}", plural(n, "message", "messages")));
        }
        if self.gave_up {
            lines.push(format!("The server failed {MAX_FAILURES} mailboxes in a row; the rest wait for the next run."));
        }
        if self.left_out > 0 {
            lines.push(format!("{} not read: a run reads {MAX_MAILBOXES} at most.", plural(self.left_out, "mailbox was", "mailboxes were")));
        }
        lines
    }
}

/// What the server's list of mailboxes says, for the windows.
struct Places {
    /// The listing, as the engine takes it in.
    listing: FolderListing,
    /// The mailboxes that are places for mail (only they say where an email lies), each with its rank in
    /// the order that says which of them lists an email that lies in several ([`rank`]).
    claimable: HashMap<String, usize>,
    /// How many emails each says it holds, for those that say so with a number.
    counts: HashMap<String, u32>,
}

impl Places {
    fn of(listing: FolderListing, counts: HashMap<String, u32>) -> Places {
        let mut claimable = HashMap::new();
        for folder in &listing.folders {
            claimable.entry(folder.id.clone()).or_insert_with(|| rank(folder.kind));
        }
        Places { claimable, counts, listing }
    }
}

/// Where a mailbox of a kind comes among those an email lies in, the first listing it: the Inbox, then the
/// special mailboxes in the order the folder pane lists them ([`SPECIAL`]), then the rest (by id, there).
fn rank(kind: FolderKind) -> usize {
    if kind == FolderKind::Inbox {
        return 0;
    }
    SPECIAL.iter().position(|special| *special == kind).map_or(SPECIAL.len().saturating_add(1), |at| at.saturating_add(1))
}

/// An email in a window: one the engine has, or one to be read whole.
enum Slot {
    Known(Box<GraphMessage>),
    New(String),
}

/// The newest messages of one mailbox, as the first request made of them.
struct Newest {
    /// In the server's order, newest first.
    slots: Vec<Slot>,
    /// How many emails the mailbox has, when the server said.
    total: Option<u32>,
    /// The window holds every email the mailbox has, as far as the first request can show.
    complete: bool,
}

/// The emails a request for them brought.
struct Details {
    found: HashMap<String, GraphMessage>,
    /// How many of those asked for are not in `found`, though the server did not say they are gone (or said
    /// that all of them are, which is more likely a mailbox it could not open).
    short: usize,
}

/// One run: the request, the connection, and what is worked out once from them.
struct Run<'a, H> {
    connection: &'a Connection<H>,
    request: &'a JmapMailRequest,
    cancel: &'a AtomicBool,
    /// What an error line must not show.
    secrets: Redactor,
    /// The most emails a window holds, and the most asked for whole at a time.
    window: usize,
    chunk: usize,
    /// The server takes two calls (or more) in a request.
    two_calls: bool,
    /// The ids the engine has, by mailbox.
    known: HashMap<&'a str, HashSet<&'a str>>,
}

impl<'a, H: HttpExchange> Run<'a, H> {
    fn new(connection: &'a Connection<H>, request: &'a JmapMailRequest, cancel: &'a AtomicBool) -> Run<'a, H> {
        let most = usize::try_from(MAX_JMAP_WINDOW).unwrap_or(500);
        // (No window asked for is the biggest.) Both are held to what the server takes in a `get`: the
        // window is read back by one.
        let asked = match usize::try_from(request.window).unwrap_or(most) {
            0 => most,
            window => window.min(most),
        };
        let client = connection.client();
        let take = usize::try_from(client.limits().max_objects_in_get()).unwrap_or(usize::MAX);
        let mut known: HashMap<&str, HashSet<&str>> = HashMap::new();
        for mailbox in &request.known {
            known.entry(mailbox.mailbox.as_str()).or_default().extend(mailbox.ids.iter().map(String::as_str));
        }
        Run {
            connection,
            request,
            cancel,
            secrets: secrets_of(&request.email, &request.session, client.redactor(), client.account_id().as_str()),
            window: asked.min(take),
            chunk: MAX_CHUNK.min(take).max(1),
            two_calls: client.limits().max_calls_in_request() >= 2,
            known,
        }
    }

    /// The account's id, for the calls.
    fn account(&self) -> &str {
        self.connection.client().account_id().as_str()
    }

    /// A piece of the result, for this run's account.
    fn piece(&self, piece: JmapMailResult) -> JmapMailResult {
        JmapMailResult { account: self.request.account, email: self.request.email.clone(), ..piece }
    }

    /// What `error` says, fit for a line: no credential and no address in it.
    fn said(&self, error: &dyn fmt::Display) -> String {
        said(&self.secrets, error)
    }

    /// Why a request failed as a whole: the credentials or the access refused, the server busy, and the exchange
    /// called off because the owner stopped the run, end it; a request too large for the server, or an answer too
    /// large for it, is for the step; anything else says the server can not be had.
    fn call_stop(&self, error: Error) -> Stop {
        match error {
            Error::CredentialsRefused | Error::Forbidden { .. } => Stop::Auth,
            Error::Transport(TransportError::Aborted) if self.cancel.load(Ordering::Relaxed) => Stop::Cancelled,
            Error::Busy { .. } => Stop::Busy(busy_seconds(&error).unwrap_or(BUSY_FOR), self.said(&error)),
            Error::TooLarge { .. } | Error::Build(_) | Error::Unusable(Unusable::BodyTooLarge | Unusable::TooComplex) => Stop::Failed(error.into()),
            other => Stop::Down(other.into()),
        }
    }

    /// Why one call of a request failed: the server saying it is unavailable is a server that can not be
    /// had; any other error is for the step.
    fn method_stop(failure: CallFailure) -> Stop {
        match &failure {
            CallFailure::Method(error) if error.kind().may_succeed_later() => Stop::Down(failure.into()),
            _ => Stop::Failed(failure.into()),
        }
    }

    /// Send a request of mail's method calls, its answer held to the request's own limit.
    async fn post(&self, request: &ApiRequest) -> Result<ApiResponse, Stop> {
        self.connection.call(request, &self.secrets).await.map_err(|error| self.call_stop(error))
    }

    /// The answer to the call `call` of `method`, taken out of `answers`.
    fn take(answers: &mut ApiResponse, call: &CallId, method: &str) -> Result<Value, Stop> {
        answers.take_named(call, method).map(Value::Object).map_err(Self::method_stop)
    }

    /// A request of the one call `method` with `args`, and its answer, held to `most` bytes.
    async fn one_call(&self, method: &str, args: Value, most: usize) -> Result<Value, Stop> {
        let mut request = mail_request(most)?;
        let call = request.add_call(method, arguments(args))?;
        let mut answers = self.post(&request).await?;
        Self::take(&mut answers, &call, method)
    }

    /// Whether the owner asked to stop.
    fn check(&self) -> Result<(), Stop> {
        if self.cancel.load(Ordering::Relaxed) { Err(Stop::Cancelled) } else { Ok(()) }
    }

    /// The whole run: the mailboxes, then the windows.
    async fn all(&self, out: &mut Outcome, emit: &mut dyn FnMut(JmapMailResult)) -> Result<(), Stop> {
        let places = self.list(out).await?;
        if self.request.list_folders
            && let Some(places) = &places
        {
            emit(self.piece(JmapMailResult { listing: Some(places.listing.clone()), partial: true, ..Default::default() }));
        }
        let targets = self.targets(places.as_ref().map(|places| &places.listing), out);
        let of = u32::try_from(targets.len()).unwrap_or(u32::MAX);
        log::debug!("jmap mail: {} mailboxes to read, a window of {}", targets.len(), self.window);
        let mut down = 0usize;
        for (n, mailbox) in targets.iter().enumerate() {
            if down >= MAX_FAILURES {
                out.gave_up = true;
                break;
            }
            let number = u32::try_from(n.saturating_add(1)).unwrap_or(u32::MAX);
            match self.read(mailbox, number, of, places.as_ref(), out, emit).await {
                Ok(true) => down = 0,
                // (Its trouble is told already.)
                Ok(false) => down = down.saturating_add(1),
                // The server answered, if with an error: it is there.
                Err(Stop::Failed(error)) => {
                    self.window_failed(out, &error);
                    down = 0;
                }
                Err(Stop::Down(error)) => {
                    self.window_failed(out, &error);
                    down = down.saturating_add(1);
                }
                Err(stop) => return Err(stop),
            }
        }
        Ok(())
    }

    fn window_failed(&self, out: &mut Outcome, error: &Failure) {
        out.windows.0 = out.windows.0.saturating_add(1);
        out.windows.1.get_or_insert_with(|| self.said(error));
    }

    /// `Mailbox/get`, for every mailbox. A failure that is for this step alone is told, and the run goes on
    /// without a listing; one that says the server can not be had ends the run.
    async fn list(&self, out: &mut Outcome) -> Result<Option<Places>, Stop> {
        self.check()?;
        let args = json!({"accountId": self.account(), "properties": MAILBOX_PROPERTIES});
        let told = |out: &mut Outcome, error: &dyn fmt::Display| out.listing = Some(self.said(error));
        let answer = match self.one_call("Mailbox/get", args, LIST_BYTES).await {
            Ok(answer) => answer,
            // (An answer too large, an error for the call: this step alone.)
            Err(Stop::Failed(error)) => {
                told(out, &error);
                return Ok(None);
            }
            Err(stop) => return Err(stop),
        };
        let places = folders::listing_with_counts(&answer).map(|(listing, counts)| Places::of(listing, counts));
        match &places {
            None => told(out, &unusable("the answer has no list of mailboxes")),
            // (An account always has an Inbox: a server that lists none has not told the whole of it, and a run
            // that read nothing is not an "up to date".)
            Some(places) if places.listing.folders.is_empty() => told(out, &unusable("the answer lists no mailbox")),
            Some(_) => {}
        }
        Ok(places)
    }

    /// The mailboxes to read, in order: the Inbox, then the ones the request names (a light run) or the
    /// special ones and the rest (a run that reads all). Without a listing, those the request names, as
    /// they are.
    fn targets(&self, listing: Option<&FolderListing>, out: &mut Outcome) -> Vec<String> {
        let also: Vec<&str> = match &self.request.scope {
            JmapScope::Light { also } => also.iter().map(String::as_str).filter(|id| Id::new(id).is_ok()).take(MAX_ALSO).collect(),
            JmapScope::All => Vec::new(),
        };
        let light = matches!(self.request.scope, JmapScope::Light { .. });
        let mut order: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut take = |id: &str| {
            if seen.insert(id.to_string()) {
                order.push(id.to_string());
            }
        };
        match listing {
            Some(listing) => {
                listing.folders.iter().filter(|f| f.kind == FolderKind::Inbox).for_each(|f| take(&f.id));
                if light {
                    // (One the server does not list is one the engine has stale.)
                    also.iter().filter(|id| listing.folders.iter().any(|f| f.id == **id)).for_each(|id| take(id));
                } else {
                    for kind in SPECIAL {
                        listing.folders.iter().filter(|f| f.kind == kind).for_each(|f| take(&f.id));
                    }
                    listing.folders.iter().for_each(|f| take(&f.id));
                }
            }
            None if light => also.iter().for_each(|id| take(id)),
            None => {}
        }
        if order.len() > MAX_MAILBOXES {
            out.left_out = order.len().saturating_sub(MAX_MAILBOXES);
            order.truncate(MAX_MAILBOXES);
        }
        order
    }

    /// One mailbox's window: its newest messages, then the ones the engine lacks, whole; handed on as one
    /// piece. Whether the server could be had throughout (if not, the trouble is told already).
    async fn read(
        &self,
        mailbox: &str,
        number: u32,
        of: u32,
        places: Option<&Places>,
        out: &mut Outcome,
        emit: &mut dyn FnMut(JmapMailResult),
    ) -> Result<bool, Stop> {
        self.check()?;
        let top = self.newest(mailbox, places).await?;
        let wanted: Vec<&str> = top.slots.iter().filter_map(|slot| if let Slot::New(id) = slot { Some(id.as_str()) } else { None }).collect();
        let mut fetched: HashMap<String, GraphMessage> = HashMap::new();
        // Emails of the window that the page will lack, though the server has them (or does not say they are gone,
        // or says all of a call are).
        let mut short = 0usize;
        let mut reached = true;
        // The newest first: what is read of a mailbox that fails is its newest.
        let mut work: Vec<&[&str]> = wanted.chunks(self.chunk).rev().collect();
        while let Some(ids) = work.pop() {
            self.check()?;
            match self.whole(mailbox, ids, places.map(|places| &places.claimable)).await {
                Ok(got) => {
                    if got.short > 0 {
                        // Emails the page lacks, though the server has them or did not say they are gone (they
                        // lie in another mailbox now, the answer left them out, or it said all of the call are
                        // gone). The run is not whole
                        // then: one that was moved here from a mailbox that was read in full is taken for
                        // removed there, and would be swept before the next run lists it.
                        out.details.0 = out.details.0.saturating_add(got.short);
                        out.details.1.get_or_insert_with(|| "the server's answer did not have them".to_string());
                    }
                    short = short.saturating_add(got.short);
                    fetched.extend(got.found);
                }
                // Something in these emails breaks the call, or makes its answer too large: it is halved,
                // the newer half first, until the one email is found. That one is not read; the rest are.
                // (A call that got no answer at all is halved a few times only.)
                Err(stop) if halvable(&stop, ids.len(), out.splits) => {
                    log::debug!("jmap mail: a call for {} emails failed; it is halved", ids.len());
                    out.splits = out.splits.saturating_add(1);
                    let (newer, older) = ids.split_at(ids.len() / 2);
                    work.push(older);
                    work.push(newer);
                }
                Err(Stop::Failed(error)) if ids.len() == 1 => {
                    out.details.0 = out.details.0.saturating_add(1);
                    out.details.1.get_or_insert_with(|| self.said(&error));
                    short = short.saturating_add(1);
                }
                // The server keeps failing: what is left of this mailbox waits.
                Err(Stop::Failed(error)) => {
                    short = short.saturating_add(self.give_up(out, &error, ids, &work));
                    break;
                }
                // The server can not be had: what is left of this mailbox waits, and the run is told so.
                Err(Stop::Down(error)) => {
                    short = short.saturating_add(self.give_up(out, &error, ids, &work));
                    reached = false;
                    break;
                }
                Err(stop) => return Err(stop),
            }
        }
        let messages: Vec<GraphMessage> = top
            .slots
            .into_iter()
            .filter_map(|slot| match slot {
                Slot::Known(message) => Some(*message),
                Slot::New(id) => fetched.remove(&id),
            })
            .collect();
        let read = u32::try_from(messages.len()).unwrap_or(u32::MAX);
        let progress = GraphProgress { folder: mailbox.to_string(), read, total: top.total.unwrap_or(read).max(read), number, folders: of };
        let page = MessagePage { folder: mailbox.to_string(), messages, afresh: top.complete && short == 0, ..Default::default() };
        emit(self.piece(JmapMailResult { pages: vec![page], partial: true, progress: Some(progress), ..Default::default() }));
        Ok(reached)
    }

    /// Give up on `ids` and the `rest` of the calls for emails of a mailbox: how many emails that is.
    fn give_up(&self, out: &mut Outcome, error: &Failure, ids: &[&str], rest: &[&[&str]]) -> usize {
        let missed = ids.len().saturating_add(rest.iter().map(|ids| ids.len()).sum::<usize>());
        out.details.0 = out.details.0.saturating_add(missed);
        out.details.1.get_or_insert_with(|| self.said(error));
        missed
    }

    /// The first request of a window: the ids of the mailbox's newest emails and what they carry, the
    /// keywords and the mailboxes they lie in.
    async fn newest(&self, mailbox: &str, places: Option<&Places>) -> Result<Newest, Stop> {
        let account = self.account();
        let query = json!({
            "accountId": account,
            "filter": {"inMailbox": mailbox},
            "sort": [{"property": "receivedAt", "isAscending": false}],
            "limit": self.window,
            "calculateTotal": true,
        });
        let (found, marks) = if self.two_calls {
            // The get is given the ids the query finds (RFC 8620 §3.7).
            let mut request = mail_request(WINDOW_BYTES)?;
            let listed = request.add_call("Email/query", arguments(query))?;
            let ids = request.reference(&listed, "Email/query", "/ids")?;
            let get = json!({"accountId": account, "properties": WINDOW_PROPERTIES});
            let read = request.add_call_with_references("Email/get", arguments(get), vec![("ids", ids)])?;
            let mut answers = self.post(&request).await?;
            (Self::take(&mut answers, &listed, "Email/query")?, Self::take(&mut answers, &read, "Email/get")?)
        } else {
            // A server that takes one call to a request: the ids first, then the emails by those.
            let found = self.one_call("Email/query", query, WINDOW_BYTES).await?;
            self.check()?;
            // (The same entries of the list as the window is made of: the first `window`.)
            let ids: Vec<&str> = found
                .get("ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(self.window)
                .filter_map(Value::as_str)
                .filter(|id| Id::new(id).is_ok())
                .collect();
            let get = json!({"accountId": account, "ids": ids, "properties": WINDOW_PROPERTIES});
            let marks = self.one_call("Email/get", get, WINDOW_BYTES).await?;
            (found, marks)
        };
        self.newest_of(mailbox, &found, &marks, places).map_err(|error| Stop::Failed(error.into()))
    }

    /// What the answers to the window's two calls make of it.
    fn newest_of(&self, mailbox: &str, found: &Value, marks: &Value, places: Option<&Places>) -> Result<Newest, Error> {
        let listed = found.get("ids").and_then(Value::as_array).ok_or_else(|| unusable("the answer has no list of ids"))?;
        let emails = marks.get("list").and_then(Value::as_array).ok_or_else(|| unusable("the answer has no list of emails"))?;
        // What the server says has gone since it made the list.
        let gone: HashSet<&str> =
            marks.get("notFound").and_then(Value::as_array).into_iter().flatten().take(self.window).filter_map(Value::as_str).collect();
        // The emails by id (the first of an id), as many as were asked for.
        let mut by_id: HashMap<&str, &Value> = HashMap::new();
        for email in emails.iter().take(self.window) {
            if let Some(id) = email.get("id").and_then(Value::as_str) {
                by_id.entry(id).or_insert(email);
            }
        }
        let have = self.known.get(mailbox);
        let mut slots = Vec::new();
        let mut named: HashSet<&str> = HashSet::new();
        // Everything the list names is in the page, or the server says it has gone: if not, the list is not
        // all of the mailbox (an id that is none, an email the second answer does not have, one that
        // another mailbox lists).
        let mut accounted = true;
        for id in listed.iter().take(self.window) {
            let Some(id) = id.as_str().filter(|id| Id::new(id).is_ok()) else {
                accounted = false;
                continue;
            };
            // (An id twice is one email.)
            if !named.insert(id) {
                continue;
            }
            let Some(email) = by_id.get(id) else {
                accounted &= gone.contains(id);
                continue;
            };
            if !claims(mailbox, email.get("mailboxIds"), places.map(|places| &places.claimable)) {
                accounted = false;
                continue;
            }
            let marks = Marks::of(email.get("keywords"));
            slots.push(if have.is_some_and(|have| have.contains(id)) {
                Slot::Known(Box::new(message::known(id, mailbox, marks)))
            } else {
                Slot::New(id.to_string())
            });
        }
        // A server that says every email it just listed is gone has more likely failed to open the mailbox
        // than lost all of them between two calls of one request.
        let vanished = named.iter().filter(|id| !by_id.contains_key(**id) && gone.contains(**id)).count();
        accounted &= vanished == 0 || vanished < named.len();
        // A window that names no email is the whole of the mailbox only if the list of mailboxes says the
        // mailbox holds none (a number, and zero): a server that could not open it answers with no emails, as
        // does an index that is being made again, and a list that was not had, or that gives no count, says
        // nothing against it either.
        let confirmed_empty = places.and_then(|places| places.counts.get(mailbox)) == Some(&0);
        accounted &= !named.is_empty() || confirmed_empty;
        // Complete when the server says how many emails the mailbox has and that is the number of emails it
        // named, the list began at the newest and was not cut here, and all of it is accounted for. A server
        // that gives no total, or none that is a number, or one that is not the number of emails it listed,
        // or a position that is not 0, may have left out any of the mailbox.
        let total = found.get("total").and_then(Value::as_u64);
        let from_the_start = match found.get("position") {
            None | Some(Value::Null) => true,
            Some(position) => position.as_i64() == Some(0),
        };
        let all_named = total.is_some_and(|total| usize::try_from(total).is_ok_and(|total| total == named.len()));
        let complete = accounted && from_the_start && listed.len() <= self.window && all_named;
        Ok(Newest { slots, total: total.map(|total| u32::try_from(total).unwrap_or(u32::MAX)), complete })
    }

    /// `Email/get` of the emails `ids`, for what a message list shows of them.
    async fn whole(&self, mailbox: &str, ids: &[&str], claimable: Option<&HashMap<String, usize>>) -> Result<Details, Stop> {
        let args = json!({"accountId": self.account(), "ids": ids, "properties": EMAIL_PROPERTIES});
        let got = self.one_call("Email/get", args, DETAILS_BYTES).await?;
        let list = got.get("list").and_then(Value::as_array).ok_or_else(|| Stop::Failed(unusable("the answer has no list of emails").into()))?;
        let gone: HashSet<&str> =
            got.get("notFound").and_then(Value::as_array).into_iter().flatten().take(ids.len()).filter_map(Value::as_str).collect();
        let asked: HashSet<&str> = ids.iter().copied().collect();
        let mut found: HashMap<String, GraphMessage> = HashMap::new();
        for email in list.iter().take(ids.len()) {
            // Only what was asked for, once, and still in this mailbox's name.
            let Some(id) = email.get("id").and_then(Value::as_str).filter(|id| asked.contains(id) && !found.contains_key(*id)) else { continue };
            if claims(mailbox, email.get("mailboxIds"), claimable) {
                found.insert(id.to_string(), message::whole(id, mailbox, email, Marks::of(email.get("keywords"))));
            }
        }
        // The server says that every email of the call is gone: more likely it could not open the mailbox (it says
        // so of each id then) than that they all went since the window was read. They are not taken for gone.
        let all_gone = !ids.is_empty() && found.is_empty() && ids.iter().all(|id| gone.contains(*id));
        let short = if all_gone { ids.len() } else { ids.iter().filter(|id| !found.contains_key(**id) && !gone.contains(**id)).count() };
        Ok(Details { found, short })
    }
}

/// Whether a call for `count` emails that failed with `stop` is worth halving: the server answered with an
/// error, or the answer was too large, and there is more than one email to tell apart (a call that got no
/// answer at all is halved a few times only).
fn halvable(stop: &Stop, count: usize, splits: usize) -> bool {
    count > 1
        && match stop {
            Stop::Failed(_) => splits < MAX_SPLITS,
            Stop::Down(_) => splits < MAX_DOWN_SPLITS,
            Stop::Cancelled | Stop::Auth | Stop::Busy(..) => false,
        }
}

/// A request of mail's method calls, whose answer may be `most` bytes at the most: the capability of mail in
/// `using`, beside the core's (RFC 8621 §1.3).
fn mail_request(most: usize) -> Result<ApiRequest, BuildError> {
    let mut request = ApiRequest::new();
    request.using(CAPABILITY_MAIL)?;
    request.set_response_limit(most);
    Ok(request)
}

/// The arguments of a call, from the JSON object they are written as.
fn arguments(args: Value) -> Map<String, Value> {
    match args {
        Value::Object(args) => args,
        _ => Map::new(),
    }
}

/// What must not show in a line: what the client hides (the credentials, the session's address and what its
/// query holds), the account's address `email` and the session's address as the request has it, both in every
/// form a redactor knows (as written, percent-encoded), and the account's id when it is long enough to be one
/// of the first (a server may make it of the address). Also for the lines of a fetch ([`crate::fetch`]).
pub(crate) fn secrets_of(email: &str, session: &str, client: &Redactor, account_id: &str) -> Redactor {
    let mut secrets = client.clone();
    for personal in [email, session] {
        secrets.add_personal(personal);
    }
    if account_id.len() >= SECRET_ID {
        secrets.add_personal(account_id);
    }
    secrets
}

/// `error` as a line: one line, short, the secrets taken out before it is cut ([`Redactor::excerpt`]), and
/// any word that holds an `@` (an address, whose it may be), or the `%40` it is written as in an address,
/// taken out after.
pub(crate) fn said(secrets: &Redactor, error: &dyn fmt::Display) -> String {
    let cut = secrets.excerpt(&error.to_string());
    cut.as_str().split(' ').map(|word| if word.contains('@') || word.contains("%40") { "[redacted]" } else { word }).collect::<Vec<&str>>().join(" ")
}
