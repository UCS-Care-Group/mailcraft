// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules kept on a Microsoft account's server, in the window (#566).
//!
//! A job of its own, like the to-dos' ([`crate::task_sync`]): when an account's rules are due
//! (every few minutes, a moment after a change here) the window hands the host a [`RulesJob`]
//! with what is to be told to the server (`sync.ruleChanges`); the host tells it, then reads the
//! server's rules whole, and the window takes in what the server made of the changes
//! (`sync.applyRuleChanges`) and then what it has (`sync.applyRules`). The merge is the engine's
//! ([`mailcraft_engine::cmd::rules_remote`]).
//!
//! **The permission.** Reading the server's rules takes `MailboxSettings.Read`, which a sign-in
//! asks for; changing them takes `MailboxSettings.ReadWrite`, which a new sign-in's page asks for
//! beside the rest, as an extra a page that won't have it is asked again without (#661). For an
//! account that signed in before, the first job that has something to send finds its sign-in
//! doesn't cover it ([`RulesResult::consent_needed`]): nothing of the rules is sent, and the window
//! asks the owner the one question of [`crate::server_sync`], then Microsoft's page. Until it is
//! allowed the account's rules stay on this computer and run here, and the server's are still read
//! and shown: only the changes wait. File › Info says where it stands, with Ask again.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::rules_remote;
use mailcraft_model::remote_rules::{RulesDownload, RulesUpload, RulesUploaded};
use mailcraft_model::{Account, Id};

use crate::task_sync::fingerprint;
use crate::{MailApp, Tokens, now_ms};

/// How often an account's rules are read when nothing is waiting to go, how long a change is
/// left to settle before it goes (milliseconds), and the wait after a job that failed.
const REFRESH_MS: f64 = 5.0 * 60_000.0;
const QUIET_MS: f64 = 3_000.0;
const FIRST_RETRY_MS: f64 = 30_000.0;
const LONGEST_RETRY_MS: f64 = 15.0 * 60_000.0;
/// How many times a refusal right after the owner allowed changing the rules is taken for
/// Microsoft not having taken it in yet (#491).
const SETTLING_TRIES: u32 = 3;

/// What the host is asked to do for one account's rules ([`crate::Services::graph_rules`]).
pub struct RulesJob {
    pub account: Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: mailcraft_model::OAuth,
    /// The account's refresh token: the job gets its own token with it.
    pub refresh: String,
    /// What the server is to be told first. Empty for a job that only reads: nothing waits, or
    /// the owner hasn't allowed changing the rules on the server. Only a job with something to
    /// send asks for the permission to change them.
    pub upload: RulesUpload,
    /// What the account's mail calls its folders: a rule that names one otherwise has it
    /// looked up.
    pub folders: Vec<String>,
    pub cancel: Arc<AtomicBool>,
}

/// What a [`RulesJob`] came to. Never an error as a whole: every trouble is a line in the answer.
#[derive(Default)]
pub struct RulesResult {
    pub account: Id,
    /// The refresh token is the account's newest.
    pub tokens: Option<Tokens>,
    /// There were changes to send and the sign-in doesn't cover changing the rules: nothing was
    /// sent, and the owner is to be asked. (The server's rules were still read.)
    pub consent_needed: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
    /// The server said this account may not change its rules.
    pub denied: bool,
    /// The server's rules can't be read with this sign-in (it doesn't cover reading the
    /// mailbox's settings).
    pub unreadable: bool,
    pub uploaded: Option<RulesUploaded>,
    pub download: Option<RulesDownload>,
    pub errors: Vec<String>,
}

/// Where an account's permission to change its rules on the server stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    /// A job changed them with it.
    Given,
    /// A job found the sign-in doesn't cover it: the owner is to be asked.
    Needed,
    /// The owner is being asked: the question, or Microsoft's page.
    Asking,
    /// The question went away unanswered, or Not now: not asked again by itself.
    Declined,
    /// Asked, and no: why, in a few words.
    Refused(String),
    /// Microsoft no longer took the sign-in the job had (its refresh token's [`fingerprint`]).
    SignInOver(u64),
}

/// The state of the rules jobs, in the window.
#[derive(Default)]
pub struct RuleSync {
    /// The job under way: for which account, since when, the [`fingerprint`] of the refresh
    /// token it was given, and the answer to come.
    rx: Option<(Id, f64, u64, Receiver<RulesResult>)>,
    /// When each account's rules were last asked for, and the failures since the last answer.
    checked: HashMap<Id, f64>,
    failures: HashMap<Id, (u32, f64)>,
    /// The time as the last look read it: a job's answer is taken in within a frame of it, so a
    /// job that came to nothing waits from when it ended, however long it ran (#661).
    pub(crate) clock: f64,
    pub(crate) perm: HashMap<Id, Permission>,
    settling: HashMap<Id, u32>,
    /// Which mailbox the job under way is for ([`Session::openings`]): its answer is for that one
    /// alone, and is let go of when another has been opened since. And what it was to send.
    opening: u64,
    sending: RulesUpload,
    /// How many changes were waiting the last time, and since when.
    waiting: HashMap<Id, (usize, f64)>,
    /// How many changes wait per account, worked out for the mailbox as of this revision.
    ready: Option<(u64, HashMap<Id, usize>)>,
    /// The accounts whose server's rules have been read, and those whose can't be.
    read: HashSet<Id>,
    unreadable: HashSet<Id>,
}

impl RuleSync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
}

impl MailApp {
    /// Whether changes to the rules of `account` wait for the owner's permission. The server's
    /// rules are read all the same.
    pub(crate) fn rules_held(&self, account: Id) -> bool {
        matches!(self.rules.perm.get(&account), Some(Permission::Needed | Permission::Asking | Permission::Declined | Permission::Refused(_)))
    }

    /// Whether the sign-in the last job had is over, and there is no newer one.
    fn rules_signed_out(&self, account: Id) -> bool {
        match self.rules.perm.get(&account) {
            Some(Permission::SignInOver(over)) => {
                !self.tokens.get(&account).is_some_and(|t| !t.refresh.is_empty() && fingerprint(&t.refresh) != *over)
            }
            _ => false,
        }
    }

    /// Look whether any account's rules are due, and start a job when they are. Called every
    /// frame. (Public for a host's tests, which have no window.)
    pub fn rules_tick_at(&mut self, now: f64) -> bool {
        self.rules.clock = now;
        self.rules_ask_tick();
        if !self.auto_sync || self.services.graph_rules.is_none() || self.rules.rx.is_some() {
            return false;
        }
        // One thing at a time: the calendar's job, the to-dos', the automatic replies', a
        // Send/Receive and a sign-in go first (the jobs renew the same sign-in).
        if self.calendar.rx.is_some()
            || self.tasks.busy()
            || self.auto_replies.busy()
            || self.graph_rx.is_some()
            || self.sync_rx.is_some()
            || self.signing_in.is_some()
            || !self.sync_queue.is_empty()
        {
            return false;
        }
        for account in rules_remote::rule_accounts(&self.session.mailbox) {
            if self.rules_signed_out(account) {
                continue;
            }
            if matches!(self.rules.perm.get(&account), Some(Permission::SignInOver(_))) {
                self.rules.perm.remove(&account);
            }
            let writes = !self.rules_held(account);
            let rev = self.session.rev();
            if self.rules.ready.as_ref().is_none_or(|(at, _)| *at != rev) {
                self.rules.ready = Some((rev, HashMap::new()));
            }
            let ready = match self.rules.ready.as_ref().and_then(|(_, m)| m.get(&account).copied()) {
                Some(n) => n,
                None => {
                    let n = rules_remote::rules_waiting(&self.session.mailbox, account);
                    if let Some((_, m)) = &mut self.rules.ready {
                        m.insert(account, n);
                    }
                    n
                }
            };
            // What waits for a permission not given doesn't make a job: the reads come as ever.
            let ready = if writes { ready } else { 0 };
            let since = match self.rules.waiting.get(&account) {
                Some((seen, since)) if *seen == ready => *since,
                _ => {
                    self.rules.waiting.insert(account, (ready, now));
                    now
                }
            };
            let pushes = ready > 0 && now - since >= QUIET_MS;
            let asked = self.rules.checked.get(&account).copied();
            if !(pushes || asked.is_none_or(|at| now - at >= REFRESH_MS)) {
                continue;
            }
            let (failures, at) = self.rules.failures.get(&account).copied().unwrap_or((0, 0.0));
            let patience = (FIRST_RETRY_MS * f64::from(1u32 << failures.saturating_sub(1).min(8))).min(LONGEST_RETRY_MS);
            if failures > 0 && now - at < patience {
                continue;
            }
            if self.start_rules_job(account, writes, now) {
                return true;
            }
        }
        false
    }

    pub(crate) fn rules_tick(&mut self, ctx: &egui::Context) {
        self.rules_tick_at(now_ms());
        if self.rules.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.services.graph_rules.is_some() && self.auto_sync && !rules_remote::rule_accounts(&self.session.mailbox).is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
    }

    fn start_rules_job(&mut self, account: Id, writes: bool, now: f64) -> bool {
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        let Some(oauth) = owner.oauth.clone() else { return false };
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", owner.id)).unwrap_or_default();
        if refresh.is_empty() {
            // (Nothing to renew with: Send/Receive signs in first.)
            return false;
        }
        let upload = if writes {
            let Ok(upload) = rules_remote::rule_upload(&self.session.mailbox, account) else { return false };
            upload
        } else {
            RulesUpload { account, email: owner.email.trim().to_string(), ops: Vec::new() }
        };
        let folders = self
            .session
            .mailbox
            .folders
            .iter()
            .filter(|f| f.account == account)
            .filter_map(|f| f.server.as_ref().map(|s| s.id.clone()))
            .filter(|id| !id.is_empty())
            .collect();
        let given = fingerprint(&refresh);
        let sent = upload.clone();
        let job = RulesJob { account: owner, oauth: self.effective_oauth(oauth), refresh, upload, folders, cancel: Default::default() };
        let Some(graph) = self.services.graph_rules.as_ref().filter(|_| !self.offline()) else { return false };
        let rx = graph(job);
        // What goes to be made is not sent again until an answer, or the server's rules read
        // whole, say what became of it: an answer lost would make it twice.
        rules_remote::mark_sent(&mut self.session, &sent);
        self.rules.sending = sent;
        self.rules.rx = Some((account, now, given, rx));
        self.rules.opening = self.session.openings();
        self.rules.checked.insert(account, now);
        true
    }

    /// The job answered: keep the refresh token, take in what the server made of the changes, and
    /// what it has. (Public for a host's tests.)
    pub fn poll_rules(&mut self) {
        let Some((account, started, given, rx)) = &self.rules.rx else { return };
        let (account, started, given) = (*account, *started, *given);
        let (result, ended_early) = match rx.try_recv() {
            Ok(result) => (result, false),
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("rules: the job ended without an answer (its thread ended early)");
                let errors = vec![tl!("The rules on the server couldn't be read: the job stopped unexpectedly").to_string()];
                (RulesResult { account, errors, ..Default::default() }, true)
            }
        };
        self.rules.rx = None;
        let sending = std::mem::take(&mut self.rules.sending);
        // For a mailbox that has been closed since: its ids may name other things in this one.
        if self.rules.opening != self.session.openings() {
            log::info!("rules: the answer is for a mailbox that is no longer open; let go of");
            return;
        }
        // What was to be made and went nowhere goes with the next job: all of it when nothing went
        // out (no sign-in that covers it, or none at all), and else what the job stopped before. A
        // job that ended without a word may have sent it.
        if !ended_early {
            let told = result.uploaded.as_ref().map_or(&[][..], |u| u.outcomes.as_slice());
            rules_remote::not_sent(&mut self.session, &sending, told);
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
        let settling = self.rules.settling.get(&account).copied().unwrap_or(0);
        if !result.consent_needed {
            self.rules.settling.remove(&account);
        }
        if result.consent_needed && settling > 0 {
            log::info!("rules: refused just after the owner allowed them; trying again ({settling} more)");
            self.rules.settling.insert(account, settling - 1);
            let (n, _) = self.rules.failures.get(&account).copied().unwrap_or((0, 0.0));
            self.rules.failures.insert(account, (n.saturating_add(1), self.rules.clock.max(started)));
            self.rules.checked.remove(&account);
        } else if result.consent_needed {
            log::info!("rules: the sign-in doesn't cover changing the rules; the owner is to be asked");
            if !matches!(self.rules.perm.get(&account), Some(Permission::Asking | Permission::Declined | Permission::Refused(_))) {
                self.rules.perm.insert(account, Permission::Needed);
            }
        } else if result.denied {
            let said = tl!("This account may not change its rules on the server").to_string();
            self.rules.perm.insert(account, Permission::Refused(said));
        } else if result.sign_in_refused && result.tokens.is_none() && settling == 0 {
            log::warn!("rules: the sign-in was refused; waiting for a new one");
            if !matches!(self.rules.perm.get(&account), Some(Permission::Asking | Permission::Declined | Permission::Refused(_))) {
                self.rules.perm.insert(account, Permission::SignInOver(given));
            }
        } else if result.sign_in_refused {
            log::warn!("rules: the server refused the sign-in; trying again later");
        } else if result.uploaded.is_some() {
            self.rules.perm.insert(account, Permission::Given);
        }
        if result.unreadable {
            self.rules.unreadable.insert(account);
        } else if result.download.is_some() {
            self.rules.unreadable.remove(&account);
            self.rules.read.insert(account);
        }
        let mut worked = false;
        // (The host names the account it was for, address and all, which the engine checks.)
        if let Some(uploaded) = &result.uploaded {
            match rules_remote::apply_rule_changes(&mut self.session, uploaded) {
                Ok(merged) => {
                    errors.extend(merged.messages);
                    worked = true;
                }
                Err(e) => errors.push(crate::i18n::fmt(tl!("The rules sent couldn't be noted: {error}"), &[("error", &e.to_string())])),
            }
        }
        if let Some(download) = &result.download {
            match rules_remote::apply_rules(&mut self.session, download) {
                Ok(merged) => {
                    errors.extend(merged.messages);
                    worked = true;
                }
                Err(e) => errors.push(crate::i18n::fmt(tl!("The rules on the server couldn't be taken in: {error}"), &[("error", &e.to_string())])),
            }
        }
        if worked && errors.is_empty() {
            self.rules.failures.remove(&account);
        } else if !result.consent_needed && !result.sign_in_refused && !result.unreadable {
            let (n, _) = self.rules.failures.get(&account).copied().unwrap_or((0, 0.0));
            self.rules.failures.insert(account, (n.saturating_add(1), self.rules.clock.max(started)));
        }
        self.note_problems_from("rules", account, &errors, None, false);
    }

    /// Ask the owner, once, when a job had a rule to send and the sign-in doesn't cover
    /// changing the rules: a question first, then Microsoft's page. Not while another dialog is
    /// up or File is open.
    fn rules_ask_tick(&mut self) {
        // (A question that went away unanswered is a no: `sync_ask_tick`.)
        self.sync_ask_tick();
        // (Working offline, Microsoft's page can't be had: the question waits.)
        if self.dialog.is_some() || self.ui.backstage || self.signing_in.is_some() || self.offline() {
            return;
        }
        if let Some(account) = self.rules.perm.iter().find(|(_, p)| **p == Permission::Needed).map(|(a, _)| *a) {
            self.ask_sync(account);
        }
    }

    /// For a stand-in host (the screenshot example): every account that keeps rules on its
    /// server has them working, as if a job had changed them.
    #[doc(hidden)]
    pub fn stand_in_rules_work(&mut self) {
        for account in rules_remote::rule_accounts(&self.session.mailbox) {
            self.rules.perm.insert(account, Permission::Given);
            self.rules.read.insert(account);
        }
    }

    /// The one question is put ([`crate::server_sync`]): the rules that wait for it are being
    /// asked about.
    pub(crate) fn rules_sync_asking(&mut self, account: Id) {
        if matches!(self.rules.perm.get(&account), Some(Permission::Needed | Permission::Declined | Permission::Refused(_))) {
            self.rules.perm.insert(account, Permission::Asking);
        }
    }

    /// The one question went away unanswered: the rules stay here, and nobody is asked again by
    /// itself (File › Info has Ask again).
    pub(crate) fn rules_sync_not_now(&mut self, account: Id) {
        if matches!(self.rules.perm.get(&account), Some(Permission::Needed | Permission::Asking)) {
            self.rules.perm.insert(account, Permission::Declined);
        }
    }

    /// The page signed in somebody else: the rules wait as they did.
    pub(crate) fn rules_sync_waits(&mut self, account: Id) {
        if self.rules.perm.get(&account) == Some(&Permission::Asking) {
            self.rules.perm.insert(account, Permission::Needed);
        }
    }

    /// Asked, and no: why, in a few words.
    pub(crate) fn rules_sync_refused(&mut self, account: Id, why: &str) {
        if matches!(self.rules.perm.get(&account), Some(Permission::Needed | Permission::Asking | Permission::Declined)) {
            self.rules.perm.insert(account, Permission::Refused(why.to_string()));
        }
    }

    /// The rules' part of File › Info's line about keeping in step with Outlook.
    pub(crate) fn rules_sync_part(&self, account: Id) -> Option<(String, bool, crate::server_sync::Perm)> {
        use crate::server_sync::Perm;
        let (line, again) = self.rules_line(account)?;
        let perm = match self.rules.perm.get(&account) {
            Some(Permission::Asking) => Perm::Asking,
            Some(Permission::Given) | None => Perm::Fine,
            Some(_) => Perm::Waits,
        };
        Some((line, again, perm))
    }

    /// The owner allowed changing the mailbox's settings on Microsoft's page, for the rules or for
    /// the automatic replies: one permission for both, so the rules that waited for it go with the
    /// next job.
    pub(crate) fn rules_allowed(&mut self, account: Id) {
        self.rules.perm.remove(&account);
        self.rules.settling.insert(account, SETTLING_TRIES);
        self.rules.failures.remove(&account);
        self.rules.checked.remove(&account);
    }

    /// Why a rule of `account` that is to go to the server is still only on this computer, as the
    /// Rules window says it.
    pub(crate) fn rules_wait_reason(&self, account: Id) -> String {
        match self.rules.perm.get(&account) {
            Some(Permission::Needed | Permission::Asking) => tl!("waiting for your permission to change the rules on the server").to_string(),
            Some(Permission::Declined) => tl!("not allowed on the server yet: File › Info › Ask again").to_string(),
            Some(Permission::Refused(why)) => why.clone(),
            Some(Permission::SignInOver(_)) => tl!("Microsoft no longer takes the sign-in").to_string(),
            Some(Permission::Given) | None => tl!("it goes to the server at the next connection").to_string(),
        }
    }

    /// What File › Info says under an account's card about its rules, and whether there is a way
    /// to ask again. `None` for an account whose server keeps no rules.
    pub(crate) fn rules_line(&self, account: Id) -> Option<(String, bool)> {
        self.services.graph_rules.as_ref()?;
        if !rules_remote::rule_accounts(&self.session.mailbox).contains(&account) {
            return None;
        }
        let shown = |why: &str| crate::forms::kit::shown(why, crate::server_sync::REASON_SHOWN);
        Some(match self.rules.perm.get(&account) {
            Some(Permission::Given) => (tl!("Rules: kept on the server").to_string(), false),
            Some(Permission::Needed) => (tl!("Rules: waiting for your permission to keep them on the server").to_string(), true),
            Some(Permission::Asking) => (tl!("Rules: asking for your permission…").to_string(), false),
            Some(Permission::Declined) => (tl!("Rules: not allowed on the server yet; new ones and changes stay on this computer").to_string(), true),
            Some(Permission::Refused(why)) => {
                (crate::i18n::fmt(tl!("Rules: not allowed on the server ({why})"), &[("why", shown(why).as_str())]), true)
            }
            Some(Permission::SignInOver(_)) => (tl!("Rules: Microsoft no longer takes the sign-in").to_string(), true),
            None if self.rules.unreadable.contains(&account) => (tl!("Rules: the server's rules can't be read with this sign-in").to_string(), false),
            None if self.rules.read.contains(&account) => (tl!("Rules: the server's are shown").to_string(), false),
            None => (tl!("Rules: not read from the server yet").to_string(), false),
        })
    }
}
