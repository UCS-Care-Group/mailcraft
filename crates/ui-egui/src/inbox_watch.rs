//! New mail heard of as it comes, between two Send/Receives: for an account whose server can
//! say so (IMAP IDLE), the host keeps one more connection waiting on the Inbox
//! ([`Services::idle`], `mailcraft_net::idle` on the desktop), and when it hears that something
//! came or went there, the Inbox alone is looked at ([`crate::inbox_looks`]): quietly, with the
//! sign-in at hand, and without a run over every folder of the account, which the timed round
//! does. While a run of the account is at it (or waits its turn) the word is left to the run,
//! which glances at its Inbox every minute itself: what the run pushes shows on the watch's
//! connection too, and must not start sweep after sweep.
//!
//! A watch begins once a run of the account has gone through (its sign-in is known to be good
//! then), ends with the app, when the account goes, and when Send/Receive by itself is turned off;
//! one whose server doesn't offer IDLE is not asked for again this session, and one whose
//! sign-in stopped working begins again after the next run that signs in anew.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_model::Id;
use mailcraft_model::sync::SyncRequest;

use crate::MailApp;
#[cfg(test)]
use crate::Services;

/// What a watch on an Inbox says ([`InboxWatch::news`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchNews {
    /// Something came, went or changed in the Inbox: time for a run.
    Changed,
    /// The server doesn't offer IDLE: its new mail comes with the timed runs.
    NotOffered,
    /// The server no longer takes the sign-in; a watch begins again after a run that signs in.
    SignIn,
    /// It ended for another reason (said for the log).
    Ended(String),
}

/// A watch the host keeps on an account's Inbox. Dropping it tells the host to end it.
pub struct InboxWatch {
    pub news: Receiver<WatchNews>,
    /// Set to end the watch; the host looks at it every second or so.
    pub stop: Arc<AtomicBool>,
}

impl Drop for InboxWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The watches of the accounts, and what is known about watching them.
#[derive(Default)]
pub(crate) struct Watches {
    live: HashMap<Id, InboxWatch>,
    /// Accounts whose server doesn't offer IDLE.
    off: HashSet<Id>,
    /// Accounts whose last run went through, with what it signed in with (in memory only): a
    /// watch may begin with it.
    ready: HashMap<Id, SyncRequest>,
}

impl Watches {
    /// Whether `account` is being watched.
    #[cfg(test)]
    pub(crate) fn watching(&self, account: Id) -> bool {
        self.live.contains_key(&account)
    }
}

/// Only what a watch needs of a run's request: the account and how it signs in.
pub(crate) fn sign_in_of(request: &SyncRequest) -> SyncRequest {
    SyncRequest {
        account: request.account.clone(),
        password: request.password.clone(),
        access_token: request.access_token.clone(),
        send_login: request.send_login.clone(),
        ..Default::default()
    }
}

impl MailApp {
    /// A run of `account` went through with this sign-in: a watch on its Inbox may begin.
    pub(crate) fn sign_in_worked(&mut self, account: Id, request: SyncRequest) {
        // (Mail read through Microsoft Graph has no IMAP connection to wait on: its runs come by
        // the clock.)
        if self.services.idle.is_some() && request.account.mail_source != mailcraft_model::MailSource::Graph {
            self.watches.ready.insert(account, request);
        }
    }

    /// Every frame: hear what the watches say, start a run for an Inbox that changed, begin the
    /// watches that may, and end those that must.
    pub(crate) fn poll_watches(&mut self) {
        if !self.auto_sync || self.services.idle.is_none() {
            self.watches.live.clear();
            self.watches.ready.clear();
            return;
        }
        let accounts: HashSet<Id> = self.server_accounts().into_iter().collect();
        // The account went: so does its watch.
        self.watches.live.retain(|id, _| accounts.contains(id));
        self.watches.ready.retain(|id, _| accounts.contains(id));
        let mut changed: Vec<Id> = Vec::new();
        let mut ended: Vec<Id> = Vec::new();
        for (id, watch) in &self.watches.live {
            // Only so much a frame: a server that talks without end doesn't hold the window up.
            for _ in 0..64 {
                match watch.news.try_recv() {
                    Ok(WatchNews::Changed) => {
                        if !changed.contains(id) {
                            changed.push(*id);
                        }
                    }
                    Ok(WatchNews::NotOffered) => {
                        self.watches.off.insert(*id);
                        ended.push(*id);
                        break;
                    }
                    Ok(WatchNews::SignIn) => {
                        ended.push(*id);
                        break;
                    }
                    Ok(WatchNews::Ended(why)) => {
                        log::debug!("waiting for new mail ended: {why}");
                        ended.push(*id);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        ended.push(*id);
                        break;
                    }
                }
            }
        }
        for id in ended {
            self.watches.live.remove(&id);
        }
        let running = self.sync_run.map(|(id, ..)| id).filter(|_| self.sync_rx.is_some());
        for id in changed {
            // A run of the account at it, or waiting its turn, brings its new mail itself.
            if running == Some(id) || self.sync_queue.iter().any(|job| job.account == id) {
                continue;
            }
            self.want_inbox_look(id);
        }
        let ready: Vec<(Id, SyncRequest)> = self.watches.ready.drain().collect();
        for (id, request) in ready {
            if self.watches.off.contains(&id) || self.watches.live.contains_key(&id) {
                continue;
            }
            if let Some(start) = &self.services.idle {
                self.watches.live.insert(id, start(request));
            }
        }
    }
}

/// For the tests: a host whose watches are channels the test talks into.
#[cfg(test)]
pub(crate) fn test_host(services: &mut Services) -> std::sync::Arc<std::sync::Mutex<Vec<(Id, std::sync::mpsc::Sender<WatchNews>, Arc<AtomicBool>)>>> {
    let started: std::sync::Arc<std::sync::Mutex<Vec<(Id, std::sync::mpsc::Sender<WatchNews>, Arc<AtomicBool>)>>> = Default::default();
    let kept = started.clone();
    services.idle = Some(Box::new(move |request: SyncRequest| {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        if let Ok(mut list) = kept.lock() {
            list.push((request.account.id, tx, stop.clone()));
        }
        InboxWatch { news: rx, stop }
    }));
    started
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    #[test]
    fn news_on_a_watched_inbox_is_answered_with_a_look_at_the_inbox_and_a_watch_ends_with_its_account() {
        let mut services = Services::default();
        let started = test_host(&mut services);
        // A host whose runs are recorded: whose, and whether a look at the Inbox alone. A look
        // is answered at once; a run stays at it.
        let runs: Arc<Mutex<Vec<(Id, bool)>>> = Default::default();
        let open: Arc<Mutex<Vec<std::sync::mpsc::Sender<mailcraft_model::sync::SyncResult>>>> = Default::default();
        let (keep, hold) = (runs.clone(), open.clone());
        services.sync = Some(Box::new(move |request: SyncRequest| {
            // (A look reads the Inbox and the folders under it, and pushes nothing.)
            assert!(!request.inbox_only || (request.scope == mailcraft_model::sync::SyncScope::InboxTree && request.ops.is_empty()));
            keep.lock().unwrap().push((request.account.id, request.inbox_only));
            let (tx, rx) = std::sync::mpsc::channel();
            if request.inbox_only {
                let answer =
                    mailcraft_model::sync::SyncResult { account: request.account.id, email: request.account.email.clone(), ..Default::default() };
                let _ = tx.send(answer);
            }
            hold.lock().unwrap().push(tx);
            rx
        }));
        let mut app = MailApp::new(mailcraft_engine::Session::empty(), services);
        let servers = json!({"email": "ada@example.org", "incoming": {"host": "127.0.0.1", "port": 1, "security": "none"}});
        let account = app.session.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
        app.passwords.insert(account, "hunter2".into());
        // Nothing is watched before a run has gone through.
        app.poll_watches();
        assert!(started.lock().unwrap().is_empty());
        let request = SyncRequest { password: "hunter2".into(), ..Default::default() };
        let mut request = sign_in_of(&request);
        request.account.id = account;
        app.sign_in_worked(account, request);
        app.poll_watches();
        assert!(app.watches.watching(account));
        let (tx, stop) = {
            let list = started.lock().unwrap();
            assert_eq!(list.len(), 1);
            (list[0].1.clone(), list[0].2.clone())
        };
        app.sync_queue.clear();
        // Word of a change: the Inbox alone is looked at, once, and no run over every folder is
        // queued for it.
        tx.send(WatchNews::Changed).unwrap();
        tx.send(WatchNews::Changed).unwrap();
        app.poll_watches();
        app.poll_inbox_looks(1_000.0);
        app.poll_inbox_looks(1_001.0);
        assert_eq!(runs.lock().unwrap().clone(), [(account, true)]);
        assert!(app.sync_queue.is_empty());
        // While a run of the account is at it, the word is the run's: it glances at the Inbox itself.
        app.execute("ui.sync", json!({"account": account})).unwrap();
        assert!(app.syncing());
        tx.send(WatchNews::Changed).unwrap();
        app.poll_watches();
        app.poll_inbox_looks(2_000.0);
        assert_eq!(runs.lock().unwrap().clone(), [(account, true), (account, false)]);
        assert!(app.sync_queue.is_empty());
        // The run over, word is answered with a look again.
        open.lock().unwrap().clear();
        app.poll_sync();
        assert!(!app.syncing());
        tx.send(WatchNews::Changed).unwrap();
        app.poll_watches();
        app.poll_inbox_looks(3_000.0);
        assert_eq!(runs.lock().unwrap().len(), 3);
        assert_eq!(runs.lock().unwrap()[2], (account, true));
        // A server without IDLE is not asked again.
        tx.send(WatchNews::NotOffered).unwrap();
        app.poll_watches();
        assert!(!app.watches.watching(account) && stop.load(Ordering::Relaxed));
        app.sign_in_worked(account, SyncRequest::default());
        app.poll_watches();
        assert_eq!(started.lock().unwrap().len(), 1);
        // The account goes: so does a watch on it.
        app.watches.off.clear();
        app.sign_in_worked(account, SyncRequest::default());
        app.poll_watches();
        let stop = started.lock().unwrap()[1].2.clone();
        app.session.run("account.add", &json!({"email": "bob@example.org"})).unwrap();
        app.session.run("account.remove", &json!({"id": account})).unwrap();
        app.poll_watches();
        assert!(!app.watches.watching(account) && stop.load(Ordering::Relaxed));
    }
}
