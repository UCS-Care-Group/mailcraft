// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `Platform` as the native side implements it (Kotlin on Android, Swift on iOS), mirroring
//! [`mailcraft_host::Platform`]. The core holds the platform; the platform never holds the core
//! (it is handed what it needs by the calls it gets), so there is no cycle for the native side to
//! leak.
//!
//! Left out, with the host trait's default standing in: `connect` (plain sockets; iOS's own
//! connection comes with its own task).

use std::sync::Arc;

use mailcraft_host::{NetworkState, Notification, NotificationKind, Redirect, SecretError, SecretStore as HostSecretStore, StoreTrouble};

use crate::HostError;

/// How the provider's page hands the owner back.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RedirectKind {
    /// A listener on the loopback address (a desktop; a phone doesn't answer this).
    Loopback,
    /// The program's own scheme (`com.example.app`), which the system routes back to it.
    CustomScheme { scheme: String },
}

/// What the platform says of the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NetworkKind {
    Unknown,
    Offline,
    Online { metered: bool, data_saving: bool },
}

/// What a notification is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NotificationTopic {
    Mail,
    Reminder,
}

/// A notification to show, or to show later. `key` is the program's own number for it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NotificationRecord {
    pub key: u64,
    pub topic: NotificationTopic,
    pub title: String,
    pub body: String,
}

/// Why the credential store couldn't be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TroubleKind {
    Unavailable,
    Locked,
}

/// The platform's credential store: Android's Keystore, iOS's Keychain. Keys are
/// `password:<address>` and `refresh:<address>`. Access tokens are never given to it.
#[uniffi::export(with_foreign)]
pub trait SecretStore: Send + Sync {
    fn get(&self, key: String) -> Result<Option<String>, HostError>;
    fn set(&self, key: String, secret: String) -> Result<(), HostError>;
    fn delete(&self, key: String) -> Result<(), HostError>;
    /// Why the store couldn't be used the last time; asking clears it.
    fn take_trouble(&self) -> Result<Option<TroubleKind>, HostError>;
}

/// What the native app gives the core.
///
/// Every method returns a `Result`: UniFFI turns a native exception into a panic in Rust for a
/// method that has no error to carry it in, and the core never panics. A method that fails is
/// given a safe stand-in by [`Native`] (no store, an unknown network, the system clock, no offset, no zone name).
#[uniffi::export(with_foreign)]
pub trait Platform: Send + Sync {
    /// The credential store, when there is one.
    fn secrets(&self) -> Result<Option<Arc<dyn SecretStore>>, HostError>;
    fn redirect(&self) -> Result<RedirectKind, HostError>;
    /// Show the provider's sign-in page at `url` in the system's browser (Custom Tabs,
    /// `ASWebAuthenticationSession`). An error means nothing was opened.
    fn open_sign_in_page(&self, url: String) -> Result<(), HostError>;
    fn notify(&self, notification: NotificationRecord) -> Result<(), HostError>;
    /// Have the system show `notification` at `at` (seconds since 1970, UTC), even when the app
    /// isn't running.
    fn schedule_reminder(&self, at: i64, notification: NotificationRecord) -> Result<(), HostError>;
    /// Open an attachment with the system's program for its kind; `name` is already made safe.
    fn open_file(&self, name: String, bytes: Vec<u8>) -> Result<(), HostError>;
    /// Now, in seconds since 1970 (UTC).
    fn now(&self) -> Result<i64, HostError>;
    /// Minutes east of UTC of the owner's time zone at `at`, daylight saving included.
    fn utc_offset_minutes(&self, at: i64) -> Result<i32, HostError>;
    /// The owner's time zone by its IANA name (`Europe/Berlin`: Android's `TimeZone.getDefault().id`,
    /// iOS's `TimeZone.current.identifier`). The core uses it as the calendar's zone when it knows
    /// the name, so that a mailbox stamped with a named zone is rebased correctly; a name that is
    /// missing or unknown leaves [`Platform::utc_offset_minutes`] in charge.
    fn time_zone_name(&self) -> Result<String, HostError>;
    /// The most bytes of mail content the system lets the app keep in memory; `None` when it
    /// doesn't say.
    fn memory_budget(&self) -> Result<Option<u64>, HostError>;
    fn network(&self) -> Result<NetworkKind, HostError>;
}

/// [`mailcraft_host::Platform`] over the native side. A native call that fails (it threw, or
/// answered in a way UniFFI can't read) is an error here and never a panic.
pub(crate) struct Native {
    pub(crate) inner: Arc<dyn Platform>,
}

pub(crate) struct NativeSecrets {
    inner: Arc<dyn SecretStore>,
}

#[cfg(test)]
impl NativeSecrets {
    pub(crate) fn new(inner: Arc<dyn SecretStore>) -> NativeSecrets {
        NativeSecrets { inner }
    }
}

impl HostSecretStore for NativeSecrets {
    fn get(&self, key: &str) -> Option<String> {
        self.inner.get(key.to_string()).ok().flatten()
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.inner.set(key.to_string(), secret.to_string()).map_err(SecretError::from)
    }

    fn delete(&self, key: &str) {
        let _ = self.remove(key);
    }

    fn read(&self, key: &str) -> Result<Option<String>, SecretError> {
        self.inner.get(key.to_string()).map_err(SecretError::from)
    }

    fn remove(&self, key: &str) -> Result<(), SecretError> {
        self.inner.delete(key.to_string()).map_err(SecretError::from)
    }

    fn take_trouble(&self) -> Option<StoreTrouble> {
        match self.inner.take_trouble() {
            Ok(Some(TroubleKind::Locked)) => Some(StoreTrouble::Locked),
            Ok(Some(TroubleKind::Unavailable)) | Err(_) => Some(StoreTrouble::Unavailable),
            Ok(None) => None,
        }
    }
}

fn record(n: &Notification) -> NotificationRecord {
    let topic = match n.kind {
        NotificationKind::Mail => NotificationTopic::Mail,
        NotificationKind::Reminder => NotificationTopic::Reminder,
    };
    NotificationRecord { key: n.key, topic, title: n.title.clone(), body: n.body.clone() }
}

impl mailcraft_host::Platform for Native {
    fn secrets(&self) -> Option<Arc<dyn HostSecretStore>> {
        let inner = self.inner.secrets().ok().flatten()?;
        Some(Arc::new(NativeSecrets { inner }) as Arc<dyn HostSecretStore>)
    }

    fn redirect(&self) -> Redirect {
        match self.inner.redirect() {
            Ok(RedirectKind::CustomScheme { scheme }) => Redirect::CustomScheme(scheme),
            Ok(RedirectKind::Loopback) | Err(_) => Redirect::Loopback,
        }
    }

    fn open_sign_in_page(&self, url: &str) -> Result<(), mailcraft_host::PlatformError> {
        self.inner.open_sign_in_page(url.to_string()).map_err(Into::into)
    }

    fn notify(&self, notification: &Notification) -> Result<(), mailcraft_host::PlatformError> {
        self.inner.notify(record(notification)).map_err(Into::into)
    }

    fn schedule_reminder(&self, at: mailcraft_time::Timestamp, notification: &Notification) -> Result<(), mailcraft_host::PlatformError> {
        self.inner.schedule_reminder(at, record(notification)).map_err(Into::into)
    }

    fn open_file(&self, name: &str, bytes: &[u8]) -> Result<(), mailcraft_host::PlatformError> {
        self.inner.open_file(name.to_string(), bytes.to_vec()).map_err(Into::into)
    }

    fn now(&self) -> mailcraft_time::Timestamp {
        match self.inner.now() {
            Ok(t) => t,
            Err(_) => {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().and_then(|d| i64::try_from(d.as_secs()).ok()).unwrap_or(0)
            }
        }
    }

    fn utc_offset_minutes(&self, at: mailcraft_time::Timestamp) -> i32 {
        self.inner.utc_offset_minutes(at).unwrap_or(0)
    }

    fn memory_budget(&self) -> Option<u64> {
        self.inner.memory_budget().ok().flatten()
    }

    fn network(&self) -> NetworkState {
        match self.inner.network() {
            Ok(NetworkKind::Offline) => NetworkState::Offline,
            Ok(NetworkKind::Online { metered, data_saving }) => NetworkState::Online { metered, data_saving },
            Ok(NetworkKind::Unknown) | Err(_) => NetworkState::Unknown,
        }
    }
}
