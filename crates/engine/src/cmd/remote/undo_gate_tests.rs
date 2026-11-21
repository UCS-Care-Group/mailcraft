// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The question asked of an agent's undo or redo (#494, #1674) never says less than the undo or
//! redo does: every event it takes away, or puts back as another, is said to go
//! ([`Session::undo_removes`]), and when one is gone the Delete class is asked for
//! ([`Session::undo_deletes`]). Over a matrix of steps, downloads before and after, a subscribed
//! calendar or none, and the zone: as it was, the mailbox's stamp moved, the computer carried to
//! another zone and settled, or not settled yet (the gate is asked before `Session::run` settles
//! it), and carried back to the step's zone unsettled. Of the subscribed calendars' events, which an
//! undo or redo always keeps, it says exactly what happens, both ways: none is said to go, a
//! calendar subscribed after the step included.

use std::collections::BTreeMap;

use mailcraft_model::Id;
use mailcraft_model::remote::{EventsDownload, RemoteEvent};
use mailcraft_time::{Date, DateTime, Timestamp, Zone};
use serde_json::{Value, json};

use crate::Session;

fn at(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> Timestamp {
    DateTime::new(Date::new(year, month, day).unwrap(), hour, minute, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn remote(id: &str, title: &str, start: Timestamp, minutes: i64, version: u32) -> RemoteEvent {
    RemoteEvent {
        id: id.into(),
        change_key: format!("ck-{id}-{version}"),
        title: title.into(),
        start,
        end: start + minutes * 60,
        ..Default::default()
    }
}

/// All of October 2026 for `account`, complete.
fn october(account: Id, email: &str, events: Vec<RemoteEvent>) -> Value {
    let d = EventsDownload {
        account,
        email: email.into(),
        from: at(2026, 10, 1, 0, 0),
        to: at(2026, 11, 1, 0, 0),
        complete: true,
        events,
        ..Default::default()
    };
    json!({ "download": d })
}

fn budget() -> RemoteEvent {
    remote("e-1", "Budget review", at(2026, 10, 12, 13, 0), 60, 1)
}

fn dentist(version: u32) -> RemoteEvent {
    remote("e-2", "Dentist", at(2026, 10, 13, 18, 0) + i64::from(version - 1) * 3600, 30, version)
}

fn standup() -> RemoteEvent {
    remote("e-3", "Standup", at(2026, 10, 15, 13, 0), 15, 1)
}

fn retro() -> RemoteEvent {
    remote("e-4", "Retro", at(2026, 10, 16, 13, 0), 30, 1)
}

const ADDRESS: &str = "webcal://cal.example.org/club/basic.ics";

fn ics(events: &[(&str, &str, &str)]) -> String {
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\n");
    for (uid, summary, start) in events {
        text.push_str(&format!("BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\nDTSTART:{start}\r\nDTEND:{start}\r\nEND:VEVENT\r\n"));
    }
    text.push_str("END:VCALENDAR\r\n");
    text
}

/// A subscribed calendar's file, come down as a host brings it.
fn came(s: &mut Session, calendar: Id, text: &str, etag: &str) {
    let outcome = json!({"kind": "changed", "text": text, "etag": etag, "lastModified": "Tue, 03 Nov 2026 10:00:00 GMT"});
    run(s, "sync.applySubscription", json!({"calendar": calendar, "outcome": outcome}));
}

#[derive(Clone, Copy, Debug)]
enum Step {
    EditServed,
    NewLocal,
    DeleteLocal,
    DeleteServed,
    AddAccount,
    Subscribe,
    Unrelated,
    UnsubscribeOld,
}

#[derive(Clone, Copy, Debug)]
enum Down {
    None,
    Add,
    Change,
    Delete,
    AddAndChange,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Moved {
    Not,
    /// The mailbox's stamp alone (a file from another machine, say).
    Stamp,
    /// The computer carried to London, and no command or frame since.
    Unsettled,
    Settled,
    /// To London and settled, then back to New York, the step's zone, unsettled.
    BackUnsettled,
}

struct World {
    s: Session,
    account: Id,
    second: Option<Id>,
    sub: Option<Id>,
    /// A calendar subscribed after the step ([`Since`]): the step's mailbox has none such.
    late: Option<Id>,
}

fn world(old_sub: bool) -> World {
    let now = at(2026, 10, 14, 16, 0);
    let mut s = Session::in_zone(crate::sample::mailbox(now, -240), now, Zone::named("America/New_York").unwrap());
    s.settle_zone();
    let added = run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}));
    let account = added["id"].as_u64().unwrap();
    run(&mut s, "sync.applyEvents", october(account, "ada@example.org", vec![budget(), dentist(1)]));
    let sub = old_sub.then(|| {
        let id = run(&mut s, "calendar.subscribe", json!({"address": ADDRESS}))["id"].as_u64().unwrap();
        came(&mut s, id, &ics(&[("u-1", "Match one", "20261015T100000Z")]), "\"1\"");
        id
    });
    s.forget_history();
    World { s, account, second: None, sub, late: None }
}

fn local_event(s: &Session) -> Id {
    s.mailbox.events.values().find(|e| e.origin.is_local() && e.recurrence.is_none() && e.attendees.is_empty()).map(|e| e.id).unwrap()
}

fn called(s: &Session, account: Id, remote: &str) -> Option<Id> {
    s.mailbox.events.values().find(|e| e.origin.account == account && e.origin.id == remote).map(|e| e.id)
}

fn step(w: &mut World, step: Step) {
    let s = &mut w.s;
    match step {
        Step::EditServed => {
            let first = called(s, w.account, "e-1").unwrap();
            run(s, "calendar.edit", json!({"id": first, "title": "Mine"}));
        }
        Step::NewLocal => {
            let calendar = s.mailbox.event(local_event(s)).map(|e| e.calendar).unwrap();
            run(s, "calendar.new", json!({"title": "Lunch here", "start": "2026-10-15T12:00", "calendar": calendar}));
        }
        Step::DeleteLocal => {
            let local = local_event(s);
            run(s, "calendar.delete", json!({"id": local}));
        }
        Step::DeleteServed => {
            let second = called(s, w.account, "e-2").unwrap();
            run(s, "calendar.delete", json!({"id": second}));
        }
        Step::AddAccount => {
            let added = run(s, "account.add", json!({"email": "bob@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}));
            w.second = added["id"].as_u64();
        }
        Step::Subscribe => {
            w.sub = run(s, "calendar.subscribe", json!({"address": "webcal://cal.example.org/other/basic.ics"}))["id"].as_u64();
        }
        Step::Unrelated => {
            let message = *s.mailbox.messages.keys().next().unwrap();
            run(s, "mail.flag", json!({"id": message}));
        }
        Step::UnsubscribeOld => {
            let id = w.sub.unwrap();
            run(s, "calendar.unsubscribe", json!({"id": id}));
        }
    }
}

/// What the downloads bring after the step (`round` 0) or after the undo (`round` 1).
fn after(w: &mut World, down: Down, round: u32) {
    let (account, s) = (w.account, &mut w.s);
    let mut list = vec![budget()];
    match down {
        Down::None => return,
        Down::Add => list.extend([dentist(1), standup()]),
        Down::Change => list.push(dentist(2 + round)),
        Down::Delete => {}
        Down::AddAndChange => list.extend([dentist(2 + round), standup(), retro()]),
    }
    if s.mailbox.account(account).is_some() {
        run(s, "sync.applyEvents", october(account, "ada@example.org", list));
    }
    if let Some(second) = w.second
        && s.mailbox.account(second).is_some()
    {
        run(s, "sync.applyEvents", october(second, "bob@example.org", vec![remote("b-1", "Bob's", at(2026, 10, 15, 9, 0), 30, 1 + round)]));
    }
    if let Some(sub) = w.sub
        && s.mailbox.calendar(sub).is_some()
    {
        let etag = format!("\"{}{round}\"", down as u8);
        came(s, sub, &ics(&[("u-1", "Match one", "20261015T100000Z"), ("u-2", "Match two", "20261016T100000Z")]), &etag);
    }
}

fn zone(w: &mut World, moved: Moved) {
    let s = &mut w.s;
    let (london, new_york) = (Zone::named("Europe/London"), Zone::named("America/New_York"));
    match moved {
        Moved::Not => {}
        Moved::Stamp => s.mailbox.zone = format!("{}, moved", s.mailbox.zone),
        Moved::Unsettled => s.clock.zone = london,
        Moved::Settled => {
            s.clock.zone = london;
            s.settle_zone();
        }
        Moved::BackUnsettled => {
            s.clock.zone = london;
            s.settle_zone();
            s.clock.zone = new_york;
        }
    }
}

/// What is done to the subscriptions between the step and the undo. Subscribing and unsubscribing
/// are no steps of the history (`calendar.subscribe` and `calendar.unsubscribe` are pure), so a
/// calendar subscribed then is one the step's mailbox has not, and the undo keeps it all the same.
#[derive(Clone, Copy, Debug)]
enum Since {
    Nothing,
    /// Another calendar subscribed, and its file come down.
    SubscribedAnother,
    /// The old calendar unsubscribed, and its address subscribed to again, its file come down.
    Resubscribed,
}

fn since(w: &mut World, since: Since) {
    let s = &mut w.s;
    match since {
        Since::Nothing => {}
        Since::SubscribedAnother => {
            let id = run(s, "calendar.subscribe", json!({"address": "webcal://cal.example.org/cup/basic.ics"}))["id"].as_u64().unwrap();
            came(s, id, &ics(&[("c-1", "Cup one", "20261017T100000Z")]), "\"c\"");
            w.late = Some(id);
        }
        Since::Resubscribed => {
            let old = w.sub.unwrap();
            run(s, "calendar.unsubscribe", json!({"id": old}));
            let id = run(s, "calendar.subscribe", json!({"address": ADDRESS}))["id"].as_u64().unwrap();
            came(s, id, &ics(&[("u-1", "Match one", "20261015T100000Z")]), "\"r\"");
            (w.sub, w.late) = (Some(id), Some(id));
        }
    }
}

#[derive(Default)]
struct Tally {
    checked: u32,
    removed: u32,
    pending: u32,
    pending_removed: u32,
}

/// What the gate says against what the command does, one direction: every event that is gone,
/// or another, after the command was said to go, and the Delete class was asked for when one is
/// gone.
fn check(w: &mut World, redo: bool, label: &str, under: &mut Vec<String>, tally: &mut Tally) {
    let s = &mut w.s;
    if (redo && !s.can_redo()) || (!redo && !s.can_undo()) {
        return;
    }
    let pending = s.mailbox.zone != s.zone().id();
    let before: BTreeMap<Id, String> = s.mailbox.events.iter().map(|(id, e)| (*id, e.uid.clone())).collect();
    let said: BTreeMap<Id, bool> = before.keys().map(|id| (*id, s.undo_removes(redo, *id))).collect();
    let deletes = s.undo_deletes(redo);
    run(s, if redo { "edit.redo" } else { "edit.undo" }, json!({}));
    tally.checked += 1;
    tally.pending += u32::from(pending);
    let mut gone_for_good = Vec::new();
    for (id, uid) in &before {
        if s.mailbox.event(*id).map(|e| e.uid.as_str()) == Some(uid.as_str()) {
            continue;
        }
        tally.removed += 1;
        tally.pending_removed += u32::from(pending);
        if s.mailbox.event(*id).is_none() {
            gone_for_good.push(*id);
        }
        if !said.get(id).copied().unwrap_or(false) {
            under.push(format!("undo_removes {label}: event {id} ({uid}) taken away, said to stay"));
        }
    }
    if !gone_for_good.is_empty() && !deletes {
        under.push(format!("undo_deletes {label}: events {gone_for_good:?} gone, Delete not asked"));
    }
}

#[test]
fn the_undo_gate_never_says_less_than_the_undo_does() {
    let steps = [
        Step::EditServed,
        Step::NewLocal,
        Step::DeleteLocal,
        Step::DeleteServed,
        Step::AddAccount,
        Step::Subscribe,
        Step::Unrelated,
        Step::UnsubscribeOld,
    ];
    let downs = [Down::None, Down::Add, Down::Change, Down::Delete, Down::AddAndChange];
    let moves = [Moved::Not, Moved::Stamp, Moved::Unsettled, Moved::Settled, Moved::BackUnsettled];
    let mut under = Vec::new();
    let mut tally = Tally::default();
    for st in steps {
        for old_sub in [false, true] {
            if matches!(st, Step::UnsubscribeOld) && !old_sub {
                continue;
            }
            for d1 in downs {
                for z1 in moves {
                    for d2 in downs {
                        for z2 in [Moved::Not, Moved::Unsettled] {
                            let mut w = world(old_sub);
                            step(&mut w, st);
                            after(&mut w, d1, 0);
                            zone(&mut w, z1);
                            let label = format!("{st:?} subscribed={old_sub} {d1:?} {z1:?}");
                            check(&mut w, false, &format!("undo {label}"), &mut under, &mut tally);
                            // Then another download (and maybe another unsettled move) and the redo.
                            after(&mut w, d2, 1);
                            if z2 == Moved::Unsettled {
                                w.s.clock.zone = Zone::named("Asia/Tokyo");
                            }
                            check(&mut w, true, &format!("redo {label} then {d2:?} {z2:?}"), &mut under, &mut tally);
                        }
                    }
                }
            }
        }
    }
    // The matrix reaches what it is for: removals, with a zone change pending among them.
    assert!(tally.checked >= 4_000 && tally.removed >= 3_000, "checked {}, removed {}", tally.checked, tally.removed);
    assert!(tally.pending >= 1_000 && tally.pending_removed >= 1_000, "pending {}, removed then {}", tally.pending, tally.pending_removed);
    let shown: Vec<&String> = under.iter().take(20).collect();
    assert!(under.is_empty(), "{} under-reports, first: {shown:#?}", under.len());
}

/// What the gate says of the subscribed calendars' events against what the command does with
/// them, both ways: each is said to go exactly when it goes, and as `subscribed_stay` keeps every
/// one as it is now, whatever the step, the downloads and the zone (CodeRabbit on #1710), none is.
/// How many it compared, and how many of those are in the calendar subscribed after the step.
fn check_subscribed(w: &mut World, redo: bool, label: &str, wrong: &mut Vec<String>) -> (u32, u32) {
    let late = w.late;
    let s = &mut w.s;
    if (redo && !s.can_redo()) || (!redo && !s.can_undo()) {
        return (0, 0);
    }
    let subscribed = |s: &Session, calendar: Id| s.mailbox.calendar(calendar).is_some_and(|c| c.is_subscribed());
    let before: BTreeMap<Id, (String, Id)> =
        s.mailbox.events.values().filter(|e| subscribed(s, e.calendar)).map(|e| (e.id, (e.uid.clone(), e.calendar))).collect();
    let said: BTreeMap<Id, bool> = before.keys().map(|id| (*id, s.undo_removes(redo, *id))).collect();
    run(s, if redo { "edit.redo" } else { "edit.undo" }, json!({}));
    let (mut seen, mut of_late) = (0, 0);
    for (id, (uid, calendar)) in &before {
        seen += 1;
        of_late += u32::from(late == Some(*calendar));
        let gone = s.mailbox.event(*id).map(|e| e.uid.as_str()) != Some(uid.as_str());
        if said[id] != gone {
            wrong.push(format!("undo_removes {label}: subscribed event {id} ({uid}) said gone={}, gone={gone}", said[id]));
        }
    }
    (seen, of_late)
}

#[test]
fn a_subscribed_event_a_refresh_brought_is_said_to_stay_through_an_undo_after_a_zone_move() {
    // The step, then the calendar's refresh brings "Match two", then the computer is carried to
    // London: the undo (and the redo after it) keeps "Match two", as every subscribed event,
    // so the gate must not say it goes.
    for z1 in [Moved::Unsettled, Moved::Settled] {
        let mut w = world(true);
        step(&mut w, Step::NewLocal);
        after(&mut w, Down::Add, 0);
        let sub = w.sub.unwrap();
        let two = w.s.mailbox.events.values().find(|e| e.calendar == sub && e.title == "Match two").map(|e| e.id).unwrap();
        zone(&mut w, z1);
        assert!(!w.s.undo_removes(false, two), "{z1:?}: Match two said to go with the undo");
        run(&mut w.s, "edit.undo", json!({}));
        assert!(w.s.mailbox.event(two).is_some_and(|e| e.title == "Match two"), "{z1:?}: the undo took Match two away");
        w.s.clock.zone = Zone::named("Asia/Tokyo");
        assert!(!w.s.undo_removes(true, two), "{z1:?}: Match two said to go with the redo");
        run(&mut w.s, "edit.redo", json!({}));
        assert!(w.s.mailbox.event(two).is_some_and(|e| e.title == "Match two"), "{z1:?}: the redo took Match two away");
    }
}

#[test]
fn a_calendar_subscribed_after_the_step_keeps_its_events_through_the_undo_and_is_said_to() {
    // The step, then a calendar subscribed (no step of its own) and its file come down: the step's
    // mailbox has no such calendar, yet the undo and the redo keep its events as every subscribed
    // calendar's, with the zone moved since or not at all.
    for z1 in [Moved::Not, Moved::Stamp, Moved::Unsettled, Moved::Settled, Moved::BackUnsettled] {
        let mut w = world(false);
        step(&mut w, Step::NewLocal);
        let sub = run(&mut w.s, "calendar.subscribe", json!({"address": ADDRESS}))["id"].as_u64().unwrap();
        came(&mut w.s, sub, &ics(&[("u-1", "Match one", "20261015T100000Z")]), "\"1\"");
        let one = w.s.mailbox.events.values().find(|e| e.calendar == sub).map(|e| e.id).unwrap();
        zone(&mut w, z1);
        assert!(!w.s.undo_removes(false, one), "{z1:?}: Match one said to go with the undo");
        run(&mut w.s, "edit.undo", json!({}));
        assert!(!w.s.mailbox.events.values().any(|e| e.title == "Lunch here"), "{z1:?}: the undo left the step's appointment");
        assert!(w.s.mailbox.event(one).is_some_and(|e| e.uid == "u-1"), "{z1:?}: the undo took Match one away");
        w.s.clock.zone = Zone::named("Asia/Tokyo");
        assert!(!w.s.undo_removes(true, one), "{z1:?}: Match one said to go with the redo");
        run(&mut w.s, "edit.redo", json!({}));
        assert!(w.s.mailbox.events.values().any(|e| e.title == "Lunch here"), "{z1:?}: the redo left out the step's appointment");
        assert!(w.s.mailbox.event(one).is_some_and(|e| e.uid == "u-1"), "{z1:?}: the redo took Match one away");
    }
}

#[test]
fn the_undo_gate_says_exactly_what_happens_to_the_subscribed_calendars_events() {
    // (Subscribing and unsubscribing are no steps of the history: they come between the step and
    // the undo, `Since`.)
    let steps = [Step::EditServed, Step::NewLocal, Step::DeleteLocal, Step::DeleteServed, Step::AddAccount, Step::Unrelated];
    let sinces = [Since::Nothing, Since::SubscribedAnother, Since::Resubscribed];
    let downs = [Down::None, Down::Add, Down::Change, Down::Delete, Down::AddAndChange];
    let moves = [Moved::Not, Moved::Stamp, Moved::Unsettled, Moved::Settled, Moved::BackUnsettled];
    let (mut wrong, mut seen, mut moved, mut late) = (Vec::new(), 0u32, 0u32, 0u32);
    for st in steps {
        for sc in sinces {
            for d1 in downs {
                for z1 in moves {
                    for z2 in [Moved::Not, Moved::Unsettled] {
                        let mut w = world(true);
                        step(&mut w, st);
                        since(&mut w, sc);
                        after(&mut w, d1, 0);
                        zone(&mut w, z1);
                        let label = format!("{st:?} {sc:?} {d1:?} {z1:?}");
                        let (n, n_late) = check_subscribed(&mut w, false, &format!("undo {label}"), &mut wrong);
                        // The undo is the same for both `z2`: counted once, and said once below.
                        if z2 == Moved::Not {
                            (seen, late) = (seen + n, late + n_late);
                            if z1 != Moved::Not {
                                moved += n;
                            }
                        }
                        after(&mut w, d1, 1);
                        if z2 == Moved::Unsettled {
                            w.s.clock.zone = Zone::named("Asia/Tokyo");
                        }
                        let (n, n_late) = check_subscribed(&mut w, true, &format!("redo {label} then {z2:?}"), &mut wrong);
                        (seen, late) = (seen + n, late + n_late);
                    }
                }
            }
        }
    }
    // The matrix reaches what it is for, each check counted once: subscribed events, with the zone
    // moved among them, and in a calendar subscribed after the step.
    assert!(seen >= 2_500 && moved >= 700 && late >= 1_000, "seen {seen}, moved {moved}, late {late}");
    wrong.sort();
    wrong.dedup();
    let shown: Vec<&String> = wrong.iter().take(20).collect();
    assert!(wrong.is_empty(), "{} wrong, first: {shown:#?}", wrong.len());
}
