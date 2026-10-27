// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks kept in step with Microsoft To Do in the window (#248), for the accounts that sign in
//! with Microsoft.
//!
//! **Local to-dos.** Those that exist only on this computer are never sent unasked: when an
//! account's To Do works and there are such to-dos, the window asks once ([`MailApp::offer_local_tasks`]);
//! Not now keeps them here, and File › Info offers it again.
//!
//! A job of its own, like the calendar's: when the to-dos are due (every few minutes, a moment
//! after a change here) the window hands the host a [`TasksJob`] with what is to be told to the
//! server (`sync.taskChanges`); the host tells it, then reads the lists and tasks whole, and the
//! window takes in what the server made of the changes (`sync.applyTaskChanges`) and then what it
//! has (`sync.applyTasks`). The merge is the engine's ([`mailcraft_engine::cmd::tasks_remote`]).
//!
//! **The permission.** Reading and changing the lists takes `Tasks.ReadWrite`. No sign-in page
//! asks for it (a mail sign-in must never wait on it, a tenant may need an administrator for it): the
//! first job finds its sign-in doesn't cover it ([`TasksResult::consent_needed`]), nothing of the
//! to-dos is read or sent, mail and calendar go on as ever, and the window asks the owner, by
//! itself, with a question first and then Microsoft's page ([`MailApp::ask_tasks`]). File › Info
//! says where it stands, with a way to ask again. A no that is not the owner's to change is said
//! as for the Favorites ([`crate::favorites_sync::NEEDS_APPROVAL`]…).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::tasks_remote;
use mailcraft_model::remote_tasks::{TasksDownload, TasksUpload, TasksUploaded};
use mailcraft_model::{Account, Id, LocalTasks};
use serde_json::{Value, json};

use crate::dialogs::{Dialog, Question};
use crate::{MailApp, Tokens, now_ms};

/// How often an account's to-dos are read when nothing is waiting to go, and how long a change
/// is left to settle before it goes (milliseconds), and the wait after a job that failed.
const REFRESH_MS: f64 = 5.0 * 60_000.0;
const QUIET_MS: f64 = 3_000.0;
const FIRST_RETRY_MS: f64 = 30_000.0;
const LONGEST_RETRY_MS: f64 = 15.0 * 60_000.0;
/// How many times a job tries again, quietly, when the to-dos are refused right after the owner
/// allowed them (#491): a permission given a moment ago can be refused until Microsoft has taken
/// it in everywhere. With [`FIRST_RETRY_MS`] doubling, the last try is some minutes after.
const SETTLING_TRIES: u32 = 3;

/// What the host is asked to do for one account's to-dos ([`crate::Services::graph_tasks`]).
pub struct TasksJob {
    pub account: Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: mailcraft_model::OAuth,
    /// The account's refresh token: the job gets its own token for the to-dos with it.
    pub refresh: String,
    /// What the server is to be told first.
    pub upload: TasksUpload,
    pub cancel: Arc<AtomicBool>,
}

/// What a [`TasksJob`] came to. Never an error as a whole: every trouble is a line in the answer.
#[derive(Default)]
pub struct TasksResult {
    pub account: Id,
    /// The refresh token is the account's newest.
    pub tokens: Option<Tokens>,
    /// The sign-in doesn't cover `Tasks.ReadWrite`: the owner is to be asked.
    pub consent_needed: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
    /// The server said this account may not use its to-dos.
    pub denied: bool,
    pub uploaded: Option<TasksUploaded>,
    pub download: Option<TasksDownload>,
    pub errors: Vec<String>,
}

/// Where an account's permission for its to-dos stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    /// A job read them with it.
    Given,
    /// A job found the sign-in doesn't cover it: the owner is to be asked.
    Needed,
    /// The owner is being asked: the question, or Microsoft's page.
    Asking,
    /// The question went away unanswered: not asked again by itself.
    Declined,
    /// Asked, and no: why, in a few words.
    Refused(String),
    /// Microsoft no longer took the sign-in the job had (its refresh token's [`fingerprint`]),
    /// for mail either (#520). Ask again (File › Info) is one way back; a new sign-in for mail is
    /// the other, and the next look runs a job again by itself.
    SignInOver(u64),
}

/// The state of the to-do jobs, in the window.
#[derive(Default)]
pub struct TaskSync {
    /// The job under way: for which account, since when, the [`fingerprint`] of the refresh
    /// token it was given, and the answer to come.
    rx: Option<(Id, f64, u64, Receiver<TasksResult>)>,
    /// When each account's to-dos were last asked for, and the failures since the last answer.
    checked: HashMap<Id, f64>,
    failures: HashMap<Id, (u32, f64)>,
    pub(crate) perm: HashMap<Id, Permission>,
    /// The owner allowed the to-dos on Microsoft's page: how many refusals are still taken for
    /// Microsoft not having taken it in yet, rather than for a no.
    settling: HashMap<Id, u32>,
    /// How many changes were waiting the last time, and since when.
    waiting: HashMap<Id, (usize, f64)>,
    /// The question about the to-dos that exist only here is on screen.
    pub(crate) offer_open: bool,
    /// How many changes wait per account, worked out for the mailbox as of this revision (every
    /// frame asks; the work is done again only when the mailbox changed).
    ready: Option<(u64, HashMap<Id, usize>)>,
}

impl TaskSync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
}

impl MailApp {
    /// Whether the to-dos of `account` wait for the owner's permission, or for a new sign-in.
    fn tasks_blocked(&self, account: Id) -> bool {
        match self.tasks.perm.get(&account) {
            Some(Permission::Needed | Permission::Asking | Permission::Declined | Permission::Refused(_)) => true,
            Some(Permission::SignInOver(over)) => {
                !self.tokens.get(&account).is_some_and(|t| !t.refresh.is_empty() && fingerprint(&t.refresh) != *over)
            }
            _ => false,
        }
    }

    /// Look whether any account's to-dos are due, and start a job when they are. Called every
    /// frame. (Public for a host's tests, which have no window.)
    pub fn tasks_tick_at(&mut self, now: f64) -> bool {
        self.tasks_ask_tick();
        if !self.auto_sync || self.services.graph_tasks.is_none() || self.tasks.rx.is_some() {
            return false;
        }
        // One thing at a time: the calendar's job, a Send/Receive and a sign-in go first.
        if self.calendar.rx.is_some() || self.graph_rx.is_some() || self.sync_rx.is_some() || self.signing_in.is_some() || !self.sync_queue.is_empty()
        {
            return false;
        }
        for account in tasks_remote::task_accounts(&self.session.mailbox) {
            if self.tasks_blocked(account) {
                continue;
            }
            if matches!(self.tasks.perm.get(&account), Some(Permission::SignInOver(_))) {
                // Signed in again since (for mail, say): a job finds out where the to-dos stand.
                self.tasks.perm.remove(&account);
            }
            let rev = self.session.rev();
            if self.tasks.ready.as_ref().is_none_or(|(at, _)| *at != rev) {
                self.tasks.ready = Some((rev, HashMap::new()));
            }
            let ready = match self.tasks.ready.as_ref().and_then(|(_, m)| m.get(&account).copied()) {
                Some(n) => n,
                None => {
                    let n = tasks_remote::tasks_waiting(&self.session.mailbox, account);
                    if let Some((_, m)) = &mut self.tasks.ready {
                        m.insert(account, n);
                    }
                    n
                }
            };
            let since = match self.tasks.waiting.get(&account) {
                Some((seen, since)) if *seen == ready => *since,
                _ => {
                    self.tasks.waiting.insert(account, (ready, now));
                    now
                }
            };
            let pushes = ready > 0 && now - since >= QUIET_MS;
            let asked = self.tasks.checked.get(&account).copied();
            if !(pushes || asked.is_none_or(|at| now - at >= REFRESH_MS)) {
                continue;
            }
            let (failures, at) = self.tasks.failures.get(&account).copied().unwrap_or((0, 0.0));
            let patience = (FIRST_RETRY_MS * f64::from(1u32 << failures.saturating_sub(1).min(8))).min(LONGEST_RETRY_MS);
            if failures > 0 && now - at < patience {
                continue;
            }
            if self.start_tasks_job(account, now) {
                return true;
            }
        }
        false
    }

    pub(crate) fn tasks_tick(&mut self, ctx: &egui::Context) {
        self.tasks_tick_at(now_ms());
        if self.tasks.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.services.graph_tasks.is_some() && self.auto_sync && !tasks_remote::task_accounts(&self.session.mailbox).is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
    }

    fn start_tasks_job(&mut self, account: Id, now: f64) -> bool {
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        let Some(oauth) = owner.oauth.clone() else { return false };
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", &owner.email)).unwrap_or_default();
        if refresh.is_empty() {
            // (Nothing to renew with: Send/Receive signs in first.)
            return false;
        }
        let Ok(upload) = tasks_remote::task_upload(&self.session.mailbox, account) else { return false };
        let given = fingerprint(&refresh);
        let job = TasksJob { account: owner, oauth: self.effective_oauth(oauth), refresh, upload, cancel: Default::default() };
        let Some(graph) = &self.services.graph_tasks else { return false };
        self.tasks.rx = Some((account, now, given, graph(job)));
        self.tasks.checked.insert(account, now);
        true
    }

    /// The job answered: keep the refresh token, take in what the server made of the changes, and
    /// what it has. (Public for a host's tests.)
    pub fn poll_tasks(&mut self) {
        let Some((account, started, given, rx)) = &self.tasks.rx else { return };
        let (account, started, given) = (*account, *started, *given);
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("tasks, account {account}: the job ended without an answer (its thread ended early)");
                TasksResult {
                    account,
                    errors: vec![tl!("The to-dos couldn't be kept in step: the job stopped unexpectedly").to_string()],
                    ..Default::default()
                }
            }
        };
        self.tasks.rx = None;
        let mut errors = result.errors;
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
        let settling = self.tasks.settling.get(&account).copied().unwrap_or(0);
        if !result.consent_needed {
            self.tasks.settling.remove(&account);
        }
        if result.consent_needed && settling > 0 {
            log::info!("tasks, account {account}: refused just after the owner allowed them; trying again ({settling} more)");
            self.tasks.settling.insert(account, settling - 1);
            let (n, _) = self.tasks.failures.get(&account).copied().unwrap_or((0, 0.0));
            self.tasks.failures.insert(account, (n.saturating_add(1), started));
            self.tasks.checked.remove(&account);
        } else if result.consent_needed {
            log::info!("tasks, account {account}: the sign-in doesn't cover to-dos; the owner is to be asked");
            if !matches!(self.tasks.perm.get(&account), Some(Permission::Asking | Permission::Declined | Permission::Refused(_))) {
                self.tasks.perm.insert(account, Permission::Needed);
            }
        } else if result.denied {
            let said = tl!("This account may not use its to-dos through Microsoft Graph").to_string();
            self.tasks.perm.insert(account, Permission::Refused(said));
        } else if result.sign_in_refused && result.tokens.is_none() && settling == 0 {
            // The refresh itself was refused, for mail too: never "not asked yet" with no way back
            // (#520). (Not when the owner allowed the to-dos since the job began: that sign-in is
            // newer than this answer.)
            log::warn!("tasks: the sign-in was refused; waiting for a new one");
            if !matches!(self.tasks.perm.get(&account), Some(Permission::Asking | Permission::Declined | Permission::Refused(_))) {
                self.tasks.perm.insert(account, Permission::SignInOver(given));
            }
        } else if result.sign_in_refused {
            // Graph refused a token just renewed: the next look tries again.
            log::warn!("tasks: the server refused the sign-in; trying again later");
        } else if result.download.is_some() || result.uploaded.is_some() {
            self.tasks.perm.insert(account, Permission::Given);
        }
        let mut worked = false;
        if let Some(uploaded) = &result.uploaded {
            let uploaded = uploaded.clone().for_account_of(&self.session.mailbox, account);
            match tasks_remote::apply_task_changes(&mut self.session, &uploaded) {
                Ok(merged) => {
                    errors.extend(merged.messages);
                    worked = true;
                }
                Err(e) => errors.push(crate::i18n::fmt(tl!("The to-dos sent couldn't be noted: {error}"), &[("error", &e.to_string())])),
            }
        }
        if let Some(download) = &result.download {
            match tasks_remote::apply_tasks(&mut self.session, download) {
                Ok(_) => worked = true,
                Err(e) => errors.push(crate::i18n::fmt(tl!("The to-dos couldn't be taken in: {error}"), &[("error", &e.to_string())])),
            }
        }
        if worked && errors.is_empty() {
            self.tasks.failures.remove(&account);
        } else if !worked || !errors.is_empty() {
            let (n, _) = self.tasks.failures.get(&account).copied().unwrap_or((0, 0.0));
            if !result.consent_needed && !result.sign_in_refused {
                self.tasks.failures.insert(account, (n.saturating_add(1), started));
            }
        }
        self.note_sync_problems(account, &errors, false);
    }

    /// Ask the owner, once, when a job found the sign-in doesn't cover to-dos: a question first,
    /// then Microsoft's page. Not while another dialog is up or File is open.
    fn tasks_ask_tick(&mut self) {
        // The question about the local to-dos went away unanswered: that is a "Not now", noted,
        // so it is not asked on every look (File › Info offers it again).
        if self.tasks.offer_open && !matches!(&self.dialog, Some(Dialog::Confirm { command, .. }) if command == "tasks.sendLocal") {
            self.tasks.offer_open = false;
            if self.session.mailbox.local_tasks == LocalTasks::Unasked {
                let _ = self.run("tasks.sendLocal", json!({"account": null}));
            }
        }
        // A question that went away unanswered is not asked again by itself.
        let asking: Vec<Id> = self.tasks.perm.iter().filter(|(_, p)| **p == Permission::Asking).map(|(a, _)| *a).collect();
        let still = self.signing_in.is_some() || matches!(&self.dialog, Some(Dialog::Confirm { command, .. }) if command == "ui.allowTasks");
        if !still {
            for account in asking {
                self.tasks.perm.insert(account, Permission::Declined);
            }
        }
        if self.dialog.is_some() || self.ui.backstage || self.signing_in.is_some() {
            return;
        }
        let Some(account) = self.tasks.perm.iter().find(|(_, p)| **p == Permission::Needed).map(|(a, _)| *a) else {
            if self.session.mailbox.local_tasks == LocalTasks::Unasked {
                self.offer_local_tasks();
            }
            return;
        };
        self.ask_tasks(account);
    }

    /// For a stand-in host (the screenshot example): every account that keeps to-dos has its To Do
    /// working, as if a job had read it.
    #[doc(hidden)]
    pub fn stand_in_tasks_work(&mut self) {
        for account in tasks_remote::task_accounts(&self.session.mailbox) {
            self.tasks.perm.insert(account, Permission::Given);
        }
    }

    /// The accounts whose To Do is working (a job read it with the permission).
    fn tasks_working(&self) -> Vec<Id> {
        tasks_remote::task_accounts(&self.session.mailbox).into_iter().filter(|a| self.tasks.perm.get(a) == Some(&Permission::Given)).collect()
    }

    /// The first account whose To Do works: File › Info says what is local under it.
    pub(crate) fn tasks_working_first(&self) -> Option<Id> {
        self.tasks_working().first().copied()
    }

    /// How many to-dos exist only on this computer and could be offered to an account's To Do;
    /// 0 when no account's To Do works yet.
    pub(crate) fn local_tasks_to_offer(&self) -> usize {
        if self.tasks_working().is_empty() { 0 } else { tasks_remote::local_task_count(&self.session.mailbox) }
    }

    /// Ask the owner, once, whether the to-dos that exist only here are to go to an account's To
    /// Do. Nothing is sent before a yes. (File › Info asks again.) Returns whether it asked.
    pub(crate) fn offer_local_tasks(&mut self) -> bool {
        let count = self.local_tasks_to_offer();
        let accounts = self.tasks_working();
        if count == 0 || self.dialog.is_some() || self.signing_in.is_some() {
            return false;
        }
        let address = |id: Id| self.session.mailbox.account(id).map(|a| a.email.trim().to_string()).unwrap_or_default();
        let Some(first) = accounts.first().copied() else { return false };
        let how_many = tln!(count, "Send your {n} local task to {address}'s To Do?", "Send your {n} local tasks to {address}'s To Do?");
        let title = crate::i18n::fmt(&how_many, &[("n", &count.to_string()), ("address", &address(first))]);
        let more_than_one = accounts.len() > 1;
        let text = if more_than_one {
            tl!(
                "These tasks exist only on this computer. Choose the Microsoft account whose To Do gets them, and every task you make from now on. Until you choose, they stay here and nothing of them is sent."
            )
        } else {
            tl!(
                "These tasks exist only on this computer. If you send them, they and every task you make from now on are kept in step with Microsoft To Do. Until you do, they stay here and nothing of them is sent. File › Info offers this again."
            )
        };
        let label = |id: Id| crate::i18n::fmt(tl!("Send to {address}"), &[("address", &address(id))]);
        let mut ask = Question {
            title: if more_than_one { tl!("Send Your Local Tasks to To Do?").to_string() } else { title },
            text: text.to_string(),
            action: if more_than_one { label(first) } else { tl!("Send Tasks").to_string() },
            with: json!({"account": first}),
            more: Vec::new(),
            note: None,
            // Closing it is the same as Not now (`tasks_ask_tick`), so there is no other Cancel.
            cancel: Some(tl!("Not now").to_string()),
            tick: None,
        };
        for id in accounts.iter().skip(1) {
            ask.more.push((label(*id), json!({"account": id})));
        }
        self.tasks.offer_open = true;
        self.dialog = Some(Dialog::Confirm { command: "tasks.sendLocal".to_string(), params: json!({}), ask });
        true
    }

    /// What File › Info says about the to-dos that exist only here, and whether it can offer to
    /// send them: `None` when there is nothing to say.
    pub(crate) fn local_tasks_line(&self) -> Option<(String, bool)> {
        let count = self.local_tasks_to_offer();
        if count == 0 {
            return None;
        }
        let line = tln!(count, "{n} task exists only on this computer", "{n} tasks exist only on this computer");
        Some((line, true))
    }

    /// The question before Microsoft's page: what is asked of the owner and why.
    pub(crate) fn ask_tasks(&mut self, account: Id) -> bool {
        let Some(address) = self.session.mailbox.account(account).map(|a| a.email.trim().to_string()) else { return false };
        let text = crate::i18n::fmt(
            tl!(
                "To keep the tasks of {address} in step with Microsoft To Do, MailCraft needs your permission to read and change your To Do lists and tasks. Microsoft's page opens in your browser to ask you, once.\n\nUntil you allow it, tasks stay on this computer and nothing of them is sent to Microsoft."
            ),
            &[("address", &address)],
        );
        let ask = Question {
            title: tl!("Keep Tasks in Step with Microsoft To Do").to_string(),
            text,
            action: tl!("Continue").to_string(),
            with: Value::Null,
            more: Vec::new(),
            note: None,
            cancel: None,
            tick: None,
        };
        self.tasks.perm.insert(account, Permission::Asking);
        self.dialog = Some(Dialog::Confirm { command: "ui.allowTasks".to_string(), params: json!({"account": account}), ask });
        true
    }

    /// File › Info's "Ask again".
    pub(crate) fn ask_tasks_again(&mut self, account: Id) {
        if self.dialog.is_none() && self.signing_in.is_none() {
            self.ask_tasks(account);
        }
    }

    /// `ui.allowTasks`: Microsoft's page, for the to-dos too, because the owner said go ahead.
    pub(crate) fn begin_tasks_sign_in(&mut self, account: Id) -> Result<Value, String> {
        let owner = self.session.mailbox.account(account).filter(|a| a.oauth.is_some() && !a.is_shared()).cloned();
        let owner = owner.ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        let oauth = owner.oauth.clone().ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        self.tasks_next = true;
        let begun = self.begin_sign_in(owner.id, oauth, &owner.email, String::new(), false, true);
        self.tasks_next = false;
        if let Err(e) = begun {
            self.tasks.perm.insert(account, Permission::Needed);
            return Err(e);
        }
        self.tasks.perm.insert(account, Permission::Asking);
        self.session.sync_status =
            tl!("Allow MailCraft to keep your tasks in step with Microsoft To Do on the page that opened in your browser…").to_string();
        Ok(json!({"pending": "signIn", "account": owner.id}))
    }

    /// The owner answered on Microsoft's page about to-dos: with the tokens, the to-dos are kept
    /// in step at once; without, why not is said.
    pub(crate) fn tasks_signed_in(&mut self, account: Id, answer: Result<Tokens, String>) {
        use crate::approval::NO_BROWSER;
        use crate::favorites_sync::{NEEDS_APPROVAL, REFUSED_BY_ORGANISATION, REFUSED_FOR_ACCOUNT};
        self.session.sync_status.clear();
        let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
        match answer {
            // (Somebody else's tokens are neither used nor remembered, #103.)
            Ok(tokens) if self.signed_in_as_other(account, &tokens) => {
                self.tasks.perm.insert(account, Permission::Needed);
            }
            Ok(tokens) => {
                if !tokens.refresh.is_empty() {
                    self.remember("refresh", &address, &tokens.refresh);
                    if let Some(kept) = self.tokens.get_mut(&account) {
                        kept.refresh = tokens.refresh.clone();
                    }
                }
                // Allowed, as far as is known: the next job finds out.
                log::info!("tasks, account {account}: the owner allowed to-dos; the next job finds out");
                self.tasks.perm.remove(&account);
                self.tasks.settling.insert(account, SETTLING_TRIES);
                self.tasks.failures.remove(&account);
                self.tasks.checked.remove(&account);
            }
            Err(e) if e == NEEDS_APPROVAL => {
                let said = tl!("Your organisation's administrator has to approve keeping tasks in step with Microsoft To Do once: File › Info");
                self.tasks.perm.insert(account, Permission::Refused(said.to_string()));
                self.status(said);
                let _ = self.run("ui.approvalHelp", json!({"account": account}));
            }
            Err(e) if e == REFUSED_BY_ORGANISATION || e == REFUSED_FOR_ACCOUNT => {
                let said = tl!("Your organisation doesn't allow MailCraft to keep tasks in step with Microsoft To Do");
                self.tasks.perm.insert(account, Permission::Refused(said.to_string()));
                self.status(said);
            }
            Err(e) if e == NO_BROWSER => {
                self.tasks.perm.insert(account, Permission::Declined);
                self.status(crate::i18n::fmt(tl!("Tasks stay on this computer for now: {error}"), &[("error", &e)]));
            }
            Err(e) => {
                let said = crate::i18n::fmt(tl!("Tasks weren't allowed to be kept in step: {error}"), &[("error", &e)]);
                self.tasks.perm.insert(account, Permission::Refused(said.clone()));
                self.status(said);
            }
        }
    }

    /// What File › Info says under an account's card about its tasks, and whether there is a way
    /// to ask again. `None` for an account whose tasks Microsoft doesn't keep.
    pub(crate) fn tasks_line(&self, account: Id) -> Option<(String, bool)> {
        self.services.graph_tasks.as_ref()?;
        if !tasks_remote::task_accounts(&self.session.mailbox).contains(&account) {
            return None;
        }
        Some(match self.tasks.perm.get(&account) {
            Some(Permission::Given) => (tl!("Tasks: kept in step with Microsoft To Do").to_string(), false),
            Some(Permission::Needed) => (tl!("Tasks: waiting for your permission").to_string(), true),
            Some(Permission::Asking) => (tl!("Tasks: asking for your permission…").to_string(), false),
            Some(Permission::Declined) => (tl!("Tasks: kept on this computer only (not allowed yet)").to_string(), true),
            Some(Permission::Refused(why)) => {
                (crate::i18n::fmt(tl!("Tasks: not allowed ({why})"), &[("why", crate::forms::kit::shown(why, 90).as_str())]), true)
            }
            Some(Permission::SignInOver(_)) => (tl!("Tasks: Microsoft no longer takes the sign-in").to_string(), true),
            None => (tl!("Tasks: not asked yet").to_string(), false),
        })
    }
}

trait ForAccount {
    fn for_account_of(self, mailbox: &mailcraft_model::Mailbox, account: Id) -> TasksUploaded;
}

impl ForAccount for TasksUploaded {
    /// The answer as one for `account` (the host only knows who signed in).
    fn for_account_of(self, mailbox: &mailcraft_model::Mailbox, account: Id) -> TasksUploaded {
        match mailbox.account(account) {
            Some(a) => self.for_account(a),
            None => self,
        }
    }
}

/// A refresh token, told apart from another without being kept: whether the account has signed
/// in again since a job's was refused.
fn fingerprint(refresh: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    refresh.hash(&mut hasher);
    hasher.finish()
}
