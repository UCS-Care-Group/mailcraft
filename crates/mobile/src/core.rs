// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `Core`: the one object the apps hold. Its own thread owns the `Session`, so the session never
//! has to be `Send` and a slow command never holds the app's main thread. Everything goes to that
//! thread as a job and comes back through a queue the foreign side awaits.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use mailcraft_engine::Session;
use mailcraft_host::sync::SendReceive;
use mailcraft_i18n::Lang;
use serde_json::Value;

use crate::events::{Event, EventSink};
use crate::keep::Keep;
use crate::platform::{Native, NetworkKind, Platform};
use crate::queue::Queue;
use crate::sign_in::Redirects;
use crate::sync::{FIRST_WAIT, SyncHost, follow_time_zone};
use crate::{CoreError, DataRemoved, android};

/// The longest id of a command that is looked up, and the most bytes of parameters that are read:
/// more is refused, as nothing a phone sends is that long.
const MAX_COMMAND_ID: usize = 128;
/// The longest `shutdown` waits for the last save.
const SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_PARAMS_BYTES: usize = 8 * 1024 * 1024;

pub(crate) type Job = Box<dyn FnOnce(&mut Session) + Send>;

/// Most calls waiting for the session thread, and most bytes of parameters they hold between
/// them: a call past either is refused at once (`Failed`), never waited for, so a session thread
/// held by a slow command can't be made to hold memory without end by repeated or cancelled calls.
pub(crate) const MAX_WAITING_JOBS: usize = 256;
const MAX_WAITING_BYTES: usize = 64 * 1024 * 1024;

/// The bytes of parameters that waiting calls hold.
#[derive(Default)]
struct Waiting(AtomicUsize);

/// A call's claim on [`MAX_WAITING_BYTES`], given back when its job has run or been dropped.
struct Claim {
    waiting: Arc<Waiting>,
    bytes: usize,
}

impl Claim {
    fn make(waiting: &Arc<Waiting>, bytes: usize) -> Option<Claim> {
        let before = waiting.0.fetch_add(bytes, Ordering::AcqRel);
        let claim = Claim { waiting: waiting.clone(), bytes };
        // (Dropping `claim` on a refusal gives the bytes back.)
        (before.saturating_add(bytes) <= MAX_WAITING_BYTES).then_some(claim)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.waiting.0.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// The device budgets of the cores that are alive. The store holds one limit for the process, so
/// it is the lowest of them: never above what any live core was told it may keep, and a core that
/// ends gives its share back.
#[derive(Default)]
pub(crate) struct Budgets(Vec<(u64, u64)>);

impl Budgets {
    pub(crate) fn add(&mut self, core: u64, bytes: u64) -> Option<u64> {
        self.0.push((core, bytes));
        self.lowest()
    }

    pub(crate) fn remove(&mut self, core: u64) -> Option<u64> {
        self.0.retain(|(c, _)| *c != core);
        self.lowest()
    }

    fn lowest(&self) -> Option<u64> {
        self.0.iter().map(|(_, b)| *b).min()
    }
}

static BUDGETS: Mutex<(u64, Budgets)> = Mutex::new((0, Budgets(Vec::new())));

fn enter_budget(device: Option<u64>) -> Option<u64> {
    let mut guard = BUDGETS.lock().unwrap_or_else(PoisonError::into_inner);
    guard.0 = guard.0.wrapping_add(1);
    let id = guard.0;
    let lowest = match device {
        Some(bytes) => guard.1.add(id, bytes),
        None => guard.1.0.iter().map(|(_, b)| *b).min(),
    };
    mailcraft_engine::store::set_device_budget(lowest);
    device.map(|_| id)
}

fn leave_budget(id: u64) {
    let mut guard = BUDGETS.lock().unwrap_or_else(PoisonError::into_inner);
    let lowest = guard.1.remove(id);
    mailcraft_engine::store::set_device_budget(lowest);
}

/// Cancels a call that was not yet started when the future waiting for it is dropped (the app's
/// coroutine was cancelled): the session thread then skips it.
pub(crate) struct CancelOnDrop(pub(crate) Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        // (Set after the job ran too, where nothing reads it any more.)
        self.0.store(true, Ordering::Release);
    }
}

#[derive(uniffi::Object)]
pub struct Core {
    jobs: Mutex<Option<SyncSender<Job>>>,
    waiting: Arc<Waiting>,
    /// This core's entry in [`BUDGETS`], when its device gave a budget.
    budget_id: Option<u64>,
    events: Arc<Queue<Event>>,
    pub(crate) platform: Arc<dyn Platform>,
    memory_budget: u64,
    language: AtomicU8,
    /// The latest language asked for, until the session thread has applied it (before its next job):
    /// it can't be lost to a full queue.
    pending_language: Arc<Mutex<Option<Lang>>>,
    /// Set when `shutdown` begins: a call still waiting in the queue is then cancelled and not run.
    pub(crate) closing: Arc<AtomicBool>,
    /// Send/Receive: what is due, what a run brought, the sign-ins of the session.
    pub(crate) sync: Arc<SyncHost>,
    /// Where the address a sign-in page came back to is handed over.
    pub(crate) redirects: Arc<Redirects>,
    /// A sign-in is open (one at a time).
    pub(crate) signing_in: Arc<AtomicBool>,
    /// The mailbox's storage, when it has one.
    keep: Option<Arc<Keep>>,
    /// Told once the session thread has ended (after its last save).
    finished: Mutex<Option<mpsc::Receiver<()>>>,
    session_thread: Option<std::thread::ThreadId>,
}

impl Core {
    /// A core over a session built by `make` on the core's own thread. For the tests, and for a
    /// host that opens a mailbox itself.
    pub fn start(platform: Arc<dyn Platform>, make: impl FnOnce() -> Session + Send + 'static) -> Result<Arc<Core>, CoreError> {
        Core::start_with(platform, make, SendReceive::new())
    }

    /// A core over the mailbox kept in `dir` (the app's private, no-backup folder), restored when
    /// there is one.
    pub(crate) fn start_in(platform: Arc<dyn Platform>, dir: &str) -> Result<Arc<Core>, CoreError> {
        Core::start_stored(platform, |keep| keep.map_or_else(Session::empty, |k| Session::new(k.open())), SendReceive::new(), Some(dir.to_string()))
    }

    /// [`Core::start`] with the Send/Receive state the caller made (the tests': runs that ask no server).
    pub(crate) fn start_with(
        platform: Arc<dyn Platform>,
        make: impl FnOnce() -> Session + Send + 'static,
        send_receive: SendReceive,
    ) -> Result<Arc<Core>, CoreError> {
        Core::start_stored(platform, move |_| make(), send_receive, None)
    }

    /// The one start: `make` builds the session (given the mailbox's storage when there is one
    /// it is to open from), on the core's thread.
    pub(crate) fn start_stored(
        platform: Arc<dyn Platform>,
        make: impl FnOnce(Option<&Keep>) -> Session + Send + 'static,
        send_receive: SendReceive,
        dir: Option<String>,
    ) -> Result<Arc<Core>, CoreError> {
        android::check()?;
        let host = mailcraft_host::init(Arc::new(Native { inner: platform.clone() }))?;
        // What the device can spare caps what the session keeps of mail content: the lowest of the
        // live cores' budgets (a device that says nothing adds none: the engine's own cap stands).
        let budget_id = enter_budget(host.platform().memory_budget());
        let events = Queue::new();
        let sink = EventSink { queue: events.clone() };
        let keep = match dir {
            Some(dir) => match Keep::prepare(&dir, sink.clone()) {
                Ok(k) => Some(k),
                Err(e) => {
                    budget_id.iter().for_each(|id| leave_budget(*id));
                    return Err(e);
                }
            },
            None => None,
        };
        // The phone's own zone stands in where the system names none (Android often has no
        // `/etc/localtime`), so that the day near midnight is the owner's.
        let zone_host = platform.clone();
        let zone_platform = platform.clone();
        let opening = keep.clone();
        let make = move || {
            let mut session = make(opening.as_deref());
            if let Some(keep) = &opening {
                // What the file holds is the mailbox as built (what opening puts right after
                // is written with the next save).
                session.path = Some(keep.path().to_path_buf());
                keep.opened(&session);
                // The steps every opening of a mailbox file takes (mail fetched again, a retired
                // choice dropped); the next save writes what they changed.
                let _ = mailcraft_engine::cmd::file::after_opening(&mut session);
            }
            // `with_clock` set the calendar's date before the phone's zone stood in: a date `make`
            // left at that default follows the zone, one it chose stays. Asked before either change.
            let default_date = session.view.date == session.today();
            // The phone's zone by its name, when it gives one this program knows: a mailbox
            // stamped with a named zone is then rebased correctly.
            follow_time_zone(&mut session, zone_platform.as_ref());
            session.fallback_offset = Some(Arc::new(move |at| zone_host.utc_offset_minutes(at).unwrap_or(0)));
            if default_date {
                session.view.date = session.today();
            }
            session
        };
        let (jobs, inbox) = mpsc::sync_channel::<Job>(MAX_WAITING_JOBS);
        let memory_budget = host.memory_budget();
        // The phone runs Microsoft (Graph) accounts only, for a clock-started round as for an asked one.
        let sync = Arc::new(SyncHost::new(host, platform.clone(), sink.clone(), send_receive.graph_only(), keep.clone()));
        // The session is built before `start` returns, so a `make` that panics is an error here and
        // not a core whose every call would wait for a reply that never comes.
        let (ready_tx, ready) = mpsc::sync_channel::<bool>(1);
        let (done_tx, finished) = mpsc::sync_channel::<()>(1);
        let thread_keep = keep.clone();
        let spawned = std::thread::Builder::new().name("mailcraft-session".to_string()).spawn(move || {
            run_session(make, inbox, &sink, &ready_tx, thread_keep.as_deref());
            let _ = done_tx.send(());
        });
        let session_thread = spawned.as_ref().ok().map(|h| h.thread().id());
        spawned.map_err(|_| {
            budget_id.iter().for_each(|id| leave_budget(*id));
            CoreError::Init { reason: "a thread for the session could not be started".to_string() }
        })?;
        if ready.recv() != Ok(true) {
            budget_id.iter().for_each(|id| leave_budget(*id));
            return Err(CoreError::Init { reason: "the session could not be opened".to_string() });
        }
        let core = Arc::new(Core {
            jobs: Mutex::new(Some(jobs)),
            waiting: Arc::default(),
            budget_id,
            events,
            platform,
            memory_budget,
            language: AtomicU8::new(0),
            pending_language: Arc::default(),
            closing: Arc::new(AtomicBool::new(false)),
            sync,
            redirects: Arc::default(),
            signing_in: Arc::default(),
            keep,
            finished: Mutex::new(Some(finished)),
            session_thread,
        });
        core.start_driver();
        Ok(core)
    }

    /// The thread that wakes Send/Receive: it holds the core weakly, so it ends with it.
    fn start_driver(self: &Arc<Core>) {
        let weak: Weak<Core> = Arc::downgrade(self);
        let sync = self.sync.clone();
        let closing = self.closing.clone();
        let started = std::thread::Builder::new().name("mailcraft-sync".to_string()).spawn(move || {
            let mut wait = FIRST_WAIT;
            loop {
                sync.pacer.wait(wait);
                if closing.load(Ordering::Acquire) {
                    return;
                }
                let Some(core) = weak.upgrade() else { return };
                core.enqueue_tick();
                wait = sync.next_wait();
            }
        });
        if started.is_err() {
            log::warn!("a thread for Send/Receive could not be started; it runs only when asked");
        }
    }

    /// Ask the session's thread for one wake of Send/Receive, unless one is already waiting.
    pub(crate) fn enqueue_tick(&self) {
        if self.closing.load(Ordering::Acquire) || self.sync.tick_queued.swap(true, Ordering::AcqRel) {
            return;
        }
        let sync = self.sync.clone();
        let closing = self.closing.clone();
        let job: Job = Box::new(move |session: &mut Session| {
            sync.tick_queued.store(false, Ordering::Release);
            // A tick queued before `shutdown` is drained after it: it starts no round.
            if closing.load(Ordering::Acquire) {
                return;
            }
            // (A panic is caught by the caller; this one leaves the mailbox as it was.)
            let _ = catch_unwind(AssertUnwindSafe(|| sync.tick(session)));
        });
        let sent = {
            let jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
            jobs.as_ref().map(|tx| tx.try_send(job))
        };
        if !matches!(sent, Some(Ok(()))) {
            self.sync.tick_queued.store(false, Ordering::Release);
        }
    }

    /// Where the sync driver (and anything else on the Rust side) puts events.
    pub fn sink(&self) -> EventSink {
        EventSink { queue: self.events.clone() }
    }

    /// Run `f` on the session's thread and wait for what it gives. A panic in `f` is an error and
    /// nothing else: the thread goes on. Dropping the future before the job started cancels it.
    pub(crate) async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Session) -> Result<T, CoreError> + Send + 'static,
    ) -> Result<T, CoreError> {
        self.call_holding(0, f).await
    }

    /// [`Core::call`] for a call that holds `bytes` of the caller's data while it waits.
    pub(crate) async fn call_holding<T: Send + 'static>(
        &self,
        bytes: usize,
        f: impl FnOnce(&mut Session) -> Result<T, CoreError> + Send + 'static,
    ) -> Result<T, CoreError> {
        let Some(claim) = Claim::make(&self.waiting, bytes) else {
            return Err(busy());
        };
        let reply = Queue::<Result<T, CoreError>>::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let _guard = CancelOnDrop(cancel.clone());
        let closing = self.closing.clone();
        let pending_language = self.pending_language.clone();
        let job: Job = {
            let reply = reply.clone();
            Box::new(move |session: &mut Session| {
                let _claim = claim;
                if let Some(lang) = pending_language.lock().unwrap_or_else(PoisonError::into_inner).take() {
                    session.interface_language = lang;
                }
                let out = if cancel.load(Ordering::Acquire) || closing.load(Ordering::Acquire) {
                    Err(CoreError::Cancelled)
                } else {
                    catch_unwind(AssertUnwindSafe(|| f(session))).unwrap_or(Err(CoreError::Internal))
                };
                reply.push(out);
            })
        };
        let sent = {
            let jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
            jobs.as_ref().map(|tx| tx.try_send(job))
        };
        match sent {
            Some(Ok(())) => {}
            Some(Err(TrySendError::Full(_))) => return Err(busy()),
            Some(Err(TrySendError::Disconnected(_))) | None => return Err(CoreError::Cancelled),
        }
        reply.next().await.unwrap_or(Err(CoreError::Cancelled))
    }
}

fn busy() -> CoreError {
    CoreError::Failed { reason: "too many calls are waiting; try again in a moment".to_string() }
}

/// The session thread: one job after another, `RevChanged` when a job moved the revision, the
/// autosave when it is due, and a last save when the core ends.
fn run_session(make: impl FnOnce() -> Session, inbox: mpsc::Receiver<Job>, sink: &EventSink, ready: &mpsc::SyncSender<bool>, keep: Option<&Keep>) {
    let Ok(mut session) = catch_unwind(AssertUnwindSafe(make)) else {
        let _ = ready.send(false);
        return sink.queue.close();
    };
    let _ = ready.send(true);
    let mut last = session.rev();
    let mut wait: Option<std::time::Duration> = None;
    loop {
        let job = match wait {
            Some(w) => match inbox.recv_timeout(w) {
                Ok(job) => Some(job),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match inbox.recv() {
                Ok(job) => Some(job),
                Err(_) => break,
            },
        };
        // (A job catches its own panic; this is for what it does outside it.)
        if let Some(job) = job
            && catch_unwind(AssertUnwindSafe(|| job(&mut session))).is_err()
        {
            continue;
        }
        let now = session.rev();
        if now != last {
            last = now;
            sink.push(Event::RevChanged { rev: now });
        }
        wait = keep.and_then(|k| catch_unwind(AssertUnwindSafe(|| k.after_job(&session))).unwrap_or(None));
    }
    if let Some(keep) = keep {
        // The last state goes to disk before the thread ends (a failure was reported by `save`).
        let _ = catch_unwind(AssertUnwindSafe(|| keep.save(&session)));
    }
    sink.queue.close();
}

#[uniffi::export]
impl Core {
    /// A core over an empty mailbox, on `platform`. On Android the platform's TLS verifier must
    /// have been set up before ([`android::verifier_is_ready`]).
    #[uniffi::constructor]
    pub fn new(platform: Arc<dyn Platform>) -> Result<Arc<Core>, CoreError> {
        Core::start(platform, Session::empty)
    }

    /// A core over the mailbox kept in `data_dir`, an absolute path of the app's private folder
    /// that is left out of backups (Android: `noBackupFilesDir/mailbox`): restored when there is
    /// one, new on a first start. The mail's bodies stay on disk as on the desktop. It is saved
    /// a few seconds after a change, before a send goes out, by [`Core::save_now`] and when the
    /// core ends. A file that can't be read is kept aside and a `StorageNotice` says so.
    #[uniffi::constructor]
    pub fn open(platform: Arc<dyn Platform>, data_dir: String) -> Result<Arc<Core>, CoreError> {
        Core::start_in(platform, &data_dir)
    }

    /// Write the mailbox to disk now (the app calls it from `onStop`). Nothing to do for a core
    /// that keeps no file. An error leaves the last good file as it was.
    pub async fn save_now(&self) -> Result<(), CoreError> {
        let Some(keep) = self.keep.clone() else { return Ok(()) };
        self.call(move |session| keep.save(session).map_err(|e| CoreError::Failed { reason: format!("the mailbox couldn't be saved: {e}") })).await
    }

    /// Remove the mail kept on this phone (the mailbox file, its content folder and what was
    /// kept aside) and every sign-in the credential store holds, and start over with an empty
    /// mailbox, for Settings > Remove account and data. An error says that some of it is still
    /// there (the session starts over with an empty mailbox all the same, so that nothing removed
    /// is written back); calling it again tries again. `DataRemoved::SignInsMayRemain` says that it was
    /// done but the list of the sign-ins kept was damaged, so older ones may still be in the
    /// phone's credential store: the app shows its message and then calls
    /// [`Core::sign_ins_notice_shown`]; until it does, every removal says it again.
    pub async fn delete_local_data(&self) -> Result<DataRemoved, CoreError> {
        let keep = self.keep.clone();
        let sync = self.sync.clone();
        self.call(move |session| {
            // Both are tried, so that one failing doesn't leave the other undone.
            let signed_in = sync.forget_sign_ins(session);
            let removed = match &keep {
                Some(keep) => keep.delete_all(),
                None => Ok(()),
            };
            // Whatever came of it, the session lets go of the mail: some or all of its files may
            // be gone, and a mailbox kept in memory would be written back by the next save. The
            // fresh one counts as saved, so nothing is written until the owner changes something;
            // the names of sign-ins still in the store are kept by `sync` for the retry.
            session.replace_mailbox(mailcraft_model::Mailbox::with_local_account("", ""));
            if let Some(keep) = &keep {
                keep.reset(session);
            }
            let forgotten = match (signed_in, removed) {
                (Ok(forgotten), Ok(())) => forgotten,
                (Err(reason), _) | (_, Err(reason)) => return Err(CoreError::Failed { reason }),
            };
            Ok(if forgotten.some_may_remain {
                DataRemoved::SignInsMayRemain { message: may_remain(session.interface_language) }
            } else {
                DataRemoved::All
            })
        })
        .await
    }

    /// Run the command `id` with `params_json` (a JSON object; empty means `{}`) through
    /// `Session::run`, the path of the CLI, the control channel and MCP, and give its result as
    /// JSON. A command that fails leaves the mailbox as it was.
    pub async fn execute(&self, id: String, params_json: String) -> Result<String, CoreError> {
        if id.len() > MAX_COMMAND_ID {
            return Err(CoreError::BadRequest { reason: "the command's id is too long".to_string() });
        }
        if params_json.len() > MAX_PARAMS_BYTES {
            return Err(CoreError::BadRequest { reason: "the parameters are too long".to_string() });
        }
        let params: Value = if params_json.trim().is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(&params_json).map_err(|_| CoreError::BadRequest { reason: "the parameters are not JSON".to_string() })?
        };
        // (The parsed parameters take more room than their text: counted four times over.)
        let held = params_json.len().saturating_mul(4);
        self.call_holding(held, move |session| {
            let result = session.run(&id, &params)?;
            serde_json::to_string(&result).map_err(|_| CoreError::Internal)
        })
        .await
    }

    /// The next event, waiting for one. `None` once the core is shut down and what was waiting
    /// has been read.
    pub async fn next_event(&self) -> Option<Event> {
        self.events.next().await
    }

    /// Stop: calls not yet started are cancelled, later calls fail with `Cancelled`, and
    /// `next_event` ends. The session thread ends with its last job.
    pub fn shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        self.sync.pacer.poke();
        self.redirects.close();
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner).take();
        // The session thread saves the mailbox as its last act: wait for it a little (never from
        // that thread itself, and never for long), so that a process about to end keeps the mail.
        if self.keep.is_some() && self.session_thread != Some(std::thread::current().id()) {
            let finished = self.finished.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(finished) = finished {
                let _ = finished.recv_timeout(SHUTDOWN_WAIT);
            }
        }
        self.events.close();
    }

    /// The most bytes of mail content to keep in memory: the lower of the device's budget and
    /// the engine's own cap.
    pub fn memory_budget(&self) -> u64 {
        self.memory_budget
    }

    /// What the platform says of the network now ("Offline" on the lists).
    pub fn network(&self) -> NetworkKind {
        match self.platform.network() {
            Ok(n) => n,
            Err(_) => NetworkKind::Unknown,
        }
    }

    /// The interface language from now on (`en`, `fr`, `auto` is the app's to resolve first):
    /// one the program has no catalogue for is English. Both `t` and `tn` read it.
    pub fn set_language(&self, code: String) {
        let lang = Lang::from_pref(&code, None);
        let at = Lang::ALL.iter().position(|l| *l == lang).and_then(|i| u8::try_from(i).ok()).unwrap_or(0);
        // (The session says dates and its own messages in it too: it takes the latest before its next
        // job, whatever the queue holds. One lock over both, so that two callers at once can't leave
        // the session in one language and `t` in another.)
        let mut pending = self.pending_language.lock().unwrap_or_else(PoisonError::into_inner);
        self.language.store(at, Ordering::Relaxed);
        *pending = Some(lang);
    }

    /// `msgid` (the English text) in the interface language; English when there is no
    /// translation. Looks up the catalogue of `mailcraft-i18n`; the app caches the answers.
    pub fn t(&self, msgid: String) -> String {
        self.lang().t(&msgid).to_string()
    }

    /// The form of `one` / `other` (English singular and plural) that `n` takes in the interface
    /// language, with `{n}` filled in.
    pub fn tn(&self, n: u64, one: String, other: String) -> String {
        mailcraft_i18n::fmt(self.lang().tn(n, &one, &other), &[("n", &n.to_string())])
    }
}

impl Core {
    #[cfg(test)]
    pub(crate) fn waiting_bytes(&self) -> usize {
        self.waiting.0.load(Ordering::Acquire)
    }

    pub(crate) fn lang(&self) -> Lang {
        Lang::ALL.get(usize::from(self.language.load(Ordering::Relaxed))).copied().unwrap_or_default()
    }
}

/// What the owner is told when the list of the sign-ins kept was damaged and set aside.
pub(crate) fn may_remain(lang: Lang) -> String {
    lang.t("The list of sign-ins kept on this phone was damaged, so some older sign-ins may still be kept on it. Clearing MailCraft's storage in Android's Settings removes them.").to_string()
}

impl Drop for Core {
    fn drop(&mut self) {
        self.shutdown();
        self.budget_id.iter().for_each(|id| leave_budget(*id));
    }
}
