// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Other people's free and busy times (Find a Time), the decisions and the state, with no I/O:
//! [`Session::free_busy_look`] says what is known for some people on a day and whether the
//! account's server is to be asked (one question at a time, only when a look is made, and not
//! again for the same people and day before its time); the host asks, notes that it did
//! ([`Session::free_busy_noted`]) and, when the server answers, hands the answer to
//! [`Session::free_busy_answered`], which takes it in through `calendar.freeBusy` and keeps what
//! was found out. Asking reads; it writes nothing and tells nobody.

use mailcraft_i18n::Lang;
use mailcraft_model::remote::{FreeBusy, FreeBusyAsk, MAX_SCHEDULES};
use mailcraft_model::{Account, Address, Id, OAuthProvider, ShowAs};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::{Busy, Start};
use crate::Session;

/// How long the server is left alone about the same people and day after it was asked and gave
/// nothing, and how long an answer is good for while the page stays in view (milliseconds).
pub const RETRY_MS: f64 = 30_000.0;
pub const REFRESH_MS: f64 = 300_000.0;
/// Most days and people that are remembered at a time.
pub const MAX_KNOWN: usize = 8;
/// A day, in minutes.
const DAY: u32 = 24 * 60;

/// One person's day, as far as it is known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Day {
    /// The time that is taken: from and to, in minutes of the day, and how. In order, and no
    /// two stretches overlap.
    pub busy: Vec<(u32, u32, ShowAs)>,
    /// Why nothing is known of this person; empty when something is.
    pub unknown: String,
}

/// Whose time is looked at, and on which day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// The account whose server is asked; 0 when there is none to ask.
    pub account: Id,
    pub day: Date,
    /// Their addresses, in lower case, each once, in order.
    pub people: Vec<String>,
}

/// What is known for a [`Key`].
#[derive(Clone, Debug, PartialEq)]
pub struct Known {
    pub key: Key,
    /// When the server was last asked (milliseconds).
    pub asked: f64,
    pub people: Vec<(String, Day)>,
    /// Why the last question came to nothing; empty when it came to something.
    pub error: String,
}

/// What the driver remembers of other people's times: what was found out, the newest last.
#[derive(Debug, Default)]
pub struct FreeBusyDriver {
    pub known: Vec<Known>,
}

impl FreeBusyDriver {
    /// Keep what is known for `key`: when it was asked, what came back (`None`: nothing yet, or
    /// nothing at all, and what was known before stays), and why not.
    pub fn note(&mut self, key: Key, asked: f64, people: Option<Vec<(String, Day)>>, error: String) {
        let known = &mut self.known;
        let before = known.iter().position(|known| known.key == key).map(|at| known.remove(at));
        let people = people.or(before.map(|before| before.people)).unwrap_or_default();
        known.push(Known { key, asked, people, error });
        let extra = known.len().saturating_sub(MAX_KNOWN);
        known.drain(..extra);
    }

    /// What is known for `key`.
    pub fn known_for(&self, key: &Key) -> Option<&Known> {
        self.known.iter().find(|known| known.key == *key)
    }

    /// Account removal: what was found out through these accounts goes with them.
    pub fn forget_accounts(&mut self, gone: &std::collections::HashSet<Id>) {
        self.known.retain(|known| !gone.contains(&known.key.account));
    }
}

/// What Find a Time gets when it looks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    /// What is known of each of the people, by address (in lower case).
    pub people: Vec<(String, Day)>,
    /// The server is being asked right now.
    pub pending: bool,
    /// Why other people's time isn't shown, when it isn't.
    pub note: String,
}

/// What the host says of itself when a look is made.
#[derive(Clone, Copy, Debug)]
pub struct FreeBusyFacts<'a> {
    /// The host has a way to ask a server when other people are busy.
    pub reachable: bool,
    /// What is at work now.
    pub busy: Busy,
    /// The question in flight, if one is.
    pub in_flight: Option<&'a Key>,
    /// The words an explanation is said in.
    pub lang: Lang,
}

/// A question the host is to ask the server.
#[derive(Clone, Debug)]
pub struct ToAsk {
    /// The account whose server is asked.
    pub account: Account,
    pub key: Key,
    /// What to ask, as the engine says it.
    pub ask: Result<FreeBusyAsk, String>,
}

/// What the host's question came to, as the engine takes it in (borrowed from the answer).
#[derive(Clone, Copy, Debug, Default)]
pub struct FreeBusyCame<'a> {
    pub schedule: Option<&'a FreeBusy>,
    /// What went wrong otherwise, in a few words.
    pub error: &'a str,
    /// The sign-in doesn't cover the calendar: the owner is to allow it.
    pub consent_needed: bool,
}

/// An address as people's times are kept by: trimmed, in lower case. Nothing for what isn't one.
pub fn address_key(email: &str) -> Option<String> {
    let email = email.trim();
    Address::new("", email).is_valid().then(|| email.to_lowercase())
}

/// The addresses of `people` as they are asked: each once, in order, as many as one question may
/// carry.
fn asked_of(people: &[String]) -> Vec<String> {
    let mut asked: Vec<String> = Vec::new();
    for email in people.iter().filter_map(|email| address_key(email)) {
        if asked.len() < MAX_SCHEDULES && !asked.contains(&email) {
            asked.push(email);
        }
    }
    asked
}

/// A day as `calendar.freeBusy` takes it, and the midnight after it.
fn stretch(day: Date) -> (String, String) {
    (format!("{}T00:00", day.iso()), format!("{}T00:00", day.add_days(1).iso()))
}

/// What `calendar.freeBusy` made of a server's answer, as days: for each of the people, the
/// stretches of `day` that are taken, cut to the day, in order. Whatever doesn't read is left out.
pub fn days_of(answer: &Value, day: Date) -> Vec<(String, Day)> {
    let midnight = DateTime::at_midnight(day);
    let minute = |v: Option<&Value>| -> Option<u32> {
        let (at, _) = mailcraft_time::parse_iso(v?.as_str()?)?;
        u32::try_from(midnight.minutes_until(at).clamp(0, i64::from(DAY))).ok()
    };
    let people = answer.get("people").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    people
        .iter()
        .take(MAX_SCHEDULES)
        .filter_map(|person| {
            let email = address_key(person.get("email")?.as_str()?)?;
            let times = person.get("busy").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            let mut busy: Vec<(u32, u32, ShowAs)> = times
                .iter()
                .take(2_000)
                .filter_map(|time| {
                    let (from, to) = (minute(time.get("start"))?, minute(time.get("end"))?);
                    let show_as = ShowAs::parse(time.get("showAs")?.as_str()?)?;
                    (to > from && show_as != ShowAs::Free).then_some((from, to, show_as))
                })
                .collect();
            busy.sort_by_key(|(from, to, _)| (*from, *to));
            let unknown: String =
                person.get("error").and_then(Value::as_str).unwrap_or_default().chars().filter(|c| !c.is_control()).take(200).collect();
            Some((email, Day { busy, unknown }))
        })
        .collect()
}

impl Session {
    /// The account whose server keeps `calendar`, when it is one that can be asked about other
    /// people's time; else why not, in words (said in `lang`).
    pub fn free_busy_account(&self, calendar: Id, reachable: bool, lang: Lang) -> Result<Account, String> {
        let mailbox = &self.mailbox;
        let keeper = mailbox.calendar(calendar).and_then(|c| crate::cmd::remote::keeper(mailbox, &c.origin));
        let microsoft = |a: &&Account| a.oauth.as_ref().is_some_and(|o| o.provider == OAuthProvider::Microsoft);
        let Some(account) = keeper.filter(microsoft) else {
            return Err(said!(
                "This meeting's calendar is kept on this computer, which can't tell when other people are busy: only your own calendar is shown."
            )
            .in_lang(lang));
        };
        if !reachable {
            return Err(said!("This build can't ask a server when other people are busy: only your own calendar is shown.").in_lang(lang));
        }
        if self.prefs.offline {
            return Err(said!("You're working offline. Turn off Work Offline (Send / Receive) to connect.").in_lang(lang));
        }
        if self.driver.calendar.consent_needed.contains(&account.id) {
            return Err(said!("Seeing when other people are busy needs your permission: File › Info › Connect calendar and contacts").in_lang(lang));
        }
        Ok(account.clone())
    }

    /// When `people` are free and busy on `day`, as far as that is known, for a meeting in
    /// `calendar`, at `now` (milliseconds); and the question the host is to ask the account's
    /// server when that is due: only the Scheduling Assistant looks, and only while its page is in
    /// view. One question at a time, not while the account's sign-in or calendar is being seen
    /// to, and not the same one again before its time.
    pub fn free_busy_look(&mut self, calendar: Id, people: &[String], day: Date, facts: &FreeBusyFacts<'_>, now: f64) -> (Look, Option<ToAsk>) {
        let asked = asked_of(people);
        if asked.is_empty() {
            return (Look::default(), None);
        }
        let account = self.free_busy_account(calendar, facts.reachable, facts.lang);
        let key = Key { account: account.as_ref().map_or(0, |a| a.id), day, people: asked };
        let known = self.driver.free_busy.known_for(&key);
        let mut look = Look {
            people: known.map(|known| known.people.clone()).unwrap_or_default(),
            pending: facts.in_flight == Some(&key),
            note: known.map(|known| known.error.clone()).unwrap_or_default(),
        };
        let account = match account {
            Ok(account) => account,
            Err(why) => {
                // Nobody to ask: what a script handed in for these people is shown all the same.
                if look.people.is_empty() {
                    look.note = why;
                }
                return (look, None);
            }
        };
        let due = known.is_none_or(|known| now - known.asked >= if known.error.is_empty() { REFRESH_MS } else { RETRY_MS });
        if !due || facts.busy.blocks(Start::FreeBusy) {
            return (look, None);
        }
        let (from, to) = stretch(day);
        let ask = self
            .run("calendar.freeBusy", &json!({"people": key.people, "from": from, "to": to}))
            .map_err(|e| e.to_string())
            .and_then(|answer| serde_json::from_value::<FreeBusyAsk>(answer.get("ask").cloned().unwrap_or_default()).map_err(|e| e.to_string()));
        (look, Some(ToAsk { account, key, ask }))
    }

    /// The host looked at the question for `key` at `now`: it went to the server (`error` empty),
    /// or why it didn't. What was known stays in view until the answer is there.
    pub fn free_busy_noted(&mut self, key: Key, now: f64, error: String) {
        self.driver.free_busy.note(key, now, None, error);
    }

    /// What `calendar.freeBusy` makes of a server's answer for `key`.
    fn free_busy_digest(&mut self, key: &Key, schedule: &FreeBusy) -> Result<Vec<(String, Day)>, String> {
        let (from, to) = stretch(key.day);
        let taken = self.run("calendar.freeBusy", &json!({"people": key.people, "from": from, "to": to, "result": schedule}));
        taken.map(|answer| days_of(&answer, key.day)).map_err(|e| e.to_string())
    }

    /// The host's question for `key` answered at `now`: take the answer in through
    /// `calendar.freeBusy` and keep what was found out. An answer for an account removed meanwhile
    /// changes nothing.
    pub fn free_busy_answered(&mut self, key: &Key, now: f64, came: &FreeBusyCame<'_>, lang: Lang) {
        if self.mailbox.account(key.account).is_none() {
            return;
        }
        if came.consent_needed {
            self.driver.calendar.consent_needed.insert(key.account);
        }
        let asked = self.driver.free_busy.known_for(key).map_or(now, |known| known.asked);
        let (people, error) = match came.schedule {
            Some(schedule) => match self.free_busy_digest(key, schedule) {
                Ok(people) => (Some(people), String::new()),
                Err(e) => (None, e),
            },
            None => {
                let why: String = came.error.chars().filter(|c| !c.is_control()).take(300).collect();
                (None, if why.trim().is_empty() { said!("The server didn't say when other people are busy").in_lang(lang) } else { why })
            }
        };
        self.driver.free_busy.note(key.clone(), asked, people, error);
    }

    /// An answer about people's time that didn't come from the host: a script hands it in, as a
    /// stand-in for the server. It is taken in as a host's answer is, for the same people and day.
    /// The key it was kept under and how many of the people it said something of.
    pub fn free_busy_stand_in(
        &mut self,
        calendar: Id,
        people: &[String],
        day: Date,
        schedule: &Value,
        facts: &FreeBusyFacts<'_>,
        now: f64,
    ) -> Result<(Key, usize), String> {
        let schedule: FreeBusy = serde_json::from_value(schedule.clone())
            .map_err(|e| format!("`freeBusy`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(200).collect::<String>()))?;
        let asked = asked_of(people);
        if asked.is_empty() {
            return Err(said!("Nobody with an address is invited").in_lang(facts.lang));
        }
        let key = Key { account: self.free_busy_account(calendar, facts.reachable, facts.lang).map_or(0, |a| a.id), day, people: asked };
        let people = self.free_busy_digest(&key, &schedule)?;
        let said = people.iter().filter(|(_, day)| day.unknown.is_empty()).count();
        self.driver.free_busy.note(key.clone(), now, Some(people), String::new());
        Ok((key, said))
    }
}
