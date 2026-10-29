// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The operating system's credential store: Windows Credential Manager, the macOS keychain, and
//! on Linux and FreeBSD the desktop's Secret Service (GNOME Keyring, KWallet…) over D-Bus.
//!
//! A password or a refresh token the owner asks to be remembered is kept here, encrypted by the
//! system for this user, and nowhere else: never in the mailbox file, the settings or a log.
//! When the store can't be used (no Secret Service is running, or it stays locked), nothing is
//! remembered between runs, as on systems without a store, and the window is told why
//! ([`SecretStore::take_trouble`]) so that it can say so once.

use mailcraft_ui_egui::SecretStore;

/// The store on systems that have one.
pub fn store() -> Option<std::sync::Arc<dyn SecretStore>> {
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd"))]
    {
        Some(std::sync::Arc::new(os::OsStore::default()))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd")))]
    {
        None
    }
}

/// Windows keeps at most 2560 bytes per credential (1280 UTF-16 units), and a refresh token can
/// be longer, so a secret is kept in pieces: `<key>#n` holds how many, `<key>#0`… hold them.
/// (The same on every system, so that what is kept looks alike everywhere.)
#[cfg_attr(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd", test)), allow(dead_code))]
const PIECE: usize = 1000;
/// More pieces than any secret needs; a count beyond it is damage, not a secret.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd", test)), allow(dead_code))]
const MAX_PIECES: usize = 16;

/// `secret` cut at character boundaries into pieces of at most [`PIECE`] characters.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd", test)), allow(dead_code))]
fn pieces(secret: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = secret.chars().collect();
    let pieces: Vec<String> = chars.chunks(PIECE).map(|c| c.iter().collect()).collect();
    (pieces.len() <= MAX_PIECES).then_some(pieces)
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd"))]
mod os {
    use std::sync::{Arc, Mutex};

    use keyring_core::{CredentialStore, Entry, Error};
    use mailcraft_ui_egui::{SecretStore, StoreTrouble};

    use super::{MAX_PIECES, pieces};

    /// What the entries are filed under in the system's store. Together with the entry's name
    /// (`<key>#n`, `<key>#0`…) it is how an entry is found again, so it never changes: on Windows
    /// the credential is `<key>#0.MailCraft`, in the macOS keychain a generic password whose
    /// service is `MailCraft` and account `<key>#0`, and in the Secret Service an item whose
    /// `service` and `username` attributes are the two. (keyring 3, which 0.4 used, filed them
    /// alike; the attributes it added in the Secret Service are not searched on.)
    pub(super) const SERVICE: &str = "MailCraft";

    /// Opens the system's store.
    type Connect = Box<dyn Fn() -> keyring_core::Result<Arc<CredentialStore>> + Send + Sync>;

    /// The system's own store.
    fn connect() -> keyring_core::Result<Arc<CredentialStore>> {
        #[cfg(target_os = "windows")]
        let store: Arc<CredentialStore> = windows_native_keyring_store::Store::new()?;
        #[cfg(target_os = "macos")]
        let store: Arc<CredentialStore> = apple_native_keyring_store::keychain::Store::new()?;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let store: Arc<CredentialStore> = zbus_secret_service_keyring_store::Store::new()?;
        Ok(store)
    }

    /// Whether an error says the store itself can't be used, and how, rather than that nothing is
    /// kept under a key or that one entry is unreadable. Decided from the error's kind alone,
    /// never from its text.
    pub(super) fn trouble(e: &Error) -> Option<StoreTrouble> {
        match e {
            // No store answered: on Linux no Secret Service on the session bus, or no session bus.
            Error::PlatformFailure(_) | Error::NoDefaultStore => Some(StoreTrouble::Unavailable),
            // The store is there but locked, and unlocking it was dismissed or refused.
            Error::NoStorageAccess(_) => Some(StoreTrouble::Locked),
            _ => None,
        }
    }

    pub struct OsStore {
        /// Why the store couldn't be used the last time, until the window takes it.
        trouble: Mutex<Option<StoreTrouble>>,
        /// The store once opened: not before it is first needed, and opened again after it
        /// couldn't be reached (a Secret Service started, or started again, after the window).
        store: Mutex<Option<Arc<CredentialStore>>>,
        connect: Connect,
    }

    impl Default for OsStore {
        fn default() -> Self {
            Self::with(Box::new(connect))
        }
    }

    impl OsStore {
        /// A store that opens what `connect` gives: the system's, or in tests a mock.
        pub(super) fn with(connect: Connect) -> Self {
            Self { trouble: Mutex::new(None), store: Mutex::new(None), connect }
        }

        /// Remember why the store couldn't be used, when that is what `e` says.
        pub(super) fn note(&self, e: &Error) {
            let Some(t) = trouble(e) else { return };
            if let Ok(mut kept) = self.trouble.lock() {
                *kept = Some(t);
            }
            // Unreachable: open it again next time rather than keep a connection that is gone.
            if t == StoreTrouble::Unavailable
                && let Ok(mut store) = self.store.lock()
            {
                *store = None;
            }
        }

        fn store(&self) -> keyring_core::Result<Arc<CredentialStore>> {
            let mut kept = self.store.lock().map_err(|_| Error::NoDefaultStore)?;
            if let Some(store) = kept.as_ref() {
                return Ok(store.clone());
            }
            let store = (self.connect)()?;
            *kept = Some(store.clone());
            Ok(store)
        }

        fn build(&self, key: &str, part: &str) -> keyring_core::Result<Entry> {
            self.store()?.build(SERVICE, &format!("{key}#{part}"), None)
        }

        fn entry(&self, key: &str, part: &str) -> Option<Entry> {
            self.build(key, part).map_err(|e| self.note(&e)).ok()
        }

        fn read(&self, key: &str, part: &str) -> Option<String> {
            self.entry(key, part)?.get_password().map_err(|e| self.note(&e)).ok()
        }

        fn count(&self, key: &str) -> Option<usize> {
            self.read(key, "n")?.parse::<usize>().ok().filter(|n| (1..=MAX_PIECES).contains(n))
        }
    }

    impl SecretStore for OsStore {
        fn get(&self, key: &str) -> Option<String> {
            let mut secret = String::new();
            for i in 0..self.count(key)? {
                secret.push_str(&self.read(key, &i.to_string())?);
            }
            Some(secret)
        }

        fn set(&self, key: &str, secret: &str) -> Result<(), String> {
            // The error text names the store's complaint, never the secret.
            let refused = |e: Error| {
                self.note(&e);
                format!("the system's credential store refused: {e}")
            };
            let pieces = pieces(secret).ok_or("it is too long for the system's credential store")?;
            self.delete(key);
            for (i, piece) in pieces.iter().enumerate() {
                self.build(key, &i.to_string()).and_then(|e| e.set_password(piece)).map_err(refused)?;
            }
            self.build(key, "n").and_then(|e| e.set_password(&pieces.len().to_string())).map_err(refused)
        }

        fn delete(&self, key: &str) {
            let pieces = self.count(key).unwrap_or(0);
            for part in (0..pieces).map(|i| i.to_string()).chain(["n".to_string()]) {
                if let Some(e) = self.entry(key, &part) {
                    // Nothing kept under the key is what deleting wants anyway.
                    if let Err(err) = e.delete_credential() {
                        self.note(&err);
                    }
                }
            }
        }

        fn take_trouble(&self) -> Option<StoreTrouble> {
            self.trouble.lock().ok()?.take()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_secret_is_cut_at_character_boundaries_and_goes_back_together() {
        let secret: String = "töken-ñ".chars().cycle().take(PIECE * 2 + 7).collect();
        let cut = pieces(&secret).unwrap();
        assert_eq!(cut.len(), 3);
        assert!(cut.iter().all(|p| p.chars().count() <= PIECE));
        assert_eq!(cut.concat(), secret);
        assert_eq!(pieces("short").unwrap(), ["short"]);
        assert_eq!(pieces("").unwrap().len(), 0);
        // Longer than any secret: refused rather than filling the store.
        assert!(pieces(&"x".repeat(PIECE * MAX_PIECES + 1)).is_none());
    }

    /// What each of the store's complaints means for the owner, without a store to complain:
    /// only a store that can't be reached or stays locked is trouble worth telling.
    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd"))]
    fn only_a_store_that_cant_be_used_is_trouble() {
        use mailcraft_ui_egui::StoreTrouble;
        use os::trouble;
        let boxed = |s: &str| -> Box<dyn std::error::Error + Send + Sync> { Box::new(std::io::Error::other(s.to_string())) };
        // As the Secret Service answers when none is running, and when the owner won't unlock it.
        let none_running = keyring_core::Error::PlatformFailure(boxed("org.freedesktop.DBus.Error.ServiceUnknown"));
        assert_eq!(trouble(&none_running), Some(StoreTrouble::Unavailable));
        assert_eq!(trouble(&keyring_core::Error::NoStorageAccess(boxed("prompt dismissed"))), Some(StoreTrouble::Locked));
        // Nothing kept, or one damaged entry: the store works, and there is nothing to say.
        assert_eq!(trouble(&keyring_core::Error::NoEntry), None);
        assert_eq!(trouble(&keyring_core::Error::BadEncoding(vec![0xff])), None);
        assert_eq!(trouble(&keyring_core::Error::TooLong("password".into(), 10)), None);
        assert_eq!(trouble(&keyring_core::Error::Invalid("user".into(), "empty".into())), None);
        assert_eq!(trouble(&keyring_core::Error::Ambiguous(Vec::new())), None);
    }

    /// The store remembers the last trouble until it is taken, and then has nothing to say.
    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd"))]
    fn trouble_is_taken_once() {
        use mailcraft_ui_egui::StoreTrouble;
        let store = os::OsStore::default();
        assert_eq!(store.take_trouble(), None);
        let locked = keyring_core::Error::NoStorageAccess(Box::new(std::io::Error::other("locked")));
        store.note(&locked);
        // An entry that isn't there says nothing about the store, and leaves the trouble as it was.
        store.note(&keyring_core::Error::NoEntry);
        assert_eq!(store.take_trouble(), Some(StoreTrouble::Locked));
        assert_eq!(store.take_trouble(), None);
    }

    /// What is kept is filed exactly as 0.4 (keyring 3) filed it, service `MailCraft` and entries
    /// `<key>#n`, `<key>#0`…, so that what an earlier version remembered is found: in keyring's
    /// mock store, never the system's.
    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd"))]
    fn entries_keep_the_names_earlier_versions_gave_them() {
        use std::sync::Arc;

        use keyring_core::{CredentialStore, mock};
        let mock: Arc<CredentialStore> = mock::Store::new().unwrap();
        let store = {
            let mock = mock.clone();
            os::OsStore::with(Box::new(move || Ok(mock.clone())))
        };
        let entry = |user: &str| mock.build("MailCraft", user, None).unwrap();
        assert_eq!(os::SERVICE, "MailCraft");

        // Kept by an earlier version, in two pieces: found and put back together.
        let key = "refresh:old@mailcraft.example";
        entry(&format!("{key}#n")).set_password("2").unwrap();
        entry(&format!("{key}#0")).set_password("first-").unwrap();
        entry(&format!("{key}#1")).set_password("second").unwrap();
        assert_eq!(store.get(key).as_deref(), Some("first-second"));

        // Kept now: under the same names, and nothing left of the longer one before it.
        store.set(key, "new").unwrap();
        assert_eq!(entry(&format!("{key}#n")).get_password().unwrap(), "1");
        assert_eq!(entry(&format!("{key}#0")).get_password().unwrap(), "new");
        assert!(matches!(entry(&format!("{key}#1")).get_password(), Err(keyring_core::Error::NoEntry)));
        assert_eq!(store.get(key).as_deref(), Some("new"));

        // Nothing kept under a key is nothing remembered, not trouble.
        assert_eq!(store.get("password:nobody@mailcraft.example"), None);
        store.delete(key);
        assert_eq!(store.get(key), None);
        assert_eq!(store.take_trouble(), None);
    }

    /// A store that couldn't be reached is tried again the next time, so that a Secret Service
    /// started after the window is used without starting the window again.
    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd"))]
    fn a_store_that_couldnt_be_reached_is_tried_again() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use keyring_core::{CredentialStore, Error, mock};
        use mailcraft_ui_egui::StoreTrouble;
        let mock: Arc<CredentialStore> = mock::Store::new().unwrap();
        let tries = Arc::new(AtomicUsize::new(0));
        let store = {
            let (mock, tries) = (mock.clone(), tries.clone());
            os::OsStore::with(Box::new(move || {
                if tries.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(Error::PlatformFailure(Box::new(std::io::Error::other("ServiceUnknown"))))
                } else {
                    Ok(mock.clone())
                }
            }))
        };
        assert_eq!(store.get("password:a@mailcraft.example"), None);
        assert_eq!(store.take_trouble(), Some(StoreTrouble::Unavailable));
        store.set("password:a@mailcraft.example", "secret").unwrap();
        assert_eq!(store.get("password:a@mailcraft.example").as_deref(), Some("secret"));
        assert_eq!(tries.load(Ordering::SeqCst), 2, "opened once more, then kept");
        assert_eq!(store.take_trouble(), None);
    }

    /// Against the real store of the machine running the test, with a made-up key that is removed
    /// again. Run it by name: `cargo test -p mailcraft -- --ignored the_systems_store`. On Linux it
    /// needs a Secret Service running and unlocked (GNOME Keyring, KWallet); without one, the
    /// store's trouble says so.
    #[test]
    #[ignore = "writes to (and cleans up in) this user's credential store"]
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd"))]
    fn the_systems_store_keeps_a_long_secret_and_lets_it_go() {
        let store = store().unwrap();
        let key = "refresh:store-test@mailcraft.example";
        // Longer than one Windows credential holds, like a real refresh token.
        let secret: String = "0.AXoA-made_up.token~".chars().cycle().take(3_300).collect();
        store.delete(key);
        assert_eq!(store.get(key), None);
        assert_eq!(store.take_trouble(), None, "the store can't be used here");
        store.set(key, &secret).unwrap();
        assert_eq!(store.get(key).as_deref(), Some(secret.as_str()));
        // A shorter one over it leaves nothing of the longer one behind.
        store.set(key, "short").unwrap();
        assert_eq!(store.get(key).as_deref(), Some("short"));
        store.delete(key);
        assert_eq!(store.get(key), None);
        store.delete(key);
        assert_eq!(store.take_trouble(), None);
    }
}
