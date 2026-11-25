// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading a remembered calendar and contacts password where it is used (#209): by the DAV job,
//! before it starts, for the origins of an account whose servers have a password of their own.
//!
//! The store may be locked, and asking it then waits on the owner (`secret_read.rs`): so the
//! window never asks it, a thread of its own does. Each name is asked once in a session
//! ([`DavSecretRead::asked`]): a miss stays a miss until the settings name another (a user name
//! or certificate changed), so a locked store doesn't put its prompt in front of the owner at
//! every job. Nothing is asked while remembering is off, for a mailbox nobody has confirmed, after
//! Remove all data, or for a name with a forget pending. What comes back is taken only while the
//! settings still give its name and no forget is pending for it; it goes where the opening read's
//! answers go (`MailApp::secret_cache`), where `MailApp::dav_password_from` finds it.
//!
//! **Remembering.** A password of their own is written to the store only once a server took it
//! (`MailApp::remember_dav_password`, from the job's answer), as a mail password is (#802): one
//! that was only typed, or that a server refused, is never kept. Only while the owner lets
//! sign-ins be remembered, never for a name with a forget pending, and the name is noted so that
//! Remove all data finds it. The account's mail password is never written under a DAV name.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver};

use crate::{Id, MailApp, StoreTrouble};

/// What one read brings: each name asked, with what was found under it, and why the store
/// couldn't be used, when it said.
type Brought = (Vec<(String, Option<String>)>, Option<StoreTrouble>);

/// Most names one read asks for: a mailbox's accounts and their origins are few, and a file can't
/// make a read grow.
const MOST: usize = 64;

/// What the DAV job remembers of its reads of the store, for one opening of the mailbox.
#[derive(Default)]
pub(crate) struct DavSecretRead {
    /// The names asked this session, whatever came back.
    asked: HashSet<String>,
    /// The read under way: what it brings, name by name.
    reading: Option<Receiver<Brought>>,
    /// The names a forget started for while the read was under way: what it brings for them is
    /// from before, and isn't taken in.
    pub(crate) forgotten: HashSet<String>,
}

impl DavSecretRead {
    /// Whether a read of the store is under way.
    pub(crate) fn is_reading(&self) -> bool {
        self.reading.is_some()
    }
}

impl MailApp {
    /// The names the job would need from the store now: for each account whose servers have a
    /// password of their own, each origin of its homes that has none in this session.
    fn dav_names_to_read(&self) -> Vec<String> {
        let mut names = Vec::new();
        for a in &self.session.mailbox.accounts {
            let Some(server) = a.dav.as_ref().filter(|d| d.separate_password) else { continue };
            for origin in server.home_origins() {
                if self.dav_passwords.contains_key(&(a.id, origin.clone())) {
                    continue;
                }
                if let Some(key) = Self::dav_secret_key(a, &origin)
                    && !self.secret_cache.contains_key(&key)
                    && !self.dav.secrets.asked.contains(&key)
                    && !self.dav_forget_pending(&key)
                    && !names.contains(&key)
                {
                    names.push(key);
                }
            }
        }
        names.truncate(MOST);
        names
    }

    /// Before a job: take in a read of the store that came back, and start one for the names the
    /// job would need. Whether the job is to wait (a read is under way).
    pub(crate) fn dav_secrets_first(&mut self) -> bool {
        if let Some(receive) = &self.dav.secrets.reading {
            let (answers, trouble) = match receive.try_recv() {
                Ok(brought) => brought,
                Err(mpsc::TryRecvError::Empty) => return true,
                // (The thread ended without an answer: nothing was found.)
                Err(mpsc::TryRecvError::Disconnected) => (Vec::new(), None),
            };
            self.dav.secrets.reading = None;
            self.take_in_dav_secrets(answers);
            if let Some(trouble) = trouble {
                self.note_secret_trouble(trouble);
            }
        }
        if !self.ui.remember_sign_in || self.unconfirmed || self.data_removed {
            return false;
        }
        let Some(store) = self.services.secrets.clone() else { return false };
        let names = self.dav_names_to_read();
        if names.is_empty() {
            return false;
        }
        self.dav.secrets.asked.extend(names.iter().cloned());
        let (send, receive) = mpsc::channel();
        let job = std::thread::Builder::new().name("dav-secret-read".into()).spawn(move || {
            let answers = names.into_iter().map(|key| (key.clone(), store.get(&key).filter(|s| !s.is_empty()))).collect();
            let _ = send.send((answers, store.take_trouble()));
        });
        match job {
            Ok(_) => {
                self.dav.secrets.reading = Some(receive);
                true
            }
            Err(e) => {
                log::warn!("couldn't start reading the credential store: {e}");
                false
            }
        }
    }

    /// Take in what the store gave, for the names the settings still give, no forget is pending
    /// for, and none started for since the read did.
    fn take_in_dav_secrets(&mut self, answers: Vec<(String, Option<String>)>) {
        let now: HashSet<String> = self.session.mailbox.accounts.iter().flat_map(Self::dav_secret_keys).collect();
        let forgotten = std::mem::take(&mut self.dav.secrets.forgotten);
        for (key, secret) in answers {
            if !now.contains(&key) || self.dav_forget_pending(&key) || forgotten.contains(&key) {
                continue;
            }
            if let Some(secret) = secret {
                // Noted, as one the opening read brings is, so that Remove all data finds it.
                self.index_secret_key(key.clone());
                self.secret_cache.insert(key, secret);
            }
        }
    }

    /// Have the store keep the password a server of the account took for `origin`, when the owner
    /// lets sign-ins be remembered (File › Options). Nothing when it holds that one already.
    pub(crate) fn remember_dav_password(&mut self, account: Id, origin: &str, password: &str) {
        if !self.ui.remember_sign_in || password.is_empty() || self.data_removed || self.unconfirmed {
            return;
        }
        let Some(key) = self.session.mailbox.account(account).and_then(|a| Self::dav_secret_key(a, origin)) else { return };
        if self.dav_forget_pending(&key) || self.secret_cache.get(&key).is_some_and(|kept| kept == password) {
            return;
        }
        let Some(store) = self.services.secrets.clone() else { return };
        match store.set(&key, password) {
            Ok(()) => {
                self.secret_cache.insert(key.clone(), password.to_string());
                self.index_secret_key(key);
            }
            Err(_) => {
                // A store error can echo its input; never put it in diagnostics. A partial write
                // must not leave a password that seems kept.
                log::warn!("couldn't keep a calendar server's password in the operating system's credential store");
                store.delete(&key);
                self.secret_cache.remove(&key);
            }
        }
    }
}
