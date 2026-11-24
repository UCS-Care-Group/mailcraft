// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A secret kept whole, as in the macOS keychain: one item per secret, so that a build the
//! keychain doesn't yet trust asks the owner about one item, not about each piece and the count;
//! pieces from earlier versions read once and moved; and writing a secret again (a refresh token,
//! after each refresh) reading nothing. In keyring's mock store, behind a wrapper that counts what
//! the keychain would ask about: reading an item that is there, or writing over one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use keyring_core::api::{CredentialApi, CredentialStoreApi};
use keyring_core::{Credential, CredentialStore, Entry, Error, mock};
use mailcraft_host::{SecretError, SecretStore, StoreTrouble};

use super::os::{Layout, OsStore, SERVICE, WHOLE};
use super::{MAX_PIECES, PIECE};

/// What the store was asked, by item name.
#[derive(Default)]
struct Asked {
    /// Reads of an item that was there (each one a keychain prompt for an untrusted build).
    reads: Vec<String>,
    /// Writes over an item that was there (also a prompt: the keychain reads it first).
    overwrites: Vec<String>,
    /// Items removed.
    deletes: Vec<String>,
    /// When set, every read fails as a locked keychain does.
    locked: bool,
}

struct Counting {
    mock: Arc<CredentialStore>,
    asked: Arc<Mutex<Asked>>,
}

struct Counted {
    entry: Entry,
    user: String,
    asked: Arc<Mutex<Asked>>,
}

impl Counted {
    fn there(&self) -> bool {
        self.entry.get_secret().is_ok()
    }
}

impl CredentialApi for Counted {
    fn set_secret(&self, secret: &[u8]) -> keyring_core::Result<()> {
        if self.there() {
            self.asked.lock().unwrap().overwrites.push(self.user.clone());
        }
        self.entry.set_secret(secret)
    }

    fn get_secret(&self) -> keyring_core::Result<Vec<u8>> {
        if self.asked.lock().unwrap().locked {
            return Err(Error::NoStorageAccess(Box::new(std::io::Error::other("locked"))));
        }
        let got = self.entry.get_secret();
        if got.is_ok() {
            self.asked.lock().unwrap().reads.push(self.user.clone());
        }
        got
    }

    fn delete_credential(&self) -> keyring_core::Result<()> {
        let done = self.entry.delete_credential();
        if done.is_ok() {
            self.asked.lock().unwrap().deletes.push(self.user.clone());
        }
        done
    }

    fn get_credential(&self) -> keyring_core::Result<Option<Arc<Credential>>> {
        Ok(None)
    }

    fn get_specifiers(&self) -> Option<(String, String)> {
        Some((SERVICE.to_string(), self.user.clone()))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl CredentialStoreApi for Counting {
    fn vendor(&self) -> String {
        "counting mock".into()
    }

    fn id(&self) -> String {
        "counting".into()
    }

    fn build(&self, service: &str, user: &str, mods: Option<&HashMap<&str, &str>>) -> keyring_core::Result<Entry> {
        let entry = self.mock.build(service, user, mods)?;
        let counted: Arc<Credential> = Arc::new(Counted { entry, user: user.to_string(), asked: self.asked.clone() });
        Ok(Entry::new_with_credential(counted))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A store kept whole over a counting mock, the mock itself (to look in, uncounted), and the counts.
fn backend() -> (OsStore, Arc<CredentialStore>, Arc<Mutex<Asked>>) {
    let mock: Arc<CredentialStore> = mock::Store::new().unwrap();
    let asked = Arc::new(Mutex::new(Asked::default()));
    let counting: Arc<CredentialStore> = Arc::new(Counting { mock: mock.clone(), asked: asked.clone() });
    (OsStore::with_layout(Box::new(move || Ok(counting.clone())), Layout::Whole), mock, asked)
}

fn there(mock: &Arc<CredentialStore>, name: &str) -> Option<String> {
    mock.build(SERVICE, name, None).unwrap().get_password().ok()
}

fn put(mock: &Arc<CredentialStore>, name: &str, value: &str) {
    mock.build(SERVICE, name, None).unwrap().set_password(value).unwrap();
}

fn forget_counts(asked: &Arc<Mutex<Asked>>) {
    let mut asked = asked.lock().unwrap();
    asked.reads.clear();
    asked.overwrites.clear();
    asked.deletes.clear();
}

/// As long as a real refresh token: three pieces in the old layout.
fn token() -> String {
    "0.AXoA-made_up.token~".chars().cycle().take(PIECE * 2 + 300).collect()
}

#[test]
fn a_long_secret_is_one_item_and_reading_it_asks_once() {
    let (store, mock, asked) = backend();
    let key = "refresh:one@example.org|graph";
    let secret = token();
    store.set(key, &secret).unwrap();
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some(secret.as_str()));
    for part in ["n", "0", "1"] {
        assert_eq!(there(&mock, &format!("{key}#{part}")), None, "no piece #{part}");
    }

    forget_counts(&asked);
    assert_eq!(store.get(key).as_deref(), Some(secret.as_str()));
    assert_eq!(asked.lock().unwrap().reads, [format!("{key}#{WHOLE}")], "one item read, one question");
}

#[test]
fn pieces_an_earlier_version_kept_are_read_once_and_moved_into_one_item() {
    let (store, mock, asked) = backend();
    let key = "refresh:one@example.org|graph";
    put(&mock, &format!("{key}#n"), "2");
    put(&mock, &format!("{key}#0"), "first-");
    put(&mock, &format!("{key}#1"), "second");

    assert_eq!(store.get(key).as_deref(), Some("first-second"));
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some("first-second"));
    for part in ["n", "0", "1"] {
        assert_eq!(there(&mock, &format!("{key}#{part}")), None, "piece #{part} removed");
    }

    // The next start asks about one item.
    forget_counts(&asked);
    let again = OsStore::with_layout(
        {
            let counting: Arc<CredentialStore> = Arc::new(Counting { mock: mock.clone(), asked: asked.clone() });
            Box::new(move || Ok(counting.clone()))
        },
        Layout::Whole,
    );
    assert_eq!(again.get(key).as_deref(), Some("first-second"));
    assert_eq!(asked.lock().unwrap().reads, [format!("{key}#{WHOLE}")]);
}

#[test]
fn writing_a_new_refresh_token_reads_nothing_and_overwrites_nothing() {
    let (store, mock, asked) = backend();
    let key = "refresh:one@example.org|graph";
    store.set(key, &token()).unwrap();

    // A refresh brings a new token, and another, and another.
    forget_counts(&asked);
    for turn in 0..3 {
        let next = format!("{}-{turn}", token());
        store.set(key, &next).unwrap();
        assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some(next.as_str()));
    }
    let asked = asked.lock().unwrap();
    assert!(asked.reads.is_empty(), "read: {:?}", asked.reads);
    assert!(asked.overwrites.is_empty(), "written over: {:?}", asked.overwrites);
    // Removed and added again each time; the old pieces were swept once, not each time.
    assert_eq!(asked.deletes, vec![format!("{key}#{WHOLE}"); 3]);
}

#[test]
fn an_item_an_earlier_build_made_is_replaced_without_being_read() {
    let (store, mock, asked) = backend();
    let key = "password:one@example.org|imap=imap.example.org:993|smtp=none";
    // Made by another build: reading or writing over it would ask for the keychain password.
    put(&mock, &format!("{key}#{WHOLE}"), "old-password");
    store.set(key, "new-password").unwrap();
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some("new-password"));
    let asked = asked.lock().unwrap();
    assert!(asked.reads.is_empty() && asked.overwrites.is_empty());
}

#[test]
fn deleting_removes_the_item_and_any_pieces_left_behind() {
    let (store, mock, _) = backend();
    let key = "refresh:one@example.org|graph";
    let other = "refresh:two@example.org|graph";
    store.set(key, "whole").unwrap();
    store.set(other, "someone-else").unwrap();
    // Pieces of an interrupted older write, with no count.
    put(&mock, &format!("{key}#0"), "stray-0");
    put(&mock, &format!("{key}#{}", MAX_PIECES - 1), "stray-last");

    store.delete(key);
    assert_eq!(store.get(key), None);
    for part in ["all".to_string(), "n".to_string(), "0".to_string(), (MAX_PIECES - 1).to_string()] {
        assert_eq!(there(&mock, &format!("{key}#{part}")), None, "#{part} removed");
    }
    assert_eq!(store.get(other).as_deref(), Some("someone-else"));
}

#[test]
fn an_empty_secret_forgets_and_a_too_long_one_is_refused_keeping_nothing() {
    let (store, mock, _) = backend();
    let key = "refresh:one@example.org|graph";
    store.set(key, "kept").unwrap();
    store.set(key, "").unwrap();
    assert_eq!(store.get(key), None);
    let long = "x".repeat(PIECE * MAX_PIECES + 1);
    assert_eq!(store.set(key, &long), Err(SecretError::TooLong));
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")), None);
}

#[test]
fn a_locked_keychain_is_trouble_and_no_piece_is_asked_for() {
    let (store, mock, asked) = backend();
    let key = "refresh:one@example.org|graph";
    put(&mock, &format!("{key}#n"), "1");
    put(&mock, &format!("{key}#0"), "old");
    asked.lock().unwrap().locked = true;
    assert_eq!(store.get(key), None);
    assert_eq!(store.take_trouble(), Some(StoreTrouble::Locked));
    // Nothing moved or removed while the keychain couldn't say what it holds.
    assert_eq!(there(&mock, &format!("{key}#0")).as_deref(), Some("old"));
    assert!(asked.lock().unwrap().deletes.is_empty());
}

#[test]
fn nothing_kept_is_nothing_and_no_trouble() {
    let (store, _, asked) = backend();
    assert_eq!(store.get("password:nobody@example.org"), None);
    assert_eq!(store.take_trouble(), None);
    assert!(asked.lock().unwrap().reads.is_empty());
}
