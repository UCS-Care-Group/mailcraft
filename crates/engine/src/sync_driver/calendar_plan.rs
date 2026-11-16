// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar's job, asked of the engine and answered to it, with no I/O: [`Session::calendar_due`]
//! says which accounts' calendars are to be asked for now and with what plan; the host starts a
//! job for the first it can ([`Session::calendar_started`]) and, when it answers, hands what came
//! to [`Session::calendar_answered`], which takes it into the mailbox and says what the owner is to
//! be told. Where the host has a window, the window words and shows it; a host without one has
//! the same decisions.

use mailcraft_i18n::{Arg, Text};
use mailcraft_model::remote::{
    CalendarsDownload, CategoriesDownload, ContactsDownload, EventResult, EventsDownload, EventsUpload, EventsUploaded, id_tag,
};
use mailcraft_model::{Account, Id};
use mailcraft_time::Timestamp;
use serde_json::Value;

use super::{Busy, MAX_ASKED, Pacing, Start, WIDE_AHEAD, WIDE_BEHIND};
use crate::Session;
use crate::cmd::calendar_settings::MailboxHours;
use crate::cmd::remote::{self, Dropped};

/// Where a shared mailbox's calendars stand with the owner's permission for reading them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharedPermission {
    /// A run read them with it.
    Given,
    /// The owner allowed it on the provider's page just now, and no run has read them with it yet:
    /// the next one does (it isn't held back), and says [`SharedPermission::Given`] or
    /// [`SharedPermission::Needed`].
    Allowed,
    /// A run found the owner's sign-in doesn't cover it: the owner is to be asked.
    Needed,
    /// The owner is being asked: the question, or the provider's page.
    Asking,
    /// Asked, and no: why, in a few words.
    Refused(String),
}

impl SharedPermission {
    /// Whether the shared mailbox's calendars wait for the owner's permission: not asked of the
    /// server until it is had (or asked for again).
    pub fn blocks(this: Option<&SharedPermission>) -> bool {
        matches!(this, Some(SharedPermission::Needed | SharedPermission::Asking | SharedPermission::Refused(_)))
    }
}

/// What the host says of itself when it asks whether the calendar is due.
#[derive(Clone, Copy, Debug)]
pub struct CalendarFacts {
    /// The window has the focus.
    pub focused: bool,
    /// File is open over the calendar.
    pub backstage: bool,
    /// The host has a way to reach the calendar's server.
    pub reachable: bool,
    /// What is at work now.
    pub busy: Busy,
}

/// What one calendar job is to do.
#[derive(Clone, Debug, PartialEq)]
pub struct CalendarPlan {
    pub account: Id,
    /// Whether it carries changes up (`changes`).
    pub push: bool,
    /// What waits to go to the server; `Some` only when `push`.
    pub changes: Option<EventsUpload>,
    /// The wide stretch and the contacts are brought down with it.
    pub directory: bool,
    /// The mailbox's working hours are asked for with it.
    pub settings: bool,
    /// The stretch of the calendar asked for (the days on screen and a week either side, or the
    /// days of a look at other days), as instants.
    pub window: (Timestamp, Timestamp),
    /// A look at other days (`window`): it says nothing of when the days on screen were asked for.
    pub around: bool,
}

impl CalendarPlan {
    /// The range of the calendar the job asks for: the wide stretch when it brings the directory.
    pub fn range(&self, now: Timestamp) -> (Timestamp, Timestamp) {
        if self.directory { (now.saturating_sub(WIDE_BEHIND), now.saturating_add(WIDE_AHEAD)) } else { self.window }
    }
}

/// What a look at the calendar came to.
#[derive(Debug, Default)]
pub struct CalendarLook {
    /// Waiting changes that couldn't be sent any more and were given up: the owner is told.
    pub dropped: Vec<Dropped>,
    /// Nothing else is at work: a calendar ticked in a shared mailbox that the owner's permission
    /// is missing for may be asked about now.
    pub may_ask: bool,
    /// The accounts whose job is due, in the order they are tried: the host starts the first it
    /// can ([`Session::calendar_started`]); when it can start none, it says so
    /// ([`Session::calendar_none_started`]).
    pub plans: Vec<CalendarPlan>,
}

/// Why a look at other days can't be asked for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AroundRefused {
    /// Another job is at work, or something waits to go up.
    Busy,
    /// The owner has yet to allow calendar and contacts.
    Consent,
}

impl Session {
    /// Look whether the calendar is due at `now` (milliseconds): which accounts' jobs are to start,
    /// in the order they are tried. Called at every look; the engine remembers what it needs
    /// ([`CalendarDriver`]).
    pub fn calendar_due(&mut self, facts: &CalendarFacts, now: f64) -> CalendarLook {
        let mut look = CalendarLook::default();
        // A shared mailbox has a calendar to tick from the moment it is there (#70).
        if self.driver.calendar.shared_pending(self.rev()) {
            remote::ensure_shared_calendars(self);
            self.driver.calendar.shared_done(self.rev());
        }
        let in_front = self.view.module == crate::Module::Calendar && !facts.backstage;
        self.driver.calendar.looked(facts.focused, in_front);
        if !facts.reachable || !self.driver.auto_sync() {
            return look;
        }
        // One at a time; and a Send/Receive that is about to ask Graph, or is asking, goes first.
        if facts.busy.blocks(Start::Calendar) {
            return look;
        }
        // What waits and can't be sent any more is given up before anything is sent: with a
        // word to the owner, never silently. (Looked at once for each state of the mailbox.)
        if self.driver.calendar.settle_pending(self.rev()) {
            look.dropped = remote::settle_changes(self);
            self.driver.calendar.settled(self.rev());
        }
        look.may_ask = true;
        let window = self.calendar_window();
        for account in self.calendar_accounts() {
            // What is ready to go, and for how long it has been: a change gets a quiet moment.
            let ready = remote::events_waiting(self, account).due;
            // (What exactly would go: nothing, when a change was changed back; and not the very
            // same as a moment ago, which would go round in circles.)
            let changes = if self.driver.calendar.quiet_over(account, ready, now) { self.calendar_changes(account) } else { None };
            let push = self.driver.calendar.push_wanted(account, changes.as_ref(), now);
            // A change that was just sent and isn't seen on the server yet: asked for again soon.
            let unseen = remote::recheck_due(self, account);
            // Other days came into view: they are asked for without waiting.
            let elsewhere = self.driver.calendar.elsewhere(account, in_front, window);
            // The wide stretch with the contacts, every quarter of an hour: after the mail.
            let wide = self.driver.calendar.directory_is_due(account, now);
            let refresh = self.driver.calendar.refresh_due(account, in_front, unseen, elsewhere, wide, now);
            if !(push || refresh) || self.driver.calendar.consent_needed.contains(&account) || self.shared_calendars_blocked(account) {
                continue;
            }
            // A server that didn't answer the last time is given longer each time.
            if !self.driver.calendar.book.patient(&Pacing::CALENDAR, account, now) {
                continue;
            }
            look.plans.push(CalendarPlan {
                account,
                push,
                changes: changes.filter(|_| push),
                directory: wide,
                settings: !self.is_shared_account(account) && self.driver.calendar.settings_due(account, now),
                window,
                around: false,
            });
        }
        look
    }

    /// No job could be started for what [`Session::calendar_due`] said (`look`): the wish to ask at
    /// once doesn't outlive a look that really looked. One that was held off (automatic checking
    /// is off, or another job is at work) leaves it as it was, for when it can be asked.
    pub fn calendar_none_started(&mut self, look: &CalendarLook) {
        if look.may_ask {
            self.driver.calendar.due = false;
        }
    }

    /// A plan for a look at the days round `day` for `account`, as a job of its own: for a meeting
    /// that should be on the calendar and hasn't come down. It only asks: nothing is sent up with
    /// it, and it never starts while another job is at work (`busy`) or while something waits to
    /// go up.
    pub fn calendar_around(&self, account: Id, day: mailcraft_time::Date, busy: bool) -> Result<CalendarPlan, AroundRefused> {
        if busy || self.calendar_changes(account).is_some() {
            return Err(AroundRefused::Busy);
        }
        if self.driver.calendar.consent_needed.contains(&account) {
            return Err(AroundRefused::Consent);
        }
        Ok(CalendarPlan {
            account,
            push: false,
            changes: None,
            directory: false,
            settings: false,
            window: self.days_as_instants(day.add_days(-1), day.add_days(2)),
            around: true,
        })
    }

    /// The host handed a job for `plan` to its server at `now`.
    pub fn calendar_started(&mut self, plan: &CalendarPlan, now: f64) {
        if let Some(changes) = &plan.changes {
            self.driver.calendar.sent_changes(plan.account, changes, now);
        }
        let own_days = (!plan.around).then_some(plan.window);
        self.driver.calendar.started(plan.account, now, own_days, plan.directory, plan.settings);
    }

    /// What the job asks of the server for `plan`: the calendars, each with the series the mailbox
    /// expects in the stretch.
    pub fn calendar_asks_for(&self, plan: &CalendarPlan) -> Vec<mailcraft_model::remote::CalendarAsk> {
        self.calendar_asks(plan.account, plan.directory, Some(plan.window))
    }

    /// The accounts whose calendar is kept in step now: those whose calendar a Microsoft server
    /// keeps, and the shared mailboxes with a calendar ticked (#70).
    pub fn calendar_accounts(&self) -> Vec<Id> {
        let mut accounts = self.microsoft_calendar_accounts();
        accounts.extend(self.shared_calendar_accounts());
        accounts
    }

    /// Whether any calendar of the shared mailbox `shared` is ticked: the owner wants it read.
    pub fn shared_calendar_wanted(&self, shared: Id) -> bool {
        self.mailbox.calendars.iter().any(|c| c.origin.account == shared && c.visible)
    }

    /// The shared mailboxes whose calendars are kept in step now: one of them is ticked, and the
    /// account it is opened through signs in with Microsoft.
    pub fn shared_calendar_accounts(&self) -> Vec<Id> {
        let mailbox = &self.mailbox;
        let signs_in = |shared: &&Account| mailbox.owner_of(shared.id).is_some_and(|owner| owner.oauth.is_some());
        mailbox.accounts.iter().filter(|a| a.is_shared()).filter(signs_in).filter(|a| self.shared_calendar_wanted(a.id)).map(|a| a.id).collect()
    }

    /// Whether `shared`'s calendars wait for the owner's permission.
    pub fn shared_calendars_blocked(&self, shared: Id) -> bool {
        SharedPermission::blocks(self.driver.calendar.shared_perm.get(&shared))
    }

    /// A shared mailbox whose calendar was ticked and the owner's permission is missing for: the
    /// owner is to be asked about it (a question first, then the provider's page).
    pub fn shared_calendar_to_ask(&self) -> Option<Id> {
        self.driver
            .calendar
            .shared_perm
            .iter()
            .filter(|(_, perm)| **perm == SharedPermission::Needed)
            .map(|(shared, _)| *shared)
            .filter(|shared| self.mailbox.account(*shared).is_some_and(Account::is_shared))
            .min()
    }

    fn is_shared_account(&self, account: Id) -> bool {
        self.mailbox.account(account).is_some_and(Account::is_shared)
    }
}

/// What a calendar job brought, as the engine takes it in (borrowed from the host's answer).
#[derive(Clone, Copy, Debug, Default)]
pub struct CalendarCame<'a> {
    /// What the server said to the changes that went up.
    pub uploaded: Option<&'a EventsUploaded>,
    /// The days asked for, of the account's main calendar.
    pub events: Option<&'a EventsDownload>,
    /// The same stretch of its other calendars.
    pub more_events: &'a [EventsDownload],
    pub calendars: Option<&'a CalendarsDownload>,
    pub categories: Option<&'a CategoriesDownload>,
    /// The sign-in covers everything but the category list.
    pub extras_missing: bool,
    pub hours: Option<&'a MailboxHours>,
    /// The contacts, with the wide stretch.
    pub contacts: Option<&'a ContactsDownload>,
    /// The host's own job put something among its errors (a part it couldn't bring down, say):
    /// the wide stretch and the contacts are asked for again at the next job.
    pub job_errors: bool,
    /// The sign-in doesn't cover what was asked: the owner is to be asked.
    pub consent_needed: bool,
    /// The access token was refused.
    pub token_refused: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
}

/// What the owner is to be told of a calendar job's answer, in the order it came to be.
#[derive(Debug, Default)]
pub struct CalendarTaken {
    /// For the status bar, in order: the last is what stays.
    pub status: Vec<Text>,
    /// For the Send/Receive problems report of the account, as lines the engine worded.
    pub problems: Vec<Text>,
    /// What went wrong in taking in, in order: the job's problems.
    pub errors: Vec<Text>,
    /// What came of the lists of calendars taken in, for the calendars opened by address (the
    /// window's own account of them).
    pub opened: Vec<Value>,
    /// The calendar and contacts of this (not shared) account need the owner's permission, as
    /// of this answer: the owner is to be asked (File › Info), once.
    pub consent_asked: bool,
    /// The events of the main calendar came and were taken in.
    pub taken: bool,
}

impl Session {
    /// The calendar job for `account` answered with `came`: take in what the server said to the
    /// changes and what it has, and say what the owner is to be told. `carried`: the job carried
    /// changes; `started`: when it did (milliseconds); `now`: the time (milliseconds). Where the
    /// owner's permission stands (`came.consent_needed`, `token_refused`, `sign_in_refused`: the
    /// calendar's, or for a shared mailbox its calendars') is kept here too. The window takes care
    /// of tokens, dialogs and the words; the mailbox and the driver's bookkeeping are done here.
    pub fn calendar_answered(&mut self, account: Id, carried: bool, started: f64, now: f64, came: &CalendarCame<'_>) -> CalendarTaken {
        let mut out = CalendarTaken::default();
        let shared = self.is_shared_account(account);
        if shared {
            self.shared_permission_in(account, came, &mut out);
        } else if came.consent_needed && self.driver.calendar.consent_needed.insert(account) {
            out.consent_asked = true;
        }
        self.calendar_list_in(account, came.calendars, came.categories, came.extras_missing, &mut out);
        if let Some(hours) = came.hours {
            self.mailbox_hours_in(account, hours);
        }
        let mut taken = self.take_in_calendar(account, came.uploaded, came.events, &mut out);
        self.more_calendars_in(came.more_events, &mut out);
        for (name, n) in self.driver.calendar.tell_held(remote::held_for_rights(&self.mailbox, account)) {
            let said = said!(
                "{n} changes to the calendar \"{name}\" are held here, not sent: this account may no longer change it. They are sent if it may again.",
                n,
                name = mailcraft_names::shown(&name, 60)
            );
            out.status.push(said.clone());
            out.problems.push(said);
        }
        // (A shared mailbox's job may bring only calendars other than its main one.)
        taken |= shared && !came.more_events.is_empty();
        // The contacts came with the wide stretch: both in, and it is a quarter of an hour
        // until the next.
        if let Some(contacts) = came.contacts
            && let Err(e) = remote::apply_contacts(self, contacts)
        {
            out.errors.push(said!("The contacts couldn't be taken in: {error}", error = e.to_string()));
        }
        // (A part that couldn't be had is among the errors, the host's own included: then the next
        // job asks again.)
        let wide = std::mem::take(&mut self.driver.calendar.directory);
        let partial = wide && taken && (!out.errors.is_empty() || came.job_errors);
        if wide && taken && !partial {
            self.driver.calendar.checked.insert(account, now);
        }
        // Something went up: what the server has now is asked for again soon, should this
        // answer have been cut short.
        let went = came.uploaded.is_some_and(|uploaded| !uploaded.outcomes.is_empty());
        // A wide job that brought events but not all it was asked for counts as a failure too (the
        // back-off keeps the next one from starting at every look); one that carried changes is
        // left as it was, so that its quick retry isn't held up any longer.
        let nothing = !taken || (carried && !went) || (partial && !carried);
        self.driver.calendar.answered(account, nothing, started);
        out.taken = taken;
        out
    }

    /// What a job of the shared mailbox `shared` said of the owner's permission: it covers shared
    /// calendars (the job got a token for them), or it doesn't and the owner is to be asked, or the
    /// owner's own sign-in is no longer taken.
    fn shared_permission_in(&mut self, shared: Id, came: &CalendarCame<'_>, out: &mut CalendarTaken) {
        let perm = &mut self.driver.calendar.shared_perm;
        if came.consent_needed {
            // (Asked about when the calendar is in front, by the host.)
            if !matches!(perm.get(&shared), Some(SharedPermission::Asking | SharedPermission::Refused(_))) {
                perm.insert(shared, SharedPermission::Needed);
            }
        } else if came.sign_in_refused {
            out.errors.push(said!("The sign-in of the account this shared mailbox is opened through was refused: sign in again with Send/Receive"));
        } else if !came.token_refused {
            perm.insert(shared, SharedPermission::Given);
        }
    }

    /// Take in the list of the account's calendars and the mailbox's categories that came with a
    /// job, and note whether the sign-in lacks the optional extra (the category list).
    pub fn calendar_list_in(
        &mut self,
        account: Id,
        calendars: Option<&CalendarsDownload>,
        categories: Option<&CategoriesDownload>,
        extras_missing: bool,
        out: &mut CalendarTaken,
    ) {
        if let Some(list) = calendars {
            match remote::apply_calendars(self, list) {
                Ok(applied) => out.opened.push(applied),
                Err(e) => out.errors.push(said!("The list of calendars couldn't be taken in: {error}", error = e.to_string())),
            }
        }
        if let Some(list) = categories {
            // They came: the sign-in covers them.
            self.driver.calendar.extras_missing.remove(&account);
            if let Err(e) = remote::apply_categories(self, list) {
                out.errors.push(said!("The categories couldn't be taken in: {error}", error = e.to_string()));
            }
        } else if extras_missing {
            self.driver.calendar.extras_missing.insert(account);
        }
    }

    /// The working hours the mailbox has set came with a job: they are the ones in effect from
    /// now on, unless the owner would rather have the ones of the local options. That they could
    /// be read says that the sign-in covers the optional extras.
    pub fn mailbox_hours_in(&mut self, account: Id, hours: &MailboxHours) {
        self.driver.calendar.extras_missing.remove(&account);
        if let Err(e) = crate::cmd::calendar_settings::apply_mailbox_hours(self, account, Some(hours)) {
            // (Hours that aren't hours: the ones that were kept stay.)
            log::debug!("the mailbox's working hours were not taken in: {e}");
        }
    }

    /// Take in the same stretch of the account's other calendars.
    pub fn more_calendars_in(&mut self, more: &[EventsDownload], out: &mut CalendarTaken) {
        for events in more.iter().take(MAX_ASKED) {
            self.take_in_events(events, out);
        }
    }

    /// Take in what the server said to the calendar changes that were sent, and then the events
    /// it has (in that order: the events were asked for after the changes went up). Whether the
    /// events came and were taken in.
    pub fn take_in_calendar(
        &mut self,
        account: Id,
        uploaded: Option<&EventsUploaded>,
        events: Option<&EventsDownload>,
        out: &mut CalendarTaken,
    ) -> bool {
        if let Some(uploaded) = uploaded {
            match remote::apply_event_outcomes(self, uploaded) {
                Ok(applied) => {
                    if let Some(first) = applied.messages.first() {
                        out.status.push(Text::verbatim(first.clone()));
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
                    out.problems.extend(applied.messages.iter().cloned().map(Text::verbatim));
                    // The server says the account may not write to its calendar: the owner
                    // agreed to less, once. Nothing more is asked until that is put right.
                    // (Not for a shared mailbox's calendar, #70: there it is the rights the owner
                    // has on that calendar, which its owner sets and which are read again at every
                    // listing, and not a permission to ask for; everything else of the mailbox goes
                    // on, and the change is tried once more.)
                    if applied.denied && self.is_shared_account(account) {
                        out.status.push(said!(
                            "A change to a shared mailbox's calendar was refused: this account may not edit it. It is tried once more, and then put back."
                        ));
                    } else if applied.denied && self.driver.calendar.consent_needed.insert(account) {
                        out.status.push(said!("Changing the calendar needs your permission: File › Info › Connect calendar and contacts"));
                    }
                }
                Err(e) => out.errors.push(said!("The calendar changes couldn't be taken in: {error}", error = e.to_string())),
            }
            // What couldn't be sent stays as it is here and is tried again: never without a word.
            let failed = |outcome: &&mailcraft_model::remote::EventOutcome| outcome.result == EventResult::Failed;
            for (n, outcome) in uploaded.outcomes.iter().filter(failed).take(3).enumerate() {
                let line = if outcome.error.is_empty() {
                    said!(
                        "A calendar change couldn't be sent yet and will be tried again: {error}",
                        error = Arg::words("the server couldn't be reached")
                    )
                } else {
                    said!("A calendar change couldn't be sent yet and will be tried again: {error}", error = outcome.error.clone())
                };
                if n == 0 {
                    out.status.push(line.clone());
                }
                out.errors.push(line);
            }
        }
        let Some(events) = events else { return false };
        self.take_in_events(events, out)
    }

    /// Take in what the server has in a stretch of one calendar. What was sent and isn't there
    /// is put back by the merge: the owner is told which item, and the log how many (and of
    /// each calendar how much came down). Whether it was taken in.
    pub fn take_in_events(&mut self, events: &EventsDownload, out: &mut CalendarTaken) -> bool {
        match remote::take_in_events(self, events) {
            Ok(merged) => {
                if let Some(first) = merged.messages.first() {
                    out.status.push(Text::verbatim(first.clone()));
                }
                // (They name the events: for the owner to read, not for the log.)
                out.problems.extend(merged.messages.iter().cloned().map(Text::verbatim));
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
                out.errors.push(said!("The calendar couldn't be taken in: {error}", error = e.to_string()));
                false
            }
        }
    }
}
