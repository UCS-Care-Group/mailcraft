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

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use mailcraft_engine::Module;
use mailcraft_engine::cmd::remote;
use mailcraft_model::Id;
use mailcraft_model::remote::{EventsUpload, EventsUploaded, id_tag};
use mailcraft_model::sync::Tokens;
use mailcraft_time::{DateTime, Timestamp};
use serde_json::{Value, json};

use crate::{CalendarCheckRequest, CalendarCheckResult, GraphRequest, GraphResult, MailApp, now_ms};

/// How often the calendar is asked for while it is in front of the owner, and otherwise
/// (milliseconds).
const IN_FRONT_MS: f64 = 30_000.0;
const BEHIND_MS: f64 = 300_000.0;
/// How long a change made here waits before it goes: not at all. It is on the server at once,
/// as it is in Outlook. (A moment here would let a change be taken back with Undo before the
/// server hears of it, and let a second change to the same event go with the first.)
const QUIET_MS: f64 = 0.0;
/// How long the server is left alone after a job that came to nothing: this long after the
/// first, twice as long after each one more, and never longer than the second.
const FIRST_RETRY_MS: f64 = 30_000.0;
const LONGEST_RETRY_MS: f64 = 600_000.0;
/// How often the calendar is asked for while a change that was just sent is yet to be seen on
/// the server (milliseconds).
const RECHECK_MS: f64 = 10_000.0;
/// How many days either side of the days on screen are asked for.
const MARGIN_DAYS: i64 = 7;
/// The wide stretch a Send/Receive brings down, as the host has it: a month back, a year ahead
/// (seconds).
const WIDE_BEHIND: i64 = 31 * 86_400;
const WIDE_AHEAD: i64 = 366 * 86_400;
/// Most calendars of one account that a job asks for, and takes in.
const MAX_ASKED: usize = 20;
/// How often the working hours the mailbox has set are asked for (milliseconds): they change
/// when the owner changes them in the mailbox's settings, which is seldom.
const SETTINGS_MS: f64 = 3_600_000.0;

/// What the app remembers of keeping the calendar current.
#[derive(Default)]
pub(crate) struct CalendarSync {
    /// The job in flight: the account, whether it carried changes, when it started
    /// (milliseconds), and the answer to come.
    pub(crate) rx: Option<(Id, bool, f64, Receiver<GraphResult>)>,
    /// When each account's calendar was last asked for (milliseconds).
    pub(crate) checked: HashMap<Id, f64>,
    /// How many jobs in a row came to nothing for an account, and when the last one did.
    pub(crate) failures: HashMap<Id, (u32, f64)>,
    /// How many changes of an account were ready to go at the last look, and since when.
    waiting: HashMap<Id, (usize, f64)>,
    /// What an account's server was last told, and when: the very same again right away would
    /// mean that the telling doesn't settle anything, and is left for later.
    sent: HashMap<Id, (EventsUpload, f64)>,
    /// The accounts whose next job was asked for by the owner (a Send/Receive).
    pub(crate) by_hand: std::collections::HashSet<Id>,
    /// Whether the job in flight brings the wide stretch with the contacts.
    directory: bool,
    /// Ask at once: the window came back to the front, or the owner switched to the calendar.
    pub(crate) due: bool,
    /// Whether the window had the focus and the calendar was in front at the last look.
    focused: bool,
    in_front: bool,
    /// The stretch of each account's calendar that was last asked for.
    shown: HashMap<Id, (Timestamp, Timestamp)>,
    /// The accounts whose sign-in covers everything but an optional extra (the category list).
    pub(crate) extras_missing: std::collections::HashSet<Id>,
    /// A recurring item that was opened, and what the owner said was meant: one occurrence (its
    /// day) or the series. What is saved from its form is saved for that.
    pub(crate) opened: Option<(Id, Option<mailcraft_time::Date>)>,
    /// When each account's mailbox was last asked for its working hours (milliseconds).
    pub(crate) settings_asked: HashMap<Id, f64>,
    /// The state of the mailbox at which the waiting changes were last looked over for ones
    /// that can't be sent any more.
    settled_rev: Option<u64>,
    /// A check of an account's calendar connection that the host is making.
    check_rx: Option<(Id, Receiver<CalendarCheckResult>)>,
    /// What the last check of the calendar connection came to, as its report says it.
    pub(crate) last_check: Vec<String>,
}

impl MailApp {
    /// Whether a calendar job is in flight.
    pub fn calendar_syncing(&self) -> bool {
        self.calendar.rx.is_some()
    }

    /// Look whether the calendar is due, and start a job when it is. Called every frame.
    pub(crate) fn calendar_tick(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        self.calendar_tick_at(focused, now_ms());
        if self.services.graph.is_none() || !self.auto_sync {
            return;
        }
        if self.calendar.rx.is_some() {
            // An answer is on its way (the host wakes the window when it is there, too).
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if !self.microsoft_accounts().is_empty() {
            // Nothing is happening: look again in a while, sooner when a change waits to go.
            let waits = self.calendar.waiting.values().any(|(n, _)| *n > 0);
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
        self.calendar.checked.remove(&account);
        self.calendar.by_hand.insert(account);
    }

    /// The accounts whose calendar a Microsoft server keeps.
    fn microsoft_accounts(&self) -> Vec<Id> {
        let microsoft = |a: &&mailcraft_model::Account| a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft);
        self.session.mailbox.accounts.iter().filter(microsoft).map(|a| a.id).collect()
    }

    /// [`calendar_tick`](Self::calendar_tick) at `now` (milliseconds), with the window in front
    /// or not. Whether a job was started. (Public for a host's tests, which have no window.)
    pub fn calendar_tick_at(&mut self, focused: bool, now: f64) -> bool {
        let in_front = self.session.view.module == Module::Calendar && !self.ui.backstage;
        let came_back = (focused && !self.calendar.focused) || (in_front && !self.calendar.in_front);
        (self.calendar.focused, self.calendar.in_front) = (focused, in_front);
        if came_back && focused && in_front {
            self.calendar.due = true;
        }
        if !self.auto_sync || self.services.graph.is_none() {
            return false;
        }
        // One at a time; and a Send/Receive that is about to ask Graph, or is asking, goes first.
        if self.calendar.rx.is_some() || self.graph_rx.is_some() || self.sync_rx.is_some() || self.signing_in.is_some() || !self.sync_queue.is_empty()
        {
            return false;
        }
        // What waits and can't be sent any more is given up before anything is sent: with a
        // word to the owner, never silently. (Looked at once for each state of the mailbox.)
        if self.calendar.settled_rev != Some(self.session.rev()) {
            let dropped = remote::settle_changes(&mut self.session);
            self.calendar.settled_rev = Some(self.session.rev());
            self.tell_dropped(&dropped);
        }
        for account in self.microsoft_accounts() {
            // What is ready to go, and for how long it has been: a change gets a quiet moment.
            let ready = remote::events_waiting(&self.session, account).due;
            let since = match self.calendar.waiting.get(&account) {
                Some((seen, since)) if *seen == ready => *since,
                _ => {
                    self.calendar.waiting.insert(account, (ready, now));
                    now
                }
            };
            // (What exactly would go: nothing, when a change was changed back; and not the very
            // same as a moment ago, which would go round in circles.)
            let changes = if ready > 0 && now - since >= QUIET_MS { self.calendar_changes(account) } else { None };
            let again = |(sent, at): &(EventsUpload, f64)| changes.as_ref() == Some(sent) && now - at < FIRST_RETRY_MS;
            let push = changes.is_some() && !self.calendar.sent.get(&account).is_some_and(again);
            // A change that was just sent and isn't seen on the server yet: asked for again soon.
            let unseen = remote::recheck_due(&self.session, account);
            let every = match (unseen, in_front) {
                (true, _) => RECHECK_MS,
                (false, true) => IN_FRONT_MS,
                (false, false) => BEHIND_MS,
            };
            let asked = self.calendar.checked.get(&account).copied();
            // Other days came into view (a series shows there by its rule until the server has
            // said which of its days are really there): they are asked for without waiting.
            let elsewhere = in_front && self.calendar.shown.get(&account).is_some_and(|shown| *shown != self.calendar_window());
            // The wide stretch with the contacts, every quarter of an hour: after the mail.
            let wide = self.directory_due(account, now);
            let refresh = ((self.calendar.due || elsewhere) && in_front) || asked.is_none_or(|at| now - at >= every) || wide;
            if !(push || refresh) || self.graph_consent_needed.contains(&account) {
                continue;
            }
            // A server that didn't answer the last time is given longer each time.
            let (failures, at) = self.calendar.failures.get(&account).copied().unwrap_or((0, 0.0));
            let patience = (FIRST_RETRY_MS * f64::from(1u32 << failures.saturating_sub(1).min(8))).min(LONGEST_RETRY_MS);
            if failures > 0 && now - at < patience {
                continue;
            }
            if self.start_calendar_job(account, push, now) {
                return true;
            }
        }
        // Nobody to ask: the wish to ask at once doesn't outlive the look.
        self.calendar.due = false;
        false
    }

    /// Whether the wide stretch of `account`'s calendar and its contacts are due: every quarter
    /// of an hour, and at start-up.
    fn directory_due(&self, account: Id, now: f64) -> bool {
        self.graph_checked.get(&account).is_none_or(|at| now - at > 900_000.0)
    }

    /// The stretch of the calendar a refresh asks for: the days on screen and a week either
    /// side, as instants.
    pub(crate) fn calendar_window(&self) -> (Timestamp, Timestamp) {
        let (from, to) = mailcraft_engine::cmd::view::calendar_range(&self.session);
        let instant = |day: mailcraft_time::Date| {
            let midnight = DateTime::at_midnight(day);
            midnight.to_timestamp(self.session.offset_at(midnight.to_timestamp(0)))
        };
        let (from, to) = (instant(from.add_days(-MARGIN_DAYS)), instant(to.add_days(MARGIN_DAYS + 1)));
        (from, to.max(from.saturating_add(mailcraft_time::DAY)))
    }

    /// What of the calendar waits to go to `account`'s server now; `None` when nothing does.
    pub(crate) fn calendar_changes(&self, account: Id) -> Option<EventsUpload> {
        remote::event_upload(&self.session, account).ok().filter(|upload| !upload.ops.is_empty())
    }

    /// Hand the host a calendar job for `account`: its changes when `push`, then the days on
    /// screen. `false` when there is no sign-in to do it with (the owner isn't asked for one:
    /// Send/Receive does that).
    fn start_calendar_job(&mut self, account: Id, push: bool, now: f64) -> bool {
        self.calendar_job(account, push, now, None)
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
        let busy = self.calendar.rx.is_some() || self.graph_rx.is_some() || self.signing_in.is_some() || !self.sync_queue.is_empty();
        if busy || self.calendar_changes(account).is_some() {
            return Err(tl!("The calendar is being updated already.").to_string());
        }
        if self.graph_consent_needed.contains(&account) {
            return Err(tl!("Calendar and contacts need your permission: File › Info › Connect calendar and contacts").to_string());
        }
        let instant = |day: mailcraft_time::Date| {
            let midnight = DateTime::at_midnight(day);
            midnight.to_timestamp(self.session.offset_at(midnight.to_timestamp(0)))
        };
        let from = instant(day.add_days(-1));
        let round = (from, instant(day.add_days(2)).max(from.saturating_add(mailcraft_time::DAY)));
        if self.calendar_job(account, false, now_ms(), Some(round)) {
            Ok(())
        } else {
            Err(tl!("This account's calendar can't be asked yet: Send/Receive signs it in first.").to_string())
        }
    }

    /// [`start_calendar_job`](Self::start_calendar_job) for the days on screen, or for the
    /// stretch `round` instead: a look at other days, which says nothing of when the days on
    /// screen were last asked for (and brings no settings along).
    fn calendar_job(&mut self, account: Id, push: bool, now: f64, round: Option<(Timestamp, Timestamp)>) -> bool {
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        let Some(oauth) = owner.oauth.clone() else { return false };
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", &owner.email)).unwrap_or_default();
        if refresh.is_empty() && self.graph_access(account, false).is_empty() {
            return false;
        }
        // The Favorites as Outlook has them ride along when they are due ([`crate::favorites_sync`]).
        let by_hand = self.calendar.by_hand.remove(&account);
        let favorites = round.is_none() && self.favorites_ask(account, now, by_hand);
        // (A token that is known not to be good for them is not handed on when one that is can
        // be had; without a way of getting another, the one there is has to do.)
        let access = if refresh.is_empty() { self.graph_access(account, false) } else { self.graph_access_for(account, favorites) };
        let changes = if push { self.calendar_changes(account) } else { None };
        let carried = changes.is_some();
        if let Some(changes) = &changes {
            self.calendar.sent.insert(account, (changes.clone(), now));
        }
        // The working hours the mailbox has set come along once an hour (and after "Connect").
        let settings = round.is_none() && self.calendar.settings_asked.get(&account).is_none_or(|at| now - at >= SETTINGS_MS);
        let stretch = round.unwrap_or_else(|| self.calendar_window());
        // The wide stretch and the contacts, with a job for the days on screen when they are due.
        let directory = round.is_none() && self.directory_due(account, now);
        let ask = GraphRequest {
            account: owner,
            oauth: self.effective_oauth(oauth),
            refresh,
            access,
            outbox: Vec::new(),
            directory,
            calendar: Some(stretch),
            changes,
            asks: self.calendar_asks(account, directory, Some(stretch)),
            settings,
            favorites,
            favorites_scopes: favorites,
            mail: None,
            mail_pieces: None,
            cancel: Default::default(),
        };
        let Some(graph) = &self.services.graph else { return false };
        if settings {
            self.calendar.settings_asked.insert(account, now);
        }
        self.calendar.directory = directory;
        self.calendar.rx = Some((account, carried, now, graph(ask)));
        if round.is_none() {
            self.calendar.checked.insert(account, now);
            self.calendar.shown.insert(account, self.calendar_window());
            self.calendar.due = false;
        }
        true
    }

    /// The calendar job answered: keep its token, take in what the server said to the changes
    /// and what it has in the days that were asked for. (Public for a host's tests.)
    pub fn poll_calendar(&mut self) {
        let Some((account, carried, started, rx)) = &self.calendar.rx else { return };
        let (account, carried, started) = (*account, *carried, *started);
        let mut result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("calendar, account {account}: the job ended without an answer (its thread ended early)");
                GraphResult { account, ..Default::default() }
            }
        };
        self.calendar.rx = None;
        if result.consent_needed {
            log::info!("calendar, account {account}: waiting for the owner's permission (Connect calendar and contacts)");
        }
        if result.sign_in_refused {
            log::warn!("calendar, account {account}: the sign-in was refused; the next Send/Receive asks the owner");
        }
        let mut errors = std::mem::take(&mut result.errors);
        if result.token_refused {
            self.graph_tokens.remove(&account);
        }
        if let Some(tokens) = result.tokens.take() {
            self.keep_graph_tokens(account, tokens);
        }
        if result.consent_needed && self.graph_consent_needed.insert(account) {
            self.status(tl!("Calendar and contacts need your permission: File › Info › Connect calendar and contacts"));
        }
        // The Favorites as Outlook has them, when they came along ([`crate::favorites_sync`]).
        self.take_in_favorites(account, &mut result);
        self.take_in_calendar_list(account, result.calendars.as_ref(), result.categories.as_ref(), result.extras_missing, &mut errors);
        if let Some(hours) = result.mailbox_hours.take() {
            self.take_in_mailbox_hours(account, &hours);
        }
        let taken = self.take_in_calendar(account, result.uploaded.as_ref(), result.events.as_ref(), &mut errors);
        self.take_in_more_calendars(&result.more_events, &mut errors);
        // The contacts came with the wide stretch: both in, and it is a quarter of an hour
        // until the next.
        if let Some(contacts) = &result.contacts {
            match remote::apply_contacts(&mut self.session, contacts) {
                Ok(_) => {}
                Err(e) => errors.push(crate::i18n::fmt(tl!("The contacts couldn't be taken in: {error}"), &[("error", &e.to_string())])),
            }
        }
        // (A part that couldn't be had is among the errors: then the next job asks again.)
        if std::mem::take(&mut self.calendar.directory) && taken && errors.is_empty() {
            self.graph_checked.insert(account, now_ms());
        }
        // Something went up: what the server has now is asked for again soon, should this
        // answer have been cut short.
        let went = result.uploaded.as_ref().is_some_and(|uploaded| !uploaded.outcomes.is_empty());
        let nothing = !taken || (carried && !went);
        match (nothing, self.calendar.failures.get(&account).copied()) {
            (true, earlier) => {
                self.calendar.failures.insert(account, (earlier.map_or(0, |(n, _)| n).saturating_add(1), started));
            }
            (false, _) => {
                self.calendar.failures.remove(&account);
            }
        }
        self.note_sync_problems(account, &errors, false);
    }

    /// A Graph token that a job obtained: kept for the session, and its refresh token is the
    /// account's newest, for mail and Graph alike.
    pub(crate) fn keep_graph_tokens(&mut self, account: Id, tokens: Tokens) {
        // Somebody else's are neither used nor remembered (#103).
        if self.signed_in_as_other(account, &tokens) {
            return;
        }
        self.note_directory(account, &tokens.tenant);
        let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
        if !tokens.refresh.is_empty() {
            self.remember("refresh", &address, &tokens.refresh);
            if let Some(mail) = self.tokens.get_mut(&account) {
                mail.refresh = tokens.refresh.clone();
            }
        }
        let now = mailcraft_time::now();
        self.graph_tokens.insert(account, Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens });
    }

    /// Take in what the server said to the calendar changes that were sent, and then the events
    /// it has (in that order: the events were asked for after the changes went up). What went
    /// wrong is added to `errors`. Whether the events came and were taken in.
    pub(crate) fn take_in_calendar(
        &mut self,
        account: Id,
        uploaded: Option<&EventsUploaded>,
        events: Option<&mailcraft_model::remote::EventsDownload>,
        errors: &mut Vec<String>,
    ) -> bool {
        if let Some(uploaded) = uploaded {
            match remote::apply_event_outcomes(&mut self.session, uploaded) {
                Ok(applied) => {
                    if let Some(first) = applied.messages.first() {
                        self.status(first.clone());
                    }
                    log::info!(
                        "calendar: of {} changes sent, {} were taken, {} refused, {} are to be tried again, {} given up; {} still wait, {} are yet to be seen on the server",
                        uploaded.outcomes.len(),
                        applied.done,
                        applied.refused,
                        applied.failed,
                        applied.dropped.len(),
                        applied.waiting,
                        applied.unverified
                    );
                    // (They name the events: for the owner to read, not for the log.)
                    self.keep_sync_problems(account, &applied.messages);
                    // The server says the account may not write to its calendar: the owner
                    // agreed to less, once. Nothing more is asked until that is put right.
                    if applied.denied && self.graph_consent_needed.insert(account) {
                        self.status(tl!("Changing the calendar needs your permission: File › Info › Connect calendar and contacts"));
                    }
                }
                Err(e) => errors.push(crate::i18n::fmt(tl!("The calendar changes couldn't be taken in: {error}"), &[("error", &e.to_string())])),
            }
            // What couldn't be sent stays as it is here and is tried again: never without a word.
            let failed = |outcome: &&mailcraft_model::remote::EventOutcome| outcome.result == mailcraft_model::remote::EventResult::Failed;
            for (n, outcome) in uploaded.outcomes.iter().filter(failed).take(3).enumerate() {
                let why = if outcome.error.is_empty() { tl!("the server couldn't be reached") } else { outcome.error.as_str() };
                let line = crate::i18n::fmt(tl!("A calendar change couldn't be sent yet and will be tried again: {error}"), &[("error", why)]);
                if n == 0 {
                    self.status(line.clone());
                }
                errors.push(line);
            }
        }
        let Some(events) = events else { return false };
        self.take_in_events(account, events, errors)
    }

    /// Take in what the server has in a stretch of one calendar. What was sent and isn't there
    /// is put back by the merge: the owner is told which item, and the log how many (and of
    /// each calendar how much came down). Whether it was taken in.
    fn take_in_events(&mut self, account: Id, events: &mailcraft_model::remote::EventsDownload, errors: &mut Vec<String>) -> bool {
        match remote::take_in_events(&mut self.session, events) {
            Ok(merged) => {
                if let Some(first) = merged.messages.first() {
                    self.status(first.clone());
                }
                // (They name the events: for the owner to read, not for the log.)
                self.keep_sync_problems(account, &merged.messages);
                let which =
                    if events.calendar.is_empty() { "the main calendar".to_string() } else { format!("calendar #{}", id_tag(&events.calendar)) };
                log::info!(
                    "calendar: {which}: {} events came down ({} new, {} changed, {} gone here); of what was sent, {} are there as sent, {} were put back, {} are not seen yet; {} waiting changes were given up",
                    events.events.len(),
                    merged.added,
                    merged.updated,
                    merged.removed,
                    merged.verified,
                    merged.put_back,
                    merged.unverified,
                    merged.dropped.len()
                );
                true
            }
            Err(e) => {
                errors.push(crate::i18n::fmt(tl!("The calendar couldn't be taken in: {error}"), &[("error", &e.to_string())]));
                false
            }
        }
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
        if self.calendar.rx.is_some() || self.graph_rx.is_some() || self.signing_in.is_some() {
            return Err(tl!("The calendar is being updated: try again in a moment.").to_string());
        }
        let account = owner.id;
        let access = self.graph_tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone()).unwrap_or_default();
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", &owner.email)).unwrap_or_default();
        if access.is_empty() && refresh.is_empty() {
            return Err(tl!("This account's calendar can't be asked yet: Send/Receive signs it in first.").to_string());
        }
        let request = CalendarCheckRequest { account: owner, oauth: self.effective_oauth(oauth), refresh, access };
        let Some(check) = &self.services.check_calendar else { return Err(tl!("This build can't reach the account's calendar.").to_string()) };
        self.calendar.check_rx = Some((account, check(request)));
        self.status(tl!("Checking the calendar connection…"));
        Ok(json!({"pending": "calendarCheck", "account": account}))
    }

    /// Whether a check of the calendar connection is under way.
    pub fn checking_calendar(&self) -> bool {
        self.calendar.check_rx.is_some()
    }

    /// What the last check of the calendar connection came to, line by line.
    pub fn calendar_check_report(&self) -> &[String] {
        &self.calendar.last_check
    }

    /// The host is done checking the calendar connection: keep its token, show what the server
    /// answered at each step in a report, and put the same in the log. (Public for a host's tests.)
    pub fn poll_calendar_check(&mut self) {
        let Some((account, rx)) = &self.calendar.check_rx else { return };
        let account = *account;
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
        if result.consent_needed && self.graph_consent_needed.insert(account) {
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
        self.calendar.last_check = lines.clone();
        self.status(if result.ok {
            tl!("The calendar connection works")
        } else {
            tl!("The calendar connection check found a problem: see its report")
        });
        self.dialog = Some(crate::dialogs::Dialog::Report { name: "calendarCheck", title: tl!("Calendar Connection Check").to_string(), lines });
        // The calendar is looked at again at once, when it is in front.
        self.calendar.due = true;
    }

    /// [`take_in_calendar`](Self::take_in_calendar) for the Microsoft Graph part of a
    /// Send/Receive: a calendar that came with it needn't be asked for again right away.
    pub(crate) fn take_in_calendar_of_sync(
        &mut self,
        account: Id,
        uploaded: Option<&EventsUploaded>,
        events: Option<&mailcraft_model::remote::EventsDownload>,
        errors: &mut Vec<String>,
    ) -> bool {
        let taken = self.take_in_calendar(account, uploaded, events, errors);
        if taken {
            self.calendar.checked.insert(account, now_ms());
        }
        taken
    }

    /// The calendars of `account` that a job brings down, each with the series the mailbox
    /// expects in the stretch (for the host to ask about by name when the stretch brings nothing
    /// of them): the main one, the others that are shown, and any that holds something which was
    /// sent and is yet to be seen there. The stretch is the wide one of a
    /// Send/Receive (`directory`: a month back, a year ahead), or else `stretch`.
    pub(crate) fn calendar_asks(
        &self,
        account: Id,
        directory: bool,
        stretch: Option<(Timestamp, Timestamp)>,
    ) -> Vec<mailcraft_model::remote::CalendarAsk> {
        let now = self.session.now();
        let Some((from, to)) = (if directory { Some((now - WIDE_BEHIND, now + WIDE_AHEAD)) } else { stretch }) else { return Vec::new() };
        // (And one that holds something which was sent and is yet to be seen on the server,
        // shown or not: an appointment that was just put into a calendar that isn't ticked.)
        let unseen = remote::unverified_calendars(&self.session, account);
        let followed = |c: &&mailcraft_model::Calendar| {
            c.origin.account == account && (c.origin.id.is_empty() || c.visible || directory || unseen.contains(&c.id))
        };
        let mut asks: Vec<mailcraft_model::remote::CalendarAsk> = Vec::new();
        for calendar in self.session.mailbox.calendars.iter().filter(followed).take(MAX_ASKED) {
            if !asks.iter().any(|ask| ask.calendar == calendar.origin.id) {
                let series = remote::expected_series(&self.session, account, &calendar.origin.id, from, to);
                asks.push(mailcraft_model::remote::CalendarAsk { calendar: calendar.origin.id.clone(), series });
            }
        }
        asks
    }

    /// Take in the list of the account's calendars and the mailbox's categories that came with
    /// a job, and note whether the sign-in lacks the optional extra (the category list).
    pub(crate) fn take_in_calendar_list(
        &mut self,
        account: Id,
        calendars: Option<&mailcraft_model::remote::CalendarsDownload>,
        categories: Option<&mailcraft_model::remote::CategoriesDownload>,
        extras_missing: bool,
        errors: &mut Vec<String>,
    ) {
        if let Some(list) = calendars
            && let Err(e) = remote::apply_calendars(&mut self.session, list)
        {
            errors.push(crate::i18n::fmt(tl!("The list of calendars couldn't be taken in: {error}"), &[("error", &e.to_string())]));
        }
        if let Some(list) = categories {
            // They came: the sign-in covers them.
            self.calendar.extras_missing.remove(&account);
            if let Err(e) = remote::apply_categories(&mut self.session, list) {
                errors.push(crate::i18n::fmt(tl!("The categories couldn't be taken in: {error}"), &[("error", &e.to_string())]));
            }
        } else if extras_missing {
            self.calendar.extras_missing.insert(account);
        }
    }

    /// The working hours the mailbox has set came with a job: they are the ones in effect from
    /// now on, unless the owner would rather have the ones of the local options. That they
    /// could be read says that the sign-in covers the optional extras.
    pub(crate) fn take_in_mailbox_hours(&mut self, account: Id, hours: &mailcraft_engine::cmd::calendar_settings::MailboxHours) {
        self.calendar.extras_missing.remove(&account);
        if let Err(e) = mailcraft_engine::cmd::calendar_settings::apply_mailbox_hours(&mut self.session, account, Some(hours)) {
            // (Hours that aren't hours: the ones that were kept stay.)
            log::debug!("the mailbox's working hours were not taken in: {e}");
        }
    }

    /// Take in the same stretch of the account's other calendars.
    pub(crate) fn take_in_more_calendars(&mut self, more: &[mailcraft_model::remote::EventsDownload], errors: &mut Vec<String>) {
        for events in more.iter().take(MAX_ASKED) {
            self.take_in_events(events.account, events, errors);
        }
    }

    /// Whether the sign-in of `account` covers everything but an optional extra (reading the
    /// mailbox's own list of categories): the calendar works as it did, and "Connect calendar
    /// and contacts" would add it. Quieter than [`graph_consent_needed`](Self::graph_consent_needed):
    /// nothing is disconnected.
    pub fn graph_extras_missing(&self, account: Id) -> bool {
        self.calendar.extras_missing.contains(&account)
    }

    /// The owner gave Microsoft Graph its permissions for `account` ("Connect calendar and
    /// contacts"): what was held back for the lack of them goes at once.
    pub(crate) fn calendar_connected(&mut self, account: Id) {
        remote::retry_event_changes(&mut self.session, account);
        self.calendar.failures.remove(&account);
        self.calendar.checked.remove(&account);
        self.calendar.sent.remove(&account);
        // And what the new permissions may cover is asked for with the next look.
        self.calendar.settings_asked.remove(&account);
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
        app.graph_checked.insert(account, f64::MAX);
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
        app.sync_queue.push_back((account, true));
        assert!(!app.calendar_tick_at(true, t3));
        app.sync_queue.clear();
        app.auto_sync = false;
        assert!(!app.calendar_tick_at(true, t3));
        app.auto_sync = true;
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
        app.graph_consent_needed.remove(&account);
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
        assert_eq!(app.calendar.opened, Some((id, Some(day(21)))));
        app.dialog = None;
        // The form saves as forms do: asked first when there is a question, and here there is
        // always something to say (which of the series), whatever is selected by now.
        app.execute("calendar.select", json!({"id": null})).unwrap();
        let fields = json!({"id": id, "title": "Stand-up (long)", "start": "2026-10-05T13:30", "end": "2026-10-05T15:00"});
        assert!(crate::dialogs::confirmation(&app, "calendar.edit", &fields).is_some());
        let saved = app.run("calendar.edit", fields).unwrap();
        assert_eq!((saved["occurrence"].as_str(), asked(&app), app.calendar.opened), (Some("2026-10-21"), None, None));
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
