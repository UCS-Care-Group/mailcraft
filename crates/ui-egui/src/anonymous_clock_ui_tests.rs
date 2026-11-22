// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's own whole-series Delete on a host whose calendar has no named clock: the window
//! must still ask which of the series is meant, the question's own "The Entire Series" button
//! must carry it out, and Undo must take it back.
//!
//! Why this exists, in the source it was written against (2026-11-15):
//! - `MailApp::run` asks `dialogs::confirmation` (`lib.rs:2255-2258`) and then
//!   `dialogs::bind_calendar` (`lib.rs:2263-2266`), which calls `Shown::new`
//!   (`dialogs.rs:1148-1169`). `legacy_state` returns `None` for every recurring event while
//!   `session.zone().is_local()` (`agent/bind.rs:495-498`), so `bind_calendar` returns its `Err`,
//!   `run` says it in the status bar and returns it **before** `self.dialog` is set: no question is
//!   ever drawn.
//! - The window's own chooser (`dialogs.rs:1447-1461`) is the one the owner still has today on a
//!   named or fixed clock: "Delete Recurring Item", "Just This One", with "The Entire Series"
//!   (`{"scope": "series"}`) as its second way. Whole series resolves no day, so the anonymous
//!   clock is not a reason to drop it.
//!
//! The session is the real one `Session::in_zone(mailbox, now(), Zone::LOCAL)` makes — the same
//! state the browser build and an offset-only host are in — put in place of the window's session
//! exactly as the host does with a session it built (`MailApp::session` is a public field), then
//! the view is restored through the window's own `view.week`. The question is opened by the
//! owner's own `MailApp::run`, and answered by clicking the button the open question holds. No
//! dialog is set by the test, no `Shown` is forged, and no future API is called. The mailbox is the
//! sample's own invented one (`example.org` people, no file, server or real account).

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;
use mailcraft_time::Zone;

use crate::dialogs::Dialog;
use crate::people_view::testing::{dialog, now};

use super::*;

/// Press the one button called `label`. The question is drawn over the window, so where the ribbon
/// has a button of the same name the dialog's is the lower one.
fn press(window: &mut Window, label: &str) {
    let mut at: Vec<Rect> = window
        .query_all_by_label_contains(label)
        .filter(|node| node.accesskit_node().role() == Role::Button && node.accesskit_node().label().is_some_and(|said| said.trim() == label))
        .map(|node| node.rect())
        .collect();
    assert!(!at.is_empty(), "no button of the window says {label:?}");
    at.sort_by(|a, b| b.min.y.total_cmp(&a.min.y));
    let pos = at[0].center();
    super::click(window, pos, Modifiers::NONE, PointerButton::Primary);
    window.run_steps(3);
}

#[test]
fn an_anonymous_clock_still_asks_the_owner_before_a_whole_series_goes() {
    let mut w = calendar();
    // The calendar has no named clock, as in the browser build and on a host that gives offsets
    // only: the real session `Session::in_zone` makes for `Zone::LOCAL`.
    let mailbox = w.state().session.mailbox.clone();
    w.state_mut().session = Session::in_zone(mailbox, now(), Zone::LOCAL);
    // Keep display dates deterministic while retaining the anonymous system clock.
    w.state_mut().session.fallback_offset = Some(std::sync::Arc::new(|_instant: i64| 0));
    assert_eq!(w.state().session.zone(), Zone::LOCAL, "fixture: the calendar is shown by the system's offset");
    assert!(w.state().session.zone().id().is_empty(), "fixture: an anonymous clock has no identifier");
    assert_eq!(
        w.state().session.system_zone(),
        Zone::LOCAL,
        "fixture: the named-zone constructor uses the anonymous system clock, not a fixed override"
    );
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
    // (The sample has reminders due; the swap gave them a new session to be worked out in.)
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    w.run_steps(2);

    let standup = event(&w, "Team stand-up");
    let (id, start) = (standup.id, standup.start);
    assert!(standup.recurrence.is_some(), "fixture: an occurrence of a series");
    let day = date(10, 14);
    w.state_mut().execute("calendar.select", json!({ "id": id, "day": day.iso() })).expect("fixture: the owner selects the occurrence");
    w.run_steps(2);
    assert_eq!(focused(&w), (Some(id), Some(day)), "fixture: the occurrence in focus is the one the chooser names");

    // Delete, through the owner's own `run`: the window asks which of the series is meant.
    let said = w
        .state_mut()
        .run("calendar.delete", json!({ "id": id }))
        .expect("an anonymous clock must not refuse the owner's own question before it is asked");
    assert_eq!(said["pending"], json!("confirm"), "the owner is asked, not run over: {said}");
    w.run_steps(3);
    assert_eq!(dialog(&w), Some("confirm"), "one question is on screen");
    match &w.state().dialog {
        Some(Dialog::Confirm { command, ask, .. }) => {
            assert_eq!(command.as_str(), "calendar.delete");
            assert_eq!(ask.title.as_str(), "Delete Recurring Item", "the old chooser's own title");
            assert_eq!(ask.action.as_str(), "Just This One", "and its one-occurrence way");
            assert!(ask.text.contains("Team stand-up"), "the question names the series: {}", ask.text);
            assert!(
                ask.more.iter().any(|(label, with)| label.as_str() == "The Entire Series" && *with == json!({ "scope": "series" })),
                "and the whole-series way: {:?}",
                ask.more
            );
        }
        _ => panic!("no question is open"),
    }

    // The owner presses the whole-series answer the question itself offers.
    press(&mut w, "The Entire Series");
    assert!(w.state().session.mailbox.event(id).is_none(), "the whole series went, not one day of it");
    assert!(w.state_mut().session.undo(), "one step back");
    w.run_steps(2);
    let back = event(&w, "Team stand-up");
    assert_eq!((back.id, back.start), (id, start), "Undo brings the series back where it was");
}

/// The same anonymous clock and the same real window, the one-occurrence way: `scope` and
/// `occurrence` are here present and *null*, which names no scope — the shape the owner's question
/// reads (`has` is present and not null), so the chooser is still asked for it — and the owner
/// presses its own "Just This One". Exactly the occurrence the question named goes, the series and
/// every other day of it stay, and one Undo takes that one day back.
#[test]
fn an_anonymous_clock_asks_which_occurrence_and_just_this_one_goes_alone() {
    let mut w = calendar();
    // The calendar has no named clock, as in the browser build and on a host that gives offsets
    // only. The host's answer is the sample's own zero offset at every instant, so the day the
    // question names is the sample's on every host. The clock stays anonymous: no fixed offset
    // takes the host's place.
    let mailbox = w.state().session.mailbox.clone();
    w.state_mut().session = Session::in_zone(mailbox, now(), Zone::LOCAL);
    w.state_mut().session.fallback_offset = Some(std::sync::Arc::new(|_instant: i64| 0));
    assert_eq!(w.state().session.zone(), Zone::LOCAL, "fixture: the calendar is shown by the system's offset");
    assert!(w.state().session.zone().id().is_empty(), "fixture: an anonymous clock has no identifier");
    assert_eq!(
        w.state().session.system_zone(),
        Zone::LOCAL,
        "fixture: the named-zone constructor uses the anonymous system clock, not a fixed override"
    );
    assert!(w.state().session.clock.offset.is_none(), "fixture: nothing pins a fixed offset in the host's place");
    assert_eq!(w.state().session.offset_at(now()), 0, "fixture: the host's rules are read through the session");
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
    // (The sample has reminders due; the swap gave them a new session to be worked out in.)
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    w.run_steps(2);

    let standup = event(&w, "Team stand-up");
    let (id, start) = (standup.id, standup.start);
    assert!(standup.recurrence.is_some(), "fixture: an occurrence of a series");
    assert!(standup.exceptions.is_empty(), "fixture: no day of it has been cancelled yet");
    let day = date(10, 14);
    w.state_mut().execute("calendar.select", json!({ "id": id, "day": day.iso() })).expect("fixture: the owner selects the occurrence");
    w.run_steps(2);
    assert_eq!(focused(&w), (Some(id), Some(day)), "fixture: the occurrence in focus is the one the chooser names");

    // Delete with `scope` and `occurrence` present and null. Neither names a scope, so the chooser
    // is still asked; the day it offers is the day the yes must be held to, taken from the question
    // rather than skipped because the keys are there.
    let steps = w.state().session.undo_steps_moved();
    let said = w
        .state_mut()
        .run("calendar.delete", json!({ "id": id, "scope": null, "occurrence": null }))
        .expect("an anonymous clock must not refuse the owner's own question before it is asked");
    assert_eq!(said["pending"], json!("confirm"), "the owner is asked, not run over: {said}");
    w.run_steps(3);
    assert_eq!(dialog(&w), Some("confirm"), "one question is on screen");
    let one = json!({ "scope": "occurrence", "occurrence": day.iso() });
    match &w.state().dialog {
        Some(Dialog::Confirm { command, ask, .. }) => {
            assert_eq!(command.as_str(), "calendar.delete");
            assert_eq!(ask.title.as_str(), "Delete Recurring Item", "the old chooser's own title");
            assert_eq!(ask.action.as_str(), "Just This One", "and its one-occurrence way");
            assert_eq!(ask.with, one, "the one-occurrence answer is the occurrence the owner is shown, not the null parameters");
            assert!(ask.text.contains("Team stand-up"), "the question names the series: {}", ask.text);
            assert!(
                ask.more.iter().any(|(label, with)| label.as_str() == "The Entire Series" && *with == json!({ "scope": "series" })),
                "and the whole-series way: {:?}",
                ask.more
            );
        }
        _ => panic!("no question is open"),
    }

    // The owner presses the one-occurrence answer the question itself offers.
    press(&mut w, "Just This One");
    assert_eq!(dialog(&w), None, "the question is answered and gone");
    let after = event(&w, "Team stand-up");
    assert_eq!(after.id, id, "the series is still there, so the entire series did not go");
    assert!(after.recurrence.is_some(), "and it is still a series");
    assert_eq!(after.exceptions, vec![day], "exactly the occurrence the question named went, and no other");
    assert_eq!((after.start, after.end), (start, standup.end), "the series itself did not move");
    assert_eq!(after.title, standup.title, "and did not change");
    assert_eq!(w.state().session.undo_steps_moved(), steps + 1, "one command, one undo step");
    // The days beside it still happen: what went is one occurrence, not the week.
    let off = |_instant: i64| 0;
    assert!(after.occurrences_in(day, day, &off).is_empty(), "the cancelled day no longer happens");
    assert!(
        after.occurrences_in(day.add_days(-1), day.add_days(-1), &off).iter().any(|o| o.start.date == day.add_days(-1)),
        "the day before it still happens"
    );
    assert!(
        after.occurrences_in(day.add_days(1), day.add_days(1), &off).iter().any(|o| o.start.date == day.add_days(1)),
        "the day after it still happens"
    );

    // One Undo takes the one cancelled day back.
    assert!(w.state_mut().session.undo(), "one step back");
    w.run_steps(2);
    let back = event(&w, "Team stand-up");
    assert_eq!((back.id, back.start, back.end), (id, start, standup.end), "Undo brings the series back as it was");
    assert!(back.exceptions.is_empty(), "one Undo takes back the one day that went");
}

/// The same anonymous clock and the same real window, the one-occurrence way, where the host's
/// offset rules for the clock move while the owner is being asked.
///
/// `scope` and `occurrence` are present and *null* — `p::str` and `p::date` read both as absent
/// (`crates/engine/src/lib.rs:286-288`, `:388-394`) — so the question's own offered "Just This One"
/// day is the only target the parameters name, and it is taken from `session.sel.event_day` when
/// the question is made (`dialogs.rs:1432`, `:1460`). A repair that stops refusing an anonymous
/// clock and pins only the parameters' own explicit target pins no day here at all; the same
/// displayed day then resolves through `series_day_on` to another intrinsic occurrence when the
/// yes is answered, and the command would cancel that other day, not the one the owner was shown.
///
/// The fixture is a daily series at 23:30 by `Africa/Abidjan`'s clock, in the sample's own calendar
/// through `Mailbox::add_event`. The host's rules are an hour east from the start of the sample's
/// Wednesday; only the provider's flag moves them, and only after the real question exists.
/// `temporary` says whether they stay moved (`false`, the returned regression) or fall back to
/// zero at the start of Thursday (`true`), so that the series' own Thursday at 23:30 still shows
/// on Thursday while Wednesday's at 23:30 is carried past midnight onto the same displayed date.
/// The question is opened by the owner's own `MailApp::run` and answered by pressing its own
/// button; the world after the refusal is the world before it: no event byte, no undo step and
/// nothing queued for a server.
fn anonymous_clock_null_question_with_moving_rules(temporary: bool) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    use mailcraft_model::{Freq, Recurrence};

    // Minutes east of UTC the host reports once its rules have moved.
    const MOVED: i32 = 60;

    // Whether an instant is one the moved rules apply to. One reader, so the flag the session
    // hands to the window, the pure reader the guards use, and the probe's provider cannot drift.
    fn moved_here(instant: i64, moved_at: i64, falls_back_at: i64, temporary: bool) -> bool {
        instant >= moved_at && (!temporary || instant < falls_back_at)
    }

    let mut w = calendar();
    // A daily series at 23:30 by Abidjan's clock: the day a calendar shows it on can be the day
    // before the one the series names, and which day that is is the host's rules to say.
    let calendar = w.state().session.mailbox.default_calendar().expect("fixture: the sample has a local calendar");
    let start = at(date(10, 12), 23, 30);
    let id = w
        .state_mut()
        .session
        .mailbox
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

    // The real session an anonymous host has, with the host's rules read at every instant asked
    // about. They are one set of rules before the question; only the flag moves them, and later.
    let mailbox = w.state().session.mailbox.clone();
    w.state_mut().session = Session::in_zone(mailbox, now(), Zone::LOCAL);
    let moved_at = at(date(10, 14), 0, 0).to_timestamp(0);
    let falls_back_at = at(date(10, 15), 0, 0).to_timestamp(0);
    let moved = Arc::new(AtomicI32::new(MOVED));
    let armed = Arc::new(AtomicBool::new(false));
    let provider: Arc<dyn Fn(i64) -> i32 + Send + Sync> = {
        let (moved, armed) = (Arc::clone(&moved), Arc::clone(&armed));
        Arc::new(move |instant| {
            if armed.load(Ordering::SeqCst) && moved_here(instant, moved_at, falls_back_at, temporary) { moved.load(Ordering::SeqCst) } else { 0 }
        })
    };
    w.state_mut().session.fallback_offset = Some(provider);
    assert_eq!(w.state().session.zone(), Zone::LOCAL, "fixture: the calendar is shown by the system's offset");
    assert!(w.state().session.zone().id().is_empty(), "fixture: an anonymous clock has no identifier");
    assert!(w.state().session.clock.offset.is_none(), "fixture: no fixed offset stands in for the host's rules");
    assert_eq!(w.state().session.system_zone(), Zone::LOCAL, "fixture: the anonymous system clock, not a fixed override");
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
    // (The sample has reminders due; the swap gave them a new session to be worked out in.)
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    w.run_steps(2);

    // The occurrence the owner picks in the week: a day the series happens on by the clock as it
    // is before the host's rules move.
    let raw = date(10, 15);
    w.state_mut()
        .execute("calendar.select", json!({ "id": id, "day": raw.iso() }))
        .expect("fixture: the owner selects an occurrence of the new series");
    w.run_steps(2);
    assert_eq!(focused(&w), (Some(id), Some(raw)), "fixture: the occurrence in focus is the one the chooser names");

    let series = w.state().session.mailbox.event(id).cloned().expect("fixture: the series is there");
    let before = |_instant: i64| 0;
    let after = |instant: i64| if moved_here(instant, moved_at, falls_back_at, temporary) { MOVED } else { 0 };
    let instant = series.occurrence_instant(raw, &before);
    // Guards, all read off the fixture: the raw day names itself under the first rules and another
    // intrinsic day under the moved ones, and the series' anchor is the same instant under both.
    assert_eq!(series.series_day_on(raw, &before), Some(raw), "fixture: under the first rules the raw day names itself");
    let day_moved = series.series_day_on(raw, &after).expect("fixture: the moved rules name an occurrence on the raw day");
    assert_ne!(day_moved, raw, "fixture: the moved rules must map the raw day to another intrinsic day");
    assert!(
        series.occurrences_in(raw.add_days(-1), raw.add_days(1), &after).iter().any(|o| o.start.date == raw && o.day == day_moved),
        "fixture: the other intrinsic day is carried on the same displayed date by the iterator"
    );
    if temporary {
        // The moved rules last one day: the series' own Thursday at 23:30 still displays on
        // Thursday, while Wednesday's at 23:30 is carried past midnight onto it. So the displayed
        // day the question offers carries two intrinsic occurrences, and the reader that names
        // "the occurrence on this day" prefers the earlier one. Both facts come from the
        // iterator, not from a guessed intrinsic key.
        let on_raw = series.occurrences_in(raw, raw, &after);
        assert_eq!(on_raw.iter().filter(|o| o.start.date == raw).count(), 2, "fixture: the raw displayed day carries two occurrences of the series");
        let carried = on_raw
            .iter()
            .find(|o| o.day == raw.add_days(-1) && o.start.date == raw)
            .expect("fixture: the preceding occurrence is carried onto the raw day");
        assert_eq!(
            (carried.start.date, carried.start.hour, carried.start.minute),
            (raw, 0, 30),
            "fixture: Wednesday's 23:30 is carried to Thursday's 00:30"
        );
        let own = on_raw
            .iter()
            .find(|o| o.day == raw && o.start.date == raw)
            .expect("fixture: the series' own raw occurrence still displays on the raw day");
        assert_eq!(
            (own.start.date, own.start.hour, own.start.minute),
            (raw, 23, 30),
            "fixture: the series' own Thursday 23:30 still displays on Thursday at 23:30"
        );
        let own_before = series
            .occurrences_in(raw.add_days(-2), raw.add_days(2), &before)
            .into_iter()
            .find(|o| o.day == raw)
            .expect("fixture: the series' own raw occurrence happens");
        assert_eq!(own_before.start.date, raw, "fixture: before the move it displays on the raw day");
        assert_eq!(own.start.date, raw, "fixture: after the move its displayed date is still the raw day");
        assert_eq!(day_moved, raw.add_days(-1), "fixture: the moved reader prefers the preceding occurrence on that displayed day");
    }
    // The preceding occurrence's own instant: the one an hour east moves when the window covers
    // it, whereas the offered occurrence's instant can lie after the window.
    let affected = series.occurrence_instant(day_moved, &before);
    assert_eq!(w.state().session.offset_at(instant), before(instant), "fixture: the effective offset is the host's first one");
    assert_eq!(
        series.occurrence_instant(series.start.date, &after),
        series.occurrence_instant(series.start.date, &before),
        "fixture: the host's rules move after the series' anchor, so no event byte reads differently"
    );

    // The question a Delete asks with both keys present and null: neither names a target, so the
    // window must ask, and the day it offers is the only target the owner can be held to.
    let raw_params = json!({ "id": id, "scope": null, "occurrence": null });
    let steps = w.state().session.undo_steps_moved();
    let openings = w.state().session.openings();
    let untouched = w.state().session.mailbox.clone();
    let said = w
        .state_mut()
        .run("calendar.delete", raw_params.clone())
        .expect("an anonymous clock must not refuse the owner's own question before it is asked");
    assert_eq!(said["pending"], json!("confirm"), "the owner is asked, not run over: {said}");
    w.run_steps(3);
    assert_eq!(dialog(&w), Some("confirm"), "one question is on screen");
    match &w.state().dialog {
        Some(Dialog::Confirm { command, params, ask, .. }) => {
            assert_eq!(command.as_str(), "calendar.delete");
            assert_eq!(params, &raw_params, "the question keeps the parameters it was asked with, null keys and all");
            assert_eq!(ask.title.as_str(), "Delete Recurring Item", "the old chooser's own title");
            assert_eq!(ask.action.as_str(), "Just This One", "and its one-occurrence way");
            assert_eq!(
                ask.with,
                json!({ "scope": "occurrence", "occurrence": raw.iso() }),
                "the offered target is the occurrence the owner is shown, taken from the question"
            );
            assert!(ask.text.contains("Night shift"), "the question names the series: {}", ask.text);
        }
        _ => panic!("no question is open"),
    }

    // The host's rules move while the question waits: only the flag changes. The clock is still
    // anonymous, no fixed offset appears, no mailbox byte moves and the raw parameters stay as
    // they were.
    armed.store(true, Ordering::SeqCst);
    assert_eq!(w.state().session.zone(), Zone::LOCAL, "fixture: the calendar still has no named clock");
    assert!(w.state().session.zone().id().is_empty(), "fixture: the anonymous clock still has no identifier");
    assert!(w.state().session.clock.offset.is_none(), "fixture: and no fixed offset stands in for the host's rules");
    assert_eq!(w.state().session.offset_at(instant), after(instant), "fixture: the effective offset at the occurrence is the host's new one");
    if temporary {
        // The offered occurrence's own instant is outside the moved window, so the guard must look
        // at the preceding occurrence the window does affect: Wednesday 23:30's actual instant.
        assert!(instant >= falls_back_at, "fixture: the offered occurrence's own instant is after the moved window");
        assert_eq!(w.state().session.wall_at(instant).date, raw, "fixture: the session still displays the offered occurrence on the raw day");
        assert!(affected >= moved_at && affected < falls_back_at, "fixture: the preceding occurrence's instant is inside the moved window");
        assert_eq!(
            w.state().session.offset_at(affected),
            after(affected),
            "fixture: the session reads the moved rules at the affected occurrence's instant"
        );
        assert_ne!(
            w.state().session.offset_at(affected),
            before(affected),
            "fixture: and it really differs from the one that occurrence was shown by"
        );
        assert_eq!(w.state().session.offset_at(falls_back_at), 0, "fixture: the moved rules are over at the start of Thursday");
    } else {
        assert_ne!(w.state().session.offset_at(instant), before(instant), "fixture: and it really differs from the one the day was shown by");
    }
    assert_eq!(w.state().session.mailbox, untouched, "fixture: not one mailbox byte changed while the question waited");
    assert_eq!(w.state().session.openings(), openings, "fixture: no mailbox was opened while the question waited");
    match &w.state().dialog {
        Some(Dialog::Confirm { params, .. }) => assert_eq!(params, &raw_params, "fixture: the raw parameters are unchanged"),
        _ => panic!("the question is gone before it was answered"),
    }
    assert_eq!(series.series_day_on(raw, &after), Some(day_moved), "fixture: under the moved rules the same raw day names the other intrinsic day");

    // What the yes would reach under each rule, in sessions of its own: the raw day the question
    // offers resolves to another intrinsic occurrence once the host has moved, although nothing
    // the question shows, keeps, or would add to its parameters, has changed.
    let answered = json!({ "id": id, "scope": "occurrence", "occurrence": raw.iso() });
    let mut probe_before = Session::in_zone(untouched.clone(), now(), Zone::LOCAL);
    probe_before.fallback_offset = Some(Arc::new(|_instant: i64| 0));
    let rules = move |instant: i64| if moved_here(instant, moved_at, falls_back_at, temporary) { MOVED } else { 0 };
    let mut probe_after = Session::in_zone(untouched.clone(), now(), Zone::LOCAL);
    probe_after.fallback_offset = Some(Arc::new(rules));
    assert_eq!(probe_after.offset_at(instant), after(instant), "fixture: the probe's provider is the same window as the session's and the guards'");
    assert_eq!(probe_after.offset_at(affected), after(affected), "fixture: and it moves the same preceding occurrence the session does");
    let answer_before = probe_before.run("calendar.delete", &answered).expect("fixture: the offered day deletes under the first rules");
    let answer_after = probe_after.run("calendar.delete", &answered).expect("fixture: the offered day deletes under the moved rules");
    assert_eq!(answer_before["occurrence"], json!(raw.iso()), "fixture: the first rules reach the occurrence the raw day was shown as");
    assert_eq!(answer_after["occurrence"], json!(day_moved.iso()), "fixture: the moved rules reach the other intrinsic day");
    assert_ne!(answer_before["occurrence"], answer_after["occurrence"], "fixture: the operation's resolved target really differs");
    assert_eq!(
        probe_before.mailbox.event(id).map(|e| e.exceptions.clone()),
        Some(vec![raw]),
        "fixture: the first rules really change the day the owner was shown"
    );
    assert_eq!(
        probe_after.mailbox.event(id).map(|e| e.exceptions.clone()),
        Some(vec![day_moved]),
        "fixture: the moved rules really change that other day"
    );

    // The owner presses the one-occurrence answer the question itself offers. The raw day it names
    // now addresses another intrinsic day, so the yes must be refused rather than retargeted: the
    // question goes, nothing runs, and the world is as it was.
    press(&mut w, "Just This One");
    assert_eq!(dialog(&w), None, "the refused question is gone, not answered");
    let after_click = w.state().session.mailbox.event(id).cloned().expect("the series is still there");
    assert_eq!(after_click, series, "the series did not move, change or lose a day");
    assert!(after_click.exceptions.is_empty(), "the day the yes would have retargeted did not go");
    assert_eq!(w.state().session.mailbox, untouched, "not one mailbox byte changed: the yes performed nothing");
    assert_eq!(w.state().session.mailbox.queued, untouched.queued, "and nothing was queued for a server either");
    assert_eq!(w.state().session.undo_steps_moved(), steps, "the refused yes is not an undo step");
    assert_eq!(w.state().session.openings(), openings, "and no mailbox was opened");
    let told = w.state().status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default();
    assert!(told.contains("Nothing was done"), "the owner is told the question could not be honoured as asked: {told:?}");
}

/// The returned regression: the host's rules step an hour east from the start of Wednesday and
/// stay there, so the raw displayed day's own occurrence moves off it and another day moves onto
/// it.
#[test]
fn an_anonymous_clock_does_not_let_a_null_question_yes_reach_another_occurrence() {
    anonymous_clock_null_question_with_moving_rules(false);
}

/// The same fixture where the moved rules last one day: the series' own Thursday at 23:30 still
/// displays on Thursday while Wednesday's at 23:30 is carried past midnight onto the same date.
/// The question's displayed day now carries two intrinsic occurrences, and `series_day_on`
/// prefers the earlier. A repair that hashes only the displayed day — or that checks only that
/// the offered occurrence still displays on it — would approve the yes, and the command would
/// cancel the other occurrence on the same displayed day. The yes must be refused instead.
#[test]
fn an_anonymous_clock_does_not_let_a_same_displayed_day_stand_for_another_occurrence() {
    anonymous_clock_null_question_with_moving_rules(true);
}
