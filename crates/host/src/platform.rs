// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a platform gives the program: the one trait the desktop, the phones and the tests
//! implement. Everything that differs from one system to the next is an entry here; the rest of
//! the program asks for it by this name and never by the system's own.

use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use mailcraft_time::Timestamp;

use crate::SecretStore;

/// Why a platform entry did nothing. The words are ours: they never carry a secret, an address
/// or what the system itself said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformError {
    /// This platform has no such thing (reminders scheduled for later on a desktop, say).
    Unsupported,
    /// The system refused or failed; `why` is a fixed phrase of ours, never what the system said.
    Failed(&'static str),
    /// A connection or a file failed in the way `kind` says.
    Io(std::io::ErrorKind),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::Unsupported => f.write_str("this platform doesn't offer that"),
            PlatformError::Failed(why) => f.write_str(why),
            PlatformError::Io(kind) => write!(f, "{kind}"),
        }
    }
}

impl std::error::Error for PlatformError {}

/// How the provider's page hands the owner back to the program once they have signed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redirect {
    /// A short-lived listener on the loopback address (the desktop).
    Loopback,
    /// The program's own scheme, which the system routes back to it (a phone).
    CustomScheme(String),
}

/// A notification to show, or to show later. `key` is the program's own number for it: a click
/// comes back with this and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub key: u64,
    pub kind: NotificationKind,
    /// The first line, plain text, already cleaned.
    pub title: String,
    /// The second line, plain text, already cleaned.
    pub body: String,
}

/// What a notification is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind {
    Mail,
    Reminder,
}

/// What the platform says of the network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetworkState {
    /// The platform doesn't say: try, as always.
    #[default]
    Unknown,
    Offline,
    Online {
        /// Paid by the byte: mobile data, a hotspot.
        metered: bool,
        /// The owner asked the system to save data (Data Saver, Low Data Mode).
        data_saving: bool,
    },
}

/// A connection that carries mail, from [`Platform::connect`].
pub trait Stream: Read + Write + Send {}

impl<T: Read + Write + Send> Stream for T {}

/// How many addresses of a name are tried before giving up.
const MOST_ADDRESSES: usize = 8;

/// What a platform gives the program. Every entry but the clock and the connection has no default
/// that does anything: an entry that isn't overridden says [`PlatformError::Unsupported`] or that
/// nothing is there.
///
/// It is `Send + Sync`: the host's worker threads hold it too (a job asks for the clock, the
/// network state or a secret from its own thread). A platform whose parts belong to one thread
/// (a window's) keeps them there and passes messages.
pub trait Platform: Send + Sync {
    /// The system's credential store, where there is one.
    fn secrets(&self) -> Option<Arc<dyn SecretStore>> {
        None
    }

    /// How the provider's page hands the owner back.
    fn redirect(&self) -> Redirect {
        Redirect::Loopback
    }

    /// Show the provider's sign-in page at `url`, in the system's browser. An error means nothing
    /// was opened, so nothing will come back.
    fn open_sign_in_page(&self, _url: &str) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Show `notification` now.
    fn notify(&self, _notification: &Notification) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Have the system show `notification` at `at`, even when the program isn't running.
    fn schedule_reminder(&self, _at: Timestamp, _notification: &Notification) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Open a file the owner asked to open (an attachment) with the system's own program for its
    /// kind. `name` is already made safe.
    fn open_file(&self, _name: &str, _bytes: &[u8]) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Now, in seconds since 1970 (UTC).
    fn now(&self) -> Timestamp {
        mailcraft_time::now()
    }

    /// Minutes east of UTC of the owner's time zone at `at`, daylight saving included.
    fn utc_offset_minutes(&self, at: Timestamp) -> i32 {
        mailcraft_time::local_offset_minutes(at)
    }

    /// The most memory, in bytes, the system lets the program use for what it keeps in memory;
    /// `None` when it doesn't say. See [`crate::memory_budget`].
    fn memory_budget(&self) -> Option<u64> {
        None
    }

    fn network(&self) -> NetworkState {
        NetworkState::Unknown
    }

    /// A connection to `host:port`, for mail. Plain sockets unless the platform has its own way
    /// (iOS, where account traffic goes by name). Encryption is the caller's: this is the bytes.
    /// `timeout` bounds making the connection and, on the returned stream, every read and every
    /// write: a peer that goes silent gives an error after it, never a hang.
    fn connect(&self, host: &str, port: u16, timeout: Duration) -> Result<Box<dyn Stream>, PlatformError> {
        let addresses = (host, port).to_socket_addrs().map_err(|e| PlatformError::Io(e.kind()))?;
        let mut last = std::io::ErrorKind::NotFound;
        for address in addresses.take(MOST_ADDRESSES) {
            match TcpStream::connect_timeout(&address, timeout) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(timeout)).map_err(|e| PlatformError::Io(e.kind()))?;
                    stream.set_write_timeout(Some(timeout)).map_err(|e| PlatformError::Io(e.kind()))?;
                    return Ok(Box::new(stream));
                }
                Err(e) => last = e.kind(),
            }
        }
        Err(PlatformError::Io(last))
    }
}
