// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar and Microsoft To Do kept in step on the phone (MB3.18), between Send/Receive runs
//! and at the end of a round, the background's included: the same jobs as the desktop's, through
//! the same code. What is due and what each job asks for is the engine's
//! ([`Session::calendar_due`], [`Session::tasks_due`]); the job in flight and its answer taken in
//! are the host's ([`CalendarJobs`], [`TasksJobs`]); this only says which sign-in a job goes with,
//! starts it on a thread of the host's and turns its answer into what the app is told.
//!
//! The rules the desktop keeps, kept here:
//! - Only a Microsoft account of the owner's own (never a shared mailbox: its calendars take a
//!   permission of their own, which this version doesn't ask for on the phone).
//! - The to-dos' job finds out from its own token whether the owner allowed `Tasks.ReadWrite`, and
//!   when not, nothing of the to-dos is read or sent and the app is asked to put the question
//!   (`PermissionQuestion`); nobody is asked silently, and no page opens by itself.
//! - A to-do that exists only on this phone is never sent before the owner said where it goes
//!   (`tasks.sendLocal`): the engine leaves it out of what goes up.
//! - Nothing starts with no network or while Work Offline is on.
//!
//! The reminders' schedule moves with what a job takes in: the session's thread raises
//! `ReminderScheduleChanged` when it does. Nothing of an event or a task is logged, and no token
//! leaves this module but to Send/Receive, which keeps the newest refresh token.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::tasks_remote::task_accounts;
use mailcraft_engine::sync_driver::{AskWindow, Busy, CalendarFacts, TasksFacts, TasksQuestion, effective_oauth};
use mailcraft_host::NetworkState;
use mailcraft_host::calendar_job::{CalendarAuth, CalendarJobs, CalendarPolled, take_answer};
use mailcraft_host::graph::{GraphRequest, GraphResult};
use mailcraft_host::sync::Adopted;
use mailcraft_host::tasks::{TasksJob, TasksResult};
use mailcraft_host::tasks_job::TasksJobs;
use mailcraft_model::Id;
use mailcraft_model::sync::Tokens;

use crate::Permission;
use crate::events::Event;
use crate::sync::{Inner, SyncHost};

/// How a calendar job is run: over the network, or as a test says.
pub(crate) type CalendarRunner = Arc<dyn Fn(GraphRequest) -> GraphResult + Send + Sync>;
/// How a to-do job is run.
pub(crate) type TasksRunner = Arc<dyn Fn(TasksJob) -> TasksResult + Send + Sync>;

/// The two runners a core has.
#[derive(Clone)]
pub(crate) struct Runners {
    pub(crate) calendar: CalendarRunner,
    pub(crate) tasks: TasksRunner,
}

/// The runners a core starts with: over the network, and never in a test's build, where a test that
/// doesn't swap its own in reaches nobody (an answer that brings nothing).
pub(crate) fn default_runners() -> Runners {
    #[cfg(test)]
    return Runners {
        calendar: Arc::new(|req: GraphRequest| GraphResult { account: req.account.id, ..Default::default() }),
        tasks: Arc::new(|job: TasksJob| TasksResult { account: job.account.id, ..Default::default() }),
    };
    #[cfg(not(test))]
    real_runners()
}

#[cfg_attr(test, allow(dead_code))]
fn real_runners() -> Runners {
    #[cfg(not(target_arch = "wasm32"))]
    return Runners { calendar: Arc::new(mailcraft_host::graph::graph), tasks: Arc::new(mailcraft_host::tasks::run) };
    #[cfg(target_arch = "wasm32")]
    Runners {
        calendar: Arc::new(|req: GraphRequest| GraphResult { account: req.account.id, ..Default::default() }),
        tasks: Arc::new(|job: TasksJob| TasksResult { account: job.account.id, ..Default::default() }),
    }
}

/// The calendar's and the to-dos' jobs in flight.
#[derive(Default)]
pub(crate) struct Jobs {
    pub(crate) calendar: CalendarJobs,
    pub(crate) tasks: TasksJobs,
}

impl Jobs {
    pub(crate) fn busy(&self) -> bool {
        self.calendar.busy() || self.tasks.busy()
    }
}

/// Marks a sentence for the catalogue (`cargo xtask i18n` gathers what is passed to it); it is put
/// through the interface language where it is said.
fn words(text: &'static str) -> &'static str {
    text
}

impl SyncHost {
    /// Run calendar and to-do jobs as `runners` say (the tests': nothing leaves the machine).
    #[cfg(test)]
    pub(crate) fn set_job_runners(&self, runners: Runners) {
        *self.job_runners.lock().unwrap_or_else(PoisonError::into_inner) = runners;
    }

    /// One wake of the calendar's and the to-dos' jobs, on the session's thread: the answers taken
    /// in, the owner asked about the to-dos when a job found that he has to be, and (`may_start`)
    /// the next job that is due started. How many were started (0 or 1: one job at a time).
    pub(crate) fn jobs_step(&self, inner: &mut Inner, session: &mut Session, may_start: bool) -> u32 {
        self.take_calendar(inner, session);
        self.take_tasks(inner, session);
        self.ask_tasks(session);
        let started = if may_start && !inner.jobs.busy() { self.start_job(inner, session) } else { 0 };
        self.note_keys(inner);
        started
    }

    /// The calendar job's answer, when it has come.
    fn take_calendar(&self, inner: &mut Inner, session: &mut Session) {
        // (A job for a mailbox that is no longer open is let go of; the engine forgot it with the mailbox.)
        let CalendarPolled::Answer(answer) = inner.jobs.calendar.poll(session) else { return };
        let mut answer = *answer;
        let account = answer.account;
        let mut errors = std::mem::take(&mut answer.result.errors);
        if answer.stopped_early {
            errors.insert(0, words("The calendar couldn't be brought up to date: the job stopped unexpectedly").to_string());
        }
        // Somebody else's sign-in: the host refused the answer whole (nothing of what it brought is
        // this account's calendar, and its token is not kept); the engine takes it as a job that
        // failed, and waits before the next.
        if answer.foreign.is_some() {
            errors.push(words("That sign-in is for somebody else's mailbox, not this account's. Nothing was kept.").to_string());
        }
        if let Some(tokens) = answer.result.tokens.take()
            && !self.keep_refresh(inner, session, account, &tokens, &mut errors)
        {
            // (Judged again as Send/Receive keeps it, and said there: nothing of it is taken in.)
            answer.result = GraphResult { account, ..Default::default() };
        }
        if answer.result.sign_in_refused {
            log::info!("calendar: the sign-in was refused; the next Send/Receive finds out");
        }
        let taken = take_answer(session, &answer, !errors.is_empty(), self.now_ms());
        let lang = session.interface_language;
        errors.extend(taken.problems.iter().chain(&taken.errors).map(|t| t.in_lang(lang)));
        for line in errors {
            self.job_error(inner, session, line);
        }
    }

    /// The to-dos' job's answer, when it has come.
    fn take_tasks(&self, inner: &mut Inner, session: &mut Session) {
        let lang = session.interface_language;
        let Some(answer) = inner.jobs.tasks.poll(session, lang) else { return };
        let mut errors = answer.errors;
        if answer.stopped_early {
            errors.insert(0, words("The to-dos couldn't be kept in step: the job stopped unexpectedly").to_string());
        }
        // Somebody else's sign-in: the host refused the answer whole before the engine saw it
        // (nothing of it was taken in, and there is no token to keep).
        if answer.foreign.is_some() {
            errors.push(words("That sign-in is for somebody else's mailbox, not this account's. Nothing was kept.").to_string());
        }
        if let Some(tokens) = &answer.tokens {
            let _ = self.keep_refresh(inner, session, answer.account, tokens, &mut errors);
        }
        for line in errors {
            self.job_error(inner, session, line);
        }
    }

    /// The refresh token a job got is the account's newest: Send/Receive keeps it (somebody else's
    /// is not kept, and said). Whether the sign-in was the account's own.
    fn keep_refresh(&self, inner: &mut Inner, session: &Session, account: Id, tokens: &Tokens, errors: &mut Vec<String>) -> bool {
        let secrets = self.host.platform().secrets();
        let ctx = self.context(session, secrets.as_deref());
        if inner.sr.adopt_refresh(session, &ctx, account, tokens) == Adopted::WrongPerson {
            errors.push(words("That sign-in is for somebody else's mailbox, not this account's. Nothing was kept.").to_string());
            return false;
        }
        true
    }

    /// A line for the owner: in the round's summary while a round is under way, else in what the
    /// last one said ([`crate::SyncStatus::messages`]).
    fn job_error(&self, inner: &mut Inner, session: &Session, line: String) {
        if inner.active {
            crate::sync::add_error(&mut inner.errors, line);
        } else {
            crate::sync::add_error(&mut inner.last_errors, session.interface_language.t(&line).to_string());
        }
    }

    /// A job found that the owner hasn't allowed the to-dos: the app is asked, once, to put the
    /// question in its own words (it calls `grant_permission` on a yes; the core never answers it).
    fn ask_tasks(&self, session: &mut Session) {
        let Some(TasksQuestion::Permission(account)) = session.tasks_question(&AskWindow::default()) else { return };
        // Being asked about: not asked again in this run unless a job finds it needed again.
        session.driver.tasks.asking(account);
        // (Only a to-do job finds the question needed, and it runs for an account of the phone's
        // own; one held back for want of room is still asked: a question fetches nothing.)
        if session.mailbox.account(account).is_some() {
            self.sink.push(Event::PermissionQuestion { account, permission: Permission::Tasks });
        }
    }

    /// Start the first job that is due: the calendar's, else the to-dos'.
    fn start_job(&self, inner: &mut Inner, session: &mut Session) -> u32 {
        let reachable = !session.prefs.offline && !matches!(self.host.platform().network(), NetworkState::Offline);
        let busy = Busy { graph: inner.sr.busy(), queued: !session.driver.round.queue.is_empty(), ..Default::default() };
        let now = self.now_ms();
        let secrets = self.host.platform().secrets();
        let ctx = self.context(session, secrets.as_deref());
        let runners = self.job_runners.lock().unwrap_or_else(PoisonError::into_inner).clone();

        let look = session.calendar_due(&CalendarFacts { focused: false, backstage: false, reachable, busy }, now);
        for dropped in look.dropped.iter().filter(|d| !d.message.is_empty()).take(20) {
            self.job_error(inner, session, dropped.message.clone());
        }
        for (_, line) in look.said.iter().take(20) {
            self.job_error(inner, session, line.clone());
        }
        for plan in &look.plans {
            // (Only an account of the owner's own: a shared mailbox's calendars take a permission of their own.)
            if !inner.sr.eligible(session, plan.account) || session.mailbox.account(plan.account).is_none_or(|a| a.is_shared()) {
                continue;
            }
            let Some(oauth) = session.mailbox.account(plan.account).and_then(|a| a.oauth.clone()) else { continue };
            let Some(refresh) = inner.sr.refresh_for(session, &ctx, plan.account) else { continue };
            let auth = CalendarAuth { oauth: effective_oauth(oauth, &ctx.ids), refresh, access: String::new(), favorites: false };
            let run = runners.calendar.clone();
            let wake = self.pacer.clone();
            let launch = |req: GraphRequest| Some(self.host.run_job("mailcraft-calendar", move |_| run(req), move || wake.poke()));
            if inner.jobs.calendar.start(session, plan, auth, now, launch) {
                return 1;
            }
        }
        session.calendar_none_started(&look);

        // The refresh tokens first: the engine's look takes the session whole.
        let refreshes: HashMap<Id, String> =
            task_accounts(&session.mailbox).into_iter().filter_map(|a| inner.sr.refresh_for(session, &ctx, a).map(|r| (a, r))).collect();
        let facts = TasksFacts { reachable, running: inner.jobs.tasks.busy(), busy };
        for account in session.tasks_due(&facts, |a| refreshes.get(&a).cloned(), now) {
            if !inner.sr.eligible(session, account) {
                continue;
            }
            let refresh = refreshes.get(&account).cloned();
            let run = runners.tasks.clone();
            let wake = self.pacer.clone();
            let launch = |job: TasksJob| Some(self.host.run_job("mailcraft-tasks", move |_| run(job), move || wake.poke()));
            if inner.jobs.tasks.start(session, account, |_| refresh, |o| effective_oauth(o, &ctx.ids), now, launch) {
                return 1;
            }
        }
        0
    }
}
