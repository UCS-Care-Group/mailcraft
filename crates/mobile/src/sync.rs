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
/// The credential-store entry that says a removal found the index damaged and the owner hasn't
/// been told yet that sign-ins it alone named may remain: kept there so that a restart between a
/// removal that failed and its retry doesn't lose the notice. Removed once it has been given.
const NOTICE_KEY: &str = "mailcraft:notice-sign-ins-may-remain";

/// Whether `name` is gone from `store`: removed without a failure, and read back as absent. A
/// read that fails counts as "still there".
fn removed(store: &dyn HostSecretStore, name: &str) -> bool {
    store.remove(name).is_ok() && matches!(store.read(name), Ok(None))
}

/// What a removal of the sign-ins came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Forgotten {
    /// How many entries were looked for.
    pub(crate) count: usize,
    /// The index of names didn't parse: it was set aside (or kept) and the sign-ins it alone
    /// named couldn't be found, so some may still be in the store.
    pub(crate) some_may_remain: bool,
}

/// Whether the index of names is in the store but doesn't parse (`None` when it is absent,
/// parses, or can't be read), and then whether it was set aside under a name of its own
/// (`mailcraft:index.unreadable-<ms>`) and read back as it was. It is set aside only once the
/// notice that sign-ins may remain is kept in the store (`NOTICE_KEY`), so that a restart can't
/// lose both; a store that can't hold either leaves the index where it is, and the next removal
/// finds it damaged again.
fn set_aside_index(store: &dyn HostSecretStore, now_s: i64) -> Option<bool> {
    let old = store.read(INDEX_KEY).ok().flatten()?;
    if serde_json::from_str::<Vec<String>>(&old).is_ok() {
        return None;
    }
    let noted = store.set(NOTICE_KEY, "1").is_ok() && store.read(NOTICE_KEY).is_ok_and(|v| v.is_some());
    if !noted {
        log::warn!("send/receive: the notice that sign-ins may remain couldn't be kept, so the damaged index is kept instead");
        return Some(false);
    }
    let aside = format!("{INDEX_KEY}.unreadable-{}", now_s.saturating_mul(1000));
    let kept = store.set(&aside, &old).is_ok() && store.read(&aside).is_ok_and(|v| v.as_deref() == Some(old.as_str()));
    if !kept {
        log::warn!("send/receive: an index of credential-store entries that doesn't parse couldn't be set aside; it is kept");
    }
    Some(kept)
}

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
    /// Names written while the index couldn't be read (the store unreachable or failing): they
    /// join it once it is read, and are never written over it unread.
    unindexed: BTreeSet<String>,
    /// The last write of the index of names failed: it is written again at the next wake.
    index_pending: bool,
    /// An index that didn't parse was set aside (or kept) by a removal of the sign-ins, so some
    /// it named may still be in the store: removals say so until one that succeeds has.
    index_garbled: bool,
    /// The store's word on a notice still to give (`NOTICE_KEY`) has been read, or written, in
    /// this run: it isn't read again.
    notice_checked: bool,
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
        if written.is_empty() && !inner.index_pending {
            return;
        }
        let secrets = self.host.platform().secrets();
        let Some(keys) = self.keys_of(inner, secrets.as_deref()) else {
            // The index can't be read now: what was written waits beside it for a later wake.
            for key in written {
                if inner.unindexed.len() < MOST_KEYS {
                    inner.unindexed.insert(key);
                } else {
                    log::warn!("send/receive: too many credential-store entries to index");
                }
            }
            inner.index_pending = true;
            return;
        };
        for key in written {
            if keys.len() < MOST_KEYS {
                keys.insert(key);
            } else {
                log::warn!("send/receive: too many credential-store entries to index");
            }
        }
        // Every batch that named a credential writes the index, and so does a wake after a write
        // that failed, so that "remove my data" can find every name after a restart.
        if let Some(store) = secrets.as_deref() {
            let list: Vec<&String> = keys.iter().collect();
            let kept = serde_json::to_string(&list).is_ok_and(|json| store.set(INDEX_KEY, &json).is_ok());
            if !kept {
                log::warn!("send/receive: the index of credential-store entries couldn't be kept");
            }
            inner.index_pending = !kept;
        }
    }

    /// The index of names, read from the store the first time it can be. `None` while the store
    /// can't be reached or read, or what it holds as the index doesn't parse: an index nobody could read is never taken for an empty one (and
    /// written over). Names noted meanwhile join it once it is read.
    fn keys_of<'a>(&self, inner: &'a mut Inner, store: Option<&dyn HostSecretStore>) -> Option<&'a mut BTreeSet<String>> {
        if inner.keys.is_none() {
            let found = store?.read(INDEX_KEY).ok()?;
            // An absent index is an empty one; one that is there but doesn't parse is as unread.
            let mut keys: BTreeSet<String> = match found {
                None => BTreeSet::new(),
                Some(json) => serde_json::from_str::<Vec<String>>(&json).ok()?.into_iter().take(MOST_KEYS).collect(),
            };
            let waiting = std::mem::take(&mut inner.unindexed);
            if !waiting.is_empty() {
                inner.index_pending = true;
            }
            for name in waiting {
                if keys.len() < MOST_KEYS {
                    keys.insert(name);
                }
            }
            inner.keys = Some(keys);
        }
        inner.keys.as_mut()
    }

    /// Remove every sign-in this core kept: the entries it wrote (by the index), and those of the
    /// mailbox's accounts by their names. Forgets the session's too. An error says that some are
    /// still in the credential store, or may be (it can't be reached, or a removal or a read
    /// failed): their names are kept, and an index that couldn't be read is left as it is, so
    /// that calling it again tries again. An index that is read but doesn't parse is set aside
    /// instead (kept as it is when it can't be), and the answer says that sign-ins only it
    /// named may still be in the store.
    pub(crate) fn forget_sign_ins(&self, session: &Session) -> Result<Forgotten, String> {
        let mut inner = self.lock();
        let store = self.platform.secrets();
        let secrets = self.host.platform().secrets();
        let mut indexed = self.keys_of(&mut inner, secrets.as_deref()).map(|keys| keys.clone());
        // A notice an earlier run couldn't give (its removal failed, then the app ended) is still
        // due. When the store can't say whether one is, the removal isn't taken for complete.
        let mut notice_unread = false;
        if !inner.notice_checked
            && let Some(store) = secrets.as_deref()
        {
            match store.read(NOTICE_KEY) {
                Ok(found) => {
                    inner.notice_checked = true;
                    inner.index_garbled |= found.is_some();
                }
                Err(_) => notice_unread = true,
            }
        }
        // An index that is there but doesn't parse would make every removal refuse for ever: it is
        // set aside under a name of its own and a fresh one starts from the names known now. When
        // it can't be set aside it stays as it is, and nothing is written over it.
        let mut garbled_kept = false;
        if indexed.is_none()
            && let Some(store) = secrets.as_deref()
            && let Some(set_aside) = set_aside_index(store, self.host.platform().now())
        {
            inner.index_garbled = true;
            if set_aside {
                let mut fresh: BTreeSet<String> = std::mem::take(&mut inner.unindexed).into_iter().take(MOST_KEYS).collect();
                for a in &session.mailbox.accounts {
                    if fresh.len() < MOST_KEYS {
                        fresh.insert(secret_key("refresh", a));
                    }
                }
                let kept = serde_json::to_string(&fresh.iter().collect::<Vec<_>>()).is_ok_and(|json| store.set(INDEX_KEY, &json).is_ok());
                inner.index_pending = !kept;
                inner.keys = Some(fresh.clone());
                indexed = Some(fresh);
            } else {
                garbled_kept = true;
            }
        }
        let index_read = indexed.is_some();
        let mut names: BTreeSet<String> = indexed.unwrap_or_default();
        names.extend(inner.unindexed.iter().cloned());
        for a in &session.mailbox.accounts {
            names.insert(secret_key("refresh", a));
        }
        inner.sr.forget();
        inner.needs_sign_in.clear();
        let no_store = matches!(store, Ok(None));
        let mut left: BTreeSet<String> = BTreeSet::new();
        match (store, secrets.as_deref()) {
            (Ok(Some(_)), Some(store)) => {
                for name in &names {
                    if !removed(store, name) {
                        left.insert(name.clone());
                    }
                }
                if left.is_empty() && index_read && !removed(store, INDEX_KEY) {
                    left.insert(INDEX_KEY.to_string());
                }
            }
            // No store was ever there: nothing was kept in it.
            (Ok(None), _) => {}
            // The store can't be reached now: what it holds can't be removed.
            _ => left.extend(names.iter().cloned()),
        }
        if left.is_empty() && notice_unread {
            // Everything named is gone, but sign-ins a damaged index named in an earlier run may
            // not be: the retry reads the notice again.
            return Err("the credential store couldn't be read, so sign-ins may still be in it".to_string());
        }
        if left.is_empty() && (index_read || no_store || garbled_kept) {
            // An index kept as it was, unparsed, stays unread: names written later wait beside it.
            inner.keys = if garbled_kept { None } else { Some(BTreeSet::new()) };
            inner.unindexed.clear();
            inner.index_pending = false;
            // The notice stays until the caller has given it (`notice_given`).
            return Ok(Forgotten { count: names.len(), some_may_remain: inner.index_garbled });
        }
        // What is left stays in the index, so that the next call finds it.
        left.remove(INDEX_KEY);
        let count = left.len();
        if !index_read {
            // The index couldn't be read: it stays in the store as it is, unread names and all,
            // and what is known to be left waits beside it.
            inner.unindexed = left.into_iter().take(MOST_KEYS).collect();
            inner.index_pending = !inner.unindexed.is_empty();
            return Err("the credential store couldn't be read, so sign-ins may still be in it".to_string());
        }
        let rewrite = !left.is_empty();
        inner.keys = Some(left.clone());
        inner.unindexed.clear();
        inner.index_pending = rewrite;
        if rewrite
            && let Some(store) = secrets.as_deref()
            && serde_json::to_string(&left.iter().collect::<Vec<_>>()).is_ok_and(|json| store.set(INDEX_KEY, &json).is_ok())
        {
            inner.index_pending = false;
        }
        Err(format!("{} sign-ins couldn't be removed from the credential store", count.max(1)))
    }

    /// The owner was told that sign-ins only a damaged index named may still be in the store:
    /// the next removal doesn't say it again.
    pub(crate) fn notice_given(&self) {
        let mut inner = self.lock();
        inner.index_garbled = false;
        inner.notice_checked = true;
        if let Some(store) = self.host.platform().secrets().as_deref()
            && !removed(store, NOTICE_KEY)
        {
            // It is given again after a restart, never lost.
            log::warn!("send/receive: the notice that sign-ins may remain couldn't be taken back from the store");
        }
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
            if inner.sr.eligible(session, id) {
                supported.push(id);
            } else if let Some(a) = session.mailbox.account(id) {
                inner.unsupported.insert(id, a.kind);
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
        // What the phone can't run is told of as `ask` does, whoever starts the round; an account
        // that went, or that the phone can now run, is no longer told of.
        inner.unsupported = server_accounts(session)
            .into_iter()
            .filter(|id| !inner.sr.eligible(session, *id))
            .filter_map(|id| session.mailbox.account(id).map(|a| (id, a.kind)))
            .collect();
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
