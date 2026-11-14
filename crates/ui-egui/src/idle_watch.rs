// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! New mail the moment it arrives, on IMAP accounts (#276): while the window is open and
//! Send/Receive runs by itself, each account whose server can do IDLE has a connection kept open
//! to its own server, in its Inbox, by the host ([`Services::idle`]). When the server says there
//! is more mail, the account's Send/Receive is queued, quietly, as a change made here is: it is
//! cheap (the Inbox is read first, and folders that haven't changed are passed over), and it
//! brings the mail through the one way mail comes in.
//!
//! - Only accounts a plain mail-server connection serves, with something to sign in with that is
//!   already in memory (a password entered, a token still good): nothing is asked, no page is
//!   opened and the credential store is not read for this.
//! - Never while Send/Receive is off (`syncMinutes` 0, or working offline): an owner who stopped
//!   the timer is not listened for behind their back. Turning it off, removing the account or
//!   closing the mailbox stops the watch.
//! - A server that can't do IDLE, one that refuses the sign-in, and one whose connection keeps
//!   dropping are left alone for a while ([`retry_after`]); the timer brings their mail as ever.
//! - Nothing of a message passes through here: the watch hears a count and says "more mail".

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_model::sync::{IdleEnd, IdleNote, SyncRequest};
use mailcraft_model::{AccountKind, Id};

use crate::MailApp;

/// A watch the host runs: the flag that stops it, and what it says.
pub struct IdleWatch {
    pub stop: Arc<AtomicBool>,
    pub notes: Receiver<IdleNote>,
}

/// The watches under way, and the accounts left alone for a while.
#[derive(Default)]
pub(crate) struct IdleWatches {
    running: HashMap<Id, IdleWatch>,
    /// What each running watch was started with ([`MailApp::idle_key`]): when it changes, the
    /// watch is stopped and made anew.
    keys: HashMap<Id, u64>,
    /// When (the session's clock, seconds) an account may be watched again.
    retry: HashMap<Id, i64>,
}

impl IdleWatches {
    /// Stop every watch (another mailbox took the place of the open one, or the window closes).
    pub(crate) fn stop_all(&mut self) {
        for (_, watch) in self.running.drain() {
            watch.stop.store(true, Ordering::Relaxed);
        }
        self.keys.clear();
        self.retry.clear();
    }

    /// Whether a watch is under way for `account`.
    #[cfg(test)]
    pub(crate) fn watching(&self, account: Id) -> bool {
        self.running.contains_key(&account)
    }
}

impl Drop for IdleWatches {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// How long an account is left alone (seconds) after its watch ended for `end`.
pub(crate) fn retry_after(end: IdleEnd) -> Option<i64> {
    match end {
        // Asked to stop: nothing to retry; whoever stopped it decides.
        IdleEnd::Stopped => None,
        // A server's abilities change rarely.
        IdleEnd::NoIdle => Some(24 * 3600),
        // The same credentials would be refused again; a run renews or asks, and a new token
        // is a new request.
        IdleEnd::Refused => Some(10 * 60),
        IdleEnd::TooOften => Some(3600),
    }
}

impl MailApp {
    /// Every frame: start the watches that should be running, hear the ones that are, and queue
    /// the Send/Receive of an account that has new mail.
    pub(crate) fn idle_tick(&mut self) {
        let now = self.session.now();
        let wanted = self.idle_wanted();
        // Those no longer wanted stop: the timer was turned off, the account removed, the
        // mailbox changed.
        // Nor does one whose account was edited (server, security, user name, sign-in) or whose
        // secret has left memory (signed out, forgotten by the edit): it would go on signing in
        // to what the owner changed.
        let gone: Vec<Id> =
            self.idle.running.keys().copied().filter(|id| !wanted.contains(id) || self.idle.keys.get(id) != self.idle_key(*id).as_ref()).collect();
        for id in gone {
            if let Some(watch) = self.idle.running.remove(&id) {
                watch.stop.store(true, Ordering::Relaxed);
            }
            self.idle.keys.remove(&id);
        }
        // What the running ones say.
        let mut arrived = Vec::new();
        let mut ended: Vec<(Id, Option<IdleEnd>)> = Vec::new();
        for (id, watch) in &self.idle.running {
            loop {
                match watch.notes.try_recv() {
                    Ok(IdleNote::Arrived) => {
                        if !arrived.contains(id) {
                            arrived.push(*id);
                        }
                    }
                    Ok(IdleNote::Ended(end)) => {
                        ended.push((*id, Some(end)));
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    // The thread is gone without a word (it was never started, or it panicked).
                    Err(TryRecvError::Disconnected) => {
                        ended.push((*id, None));
                        break;
                    }
                }
            }
        }
        for (id, end) in ended {
            self.idle.running.remove(&id);
            self.idle.keys.remove(&id);
            if let Some(wait) = end.map_or(Some(600), retry_after) {
                self.idle.retry.insert(id, now.saturating_add(wait));
            }
        }
        // New mail: the account's run is queued, once.
        for id in arrived {
            // A run already waiting for this account must fetch, not only send what changed here.
            self.session.driver.round.push_only_next.remove(&id);
            if self.session.driver.round.is_queued(id) || !self.can_sync_quietly(id) {
                continue;
            }
            // A round that is under way keeps its rows: the account waits its turn in the queue.
            let begin = !self.session.sync_progress.running();
            self.session.driver.round.queue.push_back((id, true));
            if begin {
                let round = self.session.driver.round.round();
                self.session.sync_progress.begin(&round);
            }
        }
        // Start the ones that should be running and aren't.
        for id in wanted {
            if self.idle.running.contains_key(&id) || self.idle.retry.get(&id).is_some_and(|at| now < *at) {
                continue;
            }
            let (Some(request), Some(key)) = (self.idle_request(id), self.idle_key(id)) else { continue };
            if let Some(start) = &self.services.idle {
                let watch = start(request);
                self.idle.running.insert(id, watch);
                self.idle.keys.insert(id, key);
            }
        }
    }

    /// The accounts that are to be watched now.
    fn idle_wanted(&self) -> Vec<Id> {
        if self.services.idle.is_none()
            || !self.session.driver.auto_sync()
            || self.session.prefs.sync_minutes == 0
            || self.offline()
            || self.unconfirmed
        {
            return Vec::new();
        }
        self.session.mailbox.accounts.iter().filter(|a| a.kind == AccountKind::Imap && a.incoming.is_some() && !a.is_shared()).map(|a| a.id).collect()
    }

    /// What a watch of `account` is for: the server, the user name, the way of signing in and the
    /// secret that is in memory (a token's own text is left out: it is renewed every hour, and
    /// the connection it opened goes on). `None` while there is nothing to sign in with.
    fn idle_key(&self, account: Id) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let a = self.session.mailbox.account(account)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let server = a.incoming.as_ref()?;
        (&server.host, server.port, format!("{:?}", server.security), a.login(), a.oauth.is_some()).hash(&mut hasher);
        if a.oauth.is_some() {
            self.tokens.get(&account)?;
        } else {
            self.passwords.get(&account).filter(|p| !p.is_empty())?.hash(&mut hasher);
        }
        Some(hasher.finish())
    }

    /// What the watch of `account` signs in with: the password the owner entered, or a token
    /// that is still good for a while, both from memory. `None` while there is neither.
    fn idle_request(&self, account: Id) -> Option<SyncRequest> {
        let a = self.session.mailbox.account(account)?;
        let mut request = SyncRequest { account: a.clone(), ..Default::default() };
        if a.oauth.is_some() {
            let token = self.tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now() + 600))?;
            request.access_token = token.access.clone();
        } else {
            request.password = self.passwords.get(&account).filter(|p| !p.is_empty())?.clone();
        }
        Some(request)
    }
}

#[cfg(test)]
#[path = "idle_watch_tests.rs"]
mod tests;
