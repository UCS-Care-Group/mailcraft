// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading the remembered sign-ins off the window's thread (#613).
//!
//! The system's credential store may be locked: asking it then puts its unlock prompt in front of
//! the owner, and the call waits until the prompt is answered (the macOS keychain, GNOME Keyring,
//! KWallet). On the window's own thread that is a window that can't be clicked, scrolled or
//! closed, and a Cancel brings the prompt straight back because the next check asks again.
//!
//! So the window never calls [`SecretStore::get`]. When the mailbox opens, one background thread
//! reads what is remembered for its accounts, and the answers are kept in memory
//! (`MailApp::secret_cache`: what was found; no entry is a miss, a refusal or an answer still to
//! come, which all mean "not ready to sync quietly"). The store is asked again only when the
//! owner acts: Send/Receive is held until it has answered ([`MailApp::hold_for_secrets`]), and a
//! secret written this session goes into the answers as it is written.
//!
//! The one-time move of secrets filed under their old names to the names bound to servers
//! (`ui.sign_ins_bound`) is done by the same thread, since it too reads the store.

use std::sync::mpsc;

use serde_json::Value;

use super::{Id, MailApp, SecretStore, StoreTrouble};

/// What one background read of the store brought back.
pub(crate) struct SecretRead {
    /// Each key asked for, with what was found under it (nothing when it isn't there or the
    /// store refused).
    answers: Vec<(String, Option<String>)>,
    /// The new names of secrets moved from their old names.
    moved: Vec<String>,
    /// Whether the move from the old names was part of it.
    migrated: bool,
    /// Why the store couldn't be used, when it said.
    trouble: Option<StoreTrouble>,
}

/// One key to read, and the old name to move a secret from first.
struct Ask {
    key: String,
    old: Option<String>,
}

/// The reading itself; runs on a thread of its own, and may wait on the store for a long time.
fn read(store: &dyn SecretStore, asks: Vec<Ask>, migrated: bool) -> SecretRead {
    let (mut answers, mut moved) = (Vec::new(), Vec::new());
    for Ask { key, old } in asks {
        if let Some(old) = old
            && let Some(secret) = store.get(&old).filter(|s| !s.is_empty())
        {
            match store.set(&key, &secret) {
                Ok(()) => {
                    store.delete(&old);
                    moved.push(key.clone());
                }
                Err(e) => log::warn!("couldn't move a remembered sign-in: {e}"),
            }
        }
        let secret = store.get(&key).filter(|s| !s.is_empty());
        answers.push((key, secret));
    }
    SecretRead { answers, moved, migrated, trouble: store.take_trouble() }
}

impl MailApp {
    /// Start reading the store for this mailbox's accounts, unless a read is under way.
    /// `migrate`: move secrets kept under their old names first.
    pub(crate) fn start_secret_read(&mut self, migrate: bool) {
        if self.secret_read.is_some() {
            return;
        }
        let Some(store) = self.services.secrets.clone() else { return };
        let asks: Vec<Ask> = self
            .session
            .mailbox
            .accounts
            .iter()
            .flat_map(|a| {
                ["password", "refresh"]
                    .map(|kind| Ask { key: Self::secret_key(kind, a), old: migrate.then(|| Self::legacy_secret_key(kind, &a.email)) })
            })
            .collect();
        let (send, receive) = mpsc::channel();
        let job = std::thread::Builder::new().name("secret-read".into()).spawn(move || {
            let _ = send.send(read(&*store, asks, migrate));
        });
        match job {
            Ok(_) => self.secret_read = Some((self.session.openings(), receive)),
            Err(e) => log::warn!("couldn't start reading the credential store: {e}"),
        }
    }

    /// Once a frame: take in what the read brought, and start the read for a mailbox that has
    /// not had one (and again when remembering is switched on).
    pub(crate) fn secrets_tick(&mut self) {
        self.poll_secret_read();
        if self.secret_read.is_some() || self.data_removed || self.unconfirmed || self.services.secrets.is_none() {
            return;
        }
        let (opening, remember) = (self.session.openings(), self.ui.remember_sign_in);
        // The mailbox the window started with: secrets kept the old way are filed under its servers.
        let migrate = opening == 0 && !self.ui.sign_ins_bound;
        if (remember || migrate) && self.secrets_asked != Some((opening, remember)) {
            self.secrets_asked = Some((opening, remember));
            self.start_secret_read(migrate);
        }
    }

    pub(crate) fn poll_secret_read(&mut self) {
        let Some((opening, receive)) = &self.secret_read else { return };
        let opening = *opening;
        let brought = match receive.try_recv() {
            Ok(read) => Some(read),
            Err(mpsc::TryRecvError::Empty) => return,
            // The thread ended without an answer (the store panicked): nothing was found.
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        self.secret_read = None;
        // A mailbox took the place of the one it was started for: its accounts are not these.
        if opening == self.session.openings()
            && let Some(brought) = brought
        {
            for (key, secret) in brought.answers {
                if let Some(secret) = secret {
                    self.secret_cache.insert(key, secret);
                }
            }
            for key in brought.moved {
                self.index_secret_key(key);
            }
            if brought.migrated {
                self.ui.sign_ins_bound = true;
            }
            if let Some(trouble) = brought.trouble {
                self.note_secret_trouble(trouble);
            }
        }
        // The Send/Receive the owner pressed while it was out goes on, with the answer in.
        for mut p in std::mem::take(&mut self.sync_after_secrets) {
            if let Some(object) = p.as_object_mut() {
                object.insert("secretsAsked".into(), Value::Bool(true));
            }
            if let Err(e) = self.start_sync(&p) {
                self.status(e);
            }
        }
    }

    /// Whether the store could still say something about what the account needs to sign in that
    /// this session doesn't have: remembered sign-ins are on, and neither memory nor the answers
    /// so far hold its secret.
    fn store_may_know(&self, account: Id) -> bool {
        if !self.ui.remember_sign_in || self.services.secrets.is_none() {
            return false;
        }
        let account = self.session.mailbox.owner_of(account).map_or(account, |owner| owner.id);
        let Some(a) = self.session.mailbox.account(account) else { return false };
        // (A JMAP account's secret is never with the store, and an Exchange server account's is
        // not yet: nothing of it is kept before it is synchronised, #252.)
        if matches!(a.kind, mailcraft_model::AccountKind::Jmap | mailcraft_model::AccountKind::Exchange) {
            return false;
        }
        if a.oauth.is_some() {
            let kept = self.tokens.get(&account).or(self.graph_tokens.get(&account)).is_some_and(|t| !t.refresh.is_empty());
            !kept && !self.secret_cache.contains_key(&Self::secret_key("refresh", a))
        } else {
            !self.passwords.contains_key(&account) && !self.secret_cache.contains_key(&Self::secret_key("password", a))
        }
    }

    /// Send/Receive started by the owner: when the store might know a sign-in nothing here has,
    /// ask it again (this is the owner's act: a locked store may ask to be unlocked) and hold the
    /// run until it has answered. Gives whether the run was held.
    pub(crate) fn hold_for_secrets(&mut self, p: &Value) -> bool {
        if p.get("secretsAsked").and_then(Value::as_bool).unwrap_or(false) || p.get("password").is_some() {
            return false;
        }
        let accounts = match p.get("account").and_then(Value::as_u64) {
            Some(account) => vec![account],
            None => self.server_accounts(),
        };
        if !accounts.into_iter().any(|a| self.store_may_know(a)) {
            return false;
        }
        self.start_secret_read(false);
        // (Few: a button pressed again and again holds one run, not a queue of them.)
        if self.secret_read.is_none() || self.sync_after_secrets.len() >= 4 {
            return false;
        }
        self.sync_after_secrets.push(p.clone());
        true
    }
}
