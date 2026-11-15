// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keeping the calendar current with an account's server, by itself and without a mail run.
//!
//! What is made, moved, changed or deleted in the calendar here is noted by the engine
//! (`Mailbox::event_changes`) and sent a moment later; what changes on the server (somebody
//! worked in Outlook) is brought down every half minute while the calendar is in front, and at
//! once when the window comes back to the front, when the owner switches to the calendar, and
//! right after something was sent. Otherwise every few minutes will do.
//!
//! Each of these is one small job for the host ([`Services::graph`](crate::Services::graph)):
//! tell the server what waits (when anything does), then ask for the days on screen and a week
//! either side. That is one question when nothing waits. The wide stretch (a month back, a year
//! ahead) and the contacts come in the same kind of job every quarter of an hour, after the
//! Send/Receive round, never in it (#276): no mailbox's mail waits for them.
//!
//! One job at a time, and never while the Microsoft Graph part of a Send/Receive is at work:
//! an answer that was asked for before a change went up could otherwise be taken in after it.
//!
//! A change that the server said it took is looked for in what comes down next (the engine's
//! `EventCheck`): while one is yet to be seen, the calendar is asked for again every few
//! seconds rather than when it is next due. What the server turns out not to have is put back,
//! and what couldn't be sent at all is given up; either way the owner is told, in the status
//! bar and in the Send/Receive problems report, and a line goes to the log (which names no
//! item: a tag made of the event's id is all it says of it).
//!
//! The owner can also have the connection checked on the real account (File › Info › Check
//! Calendar Connection, `ui.checkCalendar`): the host makes, reads, changes and deletes one
//! test appointment a year ahead and reports what the server answered at each step.

use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::remote;
use mailcraft_model::Id;
use mailcraft_model::sync::Tokens;
use serde_json::{Value, json};

use crate::{CalendarCheckRequest, CalendarCheckResult, GraphRequest, GraphResult, MailApp, now_ms};

// How often the calendar is asked for, how long a change waits, how long the server is left alone
// after a job that came to nothing, the stretch asked for and what is remembered between looks are
// the engine's ([`mailcraft_engine::sync_driver::calendar`]); this keeps the jobs on the host.
pub(crate) use mailcraft_engine::sync_driver::consent_wait;
use mailcraft_engine::sync_driver::{AroundRefused, CalendarCame, CalendarFacts, CalendarPlan, CalendarTaken, Start};

/// The calendar's jobs on the host, in the window: the handles only. What is remembered of
/// keeping the calendar current is in the session ([`mailcraft_engine::Session::driver`]).
#[derive(Default)]
pub(crate) struct CalendarSync {
    /// The job in flight: the account, whether it carried changes, when it started
    /// (milliseconds), and the answer to come.
    pub(crate) rx: Option<(Id, bool, f64, u64, Receiver<GraphResult>)>,
    /// A check of an account's calendar connection that the host is making.
    check_rx: Option<(Id, u64, Receiver<CalendarCheckResult>)>,
}

impl CalendarSync {
    /// Another mailbox took the place of the open one: the job and the check in flight are let
    /// go of (AU3-V8).
    pub(crate) fn forget_jobs(&mut self) {
        (self.rx, self.check_rx) = (None, None);
    }
}

impl MailApp {
    /// Let go of the calendar's job and check in flight, and of what the driver knew of them.
    pub(crate) fn forget_calendar_jobs(&mut self) {
        self.calendar.forget_jobs();
        self.session.driver.calendar.forget_jobs();
    }

    /// Whether a calendar job is in flight.
    pub fn calendar_syncing(&self) -> bool {
        self.calendar.rx.is_some()
    }

    /// Look whether the calendar is due, and start a job when it is. Called every frame.
    pub(crate) fn calendar_tick(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        self.calendar_tick_at(focused, now_ms());
        if self.services.graph.is_none() || !self.session.driver.auto_sync() {
            return;
        }
        if self.calendar.rx.is_some() {
            // An answer is on its way (the host wakes the window when it is there, too).
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if !self.session.calendar_accounts().is_empty() {
            // Nothing is happening: look again in a while, sooner when a change waits to go.
            let waits = self.session.driver.calendar.book.any_waiting();
            ctx.request_repaint_after(std::time::Duration::from_secs(if waits { 1 } else { 5 }));
        }
    }

    /// Whether the calendar's job is at work for `account`, or for the owner whose sign-in a
    /// shared mailbox is read with: no other job of it starts until that one has ended.
    pub(crate) fn calendar_job_for(&self, account: Id) -> bool {
        let owner = self.session.mailbox.owner_of(account).map_or(account, |owner| owner.id);
        self.calendar.rx.as_ref().is_some_and(|(busy, ..)| *busy == owner || *busy == account)
    }

    /// A Send/Receive the owner asked for was started for `account`: its calendar and contacts
    /// are asked for in the job that follows the round, the days on screen at least.
    pub(crate) fn calendar_after_round(&mut self, account: Id) {
        self.session.driver.calendar.after_round(account);
    }

    /// The accounts whose calendar a Microsoft server keeps.
    fn microsoft_accounts(&self) -> Vec<Id> {
        self.session.microsoft_calendar_accounts()
    }

    /// [`calendar_tick`](Self::calendar_tick) at `now` (milliseconds), with the window in front
    /// or not. Whether a job was started. (Public for a host's tests, which have no window.)
    /// What is due is the engine's to say ([`mailcraft_engine::Session::calendar_due`]); this
    /// starts the job on the host, and tells the owner.
    pub fn calendar_tick_at(&mut self, focused: bool, now: f64) -> bool {
        let facts = CalendarFacts { focused, backstage: self.ui.backstage, reachable: self.services.graph.is_some(), busy: self.job_busy() };
        let look = self.session.calendar_due(&facts, now);
        self.tell_dropped(&look.dropped);
        // A calendar ticked in a shared mailbox that the owner's permission is missing for: asked.
        if look.may_ask {
            self.shared_calendars_ask_tick();
        }
        for plan in &look.plans {
            if self.calendar_job(plan, now) {
                return true;
            }
        }
        // Nobody to ask: the wish to ask at once doesn't outlive the look.
        self.session.calendar_none_started(&look);
        false
    }

    /// Ask the server of `account` for the days round `day` now, as a calendar job of its own:
    /// for a meeting that should be on the calendar and hasn't come down (an invitation in the
    /// mail that is to be answered, [`crate::reading`]). It only asks: nothing is sent up with
    /// it, and it never starts while another job is at work or while something waits to go up
    /// (that goes first, in a job of its own). The answer is taken in like any other
    /// ([`poll_calendar`](Self::poll_calendar)). Why not, when it can't be asked now.
    pub(crate) fn calendar_job_around(&mut self, account: Id, day: mailcraft_time::Date) -> Result<(), String> {
        if self.services.graph.is_none() {
            return Err(tl!("This build can't reach the account's calendar.").to_string());
        }
        let busy = self.job_busy().blocks(Start::CalendarAround);
        let plan = match self.session.calendar_around(account, day, busy) {
            Ok(plan) => plan,
            Err(AroundRefused::Busy) => return Err(tl!("The calendar is being updated already.").to_string()),
            Err(AroundRefused::Consent) => {
                return Err(tl!("Calendar and contacts need your permission: File › Info › Connect calendar and contacts").to_string());
            }
        };
        if self.calendar_job(&plan, now_ms()) {
            Ok(())
        } else {
            Err(tl!("This account's calendar can't be asked yet: Send/Receive signs it in first.").to_string())
        }
    }

    /// Hand the host the calendar job `plan` says: its changes when it carries any, then the days
    /// asked for. `false` when there is no sign-in to do it with (the owner isn't asked for one:
    /// Send/Receive does that).
    fn calendar_job(&mut self, plan: &CalendarPlan, now: f64) -> bool {
        let account = plan.account;
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        // A shared mailbox's calendars are read with the sign-in of the account it is opened
        // through, and with a token of their own (#70).
        let shared = owner.is_shared();
        let signer = if shared { self.session.mailbox.owner_of(account).cloned() } else { Some(owner.clone()) };
        let Some(signer) = signer else { return false };
        let Some(oauth) = signer.oauth.clone() else { return false };
        let kept = self.tokens.get(&signer.id).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", signer.id)).unwrap_or_default();
        let held = if shared { self.shared_calendar_access(account) } else { self.graph_access(account, false) };
        if refresh.is_empty() && held.is_empty() {
            return false;
        }
        // The Favorites as Outlook has them ride along when they are due ([`crate::favorites_sync`]).
        let by_hand = self.session.driver.calendar.by_hand.remove(&account);
        let favorites = !shared && !plan.around && self.favorites_ask(account, now, by_hand);
        // (A token that is known not to be good for them is not handed on when one that is can
        // be had; without a way of getting another, the one there is has to do.)
        let access = if shared || refresh.is_empty() { held } else { self.graph_access_for(account, favorites) };
        let ask = GraphRequest {
            account: owner,
            oauth: self.effective_oauth(oauth),
            refresh,
            access,
            outbox: Vec::new(),
            directory: plan.directory,
            // (The wide stretch is counted from this session's clock, as its asks are.)
            calendar: Some(plan.range(self.session.now())),
            changes: plan.changes.clone(),
            asks: self.session.calendar_asks_for(plan),
            settings: plan.settings,
            favorites,
            favorites_scopes: favorites,
            mail: None,
            mail_pieces: None,
            cancel: Default::default(),
        };
        let Some(graph) = self.services.graph.as_ref().filter(|_| !self.offline()) else { return false };
        self.session.calendar_started(plan, now);
        self.calendar.rx = Some((account, plan.changes.is_some(), now, self.session.openings(), graph(ask)));
        true
    }

    /// The calendar job answered: keep its token, take in what the server said to the changes
    /// and what it has in the days that were asked for. (Public for a host's tests.)
    pub fn poll_calendar(&mut self) {
        let Some((account, carried, started, opening, rx)) = &self.calendar.rx else { return };
        let (account, carried, started) = (*account, *carried, *started);
        // For a mailbox that has been closed since: its accounts are other accounts, whatever
        // their ids and addresses, and the answer is nobody's (AU3-V8).
        if *opening != self.session.openings() {
            log::info!("calendar: the answer is for a mailbox that is no longer open; let go of");
            self.forget_calendar_jobs();
            return;
        }
        let mut result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("calendar, account {account}: the job ended without an answer (its thread ended early)");
                GraphResult { account, ..Default::default() }
            }
        };
        self.calendar.rx = None;
        // A shared mailbox's calendars (#70): their own token, and their own permission.
        let shared = self.session.mailbox.account(account).is_some_and(mailcraft_model::Account::is_shared);
        if result.consent_needed {
            log::info!("calendar, account {account}: {}", consent_wait(shared));
        }
        if result.sign_in_refused {
            log::warn!("calendar, account {account}: the sign-in was refused; the next Send/Receive asks the owner");
        }
        let mut errors = std::mem::take(&mut result.errors);
        if result.token_refused {
            if shared {
                self.shared_calendar_tokens.remove(&account);
            } else {
                self.graph_tokens.remove(&account);
            }
        }
        if let Some(tokens) = result.tokens.take() {
            if shared {
                let owner = self.session.mailbox.owner_of(account).map(|o| o.id);
                if let Some(owner) = owner.filter(|owner| !self.signed_in_as_other(*owner, &tokens)) {
                    self.keep_shared_calendar_tokens(account, owner, tokens);
                }
            } else {
                self.keep_graph_tokens(account, tokens);
            }
        }
        if shared {
            self.shared_calendars_send_answered(account, &result, &mut errors);
        }
        // What the engine keeps of the answer (the owner's permission among it); the words and the
        // dialogs are the window's, below.
        let came = CalendarCame {
            uploaded: result.uploaded.as_ref(),
            events: result.events.as_ref(),
            more_events: &result.more_events,
            calendars: result.calendars.as_ref(),
            categories: result.categories.as_ref(),
            extras_missing: result.extras_missing,
            hours: result.mailbox_hours.as_ref(),
            contacts: result.contacts.as_ref(),
            job_errors: !errors.is_empty(),
            consent_needed: result.consent_needed,
            token_refused: result.token_refused,
            sign_in_refused: result.sign_in_refused,
        };
        let taken = self.session.calendar_answered(account, carried, started, now_ms(), &came);
        if taken.consent_asked {
            self.status(tl!("Calendar and contacts need your permission: File › Info › Connect calendar and contacts"));
        }
        // The Favorites as Outlook has them, when they came along ([`crate::favorites_sync`]).
        self.take_in_favorites(account, &mut result);
        // Calendars opened by address that the sign-in doesn't let be read yet: the owner is asked (#70).
        if result.shared_calendars_consent_needed {
            self.shared_calendars_consent(account);
        }
        self.say_calendar_taken(account, taken, &mut errors);
        self.note_problems_from("calendar", account, &errors, None, false);
    }

    /// Tell the owner what taking a calendar job's answer in came to: the status bar, the problems
    /// report of the account (`errors`, which the caller reports) and the calendars opened by address.
    pub(crate) fn say_calendar_taken(&mut self, account: Id, taken: CalendarTaken, errors: &mut Vec<String>) {
        let lang = crate::i18n::current();
        for opened in &taken.opened {
            self.take_in_opened(account, opened);
        }
        for said in &taken.status {
            self.status(said.in_lang(lang));
        }
        let problems: Vec<String> = taken.problems.iter().map(|p| p.in_lang(lang)).collect();
        self.keep_sync_problems(account, &problems);
        errors.extend(taken.errors.iter().map(|e| e.in_lang(lang)));
    }

    /// A Graph token that a job obtained: kept for the session, and its refresh token is the
    /// account's newest, for mail and Graph alike.
    pub(crate) fn keep_graph_tokens(&mut self, account: Id, tokens: Tokens) {
        // Somebody else's are neither used nor remembered (#103).
        if self.signed_in_as_other(account, &tokens) {
            return;
        }
        self.note_directory(account, &tokens.tenant);
        if !tokens.refresh.is_empty() {
            self.remember("refresh", account, &tokens.refresh);
            if let Some(mail) = self.tokens.get_mut(&account) {
                mail.refresh = tokens.refresh.clone();
            }
        }
        let now = mailcraft_time::now();
        self.graph_tokens.insert(account, Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens });
    }

    /// Tell the owner of the waiting changes that were given up without being sent: in the
    /// status bar and in the Send/Receive problems report. (The engine wrote the log line.)
    fn tell_dropped(&mut self, dropped: &[remote::Dropped]) {
        for one in dropped.iter().filter(|one| !one.message.is_empty()).take(20) {
            self.status(one.message.clone());
            self.keep_sync_problems(one.account, std::slice::from_ref(&one.message));
        }
    }

    /// What the status bar says after a calendar command that changed an item which is kept on
    /// this computer only, although the account has a calendar on its server: the change goes
    /// nowhere else, and that must not pass for a change to the Outlook calendar.
    pub(crate) fn stays_here_note(&self, id: &str, answer: &Value) -> Option<String> {
        let changes = matches!(
            id,
            "calendar.new"
                | "calendar.newMeeting"
                | "calendar.edit"
                | "calendar.move"
                | "calendar.showAs"
                | "calendar.reminder"
                | "calendar.private"
                | "calendar.categorize"
        );
        let event = answer.get("id").and_then(Value::as_u64).filter(|_| changes)?;
        remote::stays_here(&self.session.mailbox, event)
            .then(|| tl!("Saved on this computer only: this item is not in your Outlook calendar").to_string())
    }

    /// A Graph token as a sign-in leaves it, for a host's tests (which sign nobody in).
    #[doc(hidden)]
    pub fn seed_graph_token(&mut self, account: Id, tokens: Tokens) {
        self.note_directory(account, &tokens.tenant);
        self.tokens.insert(account, tokens.clone());
        self.graph_tokens.insert(account, tokens);
    }

    /// File › Info › Check Calendar Connection (`ui.checkCalendar`): have the host check the
    /// calendar connection of an account on its real server. The owner was asked first (the
    /// question says exactly what is done: [`crate::dialogs::confirmation`]); a script says
    /// `"confirmed": true`. The report comes when the host is done
    /// ([`poll_calendar_check`](Self::poll_calendar_check)).
    pub(crate) fn check_calendar(&mut self, p: &Value) -> Result<Value, String> {
        if p.get("confirmed").and_then(Value::as_bool) != Some(true) {
            return Err(tl!(
                "This makes, changes and deletes a test appointment in the account's real calendar. Say that this is meant with \"confirmed\": true."
            )
            .to_string());
        }
        if self.services.check_calendar.is_none() {
            return Err(tl!("This build can't reach the account's calendar.").to_string());
        }
        if self.calendar.check_rx.is_some() {
            return Err(tl!("The calendar connection is being checked already.").to_string());
        }
        let account = p.get("account").and_then(Value::as_u64).or_else(|| self.microsoft_accounts().first().copied());
        let Some(owner) = account.and_then(|id| self.session.mailbox.account(id)).cloned() else {
            return Err(tl!("No account signs in with Microsoft").to_string());
        };
        let Some(oauth) = owner.oauth.clone().filter(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft) else {
            return Err(tl!("This account doesn't sign in with Microsoft").to_string());
        };
        if self.job_busy().blocks(Start::CalendarCheck) {
            return Err(tl!("The calendar is being updated: try again in a moment.").to_string());
        }
        let account = owner.id;
        let access = self.graph_tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone()).unwrap_or_default();
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", owner.id)).unwrap_or_default();
        if access.is_empty() && refresh.is_empty() {
            return Err(tl!("This account's calendar can't be asked yet: Send/Receive signs it in first.").to_string());
        }
        if self.offline() {
            return Err(crate::send_receive::said());
        }
        let request = CalendarCheckRequest { account: owner, oauth: self.effective_oauth(oauth), refresh, access };
        let Some(check) = &self.services.check_calendar else { return Err(tl!("This build can't reach the account's calendar.").to_string()) };
        self.calendar.check_rx = Some((account, self.session.openings(), check(request)));
        self.status(tl!("Checking the calendar connection…"));
        Ok(json!({"pending": "calendarCheck", "account": account}))
    }

    /// Whether a check of the calendar connection is under way.
    pub fn checking_calendar(&self) -> bool {
        self.calendar.check_rx.is_some()
    }

    /// What the last check of the calendar connection came to, line by line.
    pub fn calendar_check_report(&self) -> &[String] {
        &self.session.driver.calendar.last_check
    }

    /// The host is done checking the calendar connection: keep its token, show what the server
    /// answered at each step in a report, and put the same in the log. (Public for a host's tests.)
    pub fn poll_calendar_check(&mut self) {
        let Some((account, opening, rx)) = &self.calendar.check_rx else { return };
        let account = *account;
        if *opening != self.session.openings() {
            log::info!("calendar check: the answer is for a mailbox that is no longer open; let go of");
            self.calendar.check_rx = None;
            return;
        }
        let mut result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("calendar check, account {account}: the check ended without an answer (its thread ended early)");
                CalendarCheckResult { account, error: tl!("the check stopped unexpectedly").to_string(), cleaned_up: true, ..Default::default() }
            }
        };
        self.calendar.check_rx = None;
        if result.token_refused {
            self.graph_tokens.remove(&account);
        }
        if let Some(tokens) = result.tokens.take() {
            self.keep_graph_tokens(account, tokens);
            // (Asked for as tokens always were: not known to be good for Outlook's Favorites.)
            self.favorites_token_is_plain(account);
        }
        if result.consent_needed && self.session.driver.calendar.consent_needed.insert(account) {
            self.status(tl!("Calendar and contacts need your permission: File › Info › Connect calendar and contacts"));
        }
        let whose = self
            .session
            .mailbox
            .account(account)
            .map(|a| if a.email.trim().is_empty() { a.name.clone() } else { a.email.clone() })
            .unwrap_or_default();
        let mut lines: Vec<String> = Vec::new();
        let verdict = if !result.error.is_empty() {
            crate::i18n::fmt(tl!("The calendar connection couldn't be checked: {error}"), &[("error", &result.error)])
        } else if result.ok {
            tl!("The calendar connection works: what was sent arrived, and the server has it as it was sent.").to_string()
        } else {
            tl!("Something did not go through, or did not arrive as it was sent. The steps below say what.").to_string()
        };
        lines.push(format!("{whose}: {verdict}"));
        for (n, step) in result.steps.iter().take(20).enumerate() {
            let answered = if step.status == 0 {
                tl!("no answer").to_string()
            } else {
                crate::i18n::fmt(tl!("the server answered {status}"), &[("status", &step.status.to_string())])
            };
            let how = if step.ok { tl!("OK") } else { tl!("FAILED") };
            let line = crate::i18n::fmt(
                tl!("{n}. {step}: {result} ({answered}). {said}"),
                &[("n", &(n + 1).to_string()), ("step", tl!(&step.what)), ("result", how), ("answered", &answered), ("said", &step.said)],
            );
            // What was sent, what the server said, and when: nothing of anybody's calendar.
            if step.ok {
                log::info!("calendar check, account {account}: {line}");
            } else {
                log::warn!("calendar check, account {account}: {line}");
            }
            lines.push(line);
        }
        if !result.steps.is_empty() {
            lines.push(if result.cleaned_up {
                tl!("The test appointment is gone again. Nothing else in the calendar was read or changed.").to_string()
            } else {
                tl!(
                    "The test appointment (\"MailCraft connection check\", a year from today) could not be deleted: please delete it in the calendar. Nothing else was read or changed."
                )
                .to_string()
            });
        }
        log::info!(
            "calendar check, account {account}: {}",
            if result.ok { "everything arrived as it was sent" } else { "not everything arrived as it was sent" }
        );
        self.session.driver.calendar.last_check = lines.clone();
        self.status(if result.ok {
            tl!("The calendar connection works")
        } else {
            tl!("The calendar connection check found a problem: see its report")
        });
        self.dialog = Some(crate::dialogs::Dialog::Report { name: "calendarCheck", title: tl!("Calendar Connection Check").to_string(), lines });
        // The calendar is looked at again at once, when it is in front.
        self.session.driver.calendar.due = true;
    }

    /// The calendar part of the Microsoft Graph part of a Send/Receive: a calendar that came
    /// with it needn't be asked for again right away. Whether the main calendar's events came.
    pub(crate) fn take_in_calendar_of_sync(&mut self, account: Id, came: &CalendarCame<'_>, errors: &mut Vec<String>) -> bool {
        let mut taken = CalendarTaken::default();
        self.session.calendar_list_in(account, came.calendars, came.categories, came.extras_missing, &mut taken);
        let ok = self.session.take_in_calendar(account, came.uploaded, came.events, &mut taken);
        self.session.more_calendars_in(came.more_events, &mut taken);
        self.say_calendar_taken(account, taken, errors);
        if ok {
            self.session.driver.calendar.book.started(account, now_ms());
        }
        ok
    }

    /// The list of calendars and categories that came with a job (for a host's tests).
    #[cfg(test)]
    pub(crate) fn take_in_calendar_list(
        &mut self,
        account: Id,
        calendars: Option<&mailcraft_model::remote::CalendarsDownload>,
        categories: Option<&mailcraft_model::remote::CategoriesDownload>,
        extras_missing: bool,
        errors: &mut Vec<String>,
    ) {
        let mut taken = CalendarTaken::default();
        self.session.calendar_list_in(account, calendars, categories, extras_missing, &mut taken);
        self.say_calendar_taken(account, taken, errors);
    }

    /// The working hours the mailbox has set came with a job (for a host's tests).
    #[cfg(test)]
    pub(crate) fn take_in_mailbox_hours(&mut self, account: Id, hours: &mailcraft_engine::cmd::calendar_settings::MailboxHours) {
        self.session.mailbox_hours_in(account, hours);
    }

    /// Whether the sign-in of `account` covers everything but an optional extra (reading the
    /// mailbox's own list of categories): the calendar works as it did, and "Connect calendar
    /// and contacts" would add it. Quieter than [`graph_consent_needed`](Self::graph_consent_needed):
    /// nothing is disconnected.
    pub fn graph_extras_missing(&self, account: Id) -> bool {
        self.session.driver.calendar.extras_missing.contains(&account)
    }

    /// The owner gave Microsoft Graph its permissions for `account` ("Connect calendar and
    /// contacts"): what was held back for the lack of them goes at once.
    pub(crate) fn calendar_connected(&mut self, account: Id) {
        remote::retry_event_changes(&mut self.session, account);
        self.session.driver.calendar.connected(account);
    }
}

/// A stand-in for Microsoft Graph's calendar, for the tests of this crate: it keeps events, takes
/// the changes it is told, and hands back what it has.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_model::remote::{
        ContactsDownload, EventChangeKind, EventOp, EventOutcome, EventResult, EventsDownload, EventsUploaded, RemoteEvent, RemoteEventKind,
    };
    use mailcraft_model::sync::Tokens;
    use mailcraft_model::{Id, Response};
    use mailcraft_time::DateTime;
    use serde_json::json;

    use crate::{GraphRequest, GraphResult, MailApp};

    /// One job the stand-in was handed.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub(crate) struct Job {
        /// The changes it was told (none when there were none).
        pub ops: Vec<EventOp>,
        /// The stretch of the calendar that was asked for.
        pub calendar: Option<(i64, i64)>,
        /// Whether the wide stretch and the contacts were (a Send/Receive's part).
        pub directory: bool,
        pub had_token: bool,
        /// The series the mailbox said it expects in the stretch of the main calendar.
        pub series: Vec<String>,
        /// Whether the mailbox's working hours were asked for.
        pub settings: bool,
    }

    #[derive(Default)]
    pub(crate) struct Server {
        /// The events it has: single ones, series (with their rule) and the occurrences of
        /// series that were changed on their own.
        pub events: Vec<RemoteEvent>,
        pub jobs: Vec<Job>,
        /// It can't be reached.
        pub down: bool,
        /// The owner hasn't agreed to what is asked.
        pub consent_needed: bool,
        /// The owner agreed to everything but the optional extra.
        pub extras_missing: bool,
        /// The working hours its mailbox has set.
        pub hours: Option<mailcraft_engine::cmd::calendar_settings::MailboxHours>,
        /// What it says to every change instead of making it.
        pub say: Option<EventResult>,
        /// The mail it sent to other people, as a real server would on being told these
        /// things: `invitation:Title:people`, `update:Title:people`, `cancellation:Title:people:note`,
        /// `response:Title:answer:note`, `proposal:Title:start-end` (after the response it came
        /// with), `forward:Title:people:note`.
        pub mail: Vec<String>,
        /// Its account can't make Teams meetings: an event that was to be one is made without.
        pub no_teams: bool,
        /// The calendar each event is in that isn't in the account's main one (by the event's
        /// id; what the server calls the calendar).
        pub placed: std::collections::HashMap<String, String>,
        /// The occurrences of series that are no longer the series' own: deleted, or changed
        /// into events of their own (the series, and the day of the occurrence, counted from 1970).
        pub taken: HashSet<(String, i64)>,
        made: usize,
    }

    /// Whether the owner organizes a meeting with people in it: what a server writes mail about.
    fn meeting(event: &RemoteEvent) -> bool {
        !event.attendees.is_empty() && event.is_organizer != Some(false)
    }

    impl Server {
        fn change(&mut self, op: &EventOp) -> EventOutcome {
            let mut outcome = EventOutcome { op: op.clone(), result: self.say.unwrap_or(EventResult::Done), ..Default::default() };
            if outcome.result != EventResult::Done {
                outcome.error = "the stand-in said so".into();
                return outcome;
            }
            let version = format!("ck-{}", self.jobs.len());
            let put = |event: &mut RemoteEvent| {
                let fields = &op.fields;
                if let Some(title) = &fields.title {
                    event.title = title.clone();
                }
                if let Some(place) = &fields.location {
                    event.location = place.clone();
                }
                if let Some(words) = &fields.description {
                    event.description = words.clone();
                }
                if let Some(time) = fields.time {
                    (event.start, event.end, event.all_day) = (time.start, time.end, time.all_day);
                }
                if let Some(show_as) = fields.show_as {
                    event.show_as = show_as;
                }
                if let Some(private) = fields.private {
                    event.private = private;
                }
                if let Some(reminder) = fields.reminder {
                    event.reminder_minutes = reminder.on.then_some(reminder.minutes);
                }
                if let Some(repeat) = &fields.repeat {
                    event.recurrence = repeat.rule.clone();
                    event.series_day = repeat.rule.as_ref().map(|_| repeat.first);
                    event.kind = if repeat.rule.is_some() { RemoteEventKind::SeriesMaster } else { RemoteEventKind::Single };
                }
                if let Some(people) = &fields.attendees {
                    event.attendees = people.clone();
                }
                if let Some(categories) = &fields.categories {
                    event.categories = categories.clone();
                }
                if let Some(importance) = fields.importance {
                    event.importance = importance;
                }
                if let Some(asked) = fields.request_responses {
                    event.options.request_responses = asked;
                }
                if let Some(allowed) = fields.allow_proposals {
                    event.options.allow_proposals = allowed;
                }
                event.change_key = version.clone();
            };
            // An event that is to be held online: made so, with a link to join by, or (an
            // account that can't) left as it is, which the answer then says.
            let no_teams = self.no_teams;
            let online = |event: &mut RemoteEvent, outcome: &mut EventOutcome| {
                let Some(provider) = op.fields.online else { return };
                if no_teams {
                    (outcome.online, outcome.error) = (Some(false), "the stand-in has no Teams".into());
                } else {
                    let join_url = format!("https://teams.example.org/l/meetup-join/{}", event.id);
                    event.online = Some(mailcraft_model::OnlineMeeting { provider, join_url: join_url.clone() });
                    (outcome.online, outcome.join_url) = (Some(true), join_url);
                }
            };
            // What the people invited are written to about: anything but how it shows to the owner.
            let seen = {
                let f = &op.fields;
                f.title.is_some() || f.location.is_some() || f.description.is_some() || f.time.is_some() || f.private.is_some() || f.repeat.is_some()
            } || op.fields.attendees.is_some()
                || op.fields.importance.is_some()
                || op.fields.request_responses.is_some()
                || op.fields.allow_proposals.is_some()
                || op.fields.online.is_some();
            // One occurrence of a series, known by its series and its time.
            if op.of_occurrence() {
                let Some(series) = self.events.iter().find(|event| event.id == op.series && event.recurrence.is_some()).cloned() else {
                    outcome.result = EventResult::Gone;
                    return outcome;
                };
                if !self.taken.insert((op.series.clone(), op.occurrence.div_euclid(86_400))) {
                    outcome.result = EventResult::Gone;
                    return outcome;
                }
                match op.kind {
                    EventChangeKind::Update => {
                        self.made += 1;
                        let mut one = RemoteEvent {
                            id: format!("x-{}", self.made),
                            kind: RemoteEventKind::Exception,
                            series: op.series.clone(),
                            original: Some(op.occurrence),
                            start: op.occurrence,
                            end: op.occurrence + (series.end - series.start),
                            recurrence: None,
                            series_day: None,
                            ..series.clone()
                        };
                        put(&mut one);
                        if meeting(&one) && seen {
                            self.mail.push(format!("update:{}:{}", one.title, one.attendees.len()));
                        }
                        outcome.remote = one.id.clone();
                        self.events.push(one);
                    }
                    EventChangeKind::Delete if meeting(&series) => {
                        self.mail.push(format!("cancellation:{}:{}:{}", series.title, series.attendees.len(), op.comment))
                    }
                    EventChangeKind::Respond if op.send => {
                        self.mail.push(format!("response:{}:{:?}:{}", series.title, op.response, op.comment));
                        if let Some(time) = op.proposed {
                            self.mail.push(format!("proposal:{}:{}-{}", series.title, time.start, time.end));
                        }
                    }
                    EventChangeKind::Forward if op.send && !op.to.is_empty() => {
                        self.mail.push(format!("forward:{}:{}:{}", series.title, op.to.len(), op.comment))
                    }
                    _ => {}
                }
                outcome.change_key = version;
                return outcome;
            }
            match op.kind {
                EventChangeKind::Create => {
                    self.made += 1;
                    let mut event =
                        RemoteEvent { id: format!("made-{}", self.made), token: op.token.clone(), is_organizer: Some(true), ..Default::default() };
                    put(&mut event);
                    online(&mut event, &mut outcome);
                    if meeting(&event) {
                        self.mail.push(format!("invitation:{}:{}", event.title, event.attendees.len()));
                    }
                    if !op.calendar.is_empty() {
                        self.placed.insert(event.id.clone(), op.calendar.clone());
                    }
                    outcome.remote = event.id.clone();
                    self.events.push(event);
                }
                EventChangeKind::Update => match self.events.iter_mut().find(|event| event.id == op.remote) {
                    Some(event) => {
                        let before = event.attendees.len();
                        put(event);
                        online(event, &mut outcome);
                        if (meeting(event) || before > 0) && event.is_organizer != Some(false) && seen {
                            let (title, people) = (event.title.clone(), event.attendees.len().max(before));
                            self.mail.push(format!("update:{title}:{people}"));
                        }
                        outcome.remote = op.remote.clone();
                    }
                    None => outcome.result = EventResult::Gone,
                },
                EventChangeKind::Delete => match self.events.iter().position(|event| event.id == op.remote) {
                    Some(at) => {
                        let event = self.events.remove(at);
                        // Deleting a meeting the owner called tells everybody, asked to or not.
                        if meeting(&event) {
                            self.mail.push(format!("cancellation:{}:{}:{}", event.title, event.attendees.len(), op.comment));
                        }
                        self.events.retain(|other| other.series != event.id);
                    }
                    None => outcome.result = EventResult::Gone,
                },
                // An answer: kept with the event; one that is declined goes out of the calendar.
                EventChangeKind::Respond => match self.events.iter().position(|event| event.id == op.remote) {
                    Some(at) => {
                        let title = self.events.get(at).map(|event| event.title.clone()).unwrap_or_default();
                        if op.send {
                            self.mail.push(format!("response:{title}:{:?}:{}", op.response, op.comment));
                            // Another time, proposed with the answer.
                            if let Some(time) = op.proposed {
                                self.mail.push(format!("proposal:{title}:{}-{}", time.start, time.end));
                            }
                        }
                        if op.response == Response::Declined {
                            self.events.remove(at);
                        } else if let Some(event) = self.events.get_mut(at) {
                            event.response = op.response;
                            event.change_key = version.clone();
                        }
                    }
                    None => outcome.result = EventResult::Gone,
                },
                // Sent on: mail to the people named, and nothing else. (Never without the yes
                // that an upload carries, which is what a real server is never asked without.)
                EventChangeKind::Forward => match self.events.iter().find(|event| event.id == op.remote) {
                    Some(event) if op.send && !op.to.is_empty() => self.mail.push(format!("forward:{}:{}:{}", event.title, op.to.len(), op.comment)),
                    Some(_) => outcome.result = EventResult::Refused,
                    None => outcome.result = EventResult::Gone,
                },
            }
            outcome.change_key = version;
            outcome
        }

        /// What a stretch of the calendar holds: every event of its own, and every series as
        /// itself and as its occurrences in the stretch (by the clock in UTC, which is the zone
        /// these tests run in), less those that were deleted or changed on their own.
        fn stretch(&self, from: i64, to: i64) -> Vec<RemoteEvent> {
            let mut out = Vec::new();
            let day = |instant: i64| DateTime::from_timestamp(instant.clamp(0, 4_000_000_000), 0).date;
            // (Of the account's main calendar: what was put into another one is that one's.)
            for event in self.events.iter().filter(|event| !self.placed.contains_key(&event.id)) {
                let Some(rule) = &event.recurrence else {
                    out.push(event.clone());
                    continue;
                };
                let first = DateTime::from_timestamp(event.start, 0);
                let mut any = false;
                for on in rule.dates(first.date, day(from), day(to), &[]) {
                    let start = DateTime { date: on, ..first }.to_timestamp(0);
                    if (from..to).contains(&start) && !self.taken.contains(&(event.id.clone(), start.div_euclid(86_400))) {
                        any = true;
                        out.push(RemoteEvent {
                            id: format!("{}@{start}", event.id),
                            kind: RemoteEventKind::Occurrence,
                            series: event.id.clone(),
                            original: Some(start),
                            start,
                            end: start + (event.end - event.start),
                            recurrence: None,
                            series_day: None,
                            ..event.clone()
                        });
                    }
                }
                // The series itself comes with its occurrences, or when it is asked for by name.
                if any || self.jobs.last().is_some_and(|job| job.series.contains(&event.id)) {
                    out.push(RemoteEvent { kind: RemoteEventKind::SeriesMaster, series_day: Some(first.date), ..event.clone() });
                }
            }
            out
        }

        fn answer(&mut self, request: GraphRequest) -> GraphResult {
            let mut out = GraphResult { account: request.account.id, ..Default::default() };
            let series = request.asks.iter().find(|ask| ask.calendar.is_empty()).map(|ask| ask.series.clone()).unwrap_or_default();
            self.jobs.push(Job {
                ops: request.changes.as_ref().map(|changes| changes.ops.clone()).unwrap_or_default(),
                calendar: request.calendar,
                directory: request.directory,
                had_token: !request.access.is_empty(),
                series: series.clone(),
                settings: request.settings,
            });
            if self.consent_needed {
                out.consent_needed = true;
                return out;
            }
            if self.down {
                out.errors.push("Couldn't reach Microsoft for the calendar: the stand-in is down".into());
                return out;
            }
            out.extras_missing = self.extras_missing;
            // The mailbox's settings are one of the extras.
            if request.settings && !self.extras_missing {
                out.mailbox_hours = self.hours.clone();
            }
            if request.access.is_empty() {
                out.tokens = Some(Tokens {
                    access: "graph-tok".into(),
                    refresh: "ref-2".into(),
                    expires_at: mailcraft_time::now() + 3600,
                    ..Default::default()
                });
            }
            if let Some(changes) = &request.changes {
                let outcomes = changes.ops.iter().map(|op| self.change(op)).collect();
                out.uploaded = Some(EventsUploaded { account: changes.account, email: changes.email.clone(), outcomes, ..Default::default() });
            }
            // After the changes: what comes down is what there is now.
            let stretch = if request.directory { Some((0, i64::MAX / 4)) } else { request.calendar };
            if let Some((from, to)) = stretch {
                // A series that was asked for by name and is no longer here is said to be gone.
                let gone = series.into_iter().filter(|name| !self.events.iter().any(|event| event.id == *name)).collect();
                let all = EventsDownload {
                    from,
                    to,
                    complete: true,
                    events: self.stretch(from, to),
                    gone,
                    has_categories: true,
                    has_options: true,
                    ..Default::default()
                };
                out.events = Some(all.for_account(&request.account));
                // The account's other calendars that were asked for: what was put into each.
                for ask in request.asks.iter().filter(|ask| !ask.calendar.is_empty()) {
                    let there = |event: &&RemoteEvent| self.placed.get(&event.id) == Some(&ask.calendar) && (from..to).contains(&event.start);
                    let other = EventsDownload {
                        calendar: ask.calendar.clone(),
                        from,
                        to,
                        complete: true,
                        events: self.events.iter().filter(there).cloned().collect(),
                        has_categories: true,
                        has_options: true,
                        ..Default::default()
                    };
                    out.more_events.push(other.for_account(&request.account));
                }
            }
            if request.directory {
                out.contacts = Some(ContactsDownload { complete: true, ..Default::default() }.for_account(&request.account));
            }
            out
        }
    }

    /// Give `app` a stand-in for Graph, which answers at once.
    pub(crate) fn serve(app: &mut MailApp) -> Arc<Mutex<Server>> {
        let server: Arc<Mutex<Server>> = Default::default();
        let shared = server.clone();
        app.services.graph = Some(Box::new(move |request: GraphRequest| {
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(shared.lock().unwrap().answer(request));
            rx
        }));
        server
    }

    /// An account that signs in with Microsoft, with a Graph token at hand. Its id.
    pub(crate) fn microsoft_account(app: &mut MailApp) -> Id {
        let added = app.execute(
            "account.add",
            json!({"email": "avery@mail.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "imap.mail.example"}}),
        );
        let id = added.unwrap()["id"].as_u64().unwrap();
        let fresh = Tokens { access: "graph-tok".into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() };
        app.tokens.insert(id, fresh.clone());
        app.graph_tokens.insert(id, fresh);
        id
    }

    /// An event an hour long as the stand-in has it, the owner's own.
    pub(crate) fn event(id: &str, title: &str, start: i64) -> RemoteEvent {
        RemoteEvent {
            id: id.into(),
            change_key: "ck-0".into(),
            title: title.into(),
            start,
            end: start + 3600,
            is_organizer: Some(true),
            ..Default::default()
        }
    }
}

#[cfg(test)]
#[path = "calendar_off_path_tests.rs"]
mod calendar_off_path_tests;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mailcraft_engine::Session;
    use mailcraft_model::remote::{EventChangeKind, EventResult};
    use serde_json::json;

    use super::testing::{Job, Server, event, microsoft_account, serve};
    use super::*;
    use crate::Services;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;
    const SECOND: f64 = 1_000.0;

    /// The sample mailbox with a Microsoft account, Mail in front, and a stand-in for Graph that
    /// has one event today.
    fn app() -> (MailApp, Arc<Mutex<Server>>, Id) {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        let server = serve(&mut app);
        let account = microsoft_account(&mut app);
        // (The wide stretch and the contacts, due at start-up, are not what these look at.)
        app.session.driver.calendar.checked.insert(account, f64::MAX);
        server.lock().unwrap().events = vec![event("e-1", "Budget review", NOW - 3 * 3600)];
        (app, server, account)
    }

    fn jobs(server: &Arc<Mutex<Server>>) -> usize {
        server.lock().unwrap().jobs.len()
    }

    /// Let the job in flight answer.
    fn settle(app: &mut MailApp) {
        for _ in 0..4 {
            app.poll_calendar();
        }
    }

    #[test]
    fn the_calendar_is_asked_for_when_it_is_due_and_no_more_often() {
        let (mut app, server, account) = app();
        let t0 = now_ms();
        // Mail is in front. The calendar is asked for once to begin with: the days on screen,
        // nothing else, with the token that is at hand.
        assert!(app.calendar_tick_at(true, t0));
        assert!(app.calendar_syncing());
        // One job at a time.
        assert!(!app.calendar_tick_at(true, t0 + SECOND));
        settle(&mut app);
        assert!(!app.calendar_syncing());
        let first = server.lock().unwrap().jobs[0].clone();
        assert_eq!((first.ops.len(), first.directory, first.had_token), (0, false, true));
        let (from, to) = first.calendar.expect("the days on screen were asked for");
        // The week of the 14th (the 11th to the 17th) and a week either side.
        assert_eq!((from, to), (NOW - 16 * 3600 - 10 * 86_400, NOW - 16 * 3600 + 11 * 86_400));
        // What the server has is in the mailbox, in a calendar under the account.
        assert!(app.session.mailbox.events.values().any(|e| e.title == "Budget review" && e.origin.account == account));
        // Behind the mail, every five minutes will do.
        assert!(!app.calendar_tick_at(true, t0 + 60.0 * SECOND));
        assert!(!app.calendar_tick_at(true, t0 + 299.0 * SECOND));
        assert!(app.calendar_tick_at(true, t0 + 301.0 * SECOND));
        settle(&mut app);
        assert_eq!(jobs(&server), 2);

        // The owner switches to the calendar: at once.
        let t1 = t0 + 310.0 * SECOND;
        app.execute("view.calendar", json!({})).unwrap();
        assert!(app.calendar_tick_at(true, t1));
        settle(&mut app);
        assert_eq!(jobs(&server), 3);
        // In front, about every half minute.
        assert!(!app.calendar_tick_at(true, t1 + SECOND));
        assert!(!app.calendar_tick_at(true, t1 + 29.0 * SECOND));
        assert!(app.calendar_tick_at(true, t1 + 31.0 * SECOND));
        settle(&mut app);
        assert_eq!(jobs(&server), 4);
        // The window goes behind another and comes back: at once.
        let t2 = t1 + 40.0 * SECOND;
        assert!(!app.calendar_tick_at(false, t2));
        assert!(app.calendar_tick_at(true, t2 + 2.0 * SECOND));
        settle(&mut app);
        assert_eq!(jobs(&server), 5);
        // And not again for looking at it: only the half minute brings the next.
        assert!(!app.calendar_tick_at(true, t2 + 3.0 * SECOND));
        // The File page is in front of the calendar: it is behind, like any other module.
        app.execute("ui.backstage", json!({"value": true})).unwrap();
        assert!(!app.calendar_tick_at(true, t2 + 40.0 * SECOND));
        app.execute("ui.backstage", json!({"value": false})).unwrap();
        assert!(app.calendar_tick_at(true, t2 + 41.0 * SECOND));
        settle(&mut app);

        // What was made in Outlook meanwhile is here after the next look.
        server.lock().unwrap().events.push(event("e-2", "Made in Outlook", NOW + 3600));
        assert!(app.calendar_tick_at(true, t2 + 80.0 * SECOND));
        settle(&mut app);
        assert!(app.session.mailbox.events.values().any(|e| e.title == "Made in Outlook"));
        // Not while a Send/Receive is asking Graph, or is about to; and not when nothing may
        // start by itself.
        let t3 = t2 + 200.0 * SECOND;
        app.session.driver.round.queue.push_back((account, true));
        assert!(!app.calendar_tick_at(true, t3));
        app.session.driver.round.queue.clear();
        app.session.driver.set_auto_sync(false);
        assert!(!app.calendar_tick_at(true, t3));
        app.session.driver.set_auto_sync(true);
        assert!(app.calendar_tick_at(true, t3));
        settle(&mut app);
    }

    #[test]
    fn a_change_goes_at_once_and_the_calendar_comes_back_with_it() {
        let (mut app, server, account) = app();
        app.execute("view.calendar", json!({})).unwrap();
        let t0 = now_ms();
        assert!(app.calendar_tick_at(true, t0));
        settle(&mut app);
        let review = app.session.mailbox.events.values().find(|e| e.title == "Budget review").map(|e| e.id).unwrap();
        // Nothing is due for half a minute; a change made here doesn't wait for that.
        assert!(!app.calendar_tick_at(true, t0 + SECOND));
        app.execute("calendar.move", json!({"id": review, "start": "2026-10-15T09:00"})).unwrap();
        assert_eq!(app.session.mailbox.event_changes.len(), 1);
        assert!(app.calendar_tick_at(true, t0 + 2.0 * SECOND));
        settle(&mut app);
        // One job: the change, and then the days on screen, which have the event where it is now.
        let job: Job = server.lock().unwrap().jobs.last().cloned().unwrap();
        assert_eq!(job.ops.iter().map(|op| (op.kind, op.remote.as_str())).collect::<Vec<_>>(), [(EventChangeKind::Update, "e-1")]);
        assert!(job.calendar.is_some() && !job.directory);
        assert!(app.session.mailbox.event_changes.is_empty());
        let event = app.session.mailbox.event(review).unwrap();
        assert_eq!((event.start.iso(), event.origin.change_key.as_str()), ("2026-10-15T09:00:00".to_string(), "ck-2"));
        assert_eq!(server.lock().unwrap().events[0].start, NOW - 7 * 3600 + 86_400);
        // A new event, typed or otherwise: made on the server, and here once, not twice.
        app.execute("calendar.new", json!({"title": "Lunch", "start": "2026-10-15T12:00"})).unwrap();
        assert!(app.calendar_tick_at(true, t0 + 3.0 * SECOND));
        settle(&mut app);
        assert_eq!(app.session.mailbox.events.values().filter(|e| e.title == "Lunch").count(), 1);
        let lunch = app.session.mailbox.events.values().find(|e| e.title == "Lunch").unwrap();
        assert_eq!((lunch.origin.id.as_str(), lunch.origin.account), ("made-1", account));
        assert!(app.session.mailbox.event_changes.is_empty() && !app.has_sync_problems());

        // The server refuses a change for good: it is put back, and the status bar says why.
        server.lock().unwrap().say = Some(EventResult::Refused);
        app.execute("calendar.move", json!({"id": review, "start": "2026-10-16T09:00"})).unwrap();
        assert!(app.calendar_tick_at(true, t0 + 4.0 * SECOND));
        settle(&mut app);
        assert_eq!(app.session.mailbox.event(review).map(|e| e.start.iso()), Some("2026-10-15T09:00:00".to_string()));
        let said = app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
        assert_eq!(said, "The server wouldn't take the change to \"Budget review\" (the stand-in said so). It was put back as it was.");
        assert!(app.session.mailbox.event_changes.is_empty() && app.has_sync_problems());
    }

    #[test]
    fn a_server_that_does_not_answer_is_not_hammered() {
        let (mut app, server, account) = app();
        app.execute("view.calendar", json!({})).unwrap();
        server.lock().unwrap().down = true;
        let t0 = now_ms();
        assert!(app.calendar_tick_at(true, t0));
        settle(&mut app);
        assert!(app.has_sync_problems());
        // In front, with the window coming back again and again: not before half a minute is up.
        for seconds in [1.0, 5.0, 29.0] {
            assert!(!app.calendar_tick_at(false, t0 + seconds * SECOND - 500.0));
            assert!(!app.calendar_tick_at(true, t0 + seconds * SECOND), "{seconds}");
        }
        assert!(app.calendar_tick_at(true, t0 + 31.0 * SECOND));
        settle(&mut app);
        // Twice as long after every failure, ten minutes at most.
        let mut at = t0 + 31.0 * SECOND;
        for wait in [60.0, 120.0, 240.0, 480.0, 600.0, 600.0] {
            assert!(!app.calendar_tick_at(true, at + (wait - 1.0) * SECOND), "{wait}");
            at += (wait + 1.0) * SECOND;
            assert!(app.calendar_tick_at(true, at), "{wait}");
            settle(&mut app);
        }
        assert_eq!(jobs(&server), 8);
        // What is made meanwhile is made all the same (here: on this computer, as no calendar of
        // the server's has come down yet), and doesn't cut the waiting short.
        let made = app.execute("calendar.new", json!({"title": "Offline", "start": "2026-10-15T12:00"}));
        assert!(made.is_ok() && !app.calendar_tick_at(true, at + SECOND));
        // The server is back: the next look brings the calendar, and the waiting is over.
        server.lock().unwrap().down = false;
        at += 601.0 * SECOND;
        assert!(app.calendar_tick_at(true, at));
        settle(&mut app);
        assert!(app.session.mailbox.event_changes.is_empty());
        assert!(app.calendar_tick_at(true, at + 31.0 * SECOND));
        settle(&mut app);

        // No permission: the owner is told once, and Graph is left alone until it is given.
        server.lock().unwrap().consent_needed = true;
        at += 100.0 * SECOND;
        assert!(app.calendar_tick_at(true, at));
        settle(&mut app);
        assert!(app.graph_consent_needed(account));
        let asked = jobs(&server);
        for seconds in [31.0, 301.0, 3_600.0] {
            assert!(!app.calendar_tick_at(true, at + seconds * SECOND));
        }
        assert_eq!(jobs(&server), asked);
    }

    #[test]
    fn a_change_the_account_may_not_make_asks_for_permission_and_waits() {
        let (mut app, server, account) = app();
        app.execute("view.calendar", json!({})).unwrap();
        let t0 = now_ms();
        assert!(app.calendar_tick_at(true, t0));
        settle(&mut app);
        let review = app.session.mailbox.events.values().find(|e| e.title == "Budget review").map(|e| e.id).unwrap();
        // The sign-in is from before the calendar was written to: the server says 403.
        server.lock().unwrap().say = Some(EventResult::Denied);
        app.execute("calendar.move", json!({"id": review, "start": "2026-10-15T09:00"})).unwrap();
        assert!(app.calendar_tick_at(true, t0 + SECOND));
        settle(&mut app);
        // The change is kept, the owner is asked to connect, and nothing more is asked of Graph.
        assert!(app.graph_consent_needed(account));
        assert_eq!(app.session.mailbox.event(review).map(|e| e.start.iso()), Some("2026-10-15T09:00:00".to_string()));
        assert_eq!(app.session.mailbox.event_changes.len(), 1);
        let said = app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
        assert_eq!(said, "Changing the calendar needs your permission: File › Info › Connect calendar and contacts");
        assert!(!app.calendar_tick_at(true, t0 + 400.0 * SECOND));
        // "Connect calendar and contacts": the permission is given, and the change goes.
        server.lock().unwrap().say = None;
        app.session.driver.calendar.consent_needed.remove(&account);
        app.calendar_connected(account);
        assert!(app.calendar_tick_at(true, t0 + 401.0 * SECOND));
        settle(&mut app);
        assert!(app.session.mailbox.event_changes.is_empty());
        assert_eq!(server.lock().unwrap().events[0].start, NOW - 7 * 3600 + 86_400);
    }

    use crate::dialogs::{Dialog, Question};
    use mailcraft_model::remote::{RemoteEvent, RemoteEventKind};
    use mailcraft_model::{Address, Attendee, Freq, Recurrence, Response};

    /// The app with the calendar in front and what the stand-in has taken in. `events` are what
    /// the stand-in has beside the one of [`app`].
    fn calendar(events: Vec<RemoteEvent>) -> (MailApp, Arc<Mutex<Server>>, Id, f64) {
        let (mut app, server, account) = app();
        server.lock().unwrap().events.extend(events);
        app.execute("view.calendar", json!({})).unwrap();
        let t0 = now_ms();
        assert!(app.calendar_tick_at(true, t0));
        settle(&mut app);
        (app, server, account, t0)
    }

    /// Let what waits go to the stand-in, and take in what it says.
    fn sync(app: &mut MailApp, at: &mut f64) {
        *at += 31.0 * SECOND;
        app.calendar_tick_at(true, *at);
        settle(app);
    }

    /// The question that is being asked: its title, the button that goes ahead, and the others.
    fn asked(app: &MailApp) -> Option<(String, String, Vec<String>)> {
        match &app.dialog {
            Some(Dialog::Confirm { ask, .. }) => {
                Some((ask.title.clone(), ask.action.clone(), ask.more.iter().map(|(label, _)| label.clone()).collect()))
            }
            _ => None,
        }
    }

    /// Answer the question that is being asked with its button `label`, and with a note when
    /// one is written.
    fn answer(app: &mut MailApp, label: &str, note: &str) {
        let Some(Dialog::Confirm { command, params, ask }) = app.dialog.take() else { panic!("no question is being asked") };
        let Question { action, with, more, note: field, .. } = ask;
        let with = if action == label {
            with
        } else {
            more.into_iter().find(|(other, _)| other == label).map(|(_, with)| with).unwrap_or_else(|| panic!("no button {label}"))
        };
        let params = crate::dialogs::answered(&params, &with, field.as_ref().map(|(key, _, _)| (*key, note)));
        crate::dialogs::go_ahead(app, &command, params);
    }

    fn mail(server: &Arc<Mutex<Server>>) -> Vec<String> {
        server.lock().unwrap().mail.clone()
    }

    fn titled(app: &MailApp, title: &str) -> Vec<mailcraft_model::Event> {
        let mut events: Vec<_> = app.session.mailbox.events.values().filter(|e| e.title == title).map(|e| e.as_ref().clone()).collect();
        events.sort_by_key(|e| (e.start, e.id));
        events
    }

    #[test]
    fn nobody_is_written_to_until_the_owner_presses_send() {
        let (mut app, server, account, mut at) = calendar(Vec::new());
        let sync_meeting = json!({"title": "Sync", "start": "2026-10-15T10:00", "attendees": "bo@example.org; cy@example.org"});
        // A new meeting in the account's calendar: the invitations are asked about first, and
        // until the answer nothing is made and nothing is sent.
        assert_eq!(app.run("calendar.newMeeting", sync_meeting.clone()).unwrap()["pending"], "confirm");
        let question = asked(&app).unwrap();
        assert_eq!((question.0.as_str(), question.1.as_str()), ("Send This Invitation?", "Send"));
        assert_eq!(question.2, ["Don't Send, Keep on This Computer"]);
        sync(&mut app, &mut at);
        assert!(titled(&app, "Sync").is_empty() && mail(&server).is_empty());
        // Not sent: it is kept on this computer, and nobody hears of it.
        answer(&mut app, "Don't Send, Keep on This Computer", "");
        sync(&mut app, &mut at);
        let kept = titled(&app, "Sync");
        assert!(kept.len() == 1 && kept[0].origin.is_local() && mail(&server).is_empty());
        app.execute("calendar.delete", json!({"id": kept[0].id})).unwrap();
        // Cancel (the dialog is closed): as if it had never been asked for.
        app.run("calendar.newMeeting", sync_meeting.clone()).unwrap();
        app.dialog = None;
        sync(&mut app, &mut at);
        assert!(titled(&app, "Sync").is_empty() && mail(&server).is_empty());
        // Send: the meeting is made on the server, which invites the two.
        app.run("calendar.newMeeting", sync_meeting.clone()).unwrap();
        answer(&mut app, "Send", "");
        assert_eq!(asked(&app), None);
        sync(&mut app, &mut at);
        let sent = titled(&app, "Sync");
        assert_eq!((sent.len(), sent[0].origin.account, sent[0].origin.id.as_str()), (1, account, "made-1"));
        assert_eq!(mail(&server), ["invitation:Sync:2"]);
        let id = sent[0].id;

        // Moving it is an update in two people's mail: asked about, and left alone on Cancel.
        assert_eq!(app.run("calendar.move", json!({"id": id, "start": "2026-10-15T11:00"})).unwrap()["pending"], "confirm");
        assert_eq!(asked(&app).map(|q| (q.0, q.1)), Some(("Change This Meeting?".to_string(), "Send Update".to_string())));
        app.dialog = None;
        sync(&mut app, &mut at);
        assert_eq!((titled(&app, "Sync")[0].start.hour, mail(&server).len()), (10, 1));
        app.run("calendar.move", json!({"id": id, "start": "2026-10-15T11:00"})).unwrap();
        answer(&mut app, "Send Update", "");
        sync(&mut app, &mut at);
        assert_eq!((titled(&app, "Sync")[0].start.hour, mail(&server).last().cloned()), (11, Some("update:Sync:2".to_string())));
        // How it shows in the owner's own calendar is asked about by nobody, and told to nobody.
        assert!(app.run("calendar.showAs", json!({"id": id, "value": "tentative"})).unwrap().get("pending").is_none());
        assert!(app.run("calendar.categorize", json!({"id": id, "category": "Blue category"})).unwrap().get("pending").is_none());
        sync(&mut app, &mut at);
        assert_eq!(mail(&server).len(), 2);
        assert!(app.session.mailbox.event_changes.is_empty());
        // A script that doesn't say "send" changes nothing, and is told why.
        let why = app.execute("calendar.move", json!({"id": id, "start": "2026-10-15T12:00"})).unwrap_err();
        assert!(why.contains("Say that this is meant with \"send\": true."), "{why}");
        sync(&mut app, &mut at);
        assert_eq!((titled(&app, "Sync")[0].start.hour, mail(&server).len()), (11, 2));

        // Deleting it is a cancellation, with a note for the people invited.
        app.run("calendar.delete", json!({"id": id})).unwrap();
        let question = asked(&app).unwrap();
        assert_eq!((question.0.as_str(), question.1.as_str(), question.2.len()), ("Cancel This Meeting?", "Send Cancellation", 0));
        assert!(app.session.mailbox.event(id).is_some());
        answer(&mut app, "Send Cancellation", " Sorry, something came up. ");
        assert!(app.session.mailbox.event(id).is_none());
        sync(&mut app, &mut at);
        assert_eq!(mail(&server).last().map(String::as_str), Some("cancellation:Sync:2:Sorry, something came up."));
        assert!(server.lock().unwrap().events.iter().all(|e| e.title != "Sync") && !app.has_sync_problems());
    }

    #[test]
    fn a_series_is_asked_about_and_one_day_or_all_of_it_changes_on_the_server() {
        // Monday, Wednesday and Friday at one o'clock, from Monday the 5th.
        let rule = Recurrence {
            by_day: vec![mailcraft_time::Weekday::Mon, mailcraft_time::Weekday::Wed, mailcraft_time::Weekday::Fri],
            ..Recurrence::every(Freq::Weekly)
        };
        let series = RemoteEvent {
            kind: RemoteEventKind::SeriesMaster,
            recurrence: Some(rule),
            end: NOW - 9 * 86_400 - 3 * 3600 + 1800,
            ..event("s-1", "Stand-up", NOW - 9 * 86_400 - 3 * 3600)
        };
        let (mut app, server, _, mut at) = calendar(vec![series]);
        // One event here, which repeats; the week in view has it on its three days.
        let all = titled(&app, "Stand-up");
        assert_eq!((all.len(), all[0].recurrence.is_some(), all[0].exceptions.len()), (1, true, 0));
        let id = all[0].id;
        let day = |d: u8| mailcraft_time::Date::new(2026, 10, d).unwrap();
        let on: Vec<u8> = all[0].occurrences(day(11), day(17)).iter().map(|o| o.start.date.day).collect();
        assert_eq!(on, [12, 14, 16]);

        // Friday's is dragged an hour down: which is meant is asked first.
        app.execute("calendar.select", json!({"id": id, "day": "2026-10-16"})).unwrap();
        assert_eq!(app.run("calendar.move", json!({"id": id, "start": "2026-10-05T14:00"})).unwrap()["pending"], "confirm");
        let question = asked(&app).unwrap();
        assert_eq!((question.0.as_str(), question.1.as_str()), ("Move Recurring Item", "Just This One"));
        assert_eq!(question.2, ["The Entire Series"]);
        let Some(Dialog::Confirm { ask, .. }) = &app.dialog else { panic!() };
        assert_eq!(ask.text, "\"Stand-up\" is one of a series. Do you mean just the one on Friday, October 16, 2026, or the entire series?");
        // Just this one: Friday's is an event of its own at two, and the server has it so.
        answer(&mut app, "Just This One", "");
        let all = titled(&app, "Stand-up");
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].exceptions.as_slice(), all[1].start.date, all[1].start.hour), (&[day(16)][..], day(16), 14));
        sync(&mut app, &mut at);
        let all = titled(&app, "Stand-up");
        assert_eq!((all.len(), all[1].origin.id.as_str(), all[1].origin.series.as_str()), (2, "x-1", "s-1"));
        assert_eq!(all[0].exceptions, [day(16)]);
        assert!(app.session.mailbox.event_changes.is_empty() && !app.has_sync_problems());
        let moved = server.lock().unwrap().events.iter().find(|e| e.id == "x-1").map(|e| (e.start, e.original)).unwrap();
        assert_eq!(moved, (NOW + 2 * 86_400 - 2 * 3600, Some(NOW + 2 * 86_400 - 3 * 3600)));

        // Monday's is deleted: only that one, when that is the answer.
        app.execute("calendar.select", json!({"id": id, "day": "2026-10-12"})).unwrap();
        app.run("calendar.delete", json!({"id": id})).unwrap();
        assert_eq!(asked(&app).map(|q| q.0), Some("Delete Recurring Item".to_string()));
        answer(&mut app, "Just This One", "");
        sync(&mut app, &mut at);
        let all = titled(&app, "Stand-up");
        assert_eq!((all.len(), all[0].exceptions.as_slice()), (2, &[day(12), day(16)][..]));
        assert!(server.lock().unwrap().taken.contains(&("s-1".to_string(), (NOW - 2 * 86_400).div_euclid(86_400))));

        // Wednesday's is dragged half an hour down, and the entire series is meant: every day
        // of it is at half past one from now on, on the server too.
        app.execute("calendar.select", json!({"id": id, "day": "2026-10-14"})).unwrap();
        app.run("calendar.move", json!({"id": id, "start": "2026-10-05T13:30"})).unwrap();
        answer(&mut app, "The Entire Series", "");
        assert_eq!(asked(&app), None);
        sync(&mut app, &mut at);
        let all = titled(&app, "Stand-up");
        assert_eq!((all[0].start.hour, all[0].start.minute, all[0].recurrence.is_some()), (13, 30, true));
        assert_eq!(server.lock().unwrap().events.iter().find(|e| e.id == "s-1").map(|e| e.start), Some(NOW - 9 * 86_400 - 3 * 3600 + 1800));
        assert!(app.session.mailbox.event_changes.is_empty() && !app.has_sync_problems());

        // Opened: which is meant is asked before the form, and what the form saves is saved for
        // that, without asking again.
        app.execute("calendar.select", json!({"id": id, "day": "2026-10-21"})).unwrap();
        assert_eq!(app.run("calendar.edit", json!({})).unwrap()["pending"], "editEvent");
        assert_eq!(asked(&app).map(|q| (q.0, q.1)), Some(("Open Recurring Item".to_string(), "Just This One".to_string())));
        assert!(crate::dialogs::confirmation(&app, "ui.openWindow", &json!({"event": id})).is_some_and(|ask| ask.title == "Open Recurring Item"));
        answer(&mut app, "Just This One", "");
        assert!(matches!(app.dialog, Some(Dialog::Form(_))) || app.windows.of_event(id).is_some(), "the form opens once the question is answered");
        assert_eq!(app.session.driver.calendar.opened, Some((id, Some(day(21)))));
        app.dialog = None;
        // The form saves as forms do: asked first when there is a question, and here there is
        // always something to say (which of the series), whatever is selected by now.
        app.execute("calendar.select", json!({"id": null})).unwrap();
        let fields = json!({"id": id, "title": "Stand-up (long)", "start": "2026-10-05T13:30", "end": "2026-10-05T15:00"});
        assert!(crate::dialogs::confirmation(&app, "calendar.edit", &fields).is_some());
        let saved = app.run("calendar.edit", fields).unwrap();
        assert_eq!((saved["occurrence"].as_str(), asked(&app), app.session.driver.calendar.opened), (Some("2026-10-21"), None, None));
        let long = titled(&app, "Stand-up (long)");
        assert_eq!((long.len(), long[0].start.date, long[0].duration_minutes()), (1, day(21), 90));
        // A script that says which is meant is asked nothing.
        assert!(crate::dialogs::confirmation(&app, "calendar.delete", &json!({"id": id, "scope": "series"})).is_none());
        assert!(crate::dialogs::confirmation(&app, "calendar.delete", &json!({"id": id, "occurrence": "2026-10-23"})).is_none());

        // The series is deleted on the server: asked about by name, it is said to be gone, and
        // goes here with the days that were changed.
        server.lock().unwrap().events.retain(|e| e.id != "s-1" && e.series != "s-1");
        sync(&mut app, &mut at);
        assert!(server.lock().unwrap().jobs.last().unwrap().series.contains(&"s-1".to_string()));
        assert!(app.session.mailbox.events.values().all(|e| e.origin.id != "s-1" && e.origin.series != "s-1"));
    }

    #[test]
    fn an_invitation_is_answered_and_declined_from_the_calendar() {
        let invited = RemoteEvent {
            organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
            attendees: vec![Attendee { address: Address::new("Avery", "avery@mail.example"), ..Default::default() }],
            is_organizer: Some(false),
            ..event("g-1", "Board", NOW + 86_400)
        };
        let (mut app, server, _, mut at) = calendar(vec![invited]);
        let id = titled(&app, "Board")[0].id;
        // Accept: send the answer now (with a note), or keep it to the calendar.
        assert_eq!(app.run("calendar.respond", json!({"id": id, "response": "accept"})).unwrap()["pending"], "confirm");
        let question = asked(&app).unwrap();
        assert_eq!((question.0.as_str(), question.1.as_str()), ("Send a Response?", "Send the Response Now"));
        assert_eq!(question.2, ["Don't Send a Response"]);
        answer(&mut app, "Send the Response Now", "See you there");
        assert_eq!(titled(&app, "Board")[0].attendees[0].response, Response::Accepted);
        sync(&mut app, &mut at);
        assert_eq!(mail(&server), ["response:Board:Accepted:See you there"]);
        assert_eq!(server.lock().unwrap().events.iter().find(|e| e.id == "g-1").map(|e| e.response), Some(Response::Accepted));
        // Tentatively, and nobody told: the server has the answer, and wrote to nobody.
        app.run("calendar.respond", json!({"id": id, "response": "tentative"})).unwrap();
        answer(&mut app, "Don't Send a Response", "a note nobody reads");
        sync(&mut app, &mut at);
        assert_eq!(mail(&server).len(), 1);
        assert_eq!(server.lock().unwrap().events.iter().find(|e| e.id == "g-1").map(|e| e.response), Some(Response::Tentative));

        // Deleting the invitation: only the owner's copy, or declining it, told or not.
        app.run("calendar.delete", json!({"id": id})).unwrap();
        let question = asked(&app).unwrap();
        assert_eq!((question.0.as_str(), question.1.as_str()), ("Delete Your Copy of This Meeting?", "Delete My Copy"));
        assert_eq!(question.2, ["Decline and Tell the Organizer", "Decline without Telling"]);
        answer(&mut app, "Decline and Tell the Organizer", "Can't make it");
        assert!(app.session.mailbox.event(id).is_none());
        sync(&mut app, &mut at);
        assert_eq!(mail(&server).last().map(String::as_str), Some("response:Board:Declined:Can't make it"));
        assert!(server.lock().unwrap().events.iter().all(|e| e.id != "g-1"));
        assert!(app.session.mailbox.event_changes.is_empty() && !app.has_sync_problems());
    }

    #[test]
    fn the_owner_can_have_the_calendar_connection_checked() {
        use crate::{CalendarCheckRequest, CalendarCheckResult, CalendarCheckStep};
        let (mut app, server, _) = app();
        // A build that can't reach a server can't check one.
        assert_eq!(app.execute("ui.checkCalendar", json!({"confirmed": true})).unwrap_err(), "This build can't reach the account's calendar.");
        // A host that reports three steps, of which the reading didn't come back as it was sent.
        let calls: Arc<Mutex<Vec<(String, bool)>>> = Default::default();
        let seen = calls.clone();
        app.services.check_calendar = Some(Box::new(move |request: CalendarCheckRequest| {
            seen.lock().unwrap().push((request.account.email.clone(), !request.access.is_empty()));
            let step = |what: &str, ok: bool, status: u16, said: &str| CalendarCheckStep { what: what.into(), ok, status, said: said.into() };
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(CalendarCheckResult {
                account: request.account.id,
                steps: vec![
                    step("Make an appointment", true, 201, "sent: A to B"),
                    step("Read it back", false, 200, "sent: A to B; the server has: C to D (the start and the end differ)"),
                    step("Delete it", true, 204, "deleted"),
                ],
                ok: false,
                cleaned_up: true,
                ..Default::default()
            });
            rx
        }));
        // The owner is asked first, and told exactly what is about to happen on the real account.
        assert_eq!(app.run("ui.checkCalendar", json!({})).unwrap()["pending"], "confirm");
        let Some(Dialog::Confirm { ask, .. }) = &app.dialog else { panic!("no question is being asked") };
        assert_eq!((ask.title.as_str(), ask.action.as_str(), ask.more.len()), ("Check Calendar Connection", "Check Now", 0));
        for said in [
            "\"MailCraft connection check\"",
            "avery@mail.example",
            "a year from today",
            "deletes it again",
            "Nothing else in your calendar is read or changed",
        ] {
            assert!(ask.text.contains(said), "{said}: {}", ask.text);
        }
        assert!(calls.lock().unwrap().is_empty() && !app.checking_calendar());
        // A script has to say that it means it; closing the question does nothing at all.
        assert!(app.execute("ui.checkCalendar", json!({})).unwrap_err().contains("\"confirmed\": true"));
        app.dialog = None;
        assert!(calls.lock().unwrap().is_empty());
        // Yes: the host gets the job, with the token that is at hand, and one check at a time.
        app.run("ui.checkCalendar", json!({})).unwrap();
        answer(&mut app, "Check Now", "");
        assert_eq!(*calls.lock().unwrap(), [("avery@mail.example".to_string(), true)]);
        assert!(app.checking_calendar());
        assert_eq!(app.execute("ui.checkCalendar", json!({"confirmed": true})).unwrap_err(), "The calendar connection is being checked already.");
        // The report: every step with the server's status and what it said, to read and to copy.
        app.poll_calendar_check();
        assert!(!app.checking_calendar());
        let Some(Dialog::Report { name, title, lines }) = &app.dialog else { panic!("the report is shown") };
        assert_eq!((*name, title.as_str()), ("calendarCheck", "Calendar Connection Check"));
        assert_eq!(
            lines,
            &[
                "avery@mail.example: Something did not go through, or did not arrive as it was sent. The steps below say what.",
                "1. Make an appointment: OK (the server answered 201). sent: A to B",
                "2. Read it back: FAILED (the server answered 200). sent: A to B; the server has: C to D (the start and the end differ)",
                "3. Delete it: OK (the server answered 204). deleted",
                "The test appointment is gone again. Nothing else in the calendar was read or changed.",
            ]
        );
        assert_eq!(app.calendar_check_report(), lines.as_slice());
        // It asked the calendar's own stand-in for nothing: the check is the host's business.
        assert_eq!(jobs(&server), 0);
    }

    #[test]
    fn a_change_that_cannot_be_sent_is_given_up_out_loud() {
        let (mut app, server, _, mut at) = calendar(Vec::new());
        // A meeting the owner called, on the server.
        app.run("calendar.newMeeting", json!({"title": "Sync", "start": "2026-10-15T10:00", "attendees": "bo@example.org"})).unwrap();
        answer(&mut app, "Send", "");
        sync(&mut app, &mut at);
        let id = titled(&app, "Sync")[0].id;
        assert_eq!(mail(&server), ["invitation:Sync:1"]);
        // A change to it that waits without the owner's yes to the mail it makes (only a
        // mailbox file from elsewhere holds such a thing): it is not sent, and it doesn't just
        // vanish either. The event is as the server has it again, and the owner is told.
        app.execute("calendar.move", json!({"id": id, "start": "2026-10-15T11:00", "send": true})).unwrap();
        app.session.mailbox.event_changes[0].send = false;
        app.session.mailbox_changed();
        sync(&mut app, &mut at);
        assert_eq!((titled(&app, "Sync")[0].start.hour, mail(&server).len()), (10, 1));
        assert!(app.session.mailbox.event_changes.is_empty());
        let line = "The change to \"Sync\" was not sent: it would have made the server write to the people invited, and nobody said to send that. It was put back to what the server has.";
        assert_eq!(app.status_msg.as_ref().map(|(text, _)| text.as_str()), Some(line));
        assert!(app.has_sync_problems() && app.sync_problem_lines().iter().any(|said| said.trim() == line));
    }

    #[test]
    fn a_change_to_what_is_only_on_this_computer_says_that_it_stays_here() {
        let (mut app, _, _, _) = calendar(Vec::new());
        // The sample's own appointments are on this computer; the account's calendar is on its server.
        let lunch = app.session.mailbox.events.values().find(|e| e.origin.is_local() && e.attendees.is_empty()).map(|e| e.id).unwrap();
        app.status_msg = None;
        app.run("calendar.showAs", json!({"id": lunch, "value": "tentative"})).unwrap();
        let said = app.status_msg.as_ref().map(|(text, _)| text.clone());
        assert_eq!(said.as_deref(), Some("Saved on this computer only: this item is not in your Outlook calendar"));
        assert!(app.session.mailbox.event_changes.is_empty());
        // One of the server's says nothing of the kind: it goes there.
        let review = titled(&app, "Budget review")[0].id;
        app.status_msg = None;
        app.run("calendar.showAs", json!({"id": review, "value": "tentative"})).unwrap();
        assert!(app.status_msg.is_none() && app.session.mailbox.event_changes.len() == 1);
    }

    #[test]
    fn a_sign_in_without_the_newest_permission_is_no_disconnected_calendar() {
        let (mut app, server, account) = app();
        server.lock().unwrap().extras_missing = true;
        app.execute("view.calendar", json!({})).unwrap();
        let t0 = now_ms();
        assert!(app.calendar_tick_at(true, t0));
        settle(&mut app);
        // The calendar came, and keeps coming; what is missing is noted quietly.
        assert!(app.graph_extras_missing(account) && !app.graph_consent_needed(account) && !app.has_sync_problems());
        assert!(app.session.mailbox.events.values().any(|e| e.title == "Budget review"));
        assert!(app.calendar_tick_at(true, t0 + 31.0 * SECOND));
        settle(&mut app);
        assert_eq!(jobs(&server), 2);
        // Once the categories come (the owner agreed to it), nothing is missing any more.
        let categories = mailcraft_model::remote::CategoriesDownload {
            account,
            email: "avery@mail.example".into(),
            categories: vec![mailcraft_model::Category { name: "Project X".into(), color: mailcraft_model::Rgb(1, 2, 3) }],
        };
        let mut errors = Vec::new();
        app.take_in_calendar_list(account, None, Some(&categories), false, &mut errors);
        assert!(!app.graph_extras_missing(account) && errors.is_empty());
        assert!(app.session.mailbox.category("Project X").is_some());
    }
}
