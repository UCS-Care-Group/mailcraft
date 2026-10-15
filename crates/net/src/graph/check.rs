//! A check of the calendar connection, for the owner to run on the real account.
//!
//! [`check_calendar`] makes one appointment called "MailCraft connection check" a year from
//! today in the account's main calendar, reads it back, changes its time, reads it back again,
//! changes it once more with the time said in the owner's own time zone (as a whole-day event
//! and a series are said), reads that back too, and deletes it. Each step comes with what
//! Graph answered, and every reading is held against what was sent: the start and the end,
//! to the second. It is the one way to see that what is written here arrives there as it was
//! meant, and it touches nothing else: no other event is read, changed or deleted, nobody is
//! invited, and no reminder is set.
//!
//! Whatever was made is deleted again, also when a step in between went wrong. What Graph says
//! when it refuses (its status, the short name of the refusal and its words) is what the owner
//! needs here, so it is kept: one line, cut to size, and never with anything of the token in it.

use mailcraft_time::{DAY, DateTime, Timestamp};
use ring::rand::SecureRandom as _;
use serde_json::{Value, json};

use super::{ASK_TIMEOUT, EVENT_PREFERENCES, EVENTS, Graph, MAX_ECHO, MAX_WRITTEN, WRITE_TIMEOUT, date_time, text, when, zone_name};
use crate::{NetError, http};

/// What the appointment of the check is called.
pub const CHECK_TITLE: &str = "MailCraft connection check";
/// How far ahead of today it is made (days), and how long it is (seconds).
const AHEAD_DAYS: i64 = 365;
const LENGTH: i64 = 1_800;
/// How far its time is changed each time (seconds).
const MOVED_BY: i64 = 3_600;
/// What the steps are called.
pub const STEPS: [&str; 7] = [
    "Make an appointment",
    "Read it back",
    "Change its time",
    "Read it back again",
    "Change its time once more, said in your time zone",
    "Read it back a third time",
    "Delete it",
];
/// Longest text kept of what Graph said to a step.
const MAX_SAID: usize = 300;

/// One step of [`check_calendar`], and what Graph answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckStep {
    /// What was done ([`STEPS`]).
    pub what: &'static str,
    /// Graph did it, and what came back was what was sent.
    pub ok: bool,
    /// The status of Graph's answer; 0 when there was none (it couldn't be reached).
    pub status: u16,
    /// What came of it, in a line: for a reading, what was sent and what the server has; for a
    /// step Graph refused, its own name for the refusal and its words.
    pub said: String,
}

/// What [`check_calendar`] came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarCheck {
    /// The steps that were taken, in order. One that could not be taken (nothing was made, so
    /// there is nothing to read; no time zone is known to say a time in) is not among them.
    pub steps: Vec<CheckStep>,
    /// Graph didn't take the sign-in: a new one is needed.
    pub token_refused: bool,
    /// When the appointment was made for (Unix seconds).
    pub start: Timestamp,
    /// What was made is gone again (or nothing was made).
    pub cleaned_up: bool,
}

impl CalendarCheck {
    /// Everything was made, read, changed and deleted, and every reading was what was sent.
    pub fn ok(&self) -> bool {
        self.cleaned_up && self.steps.len() >= 5 && self.steps.iter().all(|step| step.ok)
    }
}

/// When a check started at `now` makes its appointment for: noon UTC, a year on.
pub fn check_start(now: Timestamp) -> Timestamp {
    now.div_euclid(DAY).saturating_add(AHEAD_DAYS).saturating_mul(DAY).saturating_add(DAY / 2)
}

/// Check the calendar connection of the owner of `token` (an access token for Graph), as of
/// `now` (Unix seconds). `zone` is the owner's time zone by a name Graph knows, and `offset_at`
/// how far east of UTC it is at an instant, in minutes, as this program counts it: the step
/// that says a time in that zone shows whether Graph counts the same. With no zone that step is
/// left out. Blocks for as long as seven questions take.
pub fn check_calendar(token: &str, now: Timestamp, zone: &str, offset_at: &dyn Fn(Timestamp) -> i32) -> CalendarCheck {
    check_calendar_at(&super::graph_base(), token, now, zone, offset_at)
}

/// [`check_calendar`] of the calendar of a shared mailbox the owner of `token` opens (`mailbox`,
/// its address; empty for the owner's own): every step speaks of `/users/{address}/…` in the
/// place of `/me/…`, which takes the permission `Calendars.ReadWrite.Shared` and Full Access to
/// the mailbox. A refusal of either is a 403 in the first step, with Graph's words.
pub fn check_calendar_of(token: &str, mailbox: &str, now: Timestamp, zone: &str, offset_at: &dyn Fn(Timestamp) -> i32) -> CalendarCheck {
    check_calendar_for(&super::graph_base(), token, mailbox, now, zone, offset_at)
}

/// A mark for the appointment (`transactionId`), so that asking twice makes it once.
fn mark(now: Timestamp) -> String {
    let mut bytes = [0u8; 16];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        // No random numbers to be had: the clock will do for a mark.
        for (slot, byte) in bytes.iter_mut().zip(now.to_le_bytes().iter().cycle()) {
            *slot = *byte;
        }
    }
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let part = |from: usize, to: usize| hex.get(from..to).unwrap_or("0");
    format!("{}-{}-{}-{}-{}", part(0, 8), part(8, 12), part(12, 16), part(16, 20), part(20, 32))
}

/// What Graph says in a refusal: its short name for it and its words, on one line. Nothing of
/// `secret` (the token) comes back in it: words that repeat a run of it are left out.
pub(super) fn refusal_words(body: &[u8], secret: &str) -> String {
    let json = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let error = json.get("error").unwrap_or(&Value::Null);
    let clean = |words: &str| {
        let line = http::tidy(words);
        let short: String = line.chars().take(MAX_SAID).collect();
        let echoes =
            short.as_bytes().windows(MAX_ECHO).any(|run| std::str::from_utf8(run).is_ok_and(|run| !run.trim().is_empty() && secret.contains(run)));
        if echoes { String::new() } else { short }
    };
    let (code, message) = (clean(text(error, "code")), clean(text(error, "message")));
    match (code.is_empty(), message.is_empty()) {
        (false, false) => format!("{code}: {message}"),
        (false, true) => code,
        (true, false) => message,
        (true, true) => String::new(),
    }
}

/// [`check_calendar`] against Graph at `base`.
pub(super) fn check_calendar_at(base: &str, token: &str, now: Timestamp, zone: &str, offset_at: &dyn Fn(Timestamp) -> i32) -> CalendarCheck {
    check_calendar_for(base, token, "", now, zone, offset_at)
}

/// [`check_calendar_of`] against Graph at `base`.
pub(super) fn check_calendar_for(
    base: &str,
    token: &str,
    mailbox: &str,
    now: Timestamp,
    zone: &str,
    offset_at: &dyn Fn(Timestamp) -> i32,
) -> CalendarCheck {
    let start = check_start(now);
    let mut check = CalendarCheck { start, cleaned_up: true, ..Default::default() };
    let step = |what: usize, ok: bool, status: u16, said: String| CheckStep { what: STEPS.get(what).copied().unwrap_or(""), ok, status, said };
    // (An address that can't be a mailbox's is refused here, and nothing is asked.)
    let graph = match Graph::at(base, token).and_then(|graph| graph.for_mailbox(mailbox)) {
        Ok(graph) => graph,
        Err(e) => {
            check.token_refused = e.is_refused_sign_in();
            check.steps.push(step(0, false, 0, e.to_string()));
            return check;
        }
    };
    // One question, and what came of it: the status, the answer as JSON, and Graph's words
    // when it said no (or why it couldn't be asked).
    let ask = |method: &str, path: &str, body: Option<String>| -> (u16, Value, String) {
        let json = [("Content-Type", "application/json"), ("Prefer", EVENT_PREFERENCES)];
        let plain = [("Prefer", EVENT_PREFERENCES)];
        let headers: &[(&str, &str)] = match (method, body.is_some()) {
            (_, true) => &json,
            ("GET", false) => &plain,
            _ => &[],
        };
        let patience = if method == "GET" { ASK_TIMEOUT } else { WRITE_TIMEOUT };
        match graph.ask(&graph.url(path), method, headers, body.as_deref(), patience, MAX_WRITTEN) {
            Ok(reply) => {
                let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                let said = if (200..300).contains(&reply.status) { String::new() } else { refusal_words(&reply.body, token) };
                (reply.status, json, said)
            }
            Err(e) => (0, Value::Null, e.to_string()),
        }
    };
    let at = |instant: Timestamp| format!("{} UTC", DateTime::from_timestamp(instant, 0).iso().replace('T', " "));
    let span = |from: Timestamp| format!("{} to {}", at(from), at(from.saturating_add(LENGTH)));
    let times =
        |from: Timestamp| json!({"start": date_time(from, false, ""), "end": date_time(from.saturating_add(LENGTH), false, ""), "isAllDay": false});

    // 1. Make it: free time, no reminder, nobody invited.
    let mut made = times(start);
    if let Some(made) = made.as_object_mut() {
        made.insert("subject".into(), json!(CHECK_TITLE));
        made.insert(
            "body".into(),
            json!({"contentType": "text", "content": "Made by MailCraft to check that changes reach your calendar. It deletes itself."}),
        );
        made.insert("showAs".into(), json!("free"));
        made.insert("isReminderOn".into(), json!(false));
        made.insert("transactionId".into(), json!(mark(now)));
    }
    let (status, answer, said) = ask("POST", EVENTS, Some(made.to_string()));
    let id = mailcraft_model::remote::clean_key(text(&answer, "id"));
    check.token_refused |= status == 401;
    match ((200..300).contains(&status), id.is_empty()) {
        (true, false) => check.steps.push(step(0, true, status, format!("sent: {}", span(start)))),
        (true, true) => {
            let said = "the server said it made it, but not under what name: it can't be read back or deleted from here".to_string();
            check.steps.push(step(0, false, status, said));
            check.cleaned_up = false;
            return check;
        }
        (false, _) => {
            check.steps.push(step(0, false, status, said));
            return check;
        }
    }
    check.cleaned_up = false;
    let event = format!("{EVENTS}/{}", http::encode(&id));
    let select = http::form(&[("$select", "id,subject,start,end,isAllDay")]);
    // What the server has now, held against what was sent: the start and the end, exactly.
    let read = |n: usize, expected: Timestamp| -> CheckStep {
        let (status, answer, said) = ask("GET", &format!("{event}?{select}"), None);
        if status != 200 {
            return step(n, false, status, said);
        }
        let (starts, ends) = (when(answer.get("start")), when(answer.get("end")));
        let whole_day = answer.get("isAllDay").and_then(Value::as_bool).unwrap_or(false);
        match (starts, ends) {
            (Some(starts), Some(ends)) if text(&answer, "subject") == CHECK_TITLE => {
                let (same_start, same_end) = (starts == expected, ends == expected.saturating_add(LENGTH));
                let has = format!("{} to {}", at(starts), at(ends));
                match (same_start && same_end && !whole_day, whole_day) {
                    (true, _) => step(n, true, status, format!("the server has: {has}, as it was sent")),
                    (false, true) => step(n, false, status, format!("sent: {}; the server has it as a whole-day event: {has}", span(expected))),
                    (false, false) => {
                        let which = match (same_start, same_end) {
                            (false, false) => "the start and the end differ",
                            (false, true) => "the start differs",
                            _ => "the end differs",
                        };
                        step(n, false, status, format!("sent: {}; the server has: {has} ({which})", span(expected)))
                    }
                }
            }
            _ => {
                step(n, false, status, "what the server sent back is not the appointment that was made, or has no times that can be read".to_string())
            }
        }
    };
    // 2. Read it back.
    check.steps.push(read(1, start));
    // 3. Change its time, said as an instant (in UTC), as the time of an appointment is said.
    let later = start.saturating_add(MOVED_BY);
    let (status, _, said) = ask("PATCH", &event, Some(times(later).to_string()));
    check.token_refused |= status == 401;
    let moved = (200..300).contains(&status);
    check.steps.push(step(2, moved, status, if moved { format!("sent: {}", span(later)) } else { said }));
    // 4. Read it back again: at its new time when the change went through.
    check.steps.push(read(3, later));
    // 5 and 6. Once more, with the time said by the clock in the owner's zone, as the days of a
    // whole-day event and the hours of a series are said: does Graph count that zone as this
    // program does?
    let zone = zone_name(zone);
    if !zone.is_empty() {
        let last = later.saturating_add(MOVED_BY);
        let wall = |instant: Timestamp| DateTime::from_timestamp(instant, offset_at(instant).clamp(-18 * 60, 18 * 60)).iso();
        let body = json!({
            "start": {"dateTime": wall(last), "timeZone": zone},
            "end": {"dateTime": wall(last.saturating_add(LENGTH)), "timeZone": zone},
            "isAllDay": false,
        });
        let (status, _, said) = ask("PATCH", &event, Some(body.to_string()));
        check.token_refused |= status == 401;
        let moved = (200..300).contains(&status);
        let sent = format!(
            "sent: {} to {} in the zone \"{zone}\", which is {} here",
            wall(last).replace('T', " "),
            wall(last.saturating_add(LENGTH)).replace('T', " "),
            span(last)
        );
        check.steps.push(step(4, moved, status, if moved { sent } else { said }));
        check.steps.push(read(5, last));
    }
    // 7. Delete it, whatever became of the steps before.
    let (status, _, said) = ask("DELETE", &event, None);
    check.token_refused |= status == 401;
    check.cleaned_up = (200..300).contains(&status);
    let gone = if check.cleaned_up { "deleted".to_string() } else { said };
    check.steps.push(step(6, check.cleaned_up, status, gone));
    check
}

/// The error of a check that couldn't start, for a host that has no token to run it with.
pub fn no_sign_in(error: &NetError) -> CalendarCheck {
    CalendarCheck {
        steps: vec![CheckStep { what: STEPS.first().copied().unwrap_or(""), ok: false, status: 0, said: error.to_string() }],
        token_refused: error.is_refused_sign_in(),
        start: 0,
        cleaned_up: true,
    }
}
