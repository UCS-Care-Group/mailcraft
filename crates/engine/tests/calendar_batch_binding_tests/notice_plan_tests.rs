// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One plan of who a batch writes to (rust-review-01 F6, owner decision D1): the command's count of
//! the people it asks about, the agent's question and the window's question all come from
//! `calendar::delete_notices` / `paste_notices`, so the same set can't be one number in the
//! refusal and other people in a question. The same person invited to two of the meetings is one
//! person; an attendee without an address is still somebody the server may write to (the host's
//! rule counts every attendee), counted on their own and named as such.
//!
//! One module of `calendar_batch_binding_tests` (`use super::*`: `fixture()`, `d`, `at`, `NOW`,
//! `START`). Synthetic people on `example.org`; the second account's server is a name nobody
//! contacts.

use super::*;

use mailcraft_engine::cmd::calendar::{self, Mail, NoticeKind};

/// The fixture's second account, whose calendar is `Fixture::remote`.
fn second(f: &Fixture) -> Id {
    f.s.mailbox.accounts.iter().find(|a| a.email == "second@example.org").map(|a| a.id).expect("fixture: the second account")
}

/// A meeting the owner organizes, kept by the second account's server, with `who` invited.
fn kept_meeting(f: &mut Fixture, title: &str, hour: u8, remote: &str, who: &[(&str, &str)]) -> Id {
    let account = second(f);
    let start = at(d(2026, 11, 12), hour, 0);
    let attendees: Vec<Attendee> =
        who.iter().map(|(name, address)| Attendee { address: Address::new(*name, *address), ..Default::default() }).collect();
    f.s.mailbox
        .add_event(Event {
            title: title.into(),
            start,
            end: start.add_minutes(30),
            calendar: f.remote,
            attendees,
            origin: Origin { account, id: remote.into(), ..Default::default() },
            ..Default::default()
        })
        .expect("fixture: a server calendar takes a meeting")
}

#[test]
fn a_batch_delete_counts_asks_and_names_the_same_people() {
    let mut f = fixture();
    let account = second(&f);
    // Ann, nobody (no address), Bo, and Bo again in capitals; then Bo and Cy.
    let first = kept_meeting(
        &mut f,
        "Planning",
        9,
        "e-planning",
        &[("Ann Example", "ann@example.org"), ("Room", ""), ("Bo Example", "bo@example.org"), ("Bo Again", "BO@example.org")],
    );
    let other = kept_meeting(&mut f, "Retro", 11, "e-retro", &[("Bo Example", "bo@example.org"), ("Cy Example", "cy@example.org")]);
    let batch = json!({"items": [{"id": first}, {"id": other}]});

    let plan = calendar::delete_notices(&f.s, &batch).expect("a Delete of named items is a set");
    assert_eq!(plan.len(), 2, "each meeting writes to somebody: {plan:?}");
    for notice in &plan {
        assert_eq!((notice.kind, notice.account), (NoticeKind::Cancellation, account), "a cancellation, by the second account's server");
    }
    assert_eq!(plan[0].to, ["ann@example.org", "bo@example.org"], "each person once, by address");
    assert_eq!((plan[0].unnamed, plan[1].unnamed), (1, 0), "and the room without an address counted apart");
    assert_eq!(plan[1].to, ["bo@example.org", "cy@example.org"]);
    let people = calendar::recipients(&plan);
    assert_eq!(people, ["ann@example.org", "bo@example.org", "cy@example.org"], "the set writes to three people by address");
    assert_eq!(calendar::reached(&plan), 4, "and to the room, which has none");

    // The command's own count: what it asks about and what it refuses unasked.
    assert_eq!(calendar::mail_of(&f.s, "calendar.delete", &batch), Some(Mail::Cancellation(4)), "the count is the plan's");
    let mut probe = Session::at(f.s.mailbox.clone(), NOW, 0);
    let refused = probe.run("calendar.delete", &batch).expect_err("a cancellation isn't sent unasked").to_string();
    assert!(refused.contains("4 people"), "the refusal counts the plan's people: {refused}");

    // The agent's question names exactly those people, and nobody else.
    let asked = json!({"items": [{"id": first}, {"id": other}], "send": true});
    let what = agent::describe(&f.s, "calendar.delete", &asked);
    for person in &people {
        assert!(what.contains(person), "the agent's question names {person}: {what}");
    }
    assert!(what.contains("4 people") && what.contains("1 person without an address"), "and counts them as the command does: {what}");
}

#[test]
fn a_cut_counts_asks_and_names_the_same_people() {
    let mut f = fixture();
    let account = second(&f);
    let first = kept_meeting(&mut f, "Planning", 9, "e-planning", &[("Ann Example", "ann@example.org"), ("Room", "")]);
    let other = kept_meeting(&mut f, "Retro", 11, "e-retro", &[("Ann Again", "Ann@Example.org"), ("Cy Example", "cy@example.org")]);
    let cut = json!({"items": [{"id": first}, {"id": other}], "start": START, "cut": true});

    let plan = calendar::paste_notices(&f.s, &cut).expect("a cut of named items");
    assert!(plan.iter().all(|notice| notice.kind == NoticeKind::Update && notice.account == account), "updates, by the second account: {plan:?}");
    assert_eq!(calendar::recipients(&plan), ["ann@example.org", "cy@example.org"], "two people by address, each once");
    assert_eq!(calendar::reached(&plan), 3, "and the room without one");
    assert_eq!(calendar::mail_of(&f.s, "calendar.paste", &cut), Some(Mail::Update(3)), "the count is the plan's");
    let mut probe = Session::at(f.s.mailbox.clone(), NOW, 0);
    let refused = probe.run("calendar.paste", &cut).expect_err("an update isn't sent unasked").to_string();
    assert!(refused.contains("3 people"), "the refusal counts the plan's people: {refused}");

    let what = agent::describe(&f.s, "calendar.paste", &json!({"items": [{"id": first}, {"id": other}], "start": START, "cut": true, "send": true}));
    assert!(what.contains("ann@example.org") && what.contains("cy@example.org"), "the agent's question names both: {what}");

    // A copy writes to nobody.
    let copy = json!({"items": [{"id": first}, {"id": other}], "start": START});
    assert_eq!(calendar::paste_notices(&f.s, &copy), Some(Vec::new()), "a copy is a new appointment with nobody in it");
}
