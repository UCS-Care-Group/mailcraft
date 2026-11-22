// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Regressions for what an agent may do with a *set* of calendar items: how a batch is
//! classified, what the owner's question names, and what a malformed, ambiguous or stale batch
//! may not do.
//!
//! Nothing here is production code, and nothing here calls a private helper: every case goes
//! through the public [`mailcraft_engine::agent`] gate the window and the MCP server use.
//!
//! Fixtures only: synthetic people on `example.org`, the engine calendar tests' fixed clock, and
//! events added through the public `Mailbox::add_event`. No files, servers or wall clock.

use mailcraft_engine::Session;
use mailcraft_engine::agent::{self, Access, Class, Verdict};
use mailcraft_engine::model::{Address, Attendee, Calendar, Event, Freq, Id, Origin, Recurrence};
use mailcraft_engine::time::{Date, DateTime};
use serde_json::{Value, json};

/// Wednesday 2026-10-14 16:00 UTC: the fixed clock the engine's calendar tests use.
const NOW: i64 = 1_791_993_600;
/// Where a paste is asked to put its items.
const START: &str = "2026-12-01T09:00";

fn d(year: i32, month: u8, day: u8) -> Date {
    Date::new(year, month, day).expect("a date that exists")
}

fn at(day: Date, hour: u8, minute: u8) -> DateTime {
    DateTime::new(day, hour, minute, 0).expect("a time of day that exists")
}

/// The appointments a batch is made of, and the ones around them.
struct Fixture {
    s: Session,
    /// Wednesday 2026-11-04 09:00–09:30, weekly: 11-04, 11-11, 11-18 …
    series: Id,
    /// A meeting of its own: Alice is invited.
    kickoff: Id,
    /// Another meeting: Bo is invited.
    review: Id,
    /// What the owner may have focused instead: no batch names it.
    unrelated: Id,
    /// A calendar of another account, kept on that account's server.
    remote: Id,
}

fn add(s: &mut Session, title: &str, start: DateTime, minutes: i64, who: &[(&str, &str)]) -> Id {
    let calendar = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    let attendees: Vec<Attendee> =
        who.iter().map(|(name, address)| Attendee { address: Address::new(*name, *address), ..Default::default() }).collect();
    let event = Event { title: title.to_string(), start, end: start.add_minutes(minutes), calendar, attendees, ..Default::default() };
    s.mailbox.add_event(event).expect("a local calendar takes an event")
}

fn fixture() -> Fixture {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let calendar = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    let start = at(d(2026, 11, 4), 9, 0);
    let series = s
        .mailbox
        .add_event(Event {
            title: "Weekly sync".into(),
            start,
            end: start.add_minutes(30),
            calendar,
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            ..Default::default()
        })
        .expect("a local calendar takes a series");
    let kickoff = add(&mut s, "Kickoff", at(d(2026, 11, 9), 9, 0), 45, &[("Alice Example", "alice@example.org")]);
    let review = add(&mut s, "Review", at(d(2026, 11, 11), 10, 15), 45, &[("Bo Example", "bo@example.org")]);
    let unrelated = add(&mut s, "Unrelated dental", at(d(2026, 11, 10), 14, 0), 30, &[("Eve Example", "eve@example.org")]);
    // A second account, and a calendar its server keeps: pasting there reaches a server other
    // than the source's, as moving mail there does.
    let mut second = s.mailbox.accounts[0].clone();
    second.id = second.id.saturating_add(1_000);
    second.email = "second@example.org".into();
    second.name = "second@example.org".into();
    let account = second.id;
    s.mailbox.accounts.push(second);
    s.mailbox.repair();
    let id = s.mailbox.calendars.iter().map(|c| c.id).max().unwrap_or(0).saturating_add(1);
    let origin = Origin { account, id: "c-second".into(), ..Default::default() };
    s.mailbox.calendars.push(Calendar { id, name: "Second account".into(), visible: true, origin, ..Default::default() });
    Fixture { s, series, kickoff, review, unrelated, remote: id }
}

/// Reading what the calendar shows is not a change to the mailbox: selecting a set is Safe.
#[test]
fn select_all_and_a_batch_paste_are_classified_for_agents() {
    let f = fixture();
    let s = &f.s;
    assert_eq!(
        agent::access("calendar.selectAll"),
        Some(Access::Safe),
        "`calendar.selectAll` must have a class: it changes the selection and nothing else"
    );
    assert_eq!(agent::classify(s, "calendar.selectAll", &json!({})), Access::Safe);
    assert_eq!(agent::classify(s, "calendar.selectAll", &json!({"extend": false})), Access::Safe);

    // A copy is calendar.copy's: a new appointment of its own, inviting nobody.
    let copy = json!({"items": [{"id": f.kickoff}], "start": START});
    assert!(agent::access("calendar.paste").is_some(), "`calendar.paste` must have a class: an unclassified batch is a bypass, not a guard");
    assert_eq!(agent::classify(s, "calendar.paste", &copy), Access::Safe, "copying an appointment invites nobody");
    assert_eq!(agent::check(s, "calendar.paste", &copy, &[]), Verdict::Run, "an agent may copy without a question");

    // Every item is named, strictly: there is no ids-only shorthand for a batch.
    assert!(
        matches!(agent::check(s, "calendar.paste", &json!({"ids": [f.kickoff], "start": START}), &[]), Verdict::Refuse(_)),
        "a batch names its items, never `ids`"
    );
    assert!(
        matches!(agent::check(s, "calendar.paste", &json!({"start": START}), &Class::ALL), Verdict::Refuse(_)),
        "a programmatic paste names its items: it never takes the owner's selection"
    );
}

/// A cut takes the appointments away, and a paste onto another account's server puts them there:
/// both need the owner, as a send does.
#[test]
fn a_cut_or_a_paste_onto_another_server_needs_the_owner() {
    let f = fixture();
    let s = &f.s;
    let cut = json!({"items": [{"id": f.kickoff}, {"id": f.review}], "start": START, "cut": true});
    let class = agent::classify(s, "calendar.paste", &cut);
    assert!(matches!(class, Access::Owner(_)), "a cut needs the owner's yes, never a silent move: {class:?}");
    assert!(matches!(agent::check(s, "calendar.paste", &cut, &[]), Verdict::Ask(..)), "so the owner is asked about it");

    // Onto a calendar another account's server keeps.
    let there = json!({"items": [{"id": f.kickoff}], "start": START, "calendar": f.remote});
    assert_eq!(
        agent::classify(s, "calendar.paste", &there),
        Access::Owner(Class::Send),
        "a destination another account's server keeps reaches that server: asked as a send"
    );

    // And what is to be sent to the people invited is a send, as any calendar command's is.
    assert_eq!(
        agent::classify(s, "calendar.paste", &json!({"items": [{"id": f.kickoff}], "start": START, "send": true})),
        Access::Owner(Class::Send)
    );
    // Nothing of it is hidden: a plain copy stays Safe, and declining tells nobody unless `send`.
    assert_eq!(agent::classify(s, "calendar.paste", &json!({"items": [{"id": f.kickoff}], "start": START})), Access::Safe);
    assert_eq!(
        agent::classify(s, "calendar.delete", &json!({"items": [{"id": f.kickoff}], "decline": true})),
        Access::Owner(Class::Delete),
        "declining on the owner's behalf tells nobody unless `send` says so"
    );
    assert_eq!(
        agent::classify(s, "calendar.delete", &json!({"items": [{"id": f.kickoff}], "send": true})),
        Access::Owner(Class::Send),
        "cancelling a meeting sends the people invited the cancellation"
    );
}

/// The question names every item of the batch and the people of those items, never the
/// appointment the owner happens to have focused.
#[test]
fn the_question_names_every_explicit_item_and_its_people_not_the_ui_focus() {
    let mut f = fixture();
    // A cancellation only reaches invitees of a server-kept meeting. These two used to be
    // local appointments, which made the assertions below demand mail the command never sends.
    let keeper =
        f.s.run("account.add", &json!({"email": "keeper@example.org", "incoming": {"host": "imap.example.org"}})).expect("a synthetic keeper");
    let account = keeper["id"].as_u64().expect("the keeper id");
    for id in [f.kickoff, f.review] {
        let event = f.s.mailbox.event_mut(id).expect("the fixture appointment");
        event.origin = Origin { account, id: format!("stored-{id}"), ..Default::default() };
        event.organizer = Some(Address::new("Keeper", "keeper@example.org"));
    }
    assert_eq!(
        mailcraft_engine::cmd::calendar::mail_of(&f.s, "calendar.delete", &json!({"items": [{"id": f.kickoff}, {"id": f.review}]})),
        Some(mailcraft_engine::cmd::calendar::Mail::Cancellation(2)),
        "fixture: the two kept meetings actually require cancellation mail to both invitees"
    );
    f.s.run("calendar.select", &json!({"id": f.unrelated})).expect("the owner has an appointment selected");
    let s = &f.s;

    let paste = agent::describe(s, "calendar.paste", &json!({"items": [{"id": f.kickoff}, {"id": f.review}], "start": START}));
    assert!(paste.contains("Kickoff") && paste.contains("Review"), "the question names both appointments it would paste: {paste}");
    assert!(!paste.contains("Unrelated"), "and nothing the owner has focused: {paste}");

    let delete = agent::describe(s, "calendar.delete", &json!({"items": [{"id": f.kickoff}, {"id": f.review}]}));
    assert!(delete.contains("Kickoff") && delete.contains("Review"), "the question names both appointments it would delete: {delete}");
    assert!(!delete.contains("Unrelated"), "and nothing the owner has focused: {delete}");

    // The people: everybody the batch would write to, from every item, and nobody of the focus.
    let send = agent::describe(s, "calendar.delete", &json!({"items": [{"id": f.kickoff}, {"id": f.review}], "send": true}));
    assert!(send.contains("alice@example.org") && send.contains("bo@example.org"), "the cancellation names every person it is sent to: {send}");
    assert!(!send.contains("eve@example.org"), "and nobody the batch doesn't name: {send}");
}

/// Two occurrences of one series can't be asked about in the same words: the owner has to be
/// told which day would go.
#[test]
fn a_question_about_one_occurrence_of_a_series_says_which_day() {
    let f = fixture();
    let s = &f.s;
    let first = agent::describe(s, "calendar.delete", &json!({"items": [{"id": f.series, "day": "2026-11-11"}]}));
    let second = agent::describe(s, "calendar.delete", &json!({"items": [{"id": f.series, "day": "2026-11-18"}]}));
    assert!(first.contains("Weekly sync"), "the question names the series: {first}");
    assert_ne!(first, second, "the owner isn't told which of the two occurrences would go:\n  {first}\n  {second}");
}

/// A malformed or missing list is refused, never read as the owner's selection, and a day that
/// isn't an occurrence of the series is refused too.
#[test]
fn malformed_or_missing_items_are_refused_and_never_borrow_the_focus() {
    let mut f = fixture();
    f.s.run("calendar.select", &json!({"id": f.kickoff})).expect("the owner has the first appointment selected");
    let s = &f.s;
    assert!(
        matches!(agent::check(s, "calendar.delete", &json!({"items": [{"id": f.review}]}), &[]), Verdict::Ask(..)),
        "one item named is asked about, as any delete is"
    );
    // A call that names nothing keeps its one-item meaning — the appointment in focus — so that
    // the older single-target path is not broken by the batch.
    assert!(
        matches!(agent::check(s, "calendar.delete", &json!({}), &[]), Verdict::Ask(..)),
        "a delete that names nothing is still about the appointment in focus"
    );
    let many: Vec<Value> = (1..=201).map(|id| json!({"id": id})).collect();
    for bad in [
        json!({"items": []}),
        json!({"items": [{}]}),
        json!({"items": "1,2"}),
        json!({"items": [{"id": 0}]}),
        json!({"items": [{"id": "7"}]}),
        json!({"items": [{"id": f.review, "when": "soon"}]}),
        json!({"id": f.review, "items": [{"id": f.review}]}),
        json!({"items": [{"id": f.series, "day": "next week"}]}),
        json!({"items": [{"id": f.series, "day": "2026-11-06"}]}),
        json!({"items": many}),
    ] {
        let asked = agent::check(s, "calendar.delete", &bad, &[]);
        assert!(matches!(asked, Verdict::Refuse(_)), "`calendar.delete` with {bad} must be refused, never asked about as the selection: {asked:?}");
        let said = agent::describe(s, "calendar.delete", &bad);
        assert!(!said.contains("Kickoff"), "and it names nothing of the owner's: {said}");
    }
    // A recurring item is pasted as a concrete occurrence, never as its series.
    let pasted = agent::check(s, "calendar.paste", &json!({"items": [{"id": f.series}], "start": START}), &[]);
    assert!(matches!(pasted, Verdict::Refuse(_)), "a series isn't pasted whole: {pasted:?}");
}

/// A batch may not act on the part of itself that is still there: a member that is gone refuses
/// the whole batch, and the appointment the window happened to have selected is not deleted.
#[test]
fn a_batch_that_names_a_member_that_is_gone_is_refused_whole() {
    let Fixture { mut s, kickoff, review, .. } = fixture();
    s.run("calendar.select", &json!({"id": kickoff})).expect("the owner has an appointment selected");
    s.run("calendar.delete", &json!({"id": review})).expect("the review is gone before a batch names it");
    let before = s.mailbox.clone();
    s.agent = Some(vec![Class::Delete]);
    let refused = s.run("calendar.delete", &json!({"items": [{"id": kickoff}, {"id": review}]}));
    s.agent = None;
    assert!(refused.is_err(), "a member that is gone refuses the batch: {refused:?}");
    assert!(s.mailbox == before, "and nothing of it was deleted");
    assert!(s.mailbox.event(kickoff).is_some(), "least of all the appointment that was there");
}

/// The owner's yes reaches the items the question showed: a member that went while the owner was
/// being asked refuses the whole batch, rather than a smaller batch running unreviewed.
#[test]
fn an_answer_for_a_cut_whose_member_went_refuses_the_whole_batch() {
    let Fixture { mut s, kickoff, review, .. } = fixture();
    let cut = json!({"items": [{"id": kickoff}, {"id": review}], "start": START, "cut": true});
    let what = agent::describe(&s, "calendar.paste", &cut);
    let shown = agent::bind::Shown::new(&s, "calendar.paste", &cut, &what).expect("a paste is about the appointments it moves");
    // One of them goes while the question is up.
    s.run("calendar.delete", &json!({"id": review})).expect("the owner may delete it");
    assert!(s.mailbox.event(review).is_none());
    assert!(
        shown.check(&s, "calendar.paste", &cut).is_err(),
        "the question pinned two appointments and one is gone: nothing runs, and no smaller batch either"
    );
}

#[test]
fn an_explicit_batch_approval_is_not_retargeted_or_cancelled_by_an_unrelated_selection() {
    let mut f = fixture();
    let cut = json!({"items": [{"id": f.kickoff}, {"id": f.review}], "start": START, "cut": true});
    let what = agent::describe(&f.s, "calendar.paste", &cut);
    let shown = agent::bind::Shown::new(&f.s, "calendar.paste", &cut, &what).expect("the explicit appointments are pinned");
    f.s.run("calendar.select", &json!({"id": f.unrelated})).unwrap();
    assert_eq!(
        shown.check(&f.s, "calendar.paste", &cut),
        Ok(agent::bind::Reach::All),
        "the question reaches its explicit items, not the new selection"
    );
}

#[test]
fn an_approval_refuses_a_changed_source_even_when_its_id_and_title_survive() {
    let mut f = fixture();
    let cut = json!({"items": [{"id": f.kickoff}, {"id": f.review}], "start": START, "cut": true});
    let what = agent::describe(&f.s, "calendar.paste", &cut);
    let shown = agent::bind::Shown::new(&f.s, "calendar.paste", &cut, &what).expect("the source appointments are pinned");
    let event = f.s.mailbox.event_mut(f.review).unwrap();
    event.start = event.start.add_minutes(30);
    event.end = event.end.add_minutes(30);
    assert!(shown.check(&f.s, "calendar.paste", &cut).is_err(), "a changed relative placement must be shown again, not silently approved");
}
