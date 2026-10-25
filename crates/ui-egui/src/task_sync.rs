// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks kept in step with Microsoft To Do in the window (#248), for the accounts that sign in
//! with Microsoft.
//!
//! A job of its own, like the calendar's: when the to-dos are due (every few minutes, a moment
//! after a change here) the window hands the host a [`TasksJob`] with what is to be told to the
//! server (`sync.taskChanges`); the host tells it, then reads the lists and tasks whole, and the
//! window takes in what the server made of the changes (`sync.applyTaskChanges`) and then what it
//! has (`sync.applyTasks`). The merge is the engine's ([`mailcraft_engine::cmd::tasks_remote`]).
//!
//! **The permission.** Reading and changing the lists takes `Tasks.ReadWrite`. A new sign-in's
//! page asks for it beside the rest. An account that signed in before has not been asked: the
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
use mailcraft_model::{Account, Id};
use serde_json::{Value, json};

use crate::dialogs::{Dialog, Question};
use crate::{MailApp, Tokens, now_ms};

/// How often an account's to-dos are read when nothing is waiting to go, and how long a change
/// is left to settle before it goes (milliseconds), and the wait after a job that failed.
const REFRESH_MS: f64 = 5.0 * 60_000.0;
const QUIET_MS: f64 = 3_000.0;
const FIRST_RETRY_MS: f64 = 30_000.0;
const LONGEST_RETRY_MS: f64 = 15.0 * 60_000.0;

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
}

/// The state of the to-do jobs, in the window.
#[derive(Default)]
pub struct TaskSync {
    /// The job under way: for which account, since when, and the answer to come.
    rx: Option<(Id, f64, Receiver<TasksResult>)>,
    /// When each account's to-dos were last asked for, and the failures since the last answer.
    checked: HashMap<Id, f64>,
    failures: HashMap<Id, (u32, f64)>,
    pub(crate) perm: HashMap<Id, Permission>,
    /// How many changes were waiting the last time, and since when.
    waiting: HashMap<Id, (usize, f64)>,
}

impl TaskSync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
}

impl MailApp {
    /// Whether the to-dos of `account` wait for the owner's permission.
    fn tasks_blocked(&self, account: Id) -> bool {
        matches!(self.tasks.perm.get(&account), Some(Permission::Needed | Permission::Asking | Permission::Declined | Permission::Refused(_)))
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
            let ready = tasks_remote::tasks_waiting(&self.session.mailbox, account);
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
        let job = TasksJob { account: owner, oauth: self.effective_oauth(oauth), refresh, upload, cancel: Default::default() };
        let Some(graph) = &self.services.graph_tasks else { return false };
        self.tasks.rx = Some((account, now, graph(job)));
        self.tasks.checked.insert(account, now);
        true
    }

    /// The job answered: keep the refresh token, take in what the server made of the changes, and
    /// what it has. (Public for a host's tests.)
    pub fn poll_tasks(&mut self) {
        let Some((account, started, rx)) = &self.tasks.rx else { return };
        let (account, started) = (*account, *started);
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
        if result.consent_needed {
            log::info!("tasks, account {account}: the sign-in doesn't cover to-dos; the owner is to be asked");
            if !matches!(self.tasks.perm.get(&account), Some(Permission::Asking | Permission::Declined | Permission::Refused(_))) {
                self.tasks.perm.insert(account, Permission::Needed);
            }
        } else if result.denied {
            let said = tl!("This account may not use its to-dos through Microsoft Graph").to_string();
            self.tasks.perm.insert(account, Permission::Refused(said));
        } else if result.sign_in_refused {
            log::warn!("tasks, account {account}: the sign-in was refused; the next Send/Receive asks the owner");
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
        let Some(account) = self.tasks.perm.iter().find(|(_, p)| **p == Permission::Needed).map(|(a, _)| *a) else { return };
        self.ask_tasks(account);
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
                self.tasks.perm.remove(&account);
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
