// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Info › Automatic Replies in the window, for the accounts that sign in with Microsoft:
//! the dialog in classic Outlook's shape, the line in File › Info and the status bar that say
//! when replies are going out, and the jobs that read and change them on the server.
//!
//! **Jobs.** The engine says what to ask (`account.automaticReplies`, `account.setAutomaticReplies`)
//! and takes what came back in (`sync.applyAutomaticReplies`); the host does the asking on a
//! background thread ([`crate::Services::graph_auto_replies`]): a token of its own (the
//! calendar's to read them, `mailcraft_net::oauth::refresh_graph`; one for changing the
//! mailbox's settings to change them, `mailcraft_net::oauth::refresh_graph_replies`), then
//! Graph's `GET` and `PATCH` of the mailbox settings' automatic replies. They are read when the dialog opens, when File › Info
//! is shown, and every so often while the window is open, so that the status bar can say they
//! are on; never while working offline, and never while something else of the account's is
//! being asked (a Send/Receive, a sign-in, the calendar's job).
//!
//! **The permission.** Reading them takes the `MailboxSettings.Read` a sign-in asks for.
//! Changing them takes `MailboxSettings.ReadWrite`, which no sign-in page asks for: the first
//! change that finds the sign-in without it is held, the window asks the owner, and Microsoft's
//! page asks for that permission alone (`ui.allowAutomaticReplies`); the change goes once it is
//! given and is dropped, and said, when it isn't. File › Info shows whether it was given, with
//! Ask again. Nothing else waits on it: mail, the calendar, the categories and the working
//! hours go on as they were.
//!
//! **Another mailbox.** Everything here is about the accounts of the mailbox that is open (by
//! id): when another takes its place, all of it is forgotten, and an answer still on its way is
//! thrown away.
//!
//! **What a server says** is the engine's to make text of: the replies are shown as plain text in
//! plain text fields, never as a page, and what the owner types goes up as simple HTML.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use egui::{RichText, Ui, vec2};
use mailcraft_engine::cmd::auto_replies::{Known, reply_accounts};
use mailcraft_model::auto_replies::{AutoReplies, AutoReplyStatus, ExternalAudience};
use mailcraft_model::{Account, Id};
use serde_json::{Value, json};

use crate::dialogs::{Dialog, Question};
use crate::forms::kit;
use crate::task_sync::fingerprint;
use crate::theme::{Tokens, regular};
use crate::{MailApp, Tokens as SignIn, now_ms};

/// How often an account's automatic replies are read while the window is open, and how long a
/// read that failed is left before the next: doubled at each failure in a row, up to an hour
/// (milliseconds).
const REFRESH_MS: f64 = 15.0 * 60_000.0;
const RETRY_MS: f64 = 2.0 * 60_000.0;
const RETRY_MAX_MS: f64 = 60.0 * 60_000.0;
/// How wide the dialog is, and how many lines its reply fields show.
pub const WIDTH: f32 = 560.0;
const REPLY_ROWS: usize = 7;

/// What the host is asked to do with one account's automatic replies
/// ([`crate::Services::graph_auto_replies`]).
pub struct AutoRepliesJob {
    pub account: Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: mailcraft_model::OAuth,
    /// The account's refresh token: the job gets its own token with it.
    pub refresh: String,
    /// What to change first, in Graph's names (`account.setAutomaticReplies`' answer); `None`
    /// only reads them. A change takes a token for `MailboxSettings.ReadWrite`; a read takes the
    /// calendar's, which reads the settings.
    pub change: Option<AutoReplies>,
    pub cancel: Arc<AtomicBool>,
}

/// What an [`AutoRepliesJob`] came to. Never an error as a whole: every trouble is said.
#[derive(Default)]
pub struct AutoRepliesResult {
    pub account: Id,
    /// The refresh token is the account's newest.
    pub tokens: Option<SignIn>,
    /// The automatic replies as the server has them after the job, when they could be read.
    pub setting: Option<AutoReplies>,
    /// A change was asked for and the server took it.
    pub changed: bool,
    /// A change was asked for and the sign-in doesn't cover changing the mailbox's settings:
    /// nothing was sent, and the owner is to be asked for that permission.
    pub write_consent_needed: bool,
    /// A change was asked for with the permission, and the server still said no: the
    /// organisation keeps them.
    pub write_denied: bool,
    /// The sign-in covers none of the mailbox's settings: the owner is to sign in again.
    pub consent_needed: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
    /// What went wrong otherwise, in a few words; empty when nothing did.
    pub error: String,
}

/// The state of the jobs and of what is said about them, in the window.
#[derive(Default)]
pub struct AutoRepliesSync {
    /// The job under way: for which account, the change it makes, the [`fingerprint`] of the
    /// refresh token it was given, and the answer to come.
    rx: Option<(Id, Option<AutoReplies>, u64, Receiver<AutoRepliesResult>)>,
    /// When each account's replies were last asked for, and how many reads in a row failed.
    checked: HashMap<Id, (f64, u32)>,
    /// A change waiting for the job under way to end.
    waiting: Option<(Id, AutoReplies)>,
    /// A change waiting for the owner's permission to change the settings: it goes once the
    /// permission is had, and is dropped when it isn't.
    held: HashMap<Id, AutoReplies>,
    /// What the owner is to be told of an account's replies when they can't be had.
    pub(crate) trouble: HashMap<Id, Trouble>,
    /// Where the permission to change them stands, per account.
    pub(crate) perm: HashMap<Id, Permission>,
    /// The mailbox opening all of this is about ([`mailcraft_engine::Session::openings`]).
    opening: u64,
}

/// Why an account's automatic replies can't be had just now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Trouble {
    /// The sign-in covers none of the mailbox's settings: a new sign-in on Microsoft's page asks
    /// for them. (Not the refresh token's turning over, which the sign-in does by itself on every
    /// renewal: only a sign-in on the page, of any kind, lets them be read again.)
    SignInToRead,
    /// Microsoft no longer takes the sign-in (the same): a new one is wanted.
    SignInOver(u64),
    /// The job failed: why, in a few words.
    Failed(String),
}

/// Where the permission to change an account's automatic replies (`MailboxSettings.ReadWrite`)
/// stands. Asked for by itself, the first time the owner changes them; never with a sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    /// A change went through with it.
    Given,
    /// Changing them was tried with it and the server said no: the organisation keeps them.
    /// Asking again wouldn't change that.
    Denied,
    /// A change found the sign-in doesn't cover it: the owner is to be asked.
    Needed,
    /// The owner is being asked: the question, or Microsoft's page.
    Asking,
    /// The question went away unanswered.
    Declined,
    /// Asked, and no: why, in a few words.
    Refused(String),
}

/// The way back that File › Info offers under an account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Again {
    /// Sign in again (the sign-in doesn't read them, or is over).
    SignIn,
    /// Ask for the permission to change them.
    Ask,
}

impl AutoRepliesSync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// When `account`'s replies were last asked for (for the tests).
    #[cfg(test)]
    pub(crate) fn checked_at(&self, account: Id) -> f64 {
        self.checked.get(&account).map_or(0.0, |(at, _)| *at)
    }

    /// Whether a job for `account` is in flight, and whether it changes them.
    pub(crate) fn busy_for(&self, account: Id) -> Option<bool> {
        self.rx.as_ref().filter(|(a, ..)| *a == account).map(|(_, change, ..)| change.is_some())
    }
}

/// How long a read that failed `failures` times in a row waits before the next.
fn retry_after(failures: u32) -> f64 {
    (RETRY_MS * 2f64.powi(failures.saturating_sub(1).min(16) as i32)).min(RETRY_MAX_MS)
}

impl MailApp {
    /// What is known of `account`'s automatic replies.
    pub(crate) fn auto_replies_known(&self, account: Id) -> Option<&Known> {
        self.session.auto_replies.get(&account)
    }

    /// The refresh token a job for `account` is given.
    fn auto_replies_refresh(&self, account: &Account) -> String {
        let kept = self.tokens.get(&account.id).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        kept.or_else(|| self.remembered("refresh", &account.email)).unwrap_or_default()
    }

    /// Whether `account`'s replies wait for a new sign-in: the one that was refused is still the
    /// one there is. A sign-in made since, any way at all (Send/Receive, Connect Microsoft), lets
    /// them be read again.
    fn auto_replies_blocked(&self, account: Id) -> bool {
        let over = match self.auto_replies.trouble.get(&account) {
            Some(Trouble::SignInToRead) => return true,
            Some(Trouble::SignInOver(over)) => *over,
            _ => return false,
        };
        !self.tokens.get(&account).is_some_and(|t| !t.refresh.is_empty() && fingerprint(&t.refresh) != over)
    }

    /// Read every account's automatic replies when they are due. Called every frame. (Public for
    /// a host's tests.)
    pub fn auto_replies_tick_at(&mut self, now: f64) -> bool {
        self.auto_replies_forget_if_replaced();
        self.auto_replies_ask_tick();
        if self.offline() {
            // Nothing goes up later that the owner asked for before working offline.
            let dropped = self.auto_replies.waiting.take().is_some() | !self.auto_replies.held.is_empty();
            self.auto_replies.held.clear();
            if dropped {
                self.status(tl!("Your automatic replies weren't changed: you are working offline."));
            }
            return false;
        }
        if self.auto_replies.rx.is_some() || self.services.graph_auto_replies.is_none() {
            return false;
        }
        // One thing at a time: a Send/Receive, a sign-in, the calendar's, the tasks' and the rules'
        // jobs go first.
        if self.signing_in.is_some()
            || self.sync_rx.is_some()
            || self.graph_rx.is_some()
            || self.calendar.rx.is_some()
            || self.tasks.busy()
            || self.rules.busy()
        {
            return false;
        }
        if let Some((account, change)) = self.auto_replies.waiting.take() {
            return self.start_auto_replies_job(account, Some(change), now);
        }
        if !self.auto_sync {
            return false;
        }
        for account in reply_accounts(&self.session.mailbox) {
            if self.auto_replies_blocked(account) {
                continue;
            }
            if matches!(self.auto_replies.trouble.get(&account), Some(Trouble::SignInOver(_))) {
                // Signed in again since: a read finds out where they stand, now.
                self.auto_replies.trouble.remove(&account);
                self.auto_replies.checked.remove(&account);
            }
            let due = match self.auto_replies.checked.get(&account) {
                None => true,
                Some((at, 0)) => now - at >= REFRESH_MS,
                Some((at, failures)) => now - at >= retry_after(*failures),
            };
            if due && self.start_auto_replies_job(account, None, now) {
                return true;
            }
        }
        false
    }

    pub(crate) fn auto_replies_tick(&mut self, ctx: &egui::Context) {
        self.auto_replies_tick_at(now_ms());
        if self.auto_replies.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Hand the host a job for `account`: read its replies, after changing them as `change` says.
    fn start_auto_replies_job(&mut self, account: Id, change: Option<AutoReplies>, now: f64) -> bool {
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        let Some(oauth) = owner.oauth.clone() else { return false };
        let refresh = self.auto_replies_refresh(&owner);
        if refresh.is_empty() {
            // (Nothing to renew with yet: Send/Receive signs in first.)
            if change.is_some() {
                self.status(tl!("Sign in with Send/Receive first, then change your automatic replies."));
            }
            return false;
        }
        let changing = change.is_some();
        let given = fingerprint(&refresh);
        let job = AutoRepliesJob { account: owner, oauth: self.effective_oauth(oauth), refresh, change: change.clone(), cancel: Default::default() };
        let Some(host) = self.services.graph_auto_replies.as_ref() else { return false };
        self.auto_replies.rx = Some((account, change, given, host(job)));
        let failures = self.auto_replies.checked.get(&account).map_or(0, |(_, n)| *n);
        self.auto_replies.checked.insert(account, (now, failures));
        if changing {
            self.session.sync_status = tl!("Changing your automatic replies…").to_string();
        }
        true
    }

    /// After the engine ran `id`: what the window does with `account.automaticReplies` (reads
    /// them on the server) and `account.setAutomaticReplies` (changes them there).
    pub(crate) fn auto_replies_after(&mut self, id: &str, r: Result<Value, String>) -> Result<Value, String> {
        let Ok(v) = &r else { return r };
        match id {
            "account.automaticReplies" => {
                if let Some(account) = v.pointer("/ask/account").and_then(Value::as_u64) {
                    self.auto_replies_read_now(account);
                }
            }
            "account.setAutomaticReplies" => {
                let account = v.get("account").and_then(Value::as_u64).unwrap_or(0);
                let change = v.get("change").cloned().map(|c| AutoReplies::from_graph(&c)).unwrap_or_default();
                self.auto_replies_change(account, change)?;
            }
            _ => {}
        }
        r
    }

    /// The window was asked to read `account`'s replies (`account.automaticReplies`): at once,
    /// when nothing else is under way, else at the next chance.
    pub(crate) fn auto_replies_read_now(&mut self, account: Id) {
        if self.auto_replies.busy_for(account).is_some() {
            return;
        }
        self.auto_replies.checked.remove(&account);
        if matches!(self.auto_replies.trouble.get(&account), Some(Trouble::Failed(_))) {
            self.auto_replies.trouble.remove(&account);
        }
        self.auto_replies_tick_at(now_ms());
    }

    /// The window was asked to change `account`'s replies (`account.setAutomaticReplies`). When
    /// the permission is known to be missing, the owner is asked for it first and the change
    /// waits for the answer.
    pub(crate) fn auto_replies_change(&mut self, account: Id, change: AutoReplies) -> Result<(), String> {
        if self.services.graph_auto_replies.is_none() {
            return Err(tl!("This build can't reach the server to change automatic replies. Use the desktop app.").to_string());
        }
        if self.offline() {
            return Err(crate::send_receive::said());
        }
        if matches!(self.auto_replies.trouble.get(&account), Some(Trouble::Failed(_))) {
            self.auto_replies.trouble.remove(&account);
        }
        // The owner is being asked already: this change is the one that goes once he allows it.
        if self.auto_replies.perm.get(&account) == Some(&Permission::Asking) {
            self.auto_replies.held.insert(account, change);
            return Ok(());
        }
        if matches!(self.auto_replies.perm.get(&account), Some(Permission::Needed | Permission::Declined | Permission::Refused(_))) {
            if self.dialog.is_some() || self.signing_in.is_some() || !self.ask_replies(account) {
                return Err(tl!("MailCraft needs your permission to change your automatic replies first: File › Info › Ask again.").to_string());
            }
            self.auto_replies.held.insert(account, change);
            return Ok(());
        }
        self.auto_replies.waiting = Some((account, change));
        self.auto_replies_tick_at(now_ms());
        Ok(())
    }

    /// The job answered: keep the refresh token, take in what the server has, and say how a
    /// change went. (Public for a host's tests.)
    pub fn poll_auto_replies(&mut self) {
        self.auto_replies_forget_if_replaced();
        let Some((account, change, given, rx)) = &self.auto_replies.rx else { return };
        let (account, given) = (*account, *given);
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                AutoRepliesResult { account, error: tl!("Reading the automatic replies stopped unexpectedly").to_string(), ..Default::default() }
            }
        };
        let change = change.clone();
        self.auto_replies.rx = None;
        if change.is_some() {
            self.session.sync_status.clear();
        }
        if let Some(tokens) = &result.tokens
            && !tokens.refresh.is_empty()
            && !self.signed_in_as_other(account, tokens)
        {
            let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
            self.remember("refresh", &address, &tokens.refresh);
            if let Some(mail) = self.tokens.get_mut(&account) {
                mail.refresh = tokens.refresh.clone();
            }
        }
        let mut trouble = None;
        if let Some(setting) = &result.setting {
            let params = json!({"account": account, "setting": setting});
            if let Err(e) = self.session.run("sync.applyAutomaticReplies", &params) {
                trouble = Some(Trouble::Failed(e.to_string()));
            }
        }
        let mut asked = false;
        if result.sign_in_refused && result.tokens.is_none() {
            log::warn!("automatic replies: the sign-in was refused; waiting for a new one");
            trouble = Some(Trouble::SignInOver(given));
        } else if result.consent_needed {
            log::info!("automatic replies: the sign-in doesn't cover the mailbox's settings");
            trouble = Some(Trouble::SignInToRead);
        } else if result.write_consent_needed {
            // The owner is asked, once, by itself; the change goes when he allows it. When he can't
            // be asked now (another window, a sign-in under way), the change is dropped and said:
            // a later Ask again never sends a change made long before.
            log::info!("automatic replies: changing them needs the owner's permission");
            self.auto_replies.perm.insert(account, Permission::Needed);
            asked = self.dialog.is_none() && self.signing_in.is_none() && self.ask_replies(account);
            if let Some(change) = change.clone().filter(|_| asked) {
                self.auto_replies.held.insert(account, change);
            }
        } else if result.write_denied {
            self.auto_replies.perm.insert(account, Permission::Denied);
        } else if !result.error.is_empty() {
            let why: String = result.error.chars().filter(|c| !c.is_control()).take(300).collect();
            trouble = trouble.or(Some(Trouble::Failed(why)));
        }
        if result.changed {
            self.auto_replies.perm.insert(account, Permission::Given);
        }
        let failures = match &trouble {
            Some(Trouble::Failed(_)) => self.auto_replies.checked.get(&account).map_or(0, |(_, n)| *n).saturating_add(1),
            _ => 0,
        };
        self.auto_replies.checked.insert(account, (now_ms(), failures));
        match &trouble {
            Some(t) => {
                self.auto_replies.trouble.insert(account, t.clone());
            }
            None => {
                self.auto_replies.trouble.remove(&account);
            }
        }
        if change.is_none() || asked {
            return;
        }
        // A change the owner asked for: said whichever way it went.
        let said = if result.changed {
            match self.auto_replies_known(account).map(|k| k.on_at(self.session.now())) {
                Some(true) => tl!("Automatic replies are on").to_string(),
                Some(false) if self.auto_replies_known(account).is_some_and(|k| k.status == AutoReplyStatus::Scheduled) => {
                    tl!("Automatic replies are set for the time range you chose").to_string()
                }
                _ => tl!("Automatic replies are off").to_string(),
            }
        } else if result.write_consent_needed {
            tl!("Your automatic replies weren't changed: MailCraft needs your permission first (File › Info › Ask again).").to_string()
        } else if result.write_denied {
            tl!("Your automatic replies weren't changed: your organisation doesn't let MailCraft change them.").to_string()
        } else {
            match &trouble {
                Some(Trouble::SignInToRead) => {
                    tl!("Your automatic replies weren't changed: sign in again to let MailCraft read them (File › Info).").to_string()
                }
                Some(Trouble::SignInOver(_)) => tl!("Your automatic replies weren't changed: Microsoft no longer takes your sign-in.").to_string(),
                Some(Trouble::Failed(why)) => crate::i18n::fmt(tl!("Your automatic replies weren't changed: {error}"), &[("error", why)]),
                None => tl!("Your automatic replies weren't changed").to_string(),
            }
        };
        self.status(said);
    }

    /// The question before Microsoft's page: what is asked of the owner and why. Returns whether
    /// it asked.
    pub(crate) fn ask_replies(&mut self, account: Id) -> bool {
        let Some(address) = self.session.mailbox.account(account).map(|a| a.email.trim().to_string()) else { return false };
        let text = crate::i18n::fmt(
            tl!(
                "To change the automatic replies of {address}, MailCraft needs your permission to change your mailbox settings. Microsoft's page opens in your browser to ask you, once.\n\nMailCraft changes only your automatic replies and the rules of your Inbox with it, never your categories or working hours. Reading your automatic replies doesn't need it."
            ),
            &[("address", &address)],
        );
        let ask = Question {
            title: tl!("Allow Changing Automatic Replies").to_string(),
            text,
            action: tl!("Continue").to_string(),
            with: Value::Null,
            more: Vec::new(),
            note: None,
            cancel: None,
            tick: None,
        };
        self.auto_replies.perm.insert(account, Permission::Asking);
        self.dialog = Some(Dialog::Confirm { command: "ui.allowAutomaticReplies".to_string(), params: json!({"account": account}), ask });
        true
    }

    /// Forget everything about the mailbox that was open when another has taken its place: its
    /// accounts are other accounts, whatever their ids. A job's answer still on its way is
    /// thrown away, and a change that waited never goes.
    pub(crate) fn auto_replies_forget_if_replaced(&mut self) {
        let opening = self.session.openings();
        if self.auto_replies.opening != opening {
            // (A change of the old mailbox's that was under way said so in the status bar.)
            if self.auto_replies.rx.as_ref().is_some_and(|(_, change, ..)| change.is_some())
                && self.session.sync_status == tl!("Changing your automatic replies…")
            {
                self.session.sync_status.clear();
            }
            self.auto_replies = AutoRepliesSync { opening, ..Default::default() };
        }
    }

    /// A sign-in on Microsoft's page went through for `account` (any kind: Send/Receive's,
    /// Connect Microsoft, a permission's): a sign-in that didn't read the mailbox's settings may
    /// now, so they are read again.
    pub(crate) fn auto_replies_signed_in_on_page(&mut self, account: Id) {
        if self.auto_replies.trouble.get(&account) == Some(&Trouble::SignInToRead) {
            self.auto_replies.trouble.remove(&account);
            self.auto_replies.checked.remove(&account);
        }
    }

    /// The owner allowed changing the mailbox's settings on Microsoft's page for the rules (#566):
    /// the automatic replies are changed with the same permission, so they no longer wait for it.
    pub(crate) fn auto_replies_allowed(&mut self, account: Id) {
        if matches!(self.auto_replies.perm.get(&account), Some(Permission::Needed | Permission::Declined | Permission::Refused(_))) {
            self.auto_replies.perm.remove(&account);
        }
    }

    /// File › Info's "Ask again".
    pub(crate) fn auto_replies_ask_again(&mut self, account: Id) {
        if self.dialog.is_none() && self.signing_in.is_none() {
            self.ask_replies(account);
        }
    }

    /// A question that went away unanswered: the change that waited for it is dropped, and said.
    fn auto_replies_ask_tick(&mut self) {
        let still =
            self.signing_in.is_some() || matches!(&self.dialog, Some(Dialog::Confirm { command, .. }) if command == "ui.allowAutomaticReplies");
        if still {
            return;
        }
        let asking: Vec<Id> = self.auto_replies.perm.iter().filter(|(_, p)| **p == Permission::Asking).map(|(a, _)| *a).collect();
        for account in asking {
            self.auto_replies.perm.insert(account, Permission::Declined);
            if self.auto_replies.held.remove(&account).is_some() {
                self.status(tl!("Your automatic replies weren't changed: MailCraft wasn't allowed to change them."));
            }
        }
    }

    /// `ui.allowAutomaticReplies`: Microsoft's page, for changing the mailbox's settings, by
    /// itself, because the owner said go ahead.
    pub(crate) fn begin_replies_sign_in(&mut self, account: Id) -> Result<Value, String> {
        mailcraft_engine::cmd::auto_replies::can_reply(&self.session.mailbox, account).map_err(|e| e.to_string())?;
        let owner = self.session.mailbox.account(account).cloned();
        let owner = owner.ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        let oauth = owner.oauth.clone().ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        self.replies_next = true;
        let begun = self.begin_sign_in(owner.id, oauth, &owner.email, String::new(), false, true);
        self.replies_next = false;
        if let Err(e) = begun {
            self.auto_replies.perm.insert(account, Permission::Needed);
            self.auto_replies.held.remove(&account);
            return Err(e);
        }
        self.auto_replies.perm.insert(account, Permission::Asking);
        self.session.sync_status = tl!("Allow MailCraft to change your automatic replies on the page that opened in your browser…").to_string();
        Ok(json!({"pending": "signIn", "account": owner.id}))
    }

    /// The owner answered on Microsoft's page about changing the settings: with the tokens, the
    /// change that waited goes now; without, it is dropped and why is said.
    pub(crate) fn replies_signed_in(&mut self, account: Id, answer: Result<SignIn, String>) {
        use crate::approval::NO_BROWSER;
        use crate::favorites_sync::{NEEDS_APPROVAL, REFUSED_BY_ORGANISATION, REFUSED_FOR_ACCOUNT};
        self.session.sync_status.clear();
        let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
        let refused = match answer {
            // (Somebody else's tokens are neither used nor remembered, #103.)
            Ok(tokens) if self.signed_in_as_other(account, &tokens) => Some((Permission::Needed, None)),
            Ok(tokens) => {
                if !tokens.refresh.is_empty() {
                    self.remember("refresh", &address, &tokens.refresh);
                    if let Some(kept) = self.tokens.get_mut(&account) {
                        kept.refresh = tokens.refresh.clone();
                    }
                }
                // Allowed, as far as is known: the change that waited finds out.
                log::info!("automatic replies: the owner allowed changing them");
                self.auto_replies.perm.remove(&account);
                // The rules are changed with the same permission (#566).
                if self.rules_held(account) {
                    self.rules_allowed(account);
                }
                if let Some(change) = self.auto_replies.held.remove(&account) {
                    self.auto_replies.waiting = Some((account, change));
                    self.auto_replies_tick_at(now_ms());
                }
                None
            }
            Err(e) if e == NEEDS_APPROVAL => {
                let said = tl!("Your organisation's administrator has to approve MailCraft changing automatic replies once: File › Info");
                let _ = self.run("ui.approvalHelp", json!({"account": account}));
                Some((Permission::Refused(said.to_string()), Some(said.to_string())))
            }
            Err(e) if e == REFUSED_BY_ORGANISATION || e == REFUSED_FOR_ACCOUNT => {
                let said = tl!("Your organisation doesn't let MailCraft change automatic replies");
                Some((Permission::Refused(said.to_string()), Some(said.to_string())))
            }
            Err(e) if e == NO_BROWSER => Some((Permission::Declined, Some(e))),
            Err(e) => Some((Permission::Refused(e.clone()), Some(e))),
        };
        let Some((perm, said)) = refused else { return };
        self.auto_replies.perm.insert(account, perm);
        if self.auto_replies.held.remove(&account).is_some() || said.is_some() {
            let why = said.unwrap_or_else(|| tl!("you signed in as somebody else").to_string());
            self.status(crate::i18n::fmt(tl!("Your automatic replies weren't changed: {error}"), &[("error", &why)]));
        }
    }

    /// Sign in again for `account` (its sign-in doesn't read the mailbox's settings, or is over):
    /// the provider's page. Their replies are read again once it is done.
    pub(crate) fn auto_replies_sign_in(&mut self, account: Id) -> Result<Value, String> {
        let answer = self.run("ui.connectMicrosoft", json!({"account": account}))?;
        self.auto_replies.checked.remove(&account);
        if matches!(self.auto_replies.trouble.get(&account), Some(Trouble::SignInOver(_) | Trouble::SignInToRead)) {
            self.auto_replies.trouble.remove(&account);
        }
        Ok(answer)
    }

    /// What File › Info says of `account`'s automatic replies, and the way back it offers.
    /// `None` for an account that has none here.
    pub(crate) fn auto_replies_line(&self, account: Id) -> Option<(String, Option<Again>)> {
        self.services.graph_auto_replies.as_ref()?;
        if !reply_accounts(&self.session.mailbox).contains(&account) {
            return None;
        }
        let shown = |why: &str| kit::shown(why, 90);
        Some(match self.auto_replies.trouble.get(&account) {
            Some(Trouble::SignInToRead) => {
                (tl!("Automatic replies: MailCraft needs you to sign in again to read them").to_string(), Some(Again::SignIn))
            }
            Some(Trouble::SignInOver(_)) => (tl!("Automatic replies: Microsoft no longer takes the sign-in").to_string(), Some(Again::SignIn)),
            Some(Trouble::Failed(why)) => {
                (crate::i18n::fmt(tl!("Automatic replies: couldn't be read ({why})"), &[("why", shown(why).as_str())]), None)
            }
            None => match self.auto_replies.perm.get(&account) {
                Some(Permission::Needed | Permission::Declined) => {
                    (tl!("Automatic replies: changing them waits for your permission").to_string(), Some(Again::Ask))
                }
                Some(Permission::Asking) => (tl!("Automatic replies: asking for your permission to change them…").to_string(), None),
                Some(Permission::Denied) => (tl!("Automatic replies: your organisation doesn't let MailCraft change them").to_string(), None),
                Some(Permission::Refused(why)) => {
                    (crate::i18n::fmt(tl!("Automatic replies: changing them not allowed ({why})"), &[("why", shown(why).as_str())]), Some(Again::Ask))
                }
                given => {
                    let line = match self.auto_replies_known(account) {
                        Some(known) => summary(self, known),
                        None if self.auto_replies.busy_for(account).is_some() => tl!("Automatic replies: reading…").to_string(),
                        None => tl!("Automatic replies: not read yet").to_string(),
                    };
                    if given.is_some() {
                        (crate::i18n::fmt(tl!("{line} · MailCraft may change them"), &[("line", &line)]), None)
                    } else {
                        (line, None)
                    }
                }
            },
        })
    }

    /// The accounts whose automatic replies go out now, for the status bar.
    pub(crate) fn auto_replies_on(&self) -> Vec<Id> {
        let now = self.session.now();
        reply_accounts(&self.session.mailbox).into_iter().filter(|a| self.auto_replies_known(*a).is_some_and(|k| k.on_at(now))).collect()
    }

    /// For a stand-in host (the screenshot example): `account`'s replies as a server has them,
    /// and whether changing them is allowed (else it waits for the owner's permission).
    #[doc(hidden)]
    pub fn stand_in_auto_replies(&mut self, account: Id, setting: &AutoReplies, allowed: bool) -> Result<Value, String> {
        let params = json!({"account": account, "setting": setting});
        let answer = self.session.run("sync.applyAutomaticReplies", &params).map_err(|e| e.to_string())?;
        // (A host that never answers, so that File › Info and the status bar have them.)
        if self.services.graph_auto_replies.is_none() {
            self.services.graph_auto_replies = Some(Box::new(|_| std::sync::mpsc::channel().1));
        }
        self.auto_replies.trouble.remove(&account);
        self.auto_replies.perm.insert(account, if allowed { Permission::Given } else { Permission::Needed });
        self.auto_replies.checked.insert(account, (now_ms(), 0));
        Ok(answer)
    }
}

/// One line of what is known: off, on, or on between two times.
fn summary(app: &MailApp, known: &Known) -> String {
    let when = |at: mailcraft_time::Timestamp| {
        let wall = app.session.zone().wall(at);
        format!("{} {}", crate::dates::medium(wall.date), wall.time_short(app.session.prefs.twelve_hour))
    };
    match known.status {
        AutoReplyStatus::Disabled => tl!("Automatic replies: off").to_string(),
        AutoReplyStatus::AlwaysEnabled => tl!("Automatic replies: on, until you turn them off").to_string(),
        AutoReplyStatus::Scheduled => match (known.start, known.end) {
            (Some(start), Some(end)) => {
                crate::i18n::fmt(tl!("Automatic replies: from {start} to {end}"), &[("start", &when(start)), ("end", &when(end))])
            }
            _ => tl!("Automatic replies: during a time range").to_string(),
        },
    }
}

/// Which tab of the dialog is in front.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Inside,
    Outside,
}

/// File › Info › Automatic Replies: it only collects what the owner wants, then runs
/// `account.setAutomaticReplies`.
pub struct Draft {
    pub account: Id,
    /// Who it is for, as the title says.
    pub address: String,
    /// Send automatic replies (else don't).
    pub on: bool,
    /// Only during the time range below.
    pub ranged: bool,
    pub start_date: String,
    pub start_time: String,
    pub end_date: String,
    pub end_time: String,
    pub tab: Tab,
    pub internal: String,
    pub external: String,
    /// Reply to people outside the organisation at all, and to which of them.
    pub outside: bool,
    pub contacts_only: bool,
    /// Whether the fields were filled from what the server has (else from nothing, while it is
    /// read): they are filled once, when it comes.
    pub filled: bool,
    pub error: String,
    /// The first field takes the keyboard once.
    focus: bool,
}

impl Draft {
    /// For `{"account"}` (default: the first account that has automatic replies here); `None`
    /// when the engine refuses (no such account, or one that has none), and the refusal is said
    /// as any command's is.
    pub fn open(app: &mut MailApp, params: &Value) -> Option<Draft> {
        let mut asked = json!({});
        if let Some(account) = params.get("account").filter(|v| !v.is_null()) {
            asked["account"] = account.clone();
        }
        let answer = app.run("account.automaticReplies", asked).ok()?;
        let account = answer.get("account").and_then(Value::as_u64)?;
        let address = app.session.mailbox.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
        let mut draft = Draft {
            account,
            address,
            on: false,
            ranged: false,
            start_date: String::new(),
            start_time: String::new(),
            end_date: String::new(),
            end_time: String::new(),
            tab: Tab::Inside,
            internal: String::new(),
            external: String::new(),
            outside: true,
            contacts_only: false,
            filled: false,
            error: String::new(),
            focus: false,
        };
        draft.default_range(app);
        draft.fill(app);
        Some(draft)
    }

    /// The time range a new one starts with: from the next whole hour, for a week.
    fn default_range(&mut self, app: &MailApp) {
        let zone = app.session.zone();
        let now = app.session.now();
        let start = now.saturating_add(3_600 - now.rem_euclid(3_600));
        let end = start.saturating_add(7 * 86_400);
        (self.start_date, self.start_time) = wall(app, zone, start);
        (self.end_date, self.end_time) = wall(app, zone, end);
    }

    /// Fill the fields from what is known, once it is.
    fn fill(&mut self, app: &MailApp) {
        if self.filled {
            return;
        }
        let Some(known) = app.auto_replies_known(self.account) else { return };
        let zone = app.session.zone();
        self.on = known.status != AutoReplyStatus::Disabled;
        self.ranged = known.status == AutoReplyStatus::Scheduled;
        if let Some(start) = known.start {
            (self.start_date, self.start_time) = wall(app, zone, start);
        }
        if let Some(end) = known.end {
            (self.end_date, self.end_time) = wall(app, zone, end);
        }
        self.internal = known.internal.clone();
        self.external = known.external.clone();
        self.outside = known.audience != ExternalAudience::None;
        self.contacts_only = known.audience == ExternalAudience::ContactsOnly;
        self.filled = true;
    }

    /// What `account.setAutomaticReplies` is given.
    pub fn params(&self) -> Value {
        let status = match (self.on, self.ranged) {
            (false, _) => AutoReplyStatus::Disabled,
            (true, false) => AutoReplyStatus::AlwaysEnabled,
            (true, true) => AutoReplyStatus::Scheduled,
        };
        let audience = match (self.outside, self.contacts_only) {
            (false, _) => ExternalAudience::None,
            (true, true) => ExternalAudience::ContactsOnly,
            (true, false) => ExternalAudience::All,
        };
        let mut params = json!({
            "account": self.account,
            "status": status.code(),
            "internal": self.internal,
            "external": self.external,
            "audience": audience.code(),
        });
        if status == AutoReplyStatus::Scheduled {
            params["start"] = json!(kit::stamp(&self.start_date, &self.start_time));
            params["end"] = json!(kit::stamp(&self.end_date, &self.end_time));
        }
        params
    }

    /// Draw it. `true` when it is done with (OK went through, or Cancel).
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        self.fill(app);
        let t = Tokens::get(ui.ctx());
        let mut pass = kit::Pass::new(ui, &app.session, &mut self.focus);
        let trouble = app.auto_replies.trouble.get(&self.account).cloned();
        let perm = app.auto_replies.perm.get(&self.account).cloned();
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        // What stands in the way, and the way past it.
        if let Some(trouble) = &trouble {
            let (text, sign_in) = match trouble {
                Trouble::SignInToRead => (
                    tl!(
                        "MailCraft needs you to sign in again to read your automatic replies: Microsoft's page opens in your browser. Mail and your calendar work as they are meanwhile."
                    )
                    .to_string(),
                    true,
                ),
                Trouble::SignInOver(_) => (tl!("Microsoft no longer takes your sign-in. Sign in again to read and change your automatic replies.").to_string(), true),
                Trouble::Failed(why) => (crate::i18n::fmt(tl!("Your automatic replies couldn't be read: {error}"), &[("error", why)]), false),
            };
            notice(ui, &t, &text, |ui| {
                if sign_in {
                    let button = ui.add_enabled(app.signing_in.is_none() && !app.offline(), egui::Button::new(tl!("Sign In Again")));
                    app.hit.add("autoRepliesSignIn", self.account, button.rect);
                    if button.clicked()
                        && let Err(e) = app.auto_replies_sign_in(self.account)
                    {
                        self.error = e;
                    }
                }
            });
        } else if perm == Some(Permission::Denied) {
            notice(ui, &t, tl!("Your organisation doesn't let MailCraft change your automatic replies. Ask your administrator."), |_| {});
        } else if let Some(Permission::Needed | Permission::Declined | Permission::Refused(_)) = &perm {
            let text = match &perm {
                Some(Permission::Refused(why)) => crate::i18n::fmt(
                    tl!("Changing your automatic replies wasn't allowed ({why}). Press OK to ask again: Microsoft's page opens in your browser."),
                    &[("why", kit::shown(why, 120).as_str())],
                ),
                _ => tl!(
                    "Changing your automatic replies needs your permission, once: when you press OK, Microsoft's page opens in your browser to ask you. Reading them doesn't need it."
                )
                .to_string(),
            };
            notice(ui, &t, &text, |_| {});
        }
        if !self.filled && trouble.is_none() {
            ui.label(RichText::new(tl!("Reading your automatic replies from the server…")).color(t.text_dim));
        } else if app.offline() {
            ui.label(RichText::new(crate::send_receive::said()).color(t.text_dim));
        }
        let editable = self.editable(app);
        ui.add_enabled_ui(editable, |ui| {
            if ui.radio(!self.on, tl!("Don't send automatic replies")).clicked() {
                self.on = false;
            }
            if ui.radio(self.on, tl!("Send automatic replies")).clicked() {
                self.on = true;
            }
            ui.add_enabled_ui(self.on, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    ui.checkbox(&mut self.ranged, tl!("Only send during this time range:"));
                });
                ui.add_enabled_ui(self.ranged, |ui| {
                    for (label, key, date, time) in [
                        (tl!("Start time:"), "start", &mut self.start_date, &mut self.start_time),
                        (tl!("End time:"), "end", &mut self.end_date, &mut self.end_time),
                    ] {
                        ui.horizontal(|ui| {
                            ui.add_space(44.0);
                            let (r, _) = ui.allocate_exact_size(vec2(70.0, kit::ROW_H), egui::Sense::hover());
                            ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, label, regular(13.0), t.text);
                            pass.date(ui, ("autoReplies", key, "date"), date, true);
                            pass.time(ui, ("autoReplies", key, "time"), time, true, None);
                        });
                    }
                });
                ui.add_space(4.0);
                ui.label(tl!("Each sender gets one automatic reply, with the message below:"));
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.tab == Tab::Inside, tl!("Inside My Organization")).clicked() {
                        self.tab = Tab::Inside;
                    }
                    let outside = if self.outside { tl!("Outside My Organization (On)") } else { tl!("Outside My Organization (Off)") };
                    if ui.selectable_label(self.tab == Tab::Outside, outside).clicked() {
                        self.tab = Tab::Outside;
                    }
                });
                ui.separator();
                match self.tab {
                    Tab::Inside => {
                        kit::multiline(ui, ("autoReplies", "internal"), &mut self.internal, REPLY_ROWS);
                    }
                    Tab::Outside => {
                        ui.checkbox(&mut self.outside, tl!("Reply to people outside my organization"));
                        ui.add_enabled_ui(self.outside, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(22.0);
                                if ui.radio(self.contacts_only, tl!("Only my contacts")).clicked() {
                                    self.contacts_only = true;
                                }
                                if ui.radio(!self.contacts_only, tl!("Everyone outside my organization")).clicked() {
                                    self.contacts_only = false;
                                }
                            });
                            kit::multiline(ui, ("autoReplies", "external"), &mut self.external, REPLY_ROWS - 2);
                        });
                    }
                }
            });
        });
        kit::error(ui, &self.error);
        let dates_ok = !self.on
            || !self.ranged
            || (kit::date_ok(&self.start_date, true)
                && kit::date_ok(&self.end_date, true)
                && kit::time_ok(&self.start_time, true)
                && kit::time_ok(&self.end_time, true));
        let changing = app.auto_replies.busy_for(self.account) == Some(true);
        let footer = kit::footer(ui, tl!("OK"), editable && dates_ok && !changing, false);
        if footer.cancel {
            return true;
        }
        if (footer.ok || pass.submit) && editable && dates_ok {
            match app.run("account.setAutomaticReplies", self.params()) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        false
    }

    /// Whether the owner may change anything in it, and say OK: only once the server's setting
    /// is here (what is left as it is must stay as the server has it, and nothing typed is
    /// written over when it comes), with a sign-in that reads them, while not asking for the
    /// permission, and not offline.
    pub(crate) fn editable(&self, app: &MailApp) -> bool {
        let trouble = app.auto_replies.trouble.get(&self.account);
        let asking = app.auto_replies.perm.get(&self.account) == Some(&Permission::Asking);
        self.filled && !matches!(trouble, Some(Trouble::SignInToRead | Trouble::SignInOver(_))) && !asking && !app.offline()
    }

    /// The dialog's title: whose replies they are.
    pub fn title(&self) -> String {
        if self.address.is_empty() {
            tl!("Automatic Replies").to_string()
        } else {
            crate::i18n::fmt(tl!("Automatic Replies - {address}"), &[("address", &self.address)])
        }
    }
}

/// A date and a time of day as the dialog's fields show them.
fn wall(app: &MailApp, zone: mailcraft_time::Zone, at: mailcraft_time::Timestamp) -> (String, String) {
    let wall = zone.wall(at);
    (wall.date.iso(), wall.time_short(app.session.prefs.twelve_hour))
}

/// A band across the top of the dialog that says what stands in the way, with its button.
fn notice(ui: &mut Ui, t: &Tokens, text: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(6.0).inner_margin(egui::Margin::same(10)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(t.text));
            add(ui);
        },
    );
    ui.add_space(4.0);
}

/// File › Info's Automatic Replies: a tile for each account that has them here, filled while
/// replies go out, as Outlook's is; a click opens the dialog. Nothing for a mailbox without one.
pub(crate) fn info_tiles(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    if app.services.graph_auto_replies.is_none() {
        return;
    }
    let accounts = reply_accounts(&app.session.mailbox);
    if accounts.is_empty() {
        return;
    }
    ui.add_space(10.0);
    ui.label(RichText::new(tl!("Automatic Replies")).font(crate::theme::semibold(14.0)).color(t.accent_text));
    let now = app.session.now();
    for account in accounts {
        let on = app.auto_replies_known(account).is_some_and(|k| k.on_at(now));
        let address = app.session.mailbox.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
        let (line, _) = app.auto_replies_line(account).unwrap_or_default();
        let detail = if app.auto_replies_known(account).is_some() || app.auto_replies.trouble.contains_key(&account) {
            line
        } else {
            tl!("Let people know you're away or can't answer for a while: your mailbox replies by itself").to_string()
        };
        let title = if on {
            crate::i18n::fmt(tl!("Automatic replies are being sent for {address}"), &[("address", &address)])
        } else {
            crate::i18n::fmt(tl!("Automatic Replies for {address}"), &[("address", &address)])
        };
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(560.0), 52.0), egui::Sense::click());
        let fill = match (on, resp.hovered()) {
            (true, _) => t.flagged_row,
            (false, true) => t.hover,
            (false, false) => t.panel,
        };
        ui.painter().rect(r, 6.0, fill, egui::Stroke::new(1.0, if on { t.orange } else { t.border }), egui::StrokeKind::Inside);
        let icon = egui::Rect::from_center_size(egui::pos2(r.min.x + 26.0, r.center().y), vec2(24.0, 24.0));
        crate::icons::paint(ui.painter(), icon, "reply", t.icon, t.accent);
        crate::theme::paint_clipped(
            ui.painter(),
            egui::pos2(r.min.x + 52.0, r.min.y + 18.0),
            r.width() - 64.0,
            &title,
            crate::theme::semibold(13.0),
            t.text,
        );
        crate::theme::paint_clipped(ui.painter(), egui::pos2(r.min.x + 52.0, r.min.y + 36.0), r.width() - 64.0, &detail, regular(11.5), t.text_dim);
        app.hit.add("autoRepliesTile", account, r);
        if resp.clicked() {
            let _ = app.run("ui.dialog", json!({"name": "automaticReplies", "account": account}));
        }
    }
}

/// The status bar's word while replies go out for some account, as Outlook's: a click opens the
/// dialog for the first.
pub(crate) fn status_item(app: &mut MailApp, ui: &mut Ui, t: &Tokens, size: f32) {
    let Some(first) = app.auto_replies_on().first().copied() else { return };
    let link = ui.link(RichText::new(tl!("Automatic Replies")).size(size).color(t.orange));
    app.hit.add("autoRepliesOn", first, link.rect);
    if link.on_hover_text(tl!("Your mailbox is sending automatic replies. Click to change them.")).clicked() {
        let _ = app.run("ui.dialog", json!({"name": "automaticReplies", "account": first}));
    }
    ui.add_space(10.0);
}
