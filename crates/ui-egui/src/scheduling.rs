// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Scheduling Assistant: the second page of a meeting's window, as Outlook has it.
//!
//! A row for everybody (the organizer first, then the people who are required and the ones who
//! are optional) over the day the meeting is on, with the working hours lighter than the rest
//! and each person's time that is taken as a bar: busy, tentative, out of office, working
//! elsewhere. Where the server has nothing to say of somebody, their row is hatched. The
//! meeting's own time lies across all rows as a band that can be moved, and made longer or
//! shorter by its edges: the form's start and end follow.
//!
//! The organizer's row is the owner's own calendar, as this mailbox has it. Everybody else's
//! comes from the account's server: the engine's `calendar.freeBusy` says what to ask, a host
//! with sockets asks ([`crate::Services::free_busy`]; the desktop app asks Microsoft Graph), and
//! `calendar.freeBusy` takes the answer in. Asking reads; it writes nothing and tells nobody.
//! It is done only while the page is in view, one question at a time, and not again for the same
//! people and day within [`RETRY_MS`]. Without a server to ask (the browser, a calendar that is
//! kept on this computer) the page shows the owner's own calendar and says why.

use std::sync::mpsc::Receiver;

use egui::{Align2, Color32, CursorIcon, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::remote::{FreeBusy, FreeBusyAsk, MAX_SCHEDULES};
use mailcraft_model::sync::Tokens as SignIn;
use mailcraft_model::{Address, Id, ShowAs};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, now_ms};

/// How long the server is left alone about the same people and day after it was asked and gave
/// nothing, and how long an answer is good for while the page stays in view (milliseconds).
pub(crate) const RETRY_MS: f64 = 30_000.0;
pub(crate) const REFRESH_MS: f64 = 300_000.0;
/// Most days and people that are remembered at a time.
const MAX_KNOWN: usize = 8;
/// Most people of one kind (required, optional) the page has a row for.
const MAX_ROWS: usize = 500;

/// The measures of the grid: the column of names, the row of hours, a person's row, and a
/// minute's width (a day is 24 hours of 60 points).
const NAME_W: f32 = 200.0;
const HEAD_H: f32 = 26.0;
const ROW_H: f32 = 28.0;
const MINUTE_W: f32 = 1.0;
const DAY: u32 = 24 * 60;
/// What the meeting's time snaps to when it is moved in the grid, and the least it lasts there.
const SNAP: i64 = 30;
/// How wide the part of an edge of the meeting's band is that takes hold of it.
const EDGE_W: f32 = 7.0;

/// What the host is asked to find out for the Scheduling Assistant
/// ([`Services::free_busy`](crate::Services::free_busy)): when the people of `ask` are busy, as
/// far as the account may know. It reads; nothing is written and nobody is told.
pub struct FreeBusyRequest {
    pub account: mailcraft_model::Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: mailcraft_model::OAuth,
    /// The account's refresh token, to get a token with when `access` is empty.
    pub refresh: String,
    /// An access token from earlier in this session that is still good; else empty.
    pub access: String,
    pub ask: FreeBusyAsk,
}

/// What a [`FreeBusyRequest`] came to.
#[derive(Default)]
pub struct FreeBusyResult {
    pub account: Id,
    /// A token that was obtained for this (kept for the session, as a calendar job's is).
    pub tokens: Option<SignIn>,
    /// The owner hasn't allowed this program to read the calendar.
    pub consent_needed: bool,
    /// The token that was handed in is no longer taken.
    pub token_refused: bool,
    /// What the server said; `None` when it couldn't be asked.
    pub schedule: Option<FreeBusy>,
    /// Why there is no answer, for the owner to read.
    pub error: String,
}

/// One person's day, as far as it is known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Day {
    /// The time that is taken: from and to, in minutes of the day, and how. In order, and no
    /// two stretches overlap.
    pub busy: Vec<(u32, u32, ShowAs)>,
    /// Why nothing is known of this person; empty when something is.
    pub unknown: String,
}

/// Whose time is looked at, and on which day.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    /// The account whose server is asked; 0 when there is none to ask.
    account: Id,
    day: Date,
    /// Their addresses, in lower case, each once, in order.
    people: Vec<String>,
}

/// What is known for a [`Key`].
struct Known {
    key: Key,
    /// When the server was last asked (milliseconds).
    asked: f64,
    people: Vec<(String, Day)>,
    /// Why the last question came to nothing; empty when it came to something.
    error: String,
}

/// What the app keeps of people's free and busy times.
#[derive(Default)]
pub(crate) struct Store {
    /// The question in flight, and the answer to come.
    rx: Option<(Key, Receiver<FreeBusyResult>)>,
    /// What was found out, the newest last.
    known: Vec<Known>,
    /// How many questions went to the host so far.
    pub(crate) asked: u64,
}

/// What the Scheduling Assistant gets when it looks.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Look {
    /// What is known of each of the people, by address (in lower case).
    pub people: Vec<(String, Day)>,
    /// The server is being asked right now.
    pub pending: bool,
    /// Why other people's time isn't shown, when it isn't.
    pub note: String,
}

/// An address as people's times are kept by: trimmed, in lower case. Nothing for what isn't one.
fn address_key(email: &str) -> Option<String> {
    let email = email.trim();
    Address::new("", email).is_valid().then(|| email.to_lowercase())
}

/// A day as `calendar.freeBusy` takes it, and the midnight after it.
fn stretch(day: Date) -> (String, String) {
    (format!("{}T00:00", day.iso()), format!("{}T00:00", day.add_days(1).iso()))
}

/// What `calendar.freeBusy` made of a server's answer, as days: for each of `people`, the
/// stretches of `day` that are taken, cut to the day, in order. Whatever doesn't read is left out.
fn days_of(answer: &Value, day: Date) -> Vec<(String, Day)> {
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

impl MailApp {
    /// When `people` are free and busy on `day`, as far as that is known, for a meeting in
    /// `calendar`. The account's server is asked when that is due: only the Scheduling
    /// Assistant looks, and only while its page is in view.
    pub(crate) fn free_busy(&mut self, calendar: Id, people: &[String], day: Date) -> Look {
        self.free_busy_at(calendar, people, day, now_ms())
    }

    /// The account whose server keeps `calendar`, when it is one that can be asked about other
    /// people's time; else why not, in words.
    fn free_busy_account(&self, calendar: Id) -> Result<mailcraft_model::Account, String> {
        let mailbox = &self.session.mailbox;
        let keeper = mailbox.calendar(calendar).and_then(|c| mailcraft_engine::cmd::remote::keeper(mailbox, &c.origin));
        let microsoft = |a: &&mailcraft_model::Account| a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft);
        let Some(account) = keeper.filter(microsoft) else {
            return Err(tl!(
                "This meeting's calendar is kept on this computer, which can't tell when other people are busy: only your own calendar is shown."
            )
            .to_string());
        };
        if self.services.free_busy.is_none() {
            return Err(tl!("This build can't ask a server when other people are busy: only your own calendar is shown.").to_string());
        }
        if self.graph_consent_needed.contains(&account.id) {
            return Err(tl!("Seeing when other people are busy needs your permission: File › Info › Connect calendar and contacts").to_string());
        }
        Ok(account.clone())
    }

    /// [`free_busy`](Self::free_busy) at `now` (milliseconds).
    pub(crate) fn free_busy_at(&mut self, calendar: Id, people: &[String], day: Date, now: f64) -> Look {
        let mut asked: Vec<String> = Vec::new();
        for email in people.iter().filter_map(|email| address_key(email)) {
            if asked.len() < MAX_SCHEDULES && !asked.contains(&email) {
                asked.push(email);
            }
        }
        if asked.is_empty() {
            return Look::default();
        }
        let account = self.free_busy_account(calendar);
        let key = Key { account: account.as_ref().map_or(0, |a| a.id), day, people: asked };
        let known = self.free_busy.known.iter().find(|known| known.key == key);
        let mut look = Look {
            people: known.map(|known| known.people.clone()).unwrap_or_default(),
            pending: self.free_busy.rx.as_ref().is_some_and(|(asking, _)| *asking == key),
            note: known.map(|known| known.error.clone()).unwrap_or_default(),
        };
        let account = match account {
            Ok(account) => account,
            Err(why) => {
                // Nobody to ask: what a script handed in for these people is shown all the same.
                if look.people.is_empty() {
                    look.note = why;
                }
                return look;
            }
        };
        // One question at a time, and not while the account's sign-in or calendar is being
        // seen to; and not the same question again before its time.
        let due = known.is_none_or(|known| now - known.asked >= if known.error.is_empty() { REFRESH_MS } else { RETRY_MS });
        let busy = self.free_busy.rx.is_some() || self.signing_in.is_some() || self.graph_rx.is_some() || self.calendar_syncing();
        if !due || busy {
            return look;
        }
        let Some(oauth) = account.oauth.clone() else { return look };
        let access = self.graph_tokens.get(&account.id).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone()).unwrap_or_default();
        let kept = self.tokens.get(&account.id).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", &account.email)).unwrap_or_default();
        let (from, to) = stretch(day);
        // The engine says what to ask.
        let ask = self
            .session
            .run("calendar.freeBusy", &json!({"people": key.people, "from": from, "to": to}))
            .map_err(|e| e.to_string())
            .and_then(|answer| serde_json::from_value::<FreeBusyAsk>(answer.get("ask").cloned().unwrap_or_default()).map_err(|e| e.to_string()));
        let failed = if access.is_empty() && refresh.is_empty() {
            Some(tl!("Sign in with Send/Receive to see when other people are busy.").to_string())
        } else {
            ask.as_ref().err().cloned()
        };
        let started = match (failed.is_none(), ask, &self.services.free_busy) {
            (true, Ok(ask), Some(host)) => {
                let request = FreeBusyRequest { account: account.clone(), oauth: self.effective_oauth(oauth), refresh, access, ask };
                self.free_busy.rx = Some((key.clone(), host(request)));
                self.free_busy.asked = self.free_busy.asked.saturating_add(1);
                true
            }
            _ => false,
        };
        // What was known stays in view until the answer is there.
        let error = failed.unwrap_or_default();
        self.note_free_busy(key, now, None, error.clone());
        (look.pending, look.note) = (started, error);
        look
    }

    /// Keep what is known for `key`: when it was asked, what came back (`None`: nothing yet, or
    /// nothing at all, and what was known before stays), and why not.
    fn note_free_busy(&mut self, key: Key, asked: f64, people: Option<Vec<(String, Day)>>, error: String) {
        let known = &mut self.free_busy.known;
        let before = known.iter().position(|known| known.key == key).map(|at| known.remove(at));
        let people = people.or(before.map(|before| before.people)).unwrap_or_default();
        known.push(Known { key, asked, people, error });
        let extra = known.len().saturating_sub(MAX_KNOWN);
        known.drain(..extra);
    }

    /// The host answered about people's time: keep its token, and take the answer in through
    /// `calendar.freeBusy`. Called every frame; it does nothing while no question is out.
    pub(crate) fn poll_free_busy(&mut self) {
        let Some((key, rx)) = &self.free_busy.rx else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => FreeBusyResult {
                account: key.account,
                error: tl!("Asking about other people's time stopped unexpectedly").to_string(),
                ..Default::default()
            },
        };
        let key = key.clone();
        self.free_busy.rx = None;
        let account = key.account;
        if result.token_refused {
            self.graph_tokens.remove(&account);
        }
        if let Some(tokens) = result.tokens
            && !self.signed_in_as_other(account, &tokens)
        {
            // The newest refresh token is the account's, for mail and the calendar alike.
            let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
            if !tokens.refresh.is_empty() {
                self.remember("refresh", &address, &tokens.refresh);
                if let Some(mail) = self.tokens.get_mut(&account) {
                    mail.refresh = tokens.refresh.clone();
                }
            }
            let now = mailcraft_time::now();
            self.graph_tokens.insert(account, SignIn { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens });
            // (Asked for as tokens always were: not known to be good for Outlook's Favorites.)
            self.favorites_token_is_plain(account);
        }
        if result.consent_needed {
            self.graph_consent_needed.insert(account);
        }
        let asked = self.free_busy.known.iter().find(|known| known.key == key).map_or_else(now_ms, |known| known.asked);
        let (people, error) = match &result.schedule {
            Some(schedule) => match self.digest_free_busy(&key, schedule) {
                Ok(people) => (Some(people), String::new()),
                Err(e) => (None, e),
            },
            None => {
                let why: String = result.error.chars().filter(|c| !c.is_control()).take(300).collect();
                (None, if why.trim().is_empty() { tl!("The server didn't say when other people are busy").to_string() } else { why })
            }
        };
        self.note_free_busy(key, asked, people, error);
    }

    /// What `calendar.freeBusy` makes of a server's answer for `key`.
    fn digest_free_busy(&mut self, key: &Key, schedule: &FreeBusy) -> Result<Vec<(String, Day)>, String> {
        let (from, to) = stretch(key.day);
        let taken = self.session.run("calendar.freeBusy", &json!({"people": key.people, "from": from, "to": to, "result": schedule}));
        taken.map(|answer| days_of(&answer, key.day)).map_err(|e| e.to_string())
    }

    /// An answer about people's time that didn't come from the host: a script hands it in
    /// (`ui.eventWindow` with `freeBusy`), as a stand-in for the server. It is taken in as a
    /// host's answer is, for the same people and day, and how many of them it said something of
    /// is answered.
    pub(crate) fn free_busy_stand_in(&mut self, calendar: Id, people: &[String], day: Date, schedule: &Value) -> Result<usize, String> {
        let schedule: FreeBusy = serde_json::from_value(schedule.clone())
            .map_err(|e| format!("`freeBusy`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(200).collect::<String>()))?;
        let mut asked: Vec<String> = Vec::new();
        for email in people.iter().filter_map(|email| address_key(email)) {
            if asked.len() < MAX_SCHEDULES && !asked.contains(&email) {
                asked.push(email);
            }
        }
        if asked.is_empty() {
            return Err(tl!("Nobody with an address is invited").to_string());
        }
        let key = Key { account: self.free_busy_account(calendar).map_or(0, |a| a.id), day, people: asked };
        let people = self.digest_free_busy(&key, &schedule)?;
        let said = people.iter().filter(|(_, day)| day.unknown.is_empty()).count();
        // (A question that is out for the same people is overtaken by this.)
        if self.free_busy.rx.as_ref().is_some_and(|(asking, _)| *asking == key) {
            self.free_busy.rx = None;
        }
        self.note_free_busy(key, now_ms(), Some(people), String::new());
        Ok(said)
    }
}

/// What the page needs to know of the meeting.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Meeting {
    /// The calendar it is in (or is to go into).
    pub calendar: Id,
    /// The event in the mailbox, when it is there already: its own time is not the owner's
    /// being busy with something else.
    pub event: Option<Id>,
    pub required: Vec<Address>,
    pub optional: Vec<Address>,
    /// The day the grid shows: the one the meeting starts on.
    pub day: Date,
    /// When it starts and ends; `None` while the fields don't say.
    pub span: Option<(DateTime, DateTime)>,
    pub all_day: bool,
}

/// What of the meeting's band is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    Move,
    Start,
    End,
}

/// The meeting's band being dragged: what of it, where the pointer took hold, and the minutes
/// (of the day) it started and ended at then.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    hold: Hold,
    grabbed: f32,
    start: i64,
    end: i64,
}

/// What the page drew the last time, for `ui.eventWindow` and the tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Drawn {
    /// The rows: everybody together, the organizer, and one for each person invited.
    pub rows: usize,
    /// How many of the people have a day that is known, and how many have none.
    pub known: usize,
    pub unknown: usize,
    /// How many stretches of taken time there are in all (the organizer's included).
    pub bars: usize,
    pub pending: bool,
    pub note: String,
}

/// The Scheduling Assistant's own state in a window.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct View {
    /// The grid was scrolled to the working day once.
    placed: bool,
    drag: Option<Drag>,
    /// The owner's own day, as of a state of the mailbox: its number, the day, the event that is
    /// left out, and the stretches.
    own: Option<(u64, Date, Option<Id>, Vec<(u32, u32, ShowAs)>)>,
    pub drawn: Drawn,
}

/// A row of the grid.
struct Row {
    name: String,
    /// What the tooltip says under the name.
    detail: String,
    /// Whose it is: 0 everybody, 1 the organizer, 2 required, 3 optional.
    kind: u8,
    /// What is known of the day; `None` where nothing is.
    day: Option<Day>,
}

fn mix(a: Color32, b: Color32, k: f32) -> Color32 {
    let k = k.clamp(0.0, 1.0);
    let part = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(part(a.r(), b.r()), part(a.g(), b.g()), part(a.b(), b.b()))
}

/// How heavily a way of being busy weighs when two overlap in the row for everybody.
fn weight(show_as: ShowAs) -> u8 {
    match show_as {
        ShowAs::Free => 0,
        ShowAs::WorkingElsewhere => 1,
        ShowAs::Tentative => 2,
        ShowAs::Busy => 3,
        ShowAs::OutOfOffice => 4,
    }
}

/// The owner's own day: what the calendars of this mailbox have on `day` that takes time, cut
/// to the day, without the meeting itself.
fn own_day(app: &MailApp, day: Date, without: Option<Id>) -> Vec<(u32, u32, ShowAs)> {
    let midnight = DateTime::at_midnight(day);
    let mailbox = &app.session.mailbox;
    // (A series by another zone's clock is wherever that clock puts it on this day.)
    let here = |instant: mailcraft_time::Timestamp| app.session.offset_at(instant);
    let mut busy: Vec<(u32, u32, ShowAs)> = mailbox
        .events
        .values()
        .filter(|e| Some(e.id) != without && !e.cancelled && e.show_as != ShowAs::Free)
        .flat_map(|e| e.occurrences_in(day, day, &here).into_iter().map(move |o| (o, e.show_as)))
        .take(2_000)
        .filter_map(|(o, show_as)| {
            let from = u32::try_from(midnight.minutes_until(o.start).clamp(0, i64::from(DAY))).ok()?;
            let to = u32::try_from(midnight.minutes_until(o.end).clamp(0, i64::from(DAY))).ok()?;
            (to > from).then_some((from, to, show_as))
        })
        .collect();
    busy.sort_by_key(|(from, to, show_as)| (*from, *to, weight(*show_as)));
    busy
}

/// Slanted lines across `rect`, counted from its left edge so they stay put while it scrolls.
fn hatch(painter: &egui::Painter, rect: Rect, color: Color32, pitch: f32) {
    let seen = rect.intersect(painter.clip_rect());
    if !seen.is_positive() {
        return;
    }
    let painter = painter.with_clip_rect(seen);
    let slant = rect.height();
    let first = ((seen.min.x - rect.min.x - slant) / pitch).floor().max(-(slant / pitch).ceil() - 1.0);
    let lines = (((seen.width() + 2.0 * slant) / pitch).ceil().clamp(0.0, 2_000.0)) as i32;
    for k in 0..=lines {
        let x = rect.min.x + (first + k as f32) * pitch;
        painter.line_segment([pos2(x, rect.max.y), pos2(x + slant, rect.min.y)], Stroke::new(1.0, color));
    }
}

/// One stretch of taken time, in `rect`.
fn bar(painter: &egui::Painter, rect: Rect, show_as: ShowAs, t: &Tokens) {
    let away = mix(t.blue, t.red, 0.5);
    match show_as {
        ShowAs::Free => {}
        ShowAs::Busy => {
            painter.rect_filled(rect, 2.0, t.accent);
        }
        ShowAs::OutOfOffice => {
            painter.rect_filled(rect, 2.0, away);
        }
        ShowAs::Tentative => {
            painter.rect_filled(rect, 2.0, mix(t.surface, t.accent, 0.18));
            hatch(painter, rect, t.accent, 5.0);
            painter.rect_stroke(rect, 2.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        }
        ShowAs::WorkingElsewhere => {
            painter.rect_filled(rect, 2.0, mix(t.surface, t.accent, 0.3));
            painter.rect_stroke(rect, 2.0, Stroke::new(1.0, mix(t.surface, t.accent, 0.7)), egui::StrokeKind::Inside);
        }
    }
}

/// A minute of the day brought to the grid's step.
fn snapped(minute: i64) -> i64 {
    ((minute as f64 / SNAP as f64).round() as i64).saturating_mul(SNAP)
}

/// Where a drag has the meeting now: `delta` minutes from where it was taken hold of. The
/// meeting stays within the day and keeps a length.
fn dragged(drag: &Drag, delta: i64) -> (i64, i64) {
    let day = i64::from(DAY);
    let length = drag.end.saturating_sub(drag.start).max(0);
    match drag.hold {
        Hold::Move => {
            let start = snapped(drag.start.saturating_add(delta)).clamp(0, day.saturating_sub(length).max(0));
            (start, start.saturating_add(length))
        }
        Hold::Start => (snapped(drag.start.saturating_add(delta)).clamp(0, drag.end.saturating_sub(SNAP).max(0)), drag.end),
        Hold::End => (drag.start, snapped(drag.end.saturating_add(delta)).clamp(drag.start.saturating_add(SNAP).clamp(0, day), day)),
    }
}

/// The name a row shows for somebody: the one that came with the address, the one a contact
/// with that address has (`look_up`: the contacts are looked through for it), or the address.
fn name_of(app: &MailApp, who: &Address, look_up: bool) -> String {
    let name = if !who.name.trim().is_empty() {
        who.name.clone()
    } else if look_up {
        let contact = app.session.mailbox.contacts.values().find(|c| c.has_email(&who.email)).map(|c| c.name());
        contact.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| who.email.clone())
    } else {
        who.email.clone()
    };
    name.chars().map(|c| if c.is_control() { ' ' } else { c }).take(120).collect()
}

/// Who a meeting is for: the owner whose calendar it is in, and the people invited (2 for one
/// who is required, 3 for one who is optional), each once and never the owner.
fn people_of(app: &MailApp, meeting: &Meeting) -> (Address, Vec<(u8, Address)>) {
    let mailbox = &app.session.mailbox;
    let keeper = mailbox.calendar(meeting.calendar).and_then(|c| mailcraft_engine::cmd::remote::keeper(mailbox, &c.origin));
    let owner = keeper.or(mailbox.default_account()).map(|a| a.address()).unwrap_or_default();
    let mut seen: Vec<String> = address_key(&owner.email).into_iter().collect();
    let mut invited: Vec<(u8, Address)> = Vec::new();
    for (kind, list) in [(2u8, &meeting.required), (3u8, &meeting.optional)] {
        for who in list.iter().take(MAX_ROWS) {
            let key = address_key(&who.email).unwrap_or_else(|| who.email.trim().to_lowercase());
            if !key.is_empty() && !seen.contains(&key) {
                seen.push(key);
                invited.push((kind, who.clone()));
            }
        }
    }
    (owner, invited)
}

/// Hand in an answer about the time of the people of `meeting` as a stand-in for the server's
/// (a script's `ui.eventWindow` with `freeBusy`). How many of them it said something of.
pub(crate) fn stand_in(app: &mut MailApp, meeting: &Meeting, schedule: &Value) -> Result<usize, String> {
    let (_, invited) = people_of(app, meeting);
    let asked: Vec<String> = invited.iter().filter_map(|(_, who)| address_key(&who.email)).collect();
    app.free_busy_stand_in(meeting.calendar, &asked, meeting.day, schedule)
}

/// Lay the page out in `height` points of `ui`, for the window numbered `window`. When the
/// meeting's band was moved or stretched, the start and end it has now are answered.
pub(crate) fn show(app: &mut MailApp, ui: &mut Ui, view: &mut View, meeting: &Meeting, window: u64, height: f32) -> Option<(DateTime, DateTime)> {
    let t = Tokens::get(ui.ctx());
    let twelve = app.session.prefs.twelve_hour;
    let midnight = DateTime::at_midnight(meeting.day);
    let (owner, invited) = people_of(app, meeting);
    let asked: Vec<String> = invited.iter().filter_map(|(_, who)| address_key(&who.email)).collect();
    let look = app.free_busy(meeting.calendar, &asked, meeting.day);
    // The owner's own day, looked up once for a state of the mailbox.
    let rev = app.session.rev();
    if !view.own.as_ref().is_some_and(|(at, day, without, _)| (*at, *day, *without) == (rev, meeting.day, meeting.event)) {
        view.own = Some((rev, meeting.day, meeting.event, own_day(app, meeting.day, meeting.event)));
    }
    let own = view.own.as_ref().map(|(.., busy)| busy.clone()).unwrap_or_default();
    let mut rows: Vec<Row> = Vec::with_capacity(invited.len() + 2);
    // (A meeting with very many people isn't looked up in the contacts one by one, every frame.)
    let look_up = invited.len() <= 60;
    let organizer = if owner.display().trim().is_empty() { tl!("You").to_string() } else { name_of(app, &owner, true) };
    rows.push(Row {
        name: organizer,
        detail: tl!("Organizer: your own calendar").to_string(),
        kind: 1,
        day: Some(Day { busy: own, unknown: String::new() }),
    });
    let nothing = if look.pending { tl!("Asking the server…") } else { tl!("No information") };
    for (kind, who) in &invited {
        let found = address_key(&who.email).and_then(|key| look.people.iter().find(|(email, _)| *email == key)).map(|(_, day)| day.clone());
        let role = if *kind == 2 { tl!("Required") } else { tl!("Optional") };
        let (day, why) = match found {
            Some(day) if day.unknown.is_empty() => (Some(day), String::new()),
            Some(day) => (None, day.unknown),
            None if address_key(&who.email).is_none() => (None, tl!("Not an address").to_string()),
            None => (None, nothing.to_string()),
        };
        let detail = if why.is_empty() { format!("{role} · {}", who.email) } else { format!("{role} · {} · {why}", who.email) };
        rows.push(Row { name: name_of(app, who, look_up), detail: detail.chars().filter(|c| !c.is_control()).take(300).collect(), kind: *kind, day });
    }
    // Everybody together: the heaviest of what anybody has at a time.
    let mut all: Vec<(u32, u32, ShowAs)> =
        rows.iter().filter_map(|row| row.day.as_ref()).flat_map(|day| day.busy.iter().copied()).take(20_000).collect();
    all.sort_by_key(|(from, to, show_as)| (weight(*show_as), *from, *to));
    rows.insert(
        0,
        Row { name: tl!("All Attendees").to_string(), detail: String::new(), kind: 0, day: Some(Day { busy: all, unknown: String::new() }) },
    );
    view.drawn = Drawn {
        rows: rows.len(),
        known: rows.iter().filter(|row| row.kind >= 2 && row.day.is_some()).count(),
        unknown: rows.iter().filter(|row| row.kind >= 2 && row.day.is_none()).count(),
        bars: rows.iter().filter(|row| row.kind >= 1).filter_map(|row| row.day.as_ref()).map(|day| day.busy.len()).sum(),
        pending: look.pending,
        note: look.note.clone(),
    };
    if look.pending {
        // (The host wakes the window when the answer is there; this is for a host that doesn't.)
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }

    // The meeting's own time, in minutes of the day shown.
    let span = meeting.span.map(|(start, end)| (midnight.minutes_until(start), midnight.minutes_until(end))).filter(|(start, end)| end >= start);
    let prefs = &app.session.prefs;
    let working = prefs.work_days.contains(&meeting.day.weekday());
    let (work_start, work_end) = (u32::from(prefs.work_start.min(24)) * 60, u32::from(prefs.work_end.min(24)) * 60);
    let legend_h = 44.0;
    let grid_h = (height - legend_h).max(HEAD_H + 2.0 * ROW_H);
    let content = vec2(NAME_W + f32::from(DAY as u16) * MINUTE_W, HEAD_H + rows.len() as f32 * ROW_H);
    let mut moved = None;
    let mut area = egui::ScrollArea::both().id_salt(("scheduling", window)).max_height(grid_h).auto_shrink([false, false]);
    if !std::mem::replace(&mut view.placed, true) {
        // To begin with: the working day, or the meeting when that starts before it.
        let first = span.map_or(i64::from(work_start), |(start, _)| start.min(i64::from(work_start)));
        area = area.horizontal_scroll_offset(((first - 60).clamp(0, i64::from(DAY)) as f32) * MINUTE_W);
    }
    let frame = egui::Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(4.0);
    frame.show(ui, |ui| {
        area.show_viewport(ui, |ui, viewport| {
            let (canvas, _) = ui.allocate_exact_size(content, Sense::hover());
            // What of the canvas is in view, on screen; the names and the hours stay put in it.
            let seen = Rect::from_min_size(canvas.min + viewport.min.to_vec2(), viewport.size()).intersect(ui.clip_rect());
            if !seen.is_positive() {
                return;
            }
            let grid = Rect::from_min_max(pos2(seen.min.x + NAME_W, seen.min.y + HEAD_H), seen.max);
            let x_of = |minute: f32| canvas.min.x + NAME_W + minute * MINUTE_W;
            let row_y = |row: usize| canvas.min.y + HEAD_H + row as f32 * ROW_H;
            let painter = ui.painter().with_clip_rect(grid);
            // The hours outside the working day are darker.
            painter.rect_filled(grid, 0.0, t.off_hours);
            if working && work_end > work_start {
                let hours = Rect::from_min_max(pos2(x_of(work_start as f32), grid.min.y), pos2(x_of(work_end as f32), grid.max.y));
                painter.rect_filled(hours, 0.0, t.surface);
            }
            for half in 0..=48u16 {
                let x = (x_of(f32::from(half) * 30.0)).floor() + 0.5;
                if x >= grid.min.x - 1.0 && x <= grid.max.x + 1.0 {
                    let color = if half % 2 == 0 { t.border_strong } else { t.border };
                    painter.line_segment([pos2(x, grid.min.y), pos2(x, grid.max.y)], Stroke::new(1.0, color));
                }
            }
            // The rows in view, and no others.
            let first = (((seen.min.y - canvas.min.y) / ROW_H).floor().max(0.0) as usize).min(rows.len());
            let count = ((seen.height() / ROW_H).ceil() as usize).saturating_add(2);
            for (i, row) in rows.iter().enumerate().skip(first).take(count) {
                let y = row_y(i);
                let lane = Rect::from_min_max(pos2(grid.min.x, y), pos2(grid.max.x, y + ROW_H));
                painter.line_segment([pos2(grid.min.x, y + ROW_H - 0.5), pos2(grid.max.x, y + ROW_H - 0.5)], Stroke::new(1.0, t.border));
                match &row.day {
                    Some(day) => {
                        for (from, to, show_as) in day.busy.iter().take(2_000) {
                            let stretch = Rect::from_min_max(pos2(x_of(*from as f32), y + 5.0), pos2(x_of(*to as f32), y + ROW_H - 6.0));
                            if stretch.max.x >= grid.min.x && stretch.min.x <= grid.max.x {
                                bar(&painter, stretch, *show_as, &t);
                            }
                        }
                    }
                    // Nothing is known of this person's day.
                    None => hatch(&painter, lane.shrink2(vec2(0.0, 4.0)), t.border_strong, 7.0),
                }
            }
            // A click on the grid puts the meeting there, as long as it was.
            let (pointer, pressed_at) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.press_origin()));
            let ground = ui.interact(grid, ui.scope_id().with(("scheduling_grid", window)), Sense::click());
            app.hit.add("schedulingGrid", window, grid);
            if let (true, Some(at), Some((start, end)), false) = (ground.clicked(), pointer, span, meeting.all_day) {
                let length = (end - start).clamp(0, i64::from(DAY));
                let minute = ((at.x - x_of(0.0)) / MINUTE_W) as i64;
                let to = ((minute.div_euclid(SNAP)) * SNAP).clamp(0, i64::from(DAY) - length);
                moved = Some((to, to + length));
            }
            // The meeting's own time: a band across everybody, with an edge to take hold of at
            // its start and at its end.
            if let Some((start, end)) = span {
                let (from, to) = (start.clamp(0, i64::from(DAY)) as f32, end.clamp(0, i64::from(DAY)) as f32);
                let band = Rect::from_min_max(pos2(x_of(from), grid.min.y), pos2(x_of(to).max(x_of(from) + 2.0), grid.max.y));
                painter.rect_filled(band, 0.0, t.accent.linear_multiply(if t.dark { 0.28 } else { 0.16 }));
                painter.line_segment([band.left_top(), band.left_bottom()], Stroke::new(2.0, t.green));
                painter.line_segment([band.right_top(), band.right_bottom()], Stroke::new(2.0, t.red));
                let held = band.intersect(grid);
                app.hit.add("schedulingBand", window, held);
                if !meeting.all_day && held.is_positive() {
                    let edge = |x: f32| Rect::from_min_max(pos2(x - EDGE_W / 2.0, grid.min.y), pos2(x + EDGE_W / 2.0, grid.max.y)).intersect(grid);
                    let parts = [
                        (Hold::Move, held, "schedulingMove"),
                        (Hold::Start, edge(band.min.x), "schedulingStart"),
                        (Hold::End, edge(band.max.x), "schedulingEnd"),
                    ];
                    for (hold, rect, kind) in parts {
                        // (A meeting that runs past the day has no end here to take hold of.)
                        if !rect.is_positive() || (hold == Hold::End && end > i64::from(DAY)) {
                            continue;
                        }
                        let part = ui.interact(rect, ui.scope_id().with(("scheduling_band", window, kind)), Sense::drag());
                        app.hit.add(kind, window, rect);
                        let part = part.on_hover_cursor(if hold == Hold::Move { CursorIcon::Grab } else { CursorIcon::ResizeHorizontal });
                        // (Counted from where the button went down, not from where the drag was
                        // noticed.)
                        if part.drag_started()
                            && let Some(at) = pressed_at.or(pointer)
                        {
                            view.drag = Some(Drag { hold, grabbed: at.x, start, end });
                        }
                        if part.dragged()
                            && let (Some(drag), Some(at)) = (view.drag.filter(|drag| drag.hold == hold), pointer)
                        {
                            let now = dragged(&drag, ((at.x - drag.grabbed) / MINUTE_W) as i64);
                            if now != (start, end) {
                                moved = Some(now);
                            }
                            ui.ctx().set_cursor_icon(if hold == Hold::Move { CursorIcon::Grabbing } else { CursorIcon::ResizeHorizontal });
                        }
                        if part.drag_stopped() {
                            view.drag = None;
                        }
                    }
                }
            }
            // The hours, over the rows as they scroll under them.
            let head = Rect::from_min_max(pos2(seen.min.x + NAME_W, seen.min.y), pos2(seen.max.x, seen.min.y + HEAD_H));
            let hours = ui.painter().with_clip_rect(head);
            hours.rect_filled(head, 0.0, t.panel);
            hours.line_segment([head.left_bottom(), head.right_bottom()], Stroke::new(1.0, t.border_strong));
            for hour in 0..24u8 {
                let x = x_of(f32::from(hour) * 60.0);
                if x + 60.0 >= head.min.x && x <= head.max.x {
                    hours.text(pos2(x + 5.0, head.center().y), Align2::LEFT_CENTER, crate::dates::time(hour, 0, twelve), regular(11.5), t.text_dim);
                }
            }
            // The names, over the grid as it scrolls under them.
            let names = Rect::from_min_max(seen.min, pos2(seen.min.x + NAME_W, seen.max.y));
            let column = ui.painter().with_clip_rect(names);
            column.rect_filled(names, 0.0, t.panel);
            column.line_segment([names.right_top(), names.right_bottom()], Stroke::new(1.0, t.border_strong));
            let below = ui.painter().with_clip_rect(Rect::from_min_max(pos2(names.min.x, names.min.y + HEAD_H), names.max));
            for (i, row) in rows.iter().enumerate().skip(first).take(count) {
                let y = row_y(i);
                let cell = Rect::from_min_max(pos2(names.min.x, y), pos2(names.max.x, y + ROW_H));
                below.line_segment([pos2(cell.min.x, cell.max.y - 0.5), pos2(cell.max.x, cell.max.y - 0.5)], Stroke::new(1.0, t.border));
                let mark = pos2(cell.min.x + 14.0, cell.center().y);
                match row.kind {
                    0 => crate::icons::paint(&below, Rect::from_center_size(mark, vec2(16.0, 16.0)), "meeting", t.icon, t.accent),
                    1 => {
                        below.circle_filled(mark, 4.5, t.accent);
                    }
                    2 => {
                        below.circle_filled(mark, 4.5, t.icon);
                    }
                    _ => {
                        below.circle_stroke(mark, 4.0, Stroke::new(1.2, t.text_dim));
                    }
                }
                let (font, ink) =
                    if row.kind == 0 { (semibold(12.5), t.text) } else { (regular(12.5), if row.day.is_some() { t.text } else { t.text_dim }) };
                paint_clipped(&below, pos2(cell.min.x + 28.0, cell.center().y), NAME_W - 36.0, &row.name, font, ink);
                let visible = cell.intersect(Rect::from_min_max(pos2(names.min.x, names.min.y + HEAD_H), names.max));
                if !row.detail.is_empty() && visible.is_positive() {
                    ui.interact(visible, ui.scope_id().with(("scheduling_name", window, i)), Sense::hover()).on_hover_text(row.detail.as_str());
                }
            }
            // The corner: the day that is shown.
            let corner = Rect::from_min_max(seen.min, pos2(seen.min.x + NAME_W, seen.min.y + HEAD_H));
            let top = ui.painter().with_clip_rect(corner);
            top.rect_filled(corner, 0.0, t.panel);
            top.line_segment([corner.left_bottom(), corner.right_bottom()], Stroke::new(1.0, t.border_strong));
            top.line_segment([corner.right_top(), corner.right_bottom()], Stroke::new(1.0, t.border_strong));
            let day = crate::dates::weekday_medium(meeting.day);
            paint_clipped(&top, pos2(corner.min.x + 10.0, corner.center().y), NAME_W - 16.0, &day, semibold(12.0), t.text);
        });
    });
    // What the bars mean, and what there is to say about where they come from.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let entries: [(Option<ShowAs>, &str); 5] = [
            (Some(ShowAs::Busy), "Busy"),
            (Some(ShowAs::Tentative), "Tentative"),
            (Some(ShowAs::OutOfOffice), "Out of Office"),
            (Some(ShowAs::WorkingElsewhere), "Working Elsewhere"),
            (None, "No information"),
        ];
        for (show_as, label) in entries {
            let (swatch, _) = ui.allocate_exact_size(vec2(22.0, 12.0), Sense::hover());
            match show_as {
                Some(show_as) => bar(ui.painter(), swatch, show_as, &t),
                None => {
                    ui.painter().rect_stroke(swatch, 2.0, Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
                    hatch(&ui.painter().with_clip_rect(swatch), swatch, t.border_strong, 5.0);
                }
            }
            ui.label(egui::RichText::new(tl!(label)).size(11.5).color(t.text_dim));
            ui.add_space(6.0);
        }
    });
    let note = if !look.note.is_empty() {
        look.note.clone()
    } else if look.pending {
        tl!("Asking the server when everybody is free…").to_string()
    } else if invited.is_empty() {
        tl!("Name the people to invite on the Appointment page: their time shows here.").to_string()
    } else {
        String::new()
    };
    if !note.is_empty() {
        ui.add(
            egui::Label::new(
                egui::RichText::new(note.chars().filter(|c| !c.is_control()).take(300).collect::<String>()).size(11.5).color(t.text_dim),
            )
            .truncate(),
        );
    }
    moved.map(|(start, end)| (midnight.add_minutes(start), midnight.add_minutes(end)))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_engine::Session;
    use mailcraft_model::remote::{BusyTime, PersonSchedule};

    use super::*;
    use crate::Services;
    use crate::calendar_sync::testing::{event, microsoft_account, serve};

    /// Wednesday 2026-10-14 16:00 UTC, and the midnight that day began with.
    const NOW: i64 = 1_791_993_600;
    const MIDNIGHT: i64 = NOW - 16 * 3600;

    /// What the stand-in for the host was asked, and the ends it answers through when it is time.
    type Asked = Arc<Mutex<Vec<(FreeBusyRequest, mpsc::Sender<FreeBusyResult>)>>>;

    /// The sample mailbox with a Microsoft account whose calendar has come down, and a host for
    /// free and busy times that answers when the test says so. The id of the server's calendar.
    fn app() -> (MailApp, Asked, Id) {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        let server = serve(&mut app);
        let account = microsoft_account(&mut app);
        server.lock().unwrap().events = vec![event("e-1", "Budget review", NOW - 3 * 3600)];
        assert!(app.calendar_tick_at(true, now_ms()));
        app.poll_calendar();
        app.auto_sync = false;
        let calendar = app.session.mailbox.calendars.iter().find(|c| c.origin.account == account).map(|c| c.id).expect("the server's calendar");
        let asked: Asked = Default::default();
        let seen = asked.clone();
        app.services.free_busy = Some(Box::new(move |request: FreeBusyRequest| {
            let (tx, rx) = mpsc::channel();
            seen.lock().unwrap().push((request, tx));
            rx
        }));
        (app, asked, calendar)
    }

    /// Answer the question that is out, and let the app take the answer in.
    fn answer(app: &mut MailApp, asked: &Asked, result: FreeBusyResult) {
        let (request, tx) = asked.lock().unwrap().pop().expect("a question is out");
        tx.send(FreeBusyResult { account: request.account.id, ..result }).unwrap();
        app.poll_free_busy();
    }

    fn busy(from_hour: i64, to_hour: i64, show_as: ShowAs) -> BusyTime {
        BusyTime { start: MIDNIGHT + from_hour * 3600, end: MIDNIGHT + to_hour * 3600, show_as }
    }

    fn says(people: Vec<PersonSchedule>) -> FreeBusyResult {
        FreeBusyResult { schedule: Some(FreeBusy { from: MIDNIGHT, to: MIDNIGHT + 86_400, people }), ..Default::default() }
    }

    #[test]
    fn the_server_is_asked_once_and_not_again_before_its_time() {
        let (mut app, asked, calendar) = app();
        let day = Date::new(2026, 10, 14).unwrap();
        let people =
            vec!["Maya@Lumen.example".to_string(), "tomas@lumen.example".to_string(), "maya@lumen.example".to_string(), "nobody".to_string()];
        let t0 = now_ms();
        // The first look asks: the day, from midnight to midnight, for each address once.
        let look = app.free_busy_at(calendar, &people, day, t0);
        assert_eq!((look.pending, look.people.len(), look.note.as_str(), app.free_busy.asked), (true, 0, "", 1));
        {
            let out = asked.lock().unwrap();
            let request = &out[0].0;
            assert_eq!(
                request.ask,
                FreeBusyAsk { people: vec!["maya@lumen.example".into(), "tomas@lumen.example".into()], from: MIDNIGHT, to: MIDNIGHT + 86_400 }
            );
            // It goes with the account's sign-in, as a calendar job does.
            assert_eq!((request.account.email.as_str(), request.access.as_str()), ("avery@mail.example", "graph-tok"));
        }
        // One question at a time: looking again, or at another day, asks nothing while it is out.
        for later in [1.0, 40_000.0, 400_000.0] {
            assert!(app.free_busy_at(calendar, &people, day, t0 + later).pending);
            assert!(!app.free_busy_at(calendar, &people, day.add_days(1), t0 + later).pending);
        }
        assert_eq!(app.free_busy.asked, 1);
        // The answer: Maya is busy from ten to eleven and tentative after; of Tomás the server
        // says nothing at all.
        let maya = PersonSchedule {
            email: "maya@lumen.example".into(),
            busy: vec![busy(11, 12, ShowAs::Tentative), busy(10, 11, ShowAs::Busy)],
            ..Default::default()
        };
        answer(&mut app, &asked, says(vec![maya]));
        let look = app.free_busy_at(calendar, &people, day, t0 + 2_000.0);
        assert_eq!((look.pending, look.note.as_str(), app.free_busy.asked), (false, "", 1));
        assert_eq!(look.people.len(), 2);
        assert_eq!(
            look.people[0],
            ("maya@lumen.example".to_string(), Day { busy: vec![(600, 660, ShowAs::Busy), (660, 720, ShowAs::Tentative)], unknown: String::new() })
        );
        assert!(look.people[1].0 == "tomas@lumen.example" && look.people[1].1.busy.is_empty() && !look.people[1].1.unknown.is_empty());
        // The same people and day: not again for five minutes, however often it is looked at.
        for later in [3_000.0, 29_000.0, 31_000.0, 299_000.0] {
            assert!(!app.free_busy_at(calendar, &people, day, t0 + later).pending, "{later}");
        }
        assert_eq!(app.free_busy.asked, 1);
        let look = app.free_busy_at(calendar, &people, day, t0 + 301_000.0);
        assert_eq!((look.pending, look.people.len(), app.free_busy.asked), (true, 2, 2), "what was known stays in view meanwhile");
        // That question comes to nothing: said, and tried again after half a minute, not before.
        let t1 = t0 + 301_000.0;
        answer(&mut app, &asked, FreeBusyResult { error: "Couldn't see when people are free: no route\u{7}".into(), ..Default::default() });
        let look = app.free_busy_at(calendar, &people, day, t1 + 1_000.0);
        assert_eq!((look.pending, look.note.as_str(), look.people.len()), (false, "Couldn't see when people are free: no route", 2));
        assert!(!app.free_busy_at(calendar, &people, day, t1 + 29_000.0).pending);
        assert_eq!(app.free_busy.asked, 2);
        assert!(app.free_busy_at(calendar, &people, day, t1 + 31_000.0).pending);
        assert_eq!(app.free_busy.asked, 3);
        // A host that goes away without answering is a question that came to nothing.
        asked.lock().unwrap().clear();
        app.poll_free_busy();
        assert!(app.free_busy_at(calendar, &people, day, t1 + 32_000.0).note.contains("stopped unexpectedly"));
        // Other people, or another day, are another question: asked at once.
        assert!(app.free_busy_at(calendar, &people, day.add_days(1), t1 + 33_000.0).pending);
        assert_eq!(app.free_busy.asked, 4);
        let stretch = asked.lock().unwrap()[0].0.ask.clone();
        assert_eq!((stretch.from, stretch.to), (MIDNIGHT + 86_400, MIDNIGHT + 2 * 86_400));
        answer(&mut app, &asked, says(Vec::new()));
        // A token that came with an answer is the account's from then on.
        let account = app.session.mailbox.calendars.iter().find(|c| c.id == calendar).map(|c| c.origin.account).unwrap();
        let fresh = SignIn { access: "newer".into(), refresh: "ref-9".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() };
        assert!(app.free_busy_at(calendar, &["priya@lumen.example".to_string()], day, t1 + 34_000.0).pending);
        answer(&mut app, &asked, FreeBusyResult { tokens: Some(fresh), ..says(Vec::new()) });
        assert_eq!((app.graph_tokens[&account].access.as_str(), app.tokens[&account].refresh.as_str()), ("newer", "ref-9"));
        // Nobody with an address: nothing to ask.
        assert_eq!(app.free_busy_at(calendar, &["nobody".to_string(), String::new()], day, t1 + 35_000.0), Look::default());
        assert_eq!(app.free_busy.asked, 5);
        // More people than a server answers about at once: the first twenty.
        let crowd: Vec<String> = (0..3_000).map(|i| format!("p{i}@example.org")).collect();
        assert!(app.free_busy_at(calendar, &crowd, day, t1 + 36_000.0).pending);
        assert_eq!(asked.lock().unwrap()[0].0.ask.people.len(), MAX_SCHEDULES);
    }

    #[test]
    fn without_a_server_to_ask_it_says_why_and_asks_nothing() {
        let (mut app, asked, calendar) = app();
        let day = Date::new(2026, 10, 14).unwrap();
        let people = vec!["maya@lumen.example".to_string()];
        let account = app.session.mailbox.calendar(calendar).map(|c| c.origin.account).unwrap();
        // A calendar that is kept on this computer.
        let own = app.session.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).expect("a calendar of the mailbox's own");
        let look = app.free_busy(own, &people, day);
        assert!(!look.pending && look.note.contains("kept on this computer"), "{}", look.note);
        // The owner hasn't allowed it.
        app.graph_consent_needed.insert(account);
        assert!(app.free_busy(calendar, &people, day).note.contains("needs your permission"));
        app.graph_consent_needed.clear();
        // No sign-in at hand: Send/Receive asks for one, this doesn't.
        let (tokens, graph_tokens) = (std::mem::take(&mut app.tokens), std::mem::take(&mut app.graph_tokens));
        let look = app.free_busy(calendar, &people, day);
        assert!(!look.pending && look.note.contains("Sign in with Send/Receive"), "{}", look.note);
        (app.tokens, app.graph_tokens) = (tokens, graph_tokens);
        // A build that can't ask (the browser).
        let host = app.services.free_busy.take();
        assert!(app.free_busy(calendar, &people, day.add_days(3)).note.contains("can't ask a server"));
        app.services.free_busy = host;
        // While the account's calendar is being seen to, the question waits its turn.
        app.auto_sync = true;
        app.calendar.due = true;
        app.execute("view.calendar", json!({})).unwrap();
        assert!(app.calendar_tick_at(true, now_ms() + 400_000.0));
        assert!(!app.free_busy(calendar, &people, day.add_days(5)).pending);
        app.poll_calendar();
        assert!(asked.lock().unwrap().is_empty() && app.free_busy.asked == 0);
        assert!(app.free_busy(calendar, &people, day.add_days(5)).pending);
        // The answer says the owner hasn't allowed it: said from then on, and not asked again.
        answer(&mut app, &asked, FreeBusyResult { consent_needed: true, error: "consent".into(), ..Default::default() });
        assert!(app.free_busy(calendar, &people, day.add_days(6)).note.contains("needs your permission"));
        assert_eq!(app.free_busy.asked, 1);
    }

    #[test]
    fn hostile_answers_are_cut_to_the_day_and_to_size() {
        let (mut app, asked, calendar) = app();
        let day = Date::new(2026, 10, 14).unwrap();
        let people: Vec<String> = ["maya@lumen.example", "tomas@lumen.example", "priya@lumen.example"].map(String::from).to_vec();
        assert!(app.free_busy(calendar, &people, day).pending);
        // Thousands of people nobody asked about, the ones asked about several times over, and
        // stretches out of order, backwards, endless, overlapping, and far from the day.
        let wild = |i: i64| BusyTime {
            start: MIDNIGHT + (i * 7_919) % 200_000 - 50_000,
            end: MIDNIGHT + (i * 104_729) % 300_000 - 90_000,
            show_as: ShowAs::Busy,
        };
        let mut crowd = vec![
            PersonSchedule { email: "MAYA@lumen.example".into(), busy: (0..5_000).map(wild).collect(), error: String::new() },
            PersonSchedule { email: "maya@lumen.example".into(), busy: vec![busy(1, 2, ShowAs::OutOfOffice)], error: String::new() },
        ];
        let ends = vec![
            BusyTime { start: i64::MIN, end: i64::MAX, show_as: ShowAs::Tentative },
            BusyTime { start: i64::MAX, end: i64::MIN, show_as: ShowAs::Busy },
            busy(30, 40, ShowAs::Busy),
            busy(-40, -30, ShowAs::Busy),
            busy(12, 13, ShowAs::Free),
        ];
        crowd.push(PersonSchedule { email: "tomas@lumen.example".into(), busy: ends, error: "\u{0}\u{202e}é\n".repeat(5_000) });
        crowd.push(PersonSchedule { email: "\u{0}not an address".into(), busy: vec![busy(1, 2, ShowAs::Busy)], error: String::new() });
        crowd.extend((0..4_000).map(|i| PersonSchedule {
            email: format!("stranger{i}@example.org"),
            busy: vec![busy(9, 10, ShowAs::Busy)],
            ..Default::default()
        }));
        answer(&mut app, &asked, says(crowd));
        let look = app.free_busy(calendar, &people, day);
        assert_eq!(look.people.len(), 3, "only the people who were asked about, each once");
        for (email, day) in &look.people {
            assert!(day.busy.len() <= 2_000 && day.unknown.chars().count() <= 300, "{email}");
            // In order, within the day, and no two at the same time.
            let mut before = 0;
            for (from, to, show_as) in &day.busy {
                assert!(*from >= before && to > from && *to <= DAY && *show_as != ShowAs::Free, "{email}: {from}–{to} after {before}");
                before = *to;
            }
        }
        assert!(!look.people[0].1.busy.is_empty() && look.people[2].1.busy.is_empty() && !look.people[2].1.unknown.is_empty());
        // A stand-in handed in by a script goes the same way, whatever it is.
        let meeting = Meeting {
            calendar,
            event: None,
            required: vec![Address::new("Maya Okafor", "maya@lumen.example"), Address::new("", "bob")],
            optional: vec![Address::new("", "avery@mail.example"), Address::new("", "Maya@lumen.example")],
            day: day.add_days(2),
            span: None,
            all_day: false,
        };
        let (owner, invited) = people_of(&app, &meeting);
        assert_eq!((owner.email.as_str(), invited.len()), ("avery@mail.example", 2), "the owner isn't invited to his own meeting, and nobody twice");
        let wide = json!({"people": [{"email": "maya@lumen.example", "busy": [{"start": 1e300, "end": -5}]}]});
        for junk in [json!(7), json!("x"), json!({"people": 7}), json!({"people": [{"email": 7}]}), json!({"from": "x"}), wide] {
            assert!(stand_in(&mut app, &meeting, &junk).is_err(), "{junk}");
        }
        assert_eq!(stand_in(&mut app, &meeting, &json!({})), Ok(0));
        let from = MIDNIGHT + 2 * 86_400;
        let said =
            json!({"people": [{"email": "maya@lumen.example", "busy": [{"start": from + 3600, "end": from + 7200, "showAs": "outOfOffice"}]}]});
        assert_eq!(stand_in(&mut app, &meeting, &said), Ok(1));
        let look = app.free_busy(calendar, &["maya@lumen.example".to_string()], day.add_days(2));
        assert_eq!((look.pending, look.people[0].1.busy.as_slice()), (false, [(60, 120, ShowAs::OutOfOffice)].as_slice()));
        let nobody = Meeting { required: Vec::new(), optional: Vec::new(), ..meeting };
        assert!(stand_in(&mut app, &nobody, &json!({})).is_err());
    }

    #[test]
    fn a_drag_keeps_the_meeting_within_the_day_and_on_the_half_hours() {
        let moving = Drag { hold: Hold::Move, grabbed: 0.0, start: 600, end: 660 };
        assert_eq!(dragged(&moving, 0), (600, 660));
        assert_eq!(dragged(&moving, 14), (600, 660));
        assert_eq!(dragged(&moving, 16), (630, 690));
        assert_eq!(dragged(&moving, -95), (510, 570));
        // Not out of the day, either way.
        assert_eq!(dragged(&moving, 10_000), (1380, 1440));
        assert_eq!(dragged(&moving, i64::MIN), (0, 60));
        assert_eq!(dragged(&moving, i64::MAX), (1380, 1440));
        // Its start and its end, each by itself; it keeps half an hour at least.
        let start = Drag { hold: Hold::Start, ..moving };
        assert_eq!((dragged(&start, -60), dragged(&start, 40), dragged(&start, 500)), ((540, 660), (630, 660), (630, 660)));
        assert_eq!(dragged(&start, -5_000), (0, 660));
        let end = Drag { hold: Hold::End, ..moving };
        assert_eq!((dragged(&end, 90), dragged(&end, -45), dragged(&end, -500)), ((600, 750), (600, 630), (600, 630)));
        assert_eq!(dragged(&end, 5_000), (600, 1440));
        // A meeting that isn't on the half hour comes to it when it is moved; one with no
        // length, or longer than a day, does no harm.
        assert_eq!(dragged(&Drag { hold: Hold::Move, grabbed: 0.0, start: 605, end: 650 }, 0), (600, 645));
        assert_eq!(dragged(&Drag { hold: Hold::Move, grabbed: 0.0, start: 600, end: 600 }, 30), (630, 630));
        let _ = dragged(&Drag { hold: Hold::Move, grabbed: 0.0, start: -9_000, end: 90_000 }, 7);
        let _ = dragged(&Drag { hold: Hold::End, grabbed: 0.0, start: 5_000, end: 100 }, 7);
        let _ = dragged(&Drag { hold: Hold::Start, grabbed: 0.0, start: i64::MAX, end: i64::MIN }, i64::MAX);
    }

    #[test]
    fn a_servers_answer_becomes_days() {
        let day = Date::new(2026, 10, 16).unwrap();
        // As `calendar.freeBusy` hands it on: times of this computer.
        let answer = json!({"people": [
            {"email": "Maya@Lumen.example", "busy": [
                {"start": "2026-10-16T13:00:00", "end": "2026-10-16T14:30:00", "showAs": "tentative"},
                {"start": "2026-10-16T09:00:00", "end": "2026-10-16T10:00:00", "showAs": "busy"},
                // From the day before into this one, and from this one into the next.
                {"start": "2026-10-15T22:00:00", "end": "2026-10-16T01:00:00", "showAs": "outOfOffice"},
                {"start": "2026-10-16T23:00:00", "end": "2026-10-17T02:00:00", "showAs": "workingElsewhere"},
                // What takes no time, isn't on the day, or doesn't read.
                {"start": "2026-10-16T11:00:00", "end": "2026-10-16T12:00:00", "showAs": "free"},
                {"start": "2026-10-18T11:00:00", "end": "2026-10-18T12:00:00", "showAs": "busy"},
                {"start": "2026-10-16T12:00:00", "end": "2026-10-16T11:00:00", "showAs": "busy"},
                {"start": "noon", "end": 7, "showAs": "busy"},
                {"start": "2026-10-16T15:00:00", "end": "2026-10-16T16:00:00", "showAs": "asleep"},
                null, 7, "x",
            ]},
            {"email": "tomas@lumen.example", "busy": [], "error": "their time couldn't be looked up\n(ErrorMailRecipientNotFound)"},
            {"email": "not an address", "busy": []},
            {"busy": []},
            7,
        ]});
        let days = days_of(&answer, day);
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].0, "maya@lumen.example");
        assert_eq!(
            days[0].1.busy,
            [(0, 60, ShowAs::OutOfOffice), (540, 600, ShowAs::Busy), (780, 870, ShowAs::Tentative), (1380, 1440, ShowAs::WorkingElsewhere)]
        );
        assert_eq!(
            (days[1].0.as_str(), days[1].1.unknown.as_str()),
            ("tomas@lumen.example", "their time couldn't be looked up(ErrorMailRecipientNotFound)")
        );
        // Nothing at all, and nonsense.
        for junk in [json!(null), json!([]), json!({"people": 7}), json!({"people": [[], {}, {"email": 3}]})] {
            assert!(days_of(&junk, day).is_empty(), "{junk}");
        }
        // More people than are ever asked about, each with more stretches than are taken.
        let crowd: Vec<Value> = (0..3_000)
            .map(|i| {
                let busy: Vec<Value> =
                    (0..3_000).map(|_| json!({"start": "2026-10-16T09:00", "end": "2026-10-16T10:00", "showAs": "busy"})).collect();
                json!({"email": format!("p{i}@example.org"), "busy": if i < 3 { busy } else { Vec::new() }})
            })
            .collect();
        let days = days_of(&json!({"people": crowd}), day);
        assert_eq!(days.len(), MAX_SCHEDULES);
        assert!(days.iter().all(|(_, day)| day.busy.len() <= 2_000));
        assert_eq!(address_key("  Maya@Lumen.Example "), Some("maya@lumen.example".into()));
        assert_eq!((address_key("maya"), address_key("")), (None, None));
    }
}
