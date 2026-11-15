// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail fetched on the phone (MB3.17): a Graph delta brings only headers and previews, so what the
//! owner opens, what the background keeps for reading offline, an attachment and a search on the
//! server come through here.
//!
//! - **Opened.** A message whose body isn't on the phone is fetched whole through the engine's
//!   path (`graph_fetch_request`, the Graph fetch, `apply_fetch`), ahead of the background; the
//!   app hears `MessageFetched` or `FetchFailed` with the reason.
//! - **Background.** Between Send/Receive rounds, on a connection that isn't metered and not while
//!   the owner saves data, a few at a time, the newest messages within the account's window up to
//!   its Download messages in full (1 MB on the phone): each message is
//!   asked for within that size, and the batch within [`BACKGROUND_TOTAL`]. It rests while the
//!   phone's storage is nearly full, so that it never fetches what keeping within the limits
//!   would take back.
//! - **A search on the server** brings the headers of what it finds into their folders, kept on
//!   the phone for the session whatever the window says.
//!
//! One job at a time per account, on a thread of the host's; its answer is taken in on the
//! session's thread at the driver's next wake, so the mailbox has one writer. The owner's opens
//! and searches wait in a small queue ([`QUEUE`]) ahead of the background. A job's Graph token is
//! Send/Receive's ([`mailcraft_host::sync::SendReceive::mail_access`]); a token it renewed goes
//! back to Send/Receive, which keeps the newest refresh token as a round does.
//!
//! Nothing half: a job that is called off (shutdown, the account removed, the sign-ins forgotten,
//! another mailbox) is let go of and its answer, when it comes, is never taken in; what is taken
//! in is held to sizes first (each message, the batch, the lists), whoever fetched it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::{device_limits, graph_mail, search_server};
use mailcraft_host::NetworkState;
use mailcraft_host::fetch::{Access, Renewal};
use mailcraft_host::sync::Adopted;
use mailcraft_model::sync::{GraphFetchRequest, GraphFetchResult, GraphSearchRequest, GraphSearchResult};
use mailcraft_model::{AccountKind, Id};

use crate::CoreError;
use crate::events::Event;
use crate::queue::Queue;
use crate::sync::{Inner, SyncHost};

/// Most of the owner's opens and searches waiting for their account's job.
pub(crate) const QUEUE: usize = 8;
/// Most calls waiting for a message to come (an attachment of one not on the phone yet).
pub(crate) const MOST_WAITERS: usize = 32;
/// The largest message the owner opens on the phone.
pub(crate) const OPEN_MOST_BYTES: u64 = 64 << 20;
/// Most messages a background batch asks for, and most bytes it brings together.
pub(crate) const BACKGROUND_BATCH: usize = 5;
pub(crate) const BACKGROUND_TOTAL: u64 = 8 << 20;
/// The background rests once the mail on the phone takes this share (in percent) of its storage.
pub(crate) const STORAGE_FULL_PERCENT: u64 = 90;
/// Most messages a search on the server brings onto the phone in a session.
pub(crate) const MOST_SEARCH_KEPT: usize = 2_000;
/// Most results taken from one page of a search, whatever the server sends.
const MOST_ON_A_PAGE: usize = 200;
/// The longest search sent.
pub(crate) const MAX_QUERY: usize = 512;
/// Most lines kept of what went wrong, and the longest.
const MOST_ERRORS: usize = 5;
const LONGEST_ERROR: usize = 200;
/// Most server ids a result's lists are read for.
const MOST_IDS: usize = 100;

/// Why a message couldn't be fetched, for the app to say in its own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FetchFailure {
    /// No network, or Work Offline is on: it comes once the phone is online.
    Offline,
    /// The account has to be signed in again (`SignInNeeded` says so too).
    SignInNeeded,
    /// The server no longer has it (deleted or moved meanwhile).
    Gone,
    /// Larger than the phone takes whole.
    TooLarge,
    /// The server wouldn't give it.
    Refused,
    /// The server asked to be left alone for a while.
    Busy,
    /// Called off: the core is shutting down, the account was removed, or the owner opened
    /// enough others meanwhile.
    Cancelled,
    /// Something else went wrong.
    Failed,
}

/// What `open_message` found.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OpenState {
    /// The message is on the phone whole: read it with `message`.
    Here,
    /// It is being fetched: `MessageFetched` or `FetchFailed` follows.
    Fetching,
    /// Only its headers and preview are here, and there is no network or Work Offline is on.
    Offline,
    /// Only its headers and preview are here, and it can't be fetched (`reason` is for the owner).
    Unavailable { reason: String },
}

/// What a page of a search on the server came to.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct ServerSearchAnswer {
    /// The messages found, newest first, as they are now on the phone (those that were here
    /// already among them); read them with the typed queries.
    pub messages: Vec<u64>,
    /// The server has more: `search_server_more` asks for the next page.
    pub more: bool,
    /// Some results weren't brought onto the phone: it holds as many from searches as it takes
    /// in a session, or their folder isn't on the phone.
    pub left_out: u64,
    /// The server couldn't be reached.
    pub offline: bool,
    /// What went wrong, for the log and support (never a secret or a query).
    pub errors: Vec<String>,
}

/// What a job asks of the server.
pub(crate) enum Work {
    Messages(GraphFetchRequest),
    Search(GraphSearchRequest),
}

/// What it brought.
pub(crate) enum Answer {
    Messages(GraphFetchResult),
    Search(GraphSearchResult),
}

/// A job's answer: `None` when no token could be had (the renewal says why).
pub(crate) struct FetchDone {
    pub(crate) answer: Option<Answer>,
    pub(crate) renewal: Renewal,
}

/// How a job is run: over the network, or as a test says.
pub(crate) type FetchRunner = Arc<dyn Fn(Access, Work, Arc<AtomicBool>) -> FetchDone + Send + Sync>;

/// The runner a core starts with: over the network, and never in a test's build, where a test that
/// doesn't swap its own in reaches nobody.
pub(crate) fn default_runner() -> FetchRunner {
    #[cfg(test)]
    return Arc::new(|_, _, _| FetchDone {
        answer: None,
        renewal: Renewal { offline: true, error: Some("a test's build reaches no server".to_string()), ..Default::default() },
    });
    #[cfg(not(test))]
    real_runner()
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn real_runner() -> FetchRunner {
    Arc::new(|access, work, cancel| {
        let (answer, renewal) = mailcraft_host::fetch::with_access(access, |token| match work {
            Work::Messages(request) => Answer::Messages(mailcraft_net::graph::fetch_messages(token, &request, &cancel)),
            Work::Search(request) => Answer::Search(mailcraft_net::graph::search_messages(token, &request)),
        });
        FetchDone { answer, renewal }
    })
}

type SearchReply = Arc<Queue<Result<ServerSearchAnswer, CoreError>>>;
pub(crate) type MessageWaiter = Arc<Queue<Result<(), FetchFailure>>>;

/// What waits for its account's job.
enum Want {
    Open { account: Id, message: Id },
    Search { account: Id, request: GraphSearchRequest, reply: SearchReply },
}

impl Want {
    fn account(&self) -> Id {
        match self {
            Want::Open { account, .. } | Want::Search { account, .. } => *account,
        }
    }
}

/// What a job under way is for.
enum What {
    Open(Id),
    Background,
    Search(SearchReply),
}

struct Job {
    what: What,
    /// The opening of the mailbox it was asked in ([`Session::openings`]).
    opening: u64,
    cancel: Arc<AtomicBool>,
    answer: Receiver<FetchDone>,
    /// What it asked for each message and the batch (held to it again when it comes).
    most_bytes: u64,
    most_total: u64,
}

/// The last search on the server of an account, for its next page.
struct Searched {
    query: String,
    next: String,
    ticket: u64,
}

/// Fetching's state, in the Send/Receive state's lock.
#[derive(Default)]
pub(crate) struct Fetching {
    running: BTreeMap<Id, Job>,
    wanted: VecDeque<Want>,
    waiters: Vec<(Id, MessageWaiter)>,
    searches: BTreeMap<Id, Searched>,
    /// Messages searches brought onto the phone this session.
    search_kept: usize,
    ticket: u64,
    /// The last message that couldn't come, and why (what `open` says when it ends at once).
    last_failure: Option<(Id, FetchFailure)>,
}

impl Fetching {
    /// Whether a job is under way (the driver then wakes often).
    pub(crate) fn busy(&self) -> bool {
        !self.running.is_empty() || !self.wanted.is_empty()
    }

    fn wanted_or_running(&self, message: Id) -> bool {
        self.running.values().any(|j| matches!(j.what, What::Open(m) if m == message))
            || self.wanted.iter().any(|w| matches!(w, Want::Open { message: m, .. } if *m == message))
    }
}

/// Words for the catalogue, said in the interface language where they are used.
fn words(text: &'static str) -> &'static str {
    text
}

/// What a message that couldn't come is said as, to the owner.
pub(crate) fn failure_words(lang: mailcraft_i18n::Lang, why: FetchFailure) -> String {
    let text = match why {
        FetchFailure::Offline => words("There is no network connection."),
        FetchFailure::SignInNeeded => words("The account has to be signed in again."),
        FetchFailure::Gone => words("The message is no longer on the server."),
        FetchFailure::TooLarge => words("The message is too large to download on the phone."),
        FetchFailure::Busy => words("The server is busy. Try again in a few minutes."),
        FetchFailure::Cancelled => words("The download was called off."),
        FetchFailure::Refused | FetchFailure::Failed => words("The message couldn't be downloaded. Try again in a few minutes."),
    };
    lang.t(text).to_string()
}

fn clipped(lines: &[String]) -> Vec<String> {
    lines.iter().take(MOST_ERRORS).map(|l| l.chars().take(LONGEST_ERROR).collect()).collect()
}

/// `result` held to what was asked: each message within `most_bytes`, the batch within
/// `most_total` (what is past it waits for the next batch), the lists capped. A runner, a server
/// or a fault can send anything.
pub(crate) fn bounded(mut result: GraphFetchResult, most_bytes: u64, most_total: u64) -> GraphFetchResult {
    let most = usize::try_from(most_bytes).unwrap_or(usize::MAX);
    let total = usize::try_from(most_total).unwrap_or(usize::MAX);
    let mut brought = 0usize;
    let mut kept = Vec::new();
    for f in std::mem::take(&mut result.fetched).into_iter().take(MOST_IDS) {
        if f.mime.len() > most {
            result.too_large.push(f.id.clone());
            result.failed.push(f.id);
        } else if brought.saturating_add(f.mime.len()) <= total {
            brought = brought.saturating_add(f.mime.len());
            kept.push(f);
        }
        // (Past the batch's room: not taken in, and asked for again.)
    }
    result.fetched = kept;
    for list in [&mut result.missing, &mut result.failed, &mut result.too_large] {
        list.truncate(MOST_IDS);
    }
    result.errors = clipped(&result.errors);
    result
}

impl SyncHost {
    /// Whether the phone has no network or Work Offline is on.
    fn offline(&self, session: &Session) -> bool {
        session.prefs.offline || matches!(self.host.platform().network(), NetworkState::Offline)
    }

    /// The owner opened `message`: it is shown (keeping within the limits leaves it alone), and
    /// fetched whole when only its headers are here.
    pub(crate) fn open(&self, session: &mut Session, message: Id) -> Result<OpenState, CoreError> {
        let lang = session.interface_language;
        let m = session.mailbox.message(message).ok_or_else(|| CoreError::BadRequest { reason: "there is no such message".to_string() })?;
        let (account, whole, on_server) = (m.account, !m.unfetched, m.server.is_some());
        session.sel.messages = vec![message];
        if whole || !on_server {
            return Ok(OpenState::Here);
        }
        if !self.runs(session, account) {
            return Ok(OpenState::Unavailable {
                reason: lang.t(words("Messages of this account can't be downloaded on the phone yet.")).to_string(),
            });
        }
        if self.offline(session) {
            return Ok(OpenState::Offline);
        }
        let mut inner = self.lock();
        if !inner.fetch.wanted_or_running(message) {
            self.want(&mut inner, Want::Open { account, message });
        }
        inner.fetch.last_failure = None;
        self.start_wanted(&mut inner, session);
        let under_way = inner.fetch.wanted_or_running(message);
        let failure = inner.fetch.last_failure.filter(|(m, _)| *m == message).map(|(_, why)| why);
        drop(inner);
        self.pacer.poke();
        if under_way {
            return Ok(OpenState::Fetching);
        }
        // It ended at once (no sign-in, no longer the server's, whole by now).
        Ok(match (session.mailbox.message(message).is_some_and(|m| !m.unfetched), failure) {
            (true, _) => OpenState::Here,
            (false, Some(why)) => OpenState::Unavailable { reason: failure_words(lang, why) },
            (false, None) => OpenState::Unavailable { reason: failure_words(lang, FetchFailure::Failed) },
        })
    }

    /// Whether the phone fetches `account`'s mail: a Microsoft account of its own.
    fn runs(&self, session: &Session, account: Id) -> bool {
        session.mailbox.account(account).is_some_and(|a| a.kind == AccountKind::Microsoft && !a.is_shared())
    }

    /// A call waits for `message` to come whole (asked for when it isn't yet). `None` when it is
    /// here already.
    pub(crate) fn wait_for(&self, session: &mut Session, message: Id) -> Result<Option<MessageWaiter>, CoreError> {
        let lang = session.interface_language;
        match self.open(session, message)? {
            OpenState::Here => return Ok(None),
            OpenState::Offline => return Err(CoreError::Failed { reason: lang.t(words("There is no network connection.")).to_string() }),
            OpenState::Unavailable { reason } => return Err(CoreError::Failed { reason }),
            OpenState::Fetching => {}
        }
        let mut inner = self.lock();
        if inner.fetch.waiters.len() >= MOST_WAITERS {
            return Err(CoreError::Failed { reason: "too many calls are waiting; try again in a moment".to_string() });
        }
        let waiter = Queue::new();
        inner.fetch.waiters.push((message, waiter.clone()));
        Ok(Some(waiter))
    }

    /// A search on `account`'s server: `query` afresh, or (`None`) the next page of the last one.
    pub(crate) fn search(&self, session: &mut Session, account: Id, query: Option<String>) -> Result<SearchReply, CoreError> {
        let lang = session.interface_language;
        let a = session
            .mailbox
            .account(account)
            .filter(|a| a.kind == AccountKind::Microsoft && !a.is_shared())
            .ok_or_else(|| CoreError::BadRequest { reason: "there is no such account".to_string() })?;
        let email = a.email.clone();
        if self.offline(session) {
            return Err(CoreError::Failed { reason: lang.t(words("There is no network connection.")).to_string() });
        }
        let mut inner = self.lock();
        let (query, next) = match query {
            Some(query) => {
                let query = query.trim().to_string();
                if query.is_empty() || query.len() > MAX_QUERY {
                    return Err(CoreError::BadRequest { reason: "the search is empty or too long".to_string() });
                }
                if mailcraft_model::search::Query::parse(&query).for_server().terms.is_empty() {
                    return Err(CoreError::Failed { reason: lang.t(words("Nothing of this search can be asked of the server.")).to_string() });
                }
                (query, String::new())
            }
            None => match inner.fetch.searches.get(&account).filter(|s| !s.next.is_empty()) {
                Some(s) => (s.query.clone(), s.next.clone()),
                None => return Err(CoreError::BadRequest { reason: "there is no more to ask for".to_string() }),
            },
        };
        inner.fetch.ticket = inner.fetch.ticket.wrapping_add(1);
        let ticket = inner.fetch.ticket;
        inner.fetch.searches.insert(account, Searched { query: query.clone(), next: next.clone(), ticket });
        let reply: SearchReply = Queue::new();
        let request = GraphSearchRequest { account, email, shared: false, ticket, folder: String::new(), query, next };
        self.want(&mut inner, Want::Search { account, request, reply: reply.clone() });
        self.start_wanted(&mut inner, session);
        drop(inner);
        self.pacer.poke();
        Ok(reply)
    }

    /// Put `want` in the queue; when it is full, the oldest is called off (the owner has moved on).
    fn want(&self, inner: &mut Inner, want: Want) {
        if inner.fetch.wanted.len() >= QUEUE
            && let Some(old) = inner.fetch.wanted.pop_front()
        {
            self.called_off(inner, old);
        }
        inner.fetch.wanted.push_back(want);
    }

    fn called_off(&self, inner: &mut Inner, want: Want) {
        match want {
            Want::Open { message, .. } => self.failed(inner, message, FetchFailure::Cancelled),
            Want::Search { reply, .. } => reply.push(Err(CoreError::Cancelled)),
        }
    }

    /// `message` came whole: the app hears so, and so does whoever waits for it.
    fn fetched(&self, inner: &mut Inner, message: Id) {
        self.sink.push(Event::MessageFetched { message });
        inner.fetch.waiters.retain(|(m, w)| {
            if *m == message {
                w.push(Ok(()));
            }
            *m != message
        });
    }

    fn failed(&self, inner: &mut Inner, message: Id, reason: FetchFailure) {
        inner.fetch.last_failure = Some((message, reason));
        self.sink.push(Event::FetchFailed { message, reason });
        inner.fetch.waiters.retain(|(m, w)| {
            if *m == message {
                w.push(Err(reason));
            }
            *m != message
        });
    }

    /// Call off what is under way and waiting: `account`'s, or everything. Nothing it brings is
    /// taken in.
    pub(crate) fn cancel_fetches(&self, account: Option<Id>) {
        let mut inner = self.lock();
        self.cancel_locked(&mut inner, account);
    }

    pub(crate) fn cancel_locked(&self, inner: &mut Inner, account: Option<Id>) {
        let of = |a: Id| account.is_none_or(|x| x == a);
        let gone: Vec<Id> = inner.fetch.running.keys().copied().filter(|a| of(*a)).collect();
        for a in gone {
            if let Some(job) = inner.fetch.running.remove(&a) {
                job.cancel.store(true, Ordering::Relaxed);
                match job.what {
                    What::Open(message) => self.failed(inner, message, FetchFailure::Cancelled),
                    What::Search(reply) => reply.push(Err(CoreError::Cancelled)),
                    What::Background => {}
                }
            }
        }
        let wanted = std::mem::take(&mut inner.fetch.wanted);
        for want in wanted {
            if of(want.account()) {
                self.called_off(inner, want);
            } else {
                inner.fetch.wanted.push_back(want);
            }
        }
        if account.is_none() {
            for (_, waiter) in std::mem::take(&mut inner.fetch.waiters) {
                waiter.push(Err(FetchFailure::Cancelled));
            }
        }
        inner.fetch.searches.retain(|a, _| !of(*a));
        self.fetch_busy.store(inner.fetch.busy(), Ordering::Relaxed);
    }

    /// One wake of fetching, on the session's thread: answers taken in, the owner's waiting jobs
    /// started, and (`between_rounds`) a background batch.
    pub(crate) fn fetch_step(&self, inner: &mut Inner, session: &mut Session, between_rounds: bool) {
        self.take_answers(inner, session);
        if self.offline(session) {
            // What the owner waits for can't come now: said, not held.
            for want in std::mem::take(&mut inner.fetch.wanted) {
                match want {
                    Want::Open { message, .. } => self.failed(inner, message, FetchFailure::Offline),
                    Want::Search { reply, .. } => reply
                        .push(Err(CoreError::Failed { reason: session.interface_language.t(words("There is no network connection.")).to_string() })),
                }
            }
        } else {
            self.start_wanted(inner, session);
            if between_rounds {
                self.start_background(inner, session);
            }
        }
        self.fetch_busy.store(inner.fetch.busy(), Ordering::Relaxed);
    }

    /// Start the owner's waiting jobs whose accounts have none under way.
    fn start_wanted(&self, inner: &mut Inner, session: &mut Session) {
        let wanted = std::mem::take(&mut inner.fetch.wanted);
        for want in wanted {
            if inner.fetch.running.contains_key(&want.account()) {
                inner.fetch.wanted.push_back(want);
                continue;
            }
            match want {
                Want::Open { account, message } => self.start_open(inner, session, account, message),
                Want::Search { account, request, reply } => {
                    let Some(access) = self.access(inner, session, account) else {
                        self.no_sign_in(inner, account);
                        reply.push(Err(CoreError::Failed {
                            reason: session.interface_language.t(words("The account has to be signed in again.")).to_string(),
                        }));
                        continue;
                    };
                    self.start(inner, session, account, What::Search(reply), access, Work::Search(request), 0, 0);
                }
            }
        }
        self.fetch_busy.store(inner.fetch.busy(), Ordering::Relaxed);
    }

    fn start_open(&self, inner: &mut Inner, session: &mut Session, account: Id, message: Id) {
        let mut request = match graph_mail::graph_fetch_request(session, account, Some(message), 1) {
            Ok(request) => request,
            Err(_) => return self.failed(inner, message, FetchFailure::Gone),
        };
        if request.ids.is_empty() {
            // Whole by now (a batch brought it), or no longer the server's to give.
            return match session.mailbox.message(message) {
                Some(m) if !m.unfetched => self.fetched(inner, message),
                _ => self.failed(inner, message, FetchFailure::Gone),
            };
        }
        let Some(access) = self.access(inner, session, account) else {
            self.no_sign_in(inner, account);
            return self.failed(inner, message, FetchFailure::SignInNeeded);
        };
        (request.most_bytes, request.most_total) = (OPEN_MOST_BYTES, OPEN_MOST_BYTES);
        self.start(inner, session, account, What::Open(message), access, Work::Messages(request), OPEN_MOST_BYTES, OPEN_MOST_BYTES);
    }

    /// A background batch of one account, when it is due and the phone has room for it.
    fn start_background(&self, inner: &mut Inner, session: &mut Session) {
        if inner.fetch.running.values().any(|j| matches!(j.what, What::Background)) {
            return;
        }
        // Only on a connection that isn't paid by the byte, and not while the owner saves data
        // (PRIVACY.md): what is opened comes on any connection.
        let unmetered = matches!(self.host.platform().network(), NetworkState::Unknown | NetworkState::Online { metered: false, data_saving: false });
        let now = self.now_ms();
        if !unmetered || !session.fetch_background_due(now) || !room_for_more(session) {
            return;
        }
        let accounts = session.fetch_background_accounts(|kind| kind == AccountKind::Microsoft, now);
        for account in accounts {
            if inner.fetch.running.contains_key(&account) || !self.runs(session, account) {
                continue;
            }
            let Ok(mut request) = graph_mail::graph_fetch_request(session, account, None, BACKGROUND_BATCH) else { continue };
            if request.ids.is_empty() {
                continue;
            }
            let Some(access) = self.access(inner, session, account) else { continue };
            // Each message within the account's Download messages in full (the server's size isn't
            // known before it comes: a delta gives none), the batch within its own room.
            let most = session.mailbox.account(account).and_then(|a| a.download_in_full.limit()).unwrap_or(OPEN_MOST_BYTES).min(OPEN_MOST_BYTES);
            let total = BACKGROUND_TOTAL.max(most);
            (request.most_bytes, request.most_total) = (most, total);
            self.start(inner, session, account, What::Background, access, Work::Messages(request), most, total);
            return;
        }
        session.driver.fetch.found_nothing(now);
    }

    /// How `account`'s job gets its token.
    fn access(&self, inner: &mut Inner, session: &Session, account: Id) -> Option<Access> {
        let secrets = self.host.platform().secrets();
        let ctx = self.context(session, secrets.as_deref());
        inner.sr.mail_access(session, &ctx, account)
    }

    /// Nothing to sign in with: the app is told the account has to be signed in again.
    fn no_sign_in(&self, inner: &mut Inner, account: Id) {
        if inner.needs_sign_in.insert(account) {
            self.sink.push(Event::SignInNeeded { account });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start(&self, inner: &mut Inner, session: &Session, account: Id, what: What, access: Access, work: Work, most_bytes: u64, most_total: u64) {
        let cancel = Arc::new(AtomicBool::new(false));
        let runner = self.fetch_runner.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let flag = cancel.clone();
        let answer = self.host.run_job("mailcraft-fetch", move |_| runner(access, work, flag), || {});
        inner.fetch.running.insert(account, Job { what, opening: session.openings(), cancel, answer, most_bytes, most_total });
    }

    /// Take in the answers that came.
    fn take_answers(&self, inner: &mut Inner, session: &mut Session) {
        let accounts: Vec<Id> = inner.fetch.running.keys().copied().collect();
        for account in accounts {
            let done = match inner.fetch.running.get(&account).map(|job| job.answer.try_recv()) {
                Some(Ok(done)) => Some(done),
                Some(Err(TryRecvError::Empty)) | None => continue,
                // The job's thread ended without an answer (or never started).
                Some(Err(TryRecvError::Disconnected)) => None,
            };
            let Some(job) = inner.fetch.running.remove(&account) else { continue };
            let stale = job.opening != session.openings() || job.cancel.load(Ordering::Relaxed);
            if stale {
                log::info!("fetch: an answer for a job that was called off, or for another mailbox, is let go of");
                match job.what {
                    What::Open(message) => self.failed(inner, message, FetchFailure::Cancelled),
                    What::Search(reply) => reply.push(Err(CoreError::Cancelled)),
                    What::Background => {}
                }
                continue;
            }
            self.took_job(inner, session, account, job, done);
        }
    }

    fn took_job(&self, inner: &mut Inner, session: &mut Session, account: Id, job: Job, done: Option<FetchDone>) {
        let Some(FetchDone { answer, renewal }) = done else {
            log::warn!("fetch: the job ended without an answer");
            return self.job_failed(inner, session, account, job.what, FetchFailure::Failed, &[]);
        };
        // A renewed token goes to Send/Receive, which keeps the newest refresh token as a round does.
        if let Some(tokens) = renewal.tokens {
            let secrets = self.host.platform().secrets();
            let ctx = self.context(session, secrets.as_deref());
            let adopted = inner.sr.adopt_tokens(session, &ctx, account, tokens);
            self.note_keys(inner);
            if adopted == Adopted::WrongPerson {
                // What a token of somebody else's mailbox brought is nobody's here.
                self.no_sign_in(inner, account);
                return self.job_failed(inner, session, account, job.what, FetchFailure::SignInNeeded, &[]);
            }
        }
        if renewal.sign_in_refused || renewal.consent_needed {
            self.no_sign_in(inner, account);
            return self.job_failed(inner, session, account, job.what, FetchFailure::SignInNeeded, &[]);
        }
        let Some(answer) = answer else {
            let why = if renewal.offline { FetchFailure::Offline } else { FetchFailure::Failed };
            let errors: Vec<String> = renewal.error.into_iter().collect();
            return self.job_failed(inner, session, account, job.what, why, &errors);
        };
        match (job.what, answer) {
            (What::Search(reply), Answer::Search(result)) => {
                let taken = self.took_search(inner, session, account, result);
                reply.push(taken);
            }
            (what @ (What::Open(_) | What::Background), Answer::Messages(result)) => {
                let result = bounded(result, job.most_bytes, job.most_total);
                if result.account != account {
                    return self.job_failed(inner, session, account, what, FetchFailure::Failed, &[]);
                }
                self.took_messages(inner, session, account, what, result);
            }
            (What::Search(reply), Answer::Messages(_)) => reply.push(Err(CoreError::Internal)),
            (What::Open(message), Answer::Search(_)) => self.failed(inner, message, FetchFailure::Failed),
            (What::Background, Answer::Search(_)) => {}
        }
    }

    /// A job came to nothing: the owner's hears why; the background waits a while.
    fn job_failed(&self, inner: &mut Inner, session: &mut Session, account: Id, what: What, why: FetchFailure, errors: &[String]) {
        match what {
            What::Open(message) => self.failed(inner, message, why),
            What::Search(reply) => {
                let lang = session.interface_language;
                let reason = match why {
                    FetchFailure::Offline => lang.t(words("There is no network connection.")),
                    FetchFailure::SignInNeeded => lang.t(words("The account has to be signed in again.")),
                    _ => lang.t(words("The server couldn't be searched. Try again in a few minutes.")),
                };
                reply.push(Err(CoreError::Failed { reason: reason.to_string() }));
            }
            What::Background => {
                let result = GraphFetchResult {
                    account,
                    offline: why == FetchFailure::Offline,
                    errors: if errors.is_empty() { vec!["no token for the mail".to_string()] } else { clipped(errors) },
                    ..Default::default()
                };
                let _ = session.driver.fetch.background_came(account, false, &result, self.now_ms());
            }
        }
    }

    fn took_messages(&self, inner: &mut Inner, session: &mut Session, account: Id, what: What, result: GraphFetchResult) {
        if matches!(what, What::Background)
            && let Some(stopped) = session.driver.fetch.background_came(account, false, &result, self.now_ms())
            && stopped.first
        {
            log::warn!("fetch: the background download stopped; it tries again in {} s", (stopped.wait_ms / 1_000.0) as u64);
        }
        let (too_large, missing, failed) = (result.too_large.clone(), result.missing.clone(), result.failed.clone());
        let (offline, busy) = (result.offline, result.busy_for.is_some());
        let any = !result.fetched.is_empty();
        if let Err(e) = mailcraft_engine::sync_driver::fetch::apply_fetch(session, result) {
            log::warn!("fetch: what came couldn't be taken in: {e}");
        }
        if any {
            // What came counts toward the phone's storage.
            inner.trim.due = true;
        }
        let What::Open(message) = what else { return };
        let Some(m) = session.mailbox.message(message) else { return self.failed(inner, message, FetchFailure::Gone) };
        if !m.unfetched {
            session.keep_for_now([message]);
            return self.fetched(inner, message);
        }
        let server = m.server.as_ref().map(|s| s.id.clone()).unwrap_or_default();
        let why = if too_large.contains(&server) {
            FetchFailure::TooLarge
        } else if missing.contains(&server) {
            FetchFailure::Gone
        } else if failed.contains(&server) {
            FetchFailure::Refused
        } else if offline {
            FetchFailure::Offline
        } else if busy {
            FetchFailure::Busy
        } else {
            FetchFailure::Failed
        };
        self.failed(inner, message, why);
    }

    /// A page of a search on the server: what it found into its folders, as headers, kept on the
    /// phone for the session.
    fn took_search(&self, inner: &mut Inner, session: &mut Session, account: Id, result: GraphSearchResult) -> Result<ServerSearchAnswer, CoreError> {
        let current = inner.fetch.searches.get(&account).is_some_and(|s| s.ticket == result.ticket);
        if !current || result.account != account {
            return Err(CoreError::Cancelled);
        }
        let email = session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
        if !result.email.is_empty() && !result.email.trim().eq_ignore_ascii_case(email.trim()) {
            return Err(CoreError::Failed { reason: "these results were for another mailbox".to_string() });
        }
        let here: HashMap<String, Id> = session
            .mailbox
            .messages
            .values()
            .filter(|m| m.account == account)
            .filter_map(|m| m.server.as_ref().map(|s| (s.id.clone(), m.id)))
            .collect();
        let mut answer = ServerSearchAnswer { offline: result.offline, errors: clipped(&result.errors), ..Default::default() };
        let mut added = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut found: Vec<&mailcraft_model::sync::GraphMessage> = result.messages.iter().take(MOST_ON_A_PAGE).collect();
        found.sort_by_key(|m| std::cmp::Reverse(m.received));
        for g in found {
            let id = g.server.id.as_str();
            if id.is_empty() || !seen.insert(id.to_string()) {
                continue;
            }
            if let Some(m) = here.get(id) {
                answer.messages.push(*m);
                continue;
            }
            let folder = session.mailbox.folder_by_server(account, &g.server.folder);
            let Some(folder) = folder.filter(|_| inner.fetch.search_kept < MOST_SEARCH_KEPT) else {
                answer.left_out = answer.left_out.saturating_add(1);
                continue;
            };
            match session.mailbox.add_message(search_server::as_message(account, folder, g)) {
                Ok(m) => {
                    inner.fetch.search_kept = inner.fetch.search_kept.saturating_add(1);
                    added.push(m);
                    answer.messages.push(m);
                }
                Err(_) => answer.left_out = answer.left_out.saturating_add(1),
            }
        }
        if !added.is_empty() {
            // Older than the window, as a rule: kept for the session whatever the limits say.
            session.keep_for_now(added.iter().copied());
            session.mailbox_changed();
        }
        if let Some(s) = inner.fetch.searches.get_mut(&account) {
            s.next = if result.refused.is_some() { String::new() } else { result.next.clone() };
            answer.more = !s.next.is_empty();
        }
        log::info!("fetch: a page of a search on the server brought {} new messages onto the phone", added.len());
        Ok(answer)
    }
}

/// Whether the phone's storage has room for the background to fetch more: below
/// [`STORAGE_FULL_PERCENT`] of its cap (no cap: always).
pub(crate) fn room_for_more(session: &Session) -> bool {
    let Some(limits) = session.mailbox.device_limits.map(|l| l.clamped()) else { return true };
    let used = session.mailbox.messages.values().map(|m| device_limits::local_bytes(m)).fold(0u64, u64::saturating_add);
    let full = limits.storage / 100 * STORAGE_FULL_PERCENT;
    used < full && session.mailbox.messages.len() as u64 <= limits.messages
}

/// A runner the tests swap in.
pub(crate) type RunnerSlot = Mutex<FetchRunner>;
