// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Send/Receive round's scheduler, with no I/O: which accounts wait their turn and in what
//! order, when a new round is due (the owner's `syncMinutes`), when something made here that
//! waits to go to a server starts a run of its own, and when a long run is called off so that
//! it can go out. A front end hands in the time (milliseconds) and what it knows of its accounts
//! and asks; the host runs what is decided, one account at a time.

use std::collections::{HashMap, HashSet, VecDeque};

use mailcraft_model::Id;

/// How often a run under way is looked at for something the owner did meanwhile.
pub const RUN_LOOK_MS: f64 = 3_000.0;
/// How often accounts are looked at for changes waiting to go.
pub const PUSH_LOOK_MS: f64 = 1_500.0;
/// How long what a server didn't take is left alone before it is tried again.
pub const PUSH_RETRY_MS: f64 = 300_000.0;

/// What a front end does next with the line of accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Nobody waits.
    Idle,
    /// The first in line must wait a moment (its calendar job is at work).
    Wait,
    /// Start this account's run; `quiet` says nobody asked for it.
    Start { account: Id, quiet: bool },
}

/// An account's changes waiting to go, as a look at the accounts finds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waiting {
    pub account: Id,
    /// How much waits (messages in the Outbox, changes the server hasn't been told).
    pub count: usize,
}

/// The round's state: in memory only, never saved.
#[derive(Debug, Default)]
pub struct RoundDriver {
    /// Accounts waiting their turn in a round, and whether nobody asked for it.
    pub queue: VecDeque<(Id, bool)>,
    /// Accounts whose Inbox pass is still to come at the start of a round, and whether nobody
    /// asked for it.
    pub inbox_queue: VecDeque<(Id, bool)>,
    /// When the last round that started by itself began (milliseconds; 0 before the first).
    pub last_auto: f64,
    /// When it was last looked whether something waits to go (milliseconds).
    pub push_checked: f64,
    /// Accounts whose next run that nobody asked for only tells the server what changed here.
    pub push_only_next: HashSet<Id>,
    /// Per account: how much was still waiting after its last run, and when that was.
    pub push_seen: HashMap<Id, (usize, f64)>,
}

impl RoundDriver {
    /// Another mailbox took the place of the open one: its accounts are not these. What the clock
    /// says of the last look is the window's and stays.
    pub fn reset(&mut self) {
        *self = RoundDriver { last_auto: self.last_auto, push_checked: self.push_checked, ..RoundDriver::default() };
    }

    /// Accounts were removed: they wait for nothing.
    pub fn forget_accounts(&mut self, gone: &HashSet<Id>) {
        self.queue.retain(|(a, _)| !gone.contains(a));
        self.inbox_queue.retain(|(a, _)| !gone.contains(a));
        self.push_only_next.retain(|a| !gone.contains(a));
        self.push_seen.retain(|a, _| !gone.contains(a));
    }

    /// Everything that waits its turn is let go of (a run stopped, a mailbox replaced).
    pub fn clear_lines(&mut self) {
        self.queue.clear();
        self.inbox_queue.clear();
    }

    /// Whether `account` waits its turn in the round.
    pub fn is_queued(&self, account: Id) -> bool {
        self.queue.iter().any(|(a, _)| *a == account)
    }

    /// Put `account` back first in line, as it was asked for, unless it is there.
    pub fn put_back(&mut self, account: Id, quiet: bool) {
        if !self.is_queued(account) {
            self.queue.push_front((account, quiet));
        }
    }

    /// A run under way is to be looked at for what the owner did meanwhile: true at most once in
    /// [`RUN_LOOK_MS`], and then the look is noted.
    pub fn run_look_due(&mut self, auto_sync: bool, now: f64) -> bool {
        if auto_sync && now - self.push_checked >= RUN_LOOK_MS {
            self.push_checked = now;
            return true;
        }
        false
    }

    /// The first account in line, unless it must wait (`blocked`: its calendar job is at work).
    pub fn next(&mut self, blocked: impl Fn(Id) -> bool) -> Next {
        match self.queue.front() {
            Some((account, _)) if blocked(*account) => Next::Wait,
            Some(_) => self.queue.pop_front().map_or(Next::Idle, |(account, quiet)| Next::Start { account, quiet }),
            None => Next::Idle,
        }
    }

    /// The first account whose Inbox pass is next, unless it must wait (`blocked`).
    pub fn inbox_front_blocked(&self, blocked: impl Fn(Id) -> bool) -> bool {
        self.inbox_queue.front().is_some_and(|(account, _)| blocked(*account))
    }

    /// Whether the accounts are to be looked at for changes waiting to go: true at most once in
    /// [`PUSH_LOOK_MS`], and then the look is noted.
    pub fn push_look_due(&mut self, auto_sync: bool, now: f64) -> bool {
        if auto_sync && now - self.push_checked >= PUSH_LOOK_MS {
            self.push_checked = now;
            return true;
        }
        false
    }

    /// Of the accounts with something waiting, those that start a run of their own: what waits is
    /// more or less than what the last run left (a refusal keeps waiting without more runs), or
    /// has waited [`PUSH_RETRY_MS`]. A front end then keeps those it can run without asking and
    /// puts them in line with [`RoundDriver::queue_pushes`].
    pub fn pushes_due(&self, waiting: &[Waiting], now: f64) -> Vec<Id> {
        waiting
            .iter()
            .filter(|w| {
                let (seen, at) = self.push_seen.get(&w.account).copied().unwrap_or((0, 0.0));
                w.count > 0 && (w.count != seen || now - at > PUSH_RETRY_MS)
            })
            .map(|w| w.account)
            .collect()
    }

    /// Put accounts in line to tell their servers what changed here, nobody having asked, and say
    /// the round they make up when the line is not empty.
    pub fn queue_pushes(&mut self, accounts: &[Id]) -> Option<Vec<Id>> {
        for account in accounts {
            self.queue.push_back((*account, true));
            self.push_only_next.insert(*account);
        }
        (!self.queue.is_empty()).then(|| self.queue.iter().map(|(a, _)| *a).collect())
    }

    /// Whether a new round is due at `now`: the owner's `minutes` (0 is never) have gone by since
    /// the last one, or none has been.
    pub fn round_due(&self, auto_sync: bool, minutes: f64, now: f64) -> bool {
        auto_sync && minutes != 0.0 && (self.last_auto == 0.0 || now - self.last_auto >= minutes * 60_000.0)
    }

    /// A round of `accounts`, in this order, begins at `now`, nobody having asked for it.
    pub fn begin_auto_round(&mut self, accounts: &[Id], now: f64) {
        self.last_auto = now;
        self.queue = accounts.iter().map(|a| (*a, true)).collect();
    }

    /// A round the owner asked for: `accounts` in this order, `quiet` as the caller says.
    pub fn begin_round(&mut self, accounts: &[Id], quiet: bool) {
        self.queue = accounts.iter().map(|a| (*a, quiet)).collect();
    }

    /// A run of `account` is over: `count` still waits to go to its server, seen at `now`.
    pub fn note_pushed(&mut self, account: Id, count: usize, now: f64) {
        self.push_seen.insert(account, (count, now));
    }

    /// What waits to go from `account` is not what its last run left undone: the next look starts
    /// a run for it without the wait a refusal is given.
    pub fn push_again_soon(&mut self, account: Id, count: usize, now: f64) {
        if count > 0 {
            self.push_seen.insert(account, (0, now));
        }
    }

    /// The accounts in line, in order (what Send/Receive Progress shows as the round).
    pub fn round(&self) -> Vec<Id> {
        self.queue.iter().map(|(a, _)| *a).collect()
    }

    /// Take `account` out of the line (skipped); false when it wasn't in it.
    pub fn skip(&mut self, account: Id) -> bool {
        let was = self.is_queued(account);
        self.queue.retain(|(a, _)| *a != account);
        self.push_only_next.remove(&account);
        was
    }
}

/// How much of `account` waits to go to its server: mail in the Outbox, and changes made here that
/// the server hasn't been told. (A held message waits for the owner, not for a run; and so does
/// mail in a shared mailbox's Outbox that may not be sent from it yet, #70.)
pub fn waiting_to_go(s: &crate::Session, account: Id) -> usize {
    let mb = &s.mailbox;
    let outbox = if s.shared_send_waits(account) {
        0
    } else {
        mb.folder_of_kind(account, mailcraft_model::FolderKind::Outbox).map_or(0, |f| mb.messages_in(f).filter(|m| m.held.is_empty()).count())
    };
    outbox + crate::cmd::account::unpushed(s, account)
}

/// The accounts that have a server to be sent and received with, in the mailbox's order.
pub fn server_accounts(s: &crate::Session) -> Vec<Id> {
    s.mailbox.accounts.iter().filter(|a| a.kind.has_server()).map(|a| a.id).collect()
}
