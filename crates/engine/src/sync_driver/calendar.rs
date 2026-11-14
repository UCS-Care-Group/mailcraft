// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keeping the calendar current with an account's server, the decisions and the state, with no
//! I/O: when an account's calendar is due, what waits to go up, which stretch is asked for, how
//! a server that doesn't answer is left alone, and where each account stands with its optional
//! extras and its rights. A front end looks every moment ([`CalendarDriver::looked`] and the
//! questions below), starts a job on its host when one is due, and feeds the answer back
//! ([`CalendarDriver::answered`]); nothing here reads a clock, opens a socket or starts a thread.
//!
//! What is made, moved, changed or deleted in the calendar is noted by the engine
//! (`Mailbox::event_changes`) and sent at once (the calendar's [`Pacing`] has no quiet time: it
//! is on the server as soon as it is in Outlook); what changes on the server is asked for every
//! half minute while the calendar is in front, and every few minutes otherwise. The wide stretch
//! (a month back, a year ahead) and the contacts come every quarter of an hour, after the
//! Send/Receive round, never in it (#276).

use std::collections::{HashMap, HashSet};

use mailcraft_model::remote::{CalendarAsk, EventsUpload, OPENED_PREFIX};
use mailcraft_model::{Account, Calendar, Id};
use mailcraft_time::{Date, DateTime, Timestamp};

use super::{CALENDAR_DIRECTORY_MS, CALENDAR_SETTINGS_MS, JobBook, Pacing, calendar_every_ms};
use crate::Session;
use crate::cmd::remote;

/// How many days either side of the days on screen are asked for.
pub const MARGIN_DAYS: i64 = 7;
/// The wide stretch a Send/Receive brings down: a month back (seconds).
pub const WIDE_BEHIND: i64 = 31 * 86_400;
/// The wide stretch a Send/Receive brings down: a year ahead (seconds).
pub const WIDE_AHEAD: i64 = 366 * 86_400;
/// Most calendars of one account that a job asks for, and takes in.
pub const MAX_ASKED: usize = 20;

/// What the log says a calendar job waits for when the sign-in lacks a permission: a shared
/// mailbox's calendars wait for the one to read shared mailboxes' calendars (asked for by itself,
/// #70), an account's own for Connect calendar and contacts. (No address: the account's id is
/// said beside it.)
pub fn consent_wait(shared: bool) -> &'static str {
    if shared {
        "waiting for the owner's permission to read shared mailboxes' calendars"
    } else {
        "waiting for the owner's permission (Connect calendar and contacts)"
    }
}

/// What the driver remembers of keeping the calendar current.
#[derive(Debug, Default)]
pub struct CalendarDriver {
    /// When each account's calendar was last asked for, the failures since the last answer and
    /// the changes waiting.
    pub book: JobBook,
    /// What an account's server was last told, and when: the very same again right away would
    /// mean that the telling doesn't settle anything, and is left for later.
    sent: HashMap<Id, (EventsUpload, f64)>,
    /// The accounts whose next job was asked for by the owner (a Send/Receive).
    pub by_hand: HashSet<Id>,
    /// Whether the job in flight brings the wide stretch with the contacts.
    pub directory: bool,
    /// Ask at once: the window came back to the front, or the owner switched to the calendar.
    pub due: bool,
    /// Whether the window had the focus and the calendar was in front at the last look.
    focused: bool,
    in_front: bool,
    /// The stretch of each account's calendar that was last asked for.
    shown: HashMap<Id, (Timestamp, Timestamp)>,
    /// The accounts whose sign-in covers everything but an optional extra (the category list).
    pub extras_missing: HashSet<Id>,
    /// A recurring item that was opened, and what the owner said was meant: one occurrence (its
    /// day) or the series. What is saved from its form is saved for that.
    pub opened: Option<(Id, Option<Date>)>,
    /// When each account's mailbox was last asked for its working hours (milliseconds).
    pub settings_asked: HashMap<Id, f64>,
    /// The state of the mailbox at which the waiting changes were last looked over for ones
    /// that can't be sent any more.
    settled_rev: Option<u64>,
    /// The state of the mailbox at which every shared mailbox was last given a calendar to tick.
    shared_rev: Option<u64>,
    /// The shared mailboxes whose calendar changes that make the server send mail wait for the
    /// owner's permission to send from them (#70): until it is given, or the owner asks again
    /// with a Send/Receive, those changes are not handed to the host.
    pub send_waits: HashSet<Id>,
    /// The calendars that changes are held back for, for want of rights, and how many were
    /// told to the owner (#70): said again only when that changes.
    held_told: HashMap<Id, usize>,
    /// What the last check of the calendar connection came to, as its report says it.
    pub last_check: Vec<String>,
    /// The accounts whose sign-in the owner is still to allow sending, calendar and contacts
    /// through Microsoft for ("Connect calendar and contacts").
    pub consent_needed: HashSet<Id>,
    /// When each account's Graph run last had the wide stretch and the contacts (milliseconds).
    pub checked: HashMap<Id, f64>,
    /// Where each shared mailbox's calendars stand with the owner's permission to read them.
    pub shared_perm: HashMap<Id, super::SharedPermission>,
}

impl CalendarDriver {
    /// Account removal: nothing of these accounts' calendar jobs or permission is kept.
    pub fn forget_accounts(&mut self, gone: &HashSet<Id>) {
        self.book.forget_accounts(gone);
        self.sent.retain(|account, _| !gone.contains(account));
        self.by_hand.retain(|account| !gone.contains(account));
        self.shown.retain(|account, _| !gone.contains(account));
        self.extras_missing.retain(|account| !gone.contains(account));
        self.settings_asked.retain(|account, _| !gone.contains(account));
        self.send_waits.retain(|account| !gone.contains(account));
        self.held_told.retain(|account, _| !gone.contains(account));
        self.consent_needed.retain(|account| !gone.contains(account));
        self.checked.retain(|account, _| !gone.contains(account));
        self.shared_perm.retain(|account, _| !gone.contains(account));
    }

    /// Another mailbox took the place of the open one: what a job in flight was for is let go of
    /// (AU3-V8).
    pub fn forget_jobs(&mut self) {
        self.directory = false;
        self.by_hand.clear();
    }

    /// The window is looked at: whether it has the focus and whether the calendar is in front.
    /// Coming back to either asks for a look at once (once both hold).
    pub fn looked(&mut self, focused: bool, in_front: bool) {
        let came_back = (focused && !self.focused) || (in_front && !self.in_front);
        (self.focused, self.in_front) = (focused, in_front);
        if came_back && focused && in_front {
            self.due = true;
        }
    }

    /// Whether the mailbox at `rev` has not had its waiting changes looked over yet.
    pub fn settle_pending(&self, rev: u64) -> bool {
        self.settled_rev != Some(rev)
    }

    /// The waiting changes were looked over, the mailbox now at `rev`.
    pub fn settled(&mut self, rev: u64) {
        self.settled_rev = Some(rev);
    }

    /// Whether the mailbox at `rev` has not had its shared mailboxes given calendars yet.
    pub fn shared_pending(&self, rev: u64) -> bool {
        self.shared_rev != Some(rev)
    }

    /// The shared mailboxes were given calendars, the mailbox now at `rev`.
    pub fn shared_done(&mut self, rev: u64) {
        self.shared_rev = Some(rev);
    }

    /// A Send/Receive the owner asked for was started for `account`: its calendar and contacts
    /// are asked for in the job that follows the round, the days on screen at least.
    pub fn after_round(&mut self, account: Id) {
        self.book.forget_checked(account);
        self.by_hand.insert(account);
        // (What waited for the permission to send from a shared mailbox is tried again.)
        self.send_waits.remove(&account);
    }

    /// Note that `account` has `ready` changes at `now` and say whether they have waited out the
    /// quiet time: only then is what would go worked out.
    pub fn quiet_over(&mut self, account: Id, ready: usize, now: f64) -> bool {
        self.book.quiet_over(&Pacing::CALENDAR, account, ready, now)
    }

    /// Whether `changes` (what would go now, if anything) are to be sent: not the very same as a
    /// moment ago, which would go round in circles.
    pub fn push_wanted(&self, account: Id, changes: Option<&EventsUpload>, now: f64) -> bool {
        let again = |(sent, at): &(EventsUpload, f64)| changes == Some(sent) && now - at < Pacing::CALENDAR.first_retry_ms;
        changes.is_some() && !self.sent.get(&account).is_some_and(again)
    }

    /// `changes` were handed to the host for `account` at `now`.
    pub fn sent_changes(&mut self, account: Id, changes: &EventsUpload, now: f64) {
        self.sent.insert(account, (changes.clone(), now));
    }

    /// Whether the days of `account` that are on screen differ from those last asked for: other
    /// days came into view (a series shows there by its rule until the server has said which of
    /// its days are really there), and are asked for without waiting.
    pub fn elsewhere(&self, account: Id, in_front: bool, window: (Timestamp, Timestamp)) -> bool {
        in_front && self.shown.get(&account).is_some_and(|shown| *shown != window)
    }

    /// Whether the wide stretch of `account`'s calendar and its contacts are due at `now`.
    pub fn directory_is_due(&self, account: Id, now: f64) -> bool {
        Self::directory_due(self.checked.get(&account).copied(), now)
    }

    /// Whether the wide stretch of an account's calendar and its contacts are due (`asked`: when
    /// the account's Graph run last had them): every quarter of an hour, and at start-up.
    pub fn directory_due(asked: Option<f64>, now: f64) -> bool {
        asked.is_none_or(|at| now - at > CALENDAR_DIRECTORY_MS)
    }

    /// Whether `account`'s days on screen are to be asked for at `now`: the owner or the window
    /// asked (`elsewhere` counts) while the calendar is in front, or it is due by the clock (soon
    /// after an unseen change, more often in front), or the wide stretch is (`wide`).
    pub fn refresh_due(&self, account: Id, in_front: bool, unseen: bool, elsewhere: bool, wide: bool, now: f64) -> bool {
        let every = calendar_every_ms(unseen, in_front);
        ((self.due || elsewhere) && in_front) || self.book.checked(account).is_none_or(|at| now - at >= every) || wide
    }

    /// Whether the working hours are to come along with a job for `account` at `now`: once an
    /// hour, and after "Connect".
    pub fn settings_due(&self, account: Id, now: f64) -> bool {
        self.settings_asked.get(&account).is_none_or(|at| now - at >= CALENDAR_SETTINGS_MS)
    }

    /// A job was started for `account` at `now`: for the days on screen (`window`) when
    /// `own_days`, else a look at other days that says nothing of when those were asked for.
    pub fn started(&mut self, account: Id, now: f64, own_days: Option<(Timestamp, Timestamp)>, directory: bool, settings: bool) {
        if settings {
            self.settings_asked.insert(account, now);
        }
        self.directory = directory;
        if let Some(window) = own_days {
            self.book.started(account, now);
            self.shown.insert(account, window);
            self.due = false;
        }
    }

    /// The job answered: it brought nothing (`nothing`, which the failures count), or it worked.
    pub fn answered(&mut self, account: Id, nothing: bool, started: f64) {
        if nothing {
            self.book.failed(account, started);
        } else {
            self.book.worked(account);
        }
    }

    /// The owner gave Microsoft Graph its permissions for `account` ("Connect calendar and
    /// contacts"): the failures, the last look and what was last sent are forgotten, and what the
    /// new permissions may cover is asked for with the next look.
    pub fn connected(&mut self, account: Id) {
        self.book.forget_failures(account);
        self.book.forget_checked(account);
        self.sent.remove(&account);
        self.settings_asked.remove(&account);
    }

    /// The calendars held back for want of rights are `held` (id, name, how many): which of them
    /// are to be told to the owner now, each once for as long as it stays so.
    pub fn tell_held(&mut self, held: Vec<(Id, String, usize)>) -> Vec<(String, usize)> {
        self.held_told.retain(|calendar, _| held.iter().any(|(id, ..)| id == calendar));
        let mut say = Vec::new();
        for (calendar, name, n) in held {
            if self.held_told.insert(calendar, n) != Some(n) {
                say.push((name, n));
            }
        }
        say
    }
}

impl Session {
    /// The stretch of the calendar a refresh asks for: the days on screen and a week either
    /// side, as instants.
    pub fn calendar_window(&self) -> (Timestamp, Timestamp) {
        let (from, to) = crate::cmd::view::calendar_range(self);
        self.days_as_instants(from.add_days(-MARGIN_DAYS), to.add_days(MARGIN_DAYS + 1))
    }

    /// The days from `from` up to (not with) `to` as instants of this session's time zone, at
    /// least a day long.
    pub fn days_as_instants(&self, from: Date, to: Date) -> (Timestamp, Timestamp) {
        let instant = |day: Date| {
            let midnight = DateTime::at_midnight(day);
            midnight.to_timestamp(self.offset_at(midnight.to_timestamp(0)))
        };
        let (from, to) = (instant(from), instant(to));
        (from, to.max(from.saturating_add(mailcraft_time::DAY)))
    }

    /// What of the calendar waits to go to `account`'s server now; `None` when nothing does.
    pub fn calendar_changes(&self, account: Id) -> Option<EventsUpload> {
        let mut upload = remote::event_upload(self, account).ok()?;
        // What makes the server send mail from a shared mailbox waits for the owner's permission.
        if self.driver.calendar.send_waits.contains(&account) {
            upload.ops.retain(|op| !op.mails_others());
        }
        Some(upload).filter(|upload| !upload.ops.is_empty())
    }

    /// The accounts whose calendar a Microsoft server keeps.
    pub fn microsoft_calendar_accounts(&self) -> Vec<Id> {
        let microsoft = |a: &&Account| a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft);
        self.mailbox.accounts.iter().filter(microsoft).map(|a| a.id).collect()
    }

    /// The calendars of `account` that a job brings down, each with the series the mailbox
    /// expects in the stretch (for the host to ask about by name when the stretch brings nothing
    /// of them): the main one, the others that are shown, and any that holds something which was
    /// sent and is yet to be seen there. The stretch is the wide one of a
    /// Send/Receive (`directory`: a month back, a year ahead), or else `stretch`.
    pub fn calendar_asks(&self, account: Id, directory: bool, stretch: Option<(Timestamp, Timestamp)>) -> Vec<CalendarAsk> {
        let now = self.now();
        let Some((from, to)) = (if directory { Some((now.saturating_sub(WIDE_BEHIND), now.saturating_add(WIDE_AHEAD))) } else { stretch }) else {
            return Vec::new();
        };
        // (And one that holds something which was sent and is yet to be seen on the server,
        // shown or not: an appointment that was just put into a calendar that isn't ticked.)
        let unseen = remote::unverified_calendars(self, account);
        // A shared mailbox's calendars are read once they are ticked, and not before (#70).
        let shared = self.mailbox.account(account).is_some_and(Account::is_shared);
        let followed = |c: &&Calendar| {
            c.origin.account == account && if shared { c.visible } else { c.origin.id.is_empty() || c.visible || directory || unseen.contains(&c.id) }
        };
        let mut asks: Vec<CalendarAsk> = Vec::new();
        // (Those the owner opened by address first: they count within the same cap, #70.)
        let mut ordered: Vec<&Calendar> = self.mailbox.calendars.iter().filter(followed).collect();
        ordered.sort_by_key(|c| !c.origin.id.starts_with(OPENED_PREFIX));
        for calendar in ordered.into_iter().take(MAX_ASKED) {
            if !asks.iter().any(|ask| ask.calendar == calendar.origin.id) {
                let series = remote::expected_series(self, account, &calendar.origin.id, from, to);
                asks.push(CalendarAsk { calendar: calendar.origin.id.clone(), series });
            }
        }
        asks
    }
}
