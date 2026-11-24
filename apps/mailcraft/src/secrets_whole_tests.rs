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

use super::os::{Layout, NOTED, OsStore, SERVICE, WHOLE, Writes};
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
    /// When set, every removal of an item that is there fails as a locked keychain does.
    stuck: bool,
    /// Done once, just after the item of this name is read (another thread's write or delete
    /// landing in the middle of a read).
    on_read: Option<(String, Box<dyn FnOnce() + Send>)>,
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
        // (Taken out first: what it does goes through the store, and so through these counts.)
        let hook = {
            let mut asked = self.asked.lock().unwrap();
            match asked.on_read.take() {
                Some((name, hook)) if name == self.user => Some(hook),
                other => {
                    asked.on_read = other;
                    None
                }
            }
        };
        if let Some(hook) = hook {
            hook();
        }
        got
    }

    fn delete_credential(&self) -> keyring_core::Result<()> {
        if self.asked.lock().unwrap().stuck && self.there() {
            return Err(Error::NoStorageAccess(Box::new(std::io::Error::other("locked"))));
        }
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

/// Leftover pieces the keychain wouldn't remove (locked at that moment) are removed with a later
/// write, not noted as gone and left behind.
#[test]
fn pieces_that_couldnt_be_removed_are_tried_again_with_the_next_write() {
    let (store, mock, asked) = backend();
    let key = "refresh:one@example.org|graph";
    put(&mock, &format!("{key}#n"), "1");
    put(&mock, &format!("{key}#0"), "old-token-piece");

    asked.lock().unwrap().stuck = true;
    // The new token is kept; only the old piece couldn't go yet.
    store.set(key, "new-one").unwrap();
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some("new-one"));
    assert_eq!(there(&mock, &format!("{key}#0")).as_deref(), Some("old-token-piece"));
    assert_eq!(store.take_trouble(), Some(StoreTrouble::Locked));

    asked.lock().unwrap().stuck = false;
    store.set(key, "new-two").unwrap();
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some("new-two"));
    for part in ["n", "0"] {
        assert_eq!(there(&mock, &format!("{key}#{part}")), None, "piece #{part} removed");
    }
}

/// A store kept whole, shared as the window's thread and the background read share it, with a
/// counting mock under it.
fn shared() -> (Arc<OsStore>, Arc<CredentialStore>, Arc<Mutex<Asked>>) {
    let (store, mock, asked) = backend();
    (Arc::new(store), mock, asked)
}

/// The owner removes the account while the background read is moving its sign-in out of the old
/// pieces: the old token is not written back afterwards.
#[test]
fn a_delete_during_the_move_out_of_pieces_is_not_undone() {
    let (store, mock, asked) = shared();
    let key = "refresh:one@example.org|graph";
    put(&mock, &format!("{key}#n"), "1");
    put(&mock, &format!("{key}#0"), "old-token");
    let deleter = Arc::downgrade(&store);
    asked.lock().unwrap().on_read = Some((
        format!("{key}#0"),
        Box::new(move || {
            if let Some(store) = deleter.upgrade() {
                store.delete("refresh:one@example.org|graph");
            }
        }),
    ));

    // Nor is the removed token handed back, to be kept in memory as the current one.
    assert_eq!(store.get(key), None);
    for part in [WHOLE, "n", "0"] {
        assert_eq!(there(&mock, &format!("{key}#{part}")), None, "#{part} stays removed");
    }
    assert_eq!(store.get(key), None);
}

/// A new token written while the background read moves the old one out of its pieces stays: the
/// old one doesn't go over it.
#[test]
fn a_write_during_the_move_out_of_pieces_is_not_overwritten() {
    let (store, mock, asked) = shared();
    let key = "refresh:one@example.org|graph";
    put(&mock, &format!("{key}#n"), "1");
    put(&mock, &format!("{key}#0"), "old-token");
    let writer = Arc::downgrade(&store);
    asked.lock().unwrap().on_read = Some((
        format!("{key}#0"),
        Box::new(move || {
            if let Some(store) = writer.upgrade() {
                store.set("refresh:one@example.org|graph", "newer-token").unwrap();
            }
        }),
    ));

    // The read gives the newer token, not the older one it found in the pieces.
    assert_eq!(store.get(key).as_deref(), Some("newer-token"));
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some("newer-token"));
    assert_eq!(store.get(key).as_deref(), Some("newer-token"));
}

/// A new token written just after the read found no single item, whose sweep of the old pieces
/// failed (the keychain locked at that moment), is not overwritten by the move of those pieces.
#[test]
fn a_write_between_the_look_and_the_pieces_read_is_not_overwritten() {
    let (store, mock, asked) = shared();
    let key = "refresh:one@example.org|graph";
    put(&mock, &format!("{key}#n"), "1");
    put(&mock, &format!("{key}#0"), "old-token");
    let (writer, flags) = (Arc::downgrade(&store), asked.clone());
    asked.lock().unwrap().on_read = Some((
        format!("{key}#{WHOLE}"),
        Box::new(move || {
            if let Some(store) = writer.upgrade() {
                flags.lock().unwrap().stuck = true;
                store.set("refresh:one@example.org|graph", "newer-token").unwrap();
                flags.lock().unwrap().stuck = false;
            }
        }),
    ));

    assert_eq!(store.get(key).as_deref(), Some("newer-token"));
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some("newer-token"));
}

/// The owner removes the sign-in just after the read found no single item, and the sweep of the
/// old pieces fails (the keychain locked at that moment): the move doesn't bring the token back,
/// nor does the read hand it out.
#[test]
fn a_delete_between_the_look_and_the_pieces_read_is_not_undone() {
    let (store, mock, asked) = shared();
    let key = "refresh:one@example.org|graph";
    put(&mock, &format!("{key}#n"), "1");
    put(&mock, &format!("{key}#0"), "old-token");
    let (deleter, flags) = (Arc::downgrade(&store), asked.clone());
    asked.lock().unwrap().on_read = Some((
        format!("{key}#{WHOLE}"),
        Box::new(move || {
            if let Some(store) = deleter.upgrade() {
                flags.lock().unwrap().stuck = true;
                store.delete("refresh:one@example.org|graph");
                flags.lock().unwrap().stuck = false;
            }
        }),
    ));

    assert_eq!(store.get(key), None);
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")), None, "not written back");
}

/// Another account's new token written while this one's pieces are read doesn't stop this one's
/// move, nor make its sign-in read as absent.
#[test]
fn another_keys_write_during_the_move_out_of_pieces_lets_it_go_on() {
    let (store, mock, asked) = shared();
    let key = "refresh:one@example.org|graph";
    let other = "refresh:two@example.org|graph";
    put(&mock, &format!("{key}#n"), "1");
    put(&mock, &format!("{key}#0"), "one-token");
    let writer = Arc::downgrade(&store);
    asked.lock().unwrap().on_read = Some((
        format!("{key}#0"),
        Box::new(move || {
            if let Some(store) = writer.upgrade() {
                store.set("refresh:two@example.org|graph", "two-token").unwrap();
            }
        }),
    ));

    assert_eq!(store.get(key).as_deref(), Some("one-token"));
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some("one-token"), "moved into one item");
    for part in ["n", "0"] {
        assert_eq!(there(&mock, &format!("{key}#{part}")), None, "piece #{part} removed");
    }
    assert_eq!(store.get(other).as_deref(), Some("two-token"));
}

/// Each key's writes are told apart, and past the keys that can be noted one by one, a write of
/// an unnoted key counts as a write of every key (the move is put off, never run over a write).
#[test]
fn writes_are_told_apart_by_key_and_too_many_keys_count_for_all() {
    let mut writes = Writes::default();
    writes.note("a");
    let before = 1;
    writes.note("b");
    assert!(!writes.changed_since("a", before), "b's write is not a's");
    assert!(writes.changed_since("b", before));
    writes.note("a");
    assert!(writes.changed_since("a", before));

    let mut full = Writes::default();
    for i in 0..NOTED {
        full.note(&format!("key-{i}"));
    }
    let before = NOTED as u64;
    assert!(!full.changed_since("key-0", before));
    full.note("one-too-many");
    assert!(full.changed_since("key-0", before), "an unnoted write may have been any key's");
    assert!(full.changed_since("never-written", before));
}

/// A new token written after the first of two old pieces is read sweeps the second before it is
/// read: the read gives the new token, not nothing.
#[test]
fn a_write_that_sweeps_a_piece_still_to_be_read_gives_the_new_secret() {
    let (store, mock, asked) = shared();
    let key = "refresh:one@example.org|graph";
    put(&mock, &format!("{key}#n"), "2");
    put(&mock, &format!("{key}#0"), "old-");
    put(&mock, &format!("{key}#1"), "token");
    let writer = Arc::downgrade(&store);
    asked.lock().unwrap().on_read = Some((
        format!("{key}#0"),
        Box::new(move || {
            if let Some(store) = writer.upgrade() {
                store.set("refresh:one@example.org|graph", "newer-token").unwrap();
            }
        }),
    ));

    assert_eq!(store.get(key).as_deref(), Some("newer-token"));
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")).as_deref(), Some("newer-token"));
    for part in ["n", "0", "1"] {
        assert_eq!(there(&mock, &format!("{key}#{part}")), None, "piece #{part} removed");
    }
}

/// With no write meanwhile, a piece that is missing still means nothing whole is kept.
#[test]
fn a_missing_piece_with_no_write_meanwhile_is_nothing() {
    let (store, mock, _) = shared();
    let key = "refresh:one@example.org|graph";
    put(&mock, &format!("{key}#n"), "2");
    put(&mock, &format!("{key}#0"), "half-");
    assert_eq!(store.get(key), None);
    assert_eq!(there(&mock, &format!("{key}#{WHOLE}")), None, "nothing half made is moved");
}
