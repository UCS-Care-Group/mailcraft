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

use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::tasks_remote;
use mailcraft_engine::sync_driver::{TasksCame, TasksFacts, TasksQuestion};
use mailcraft_model::{Id, LocalTasks};
use serde_json::{Value, json};

use crate::dialogs::{Dialog, Question};
use crate::{MailApp, Tokens, now_ms};

pub(crate) use mailcraft_engine::sync_driver::{Permission, fingerprint};
pub use mailcraft_host::tasks::{TasksJob, TasksResult};

/// What a to-do job was started for: the mailbox opening ([`mailcraft_engine::Session::openings`])
/// and the account as it was, whose address the answer is taken in for (AU3-08).
struct Begun {
    opening: u64,
    account: mailcraft_model::Account,
}

/// The state of the to-do jobs, in the window.
#[derive(Default)]
pub struct TaskSync {
    /// The job under way: for which account, since when, the [`fingerprint`] of the refresh
    /// token it was given, and the answer to come.
    rx: Option<(Id, f64, u64, Begun, Receiver<TasksResult>)>,
    /// The question about the to-dos that exist only here is on screen.
    pub(crate) offer_open: bool,
}

impl TaskSync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
}

impl MailApp {
    /// Look whether any account's to-dos are due, and start a job when they are. Called every
    /// frame. (Public for a host's tests, which have no window.) What is due is the engine's to
    /// say ([`mailcraft_engine::Session::tasks_due`]); this starts the job on the host.
    pub fn tasks_tick_at(&mut self, now: f64) -> bool {
        self.tasks_ask_tick();
        let facts = TasksFacts { reachable: self.services.graph_tasks.is_some(), running: self.tasks.rx.is_some(), busy: self.job_busy() };
        let tokens = &self.tokens;
        let due = self.session.tasks_due(&facts, |account| tokens.get(&account).map(|t| t.refresh.clone()), now);
        for account in due {
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
        } else if self.services.graph_tasks.is_some()
            && self.session.driver.auto_sync()
            && !tasks_remote::task_accounts(&self.session.mailbox).is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
    }

    fn start_tasks_job(&mut self, account: Id, now: f64) -> bool {
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        let Some(oauth) = owner.oauth.clone() else { return false };
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", owner.id)).unwrap_or_default();
        if refresh.is_empty() {
            // (Nothing to renew with: Send/Receive signs in first.)
            return false;
        }
        let Ok(upload) = tasks_remote::task_upload(&self.session.mailbox, account) else { return false };
        let given = fingerprint(&refresh);
        let deletes = tasks_remote::carries_delete(&upload);
        let zone = self.session.zone();
        let begun = Begun { opening: self.session.openings(), account: owner.clone() };
        let job = TasksJob { account: owner, oauth: self.effective_oauth(oauth), refresh, upload, zone, cancel: Default::default() };
        let Some(graph) = self.services.graph_tasks.as_ref().filter(|_| !self.offline()) else { return false };
        let rx = graph(job);
        // A delete that goes out can't be undone: the task would come back here, gone there (#706).
        tasks_remote::mark_sent(&mut self.session, deletes);
        self.tasks.rx = Some((account, now, given, begun, rx));
        self.session.tasks_started(account, now);
        true
    }

    /// The job answered: keep the refresh token, take in what the server made of the changes, and
    /// what it has. (Public for a host's tests.)
    pub fn poll_tasks(&mut self) {
        let Some((account, started, given, begun, rx)) = &self.tasks.rx else { return };
        let (account, started, given) = (*account, *started, *given);
        // For a mailbox that has been closed since: its ids may name other accounts and tasks in
        // this one, and the answer is nobody's (AU3-08).
        if begun.opening != self.session.openings() {
            log::info!("tasks: the answer is for a mailbox that is no longer open; let go of");
            self.tasks = TaskSync::default();
            return;
        }
        let ran_for = begun.account.clone();
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
            self.remember("refresh", account, &tokens.refresh);
            if let Some(mail) = self.tokens.get_mut(&account) {
                mail.refresh = tokens.refresh.clone();
            }
        }
        let came = TasksCame {
            job_errors: !errors.is_empty(),
            consent_needed: result.consent_needed,
            sign_in_refused: result.sign_in_refused,
            new_tokens: result.tokens.is_some(),
            denied: result.denied,
            uploaded: result.uploaded.as_ref(),
            download: result.download.as_ref(),
        };
        let lang = crate::i18n::current();
        let said = self.session.tasks_answered(account, started, given, &ran_for, lang, &came);
        errors.extend(said.iter().map(|e| e.in_lang(lang)));
        self.note_problems_from("tasks", account, &errors, None, false);
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
        let still = self.signing_in.is_some() || matches!(&self.dialog, Some(Dialog::Confirm { command, .. }) if command == "ui.allowTasks");
        self.session.driver.tasks.unanswered(still);
        // Who is asked next is the engine's to say.
        match self.session.tasks_question(&self.ask_window()) {
            Some(TasksQuestion::Permission(account)) => {
                self.ask_tasks(account);
            }
            Some(TasksQuestion::LocalTasks) => {
                self.offer_local_tasks();
            }
            None => {}
        }
    }

    /// For a stand-in host (the screenshot example): every account that keeps to-dos has its To Do
    /// working, as if a job had read it.
    #[doc(hidden)]
    pub fn stand_in_tasks_work(&mut self) {
        let accounts = tasks_remote::task_accounts(&self.session.mailbox);
        self.session.driver.tasks.working_all(&accounts);
    }

    /// The accounts whose To Do is working (a job read it with the permission).
    fn tasks_working(&self) -> Vec<Id> {
        self.session.tasks_working()
    }

    /// Whether a to-do put in `account` waits on this computer because a job found that To Do
    /// isn't allowed there (nothing is known of an account no job has looked at yet).
    pub(crate) fn tasks_held(&self, account: Id) -> bool {
        self.session.tasks_held(account)
    }

    /// The first account added whose To Do works, wherever the owner moved it (#702): File › Info
    /// says what is local under it, and the question offers it first.
    pub(crate) fn tasks_working_first(&self) -> Option<Id> {
        let working = self.tasks_working();
        self.session.mailbox.accounts.iter().map(|a| a.id).find(|id| working.contains(id))
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
        let Some(first) = self.tasks_working_first() else { return false };
        let how_many = tln!(count, "Send your {n} local task to {address}'s To Do?", "Send your {n} local tasks to {address}'s To Do?");
        let title = crate::i18n::fmt(&how_many, &[("n", &count.to_string()), ("address", &address(first))]);
        let more_than_one = accounts.len() > 1;
        let text = if more_than_one {
            tl!(
                "These tasks exist only on this computer. Choose the Microsoft account whose To Do gets them. Until you choose, they stay here and nothing of them is sent."
            )
        } else {
            tl!(
                "These tasks exist only on this computer. If you send them, they are kept in step with Microsoft To Do. Until you do, they stay here and nothing of them is sent. File › Info offers this again."
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
        for id in accounts.iter().filter(|id| **id != first) {
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
        self.session.driver.tasks.asking(account);
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
            self.session.driver.tasks.needed(account);
            return Err(e);
        }
        self.session.driver.tasks.asking(account);
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
        match answer {
            // (Somebody else's tokens are neither used nor remembered, #103.)
            Ok(tokens) if self.signed_in_as_other(account, &tokens) => {
                self.session.driver.tasks.needed(account);
            }
            Ok(tokens) => {
                if !tokens.refresh.is_empty() {
                    self.remember("refresh", account, &tokens.refresh);
                    if let Some(kept) = self.tokens.get_mut(&account) {
                        kept.refresh = tokens.refresh.clone();
                    }
                }
                // Allowed, as far as is known: the next job finds out.
                log::info!("tasks, account {account}: the owner allowed to-dos; the next job finds out");
                self.session.driver.tasks.allowed(account);
            }
            Err(e) if e == NEEDS_APPROVAL => {
                let said = tl!("Your organisation's administrator has to approve keeping tasks in step with Microsoft To Do once: File › Info");
                self.session.driver.tasks.refused(account, said.to_string());
                self.status(said);
                let _ = self.run("ui.approvalHelp", json!({"account": account}));
            }
            Err(e) if e == REFUSED_BY_ORGANISATION || e == REFUSED_FOR_ACCOUNT => {
                let said = tl!("Your organisation doesn't allow MailCraft to keep tasks in step with Microsoft To Do");
                self.session.driver.tasks.refused(account, said.to_string());
                self.status(said);
            }
            Err(e) if e == NO_BROWSER => {
                self.session.driver.tasks.declined(account);
                self.status(crate::i18n::fmt(tl!("Tasks stay on this computer for now: {error}"), &[("error", &e)]));
            }
            Err(e) => {
                let said = crate::i18n::fmt(tl!("Tasks weren't allowed to be kept in step: {error}"), &[("error", &e)]);
                self.session.driver.tasks.refused(account, said.clone());
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
        Some(match self.session.driver.tasks.perm.get(&account) {
            Some(Permission::Given) => (tl!("Tasks: kept in step with Microsoft To Do").to_string(), false),
            Some(Permission::Needed) => (tl!("Tasks: waiting for your permission").to_string(), true),
            Some(Permission::Asking) => (tl!("Tasks: asking for your permission…").to_string(), false),
            Some(Permission::Declined | Permission::Denied) => (tl!("Tasks: kept on this computer only (not allowed yet)").to_string(), true),
            Some(Permission::Refused(why)) => {
                (crate::i18n::fmt(tl!("Tasks: not allowed ({why})"), &[("why", crate::forms::kit::shown(why, 90).as_str())]), true)
            }
            Some(Permission::SignInOver(_)) => (tl!("Tasks: Microsoft no longer takes the sign-in").to_string(), true),
            None => (tl!("Tasks: not asked yet").to_string(), false),
        })
    }
}
