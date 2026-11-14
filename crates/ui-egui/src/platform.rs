// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop's [`Platform`]. The window's own [`Services`] belong to the window's thread (their
//! closures are not `Send`), while the host's worker threads hold the platform too, so the desktop
//! platform is a separate value: it keeps what is thread-safe (the credential store) and asks the
//! window, by message, for what only the window may do (open a page, open a file, show an alert).
//! The window answers in its own frame ([`Services::serve_platform`]); a worker that isn't
//! answered within [`ANSWER_WITHIN`] is told so. Nothing here does anything the window didn't
//! already do; an entry the desktop doesn't have is left to the trait's default.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use mailcraft_host::{Notification, NotificationKind, Platform, PlatformError, SecretStore};

use crate::Services;
use crate::alerts::{AlertKind, DesktopAlert, Opens};

/// How long a worker waits for the window to answer.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(10);

/// The most pages waiting to be opened off the window's thread at once.
const MOST_OPENING: usize = 8;

/// Pages being opened off the window's thread now.
static OPENING: AtomicUsize = AtomicUsize::new(0);

/// The most calls the window answers in one frame.
const MOST_PER_FRAME: usize = 16;

/// What a worker asks of the window, with where the answer goes.
enum Call {
    OpenPage(String, Answer, Arc<AtomicU8>),
    OpenFile(String, Vec<u8>, Answer),
    Alert(DesktopAlert, Answer),
}

/// A page's call has not been served yet.
const PENDING: u8 = 0;
/// The window has begun to open the page: its answer is the real outcome.
const STARTED: u8 = 1;
/// The worker gave up before the window began: the page is never opened.
const CANCELLED: u8 = 2;

type Answer = SyncSender<Result<(), PlatformError>>;

/// The calls workers have made of the window, waiting to be answered ([`Services::serve_platform`]).
pub struct PlatformCalls {
    tx: Sender<Call>,
    rx: Receiver<Call>,
}

impl Default for PlatformCalls {
    fn default() -> Self {
        let (tx, rx) = channel();
        PlatformCalls { tx, rx }
    }
}

impl PlatformCalls {
    /// The platform for the host's threads: the credential store as it is, and the window asked
    /// for the rest; `wake` makes the window look (it repaints).
    pub fn platform(&self, secrets: Option<Arc<dyn SecretStore>>, wake: Arc<dyn Fn() + Send + Sync>) -> DesktopPlatform {
        DesktopPlatform { secrets, calls: Mutex::new(self.tx.clone()), wake, window: std::thread::current().id(), patience: ANSWER_WITHIN }
    }
}

/// The desktop's [`Platform`], for the host's threads. It is for the host's threads only: a call
/// from the window's own thread (the one that made it) is refused at once, since that thread is
/// the one that answers. A call that "did not answer" may still be done by the window later, so a
/// caller must not retry it as if nothing had happened.
pub struct DesktopPlatform {
    secrets: Option<Arc<dyn SecretStore>>,
    calls: Mutex<Sender<Call>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// The thread that made this platform: the window's.
    window: ThreadId,
    patience: Duration,
}

impl DesktopPlatform {
    /// The same, waiting `patience` for an answer (tests).
    #[cfg(test)]
    pub(crate) fn patient(mut self, patience: Duration) -> Self {
        self.patience = patience;
        self
    }

    /// Put `call` to the window and wait for its answer.
    fn ask(&self, call: impl FnOnce(Answer) -> Call) -> Result<(), PlatformError> {
        self.ask_guarded(None, call)
    }

    /// The same; for a call that must not happen once it has been reported as failed, `state` is
    /// shared with the call: on a timeout the call is cancelled if the window has not begun it,
    /// and if it has, its real outcome is waited for (at most one more `patience`).
    fn ask_guarded(&self, state: Option<&AtomicU8>, call: impl FnOnce(Answer) -> Call) -> Result<(), PlatformError> {
        if std::thread::current().id() == self.window {
            return Err(PlatformError::Failed("the window cannot ask itself"));
        }
        let (answer, answered) = sync_channel(1);
        {
            let calls = self.calls.lock().map_err(|_| PlatformError::Failed("the window could not be asked"))?;
            calls.send(call(answer)).map_err(|_| PlatformError::Failed("the window could not be asked"))?;
        }
        (self.wake)();
        // wasm-clock: only the desktop app makes a `DesktopPlatform`; the browser build never does.
        match answered.recv_timeout(self.patience) {
            Ok(outcome) => outcome,
            Err(_) => match state {
                Some(state) if state.compare_exchange(PENDING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst).is_err() => {
                    // wasm-clock: as above, never in the browser.
                    answered.recv_timeout(self.patience).map_err(|_| PlatformError::Failed("the window did not answer"))?
                }
                _ => Err(PlatformError::Failed("the window did not answer")),
            },
        }
    }
}

impl Services {
    /// Answer what the host's threads asked of the window through the desktop platform (at most
    /// a few at a time; the rest wait for the next frame). Called every frame.
    /// Says whether a full batch was answered, so that more may be waiting: the caller asks for
    /// another frame then.
    pub fn serve_platform(&self) -> bool {
        for _ in 0..MOST_PER_FRAME {
            let Ok(call) = self.platform_calls.rx.try_recv() else { return false };
            match call {
                Call::OpenPage(url, answer, state) => self.open_page_for(url, answer, &state),
                Call::OpenFile(name, bytes, answer) => {
                    let _ = answer.send(match &self.open_file {
                        Some(open) => open(&name, &bytes).map_err(|_| PlatformError::Failed("the file could not be opened")),
                        None => Err(PlatformError::Unsupported),
                    });
                }
                Call::Alert(alert, answer) => {
                    let _ = answer.send(match &self.alert {
                        Some(host) => host.show(&alert).map_err(|_| PlatformError::Failed("the alert could not be shown")),
                        None => Err(PlatformError::Unsupported),
                    });
                }
            }
        }
        true
    }
}

impl Services {
    /// Open a page a worker asked for. A page that may wait for its activation token is opened on
    /// a thread of its own, as the sign-in worker did before: waiting here would stop the frame,
    /// and with it the code that gets the token (#144).
    fn open_page_for(&self, url: String, answer: Answer, state: &AtomicU8) {
        // A worker that gave up has been told nothing was opened: keep it so.
        if state.compare_exchange(PENDING, STARTED, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            let _ = answer.send(Err(PlatformError::Failed("the page was asked for too long ago")));
            return;
        }
        if let Some(open) = &self.open_page_waiting {
            if OPENING.fetch_add(1, Ordering::SeqCst) >= MOST_OPENING {
                OPENING.fetch_sub(1, Ordering::SeqCst);
                let _ = answer.send(Err(PlatformError::Failed("too many pages are being opened")));
                return;
            }
            let open = open.clone();
            let task = {
                let answer = answer.clone();
                move || {
                    let _ = answer.send(if open(&url) { Ok(()) } else { Err(PlatformError::Failed("no browser could be started")) });
                    OPENING.fetch_sub(1, Ordering::SeqCst);
                }
            };
            if std::thread::Builder::new().name("open page".into()).spawn(task).is_err() {
                OPENING.fetch_sub(1, Ordering::SeqCst);
                let _ = answer.send(Err(PlatformError::Failed("no browser could be started")));
            }
            return;
        }
        let _ = answer.send(match &self.open_page {
            Some(open) if open(&url) => Ok(()),
            Some(_) => Err(PlatformError::Failed("no browser could be started")),
            None => Err(PlatformError::Unsupported),
        });
    }
}

impl Platform for DesktopPlatform {
    fn secrets(&self) -> Option<Arc<dyn SecretStore>> {
        self.secrets.clone()
    }

    fn open_sign_in_page(&self, url: &str) -> Result<(), PlatformError> {
        let state = Arc::new(AtomicU8::new(PENDING));
        self.ask_guarded(Some(&state), |answer| Call::OpenPage(url.to_string(), answer, state.clone()))
    }

    /// Only a reminder: what a mail alert opens is the window's to say, so it raises those itself.
    fn notify(&self, notification: &Notification) -> Result<(), PlatformError> {
        if notification.kind != NotificationKind::Reminder {
            return Err(PlatformError::Unsupported);
        }
        let alert = DesktopAlert {
            key: notification.key,
            kind: AlertKind::Reminder,
            title: notification.title.clone(),
            body: notification.body.clone(),
            opens: Opens::Reminders,
        };
        self.ask(|answer| Call::Alert(alert, answer))
    }

    fn open_file(&self, name: &str, bytes: &[u8]) -> Result<(), PlatformError> {
        self.ask(|answer| Call::OpenFile(name.to_string(), bytes.to_vec(), answer))
    }
}
