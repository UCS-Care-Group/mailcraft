// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The legacy display-zone approval residual: a yes whose resolved occurrence changed must be
//! refused, not carried out. One module of `calendar_batch_binding_tests` through
//! `use super::*`.
//!
//! WHAT IS HELD. `Shown::new`/`Shown::check` are the public capture/recheck pair the window's yes
//! uses. `legacy_state` (bind.rs:484-561) hashes the raw `occurrence` day, the whole event, its
//! calendar, the destination and routing, but not the effective zone/offset and not the day
//! `part()` resolves at run time. `part()` (cmd/calendar.rs:155-171) maps that raw day through
//! `Event::series_day_on(day, offset_of(s))` (cmd/calendar.rs:170; model/zoned.rs:264). A display
//! zone change that leaves the event bytes unchanged therefore keeps the digest equal while the
//! same raw day addresses another intrinsic occurrence.
//!
//! FIXTURE. One synthetic local series on `example.org`, its own zone `Africa/Abidjan`, at 23:30
//! daily from 2026-01-05. Its first occurrence is in January, when Abidjan and London are both
//! UTC+0, so `Mailbox::rebase` (model/zoned.rs:333) rewrites no event byte when the display zone
//! becomes `Europe/London`. Later occurrences are on London summer time; an occurrence's displayed
//! date and its intrinsic `Occurrence::day` then disagree by a day, so `series_day_on` maps the
//! same raw day to a neighbour. Everything is derived from `occurrences_in`/`series_day_on`; no
//! date of the ambiguous occurrence is guessed, and the guards assert the real offsets actually
//! differ before the product assertion is reached. If they do not (another tz database, another
//! fixture), a `fixture:` assertion fails and no bug is claimed.
//!
//! `Session::in_zone` is the named-zone clock constructor; no fixed-offset clock and no
//! prefs-only change is used. The live question is then changed through the public, agent-Safe
//! `view.timeZone` command, which `Session::run` settles with `settle_zone` (session.rs:1514).

use super::*;

use mailcraft_engine::model::Mailbox;
use mailcraft_engine::time::Zone;

#[test]
fn a_legacy_occurrence_yes_is_refused_after_the_display_zone_remaps_the_shown_day() {
    // Named zones only; both real IANA names. The task's warning about `Session::at`'s fixed
    // offset overriding a named preference is respected: neither session pins a fixed offset.
    let zone_a = Zone::named("Africa/Abidjan").expect("fixture: a real IANA zone");
    let zone_b = Zone::named("Europe/London").expect("fixture: a real IANA zone");

    // A mailbox whose times are by zone A, with one daily series that has its own zone. No
    // account contacts anything and no file is opened.
    let fresh = || {
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
        let _ = mailbox.rebase(zone_a);
        (Session::in_zone(mailbox, NOW, zone_a), id)
    };

    let mut approved: Vec<&str> = Vec::new();
    let mut representative: Option<(Date, Date, Date)> = None;

    for command in ["calendar.delete", "calendar.edit", "calendar.move"] {
        let (mut session, id) = fresh();
        let event = session.mailbox.event(id).cloned().expect("fixture: the series is there");
        let off_a = |ts: i64| zone_a.offset_at(ts);
        let off_b = |ts: i64| zone_b.offset_at(ts);

        // The same occurrence iterator under both zones: find an occurrence whose displayed day
        // under A is resolved by `series_day_on` to a *different* intrinsic day under B, on a real
        // clock difference. A, B and the day all come from the iterator, never from a guess.
        let occ_a = event.occurrences_in(d(2026, 1, 1), d(2026, 12, 31), &off_a);
        let occ_b = event.occurrences_in(d(2026, 1, 1), d(2026, 12, 31), &off_b);
        let chosen = occ_a.iter().find_map(|a| {
            let under_b = event.series_day_on(a.start.date, &off_b)?;
            let instant = event.occurrence_instant(a.day, &off_a);
            (under_b != a.day && zone_a.offset_at(instant) != zone_b.offset_at(instant)).then_some((a.day, a.start.date, under_b, instant))
        });
        let (day_a, raw, day_b, instant) = chosen.expect("fixture: the two zones' clocks disagree on some occurrence of the year");

        // Guards: the raw day is what A shows that occurrence on; B selects another intrinsic day;
        // B's iterator really carries that other day on the same displayed date; the clocks differ
        // at the occurrence itself. A guard failure is a fixture failure, not a product claim.
        assert_eq!(event.series_day_on(raw, &off_a), Some(day_a), "fixture: under zone A the raw day is the occurrence it was shown as");
        assert_ne!(day_b, day_a, "fixture: under zone B the same raw day must select another intrinsic day");
        assert!(
            occ_b.iter().any(|o| o.start.date == raw && o.day == day_b),
            "fixture: the other intrinsic day is carried on the same displayed date by the iterator"
        );
        assert_ne!(zone_a.offset_at(instant), zone_b.offset_at(instant), "fixture: the named zones are on different clocks at that occurrence");
        assert_eq!(session.zone(), zone_a, "fixture: the live question is asked in zone A");

        // The legacy one-occurrence parameters carry the *displayed* day, exactly what the UI's
        // "Just This One" answer puts in the question's `with` and what an agent's payload carries.
        let params = match command {
            "calendar.delete" => json!({ "id": id, "scope": "occurrence", "occurrence": raw.iso() }),
            "calendar.edit" => json!({ "id": id, "title": "Night shift (renamed)", "scope": "occurrence", "occurrence": raw.iso() }),
            _ => json!({ "id": id, "start": "2027-01-04T09:00", "scope": "occurrence", "occurrence": raw.iso() }),
        };

        let what = agent::describe(&session, command, &params);
        let shown = agent::bind::Shown::new(&session, command, &params, &what)
            .expect("fixture: a legacy occurrence question about a readable event is captured");
        assert!(shown.check(&session, command, &params).is_ok(), "fixture: before any zone change the untouched question is answerable");

        let untouched = session.mailbox.clone();
        let openings = session.openings();
        let event_before = session.mailbox.event(id).cloned().expect("fixture: the series is there");

        // The supported command an agent may run while a legacy question waits (`view.timeZone` is
        // in the agent's SAFE list). `Session::run` settles the mailbox to the new named zone.
        let changed = session.run("view.timeZone", &json!({ "zone": zone_b.id() })).expect("fixture: choosing a real named zone");
        assert_eq!(changed["zone"], json!(zone_b.id()), "fixture: the command reports the new zone");
        assert_eq!(session.zone(), zone_b, "fixture: the effective zone is the new named zone");
        assert_eq!(session.offset_at(instant), zone_b.offset_at(instant), "fixture: the effective offset at the occurrence is the new zone's");
        assert_eq!(session.openings(), openings, "fixture: no mailbox was opened while the question waited");
        assert_eq!(session.mailbox.event(id), Some(&event_before), "fixture: the digest's event bytes are unchanged");
        if command == "calendar.delete" {
            // (A Delete's question names the day it resolves to now, CodeRabbit C2 on #1665: under
            // zone B that is the other day, so its words change too.)
            assert_ne!(agent::describe(&session, command, &params), what, "the Delete's question would now name the other day");
        } else {
            assert_eq!(agent::describe(&session, command, &params), what, "fixture: the question's words are unchanged");
        }

        // The same command under the two clocks, in separate Session copies, shows the resolved
        // target. `calendar.delete`/`edit`/`move` all answer the intrinsic `occurrence` day they
        // acted on (change_one, cmd/calendar.rs:585; delete, cmd/calendar.rs:805).
        let mut probe_a = Session::in_zone(untouched.clone(), NOW, zone_a);
        let answer_a = probe_a.run(command, &params).expect("fixture: the command runs under zone A");
        let mut probe_b = Session::in_zone(untouched.clone(), NOW, zone_b);
        let answer_b = probe_b.run(command, &params).expect("fixture: the command runs under zone B");
        assert_eq!(answer_a["occurrence"], json!(day_a.iso()), "fixture: under zone A the raw day resolves to the day it was shown as");
        assert_eq!(answer_b["occurrence"], json!(day_b.iso()), "fixture: under zone B the same raw day resolves to the other day");
        assert_ne!(answer_a["occurrence"], answer_b["occurrence"], "fixture: the operation's resolved target really differs");

        if representative.is_none() {
            representative = Some((raw, day_a, day_b));
        }
        if shown.check(&session, command, &params).is_ok() {
            approved.push(command);
        }
    }

    let (raw, day_a, day_b) = representative.expect("the three commands all ran");
    let why = format!(
        concat!(
            "a yes pinned while the raw occurrence day {raw} was shown ({day_a} under {a}) was ",
            "still approved by Shown::check for {approved:?} after the display zone became {b}; ",
            "the same raw day now maps through series_day_on to {day_b}, a different intrinsic ",
            "occurrence. A yes whose resolved target changed must be refused (review 31, suggestion 1)."
        ),
        raw = raw.iso(),
        day_a = day_a.iso(),
        b = zone_b.id(),
        day_b = day_b.iso(),
        a = zone_a.id(),
        approved = approved
    );
    assert!(approved.is_empty(), "{why}");
}
