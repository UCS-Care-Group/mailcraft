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
//! **The permission.** Reading and changing the lists takes `Tasks.ReadWrite`. A new sign-in's
//! page asks for it beside the rest, as an extra a page that won't have it is asked again without
//! (#661). For an account that signed in before, the first job finds its sign-in doesn't cover it
//! ([`TasksResult::consent_needed`]), nothing of the to-dos is read or sent, mail and calendar go
//! on as ever, and the window asks the owner the one question of [`crate::server_sync`], then
//! Microsoft's page, for the to-dos and the mailbox's settings together. File › Info says where it
//! stands, with Ask again. A no that is not the owner's to change is said as for the Favorites
//! ([`crate::favorites_sync::NEEDS_APPROVAL`]…).

use mailcraft_engine::cmd::tasks_remote;
use mailcraft_engine::sync_driver::{TasksFacts, TasksQuestion};
use mailcraft_host::tasks_job::TasksJobs;
use mailcraft_model::{Id, LocalTasks};
use serde_json::json;

use crate::dialogs::{Dialog, Question};
use crate::{MailApp, now_ms};

pub(crate) use mailcraft_engine::sync_driver::{Permission, fingerprint};
pub use mailcraft_host::tasks::{TasksJob, TasksResult};

/// The state of the to-do jobs, in the window.
#[derive(Default)]
pub struct TaskSync {
    /// The job under way, kept by the host's driver of it ([`TasksJobs`]).
    jobs: TasksJobs,
    /// The question about the to-dos that exist only here is on screen.
    pub(crate) offer_open: bool,
}

impl TaskSync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.jobs.busy()
    }
}

impl MailApp {
    /// Look whether any account's to-dos are due, and start a job when they are. Called every
    /// frame. (Public for a host's tests, which have no window.) What is due is the engine's to
    /// say ([`mailcraft_engine::Session::tasks_due`]); this starts the job on the host.
    pub fn tasks_tick_at(&mut self, now: f64) -> bool {
        self.session.driver.tasks.clock = now;
        self.tasks_ask_tick();
        let facts = TasksFacts { reachable: self.services.graph_tasks.is_some(), running: self.tasks.busy(), busy: self.job_busy() };
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
        if self.tasks.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.services.graph_tasks.is_some()
            && self.session.driver.auto_sync()
            && !tasks_remote::task_accounts(&self.session.mailbox).is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
    }

    fn start_tasks_job(&mut self, account: Id, now: f64) -> bool {
        let Some(owner) = self.session.mailbox.account(account).filter(|a| a.oauth.is_some()).map(|a| a.id) else { return false };
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", owner));
        let offline = self.offline();
        let ids = mailcraft_engine::sync_driver::ClientIds {
            microsoft_chosen: &self.ui.microsoft_client_id,
            microsoft_built_in: &self.services.microsoft_client_id,
            google_chosen: &self.ui.google_client_id,
            google_built_in: &self.services.google_client_id,
        };
        let graph = self.services.graph_tasks.as_ref().filter(|_| !offline);
        // What the job is handed, the delete marked as gone and the engine told are the host's.
        let oauth = |oauth| mailcraft_engine::sync_driver::effective_oauth(oauth, &ids);
        self.tasks.jobs.start(&mut self.session, account, |_| refresh, oauth, now, |job| graph.map(|graph| graph(job)))
    }

    /// The job answered: keep the refresh token, take in what the server made of the changes, and
    /// what it has. (Public for a host's tests.)
    pub fn poll_tasks(&mut self) {
        // For a mailbox that has been closed since: its ids may name other accounts and tasks in
        // this one, and the answer is nobody's (AU3-08).
        if self.tasks.jobs.stale(&self.session) {
            log::info!("tasks: the answer is for a mailbox that is no longer open; let go of");
            self.tasks = TaskSync::default();
            return;
        }
        let lang = crate::i18n::current();
        let Some(answer) = self.tasks.jobs.poll(&mut self.session, lang) else { return };
        let account = answer.account;
        let mut errors = answer.errors;
        if answer.stopped_early {
            errors.insert(0, tl!("The to-dos couldn't be kept in step: the job stopped unexpectedly").to_string());
        }
        // Somebody else's sign-in: the host refused the answer whole (nothing of it is taken in and
        // its token is not kept); the owner is told who to sign in as, as for any sign-in (#103).
        if let Some(foreign) = &answer.foreign {
            errors.push(self.say_signed_in_as_other(account, &foreign.signed_in_as));
        }
        if let Some(tokens) = &answer.tokens
            && !tokens.refresh.is_empty()
            && !self.signed_in_as_other(account, tokens)
        {
            self.remember("refresh", account, &tokens.refresh);
            if let Some(mail) = self.tokens.get_mut(&account) {
                mail.refresh = tokens.refresh.clone();
            }
        }
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
        // (A question that went away unanswered is a no: `sync_ask_tick`.)
        self.sync_ask_tick();
        // Who is asked next is the engine's to say.
        match self.session.tasks_question(&self.ask_window()) {
            Some(TasksQuestion::Permission(account)) => {
                self.ask_sync(account);
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

    /// The one question is put ([`crate::server_sync`]): the to-dos that wait for it are being
    /// asked about.
    pub(crate) fn tasks_sync_asking(&mut self, account: Id) {
        if matches!(self.session.driver.tasks.perm.get(&account), Some(Permission::Needed | Permission::Declined | Permission::Refused(_))) {
            self.session.driver.tasks.perm.insert(account, Permission::Asking);
        }
    }

    /// The one question went away unanswered: the to-dos stay here, and nobody is asked again by
    /// itself (File › Info has Ask again).
    pub(crate) fn tasks_sync_not_now(&mut self, account: Id) {
        if matches!(self.session.driver.tasks.perm.get(&account), Some(Permission::Needed | Permission::Asking)) {
            self.session.driver.tasks.perm.insert(account, Permission::Declined);
        }
    }

    /// The page signed in somebody else: the to-dos wait as they did.
    pub(crate) fn tasks_sync_waits(&mut self, account: Id) {
        if self.session.driver.tasks.perm.get(&account) == Some(&Permission::Asking) {
            self.session.driver.tasks.perm.insert(account, Permission::Needed);
        }
    }

    /// Asked, and no: why, in a few words.
    pub(crate) fn tasks_sync_refused(&mut self, account: Id, why: &str) {
        if matches!(self.session.driver.tasks.perm.get(&account), Some(Permission::Needed | Permission::Asking | Permission::Declined)) {
            self.session.driver.tasks.perm.insert(account, Permission::Refused(why.to_string()));
        }
    }

    /// The owner allowed it on Microsoft's page: the next job finds out, and a refusal right after
    /// is taken for Microsoft not having taken it in yet (#491).
    pub(crate) fn tasks_allowed(&mut self, account: Id) {
        self.session.driver.tasks.allowed(account);
    }

    /// The to-dos' part of File › Info's line about keeping in step with Outlook.
    pub(crate) fn tasks_sync_part(&self, account: Id) -> Option<(String, bool, crate::server_sync::Perm)> {
        use crate::server_sync::Perm;
        let (line, again) = self.tasks_line(account)?;
        let perm = match self.session.driver.tasks.perm.get(&account) {
            Some(Permission::Asking) => Perm::Asking,
            Some(Permission::Given) | None => Perm::Fine,
            Some(_) => Perm::Waits,
        };
        Some((line, again, perm))
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
                let why = crate::forms::kit::shown(why, crate::server_sync::REASON_SHOWN);
                (crate::i18n::fmt(tl!("Tasks: not allowed ({why})"), &[("why", why.as_str())]), true)
            }
            Some(Permission::SignInOver(_)) => (tl!("Tasks: Microsoft no longer takes the sign-in").to_string(), true),
            None => (tl!("Tasks: not asked yet").to_string(), false),
        })
    }
}
