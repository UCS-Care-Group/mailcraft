// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `Core`: the one object the apps hold. Its own thread owns the `Session`, so the session never
//! has to be `Send` and a slow command never holds the app's main thread. Everything goes to that
//! thread as a job and comes back through a queue the foreign side awaits.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};

use mailcraft_engine::Session;
use mailcraft_i18n::Lang;
use serde_json::Value;

use crate::events::{Event, EventSink};
use crate::platform::{Native, NetworkKind, Platform};
use crate::queue::Queue;
use crate::{CoreError, android};

/// The longest id of a command that is looked up, and the most bytes of parameters that are read:
/// more is refused, as nothing a phone sends is that long.
const MAX_COMMAND_ID: usize = 128;
const MAX_PARAMS_BYTES: usize = 8 * 1024 * 1024;

type Job = Box<dyn FnOnce(&mut Session) + Send>;

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
struct CancelOnDrop(Arc<AtomicBool>);

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
    platform: Arc<dyn Platform>,
    memory_budget: u64,
    language: AtomicU8,
    /// The latest language asked for, until the session thread has applied it (before its next job):
    /// it can't be lost to a full queue.
    pending_language: Arc<Mutex<Option<Lang>>>,
    /// Set when `shutdown` begins: a call still waiting in the queue is then cancelled and not run.
    closing: Arc<AtomicBool>,
}

impl Core {
    /// A core over a session built by `make` on the core's own thread. For the tests, and for a
    /// host that opens a mailbox itself.
    pub fn start(platform: Arc<dyn Platform>, make: impl FnOnce() -> Session + Send + 'static) -> Result<Arc<Core>, CoreError> {
        android::check()?;
        let host = mailcraft_host::init(Arc::new(Native { inner: platform.clone() }))?;
        // What the device can spare caps what the session keeps of mail content: the lowest of the
        // live cores' budgets (a device that says nothing adds none: the engine's own cap stands).
        let budget_id = enter_budget(host.platform().memory_budget());
        let events = Queue::new();
        // The phone's own zone stands in where the system names none (Android often has no
        // `/etc/localtime`), so that the day near midnight is the owner's.
        let zone_host = platform.clone();
        let make = move || {
            let mut session = make();
            // `with_clock` set the calendar's date before the phone's offset stood in: a date
            // `make` left at that default follows it, one it chose stays.
            let default_date = session.view.date == session.today();
            session.fallback_offset = Some(Arc::new(move |at| zone_host.utc_offset_minutes(at).unwrap_or(0)));
            if default_date {
                session.view.date = session.today();
            }
            session
        };
        let (jobs, inbox) = mpsc::sync_channel::<Job>(MAX_WAITING_JOBS);
        let sink = EventSink { queue: events.clone() };
        // The session is built before `start` returns, so a `make` that panics is an error here and
        // not a core whose every call would wait for a reply that never comes.
        let (ready_tx, ready) = mpsc::sync_channel::<bool>(1);
        std::thread::Builder::new().name("mailcraft-session".to_string()).spawn(move || run_session(make, inbox, &sink, &ready_tx)).map_err(
            |_| {
                budget_id.iter().for_each(|id| leave_budget(*id));
                CoreError::Init { reason: "a thread for the session could not be started".to_string() }
            },
        )?;
        if ready.recv() != Ok(true) {
            budget_id.iter().for_each(|id| leave_budget(*id));
            return Err(CoreError::Init { reason: "the session could not be opened".to_string() });
        }
        Ok(Arc::new(Core {
            jobs: Mutex::new(Some(jobs)),
            waiting: Arc::default(),
            budget_id,
            events,
            platform,
            memory_budget: host.memory_budget(),
            language: AtomicU8::new(0),
            pending_language: Arc::default(),
            closing: Arc::new(AtomicBool::new(false)),
        }))
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

/// The session thread: one job after another, and `RevChanged` when a job moved the revision.
fn run_session(make: impl FnOnce() -> Session, inbox: mpsc::Receiver<Job>, sink: &EventSink, ready: &mpsc::SyncSender<bool>) {
    let Ok(mut session) = catch_unwind(AssertUnwindSafe(make)) else {
        let _ = ready.send(false);
        return sink.queue.close();
    };
    let _ = ready.send(true);
    let mut last = session.rev();
    while let Ok(job) = inbox.recv() {
        // (A job catches its own panic; this is for what it does outside it.)
        if catch_unwind(AssertUnwindSafe(|| job(&mut session))).is_err() {
            continue;
        }
        let now = session.rev();
        if now != last {
            last = now;
            sink.push(Event::RevChanged { rev: now });
        }
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
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner).take();
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

    fn lang(&self) -> Lang {
        Lang::ALL.get(usize::from(self.language.load(Ordering::Relaxed))).copied().unwrap_or_default()
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.shutdown();
        self.budget_id.iter().for_each(|id| leave_budget(*id));
    }
}
