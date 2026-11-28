// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Two things the owner's own calendar questions must mean on a host whose calendar has no named
//! clock (`Session::zone()` is `Zone::LOCAL`: the browser build, and any host that gives offsets
//! only through `Session::fallback_offset`):
//!
//! 1. A whole-series legacy Delete, Edit or Move resolves no day at all, so an anonymous clock
//!    must not refuse its question: `Shown::new` must return `Some`, the untouched
//!    `Shown::check` must be `Ok`, and the same payload must be a valid operation.
//! 2. A one-occurrence question whose *displayed* day stays equal while the host's offset rules
//!    for that anonymous clock move must be refused, because the same raw day then resolves
//!    through `series_day_on` to a different intrinsic occurrence.
//!
//! Why these exist, in the source they were written against (2026-11-15):
//! - `legacy_state` returns `None` for **every** recurring event while `s.zone().is_local()`
//!   (`crates/engine/src/agent/bind.rs:495-498`), before it reads `scope`/`occurrence`. `Shown::new`
//!   turns that into `None` (`bind.rs:262-264`), and the owner's own dialog
//!   (`crates/ui-egui/src/dialogs.rs:1148-1169`, `lib.rs:2263-2266`) reads `None` as "the question
//!   can't be held" and refuses the command *before* the question is drawn. But a whole-series
//!   question resolves no day: `part()` (`crates/engine/src/cmd/calendar.rs:155-171`) returns
//!   `Part::Series` before `series_day_on` is ever called, and `legacy_state` keeps `one` false.
//! - `legacy_state` hashes the raw `occurrence` day (`bind.rs:501-512`, `:523`) and the zone's
//!   identity (`:525`), but not the offset an anonymous clock has *now* (`zone.id()` is empty and
//!   `clock.offset` is `None` for `Zone::LOCAL`), and not the day `part()` resolves at run time:
//!   `part()` maps the raw day through `series_day_on(day, &remote::offset_of(s))`
//!   (`cmd/calendar.rs:170`), and `offset_of` clones `s.fallback_offset` *at call time*
//!   (`cmd/remote.rs:3494-3506`). So a host that changes the offset rules under an anonymous clock
//!   keeps the digest equal while the same yes addresses another intrinsic occurrence.
//!
//! Everything is held through public API only: `Session::in_zone` (session.rs:1420-1422), the public
//! `Session::fallback_offset` field (session.rs:1114-1117), `agent::describe`, and
//! `agent::bind::Shown::{new,check}`. No stub, no invented API, no host zone or environment is
//! touched, and no mailbox file, server or wall clock is used (`example.org`, `Mailbox::add_event`).

use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use mailcraft_engine::model::Mailbox;
use mailcraft_engine::time::Zone;

/// A local daily series at 23:30 by `Africa/Abidjan`'s clock, in a mailbox whose calendar names
/// no zone (what the browser and an offset-only host have until the owner picks a display zone).
/// Its own zone is what a one-occurrence question needs a clock for; a whole-series question
/// resolves no day at all.
fn anonymous_series() -> (Mailbox, Id) {
    let mut mailbox = Mailbox::with_local_account("Owner Example", "owner@example.org");
    let calendar = mailbox.default_calendar().expect("fixture: the local account has a calendar");
    let start = at(d(2026, 1, 5), 23, 30);
    let id = mailbox
        .add_event(Event {
            title: "Night shift".into(),
            start,
            end: start.add_minutes(60),
            calendar,
            start_zone: "Africa/Abidjan".into(),
            recurrence: Some(Recurrence::every(Freq::Daily)),
            ..Event::default()
        })
        .expect("fixture: the local calendar takes the series");
    (mailbox, id)
}

/// `command` about the whole series `id`, as the owner's own dialog asks it: `id` and the fields,
/// and no `scope` and no `occurrence`, so no day of the series is named.
fn whole_series(command: &str, id: Id) -> Value {
    match command {
        "calendar.delete" => json!({ "id": id }),
        "calendar.edit" => json!({ "id": id, "title": "Night shift (whole series)" }),
        _ => json!({ "id": id, "start": "2026-01-06T09:00" }),
    }
}

/// One command, one independent case: the payload is a real operation, the question about the
/// whole series is captured and answerable while the clock has no name, and the same is true of
/// the explicit `"scope": "series"` the owner's "The Entire Series" answer adds.
fn whole_series_is_bindable(command: &str) {
    let (mailbox, id) = anonymous_series();
    let session = Session::in_zone(mailbox, NOW, Zone::LOCAL);
    assert_eq!(session.zone(), Zone::LOCAL, "fixture: the calendar is shown by the system's offset");
    assert!(session.zone().id().is_empty(), "fixture: an anonymous clock has no identifier");
    assert_eq!(session.system_zone(), Zone::LOCAL, "fixture: the named-zone constructor uses the anonymous system clock, not a fixed override");
    assert!(session.prefs.time_zone.is_empty(), "fixture: the options name no display zone");
    assert!(session.mailbox.event(id).is_some_and(|e| e.recurrence.is_some()), "fixture: the event is a series");

    // The operation is real: the same payload runs on a copy, and is the whole-series change.
    let params = whole_series(command, id);
    let mut probe = Session::in_zone(session.mailbox.clone(), NOW, Zone::LOCAL);
    assert!(probe.run(command, &params).is_ok(), "fixture: `{command}` on a whole series is a valid operation");
    match command {
        "calendar.delete" => assert!(probe.mailbox.event(id).is_none(), "fixture: the whole series really went"),
        "calendar.edit" => {
            let kept = probe.mailbox.event(id).cloned().expect("fixture: an edit keeps the series");
            assert_eq!(kept.title, "Night shift (whole series)", "fixture: the whole series really changed");
            assert!(kept.recurrence.is_some(), "fixture: and is still a series");
        }
        _ => {
            let kept = probe.mailbox.event(id).cloned().expect("fixture: a move keeps the series");
            assert_eq!(kept.start.date, d(2026, 1, 6), "fixture: the whole series really moved");
            assert!(kept.recurrence.is_some(), "fixture: and is still a series");
        }
    }

    // The owner's own question: captured, and answerable while it is untouched.
    let what = agent::describe(&session, command, &params);
    let shown = agent::bind::Shown::new(&session, command, &params, &what)
        .expect("a whole-series question resolves no day, so an anonymous clock must not refuse it");
    assert!(shown.check(&session, command, &params).is_ok(), "an untouched whole-series question is answerable");

    // The same, with the explicit scope the owner's "The Entire Series" answer adds.
    let mut named = params.clone();
    named["scope"] = json!("series");
    let what = agent::describe(&session, command, &named);
    let shown = agent::bind::Shown::new(&session, command, &named, &what)
        .expect("an explicit whole-series scope resolves no day either, so it must not be refused");
    assert!(shown.check(&session, command, &named).is_ok(), "an untouched explicit whole-series question is answerable");
}

/// A whole-series Delete on an anonymous clock is still a question the owner can be asked.
#[test]
fn a_whole_series_delete_on_an_anonymous_clock_is_still_a_bindable_question() {
    whole_series_is_bindable("calendar.delete");
}

/// A whole-series Edit on an anonymous clock is still a question the owner can be asked.
#[test]
fn a_whole_series_edit_on_an_anonymous_clock_is_still_a_bindable_question() {
    whole_series_is_bindable("calendar.edit");
}

/// A whole-series Move on an anonymous clock is still a question the owner can be asked.
#[test]
fn a_whole_series_move_on_an_anonymous_clock_is_still_a_bindable_question() {
    whole_series_is_bindable("calendar.move");
}

/// The same occurrence question, an anonymous clock whose offset rules move while the owner is
/// being asked: the event and the question's words do not change, but the day the yes resolves to
/// does, so the yes must be refused rather than retargeted.
///
/// The clock is `Session::in_zone(mailbox, NOW, Zone::LOCAL)`; its offset comes from the public
/// `fallback_offset` the host sets. The host's rules are a step — no offset until the first day of
/// March, an hour east after it — which is what a host that learns its zone's summer rules looks
/// like from here. Nothing of the session's identity changes when it is armed: `zone().id()` stays
/// empty, `clock.offset` stays `None`, and no event byte is written.
#[test]
fn an_anonymous_clock_whose_offset_rules_move_does_not_retarget_an_occurrence_yes() {
    // Minutes east of UTC the host reports once its rules have moved.
    const MOVED: i32 = 60;

    let (mailbox, id) = anonymous_series();
    let mut session = Session::in_zone(mailbox, NOW, Zone::LOCAL);
    assert_eq!(session.zone(), Zone::LOCAL, "fixture: the calendar is shown by the system's offset");
    assert!(session.zone().id().is_empty(), "fixture: an anonymous clock has no identifier");
    assert_eq!(session.system_zone(), Zone::LOCAL, "fixture: the named-zone constructor uses the anonymous system clock, not a fixed override");

    // The instant the host's rules move at: the first day of March, well after the series starts,
    // so the series' anchor (and every event byte) reads the same before and after.
    let moved_at = at(d(2026, 3, 1), 0, 0).to_timestamp(0);
    let moved = Arc::new(AtomicI32::new(MOVED));
    let armed = Arc::new(AtomicBool::new(false));
    let provider: Arc<dyn Fn(i64) -> i32 + Send + Sync> = {
        let (moved, armed) = (Arc::clone(&moved), Arc::clone(&armed));
        Arc::new(move |instant| if armed.load(Ordering::SeqCst) && instant >= moved_at { moved.load(Ordering::SeqCst) } else { 0 })
    };
    session.fallback_offset = Some(provider);

    // The rules as they are before and after the host moves them. Both are read off the fixture
    // and then tied back to `Session::offset_at`, so neither is a guess about the session.
    let before = |_instant: i64| 0;
    let after = |instant: i64| if instant >= moved_at { MOVED } else { 0 };

    let event = session.mailbox.event(id).cloned().expect("fixture: the series is there");
    let occurrences = event.occurrences_in(d(2026, 1, 1), d(2026, 12, 31), &before);
    let moved_occurrences = event.occurrences_in(d(2026, 1, 1), d(2026, 12, 31), &after);
    // An occurrence whose displayed day no longer names itself once the host's rules have moved:
    // a real one, its day and the day the same raw day then resolves to all from the iterator.
    let chosen = occurrences.iter().find_map(|o| {
        let under_moved = event.series_day_on(o.start.date, &after)?;
        let instant = event.occurrence_instant(o.day, &before);
        (under_moved != o.day && after(instant) != before(instant)).then_some((o.day, o.start.date, under_moved, instant))
    });
    let (day_shown, raw, day_moved, instant) = chosen.expect("fixture: the host's two rules disagree about some occurrence of the year");

    // Guards: the raw day is what the owner was shown; under the moved rules it selects another
    // intrinsic day; that other day is really carried on the same displayed date; the clocks
    // differ at the occurrence itself; and the anchor (the first occurrence) reads the same.
    assert_eq!(event.series_day_on(raw, &before), Some(day_shown), "fixture: under the first rules the raw day is the occurrence it was shown as");
    assert_ne!(day_moved, day_shown, "fixture: under the moved rules the same raw day must select another intrinsic day");
    assert!(
        moved_occurrences.iter().any(|o| o.start.date == raw && o.day == day_moved),
        "fixture: the other intrinsic day is carried on the same displayed date by the iterator"
    );
    assert_eq!(session.offset_at(instant), before(instant), "fixture: the effective offset is the host's first one, through the session");
    let anchor = event.occurrence_instant(event.start.date, &before);
    assert_eq!(after(anchor), before(anchor), "fixture: the host's rules move after the series' anchor, so no event byte reads differently");

    // The question the owner is shown: the occurrence the series is displayed on, by its raw day.
    let params = json!({ "id": id, "scope": "occurrence", "occurrence": raw.iso(), "title": "Night shift (one night)" });
    let what = agent::describe(&session, "calendar.edit", &params);
    let shown = agent::bind::Shown::new(&session, "calendar.edit", &params, &what)
        .expect("an occurrence question on a readable series is captured whatever the clock's name");
    assert!(shown.check(&session, "calendar.edit", &params).is_ok(), "before the host moves its rules the untouched question is answerable");

    let untouched = session.mailbox.clone();
    let openings = session.openings();
    let event_before = session.mailbox.event(id).cloned().expect("fixture: the series is there");

    // The host's rules move while the question waits. Nothing of the session's identity changes.
    armed.store(true, Ordering::SeqCst);
    assert_eq!(session.zone(), Zone::LOCAL, "fixture: the calendar still has no named clock");
    assert!(session.zone().id().is_empty(), "fixture: the anonymous clock still has no identifier");
    assert_eq!(session.system_zone(), Zone::LOCAL, "fixture: the system clock is still anonymous, not a fixed override");
    assert_eq!(session.offset_at(instant), after(instant), "fixture: the effective offset at the occurrence is the host's new one");
    assert_ne!(session.offset_at(instant), before(instant), "fixture: and it really differs from the one the day was shown by");
    assert_eq!(session.openings(), openings, "fixture: no mailbox was opened while the question waited");
    assert_eq!(session.mailbox.event(id), Some(&event_before), "fixture: the digest's event bytes are unchanged");
    assert_eq!(agent::describe(&session, "calendar.edit", &params), what, "fixture: the question's words are unchanged");

    // The same operator under the two rules, in separate sessions, really resolves another day:
    // what a yes would reach has moved although nothing the question shows has.
    let mut probe_before = Session::in_zone(untouched.clone(), NOW, Zone::LOCAL);
    probe_before.fallback_offset = Some(Arc::new(|_instant: i64| 0));
    let mut probe_after = Session::in_zone(untouched.clone(), NOW, Zone::LOCAL);
    probe_after.fallback_offset = Some(Arc::new(move |instant: i64| if instant >= moved_at { MOVED } else { 0 }));
    let answer_before = probe_before.run("calendar.edit", &params).expect("fixture: the occurrence edit runs under the first rules");
    let answer_after = probe_after.run("calendar.edit", &params).expect("fixture: the occurrence edit runs under the moved rules");
    assert_eq!(answer_before["occurrence"], json!(day_shown.iso()), "fixture: under the first rules the raw day resolves to the day it was shown as");
    assert_eq!(answer_after["occurrence"], json!(day_moved.iso()), "fixture: under the moved rules the same raw day resolves to the other day");
    assert_ne!(answer_before["occurrence"], answer_after["occurrence"], "fixture: the operation's resolved target really differs");

    // What the owner would say yes to has changed: the raw day now names another occurrence, and
    // the event, its calendar and the question's words do not say so. The yes must be refused.
    assert!(
        shown.check(&session, "calendar.edit", &params).is_err(),
        concat!(
            "a yes pinned while the raw day {} was shown ({} under the host's first rules) was still approved ",
            "after the host's offset rules moved; the same raw day now resolves through series_day_on to {}, ",
            "a different intrinsic occurrence. A yes whose resolved target changed must be refused."
        ),
        raw.iso(),
        day_shown.iso(),
        day_moved.iso()
    );
}

/// The parts of a day a half-hour correction cannot push across midnight: a local daily series at
/// midday by `Africa/Abidjan`'s clock, so both clocks name the same day for every occurrence.
fn midday_series() -> (Mailbox, Id) {
    let mut mailbox = Mailbox::with_local_account("Owner Example", "owner@example.org");
    let calendar = mailbox.default_calendar().expect("fixture: the local account has a calendar");
    let start = at(d(2026, 1, 5), 12, 0);
    let id = mailbox
        .add_event(Event {
            title: "Midday review".into(),
            start,
            end: start.add_minutes(30),
            calendar,
            start_zone: "Africa/Abidjan".into(),
            recurrence: Some(Recurrence::every(Freq::Daily)),
            ..Event::default()
        })
        .expect("fixture: the local calendar takes the series");
    (mailbox, id)
}

/// The same anonymous clock and the same owner's yes, where the host corrects its offset rules by
/// half an hour while the question waits and the occurrence is a midday one: the raw day the
/// parameters name, the intrinsic day it resolves to and the date it is displayed on are the same
/// under both rules. Nothing the owner said yes to changed, so the yes must stay answerable, and
/// the operation under each rule must reach that same intrinsic day.
///
/// This is the positive control the wrong repairs fail. Keeping the blanket `Zone::LOCAL` refusal
/// refuses it; hashing "the host's offset at the occurrence", or refusing on any change of the
/// host's offset, refuses it too, although the target did not move. Its opposite is
/// `an_anonymous_clock_whose_offset_rules_move_does_not_retarget_an_occurrence_yes`, where the days
/// do move and that yes must be refused: together they say "pin what the yes reaches", not "refuse
/// anonymous clocks" and not "refuse any offset change".
#[test]
fn a_benign_offset_change_that_leaves_the_shown_day_alone_keeps_an_anonymous_occurrence_yes_answerable() {
    // Minutes east of UTC the host reports once it has corrected its rules.
    const SHIFT: i32 = 30;

    let (mailbox, id) = midday_series();
    let mut session = Session::in_zone(mailbox, NOW, Zone::LOCAL);
    assert_eq!(session.zone(), Zone::LOCAL, "fixture: the calendar is shown by the system's offset");
    assert!(session.zone().id().is_empty(), "fixture: an anonymous clock has no identifier");
    assert_eq!(session.system_zone(), Zone::LOCAL, "fixture: the named-zone constructor uses the anonymous system clock, not a fixed override");
    assert!(session.clock.offset.is_none(), "fixture: no fixed offset stands in for the host's rules");

    // The instant the host's rules change at, and the rules as they are before and after: one
    // provider, read by the session at the instant it is asked about.
    let moved_at = at(d(2026, 3, 1), 0, 0).to_timestamp(0);
    let shifted = Arc::new(AtomicI32::new(SHIFT));
    let armed = Arc::new(AtomicBool::new(false));
    let provider: Arc<dyn Fn(i64) -> i32 + Send + Sync> = {
        let (shifted, armed) = (Arc::clone(&shifted), Arc::clone(&armed));
        Arc::new(move |instant| if armed.load(Ordering::SeqCst) && instant >= moved_at { shifted.load(Ordering::SeqCst) } else { 0 })
    };
    session.fallback_offset = Some(provider);
    let before = |_instant: i64| 0;
    let after = |instant: i64| if instant >= moved_at { SHIFT } else { 0 };

    // One occurrence the host's two rules really read differently, taken from the iterator rather
    // than guessed, with the day it is shown on.
    let event = session.mailbox.event(id).cloned().expect("fixture: the series is there");
    let occurrences = event.occurrences_in(d(2026, 1, 1), d(2026, 12, 31), &before);
    let chosen = occurrences.iter().find_map(|o| {
        let instant = event.occurrence_instant(o.day, &before);
        (after(instant) != before(instant)).then_some((o.day, o.start.date, instant))
    });
    let (day_shown, raw, instant) = chosen.expect("fixture: the host's two rules differ at some occurrence of the year");

    // Guards, all from the fixture: a midday series is shown on the day it names, under either
    // rule; and the rules really differ at this occurrence.
    assert_eq!(day_shown, raw, "fixture: a midday series is shown on the day it names");
    assert_eq!(event.series_day_on(raw, &before), Some(day_shown), "fixture: under the first rules the raw day names that occurrence");
    assert_eq!(event.series_day_on(raw, &after), Some(day_shown), "fixture: the correction does not move which occurrence the raw day names");
    let (first, later) = (event.occurrence_times(day_shown, &before), event.occurrence_times(day_shown, &after));
    assert_eq!((first.0.date, later.0.date), (raw, raw), "fixture: the occurrence is shown on the same date under both rules");
    assert_ne!(first.0, later.0, "fixture: the correction is real: the occurrence's wall time moves");
    assert_eq!(session.offset_at(instant), before(instant), "fixture: the effective offset is the host's first one, through the session");

    // The owner's question: that occurrence, by the day the series is displayed on.
    let params = json!({ "id": id, "scope": "occurrence", "occurrence": raw.iso(), "title": "Midday review (one day)" });
    let what = agent::describe(&session, "calendar.edit", &params);
    let shown = agent::bind::Shown::new(&session, "calendar.edit", &params, &what)
        .expect("an occurrence question about a readable series is captured whatever the clock's name");
    assert!(shown.check(&session, "calendar.edit", &params).is_ok(), "before the host's correction the untouched question is answerable");

    let untouched = session.mailbox.clone();
    let openings = session.openings();
    let event_before = session.mailbox.event(id).cloned().expect("fixture: the series is there");

    // The host corrects its rules while the question waits. Nothing of the session's identity, of
    // the event, or of the day the owner was shown changes.
    armed.store(true, Ordering::SeqCst);
    assert_eq!(session.zone(), Zone::LOCAL, "fixture: the calendar still has no named clock");
    assert!(session.zone().id().is_empty(), "fixture: the anonymous clock still has no identifier");
    assert!(session.clock.offset.is_none(), "fixture: and still no fixed offset stands in for the host's rules");
    assert_eq!(session.offset_at(instant), after(instant), "fixture: the effective offset at the occurrence is the host's new one");
    assert_ne!(session.offset_at(instant), before(instant), "fixture: and it really differs from the one the day was shown by");
    assert_eq!(session.openings(), openings, "fixture: no mailbox was opened while the question waited");
    assert_eq!(session.mailbox.event(id), Some(&event_before), "fixture: the digest's event bytes are unchanged");
    assert_eq!(agent::describe(&session, "calendar.edit", &params), what, "fixture: the question's words are unchanged");
    assert_eq!(event_before.series_day_on(raw, &after), Some(day_shown), "fixture: the raw day still names the occurrence after the correction");
    assert_eq!(event_before.occurrence_times(day_shown, &after).0.date, raw, "fixture: and it is still shown on the same date");

    let why = format!(
        concat!(
            "the host corrected its offset rules by {shift} minutes at the occurrence, but the raw day {raw} still names the ",
            "intrinsic day {day} and is still shown on {raw}: what the owner said yes to did not change. A blanket refusal of ",
            "an anonymous clock, and a refusal of any change of the host's offset, would both refuse this yes; only a yes ",
            "whose resolved target (its intrinsic day, or the day it is shown on) moved may be refused."
        ),
        shift = SHIFT,
        raw = raw.iso(),
        day = day_shown.iso()
    );
    assert!(shown.check(&session, "calendar.edit", &params).is_ok(), "{why}");

    // What a yes would reach, under each rule, in separate sessions: the same intrinsic day, so
    // staying answerable is not the vacuous approval of a question nobody could act on.
    let mut probe_before = Session::in_zone(untouched.clone(), NOW, Zone::LOCAL);
    probe_before.fallback_offset = Some(Arc::new(|_instant: i64| 0));
    let mut probe_after = Session::in_zone(untouched.clone(), NOW, Zone::LOCAL);
    probe_after.fallback_offset = Some(Arc::new(move |instant: i64| if instant >= moved_at { SHIFT } else { 0 }));
    let answer_before = probe_before.run("calendar.edit", &params).expect("fixture: the occurrence edit runs under the first rules");
    let answer_after = probe_after.run("calendar.edit", &params).expect("fixture: the occurrence edit runs under the corrected rules");
    assert_eq!(answer_before["occurrence"], json!(day_shown.iso()), "fixture: under the first rules the raw day resolves to the day it was shown as");
    assert_eq!(answer_after["occurrence"], json!(day_shown.iso()), "fixture: under the corrected rules the same raw day resolves to that same day");
    assert_eq!(
        answer_before["occurrence"], answer_after["occurrence"],
        "fixture: the operation's resolved target is unchanged, so the answerable yes reaches exactly what it showed"
    );
    assert_eq!(
        probe_after.mailbox.event(id).map(|e| e.exceptions.clone()),
        Some(vec![day_shown]),
        "fixture: the yes under the corrected rules really changed that one day, not another"
    );
}
