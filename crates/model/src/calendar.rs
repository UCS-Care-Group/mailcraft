// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars, events and recurrence.
//!
//! Event times are wall-clock times in the zone the calendar is shown in, which the mailbox names
//! ([`Mailbox::zone`](crate::Mailbox::zone)): a 9:00 meeting stays at 9:00 across a daylight-saving
//! change, which is what a recurring series needs. Readers convert UTC and zoned times to those
//! wall-clock times on the way in, and when the zone the calendar is shown in changes (the owner
//! travels), every timed event is rewritten to the clock of the new one
//! ([`Mailbox::rebase`](crate::Mailbox::rebase)). An event may have a zone of its own
//! ([`Event::start_zone`]); `zoned.rs` has what that means.

use std::collections::HashSet;

use mailcraft_time::{Date, DateTime, Weekday, month_name};
use serde::{Deserialize, Serialize};

use crate::{Address, Id, Importance, Origin, Rgb};

/// Most periods a recurrence walks, and most occurrences one query returns: a hostile rule (or a
/// daily series since 1970) costs bounded work.
const MAX_STEPS: i64 = 20_000;
const MAX_RESULTS: usize = 2_000;

/// What the account may do with a calendar, as its server (or, one day, a CalDAV server) says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Rights {
    /// Only when the owner is busy, never what with: no events can be listed.
    FreeBusy,
    /// Shown, never changed here.
    Read,
    /// Shown and changed, as the owner's own calendar is.
    #[default]
    Edit,
}

impl Rights {
    /// Whether the calendar's events can be changed here.
    pub fn can_edit(self) -> bool {
        self == Rights::Edit
    }

    /// The rights a file from before there were any says: `read_only` was all it knew.
    pub fn from_read_only(read_only: bool) -> Rights {
        if read_only { Rights::Read } else { Rights::Edit }
    }

    /// The rights a word names; anything that isn't one of them is the least that shows events.
    pub fn parse(word: &str) -> Rights {
        match word.trim().to_ascii_lowercase().as_str() {
            "edit" => Rights::Edit,
            "freebusy" => Rights::FreeBusy,
            _ => Rights::Read,
        }
    }
}

impl<'de> Deserialize<'de> for Rights {
    /// A file that holds something odd in place of the rights reads as [`Rights::Read`]: never
    /// more than it asked for.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Rights, D::Error> {
        Ok(match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::String(word) => Rights::parse(&word),
            _ => Rights::Read,
        })
    }
}

/// Whose a calendar is, when it isn't the account's own: the person it belongs to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarOwner {
    pub address: String,
    pub name: String,
}

/// Most characters of an owner's address and of a calendar's name or owner's name that are kept.
pub const MAX_OWNER_ADDRESS: usize = 254;

/// One line of text from outside, at most `max` characters, control characters as spaces.
pub(crate) fn flat_line(text: &str, max: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.trim().chars().take(max).collect::<String>().trim_end().to_string()
}

impl CalendarOwner {
    /// The owner as it can be kept: one line each, bounded; `None` when there is nothing in it.
    pub fn cleaned(&self) -> Option<CalendarOwner> {
        let owner =
            CalendarOwner { address: flat_line(&self.address, MAX_OWNER_ADDRESS).replace(' ', ""), name: flat_line(&self.name, crate::MAX_NAME) };
        (!owner.address.is_empty() || !owner.name.is_empty()).then_some(owner)
    }
}

/// A calendar the owner of an account opened by its owner's address (Open Shared Calendar),
/// kept here and on this computer only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OpenedCalendar {
    /// The person's address: what it is read through.
    pub address: String,
    /// What it is called (the person's name).
    pub name: String,
    pub color: Rgb,
    /// Whether it is ticked in the folder pane.
    pub ticked: bool,
}

/// Most calendars one account may have opened by address.
pub const MAX_OPENED_CALENDARS: usize = 20;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, from = "CalendarFile")]
pub struct Calendar {
    pub id: Id,
    pub name: String,
    pub color: Rgb,
    pub visible: bool,
    /// Set for a calendar that an account's server keeps (Microsoft 365, say): its events are
    /// copies of the server's, brought by a download.
    #[serde(skip_serializing_if = "Origin::is_local")]
    pub origin: Origin,
    /// What the account may do with it: a calendar of holidays or birthdays, or somebody else's
    /// that is shared for reading, is read and never changed here. A file from before there were
    /// rights says `read_only`, which reads as [`Rights::Read`] (or [`Rights::Edit`]).
    #[serde(skip_serializing_if = "is_edit")]
    pub rights: Rights,
    /// Whose it is, when it isn't the account's own (a colleague's or a shared mailbox's); `None`
    /// for the account's own and for a calendar kept on this computer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<CalendarOwner>,
    /// The group the server lists it under (`My Calendars`, `Other Calendars`); empty for none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// The colour the owner chose for it here (`calendar.color`), which it is then shown in
    /// whatever `color` is: a server's calendar keeps the server's colour there, and shows in it
    /// again when the owner's choice is taken back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own_color: Option<Rgb>,
    /// For a calendar that a server keeps: the services a meeting in it may be held online with,
    /// when the server said which they are. `None` when it didn't (making one is then tried, and
    /// the server's refusal is what says no).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online_providers: Option<Vec<OnlineProvider>>,
}

fn is_edit(rights: &Rights) -> bool {
    *rights == Rights::Edit
}

/// A calendar as a file writes it, including the `read_only` that files from before rights wrote.
#[derive(Default, Deserialize)]
#[serde(default)]
struct CalendarFile {
    id: Id,
    name: String,
    color: Rgb,
    visible: bool,
    origin: Origin,
    read_only: bool,
    rights: Option<Rights>,
    #[serde(deserialize_with = "crate::sync::lenient")]
    owner: Option<CalendarOwner>,
    group: String,
    own_color: Option<Rgb>,
    online_providers: Option<Vec<OnlineProvider>>,
}

impl From<CalendarFile> for Calendar {
    fn from(file: CalendarFile) -> Calendar {
        Calendar {
            id: file.id,
            name: file.name,
            color: file.color,
            visible: file.visible,
            origin: file.origin,
            rights: file.rights.unwrap_or_else(|| Rights::from_read_only(file.read_only)),
            owner: file.owner.and_then(|owner| owner.cleaned()),
            group: file.group,
            own_color: file.own_color,
            online_providers: file.online_providers,
        }
    }
}

impl Calendar {
    /// Whether the events of this calendar may be changed here.
    pub fn can_edit(&self) -> bool {
        self.rights.can_edit()
    }

    /// Whether a meeting in this calendar may be made a Teams meeting, as far as is known: its
    /// server didn't say which services it allows, or Teams is one of them.
    pub fn allows_teams(&self) -> bool {
        self.online_providers.as_ref().is_none_or(|allowed| allowed.contains(&OnlineProvider::Teams))
    }

    /// The colour it is shown in: the one chosen here when there is one, else its own.
    pub fn shown_color(&self) -> Rgb {
        self.own_color.unwrap_or(self.color)
    }
}

/// How an event shows in free/busy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShowAs {
    Free,
    Tentative,
    #[default]
    Busy,
    OutOfOffice,
    WorkingElsewhere,
}

impl ShowAs {
    pub const ALL: [ShowAs; 5] = [ShowAs::Free, ShowAs::WorkingElsewhere, ShowAs::Tentative, ShowAs::Busy, ShowAs::OutOfOffice];

    pub fn label(self) -> &'static str {
        match self {
            ShowAs::Free => "Free",
            ShowAs::Tentative => "Tentative",
            ShowAs::Busy => "Busy",
            ShowAs::OutOfOffice => "Out of Office",
            ShowAs::WorkingElsewhere => "Working Elsewhere",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            ShowAs::Free => "free",
            ShowAs::Tentative => "tentative",
            ShowAs::Busy => "busy",
            ShowAs::OutOfOffice => "outOfOffice",
            ShowAs::WorkingElsewhere => "workingElsewhere",
        }
    }

    pub fn parse(s: &str) -> Option<ShowAs> {
        let s = s.trim().to_ascii_lowercase().replace([' ', '-', '_'], "");
        Self::ALL.into_iter().find(|x| x.code().to_ascii_lowercase() == s || (s == "oof" && *x == ShowAs::OutOfOffice))
    }
}

/// An attendee's answer to an invitation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Response {
    #[default]
    None,
    Accepted,
    Tentative,
    Declined,
}

impl Response {
    pub fn label(self) -> &'static str {
        match self {
            Response::None => "No response",
            Response::Accepted => "Accepted",
            Response::Tentative => "Tentative",
            Response::Declined => "Declined",
        }
    }

    pub fn parse(s: &str) -> Option<Response> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "needs-action" | "" => Some(Response::None),
            "accepted" | "accept" => Some(Response::Accepted),
            "tentative" => Some(Response::Tentative),
            "declined" | "decline" => Some(Response::Declined),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Attendee {
    pub address: Address,
    pub optional: bool,
    pub response: Response,
}

/// What the people invited to a meeting are asked for, which its organizer decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResponseOptions {
    /// They are asked to answer the invitation.
    pub request_responses: bool,
    /// They may propose another time when they answer.
    pub allow_proposals: bool,
}

impl Default for ResponseOptions {
    /// Both, as a new meeting has it.
    fn default() -> Self {
        ResponseOptions { request_responses: true, allow_proposals: true }
    }
}

impl ResponseOptions {
    /// As a new meeting has them: nothing to write down.
    pub fn is_default(&self) -> bool {
        *self == ResponseOptions::default()
    }
}

/// The service a meeting is held online with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnlineProvider {
    #[default]
    Teams,
    SkypeForBusiness,
    Skype,
    /// Some other service, or one the server didn't name.
    Other,
}

impl OnlineProvider {
    /// What it is called in the interface.
    pub fn label(self) -> &'static str {
        match self {
            OnlineProvider::Teams => "Teams",
            OnlineProvider::SkypeForBusiness => "Skype for Business",
            OnlineProvider::Skype => "Skype",
            OnlineProvider::Other => "Online",
        }
    }

    /// The name Microsoft Graph has for it (`onlineMeetingProviderType`).
    pub fn code(self) -> &'static str {
        match self {
            OnlineProvider::Teams => "teamsForBusiness",
            OnlineProvider::SkypeForBusiness => "skypeForBusiness",
            OnlineProvider::Skype => "skypeForConsumer",
            OnlineProvider::Other => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<OnlineProvider> {
        match s.trim().to_ascii_lowercase().as_str() {
            "teamsforbusiness" | "teams" => Some(OnlineProvider::Teams),
            "skypeforbusiness" => Some(OnlineProvider::SkypeForBusiness),
            "skypeforconsumer" | "skype" => Some(OnlineProvider::Skype),
            "unknown" | "other" => Some(OnlineProvider::Other),
            _ => None,
        }
    }
}

/// Longest link to join a meeting by that is kept.
pub const MAX_JOIN_LINK: usize = 2_048;

/// A meeting that is (also) held online.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OnlineMeeting {
    pub provider: OnlineProvider,
    /// The link people join it by: an `https` address, as its server gave it. Empty until the
    /// server has made the meeting. It comes from outside: it is shown as it is, and followed
    /// only when the owner asks ([`OnlineMeeting::clean_link`] is what lets it in).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub join_url: String,
}

impl OnlineMeeting {
    /// A link to join a meeting by, when `url` can be one: an `https` address with a host, of
    /// one line, no longer than [`MAX_JOIN_LINK`]. Anything else is no link (`None`): not
    /// `http`, not `javascript:` or `file:`, nothing with a space or a control character in it,
    /// nothing with a name and a password before its host.
    pub fn clean_link(url: &str) -> Option<String> {
        let url = url.trim();
        let rest = url.get(..8).filter(|scheme| scheme.eq_ignore_ascii_case("https://")).and_then(|_| url.get(8..))?;
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        let plain = url.len() <= MAX_JOIN_LINK && !url.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '<' | '>' | '\\'));
        let named = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
        (plain && named).then(|| format!("https://{rest}"))
    }

    /// The same meeting with a link that can be one, or with none.
    pub fn tidied(mut self) -> OnlineMeeting {
        self.join_url = OnlineMeeting::clean_link(&self.join_url).unwrap_or_default();
        self
    }
}

/// Another time that somebody invited proposes for a meeting, as its organizer has it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Proposal {
    /// Who proposes it.
    pub from: Address,
    pub start: DateTime,
    pub end: DateTime,
}

fn is_normal(importance: &Importance) -> bool {
    *importance == Importance::Normal
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Freq {
    Daily,
    #[default]
    Weekly,
    Monthly,
    Yearly,
}

impl Freq {
    pub fn parse(s: &str) -> Option<Freq> {
        match s.trim().to_ascii_lowercase().as_str() {
            "daily" => Some(Freq::Daily),
            "weekly" => Some(Freq::Weekly),
            "monthly" => Some(Freq::Monthly),
            "yearly" => Some(Freq::Yearly),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Freq::Daily => "daily",
            Freq::Weekly => "weekly",
            Freq::Monthly => "monthly",
            Freq::Yearly => "yearly",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Freq::Daily => "day",
            Freq::Weekly => "week",
            Freq::Monthly => "month",
            Freq::Yearly => "year",
        }
    }
}

/// How an event repeats (the common subset of iCalendar's RRULE).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Recurrence {
    pub freq: Freq,
    /// Every `interval` days/weeks/months/years; 0 reads as 1.
    pub interval: u32,
    /// Weekly: the days of the week (none: the start's own). Daily: only on these days. Monthly and
    /// yearly with `nth`: the weekday(s) counted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub by_day: Vec<Weekday>,
    /// Monthly and yearly: the `nth` `by_day` weekday of the month (1–5, or -1 for the last)
    /// instead of the start's day of the month.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nth: Option<i8>,
    /// Ends after this many occurrences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// Ends on this day (inclusive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<Date>,
}

/// The `nth` (1–5, or negative for the last) `weekday` of a month; `None` when the month has no
/// such day (a fifth Friday).
fn nth_weekday(first_of_month: Date, weekday: Weekday, nth: i8) -> Option<Date> {
    if nth > 0 {
        let offset = (i64::from(weekday.index()) - i64::from(first_of_month.weekday().index())).rem_euclid(7);
        let day = 1 + offset + 7 * (i64::from(nth) - 1);
        Date::new(first_of_month.year, first_of_month.month, u8::try_from(day).ok()?)
    } else {
        let last = first_of_month.end_of_month();
        let back = (i64::from(last.weekday().index()) - i64::from(weekday.index())).rem_euclid(7);
        Some(last.add_days(-back))
    }
}

impl Recurrence {
    pub fn every(freq: Freq) -> Recurrence {
        Recurrence { freq, interval: 1, ..Default::default() }
    }

    fn step(&self) -> i64 {
        i64::from(self.interval.clamp(1, 1000))
    }

    /// The candidate dates of period `p` (0 is the period `start` is in), in order.
    fn period(&self, start: Date, p: i64) -> Vec<Date> {
        let n = p.saturating_mul(self.step());
        match self.freq {
            Freq::Daily => {
                let d = start.add_days(n);
                if self.by_day.is_empty() || self.by_day.contains(&d.weekday()) { vec![d] } else { Vec::new() }
            }
            Freq::Weekly => {
                let week = start.start_of_week(Weekday::Mon).add_days(n.saturating_mul(7));
                let mut days: Vec<Weekday> = if self.by_day.is_empty() { vec![start.weekday()] } else { self.by_day.clone() };
                days.sort();
                days.dedup();
                days.into_iter().map(|d| week.add_days(i64::from(d.index()))).collect()
            }
            Freq::Monthly | Freq::Yearly => {
                let months = if self.freq == Freq::Yearly { n.saturating_mul(12) } else { n };
                let first = start.start_of_month().add_months(i32::try_from(months).unwrap_or(i32::MAX));
                match self.nth {
                    Some(nth) if nth != 0 => {
                        let mut days: Vec<Weekday> = if self.by_day.is_empty() { vec![start.weekday()] } else { self.by_day.clone() };
                        days.sort();
                        days.dedup();
                        let mut out: Vec<Date> = days.into_iter().filter_map(|d| nth_weekday(first, d, nth)).collect();
                        out.sort();
                        out
                    }
                    // A day the month lacks (the 31st, February 29th) falls on its last day.
                    _ => vec![Date { day: start.day.min(first.end_of_month().day), ..first }],
                }
            }
        }
    }

    /// How many days every period after the first gives, when that is the same for all of them.
    fn per_period(&self) -> Option<u64> {
        match self.freq {
            Freq::Daily if self.by_day.is_empty() => Some(1),
            Freq::Daily => None,
            Freq::Weekly => Some(self.distinct_days()),
            Freq::Monthly | Freq::Yearly => match self.nth {
                None | Some(0) => Some(1),
                // The first to fourth, and the last, weekday of a month is always there.
                Some(nth) if (1..=4).contains(&nth) || nth < 0 => Some(self.distinct_days()),
                Some(_) => None,
            },
        }
    }

    fn distinct_days(&self) -> u64 {
        let mut days = self.by_day.clone();
        days.sort();
        days.dedup();
        days.len().max(1) as u64
    }

    /// The days the series falls on between `from` and `to` (inclusive), for a series that starts
    /// on `start`, without the `exdates`. Excluded days still count towards `count`.
    pub fn dates(&self, start: Date, from: Date, to: Date, exdates: &[Date]) -> Vec<Date> {
        let mut out = Vec::new();
        let last = self.until.map_or(to, |u| u.min(to));
        if last < start || to < from {
            return out;
        }
        // The periods before the window don't need walking.
        let behind = match self.freq {
            Freq::Daily => start.days_until(from) / self.step(),
            Freq::Weekly => start.days_until(from) / (7 * self.step()),
            Freq::Monthly => (i64::from(from.year - start.year) * 12 + i64::from(from.month) - i64::from(start.month)) / self.step(),
            Freq::Yearly => i64::from(from.year - start.year) / self.step(),
        };
        let skip = (behind - 1).max(0);
        let mut produced = 0u32;
        let first_period = match (self.count, self.per_period()) {
            (None, _) => skip,
            // A count: every period after the first gives the same number of days, so the
            // periods skipped are counted, not walked (the first has days before the start).
            (Some(count), Some(each)) if skip > 1 => {
                let first = self.period(start, 0).into_iter().filter(|day| *day >= start).count() as u64;
                let before = first.saturating_add(each.saturating_mul(skip as u64 - 1));
                if before >= u64::from(count) {
                    return out;
                }
                produced = u32::try_from(before).unwrap_or(u32::MAX);
                skip
            }
            (Some(_), _) => 0,
        };
        // Many excluded days are looked up in a set.
        let excluded: HashSet<Date> = exdates.iter().copied().collect();
        let mut previous: Option<Date> = None;
        for p in first_period..first_period.saturating_add(MAX_STEPS) {
            let period = self.period(start, p);
            // The calendar ends at year 9999: stop rather than repeat the clamped last day.
            if let (Some(first), Some(prev)) = (period.first(), previous)
                && *first <= prev
            {
                return out;
            }
            previous = period.last().copied().or(previous);
            for day in period {
                if day < start {
                    continue;
                }
                if day > last {
                    return out;
                }
                produced = produced.saturating_add(1);
                if self.count.is_some_and(|c| produced > c) {
                    return out;
                }
                if day >= from && !excluded.contains(&day) {
                    out.push(day);
                    if out.len() >= MAX_RESULTS {
                        return out;
                    }
                }
            }
        }
        out
    }

    /// The rule in words: `Every 2 weeks on Monday, Wednesday, until Dec 31, 2026`.
    pub fn describe(&self, start: Date) -> String {
        let mut s = match self.step() {
            1 => format!("Every {}", self.freq.unit()),
            n => format!("Every {n} {}s", self.freq.unit()),
        };
        let days = |list: &[Weekday]| {
            let mut d = list.to_vec();
            d.sort();
            d.dedup();
            d.iter().map(|d| d.name()).collect::<Vec<_>>().join(", ")
        };
        let own_day = [start.weekday()];
        let by_day = if self.by_day.is_empty() { &own_day[..] } else { &self.by_day[..] };
        match (self.freq, self.nth) {
            (Freq::Daily, _) if !self.by_day.is_empty() => s.push_str(&format!(" on {}", days(by_day))),
            (Freq::Daily, _) => {}
            (Freq::Weekly, _) => s.push_str(&format!(" on {}", days(by_day))),
            (Freq::Monthly, Some(n)) if n != 0 => s.push_str(&format!(" on the {} {}", ordinal(n), days(by_day))),
            (Freq::Monthly, _) => s.push_str(&format!(" on day {}", start.day)),
            (Freq::Yearly, Some(n)) if n != 0 => s.push_str(&format!(" on the {} {} of {}", ordinal(n), days(by_day), month_name(start.month))),
            (Freq::Yearly, _) => s.push_str(&format!(" on {} {}", month_name(start.month), start.day)),
        }
        if let Some(c) = self.count {
            s.push_str(&format!(", {c} time{}", if c == 1 { "" } else { "s" }));
        } else if let Some(u) = self.until {
            s.push_str(&format!(", until {}", u.medium()));
        }
        s
    }
}

fn ordinal(n: i8) -> &'static str {
    match n {
        1 => "first",
        2 => "second",
        3 => "third",
        4 => "fourth",
        5 => "fifth",
        _ => "last",
    }
}

/// One time an event happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occurrence {
    pub event: Id,
    pub start: DateTime,
    pub end: DateTime,
    /// The day of its series that it is: what [`Event::exceptions`] names it by, and what a
    /// command about "this occurrence" is told. It is the day it starts on, but for a series
    /// that repeats in another zone, where a change of clocks there or here can carry an
    /// occurrence across midnight ([`Event::occurrences_in`]).
    #[serde(default)]
    pub day: Date,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Event {
    pub id: Id,
    pub calendar: Id,
    /// iCalendar `UID`, kept so an exported event updates the same event elsewhere.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uid: String,
    pub title: String,
    pub location: String,
    pub description: String,
    pub start: DateTime,
    /// Exclusive: an all-day event on one day ends at the next midnight.
    pub end: DateTime,
    pub all_day: bool,
    /// The time zone the start is in, when the event has one of its own: the name of a zone of
    /// the IANA database (`Europe/Berlin`), `UTC`, or an offset (`UTC+05:30`). Empty for none,
    /// which is the zone the calendar is shown in. `start` and `end` are by the clock of the zone
    /// the calendar is shown in either way; a zone of its own says where the event was made, and
    /// for a series by whose clock it repeats ([`Event::occurrences_in`]). A whole-day event has
    /// no zone: its days are the same days everywhere.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub start_zone: String,
    /// The same for the end (a flight lands in another zone than it took off in).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub end_zone: String,
    /// The start is in the hour the clocks show twice (the night they are put back), and is the
    /// second time they show it. A wall-clock time alone can't say which of the two it is;
    /// without this it is the first. ([`Event::instants`] reads it, [`Event::set_instants`]
    /// writes it; it means nothing for a time the clocks show once.)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub start_repeated: bool,
    /// The same for the end.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub end_repeated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organizer: Option<Address>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<Attendee>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Recurrence>,
    /// Days of a recurring series that don't happen.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<Date>,
    /// Remind this many minutes before the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder_minutes: Option<u32>,
    pub show_as: ShowAs,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    pub private: bool,
    pub cancelled: bool,
    /// Set for an event that an account's server keeps: it is a copy of the server's, and what
    /// the server calls it is how the next download finds it again.
    #[serde(skip_serializing_if = "Origin::is_local")]
    pub origin: Origin,
    /// How much it matters: what the High and Low Importance buttons say.
    #[serde(skip_serializing_if = "is_normal")]
    pub importance: Importance,
    /// For a meeting: whether the people invited are asked to answer, and may propose another
    /// time. Both, unless the organizer said otherwise.
    #[serde(skip_serializing_if = "ResponseOptions::is_default")]
    pub response_options: ResponseOptions,
    /// It is held online (a Teams meeting): with what, and the link to join it by once its
    /// server has made it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online: Option<OnlineMeeting>,
    /// For a meeting the owner organizes: the other times that people invited have proposed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<Proposal>,
    /// Its server keeps files with it, which are not kept here.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub has_attachments: bool,
}

impl Event {
    /// The link to join it by, when it is held online and its server has given one.
    pub fn join_link(&self) -> Option<&str> {
        self.online.as_ref().map(|online| online.join_url.as_str()).filter(|link| !link.is_empty())
    }

    /// Minutes from start to end; never negative.
    pub fn duration_minutes(&self) -> i64 {
        self.start.minutes_until(self.end).max(0)
    }

    /// A meeting has other people in it; an appointment doesn't.
    pub fn is_meeting(&self) -> bool {
        !self.attendees.is_empty()
    }

    /// Every time the event happens on the days `from`–`to` (inclusive), in order, for an event
    /// that repeats by the clock its times are written in. A series with a zone of its own is
    /// asked through [`Event::occurrences_in`], which this is without one.
    pub fn occurrences(&self, from: Date, to: Date) -> Vec<Occurrence> {
        let minutes = self.duration_minutes();
        // An occurrence that started before `from` may still be running then.
        let reach = minutes / (24 * 60) + 1;
        let window_start = DateTime::at_midnight(from);
        let window_end = DateTime::at_midnight(to).add_days(1);
        let days = match &self.recurrence {
            Some(r) => r.dates(self.start.date, from.add_days(-reach), to, &self.exceptions),
            None => vec![self.start.date],
        };
        days.into_iter()
            .map(|day| {
                let start = DateTime { date: day, ..self.start };
                Occurrence { event: self.id, start, end: start.add_minutes(minutes), day }
            })
            // Overlap; an event without a length still shows at its start.
            .filter(|o| o.start < window_end && (o.end > window_start || o.start >= window_start))
            .collect()
    }

    /// `9:00 AM – 10:00 AM`, or `All day`.
    pub fn time_label(&self, twelve_hour: bool) -> String {
        if self.all_day { "All day".into() } else { format!("{} – {}", self.start.time_short(twelve_hour), self.end.time_short(twelve_hour)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    fn all(r: &Recurrence, start: Date, from: Date, to: Date) -> Vec<String> {
        r.dates(start, from, to, &[]).into_iter().map(Date::iso).collect()
    }

    #[test]
    fn daily_and_weekdays() {
        let start = d(2026, 10, 5); // Monday
        let r = Recurrence { interval: 2, ..Recurrence::every(Freq::Daily) };
        assert_eq!(all(&r, start, start, d(2026, 10, 10)), ["2026-10-05", "2026-10-07", "2026-10-09"]);
        let weekdays =
            Recurrence { by_day: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri], ..Recurrence::every(Freq::Daily) };
        assert_eq!(all(&weekdays, start, d(2026, 10, 9), d(2026, 10, 13)), ["2026-10-09", "2026-10-12", "2026-10-13"]);
    }

    #[test]
    fn weekly_on_several_days_with_count_and_until() {
        let start = d(2026, 10, 7); // Wednesday
        let r = Recurrence { by_day: vec![Weekday::Fri, Weekday::Mon, Weekday::Wed], count: Some(5), ..Recurrence::every(Freq::Weekly) };
        // The Monday of the first week is before the start.
        assert_eq!(all(&r, start, d(2026, 1, 1), d(2027, 1, 1)), ["2026-10-07", "2026-10-09", "2026-10-12", "2026-10-14", "2026-10-16"]);
        // The count runs from the start even when the window opens later.
        assert_eq!(all(&r, start, d(2026, 10, 13), d(2027, 1, 1)), ["2026-10-14", "2026-10-16"]);
        let r = Recurrence { interval: 2, until: Some(d(2026, 11, 4)), ..Recurrence::every(Freq::Weekly) };
        assert_eq!(all(&r, start, d(2026, 1, 1), d(2027, 1, 1)), ["2026-10-07", "2026-10-21", "2026-11-04"]);
    }

    #[test]
    fn monthly_by_day_and_by_nth_weekday() {
        let r = Recurrence::every(Freq::Monthly);
        assert_eq!(all(&r, d(2026, 1, 31), d(2026, 1, 1), d(2026, 4, 30)), ["2026-01-31", "2026-02-28", "2026-03-31", "2026-04-30"]);
        let second_tue = Recurrence { by_day: vec![Weekday::Tue], nth: Some(2), ..Recurrence::every(Freq::Monthly) };
        assert_eq!(all(&second_tue, d(2026, 10, 13), d(2026, 10, 1), d(2026, 12, 31)), ["2026-10-13", "2026-11-10", "2026-12-08"]);
        let last_fri = Recurrence { by_day: vec![Weekday::Fri], nth: Some(-1), ..Recurrence::every(Freq::Monthly) };
        assert_eq!(all(&last_fri, d(2026, 10, 30), d(2026, 10, 1), d(2026, 12, 31)), ["2026-10-30", "2026-11-27", "2026-12-25"]);
        // Only some months have a fifth Friday.
        let fifth_fri = Recurrence { by_day: vec![Weekday::Fri], nth: Some(5), ..Recurrence::every(Freq::Monthly) };
        assert_eq!(all(&fifth_fri, d(2026, 10, 30), d(2026, 10, 1), d(2027, 1, 31)), ["2026-10-30", "2027-01-29"]);
    }

    #[test]
    fn yearly_and_leap_days() {
        let r = Recurrence::every(Freq::Yearly);
        assert_eq!(all(&r, d(2024, 2, 29), d(2024, 1, 1), d(2028, 12, 31)), ["2024-02-29", "2025-02-28", "2026-02-28", "2027-02-28", "2028-02-29"]);
        // US Thanksgiving: the fourth Thursday of November.
        let r = Recurrence { by_day: vec![Weekday::Thu], nth: Some(4), ..Recurrence::every(Freq::Yearly) };
        assert_eq!(all(&r, d(2025, 11, 27), d(2025, 1, 1), d(2027, 12, 31)), ["2025-11-27", "2026-11-26", "2027-11-25"]);
    }

    #[test]
    fn far_windows_and_hostile_rules_are_bounded() {
        let r = Recurrence::every(Freq::Daily);
        assert_eq!(all(&r, d(1970, 1, 1), d(2026, 10, 11), d(2026, 10, 12)), ["2026-10-11", "2026-10-12"]);
        assert_eq!(r.dates(d(1970, 1, 1), d(1970, 1, 1), d(9999, 12, 31), &[]).len(), MAX_RESULTS);
        let hostile = Recurrence {
            interval: u32::MAX,
            count: Some(u32::MAX),
            by_day: vec![Weekday::Mon; 100],
            nth: Some(i8::MIN),
            ..Recurrence::every(Freq::Yearly)
        };
        let _ = hostile.dates(d(1, 1, 1), d(1, 1, 1), d(9999, 12, 31), &[]);
        let _ = Recurrence { interval: 1000, ..Recurrence::every(Freq::Monthly) }.dates(d(9990, 1, 1), d(1, 1, 1), d(9999, 12, 31), &[]);
        // A window before the start, or the wrong way round.
        assert!(r.dates(d(2026, 1, 1), d(2025, 1, 1), d(2025, 12, 31), &[]).is_empty());
        assert!(r.dates(d(2026, 1, 1), d(2026, 2, 1), d(2026, 1, 1), &[]).is_empty());
    }

    #[test]
    fn a_link_to_join_by_is_an_https_address_or_nothing() {
        let clean = OnlineMeeting::clean_link;
        let teams = "https://teams.example.org/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=%7b%22Tid%22%3a%221%22%7d";
        assert_eq!(clean(teams).as_deref(), Some(teams));
        assert_eq!(clean(&format!("  {teams}\r\n")).as_deref(), Some(teams));
        assert_eq!(clean("HTTPS://Teams.example.org/x").as_deref(), Some("https://Teams.example.org/x"));
        assert_eq!(clean("https://[2001:db8::1]:8443/x").as_deref(), Some("https://[2001:db8::1]:8443/x"));
        for not_one in [
            "",
            "https://",
            "https:///path",
            "http://teams.example.org/x",
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "data:text/html,<script>alert(1)</script>",
            "mailto:ada@example.org",
            "teams.example.org/x",
            "https://teams.example.org/a b",
            "https://teams.example.org/a\nb",
            "https://teams.example.org/a\u{0}",
            "https://user:secret@teams.example.org/",
            "https://teams.example.org@evil.example/",
            "https://evil.example\\@teams.example.org/",
            "https://teams.example.org/\"><script>",
            "https://tëams.example.org/x",
        ] {
            assert_eq!(clean(not_one), None, "{not_one:?}");
        }
        assert_eq!(clean(&format!("https://teams.example.org/{}", "a".repeat(MAX_JOIN_LINK))), None);
        // A meeting keeps a link that is one, and none that isn't; twice changes nothing more.
        let tidied = OnlineMeeting { provider: OnlineProvider::Teams, join_url: "javascript:alert(1)".into() }.tidied();
        assert_eq!(tidied, OnlineMeeting::default());
        assert_eq!(tidied.clone().tidied(), tidied);
        let event = Event { online: Some(OnlineMeeting { provider: OnlineProvider::Skype, join_url: teams.into() }), ..Default::default() };
        assert_eq!(
            (event.join_link(), Event::default().join_link(), Event { online: Some(tidied), ..Default::default() }.join_link()),
            (Some(teams), None, None)
        );
        // What a calendar's server allows, when it says; anything is tried when it doesn't.
        let calendar = |allowed: Option<Vec<OnlineProvider>>| Calendar { online_providers: allowed, ..Default::default() };
        assert!(calendar(None).allows_teams() && calendar(Some(vec![OnlineProvider::Skype, OnlineProvider::Teams])).allows_teams());
        assert!(!calendar(Some(Vec::new())).allows_teams() && !calendar(Some(vec![OnlineProvider::Skype])).allows_teams());
        for provider in [OnlineProvider::Teams, OnlineProvider::SkypeForBusiness, OnlineProvider::Skype, OnlineProvider::Other] {
            assert_eq!(OnlineProvider::parse(provider.code()), Some(provider));
        }
        assert_eq!(OnlineProvider::parse("somethingNew"), None);
    }

    #[test]
    fn exceptions_are_skipped_but_counted() {
        let r = Recurrence { count: Some(3), ..Recurrence::every(Freq::Daily) };
        let got = r.dates(d(2026, 10, 5), d(2026, 10, 1), d(2026, 10, 31), &[d(2026, 10, 6)]);
        assert_eq!(got, [d(2026, 10, 5), d(2026, 10, 7)]);
    }

    #[test]
    fn rules_in_words() {
        let start = d(2026, 10, 13);
        assert_eq!(Recurrence::every(Freq::Daily).describe(start), "Every day");
        assert_eq!(
            Recurrence { interval: 2, by_day: vec![Weekday::Wed, Weekday::Mon], ..Recurrence::every(Freq::Weekly) }.describe(start),
            "Every 2 weeks on Monday, Wednesday"
        );
        assert_eq!(Recurrence { count: Some(10), ..Recurrence::every(Freq::Weekly) }.describe(start), "Every week on Tuesday, 10 times");
        assert_eq!(Recurrence { nth: Some(2), ..Recurrence::every(Freq::Monthly) }.describe(start), "Every month on the second Tuesday");
        assert_eq!(
            Recurrence { until: Some(d(2030, 1, 1)), ..Recurrence::every(Freq::Yearly) }.describe(start),
            "Every year on October 13, until Jan 1, 2030"
        );
    }

    #[test]
    fn event_occurrences_overlap_the_window() {
        let at = |day: Date, h: u8| DateTime::new(day, h, 0, 0).unwrap();
        let once = Event { id: 7, start: at(d(2026, 10, 12), 9), end: at(d(2026, 10, 12), 10), ..Default::default() };
        assert_eq!(once.occurrences(d(2026, 10, 12), d(2026, 10, 12)).len(), 1);
        assert!(once.occurrences(d(2026, 10, 13), d(2026, 10, 20)).is_empty());
        assert_eq!(once.duration_minutes(), 60);
        // A three-day conference shows on each of its days.
        let conf = Event { all_day: true, start: at(d(2026, 10, 12), 0), end: at(d(2026, 10, 15), 0), ..Default::default() };
        assert_eq!(conf.occurrences(d(2026, 10, 14), d(2026, 10, 14)).len(), 1);
        assert!(conf.occurrences(d(2026, 10, 15), d(2026, 10, 15)).is_empty());
        // A weekly series, minus one cancelled day.
        let weekly = Event {
            start: at(d(2026, 10, 5), 14),
            end: at(d(2026, 10, 5), 15),
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            exceptions: vec![d(2026, 10, 19)],
            ..Default::default()
        };
        let got: Vec<String> = weekly.occurrences(d(2026, 10, 1), d(2026, 10, 31)).into_iter().map(|o| o.start.iso()).collect();
        assert_eq!(got, ["2026-10-05T14:00:00", "2026-10-12T14:00:00", "2026-10-26T14:00:00"]);
        // An event that ends before it starts has no length rather than a negative one.
        let odd = Event { start: at(d(2026, 10, 12), 9), end: at(d(2026, 10, 11), 9), ..Default::default() };
        assert_eq!((odd.duration_minutes(), odd.occurrences(d(2026, 10, 12), d(2026, 10, 12)).len()), (0, 1));
        assert_eq!(once.time_label(true), "9:00 AM – 10:00 AM");
    }
}
