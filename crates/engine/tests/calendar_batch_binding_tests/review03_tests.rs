// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the third review of #1665 found the batch plan and the binding of the owner's yes still
//! let through:
//! - somebody invited without an address was counted as nobody, while the host's rule
//!   (`remote::permitted`) counts every attendee as somebody the server may write to: a set of
//!   meetings whose people have no address asked nothing, and the host then held the change back
//!   and put the meetings back in the calendar; with `"send": true` the server was told to cancel
//!   while the question named nobody;
//! - a yes for one day of a series, asked about as a typed set, was held to the set's items but
//!   not to the Delete's form, so a pure `calendar.select` that turned it into the older focus on
//!   the same day (whose Delete takes the whole series) was refused only by the question's words;
//! - the answer a Delete gives, which its params doc described wrongly.
//!
//! One module of `calendar_batch_binding_tests` (`use super::*`: `fixture()`, `d`, `at`, `NOW`,
//! `START`). Synthetic people on `example.org`; the second account's server is a name nobody
//! contacts.

use super::*;

use mailcraft_engine::cmd::calendar::{self, Mail, NoticeKind};
use mailcraft_engine::cmd::remote;

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
            organizer: Some(Address::new("Second Example", "second@example.org")),
            attendees,
            origin: Origin { account, id: remote.into(), ..Default::default() },
            ..Default::default()
        })
        .expect("fixture: a server calendar takes a meeting")
}

/// The server ids the next upload of `account` cancels (tells the people invited of), sorted.
fn cancelled_on_the_server(s: &Session, account: Id) -> Vec<String> {
    let upload = remote::event_upload(s, account).expect("an upload can be built");
    let mut cancels: Vec<String> = upload.ops.iter().filter(|op| op.cancel).map(|op| op.remote.clone()).collect();
    cancels.sort();
    cancels
}

/// A typed set of meetings whose people have no address is asked about before it is deleted, as
/// the single Delete is, and the owner's yes then goes to the server: it is not held back for want
/// of a yes and the meetings don't come back into the calendar.
#[test]
fn a_set_of_meetings_whose_people_have_no_address_asks_first_and_then_goes_through() {
    let mut f = fixture();
    let account = second(&f);
    let room = kept_meeting(&mut f, "Room booking", 9, "e-room", &[("Room four", "")]);
    let lines = kept_meeting(&mut f, "Dial-in", 11, "e-dial", &[("Line one", ""), ("Line two", "  ")]);
    f.s.run("calendar.select", &json!({"items": [{"id": room}, {"id": lines}]})).expect("the owner's typed set");

    // Three people invited, none of them with an address: three people the server may write to.
    let plan = calendar::delete_notices(&f.s, &json!({})).expect("the selected set is a set");
    assert_eq!(plan.len(), 2, "each meeting writes to somebody: {plan:?}");
    assert!(plan.iter().all(|notice| notice.kind == NoticeKind::Cancellation && notice.to.is_empty()), "nobody by address: {plan:?}");
    assert_eq!(plan.iter().map(|notice| notice.unnamed).collect::<Vec<_>>(), [1, 2], "each attendee without an address is somebody");
    assert_eq!(calendar::mail_of(&f.s, "calendar.delete", &json!({})), Some(Mail::Cancellation(3)), "the set is asked about");

    // Unasked, nothing is changed, as the single Delete refuses it.
    let refused = f.s.run("calendar.delete", &json!({})).expect_err("a cancellation isn't sent unasked").to_string();
    assert!(refused.contains("3 people"), "the refusal counts them: {refused}");
    assert!(f.s.mailbox.event(room).is_some() && f.s.mailbox.event(lines).is_some(), "and nothing was deleted");
    let mut single = Session::at(f.s.mailbox.clone(), NOW, 0);
    assert!(single.run("calendar.delete", &json!({"id": room})).is_err(), "as the single Delete refuses one of them");
    let mut one = Session::at(f.s.mailbox.clone(), NOW, 0);
    let refused = one.run("calendar.delete", &json!({"items": [{"id": room}]})).expect_err("a set of one asks as one meeting does").to_string();
    assert!(refused.contains("1 person"), "{refused}");

    // With the owner's yes, both go, and both cancellations go to the server.
    f.s.run("calendar.delete", &json!({"send": true})).expect("with the yes the set is deleted");
    assert!(f.s.mailbox.event(room).is_none() && f.s.mailbox.event(lines).is_none(), "both meetings went");
    assert_eq!(cancelled_on_the_server(&f.s, account), ["e-dial", "e-room"], "and the server is told of both: nothing is held back");
}

/// A cut that moves such a meeting where its server keeps it is asked about too, and with the yes
/// the new time goes to the server.
#[test]
fn a_cut_of_a_meeting_whose_people_have_no_address_asks_first_and_then_goes_through() {
    let mut f = fixture();
    let account = second(&f);
    let room = kept_meeting(&mut f, "Room booking", 9, "e-room", &[("Room four", "")]);
    let cut = json!({"items": [{"id": room}], "start": START, "cut": true});
    assert_eq!(calendar::mail_of(&f.s, "calendar.paste", &cut), Some(Mail::Update(1)), "the cut is asked about");
    let before = f.s.mailbox.event(room).map(|event| event.start);
    let refused = f.s.run("calendar.paste", &cut).expect_err("an update isn't sent unasked").to_string();
    assert!(refused.contains("1 person"), "{refused}");
    assert_eq!(f.s.mailbox.event(room).map(|event| event.start), before, "and nothing moved");

    f.s.run("calendar.paste", &json!({"items": [{"id": room}], "start": START, "cut": true, "send": true})).expect("with the yes it moves");
    let upload = remote::event_upload(&f.s, account).expect("an upload can be built");
    // (START, 2026-12-01 09:00 by this session's clock, which is UTC's.)
    let new_start: i64 = 1_796_115_600;
    assert!(
        upload.ops.iter().any(|op| op.remote == "e-room" && op.fields.time.is_some_and(|time| time.start == new_start)),
        "and its new time goes to the server: {:?}",
        upload.ops.iter().map(|op| (&op.remote, op.fields.time)).collect::<Vec<_>>()
    );
}

/// The agent's question names the people without an address too, among the people it counts, and
/// the yes it is given approves the cancellation the server sends them.
#[test]
fn an_agents_question_names_the_people_without_an_address() {
    let mut f = fixture();
    let account = second(&f);
    let room = kept_meeting(&mut f, "Room booking", 9, "e-room", &[("Room four", "")]);
    let mixed = kept_meeting(&mut f, "Planning", 11, "e-plan", &[("Ann Example", "ann@example.org"), ("Line one", "")]);

    let alone = json!({"items": [{"id": room}], "send": true});
    let what = agent::describe(&f.s, "calendar.delete", &alone);
    assert!(what.contains("1 person without an address"), "the question says who is written to: {what}");
    assert_eq!(agent::classify(&f.s, "calendar.delete", &alone), Access::Owner(Class::Send), "and it is a question about sending");

    let both = json!({"items": [{"id": room}, {"id": mixed}], "send": true});
    let what = agent::describe(&f.s, "calendar.delete", &both);
    assert!(what.contains("3 people"), "Ann and two people without an address: {what}");
    assert!(what.contains("ann@example.org") && what.contains("2 people without an address"), "named as they are: {what}");

    f.s.run("calendar.delete", &both).expect("the yes is given");
    assert_eq!(cancelled_on_the_server(&f.s, account), ["e-plan", "e-room"], "both cancellations are approved");
}

/// A yes to deleting one day of a series, asked about as part of a typed set, never reaches a
/// Delete that now takes the whole series: a pure `calendar.select` that turns the set into the
/// older focus on that day changes how the Delete reads it (a set, then the one event in focus:
/// of a series, all of it), and the yes is retracted. The set is of two, so that the first
/// Delete really is a set's (a set of one in focus is already the older Delete, cal-sec N3).
#[test]
fn a_yes_for_one_day_of_a_series_never_reaches_a_delete_of_the_whole_series() {
    for params in [json!({}), json!({"send": true})] {
        let mut f = fixture();
        let set = json!({"items": [{"id": f.unrelated}, {"id": f.series, "day": "2026-11-11"}]});
        f.s.run("calendar.select", &set).expect("one day of the series and another appointment, as a typed set");
        assert_eq!(calendar::delete_form(&f.s.sel, &params).ok(), Some(calendar::DeleteForm::Batch), "fixture: a set's Delete");
        let what = agent::describe(&f.s, "calendar.delete", &params);
        assert!(!what.contains("all of its occurrences"), "fixture: the question is about one day: {what}");
        let shown = agent::bind::Shown::new(&f.s, "calendar.delete", &params, &what).expect("the question is pinned");
        assert!(shown.check(&f.s, "calendar.delete", &params).is_ok(), "fixture: untouched, the yes is answerable");
        // The agent's own pure command while the owner reads the question.
        f.s.run("calendar.select", &json!({"id": f.series, "day": "2026-11-11"})).expect("the older focus, on the same day");
        assert_eq!(calendar::delete_form(&f.s.sel, &params).ok(), Some(calendar::DeleteForm::One), "fixture: the Delete now takes one event");
        assert!(agent::describe(&f.s, "calendar.delete", &params).contains("all of its occurrences"), "fixture: which is the whole series now");
        assert!(shown.check(&f.s, "calendar.delete", &params).is_err(), "{params}: the yes must not reach a Delete of the whole series");
    }
}

/// What `calendar.delete`'s help (which agents and MCP read) says of a typed selection is what the
/// command does (cal-corr N1 on #1665): two or more are a set; one that has the focus (what a
/// click selects) is the older Delete of that event, of a series the whole of it unless
/// `"scope": "occurrence"` names the day.
#[test]
fn the_help_says_what_a_selection_of_one_in_focus_deletes() {
    let f = fixture();
    let doc = f.s.registry.get("calendar.delete").map(|spec| spec.params).unwrap_or_default();
    assert!(!doc.contains("even when it holds one item"), "the help no longer says a selection of one is a set: {doc}");
    assert!(doc.contains("a typed selection of two or more"), "{doc}");
    assert!(doc.contains("a typed selection of one item that has the focus (what a click selects)"), "{doc}");
    assert!(doc.contains(r#"of a series, the whole series unless "scope": "occurrence" names that day"#), "{doc}");
}

/// What a Delete answers, by its form: a set gives its count and its items; one whole event its
/// id; one day of a series the series' id and the day.
#[test]
fn a_delete_answers_as_its_params_doc_says() {
    let mut f = fixture();
    let set = f.s.run("calendar.delete", &json!({"items": [{"id": f.kickoff}, {"id": f.series, "day": "2026-11-11"}], "send": true})).expect("a set");
    assert_eq!(set["deleted"], json!(2), "a set's count: {set}");
    assert_eq!(set["items"].as_array().map(Vec::len), Some(2), "and its items: {set}");
    let whole = f.s.run("calendar.delete", &json!({"id": f.review, "send": true})).expect("one whole event");
    assert_eq!(whole, json!({"deleted": f.review}), "one whole event's id");
    let day = f.s.run("calendar.delete", &json!({"id": f.series, "scope": "occurrence", "occurrence": "2026-11-18"})).expect("one day");
    assert_eq!(day, json!({"id": f.series, "occurrence": "2026-11-18"}), "one day of a series");
    let doc = f.s.registry.get("calendar.delete").map(|spec| spec.params).unwrap_or_default();
    assert!(doc.contains(r#"one whole event answers {"deleted": id}"#), "the params doc says so: {doc}");
    assert!(doc.contains(r#"one day of a series answers {"id", "occurrence"}"#), "{doc}");
}
