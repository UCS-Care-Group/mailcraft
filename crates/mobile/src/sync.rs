// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send/Receive for the phone: the host's [`SendReceive`] (what is due, which run, what it
//! brought) driven on the session's thread, with the answers turned into the events the app
//! pulls and the summaries it asks for.
//!
//! One [`SyncHost`] belongs to a [`crate::Core`]. A small thread of the core wakes it (a
//! [`Pacer`]: quickly while a round is under way, now and then otherwise), the wake is a job on
//! the session's thread like any other call, and every result is applied there, so the mailbox
//! has one writer. The foreign side sees `RevChanged` (the session thread's own), `SyncProgress`,
//! `SignInNeeded`, `Notification` for new mail and `SyncFinished`.
//!
//! Secrets: a refresh token lives in memory for the session and, when the owner lets it be, in
//! the platform's credential store (through the host), and nowhere else; none is in a `Debug`
//! output, an event, an error or a log line. The names of the entries written are kept in an
//! index entry of that same store, so that "remove all my data" can find every one.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_engine::arrivals::{self, Arrival, InboxMark};
use mailcraft_engine::sync_driver::{ClientIds, server_accounts};
use mailcraft_host::sync::{Adopted, Context, Left, SendReceive, Step, Taken};
use mailcraft_host::{Host, NetworkState, SecretStore as HostSecretStore, secrets::secret_key};
use mailcraft_model::AccountKind;
use mailcraft_model::sync::Tokens;
use mailcraft_time::Zone;

use crate::events::{Event, EventSink};
use crate::keep::Keep;
use crate::platform::Platform;
use crate::queue::{Queue, Shed, Weight};
use crate::{NotificationRecord, NotificationTopic};

/// The application id the build was made with (`MAILCRAFT_MS_CLIENT_ID`): an identifier, not a secret.
pub(crate) const MS_CLIENT_ID: &str = match option_env!("MAILCRAFT_MS_CLIENT_ID") {
    Some(id) => id,
    None => "",
};

/// The credential-store entry that lists the names of the others the core wrote.
const INDEX_KEY: &str = "mailcraft:index";
/// The most names kept in the index (the host's own cap is the same).
const MOST_KEYS: usize = 1000;
/// How long the driver waits between wakes while a round is under way, and while none is.
const BUSY_WAIT: Duration = Duration::from_millis(250);
pub(crate) const IDLE_WAIT: Duration = Duration::from_secs(30);
/// The first wake after the core starts.
pub(crate) const FIRST_WAIT: Duration = Duration::from_secs(1);
/// The most lines a round keeps to tell the owner, and the longest one.
const MOST_ERRORS: usize = 20;
const LONGEST_ERROR: usize = 200;
/// How many new messages get a notification each; beyond that, one notification counts them.
const AT_ONCE: usize = 3;
const NAME_CHARS: usize = 64;
const SUBJECT_CHARS: usize = 120;

/// What `sync_now` says: whether a round is under way, and if not, why, for the owner.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncAnswer {
    pub started: bool,
    /// Empty when `started`; else the reason, in the interface language.
    pub message: String,
}

/// What a round came to.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct SyncSummary {
    /// An account was run (a round that found nothing to run, or was refused, says `false`).
    pub ran: bool,
    /// The round reached its end; `false` when it was called off for its time running out.
    pub finished: bool,
    /// New mail in the Inboxes of the owner's own accounts, as notifications count it (never an
    /// account's first download).
    pub new_mail: u64,
    /// What went wrong, in the interface language, for the owner. Never a secret or message text.
    pub errors: Vec<String>,
    /// Some account has to be signed in again.
    pub sign_in_needed: bool,
}

/// An account that is not sent and received here, and why.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct UnsupportedAccount {
    pub account: u64,
    pub message: String,
}

/// Where Send/Receive stands, for the screen that says so.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct SyncStatus {
    pub running: bool,
    /// What the last round said went wrong, in the interface language.
    pub messages: Vec<String>,
    /// Accounts whose sign-in has to be done again ([`crate::Core::sign_in`]).
    pub needs_sign_in: Vec<u64>,
    /// Accounts of a kind this version doesn't send and receive on the phone.
    pub unsupported: Vec<UnsupportedAccount>,
}

/// What wakes a `background_sync` that waits.
pub(crate) enum Wake {
    Done(SyncSummary),
    /// Its time is up.
    Budget,
}

impl Shed for Wake {
    fn weight(&self) -> Weight {
        Weight::Actionable
    }
}

/// A wake-up call for the driver thread: wait for a time, or until poked.
#[derive(Default)]
pub(crate) struct Pacer {
    poked: Mutex<bool>,
    cv: Condvar,
}

impl Pacer {
    pub(crate) fn poke(&self) {
        *self.poked.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.cv.notify_all();
    }

    pub(crate) fn wait(&self, at_most: Duration) {
        let guard = self.poked.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut guard, _) = self.cv.wait_timeout_while(guard, at_most, |poked| !*poked).unwrap_or_else(PoisonError::into_inner);
        *guard = false;
    }
}

#[derive(Default)]
struct Inner {
    sr: SendReceive,
    /// The mailbox as it was while nothing ran, and as the run under way found it.
    idle_mark: Option<InboxMark>,
    run_mark: Option<InboxMark>,
    /// A round is under way (the owner's, or one the clock started).
    active: bool,
    /// An account was run in it.
    ran: bool,
    errors: Vec<String>,
    last_errors: Vec<String>,
    last_summary: Option<SyncSummary>,
    needs_sign_in: BTreeSet<u64>,
    unsupported: BTreeMap<u64, AccountKind>,
    last_progress: Option<(u64, String, u64, u64)>,
    waiters: Vec<Arc<Queue<Wake>>>,
    /// The names of the credential-store entries written, once read from the store.
    keys: Option<BTreeSet<String>>,
    /// The app's own number for the next notification that counts mail.
    next_key: u64,
}

pub(crate) struct SyncHost {
    persist: Option<Persist>,
    host: Host,
    platform: Arc<dyn Platform>,
    sink: EventSink,
    started: Instant,
    inner: Mutex<Inner>,
    /// A round is under way (read by the driver thread to choose how long to wait).
    busy: AtomicBool,
    remember: AtomicBool,
    notify_mail: AtomicBool,
    pub(crate) tick_queued: AtomicBool,
    pub(crate) pacer: Pacer,
}

/// Writes the mailbox before a send goes out.
type Persist = Box<dyn Fn(&Session, u64) -> Result<(), String> + Send + Sync>;

impl SyncHost {
    pub(crate) fn new(host: Host, platform: Arc<dyn Platform>, sink: EventSink, sr: SendReceive, keep: Option<Arc<Keep>>) -> SyncHost {
        let persist: Option<Persist> = keep.map(|keep| Box::new(move |session: &Session, _rev: u64| keep.save(session)) as Persist);
        SyncHost {
            persist,
            host,
            platform,
            sink,
            started: Instant::now(),
            inner: Mutex::new(Inner { sr, ..Default::default() }),
            busy: AtomicBool::new(false),
            remember: AtomicBool::new(true),
            notify_mail: AtomicBool::new(true),
            tick_queued: AtomicBool::new(false),
            pacer: Pacer::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How long the driver waits before the next wake.
    pub(crate) fn next_wait(&self) -> Duration {
        if self.busy.load(Ordering::Relaxed) { BUSY_WAIT } else { IDLE_WAIT }
    }

    pub(crate) fn set_remember(&self, on: bool) {
        self.remember.store(on, Ordering::Relaxed);
    }

    pub(crate) fn set_notify_mail(&self, on: bool) {
        self.notify_mail.store(on, Ordering::Relaxed);
    }

    fn context<'a>(&'a self, session: &Session, secrets: Option<&'a dyn HostSecretStore>) -> Context<'a> {
        Context {
            secrets,
            remember: self.remember.load(Ordering::Relaxed),
            ids: ClientIds { microsoft_built_in: MS_CLIENT_ID, ..Default::default() },
            now_ms: self.started.elapsed().as_secs_f64() * 1000.0,
            now_s: self.host.platform().now(),
            minutes: f64::from(session.prefs.sync_minutes),
            // What a send marks as sending is on disk before the send goes out (AU3-26).
            persist: self.persist.as_ref().map(|p| p as &dyn Fn(&Session, u64) -> Result<(), String>),
            unconfirmed: false,
            data_removed: false,
            job_busy: None,
        }
    }

    /// The tokens of a sign-in the app made, taken in: refused when they are somebody else's.
    pub(crate) fn adopt(&self, session: &mut Session, account: u64, tokens: Tokens) -> Adopted {
        let mut inner = self.lock();
        let secrets = self.host.platform().secrets();
        let ctx = self.context(session, secrets.as_deref());
        let adopted = inner.sr.adopt_tokens(session, &ctx, account, tokens);
        if adopted == Adopted::Kept {
            inner.needs_sign_in.remove(&account);
        }
        self.note_keys(&mut inner);
        drop(inner);
        self.pacer.poke();
        adopted
    }

    /// Put what the host wrote to the credential store into the index of names.
    fn note_keys(&self, inner: &mut Inner) {
        let written = inner.sr.take_written_keys();
        if written.is_empty() {
            return;
        }
        let secrets = self.host.platform().secrets();
        let keys = self.keys_of(inner, secrets.as_deref());
        let before = keys.len();
        for key in written {
            if keys.len() < MOST_KEYS {
                keys.insert(key);
            } else {
                log::warn!("send/receive: too many credential-store entries to index");
            }
        }
        if keys.len() != before
            && let Some(store) = secrets.as_deref()
        {
            let list: Vec<&String> = keys.iter().collect();
            if let Ok(json) = serde_json::to_string(&list)
                && store.set(INDEX_KEY, &json).is_err()
            {
                log::warn!("send/receive: the index of credential-store entries couldn't be kept");
            }
        }
    }

    /// The index of names, read from the store the first time it is wanted.
    fn keys_of<'a>(&self, inner: &'a mut Inner, store: Option<&dyn HostSecretStore>) -> &'a mut BTreeSet<String> {
        inner.keys.get_or_insert_with(|| {
            let found = store.and_then(|s| s.get(INDEX_KEY)).and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok());
            found.unwrap_or_default().into_iter().take(MOST_KEYS).collect()
        })
    }

    /// Remove every sign-in this core kept: the entries it wrote (by the index), and those of the
    /// mailbox's accounts by their names. Forgets the session's too.
    pub(crate) fn forget_sign_ins(&self, session: &Session) -> usize {
        let mut inner = self.lock();
        let secrets = self.host.platform().secrets();
        let mut names: BTreeSet<String> = self.keys_of(&mut inner, secrets.as_deref()).clone();
        for a in &session.mailbox.accounts {
            names.insert(secret_key("refresh", a));
        }
        if let Some(store) = secrets.as_deref() {
            names.iter().for_each(|name| store.delete(name));
            store.delete(INDEX_KEY);
        }
        inner.keys = Some(BTreeSet::new());
        inner.sr.forget();
        inner.needs_sign_in.clear();
        names.len()
    }

    pub(crate) fn status(&self, session: &Session) -> SyncStatus {
        let inner = self.lock();
        let lang = session.interface_language;
        let message = lang.t("This kind of account can't be sent and received on the phone yet.").to_string();
        SyncStatus {
            running: inner.sr.busy() || !session.driver.round.queue.is_empty(),
            messages: inner.last_errors.clone(),
            needs_sign_in: inner.needs_sign_in.iter().copied().collect(),
            unsupported: inner.unsupported.keys().map(|account| UnsupportedAccount { account: *account, message: message.clone() }).collect(),
        }
    }

    /// The owner asked for a Send/Receive (or a background one does): start a round unless one is
    /// under way, and say so. `waiter`, when given, is woken with the round's summary.
    pub(crate) fn ask(&self, session: &mut Session, waiter: Option<Arc<Queue<Wake>>>) -> SyncAnswer {
        let mut inner = self.lock();
        let lang = session.interface_language;
        let refused = |message: &str| SyncAnswer { started: false, message: lang.t(message).to_string() };
        if session.prefs.offline {
            return refused("Work Offline is on. Turn it off to send and receive.");
        }
        if matches!(self.host.platform().network(), NetworkState::Offline) {
            return refused("There is no network connection.");
        }
        let accounts = server_accounts(session);
        if accounts.is_empty() {
            return refused("There is no account to send and receive.");
        }
        let mut supported = Vec::new();
        for id in accounts {
            match session.mailbox.account(id) {
                Some(a) if a.kind == AccountKind::Microsoft && !a.is_shared() => supported.push(id),
                Some(a) => {
                    inner.unsupported.insert(id, a.kind);
                }
                None => {}
            }
        }
        if supported.is_empty() {
            return refused("This kind of account can't be sent and received on the phone yet.");
        }
        if !inner.sr.busy() && session.driver.round.queue.is_empty() {
            let secrets = self.host.platform().secrets();
            let ctx = self.context(session, secrets.as_deref());
            if !inner.sr.start_round(session, &ctx, &supported) {
                return refused("Send/Receive isn't available now.");
            }
            inner.active = true;
            inner.ran = false;
            inner.errors.clear();
            inner.needs_sign_in.clear();
        }
        if let Some(waiter) = waiter {
            inner.waiters.push(waiter);
        }
        self.tick_locked(&mut inner, session);
        drop(inner);
        self.pacer.poke();
        SyncAnswer { started: true, message: String::new() }
    }

    /// Call the round off (its time ran out): the run under way stops at its next safe place, and
    /// the accounts still waiting are left for the next round.
    pub(crate) fn stop(&self, session: &mut Session) -> SyncSummary {
        let mut inner = self.lock();
        if !inner.active && inner.run_mark.is_none() {
            return inner.last_summary.clone().unwrap_or(SyncSummary { finished: true, ..Default::default() });
        }
        inner.sr.stop(session);
        session.driver.round.queue.clear();
        self.finish_round(&mut inner, session, false)
    }

    /// One wake of the driver, on the session's thread.
    pub(crate) fn tick(&self, session: &mut Session) {
        let mut inner = self.lock();
        self.tick_locked(&mut inner, session);
    }

    fn tick_locked(&self, inner: &mut Inner, session: &mut Session) {
        follow_time_zone(session, self.platform.as_ref());
        let platform_offline = matches!(self.host.platform().network(), NetworkState::Offline);
        let secrets = self.host.platform().secrets();
        let ctx = self.context(session, secrets.as_deref());
        if !inner.sr.busy() && inner.run_mark.is_none() {
            inner.idle_mark = Some(arrivals::mark(&session.mailbox));
        }
        let (taken, step) = if platform_offline && !inner.sr.busy() {
            // Nothing starts without a network; a round that was waiting for one is let go of.
            session.driver.round.queue.clear();
            (Taken::Nothing, Step::Idle)
        } else if platform_offline {
            (inner.sr.take_result(session, &ctx), Step::Waiting)
        } else {
            inner.sr.run_due(&self.host, session, &ctx)
        };
        self.took(inner, taken);
        self.stepped(inner, session, step);
        self.note_keys(inner);
        let working = inner.sr.busy() || !session.driver.round.queue.is_empty();
        self.busy.store(working, Ordering::Relaxed);
        if working {
            self.progress(inner, session);
        } else if inner.active || inner.run_mark.is_some() {
            self.finish_round(inner, session, true);
        }
    }

    fn took(&self, inner: &mut Inner, taken: Taken) {
        match taken {
            Taken::Done { errors, .. } => {
                for line in errors {
                    add_error(&mut inner.errors, line);
                }
            }
            Taken::SignInAgain { account, .. } => {
                inner.needs_sign_in.insert(account);
                self.sink.push(Event::SignInNeeded { account });
            }
            Taken::WrongPerson { account } => {
                inner.needs_sign_in.insert(account);
                add_error(&mut inner.errors, words("That sign-in is for somebody else's mailbox, not this account's. Nothing was kept.").to_string());
                self.sink.push(Event::SignInNeeded { account });
            }
            Taken::Nothing | Taken::Piece | Taken::Pending => {}
        }
    }

    fn stepped(&self, inner: &mut Inner, session: &mut Session, step: Step) {
        match step {
            Step::Started { .. } => {
                inner.ran = true;
                inner.active = true;
                if inner.run_mark.is_none() {
                    inner.run_mark = inner.idle_mark.take();
                }
            }
            Step::SignIn { account, .. } => {
                inner.needs_sign_in.insert(account);
                self.sink.push(Event::SignInNeeded { account });
            }
            Step::Unsupported { account, kind } => {
                session.sync_progress.leave_out(account);
                inner.unsupported.insert(account, kind);
            }
            Step::LeftOut { account, why } => match why {
                Left::NoSignIn => {
                    inner.needs_sign_in.insert(account);
                }
                Left::Gone => {}
                Left::Refused => add_error(&mut inner.errors, words("An account couldn't be sent and received.").to_string()),
                Left::NotSaved => add_error(&mut inner.errors, words("Your mail couldn't be saved first, so nothing was sent.").to_string()),
            },
            Step::Idle | Step::Waiting => {}
        }
    }

    /// Tell the app how far the round has got, when that changed.
    fn progress(&self, inner: &mut Inner, session: &Session) {
        let lang = session.interface_language;
        let now = session.sync_progress.current().map(|run| {
            let phase = run.phase.map(|p| lang.t(p.label()).to_string()).unwrap_or_default();
            (run.account, phase, u64::from(run.done), u64::from(run.total))
        });
        if now != inner.last_progress {
            if let Some((account, phase, done, total)) = &now {
                self.sink.push(Event::SyncProgress { account: *account, phase: phase.clone(), done: *done, total: *total });
            }
            inner.last_progress = now;
        }
    }

    /// The round is over (or was called off): notifications for what arrived, the summary, and
    /// whoever waits for it.
    fn finish_round(&self, inner: &mut Inner, session: &Session, finished: bool) -> SyncSummary {
        let lang = session.interface_language;
        let found = inner.run_mark.take().map(|mark| arrivals::arrivals(&session.mailbox, &mark)).unwrap_or_default();
        if self.notify_mail.load(Ordering::Relaxed) {
            for n in plan_notifications(session, &found, &mut inner.next_key) {
                self.sink.push(Event::Notification { notification: n });
            }
        }
        let errors: Vec<String> = std::mem::take(&mut inner.errors).iter().map(|e| lang.t(e).to_string()).collect();
        inner.last_errors = errors.clone();
        let summary = SyncSummary {
            ran: inner.ran,
            finished,
            new_mail: u64::try_from(found.len()).unwrap_or(u64::MAX),
            errors,
            sign_in_needed: !inner.needs_sign_in.is_empty(),
        };
        inner.active = false;
        inner.ran = false;
        inner.idle_mark = None;
        inner.last_progress = None;
        inner.last_summary = Some(summary.clone());
        for waiter in inner.waiters.drain(..) {
            waiter.push(Wake::Done(summary.clone()));
        }
        self.sink.push(Event::SyncFinished { new_mail: summary.new_mail, finished });
        summary
    }
}

/// Marks a sentence for the catalogue (`cargo xtask i18n` gathers what is passed to it); it is put
/// through the interface language when the round's summary is made.
fn words(text: &'static str) -> &'static str {
    text
}

fn add_error(errors: &mut Vec<String>, line: String) {
    if errors.len() < MOST_ERRORS {
        errors.push(line.chars().take(LONGEST_ERROR).collect());
    }
}

/// The notifications for new mail: one each for up to [`AT_ONCE`], else one that counts them. What
/// they say of a message is its own text, hostile, made plain and short as a name is shown.
fn plan_notifications(session: &Session, found: &[Arrival], next_key: &mut u64) -> Vec<NotificationRecord> {
    let lang = session.interface_language;
    if found.is_empty() {
        return Vec::new();
    }
    if found.len() > AT_ONCE {
        *next_key = next_key.wrapping_add(1);
        let body = mailcraft_i18n::fmt(lang.tn(found.len() as u64, "{n} new message", "{n} new messages"), &[("n", &found.len().to_string())]);
        return vec![NotificationRecord { key: *next_key, topic: NotificationTopic::Mail, title: lang.t("New mail").to_string(), body }];
    }
    found
        .iter()
        .map(|a| {
            let sender = mailcraft_names::shown(&a.sender, NAME_CHARS);
            let subject = mailcraft_names::shown(&a.subject, SUBJECT_CHARS);
            NotificationRecord {
                key: a.message,
                topic: NotificationTopic::Mail,
                title: if sender.is_empty() { lang.t("New message").to_string() } else { sender },
                body: if subject.is_empty() { lang.t("(no subject)").to_string() } else { subject },
            }
        })
        .collect()
}

/// The phone's time zone by its IANA name, when it names one this program knows: the calendar is
/// then shown in it, and a mailbox stamped with another zone is rebased. A missing or unknown name
/// leaves the phone's own offset (`fallback_offset`) in charge.
pub(crate) fn follow_time_zone(session: &mut Session, platform: &dyn Platform) {
    let Ok(name) = platform.time_zone_name() else { return };
    let Some(zone) = Zone::named(&name).filter(|zone| !zone.is_local()) else { return };
    if session.clock.zone != Some(zone) {
        session.clock.zone = Some(zone);
    }
    session.settle_zone();
}
