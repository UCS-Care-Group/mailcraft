// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop's [`Platform`]. The window's own [`Services`] belong to the window's thread (their
//! closures are not `Send`), while the host's worker threads hold the platform too, so the desktop
//! platform is a separate value: it keeps what is thread-safe (the credential store) and asks the
//! window, by message, for what only the window may do (open a page, open a file, show an alert).
//! The window answers in its own frame ([`Services::serve_platform`]); a worker that isn't
//! answered within [`ANSWER_WITHIN`] is told so. Nothing here does anything the window didn't
//! already do; an entry the desktop doesn't have is left to the trait's default.

use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_host::{Notification, NotificationKind, Platform, PlatformError, SecretStore};

use crate::Services;
use crate::alerts::{AlertKind, DesktopAlert, Opens};

/// How long a worker waits for the window to answer.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(10);

/// The most calls the window answers in one frame.
const MOST_PER_FRAME: usize = 16;

/// What a worker asks of the window, with where the answer goes.
enum Call {
    OpenPage(String, Answer),
    OpenFile(String, Vec<u8>, Answer),
    Alert(DesktopAlert, Answer),
}

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
        DesktopPlatform { secrets, calls: Mutex::new(self.tx.clone()), wake }
    }
}

/// The desktop's [`Platform`], for the host's threads.
pub struct DesktopPlatform {
    secrets: Option<Arc<dyn SecretStore>>,
    calls: Mutex<Sender<Call>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl DesktopPlatform {
    /// Put `call` to the window and wait for its answer.
    fn ask(&self, call: impl FnOnce(Answer) -> Call) -> Result<(), PlatformError> {
        let (answer, answered) = sync_channel(1);
        {
            let calls = self.calls.lock().map_err(|_| PlatformError::Failed("the window could not be asked"))?;
            calls.send(call(answer)).map_err(|_| PlatformError::Failed("the window could not be asked"))?;
        }
        (self.wake)();
        answered.recv_timeout(ANSWER_WITHIN).map_err(|_| PlatformError::Failed("the window did not answer"))?
    }
}

impl Services {
    /// Answer what the host's threads asked of the window through the desktop platform (at most
    /// a few at a time; the rest wait for the next frame). Called every frame.
    pub fn serve_platform(&self) {
        for _ in 0..MOST_PER_FRAME {
            let Ok(call) = self.platform_calls.rx.try_recv() else { return };
            match call {
                Call::OpenPage(url, answer) => {
                    let _ = answer.send(match &self.open_page {
                        Some(open) if open(&url) => Ok(()),
                        Some(_) => Err(PlatformError::Failed("no browser could be started")),
                        None => Err(PlatformError::Unsupported),
                    });
                }
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
    }
}

impl Platform for DesktopPlatform {
    fn secrets(&self) -> Option<Arc<dyn SecretStore>> {
        self.secrets.clone()
    }

    fn open_sign_in_page(&self, url: &str) -> Result<(), PlatformError> {
        self.ask(|answer| Call::OpenPage(url.to_string(), answer))
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
