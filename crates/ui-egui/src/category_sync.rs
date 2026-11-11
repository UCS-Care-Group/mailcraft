// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's list of categories kept in step with its server, in the window (#661).
//!
//! A job of its own, like the to-dos' ([`crate::task_sync`]): once at the start of a session (to
//! learn whether the sign-in may change the list), and a moment after a change here, the window
//! hands the host a [`CategoriesJob`] with what is to be told to the server
//! (`sync.categoryChanges`); the host tells it and reads the list back, and the window takes in
//! whether the list may be changed (`sync.categoryWrites`), what the server made of the changes
//! (`sync.applyCategoryChanges`) and what it has (`sync.applyCategories`). The calendar's job
//! reads the list too, every few minutes, so Outlook's changes show here with it. The merge is the
//! engine's ([`mailcraft_engine::cmd::categories_remote`]).
//!
//! **The permission.** Changing the list takes `MailboxSettings.ReadWrite`, as the rules and the
//! automatic replies do. Without it the list is read-only here, and the Categories window says so
//! with Allow; the window asks the owner the one question of [`crate::server_sync`] by itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::categories_remote;
use mailcraft_model::remote::{CategoriesDownload, CategoriesUpload, CategoriesUploaded};
use mailcraft_model::{Account, AccountKind, Id, Mailbox};

use crate::task_sync::{Permission, fingerprint};
use crate::{MailApp, Tokens, now_ms};

/// How long a change is left to settle before it goes, the wait after a job that failed, and the
/// longest such wait (milliseconds).
const QUIET_MS: f64 = 3_000.0;
/// How often what waits to go is worked out again at most, while the mailbox changes.
const RECOUNT_MS: f64 = 1_000.0;
const FIRST_RETRY_MS: f64 = 30_000.0;
const LONGEST_RETRY_MS: f64 = 15.0 * 60_000.0;
/// How many times a job tries again, quietly, when the list may not be changed right after the
/// owner allowed it: Microsoft may not have taken it in everywhere yet (#491).
const SETTLING_TRIES: u32 = 3;

/// What the host is asked to do for one account's list of categories
/// ([`crate::Services::graph_categories`]).
pub struct CategoriesJob {
    pub account: Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: mailcraft_model::OAuth,
    /// The account's refresh token: the job gets its own token for changing the list with it.
    pub refresh: String,
    /// What the server is to be told first.
    pub upload: CategoriesUpload,
    pub cancel: Arc<AtomicBool>,
}

/// What a [`CategoriesJob`] came to. Never an error as a whole: every trouble is a line in it.
#[derive(Default)]
pub struct CategoriesResult {
    pub account: Id,
    /// The refresh token is the account's newest.
    pub tokens: Option<Tokens>,
    /// Whether the sign-in covers changing the list; `None` when the job couldn't tell.
    pub covered: Option<bool>,
    /// The server said this account may not change its list, with a sign-in that covers it.
    pub denied: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
    pub uploaded: Option<CategoriesUploaded>,
    pub download: Option<CategoriesDownload>,
    pub errors: Vec<String>,
}

/// The state of the categories' jobs, in the window.
#[derive(Default)]
pub struct CategorySync {
    /// The job under way: for which account, since when, the [`fingerprint`] of the refresh
    /// token it was given, and the answer to come.
    pub(crate) rx: Option<(Id, f64, u64, Receiver<CategoriesResult>)>,
    /// Which mailbox the job under way is for ([`mailcraft_engine::Session::openings`]), and the
    /// account's address then: its answer is for that mailbox and that address alone.
    opening: u64,
    address: String,
    /// The accounts a job has looked at in this session, and when.
    checked: HashMap<Id, f64>,
    /// The address each account had when it was last looked at: another address is another
    /// mailbox, whose list is looked at afresh (#661).
    addresses: HashMap<Id, String>,
    pub(crate) failures: HashMap<Id, (u32, f64)>,
    /// The time as the last look read it: a job's answer is taken in within a frame of it, so a
    /// job that came to nothing waits from when it ended, however long it ran (#661).
    pub(crate) clock: f64,
    pub(crate) perm: HashMap<Id, Permission>,
    settling: HashMap<Id, u32>,
    /// How many changes were waiting the last time, and since when.
    waiting: HashMap<Id, (usize, f64)>,
    /// How many changes wait per account, for the mailbox as of this revision, worked out at
    /// this time.
    pub(crate) ready: Option<(u64, f64, HashMap<Id, usize>)>,
}

impl CategorySync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
}

/// The Microsoft accounts whose own list of categories is kept in step with their server, in the
/// owner's order of the accounts, as the To Do jobs start.
pub(crate) fn category_accounts(mailbox: &Mailbox) -> Vec<Id> {
    let own = |a: &&Account| a.kind == AccountKind::Microsoft && !a.is_shared() && a.oauth.is_some();
    mailbox.accounts_in_order().into_iter().filter(own).filter(|a| mailbox.synced_categories.contains_key(&a.id)).map(|a| a.id).collect()
}

impl MailApp {
    /// Whether `account`'s list waits for the owner's permission, or for a new sign-in.
    fn categories_blocked(&self, account: Id) -> bool {
        match self.category_sync.perm.get(&account) {
            Some(Permission::Needed | Permission::Asking | Permission::Declined | Permission::Refused(_)) => true,
            Some(Permission::SignInOver(over)) => {
                !self.tokens.get(&account).is_some_and(|t| !t.refresh.is_empty() && fingerprint(&t.refresh) != *over)
            }
            _ => false,
        }
    }

    /// Look whether any account's list is due, and start a job when it is. Called every frame.
    /// (Public for a host's tests, which have no window.)
    pub fn categories_tick_at(&mut self, now: f64) -> bool {
        self.category_sync.clock = now;
        self.categories_ask_tick();
        // (Offline, nothing goes, so what would go isn't looked through either.)
        if !self.auto_sync || self.offline() || self.services.graph_categories.is_none() || self.category_sync.rx.is_some() {
            return false;
        }
        // One thing at a time: the calendar's, the to-dos' and the rules' jobs, a Send/Receive and
        // a sign-in go first.
        if self.calendar.rx.is_some()
            || self.tasks.busy()
            || self.rules.busy()
            || self.graph_rx.is_some()
            || self.sync_rx.is_some()
            || self.signing_in.is_some()
            || !self.sync_queue.is_empty()
        {
            return false;
        }
        for account in category_accounts(&self.session.mailbox) {
            // Another address: what was learnt of the old one's sign-in and list is not this one's.
            let address = self.session.mailbox.account(account).map(|a| a.email.trim().to_lowercase()).unwrap_or_default();
            if self.category_sync.addresses.get(&account).is_some_and(|was| *was != address) {
                self.category_sync.checked.remove(&account);
                self.category_sync.perm.remove(&account);
                self.category_sync.failures.remove(&account);
                self.category_sync.settling.remove(&account);
                self.category_sync.waiting.remove(&account);
            }
            self.category_sync.addresses.insert(account, address);
            if self.categories_blocked(account) {
                continue;
            }
            // A new sign-in: look again at once, to learn whether it may change the list.
            if matches!(self.category_sync.perm.get(&account), Some(Permission::SignInOver(_))) {
                self.category_sync.perm.remove(&account);
                self.category_sync.checked.remove(&account);
            }
            // Worked out again for a new state of the mailbox, at most once a second: while a
            // deletion waits for the items that carry it, that looks through the mail.
            let rev = self.session.rev();
            if self.category_sync.ready.as_ref().is_none_or(|(at, when, _)| *at != rev && now - *when >= RECOUNT_MS) {
                self.category_sync.ready = Some((rev, now, HashMap::new()));
            }
            let ready = match self.category_sync.ready.as_ref().and_then(|(_, _, m)| m.get(&account).copied()) {
                Some(n) => n,
                None => {
                    let n = categories_remote::categories_waiting(&self.session, account);
                    if let Some((_, _, m)) = &mut self.category_sync.ready {
                        m.insert(account, n);
                    }
                    n
                }
            };
            let since = match self.category_sync.waiting.get(&account) {
                Some((seen, since)) if *seen == ready => *since,
                _ => {
                    self.category_sync.waiting.insert(account, (ready, now));
                    now
                }
            };
            let pushes = ready > 0 && now - since >= QUIET_MS;
            // Once a session, to learn whether the list may be changed; then only for a change.
            if !pushes && self.category_sync.checked.contains_key(&account) {
                continue;
            }
            let (failures, at) = self.category_sync.failures.get(&account).copied().unwrap_or((0, 0.0));
            let patience = (FIRST_RETRY_MS * f64::from(1u32 << failures.saturating_sub(1).min(8))).min(LONGEST_RETRY_MS);
            if failures > 0 && now - at < patience {
                continue;
            }
            if self.start_categories_job(account, now) {
                return true;
            }
        }
        false
    }

    pub(crate) fn categories_tick(&mut self, ctx: &egui::Context) {
        self.categories_tick_at(now_ms());
        if self.category_sync.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.services.graph_categories.is_some() && self.auto_sync && !category_accounts(&self.session.mailbox).is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
    }

    fn start_categories_job(&mut self, account: Id, now: f64) -> bool {
        // (Offline, nothing goes, from here either.)
        if self.offline() || self.services.graph_categories.is_none() {
            return false;
        }
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        let Some(oauth) = owner.oauth.clone() else { return false };
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", owner.id)).unwrap_or_default();
        if refresh.is_empty() {
            // (Nothing to renew with: Send/Receive signs in first.)
            return false;
        }
        let Ok(upload) = categories_remote::category_upload(&self.session, account) else { return false };
        let given = fingerprint(&refresh);
        let job_address = owner.email.trim().to_lowercase();
        let job = CategoriesJob { account: owner, oauth: self.effective_oauth(oauth), refresh, upload, cancel: Default::default() };
        let Some(graph) = self.services.graph_categories.as_ref().filter(|_| !self.offline()) else { return false };
        self.category_sync.rx = Some((account, now, given, graph(job)));
        self.category_sync.opening = self.session.openings();
        self.category_sync.address = job_address;
        self.category_sync.checked.insert(account, now);
        true
    }

    /// The job answered: keep the refresh token, note whether the list may be changed, take in
    /// what the server made of the changes and what it has. (Public for a host's tests.)
    pub fn poll_categories(&mut self) {
        let Some((account, started, given, rx)) = &self.category_sync.rx else { return };
        let (account, started, given) = (*account, *started, *given);
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("categories: the job ended without an answer (its thread ended early)");
                CategoriesResult {
                    account,
                    errors: vec![tl!("The categories couldn't be kept in step: the job stopped unexpectedly").to_string()],
                    ..Default::default()
                }
            }
        };
        self.category_sync.rx = None;
        // What waits is worked out again at the next look, whatever the time.
        self.category_sync.ready = None;
        // For a mailbox that has been closed since: its ids may name other things in this one.
        let address = self.session.mailbox.account(account).map(|a| a.email.trim().to_lowercase());
        if self.category_sync.opening != self.session.openings() || address.as_deref() != Some(self.category_sync.address.as_str()) {
            log::info!("categories: the answer is for a mailbox that is no longer open, or another address; let go of");
            // (The account of the mailbox open now is still to be looked at.)
            self.category_sync.checked.remove(&account);
            return;
        }
        let mut errors = result.errors;
        if let Some(tokens) = &result.tokens
            && !tokens.refresh.is_empty()
            && !self.signed_in_as_other(account, tokens)
        {
            self.remember("refresh", account, &tokens.refresh);
            if let Some(mail) = self.tokens.get_mut(&account) {
                mail.refresh = tokens.refresh.clone();
            }
        }
        let settling = self.category_sync.settling.get(&account).copied().unwrap_or(0);
        if result.covered == Some(true) {
            self.category_sync.settling.remove(&account);
        }
        let mut writes = None;
        let mut settles = false;
        if result.denied {
            let said = tl!("This account may not change its categories through Microsoft Graph").to_string();
            self.category_sync.perm.insert(account, Permission::Refused(said));
            writes = Some(false);
        } else if result.covered == Some(false) && settling > 0 {
            log::info!("categories: refused just after the owner allowed it; trying again ({settling} more)");
            self.category_sync.settling.insert(account, settling - 1);
            let (n, _) = self.category_sync.failures.get(&account).copied().unwrap_or((0, 0.0));
            self.category_sync.failures.insert(account, (n.saturating_add(1), self.category_sync.clock.max(started)));
            self.category_sync.checked.remove(&account);
            settles = true;
        } else if result.covered == Some(false) {
            log::info!("categories: the sign-in doesn't cover changing them; the list is read-only");
            if !matches!(self.category_sync.perm.get(&account), Some(Permission::Asking | Permission::Declined | Permission::Refused(_))) {
                self.category_sync.perm.insert(account, Permission::Needed);
            }
            writes = Some(false);
        } else if result.sign_in_refused && result.tokens.is_none() && settling == 0 {
            log::warn!("categories: the sign-in was refused; waiting for a new one");
            self.category_sync.perm.insert(account, Permission::SignInOver(given));
        } else if result.sign_in_refused && result.tokens.is_none() {
            // Just after the owner allowed it, as a refusal of the permission is: tried again
            // after the wait a failure has, a try fewer each time.
            log::info!("categories: the sign-in was refused just after the owner allowed it; trying again ({settling} more)");
            self.category_sync.settling.insert(account, settling - 1);
            let (n, _) = self.category_sync.failures.get(&account).copied().unwrap_or((0, 0.0));
            self.category_sync.failures.insert(account, (n.saturating_add(1), self.category_sync.clock.max(started)));
            self.category_sync.checked.remove(&account);
            settles = true;
        } else if result.covered == Some(true) {
            self.category_sync.perm.insert(account, Permission::Given);
            writes = Some(true);
        }
        if let Some(allowed) = writes
            && let Err(e) = categories_remote::set_category_writes(&mut self.session, account, allowed)
        {
            log::warn!("categories: whether the list may be changed couldn't be noted: {e}");
        }
        // (A token the server refused part-way, after it was renewed, is tried again after a wait.)
        let mut worked = result.covered.is_some() && !result.sign_in_refused;
        if let Some(uploaded) = &result.uploaded {
            // A change the server couldn't take now (busy, or changed there meanwhile) goes again
            // after the wait, as a job that came to nothing does.
            if uploaded.outcomes.iter().any(|o| o.result == mailcraft_model::remote::CategoryResult::Failed) {
                worked = false;
            }
            match self.session.mailbox.account(account).cloned() {
                Some(owner) => match categories_remote::apply_category_changes(&mut self.session, &uploaded.clone().for_account(&owner)) {
                    Ok((said, news)) => {
                        errors.extend(said);
                        // (A name Outlook had already is no problem: the owner is told it in the
                        // status bar.)
                        if !news.is_empty() {
                            self.status(news.join(" "));
                        }
                    }
                    Err(e) => {
                        errors.push(crate::i18n::fmt(tl!("The categories sent couldn't be noted: {error}"), &[("error", &e.to_string())]));
                        worked = false;
                    }
                },
                None => worked = false,
            }
        }
        if let Some(download) = &result.download
            && let Err(e) = mailcraft_engine::cmd::remote::apply_categories(&mut self.session, download)
        {
            errors.push(crate::i18n::fmt(tl!("The categories couldn't be taken in: {error}"), &[("error", &e.to_string())]));
            worked = false;
        }
        if settles {
            // (Tried again after the wait noted above.)
        } else if worked && errors.is_empty() {
            self.category_sync.failures.remove(&account);
        } else if (result.tokens.is_some() || !result.sign_in_refused) && result.covered != Some(false) {
            let (n, _) = self.category_sync.failures.get(&account).copied().unwrap_or((0, 0.0));
            self.category_sync.failures.insert(account, (n.saturating_add(1), self.category_sync.clock.max(started)));
            // (Tried again later: a job that came to nothing has told nothing.)
            if !worked {
                self.category_sync.checked.remove(&account);
            }
        }
        self.note_sync_problems(account, &errors, false);
    }

    /// Ask the owner, once, when a job found the sign-in doesn't cover changing the list: the one
    /// question. Not while another dialog is up, File is open or the window works offline.
    fn categories_ask_tick(&mut self) {
        self.sync_ask_tick();
        if self.dialog.is_some() || self.ui.backstage || self.signing_in.is_some() || self.offline() {
            return;
        }
        if let Some(account) = self.category_sync.perm.iter().find(|(_, p)| **p == Permission::Needed).map(|(a, _)| *a) {
            self.ask_sync(account);
        }
    }

    /// The one question is put ([`crate::server_sync`]).
    pub(crate) fn categories_sync_asking(&mut self, account: Id) {
        if matches!(self.category_sync.perm.get(&account), Some(Permission::Needed | Permission::Declined | Permission::Refused(_))) {
            self.category_sync.perm.insert(account, Permission::Asking);
        }
    }

    /// The one question went away unanswered: the list stays read-only, and nobody is asked again
    /// by itself (the Categories window has Allow, File › Info Ask again).
    pub(crate) fn categories_sync_not_now(&mut self, account: Id) {
        if matches!(self.category_sync.perm.get(&account), Some(Permission::Needed | Permission::Asking)) {
            self.category_sync.perm.insert(account, Permission::Declined);
        }
    }

    /// The page signed in somebody else: the list waits as it did.
    pub(crate) fn categories_sync_waits(&mut self, account: Id) {
        if self.category_sync.perm.get(&account) == Some(&Permission::Asking) {
            self.category_sync.perm.insert(account, Permission::Needed);
        }
    }

    /// Asked, and no: why, in a few words.
    pub(crate) fn categories_sync_refused(&mut self, account: Id, why: &str) {
        if matches!(self.category_sync.perm.get(&account), Some(Permission::Needed | Permission::Asking | Permission::Declined)) {
            self.category_sync.perm.insert(account, Permission::Refused(why.to_string()));
        }
    }

    /// The owner allowed it on Microsoft's page: the next look finds out at once.
    pub(crate) fn categories_allowed(&mut self, account: Id) {
        self.category_sync.perm.remove(&account);
        self.category_sync.settling.insert(account, SETTLING_TRIES);
        self.category_sync.failures.remove(&account);
        self.category_sync.checked.remove(&account);
    }

    /// For a stand-in host (the screenshot example): every Microsoft account's list may be
    /// changed (`true`), or none may and the owner was asked (`false`).
    #[doc(hidden)]
    pub fn stand_in_categories(&mut self, allowed: bool) {
        for account in category_accounts(&self.session.mailbox) {
            let perm = if allowed { Permission::Given } else { Permission::Declined };
            self.category_sync.perm.insert(account, perm);
            let _ = categories_remote::set_category_writes(&mut self.session, account, allowed);
        }
    }

    /// What the Categories window says over a Microsoft account's list that may not be changed,
    /// and whether it offers Allow; `None` for a list that may be changed, or isn't kept in step
    /// with a server.
    pub(crate) fn categories_read_only_line(&self, account: Id) -> Option<(String, bool)> {
        let owner = self.session.mailbox.categories_read_only(account)?;
        let address = self.session.mailbox.account(owner).map(|a| a.email.trim().to_string()).unwrap_or_default();
        let shown = |why: &str| crate::forms::kit::shown(why, crate::server_sync::REASON_SHOWN);
        let line = |text: &str| crate::i18n::fmt(text, &[("address", &address)]);
        // (Allow only where there is a host to sign in with: never in the browser.)
        let host = self.services.graph_categories.is_some();
        let (said, allow) = match self.category_sync.perm.get(&owner) {
            // No job has said yet in this session (the host has none in the browser).
            None if self.services.graph_categories.is_some() && !self.offline() => {
                (line(tl!("Categories from {address}'s server. Checking whether MailCraft may change them…")), false)
            }
            Some(Permission::Asking) => (line(tl!("Categories from {address}'s server. Asking for your permission to change them…")), false),
            Some(Permission::Refused(why)) => (
                crate::i18n::fmt(
                    tl!("Categories from {address}'s server. No permission to change them ({why})"),
                    &[("address", &address), ("why", shown(why).as_str())],
                ),
                true,
            ),
            Some(Permission::SignInOver(_)) => {
                (line(tl!("Categories from {address}'s server. Microsoft no longer takes the sign-in, so they can't be changed")), true)
            }
            _ => (line(tl!("Categories from {address}'s server. No permission to change them")), true),
        };
        Some((said, allow && host))
    }

    /// The categories' part of File › Info's line about keeping in step with Outlook.
    pub(crate) fn categories_sync_part(&self, account: Id) -> Option<(String, bool, crate::server_sync::Perm)> {
        use crate::server_sync::Perm;
        self.services.graph_categories.as_ref()?;
        if !category_accounts(&self.session.mailbox).contains(&account) {
            return None;
        }
        let shown = |why: &str| crate::forms::kit::shown(why, crate::server_sync::REASON_SHOWN);
        Some(match self.category_sync.perm.get(&account) {
            Some(Permission::Given) => (tl!("Categories: kept in step with Outlook").to_string(), false, Perm::Fine),
            Some(Permission::Needed | Permission::Declined) => {
                (tl!("Categories: read-only here (MailCraft may not change them)").to_string(), true, Perm::Waits)
            }
            Some(Permission::Asking) => (tl!("Categories: asking for your permission…").to_string(), false, Perm::Asking),
            Some(Permission::Refused(why)) => {
                (crate::i18n::fmt(tl!("Categories: read-only here ({why})"), &[("why", shown(why).as_str())]), true, Perm::Waits)
            }
            Some(Permission::SignInOver(_)) => (tl!("Categories: Microsoft no longer takes the sign-in").to_string(), true, Perm::Waits),
            // (Offline, or before the first look: a list MailCraft may not change yet is read-only
            // until it is known.)
            None if self.session.mailbox.categories_read_only(account).is_some() => {
                (tl!("Categories: read-only here until MailCraft can check whether it may change them").to_string(), false, Perm::Waits)
            }
            None => (tl!("Categories: checking…").to_string(), false, Perm::Fine),
        })
    }
}
