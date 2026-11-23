// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The positive control beside `legacy_single_stale_tests`: a legacy single-event approval
//! (`calendar.move`, `calendar.edit`, `calendar.delete` with an explicit `id`) must go through
//! while the event it showed is still that event, and must be refused once the same id and title
//! come back as another event — another UID, or another time. Without the first half a fix could
//! bind legacy single events by refusing every unchanged confirmation; without the second a fix
//! could pass the stale regression by trusting the id alone.
//!
//! One module of `calendar_batch_binding_tests`, sharing that fixture (`fixture()`, `Fixture`,
//! `add`, `d`, `at`, `NOW`, `START`, `agent`, `json!` and the model imports) through
//! `use super::*`.
//!
//! Everything here is synthetic: `example.org` people, events added through the public
//! `Mailbox::add_event`, and an account server that is a name nothing contacts. No files, no
//! network, no wall clock, no real mailbox.
//!
//! WHAT IS HELD (source read 2026-11-15): `Shown::new`/`Shown::check` are the public
//! capture/recheck pair the window's yes uses. The unchanged check must pass, before and after a
//! correct legacy binding. The replaced-source checks must fail while a legacy single `id` is not
//! in `BATCH`, so `pinned()` captures no source state and the unchanged description cannot see a
//! new UID at the same time; that is the control's purpose. A fix that only looks at the
//! description text, or only at the UID, or only at the time, fails at least one of the two
//! replacements.

use super::*;

/// One whole meeting the fixture's second account's server keeps: its calendar is that account's,
/// the owner organizes it and one person is invited, so moving, editing or deleting it is a
/// change that would write to that person, and each command's own parameters say `send`.
fn kept_meeting(f: &mut Fixture) -> Id {
    let calendar = f.remote;
    let account = f.s.mailbox.calendar(calendar).expect("fixture: the second account's calendar is there").origin.account;
    assert_ne!(account, 0, "fixture: the calendar belongs to an account's server, not to this computer");
    let start = at(d(2026, 12, 2), 9, 0);
    let id =
        f.s.mailbox
            .add_event(Event {
                title: "Kickoff (kept)".into(),
                start,
                end: start.add_minutes(45),
                calendar,
                organizer: Some(Address::new("Second Example", "second@example.org")),
                attendees: vec![Attendee { address: Address::new("Alice Example", "alice@example.org"), ..Attendee::default() }],
                origin: Origin { account, id: "server-kickoff".into(), ..Origin::default() },
                uid: "source-kickoff@example.org".into(),
                ..Event::default()
            })
            .expect("fixture: the calendar takes the meeting");
    let kept = f.s.mailbox.event(id).expect("fixture: the meeting is there");
    assert!(kept.recurrence.is_none(), "fixture: one whole, non-recurring meeting");
    assert_eq!(kept.origin.account, account, "fixture: kept by that account's server");
    assert!(!kept.origin.guest, "fixture: the owner's own meeting, not one they were invited to");
    assert_eq!(kept.uid, "source-kickoff@example.org", "fixture: a UID of its own");
    assert!(!kept.attendees.is_empty(), "fixture: somebody invited, so each command writes to them");
    id
}

/// What a Send/Receive can bring while a legacy question waits: the same id and title under
/// another UID at another time. `Mailbox::add_event` keeps a named id when it is free, so taking
/// the old record out first is the supported way to replace it under that id; no allocator is
/// assumed.
fn replaced(f: &mut Fixture, id: Id, uid: &str, start: DateTime) {
    let before = f.s.mailbox.event(id).cloned().expect("fixture: the meeting is there to be replaced");
    let replacement = Event { uid: uid.to_string(), start, end: start.add_minutes(before.duration_minutes()), ..before.clone() };
    f.s.mailbox.remove_event(id).expect("fixture: the old record is taken out");
    let again = f.s.mailbox.add_event(replacement).expect("fixture: the replacement is added");
    assert_eq!(again, id, "fixture: the replacement keeps the id the question names");
    let now = f.s.mailbox.event(id).expect("fixture: the replacement is under the same id");
    assert_eq!(now.title, before.title, "fixture: and keeps the title");
    assert_eq!(now.origin, before.origin, "fixture: and the origin the source had");
}

#[test]
fn a_legacy_single_event_approval_is_bound_to_the_event_it_showed() {
    // The three legacy single-event commands that take one event by an explicit `id`. Each has
    // the parameters its own command accepts for a meeting the owner organizes on a server, so
    // the `send` the question would add is already the valid command's own parameter.
    let cases: [(&str, fn(Id) -> Value); 3] = [
        ("calendar.move", |id| json!({ "id": id, "start": START, "send": true })),
        ("calendar.edit", |id| json!({ "id": id, "start": START, "send": true })),
        ("calendar.delete", |id| json!({ "id": id, "send": true })),
    ];
    for (command, params) in cases {
        // A fresh fixture for each command: one command's queued changes and replacements never
        // reach another's check.
        let mut f = fixture();
        let kept = kept_meeting(&mut f);

        // The owner has two other appointments selected: an explicit `id` names this meeting, and
        // never borrows the typed focus for the question or for the binding.
        f.s.run("calendar.select", &json!({ "items": [{ "id": f.review }, { "id": f.unrelated }] }))
            .expect("fixture: two other appointments are selected");
        f.s.sel.events.iter().for_each(|item| assert_ne!(item.id, kept, "fixture: the selected set is not the meeting the command names"));
        assert_eq!(f.s.sel.events.len(), 2, "fixture: a typed set of two others");
        assert!(f.s.mailbox.event(kept).is_some(), "fixture: the explicitly named meeting exists");

        let params = params(kept);
        let what = agent::describe(&f.s, command, &params);
        assert!(what.contains("Kickoff (kept)"), "{command}: the question is about the event its explicit id names: {what}");
        assert!(!what.contains("Unrelated dental") && !what.contains("Review"), "{command}: and never about the typed selection's events: {what}");

        let shown = agent::bind::Shown::new(&f.s, command, &params, &what)
            .expect("a calendar command about one appointment is a question whose yes can be captured");
        assert!(
            shown.check(&f.s, command, &params).is_ok(),
            "{command}: an unchanged legacy approval — the same event, nothing replaced — must go through, never be refused for being the legacy single `id`"
        );

        // The same id and title, another UID, at the very time the question showed: the question's
        // text cannot tell the two apart, so only a binding of the source event itself can.
        let start = f.s.mailbox.event(kept).map(|event| event.start).expect("fixture: the meeting is there");
        replaced(&mut f, kept, "replacement-uid@example.org", start);
        assert_eq!(f.s.mailbox.event(kept).map(|event| event.start), Some(start), "fixture: the replacement is at the time the question showed");
        assert!(
            shown.check(&f.s, command, &params).is_err(),
            "{command}: a yes to the question about the old source must be refused after the event was replaced under the same id and title with another UID"
        );

        // The source's UID again, at another time: the same id and title, and still not the event
        // the owner was shown.
        replaced(&mut f, kept, "source-kickoff@example.org", start.add_minutes(90));
        assert_eq!(
            f.s.mailbox.event(kept).map(|event| event.uid.as_str()),
            Some("source-kickoff@example.org"),
            "fixture: the second replacement has the source's UID"
        );
        assert_ne!(f.s.mailbox.event(kept).map(|event| event.start), Some(start), "fixture: and another time");
        assert!(
            shown.check(&f.s, command, &params).is_err(),
            "{command}: a yes to the question about the old source must be refused after the event was replaced under the same id, title and UID at another time"
        );
    }
}
