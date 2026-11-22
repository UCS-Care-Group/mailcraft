// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The agent gate's binding against what a review of #1663 found it let through: a batch whose
//! pin could not be read fell back to the looser unpinned check; the routing that was pinned left
//! out a calendar and contacts server's own settings; the times a batch's recurring items are
//! shown at, which a paste spaces them by, were not part of the pin; and the older one-event
//! Delete was read by the batch reader, which refuses what the command itself does.
//!
//! One module of `calendar_batch_binding_tests` (`use super::*`: `fixture()`, `d`, `at`, `NOW`,
//! `START`). Synthetic people on `example.org`, servers that are names nobody contacts, events
//! added through `Mailbox::add_event`; no files, network or wall clock.

use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_engine::model::{DavServer, Mailbox};
use mailcraft_engine::time::Zone;

/// The fixture's second account, whose calendar is `Fixture::remote`.
fn second(f: &Fixture) -> Id {
    f.s.mailbox.accounts.iter().find(|a| a.email == "second@example.org").map(|a| a.id).expect("fixture: the second account")
}

/// An appointment the second account's server keeps, in its calendar.
fn kept_by_second(f: &mut Fixture) -> Id {
    let account = second(f);
    let start = at(d(2026, 11, 12), 11, 0);
    f.s.mailbox
        .add_event(Event {
            title: "Second-account standup".into(),
            start,
            end: start.add_minutes(30),
            calendar: f.remote,
            origin: Origin { account, id: "e-second-standup".into(), ..Default::default() },
            ..Default::default()
        })
        .expect("fixture: a server calendar takes an event")
}

/// A Delete is held to its items whatever else its parameters carry: a `calendar` it doesn't read
/// (here `null`) must not turn the pinned check into the looser one, under which an item whose end
/// moved while the owner was asked is still approved (CodeRabbit on #1665, bind.rs:293).
#[test]
fn a_batch_delete_with_a_calendar_it_ignores_is_still_pinned_to_its_items() {
    for calendar in [json!(null), json!("not a calendar")] {
        let mut f = fixture();
        let batch = json!({"items": [{"id": f.kickoff}, {"id": f.review}], "calendar": calendar});
        // (The command reads no `calendar`: the fixture's batch is a real delete.)
        let mut probe = Session::at(f.s.mailbox.clone(), NOW, 0);
        assert!(probe.run("calendar.delete", &batch).is_ok(), "fixture: a Delete ignores `calendar`: {batch}");
        let what = agent::describe(&f.s, "calendar.delete", &batch);
        let shown = agent::bind::Shown::new(&f.s, "calendar.delete", &batch, &what).expect("a Delete of named items can be asked about");
        assert!(shown.check(&f.s, "calendar.delete", &batch).is_ok(), "an untouched question is answerable: {batch}");
        // The review's end moves; its title, day, people and the window stay as they were.
        let event = f.s.mailbox.event_mut(f.review).expect("the review is there");
        event.end = event.end.add_minutes(60);
        assert!(
            shown.check(&f.s, "calendar.delete", &batch).is_err(),
            "an item that changed while the owner was asked must be asked about again: {batch}"
        );
    }
}

/// A batch whose pin can't be read at all is not asked about: the gate fails closed for a batch as
/// it does for a legacy one-event question, rather than holding the yes to the window alone.
#[test]
fn a_batch_whose_pin_cant_be_read_is_never_asked_about() {
    let f = fixture();
    // A paste reads its `calendar`, and `null` names none: the command refuses it.
    let paste = json!({"items": [{"id": f.kickoff}], "start": START, "calendar": null});
    let mut probe = Session::at(f.s.mailbox.clone(), NOW, 0);
    assert!(probe.run("calendar.paste", &paste).is_err(), "fixture: the command refuses a null destination");
    let what = agent::describe(&f.s, "calendar.paste", &paste);
    assert!(
        agent::bind::Shown::new(&f.s, "calendar.paste", &paste, &what).is_none(),
        "a batch question whose items or destination can't be pinned is refused, never held by the looser check"
    );
}

/// The routing a calendar change's yes is held to includes a calendar and contacts server's own
/// settings: its homes, its user name, the certificate trusted for it and whether it has a
/// password of its own (CodeRabbit on #1665, bind.rs:657). A yes given for one server must not
/// reach another, in the batch form and the older one-event form alike.
#[test]
fn a_calendar_servers_own_settings_are_part_of_the_pinned_routing() {
    type Change = fn(&mut DavServer);
    let changes: [(&str, Change); 5] = [
        ("calendar homes", |dav| dav.calendar_homes = vec!["https://elsewhere.example.net/calendars/".into()]),
        ("contact homes", |dav| dav.contact_homes = vec!["https://elsewhere.example.net/contacts/".into()]),
        ("user name", |dav| dav.username = "someone-else".into()),
        ("trusted certificate", |dav| dav.trusted = "ab".repeat(32)),
        ("own password", |dav| dav.separate_password = !dav.separate_password),
    ];
    for (what_changes, change) in changes {
        for form in ["batch", "legacy"] {
            let mut f = fixture();
            let account = second(&f);
            f.s.mailbox.account_mut(account).expect("fixture: the second account").dav = Some(DavServer {
                calendar_homes: vec!["https://dav.example.org/calendars/second/".into()],
                contact_homes: vec!["https://dav.example.org/contacts/second/".into()],
                username: "second".into(),
                ..Default::default()
            });
            let kept = kept_by_second(&mut f);
            let params = if form == "batch" { json!({"items": [{"id": kept}]}) } else { json!({"id": kept}) };
            let what = agent::describe(&f.s, "calendar.delete", &params);
            let shown = agent::bind::Shown::new(&f.s, "calendar.delete", &params, &what).expect("a delete of a kept appointment is asked about");
            assert!(shown.check(&f.s, "calendar.delete", &params).is_ok(), "{form}: untouched, the question is answerable");
            let dav = f.s.mailbox.account_mut(account).ok().and_then(|a| a.dav.as_mut()).expect("fixture: the server's settings");
            change(dav);
            assert!(
                shown.check(&f.s, "calendar.delete", &params).is_err(),
                "{form}: a change of the server's {what_changes} while the owner was asked must be asked about again"
            );
        }
    }
}

/// A local daily series at midday by `Africa/Abidjan`'s clock and a one-off appointment the same
/// afternoon, in a mailbox whose calendar names no zone (its offsets come from the host alone).
fn anonymous_pair() -> (Mailbox, Id, Id) {
    let mut mailbox = Mailbox::with_local_account("Owner Example", "owner@example.org");
    let calendar = mailbox.default_calendar().expect("fixture: the local account has a calendar");
    let start = at(d(2026, 1, 5), 12, 0);
    let series = mailbox
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
    let later = at(d(2026, 3, 10), 15, 0);
    let one = mailbox
        .add_event(Event { title: "Afternoon call".into(), start: later, end: later.add_minutes(30), calendar, ..Event::default() })
        .expect("fixture: the local calendar takes an appointment");
    (mailbox, series, one)
}

/// What a paste puts where is worked out from the times its items are shown at here, and a
/// recurring one's comes from the host's clock: when the host's rules move while the owner is
/// asked, the same items would land with other spacing than the question was about, so the yes
/// is refused (CodeRabbit on #1665, bind.rs:442).
#[test]
fn a_paste_whose_recurring_items_are_shown_at_other_times_is_asked_about_again() {
    const MOVED: i32 = 60;
    let (mailbox, series, one) = anonymous_pair();
    let mut s = Session::in_zone(mailbox, NOW, Zone::LOCAL);
    assert!(s.zone().id().is_empty(), "fixture: an anonymous clock");
    let moved_at = at(d(2026, 3, 1), 0, 0).to_timestamp(0);
    let armed = Arc::new(AtomicBool::new(false));
    let provider: Arc<dyn Fn(i64) -> i32 + Send + Sync> = {
        let armed = Arc::clone(&armed);
        Arc::new(move |instant| if armed.load(Ordering::SeqCst) && instant >= moved_at { MOVED } else { 0 })
    };
    s.fallback_offset = Some(provider);

    let params = json!({"items": [{"id": series, "day": "2026-03-10"}, {"id": one}], "start": START});
    let what = agent::describe(&s, "calendar.paste", &params);
    let shown = agent::bind::Shown::new(&s, "calendar.paste", &params, &what).expect("a paste of named items is asked about");
    assert!(shown.check(&s, "calendar.paste", &params).is_ok(), "untouched, the question is answerable");
    // Where the copies would go before the host's rules move: the call 3 hours after the review.
    let mut before = Session::in_zone(s.mailbox.clone(), NOW, Zone::LOCAL);
    before.fallback_offset = Some(Arc::new(|_| 0));
    let made = before.run("calendar.paste", &params).expect("fixture: the paste runs");
    let ids: Vec<u64> = made["ids"].as_array().expect("ids").iter().filter_map(serde_json::Value::as_u64).collect();
    let starts: Vec<DateTime> = ids.iter().filter_map(|id| before.mailbox.event(*id).map(|e| e.start)).collect();
    assert_eq!(starts.len(), 2, "fixture: two copies");

    armed.store(true, Ordering::SeqCst);
    assert_eq!(agent::describe(&s, "calendar.paste", &params), what, "fixture: the question's words are the same");
    let mut after = Session::in_zone(s.mailbox.clone(), NOW, Zone::LOCAL);
    after.fallback_offset = Some(Arc::new(move |instant| if instant >= moved_at { MOVED } else { 0 }));
    let made = after.run("calendar.paste", &params).expect("fixture: the paste runs under the moved rules");
    let ids: Vec<u64> = made["ids"].as_array().expect("ids").iter().filter_map(serde_json::Value::as_u64).collect();
    let moved: Vec<DateTime> = ids.iter().filter_map(|id| after.mailbox.event(*id).map(|e| e.start)).collect();
    assert_ne!(
        starts[0].minutes_until(starts[1]),
        moved[0].minutes_until(moved[1]),
        "fixture: under the moved rules the same paste spaces its items otherwise"
    );
    assert!(shown.check(&s, "calendar.paste", &params).is_err(), "a yes to one spacing must not be taken for another");
}

/// The older one-event Delete is read as the command reads it: a whole series named with
/// `"scope": "series"` goes whatever day `occurrence` names (even one it skips), and deleting a day
/// the series already skips is a step that does nothing. Neither is refused to an agent the owner
/// let delete (CodeRabbit on #1665, agent.rs:1744); an event that isn't there still is.
#[test]
fn an_agent_is_held_to_the_one_event_delete_as_the_command_reads_it() {
    let mut f = fixture();
    let skipped = d(2026, 11, 11);
    f.s.mailbox.event_mut(f.series).expect("fixture: the series").exceptions.push(skipped);
    for params in [
        json!({"id": f.series, "scope": "series", "occurrence": skipped.iso()}),
        json!({"id": f.series, "scope": "occurrence", "occurrence": skipped.iso()}),
    ] {
        let mut probe = Session::at(f.s.mailbox.clone(), NOW, 0);
        assert!(probe.run("calendar.delete", &params).is_ok(), "fixture: the command takes {params}");
        assert!(
            !matches!(agent::check(&f.s, "calendar.delete", &params, &Class::ALL), Verdict::Refuse(_)),
            "an agent allowed to delete is not refused what the command does: {params}"
        );
    }
    let gone = json!({"id": 999_999, "scope": "series"});
    assert!(matches!(agent::check(&f.s, "calendar.delete", &gone, &Class::ALL), Verdict::Refuse(_)), "an event that isn't there is refused");
}
