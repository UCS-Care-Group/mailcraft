// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashSet;

use mailcraft_i18n::Lang;
use mailcraft_model::remote::{BusyTime, FreeBusy, PersonSchedule};
use mailcraft_model::{Id, ShowAs};
use mailcraft_time::Date;
use serde_json::json;

use super::free_busy::*;
use super::{Busy, Start};
use crate::Session;

/// Wednesday 2026-10-14 16:00 UTC, and the midnight that day began with.
const NOW: i64 = 1_791_993_600;
const MIDNIGHT: i64 = NOW - 16 * 3600;

/// A session with a Microsoft account that keeps the calendar.
fn session() -> (Session, Id, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let calendar = s.mailbox.calendars[0].id;
    s.mailbox.calendars[0].origin.account = account;
    (s, account, calendar)
}

fn facts(in_flight: Option<&Key>) -> FreeBusyFacts<'_> {
    FreeBusyFacts { reachable: true, busy: Busy::default(), in_flight, lang: Lang::English }
}

fn day() -> Date {
    Date::new(2026, 10, 14).unwrap()
}

fn people() -> Vec<String> {
    vec!["Maya@Lumen.example".into(), "tomas@lumen.example".into(), "maya@lumen.example".into(), "nobody".into()]
}

#[test]
fn a_look_asks_once_and_not_again_before_its_time() {
    let (mut s, account, calendar) = session();
    let (look, ask) = s.free_busy_look(calendar, &people(), day(), &facts(None), 0.0);
    assert_eq!((look.pending, look.people.len(), look.note.as_str()), (false, 0, ""));
    let ask = ask.expect("due at first");
    assert_eq!((ask.account.id, ask.key.account), (account, account));
    assert_eq!(ask.key.people, vec!["maya@lumen.example".to_string(), "tomas@lumen.example".to_string()], "lower case, each once, only addresses");
    assert!(ask.ask.is_ok());
    // The host started it: pending while it is in flight, and not asked again.
    s.free_busy_noted(ask.key.clone(), 0.0, String::new());
    let (look, again) = s.free_busy_look(calendar, &people(), day(), &facts(Some(&ask.key)), 1_000.0);
    assert!(look.pending && again.is_none());
    let (_, again) = s.free_busy_look(calendar, &people(), day(), &facts(None), REFRESH_MS - 1.0);
    assert!(again.is_none());
    let (_, again) = s.free_busy_look(calendar, &people(), day(), &facts(None), REFRESH_MS);
    assert!(again.is_some(), "an answer is good for five minutes");
    // Another day is another question.
    assert!(s.free_busy_look(calendar, &people(), day().add_days(1), &facts(None), 1.0).1.is_some());
}

#[test]
fn a_question_that_came_to_nothing_is_tried_again_after_half_a_minute() {
    let (mut s, _, calendar) = session();
    let (_, ask) = s.free_busy_look(calendar, &people(), day(), &facts(None), 0.0);
    let key = ask.unwrap().key;
    s.free_busy_noted(key.clone(), 0.0, "no token".into());
    let (look, again) = s.free_busy_look(calendar, &people(), day(), &facts(None), RETRY_MS - 1.0);
    assert!(again.is_none() && look.note == "no token");
    assert!(s.free_busy_look(calendar, &people(), day(), &facts(None), RETRY_MS).1.is_some());
}

#[test]
fn nothing_is_asked_while_something_goes_first_and_nobody_with_an_address_is_nobody() {
    let (mut s, _, calendar) = session();
    for busy in [
        Busy { free_busy: true, ..Default::default() },
        Busy { signing_in: true, ..Default::default() },
        Busy { graph: true, ..Default::default() },
        Busy { calendar: true, ..Default::default() },
    ] {
        assert!(busy.blocks(Start::FreeBusy));
        let facts = FreeBusyFacts { busy, ..facts(None) };
        assert!(s.free_busy_look(calendar, &people(), day(), &facts, 0.0).1.is_none(), "{busy:?}");
    }
    let (look, ask) = s.free_busy_look(calendar, &["nobody".into(), String::new()], day(), &facts(None), 0.0);
    assert_eq!(look, Look::default());
    assert!(ask.is_none());
    let crowd: Vec<String> = (0..400).map(|n| format!("p{n}@lumen.example")).collect();
    let (_, ask) = s.free_busy_look(calendar, &crowd, day(), &facts(None), 0.0);
    assert_eq!(ask.unwrap().key.people.len(), mailcraft_model::remote::MAX_SCHEDULES, "no more than one question carries");
}

#[test]
fn without_a_server_to_ask_the_page_says_why() {
    let (mut s, account, calendar) = session();
    let words = |s: &mut Session, facts: &FreeBusyFacts<'_>, cal: Id| s.free_busy_look(cal, &people(), day(), facts, 0.0);
    // A calendar kept on this computer.
    s.mailbox.calendars[0].origin.account = 0;
    let (look, ask) = words(&mut s, &facts(None), calendar);
    assert!(look.note.contains("kept on this computer") && ask.is_none());
    s.mailbox.calendars[0].origin.account = account;
    let (look, _) = words(&mut s, &FreeBusyFacts { reachable: false, ..facts(None) }, calendar);
    assert!(look.note.contains("can't ask a server"));
    s.prefs.offline = true;
    assert!(words(&mut s, &facts(None), calendar).0.note.contains("working offline"));
    s.prefs.offline = false;
    s.driver.calendar.consent_needed.insert(account);
    assert!(words(&mut s, &facts(None), calendar).0.note.contains("needs your permission"));
}

#[test]
fn an_answer_is_taken_in_and_a_failure_keeps_what_was_known() {
    let (mut s, account, calendar) = session();
    let (_, ask) = s.free_busy_look(calendar, &people(), day(), &facts(None), 0.0);
    let key = ask.unwrap().key;
    s.free_busy_noted(key.clone(), 0.0, String::new());
    let at = |hour: i64| MIDNIGHT + hour * 3600;
    let said = FreeBusy {
        from: at(0),
        to: at(24),
        people: vec![PersonSchedule {
            email: "maya@lumen.example".into(),
            busy: vec![BusyTime { start: at(10), end: at(12), show_as: ShowAs::Busy }],
            ..Default::default()
        }],
    };
    s.free_busy_answered(&key, 5.0, &FreeBusyCame { schedule: Some(&said), ..Default::default() }, Lang::English);
    let known = s.driver.free_busy.known_for(&key).unwrap();
    assert!((known.asked - 0.0).abs() < f64::EPSILON && known.error.is_empty());
    assert_eq!(known.people.iter().find(|(who, _)| who == "maya@lumen.example").map(|(_, d)| d.busy.len()), Some(1));
    // A later failure: the error is said, what was known stays.
    s.free_busy_answered(&key, 6.0, &FreeBusyCame { error: "  ", ..Default::default() }, Lang::English);
    let known = s.driver.free_busy.known_for(&key).unwrap();
    assert!(known.error.contains("didn't say") && !known.people.is_empty());
    // A sign-in that doesn't cover the calendar is noted for the owner.
    s.free_busy_answered(&key, 7.0, &FreeBusyCame { consent_needed: true, error: "x", ..Default::default() }, Lang::English);
    assert!(s.driver.calendar.consent_needed.contains(&account));
}

#[test]
fn an_answer_for_an_account_removed_meanwhile_changes_nothing() {
    let (mut s, account, calendar) = session();
    let (_, ask) = s.free_busy_look(calendar, &people(), day(), &facts(None), 0.0);
    let key = ask.unwrap().key;
    s.free_busy_noted(key.clone(), 0.0, String::new());
    s.run("account.remove", &json!({"id": account})).unwrap();
    assert!(s.driver.free_busy.known.is_empty(), "what was found out goes with the account");
    s.free_busy_answered(&key, 1.0, &FreeBusyCame { consent_needed: true, error: "x", ..Default::default() }, Lang::English);
    assert!(s.driver.free_busy.known.is_empty() && !s.driver.calendar.consent_needed.contains(&account));
}

#[test]
fn only_a_few_days_are_remembered() {
    let mut driver = FreeBusyDriver::default();
    for n in 0..20u32 {
        let key = Key { account: 1, day: day().add_days(i64::from(n)), people: vec!["a@b.example".into()] };
        driver.note(key, f64::from(n), None, String::new());
    }
    assert_eq!(driver.known.len(), MAX_KNOWN);
    assert_eq!(driver.known.last().map(|k| k.asked), Some(19.0));
    driver.forget_accounts(&HashSet::from([1]));
    assert!(driver.known.is_empty());
}

#[test]
fn a_stand_in_answer_is_taken_in_like_the_hosts() {
    let (mut s, _, calendar) = session();
    let said = json!({"from": 0, "to": 86_400, "people": [{"email": "maya@lumen.example", "busy": []}]});
    let (key, n) = s.free_busy_stand_in(calendar, &people(), day(), &said, &facts(None), 3.0).unwrap();
    assert_eq!(n, 1);
    assert!(s.driver.free_busy.known_for(&key).is_some());
    assert!(s.free_busy_stand_in(calendar, &["nobody".into()], day(), &said, &facts(None), 3.0).unwrap_err().contains("Nobody"));
    assert!(s.free_busy_stand_in(calendar, &people(), day(), &json!(5), &facts(None), 3.0).is_err());
}
