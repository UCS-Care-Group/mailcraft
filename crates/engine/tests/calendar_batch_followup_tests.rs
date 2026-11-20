// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Follow-ups to the calendar's multiple selection (#1663): a cut whose all-day items stay on
//! their days, mail already waiting for somebody else's meeting, the older one-event Delete's
//! question, and refusals that come before anything is asked.
//!
//! Synthetic events on `example.org` and `example.net` only, with a fixed clock; no files and no
//! real server. These are integration tests of the public engine API.

use mailcraft_engine::Session;
use mailcraft_engine::agent::{self, Class, Verdict};
use mailcraft_engine::cmd::calendar::{Mail, mail_of};
use mailcraft_engine::time::{Date, DateTime};
use mailcraft_model::remote::{EventChange, EventChangeKind};
use mailcraft_model::{Address, Attendee, Calendar, Event, Freq, Id, Origin, Recurrence, Rgb};
use serde_json::{Value, json};

/// Wednesday 2026-10-14 16:00 UTC: the fixed clock the engine's calendar tests use.
const NOW: i64 = 1_791_993_600;

/// Copy Event's refusal of an item with files attached (#1664).
const FILES: &str = "This item has files attached, which only the server keeps and which a copy would be without: it can't be copied from here.";

fn day(month: u8, date: u8) -> Date {
    Date::new(2026, month, date).expect("a date that exists")
}

fn at(month: u8, date: u8, hour: u8, minute: u8) -> DateTime {
    DateTime::new(day(month, date), hour, minute, 0).expect("a time of day that exists")
}

/// The sample mailbox with an account whose server keeps two calendars.
struct Fixture {
    s: Session,
    account: Id,
    /// The account's calendar.
    server: Id,
    /// Another calendar of the same account.
    side: Id,
    /// The mailbox's own calendar.
    local: Id,
}

fn fixture() -> Fixture {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let local = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    let account = s
        .run("account.add", &json!({"email": "zoe@example.org", "name": "Zoe", "incoming": {"host": "imap.example.org"}}))
        .expect("an account is added")["id"]
        .as_u64()
        .expect("an account id");
    let calendar = |s: &mut Session, name: &str, server_id: &str| {
        let id = s.mailbox.alloc_id();
        s.mailbox.calendars.push(Calendar {
            id,
            name: name.into(),
            color: Rgb(0x2E, 0x8B, 0x57),
            visible: true,
            origin: Origin { account, id: server_id.into(), ..Default::default() },
            ..Default::default()
        });
        id
    };
    let server = calendar(&mut s, "Zoe's", "srv-cal");
    let side = calendar(&mut s, "Zoe's other", "srv-side");
    Fixture { s, account, server, side, local }
}

fn people(who: &[&str]) -> Vec<Attendee> {
    who.iter().map(|email| Attendee { address: Address::new("", *email), ..Default::default() }).collect()
}

/// An event the account's server keeps in `calendar`, with `server_id` there.
fn kept(f: &mut Fixture, calendar: Id, title: &str, server_id: &str, start: DateTime, minutes: i64, who: &[&str]) -> Id {
    let event = Event {
        calendar,
        title: title.into(),
        start,
        end: start.add_minutes(minutes),
        attendees: people(who),
        origin: Origin { account: f.account, id: server_id.into(), ..Default::default() },
        ..Default::default()
    };
    f.s.mailbox.add_event(event).expect("a server calendar takes an event")
}

fn local(f: &mut Fixture, event: Event) -> Id {
    f.s.mailbox.add_event(Event { calendar: f.local, ..event }).expect("a local calendar takes an event")
}

fn whole_day(title: &str, on: Date) -> Event {
    Event { title: title.into(), start: DateTime::at_midnight(on), end: DateTime::at_midnight(on.add_days(1)), all_day: true, ..Default::default() }
}

fn timed(title: &str, start: DateTime, minutes: i64) -> Event {
    Event { title: title.into(), start, end: start.add_minutes(minutes), ..Default::default() }
}

fn refused(s: &mut Session, command: &str, params: &Value) -> String {
    let before = s.mailbox.clone();
    let why = s.run(command, params).expect_err("the command is refused").to_string();
    assert!(s.mailbox == before, "{command} {params}: a refusal changes nothing");
    why
}

// ---- a cut whose all-day items stay on their days (CodeRabbit, selection.rs:484) ----

#[test]
fn a_cut_whose_all_day_item_stays_on_its_day_still_moves_the_timed_one() {
    let mut f = fixture();
    let holiday = local(&mut f, whole_day("Holiday", day(11, 16)));
    let standup = local(&mut f, timed("Stand-up", at(11, 16, 9, 0), 30));
    let before = f.s.mailbox.clone();
    let was = f.s.mailbox.event(holiday).cloned().expect("the fixture's all-day item");
    // The timed item is the anchor: an hour later on the same day leaves the all-day one on its day.
    let done =
        f.s.run("calendar.paste", &json!({"items": [{"id": holiday}, {"id": standup}], "start": "2026-11-16T10:00", "cut": true}))
            .expect("the timed item moves, though the all-day one stays where it is");
    assert_eq!(done["ids"], json!([holiday, standup]), "both are what the paste reached, in the order named");
    assert!(f.s.mailbox.event(standup).is_some_and(|e| e.start == at(11, 16, 10, 0) && e.end == at(11, 16, 10, 30)));
    assert!(f.s.mailbox.event(holiday) == Some(&was), "the all-day item is left exactly as it was");
    assert_eq!(f.s.sel.events.len(), 2, "both stay selected");
    f.s.run("edit.undo", &json!({})).expect("one step of Undo");
    assert!(f.s.mailbox == before, "one Undo puts everything back");
}

#[test]
fn a_cut_where_nothing_would_move_is_still_refused() {
    let mut f = fixture();
    let holiday = local(&mut f, whole_day("Holiday", day(11, 16)));
    let standup = local(&mut f, timed("Stand-up", at(11, 16, 9, 0), 30));
    let why = refused(&mut f.s, "calendar.paste", &json!({"items": [{"id": holiday}, {"id": standup}], "start": "2026-11-16T09:00", "cut": true}));
    assert!(why.contains("already is"), "{why}");
}

#[test]
fn an_item_that_stays_put_in_a_cut_is_neither_changed_on_its_server_nor_written_about() {
    let mut f = fixture();
    let server = f.server;
    let offsite = {
        let mut e = whole_day("Offsite", day(11, 16));
        e.calendar = server;
        e.attendees = people(&["bo@example.org"]);
        e.origin = Origin { account: f.account, id: "srv-off".into(), ..Default::default() };
        f.s.mailbox.add_event(e).expect("a server calendar takes an all-day meeting")
    };
    let review = kept(&mut f, server, "Review", "srv-rev", at(11, 16, 9, 0), 30, &["al@example.org"]);
    let params = json!({"items": [{"id": offsite}, {"id": review}], "start": "2026-11-16T10:00", "cut": true});
    // Only the meeting that moves writes to its people.
    assert_eq!(mail_of(&f.s, "calendar.paste", &params), Some(Mail::Update(1)), "only Review's one person hears of a new time");
    let mut asked = params.clone();
    asked["send"] = json!(true);
    f.s.run("calendar.paste", &asked).expect("Review moves");
    assert!(
        !f.s.mailbox.event_changes.iter().any(|change| change.remote == "srv-off" || change.event == offsite),
        "nothing is noted for the server about the meeting that stays: {:?}",
        f.s.mailbox.event_changes
    );
    assert!(f.s.mailbox.event_changes.iter().any(|change| change.remote == "srv-rev" && change.send), "Review's change goes, with its mail");
}

// ---- mail already waiting (CodeRabbit, selection.rs:618) ----

#[test]
fn a_batch_cut_never_rewrites_an_answer_already_waiting_for_somebody_elses_meeting() {
    let mut f = fixture();
    let server = f.server;
    let review = kept(&mut f, server, "Review", "srv-rev", at(11, 16, 9, 0), 30, &["al@example.org"]);
    let call = {
        let e = Event {
            calendar: server,
            title: "Their call".into(),
            start: at(11, 16, 11, 0),
            end: at(11, 16, 11, 30),
            organizer: Some(Address::new("Org", "org@example.net")),
            attendees: people(&["zoe@example.org"]),
            origin: Origin { account: f.account, id: "srv-guest".into(), guest: true, ..Default::default() },
            ..Default::default()
        };
        f.s.mailbox.add_event(e).expect("a server calendar takes somebody else's meeting")
    };
    // The owner answered it here and chose not to send the answer, with a note kept for later.
    f.s.mailbox.event_changes.push(EventChange {
        account: f.account,
        kind: EventChangeKind::Respond,
        event: call,
        remote: "srv-guest".into(),
        send: false,
        comment: "kept here".into(),
        ..Default::default()
    });
    f.s.run("calendar.paste", &json!({"items": [{"id": review}, {"id": call}], "start": "2026-11-17T09:00", "cut": true, "send": true}))
        .expect("both move; Review's people hear of it");
    let answer = f.s.mailbox.event_changes.iter().find(|change| change.kind == EventChangeKind::Respond).expect("the waiting answer is still there");
    assert!(!answer.send, "the owner's choice not to send the answer is kept: {answer:?}");
    assert_eq!(answer.comment, "kept here", "and so is the note");
    assert!(f.s.mailbox.event_changes.iter().any(|change| change.remote == "srv-rev" && change.send), "Review's update is sent");
}

// ---- the older one-event Delete (CodeRabbit, agent.rs:2122 and agent.rs:2030) ----

fn weekly_meeting(f: &mut Fixture) -> Id {
    let server = f.server;
    let series = kept(f, server, "Weekly review", "srv-series", at(11, 4, 9, 0), 30, &["al@example.org"]);
    let event = f.s.mailbox.event_mut(series).expect("the series");
    event.recurrence = Some(Recurrence::every(Freq::Weekly));
    // Its 11 November is cancelled already.
    event.exceptions.push(day(11, 11));
    series
}

#[test]
fn the_question_about_one_day_of_a_series_names_that_day_and_the_whole_series_says_so() {
    let mut f = fixture();
    let series = weekly_meeting(&mut f);
    let one = agent::describe(&f.s, "calendar.delete", &json!({"id": series, "occurrence": "2026-11-18", "send": true}));
    assert!(one.contains("2026-11-18"), "one day names its day: {one}");
    assert!(one.contains("al@example.org"), "and who is told of it: {one}");
    let all = agent::describe(&f.s, "calendar.delete", &json!({"id": series, "scope": "series", "send": true}));
    assert!(all.contains("all of its occurrences"), "the whole series says it is all of it: {all}");
    assert_ne!(one, all);
    // An appointment that doesn't repeat is named as it always was.
    let lunch = local(&mut f, timed("Lunch", at(11, 20, 12, 0), 60));
    assert_eq!(agent::describe(&f.s, "calendar.delete", &json!({"id": lunch})), "delete \u{201c}Lunch\u{201d} from the calendar");
}

#[test]
fn the_question_about_a_day_the_series_already_skips_tells_of_no_cancellation() {
    let mut f = fixture();
    let series = weekly_meeting(&mut f);
    let params = json!({"id": series, "occurrence": "2026-11-11", "send": true});
    let what = agent::describe(&f.s, "calendar.delete", &params);
    assert!(!what.contains("al@example.org"), "a day already cancelled writes to nobody: {what}");
    // (The command agrees: it does nothing, and notes nothing for the server.)
    let changes = f.s.mailbox.event_changes.len();
    f.s.run("calendar.delete", &params).expect("deleting a day already cancelled does nothing");
    assert_eq!(f.s.mailbox.event_changes.len(), changes);
}

// ---- the older Delete's flags (CodeRabbit, agent.rs:1633) ----

#[test]
fn the_older_delete_reads_a_null_flag_as_the_command_does() {
    let mut f = fixture();
    let lunch = local(&mut f, timed("Lunch", at(11, 20, 12, 0), 60));
    let params = json!({"id": lunch, "decline": null});
    assert_eq!(agent::check(&f.s, "calendar.delete", &params, &[Class::Delete]), Verdict::Run, "the gate takes what the command takes");
    // The typed set is still read strictly.
    let batch = json!({"items": [{"id": lunch}], "decline": null});
    assert!(matches!(agent::check(&f.s, "calendar.delete", &batch, &[Class::Delete]), Verdict::Refuse(_)));
    f.s.run("calendar.delete", &params).expect("the command deletes it");
    assert!(f.s.mailbox.event(lunch).is_none());
}

// ---- calendar.select's shortcut (CodeRabbit, selection.rs:298) ----

#[test]
fn select_with_a_null_id_and_ids_is_refused_and_keeps_the_set() {
    let mut f = fixture();
    let lunch = local(&mut f, timed("Lunch", at(11, 20, 12, 0), 60));
    f.s.run("calendar.select", &json!({"items": [{"id": lunch}]})).expect("a set of one");
    let sel = f.s.sel.events.clone();
    assert!(f.s.run("calendar.select", &json!({"id": null, "ids": [lunch]})).is_err(), "`ids` is no way of naming calendar items");
    assert_eq!(f.s.sel.events, sel, "the set is kept");
    f.s.run("calendar.select", &json!({"id": null, "day": "2026-11-20"})).expect("`id: null` with the older day still selects nothing");
    assert!(f.s.sel.events.is_empty() && f.s.sel.event.is_none());
}

// ---- review-06 nonblocking 3: a series' day with files attached ----

#[test]
fn a_batch_copy_of_a_day_of_a_series_with_files_attached_is_refused() {
    let mut f = fixture();
    let server = f.server;
    let series = kept(&mut f, server, "Board pack", "srv-board", at(11, 4, 9, 0), 60, &[]);
    {
        let event = f.s.mailbox.event_mut(series).expect("the series");
        event.recurrence = Some(Recurrence::every(Freq::Weekly));
        event.has_attachments = true;
    }
    for params in [
        json!({"items": [{"id": series, "day": "2026-11-18"}], "start": "2026-11-20T09:00"}),
        json!({"items": [{"id": series, "day": "2026-11-18"}], "start": "2026-11-20T09:00", "calendar": f.local}),
    ] {
        let why = refused(&mut f.s, "calendar.paste", &params);
        assert!(why.contains(FILES), "{params}: {why}");
    }
}

// ---- review-06 nonblocking 4: refused before the mail question ----

#[test]
fn a_cut_that_cant_go_whole_asks_about_no_mail_and_says_why_it_cant() {
    let mut f = fixture();
    let (server, side) = (f.server, f.side);
    let review = kept(&mut f, server, "Review", "srv-rev", at(11, 16, 9, 0), 30, &["al@example.org"]);
    let contract = kept(&mut f, side, "Contract", "srv-contract", at(11, 16, 11, 0), 60, &[]);
    f.s.mailbox.event_mut(contract).expect("the appointment").has_attachments = true;
    let params = json!({"items": [{"id": review}, {"id": contract}], "start": "2026-11-17T09:00", "cut": true, "calendar": server});
    assert_eq!(mail_of(&f.s, "calendar.paste", &params), None, "nothing is asked about a cut the command refuses whole");
    let why = refused(&mut f.s, "calendar.paste", &params);
    assert!(why.contains("files attached"), "the refusal is the reason it can't go, not a question about mail: {why}");
    // The agent's question says nothing of who would be written to either.
    let what = agent::describe(&f.s, "calendar.paste", &params);
    assert!(!what.contains("al@example.org"), "{what}");
}

/// The refusal of an agent's command `id` that would move an appointment the owner has open with
/// changes that aren't saved (CAL-D3: kept, in the words of a move).
fn moved_refusal(id: &str) -> String {
    format!(
        "`{id}` would move an appointment that is open with changes the owner hasn't saved (to another time or another calendar), and the owner wasn't asked about that; nothing was changed. Try again once the owner has saved or closed its window"
    )
}

/// The refusal of an agent's command `id` that would cancel a day of a series the owner has open
/// with changes that aren't saved, or move one day out of it (in the words of that, W1 on #1665).
fn days_refusal(id: &str) -> String {
    format!(
        "`{id}` would change which days a series has (cancel one of them, or move one out of it as an appointment of its own) while that series is open with changes the owner hasn't saved, and the owner wasn't asked about that; nothing was changed. Try again once the owner has saved or closed its window"
    )
}

/// The refusal of an agent's command `id` that would take such an item away.
fn gone_refusal(id: &str) -> String {
    format!(
        "`{id}` would take away an appointment, a contact or a task the owner is changing in the window and hasn't saved, and the owner wasn't asked about that; nothing was changed. Try again once the owner has saved or closed it"
    )
}

/// An agent's move or change of time of an appointment the owner has open with changes that
/// aren't saved is refused, as the owner chose to keep it, in words that say what it would do:
/// move it, or change which days its series has, each in words of its own (W1 on #1665); "take
/// away" stays for what really goes (benandez on #1665). Nothing is changed either way.
#[test]
fn an_agents_move_of_an_appointment_open_with_unsaved_changes_is_refused_in_the_words_of_a_move() {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let calendar = s.mailbox.default_calendar().expect("the sample has a calendar");
    let start = at(10, 20, 9, 0);
    let mut add = |title: &str, recurrence: Option<Recurrence>| {
        let event = Event { calendar, title: title.into(), start, end: start.add_minutes(30), recurrence, ..Default::default() };
        s.mailbox.add_event(event).expect("the calendar takes an appointment")
    };
    let (lunch, weekly) = (add("Lunch", None), add("Weekly", Some(Recurrence::every(Freq::Weekly))));
    s.owner_changing = vec![lunch, weekly];
    s.agent = Some(Class::ALL.to_vec());
    let tries: Vec<(&str, Value, String)> = vec![
        ("calendar.move", json!({"id": lunch, "start": "2026-10-21T09:00"}), moved_refusal("calendar.move")),
        ("calendar.edit", json!({"id": lunch, "start": "2026-10-20T11:00", "end": "2026-10-20T11:30"}), moved_refusal("calendar.edit")),
        (
            "calendar.move",
            json!({"id": weekly, "start": "2026-10-20T10:00", "scope": "occurrence", "occurrence": "2026-10-27"}),
            days_refusal("calendar.move"),
        ),
        ("calendar.move", json!({"id": weekly, "start": "2026-10-20T10:00", "scope": "series"}), moved_refusal("calendar.move")),
        ("calendar.delete", json!({"id": weekly, "scope": "occurrence", "occurrence": "2026-10-27"}), days_refusal("calendar.delete")),
        ("calendar.delete", json!({"id": lunch}), gone_refusal("calendar.delete")),
    ];
    for (command, params, said) in tries {
        let before = s.mailbox.clone();
        let refused = s.run(command, &params).expect_err("refused while the owner's changes aren't saved");
        assert_eq!(refused.to_string(), said, "{command} {params}");
        assert!(s.mailbox == before, "nothing was changed: {command} {params}");
    }
}

/// An agent's command that does two of those things to two appointments the owner has open with
/// changes that aren't saved is refused in the words of the worse of the two: what goes is said
/// before a day taken out of a series, and that before a move (`Taken`'s order; cal-sec N2 on
/// #1665). Nothing is changed, whichever order they come in.
#[test]
fn the_worse_of_two_is_what_the_refusal_says() {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let calendar = s.mailbox.default_calendar().expect("the sample has a calendar");
    let start = at(10, 20, 9, 0);
    let mut add = |title: &str, recurrence: Option<Recurrence>| {
        let event = Event { calendar, title: title.into(), start, end: start.add_minutes(30), recurrence, ..Default::default() };
        s.mailbox.add_event(event).expect("the calendar takes an appointment")
    };
    let (lunch, weekly) = (add("Lunch", None), add("Weekly", Some(Recurrence::every(Freq::Weekly))));
    s.agent = Some(Class::ALL.to_vec());
    let day = json!({"id": weekly, "day": "2026-10-27"});
    let tries: Vec<(&str, Value, String)> = vec![
        // A day of the series cancelled, and the appointment taken away: what goes.
        ("calendar.delete", json!({"items": [day.clone(), {"id": lunch}]}), gone_refusal("calendar.delete")),
        ("calendar.delete", json!({"items": [{"id": lunch}, day.clone()]}), gone_refusal("calendar.delete")),
        // A day of the series moved out of it, and the appointment moved: the day.
        ("calendar.paste", json!({"items": [day.clone(), {"id": lunch}], "start": "2026-10-21T09:00", "cut": true}), days_refusal("calendar.paste")),
        ("calendar.paste", json!({"items": [{"id": lunch}, day.clone()], "start": "2026-10-21T09:00", "cut": true}), days_refusal("calendar.paste")),
    ];
    for changing in [vec![weekly, lunch], vec![lunch, weekly]] {
        s.owner_changing = changing.clone();
        for (command, params, said) in &tries {
            let before = s.mailbox.clone();
            let refused = s.run(command, params).expect_err("refused while the owner's changes aren't saved");
            assert_eq!(refused.to_string(), *said, "{command} {params} with {changing:?} open");
            assert!(s.mailbox == before, "nothing was changed: {command} {params}");
        }
    }
}
