// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The automatic replies kept in step with the mailbox's settings on the server (#566), the
//! decisions and the state, with no I/O: [`Session::auto_replies_due`] says which accounts' jobs
//! are to start now (a change the owner made first, then the reads that are due); the host starts
//! the first it can ([`Session::auto_replies_started`]) and, when it answers, hands what came to
//! [`Session::auto_replies_answered`], which keeps where the permission to change them stands,
//! takes in what the server has and says what is the matter. The questions to the owner (the
//! dialog, the provider's page) and the words that tell him how a change went are the window's.

use std::collections::HashMap;

use mailcraft_model::Id;
use mailcraft_model::auto_replies::AutoReplies;
use serde_json::json;

use super::{Busy, Permission, Start};
use crate::Session;

/// How often an account's automatic replies are read while the program is open, and how long a
/// read that failed is left before the next: doubled at each failure in a row, up to an hour
/// (milliseconds).
pub const REFRESH_MS: f64 = 15.0 * 60_000.0;
pub const RETRY_MS: f64 = 2.0 * 60_000.0;
pub const RETRY_MAX_MS: f64 = 60.0 * 60_000.0;

/// How long a read that failed `failures` times in a row waits before the next.
pub fn retry_after(failures: u32) -> f64 {
    (RETRY_MS * 2f64.powi(i32::try_from(failures.saturating_sub(1).min(16)).unwrap_or(16))).min(RETRY_MAX_MS)
}

/// Why an account's automatic replies can't be had just now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// The sign-in covers none of the mailbox's settings: a new sign-in on the provider's page
    /// asks for them. (Not the refresh token's turning over, which the sign-in does by itself on
    /// every renewal: only a sign-in on the page, of any kind, lets them be read again.)
    SignInToRead,
    /// The provider no longer takes the sign-in (the [`fingerprint`](super::fingerprint) of the
    /// refresh token that was refused): a new one is wanted.
    SignInOver(u64),
    /// The job failed: why, in a few words.
    Failed(String),
}

/// What the driver remembers of the automatic replies' jobs.
#[derive(Debug, Default)]
pub struct AutoRepliesDriver {
    /// When each account's replies were last asked for, and how many reads in a row failed.
    checked: HashMap<Id, (f64, u32)>,
    /// A change waiting for the job under way to end.
    pub waiting: Option<(Id, AutoReplies)>,
    /// A change waiting for the owner's permission to change the settings: it goes once the
    /// permission is had, and is dropped when it isn't.
    pub held: HashMap<Id, AutoReplies>,
    /// What the owner is to be told of an account's replies when they can't be had.
    pub trouble: HashMap<Id, Trouble>,
    /// Where the permission to change them stands, per account (`MailboxSettings.ReadWrite`: asked
    /// for by itself, the first time the owner changes them, never with a sign-in).
    pub perm: HashMap<Id, Permission>,
}

/// What the owner's change of an account's replies comes to, as far as the engine can say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Working offline: nothing goes to a server.
    Offline,
    /// The owner is being asked already: the change is held, and goes once he allows it.
    Held,
    /// The permission is missing: the owner is to be asked, and the change held when he is
    /// ([`AutoRepliesDriver::hold`]).
    MustAsk,
    /// The change goes at the next look, when nothing else is at work.
    Queued,
}

/// One automatic replies' job that is due.
#[derive(Clone, Debug, PartialEq)]
pub struct RepliesPlan {
    pub account: Id,
    /// What to change first, in Graph's names; `None` only reads them.
    pub change: Option<AutoReplies>,
}

/// What the host says of itself when it asks whether the automatic replies are due.
#[derive(Clone, Copy, Debug)]
pub struct RepliesFacts {
    /// The host has a way to reach the settings' server.
    pub reachable: bool,
    /// An automatic replies' job is in flight already.
    pub running: bool,
    /// What else is at work.
    pub busy: Busy,
}

/// What [`Session::auto_replies_due`] says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepliesLook {
    /// A change the owner made before working offline was let go of: it is to be said.
    pub dropped: bool,
    /// The jobs that are due, in the order they are tried.
    pub plans: Vec<RepliesPlan>,
}

/// What an automatic replies' job brought, as the engine takes it in (borrowed from the host's
/// answer).
#[derive(Clone, Copy, Debug, Default)]
pub struct RepliesCame<'a> {
    /// The automatic replies as the server has them after the job, when they could be read.
    pub setting: Option<&'a AutoReplies>,
    /// The job got a refresh token (the account's newest).
    pub new_tokens: bool,
    /// A change was asked for and the server took it.
    pub changed: bool,
    /// A change was asked for and the sign-in doesn't cover changing the mailbox's settings.
    pub write_consent_needed: bool,
    /// A change was asked for with the permission, and the server still said no.
    pub write_denied: bool,
    /// The sign-in covers none of the mailbox's settings.
    pub consent_needed: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
    /// What went wrong otherwise, in a few words; empty when nothing did.
    pub error: &'a str,
}

/// What the engine made of an answer.
#[derive(Clone, Debug, PartialEq)]
pub struct RepliesAnswer {
    /// Why the replies can't be had now, when they can't.
    pub trouble: Option<Trouble>,
    /// A change found the sign-in doesn't cover changing them: the owner is to be asked (the
    /// permission is `Needed`; the window sets it `Asking` when it puts the question).
    pub ask: bool,
}

impl AutoRepliesDriver {
    /// When `account`'s replies were last asked for (0 when never).
    pub fn checked_at(&self, account: Id) -> f64 {
        self.checked.get(&account).map_or(0.0, |(at, _)| *at)
    }

    /// Note that the host started a job for `account` at `now`.
    pub fn started(&mut self, account: Id, now: f64) {
        let failures = self.checked.get(&account).map_or(0, |(_, n)| *n);
        self.checked.insert(account, (now, failures));
    }

    /// The owner is being asked about `account`.
    pub fn asking(&mut self, account: Id) {
        self.perm.insert(account, Permission::Asking);
    }

    /// The owner is to be asked about `account`.
    pub fn needed(&mut self, account: Id) {
        self.perm.insert(account, Permission::Needed);
    }

    /// The change that goes once `account`'s owner allows it.
    pub fn hold(&mut self, account: Id, change: AutoReplies) {
        self.held.insert(account, change);
    }

    /// The change that waited for the owner's permission is let go of: whether there was one.
    pub fn drop_held(&mut self, account: Id) -> bool {
        self.held.remove(&account).is_some()
    }

    /// The window was asked to read `account`'s replies: the next look asks, and a failure of the
    /// last one is forgotten.
    pub fn read_now(&mut self, account: Id) {
        self.checked.remove(&account);
        if matches!(self.trouble.get(&account), Some(Trouble::Failed(_))) {
            self.trouble.remove(&account);
        }
    }

    /// A sign-in on the provider's page went through for `account` (any kind): a sign-in that
    /// didn't read the mailbox's settings may now, so they are read again.
    pub fn signed_in_on_page(&mut self, account: Id) {
        if self.trouble.get(&account) == Some(&Trouble::SignInToRead) {
            self.trouble.remove(&account);
            self.checked.remove(&account);
        }
    }

    /// The owner signs in again for `account` (its sign-in doesn't read the settings, or is
    /// over): they are read again once that is done.
    pub fn sign_in_again(&mut self, account: Id) {
        self.checked.remove(&account);
        if matches!(self.trouble.get(&account), Some(Trouble::SignInOver(_) | Trouble::SignInToRead)) {
            self.trouble.remove(&account);
        }
    }

    /// The owner allowed changing the mailbox's settings on the provider's page for the rules
    /// (#566): the automatic replies are changed with the same permission, so they no longer wait
    /// for it.
    pub fn allowed_for_rules(&mut self, account: Id) {
        if matches!(self.perm.get(&account), Some(Permission::Needed | Permission::Declined | Permission::Refused(_))) {
            self.perm.remove(&account);
        }
    }

    /// The owner allowed changing them on the provider's page: the change that waited goes at the
    /// next look. Whether there was one.
    pub fn allowed(&mut self, account: Id) -> bool {
        self.perm.remove(&account);
        let Some(change) = self.held.remove(&account) else { return false };
        self.waiting = Some((account, change));
        true
    }

    /// The owner's page said no, or couldn't be had: `perm` is where it stands now.
    pub fn answered_no(&mut self, account: Id, perm: Permission) {
        self.perm.insert(account, perm);
    }

    /// A question that went away unanswered is not asked again by itself: when none is on screen
    /// (`still`), every account being asked about is `Declined`, and the change that waited for
    /// each is dropped. Which accounts had one.
    pub fn unanswered(&mut self, still: bool) -> Vec<Id> {
        let declined = super::unanswered(&mut self.perm, still);
        declined.into_iter().filter(|account| self.held.remove(account).is_some()).collect()
    }

    /// Account removal: nothing of these accounts is kept.
    pub fn forget_accounts(&mut self, gone: &std::collections::HashSet<Id>) {
        self.checked.retain(|account, _| !gone.contains(account));
        if self.waiting.as_ref().is_some_and(|(account, _)| gone.contains(account)) {
            self.waiting = None;
        }
        self.held.retain(|account, _| !gone.contains(account));
        self.trouble.retain(|account, _| !gone.contains(account));
        self.perm.retain(|account, _| !gone.contains(account));
    }
}

impl Session {
    /// Whether `account`'s replies wait for a new sign-in: the one that was refused is still the
    /// one there is (`refresh` is the account's refresh token now). A sign-in made since, any way
    /// at all, lets them be read again.
    pub fn auto_replies_blocked(&self, account: Id, refresh: Option<&str>) -> bool {
        let over = match self.driver.auto_replies.trouble.get(&account) {
            Some(Trouble::SignInToRead) => return true,
            Some(Trouble::SignInOver(over)) => *over,
            _ => return false,
        };
        !refresh.is_some_and(|r| !r.is_empty() && super::fingerprint(r) != over)
    }

    /// The owner changed `account`'s replies (`account.setAutomaticReplies`): what to do with it.
    /// A failure the last read had is forgotten, since the change reads them anew.
    pub fn auto_replies_change(&mut self, account: Id, change: AutoReplies) -> Change {
        let driver = &mut self.driver.auto_replies;
        if self.prefs.offline {
            return Change::Offline;
        }
        if matches!(driver.trouble.get(&account), Some(Trouble::Failed(_))) {
            driver.trouble.remove(&account);
        }
        match driver.perm.get(&account) {
            Some(Permission::Asking) => {
                driver.held.insert(account, change);
                Change::Held
            }
            Some(Permission::Needed | Permission::Declined | Permission::Refused(_)) => Change::MustAsk,
            _ => {
                driver.waiting = Some((account, change));
                Change::Queued
            }
        }
    }

    /// The accounts whose automatic replies job is due at `now`, in the order they are tried; the
    /// host starts the first it can ([`Session::auto_replies_started`]). `refresh`: an account's
    /// refresh token now. Working offline, nothing goes later that the owner asked for before.
    pub fn auto_replies_due(&mut self, facts: &RepliesFacts, refresh: impl Fn(Id) -> Option<String>, now: f64) -> RepliesLook {
        let mut look = RepliesLook::default();
        if self.prefs.offline {
            let driver = &mut self.driver.auto_replies;
            look.dropped = driver.waiting.take().is_some() | !driver.held.is_empty();
            driver.held.clear();
            return look;
        }
        // One thing at a time: a Send/Receive, a sign-in, the calendar's, the tasks' and the
        // rules' jobs go first.
        if facts.running || !facts.reachable || facts.busy.blocks(Start::AutoReplies) {
            return look;
        }
        if let Some((account, change)) = self.driver.auto_replies.waiting.take() {
            look.plans.push(RepliesPlan { account, change: Some(change) });
            return look;
        }
        if !self.driver.auto_sync() {
            return look;
        }
        for account in crate::cmd::auto_replies::reply_accounts(&self.mailbox) {
            if self.auto_replies_blocked(account, refresh(account).as_deref()) {
                continue;
            }
            let driver = &mut self.driver.auto_replies;
            if matches!(driver.trouble.get(&account), Some(Trouble::SignInOver(_))) {
                // Signed in again since: a read finds out where they stand, now.
                driver.trouble.remove(&account);
                driver.checked.remove(&account);
            }
            let due = match driver.checked.get(&account) {
                None => true,
                Some((at, 0)) => now - at >= REFRESH_MS,
                Some((at, failures)) => now - at >= retry_after(*failures),
            };
            if due {
                look.plans.push(RepliesPlan { account, change: None });
            }
        }
        look
    }

    /// The host handed an automatic replies' job for `account` to its server at `now`.
    pub fn auto_replies_started(&mut self, account: Id, now: f64) {
        self.driver.auto_replies.started(account, now);
    }

    /// An automatic replies' job for `account` answered at `now`. `given`: the
    /// [`fingerprint`](super::fingerprint) of the refresh token it was given. Keeps where things
    /// stand and takes in what the server has. `None` for an account removed meanwhile: such an
    /// answer changes nothing.
    pub fn auto_replies_answered(&mut self, account: Id, given: u64, now: f64, came: &RepliesCame<'_>) -> Option<RepliesAnswer> {
        self.mailbox.account(account)?;
        let mut trouble = None;
        if let Some(setting) = came.setting
            && let Err(e) = self.run("sync.applyAutomaticReplies", &json!({"account": account, "setting": setting}))
        {
            trouble = Some(Trouble::Failed(e.to_string()));
        }
        let driver = &mut self.driver.auto_replies;
        let mut ask = false;
        if came.sign_in_refused && !came.new_tokens {
            log::warn!("automatic replies: the sign-in was refused; waiting for a new one");
            trouble = Some(Trouble::SignInOver(given));
        } else if came.consent_needed {
            log::info!("automatic replies: the sign-in doesn't cover the mailbox's settings");
            trouble = Some(Trouble::SignInToRead);
        } else if came.write_consent_needed {
            log::info!("automatic replies: changing them needs the owner's permission");
            driver.perm.insert(account, Permission::Needed);
            ask = true;
        } else if came.write_denied {
            driver.perm.insert(account, Permission::Denied);
        } else if !came.error.is_empty() {
            let why: String = came.error.chars().filter(|c| !c.is_control()).take(300).collect();
            trouble = trouble.or(Some(Trouble::Failed(why)));
        }
        if came.changed {
            driver.perm.insert(account, Permission::Given);
        }
        let failures = match &trouble {
            Some(Trouble::Failed(_)) => driver.checked.get(&account).map_or(0, |(_, n)| *n).saturating_add(1),
            _ => 0,
        };
        driver.checked.insert(account, (now, failures));
        match &trouble {
            Some(t) => {
                driver.trouble.insert(account, t.clone());
            }
            None => {
                driver.trouble.remove(&account);
            }
        }
        Some(RepliesAnswer { trouble, ask })
    }
}
