// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #70: an appointment moved between the owner's calendar and a shared mailbox's (or one opened
//! by address): made in the target's mailbox first, the original deleted at its own only once the
//! copy has been seen; a meeting stays where it is; nothing is lost when either half fails.

use mailcraft_model::remote::{
    CalendarsDownload, EventChangeKind, EventOp, EventOutcome, EventResult, EventsDownload, EventsUpload, EventsUploaded, RemoteCalendar,
    RemoteEvent, opened_id,
};
use mailcraft_model::{Address, Attendee, CalendarOwner, Event, Id, Rights};
use serde_json::{Value, json};

use super::{apply_calendars, apply_event_outcomes, apply_events, event_upload, events_waiting, held_for_rights};
use crate::{CmdError, Session};

const NOW: i64 = 1_791_993_600;
const OWNER: &str = "owner@example.org";
const SHARED: &str = "support@example.org";
const OTHER: &str = "help@example.org";

struct Setup {
    s: Session,
    owner: Id,
    shared: Id,
    own_main: Id,
    shared_main: Id,
}

fn calendars_of(account: Id, email: &str, rights: Rights) -> CalendarsDownload {
    let owner = Some(CalendarOwner { address: email.into(), name: "Someone".into() });
    let all = vec![
        RemoteCalendar { id: "main".into(), name: "Calendar".into(), main: true, rights, owner: owner.clone(), ..Default::default() },
        RemoteCalendar { id: "rota".into(), name: "Rota".into(), rights: Rights::Read, owner, ..Default::default() },
    ];
    CalendarsDownload { account, email: email.into(), complete: true, calendars: all, ..Default::default() }
}

fn download(account: Id, email: &str, calendar: &str, events: Vec<RemoteEvent>) -> EventsDownload {
    EventsDownload {
        account,
        email: email.into(),
        calendar: calendar.into(),
        from: NOW - 86_400,
        to: NOW + 7 * 86_400,
        complete: true,
        events,
        ..Default::default()
    }
}

fn event(id: &str, title: &str, start: i64) -> RemoteEvent {
    RemoteEvent {
        id: id.into(),
        change_key: format!("ck-{id}"),
        title: title.into(),
        start,
        end: start + 3_600,
        is_organizer: Some(true),
        ..Default::default()
    }
}

/// The owner's account with an appointment, a shared mailbox (edit rights) with one.
fn setup() -> Setup {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    apply_calendars(&mut s, &calendars_of(owner, OWNER, Rights::Edit)).unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    apply_calendars(&mut s, &calendars_of(shared, SHARED, Rights::Edit)).unwrap();
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600)])).unwrap();
    apply_events(&mut s, &download(shared, SHARED, "", vec![event("theirs", "Theirs", NOW + 7_200)])).unwrap();
    let main = |account: Id| s.mailbox.calendars.iter().find(|c| c.origin.account == account && c.origin.id.is_empty()).map(|c| c.id).unwrap();
    let (own_main, shared_main) = (main(owner), main(shared));
    Setup { s, owner, shared, own_main, shared_main }
}

fn titled(s: &Session, title: &str) -> Event {
    s.mailbox.events.values().find(|e| e.title == title).map(|e| e.as_ref().clone()).unwrap_or_else(|| panic!("no {title}"))
}

fn count(s: &Session, title: &str) -> usize {
    s.mailbox.events.values().filter(|e| e.title == title).count()
}

fn ops(s: &Session, account: Id) -> Vec<EventOp> {
    event_upload(s, account).unwrap().ops
}

/// What the server said to everything in `upload`: a new event is `made-` and its number here.
fn said(upload: &EventsUpload, result: EventResult, error: &str) -> EventsUploaded {
    let outcome = |op: &EventOp| EventOutcome {
        op: op.clone(),
        result,
        remote: match (result, op.kind) {
            (EventResult::Done, EventChangeKind::Create) => format!("made-{}", op.event),
            (EventResult::Done, _) => op.remote.clone(),
            _ => String::new(),
        },
        change_key: if result == EventResult::Done { "ck-new".into() } else { String::new() },
        error: error.into(),
        ..Default::default()
    };
    EventsUploaded { account: upload.account, email: upload.email.clone(), outcomes: upload.ops.iter().map(outcome).collect(), ..Default::default() }
}

fn push(s: &mut Session, account: Id, result: EventResult, error: &str) -> super::EventsApplied {
    let upload = event_upload(s, account).unwrap();
    apply_event_outcomes(s, &said(&upload, result, error)).unwrap()
}

fn refused(s: &mut Session, params: Value) -> String {
    match s.run("calendar.edit", &params) {
        Err(e @ (CmdError::Failed(_) | CmdError::Said(_))) => e.to_string(),
        other => panic!("{params}: expected a refusal, got {other:?}"),
    }
}

fn kinds(s: &Session) -> Vec<(EventChangeKind, Id)> {
    s.mailbox.event_changes.iter().map(|c| (c.kind, c.account)).collect()
}

#[test]
fn an_appointment_goes_into_a_shared_mailbox_copy_first_and_the_original_only_when_the_copy_is_seen() {
    let Setup { mut s, owner, shared, shared_main, own_main } = setup();
    let mine = titled(&s, "Mine");
    let done = s.run("calendar.edit", &json!({"id": mine.id, "calendar": shared_main})).unwrap();
    assert_eq!(done, json!({"id": mine.id, "moved": true}));
    let here = s.mailbox.event(mine.id).unwrap();
    assert_eq!((here.calendar, here.origin.account, here.origin.id.as_str()), (shared_main, shared, ""));
    assert_eq!(kinds(&s), [(EventChangeKind::Create, shared)]);
    // Made at the shared mailbox's address; the owner's own server hears nothing yet.
    let upload = event_upload(&s, shared).unwrap();
    assert_eq!((upload.email.as_str(), upload.ops.len(), upload.ops[0].kind), (SHARED, 1, EventChangeKind::Create));
    assert!(ops(&s, owner).is_empty());
    // The original, still on the owner's server, doesn't come back as a second one.
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600)])).unwrap();
    assert_eq!(count(&s, "Mine"), 1);
    // The server says it made it: the original still stays, and is not deleted on that word.
    apply_event_outcomes(&mut s, &said(&upload, EventResult::Done, "")).unwrap();
    assert_eq!(kinds(&s), [(EventChangeKind::Delete, owner)]);
    assert!(ops(&s, owner).is_empty() && ops(&s, shared).is_empty());
    assert_eq!((events_waiting(&s, owner).due, events_waiting(&s, owner).total), (0, 1));
    // A look at the owner's calendar still shows the original, which is still left out.
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600)])).unwrap();
    assert_eq!(count(&s, "Mine"), 1);
    // The copy is there, in the shared mailbox's calendar: now the original goes, at its own.
    let copy = format!("made-{}", mine.id);
    let seen = vec![event("theirs", "Theirs", NOW + 7_200), RemoteEvent { change_key: "ck-new".into(), ..event(&copy, "Mine", NOW + 600) }];
    apply_events(&mut s, &download(shared, SHARED, "", seen)).unwrap();
    let upload = event_upload(&s, owner).unwrap();
    assert_eq!(upload.email, OWNER);
    assert_eq!(
        upload.ops.iter().map(|op| (op.kind, op.remote.as_str(), op.calendar.as_str())).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, "mine", "")]
    );
    assert!(ops(&s, shared).is_empty(), "nothing more is told to the shared mailbox");
    apply_event_outcomes(&mut s, &said(&upload, EventResult::Done, "")).unwrap();
    assert!(s.mailbox.event_changes.is_empty());
    // A look from before the deletion doesn't bring the original back as a second one.
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600)])).unwrap();
    assert_eq!(count(&s, "Mine"), 1);
    apply_events(&mut s, &download(owner, OWNER, "", Vec::new())).unwrap();
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Mine").map(|e| e.calendar).collect::<Vec<_>>(), [shared_main]);
    assert!(s.mailbox.event_checks.is_empty() && s.mailbox.clone().repair().is_empty());
    let _ = own_main;
}

#[test]
fn an_appointment_comes_out_of_a_shared_mailbox_into_the_owners_calendar_and_between_two_shared_ones() {
    let Setup { mut s, owner, shared, own_main, .. } = setup();
    let theirs = titled(&s, "Theirs");
    s.run("calendar.edit", &json!({"id": theirs.id, "calendar": own_main})).unwrap();
    assert_eq!(kinds(&s), [(EventChangeKind::Create, owner)]);
    // Made at the owner's own server; the shared mailbox hears nothing until the copy is seen.
    let upload = event_upload(&s, owner).unwrap();
    assert_eq!((upload.email.as_str(), upload.ops[0].kind), (OWNER, EventChangeKind::Create));
    assert!(ops(&s, shared).is_empty());
    apply_event_outcomes(&mut s, &said(&upload, EventResult::Done, "")).unwrap();
    let copy = format!("made-{}", theirs.id);
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600), event(&copy, "Theirs", NOW + 7_200)])).unwrap();
    // The original is deleted at the shared mailbox's own address.
    let upload = event_upload(&s, shared).unwrap();
    assert_eq!(
        (upload.email.as_str(), upload.ops.iter().map(|op| (op.kind, op.remote.as_str())).collect::<Vec<_>>()),
        (SHARED, vec![(EventChangeKind::Delete, "theirs")])
    );
    assert!(ops(&s, owner).is_empty());
    apply_event_outcomes(&mut s, &said(&upload, EventResult::Done, "")).unwrap();
    assert!(s.mailbox.event_changes.is_empty());

    // Between two shared mailboxes: made in the second, deleted in the first.
    let other = s.run("account.addSharedMailbox", &json!({"address": OTHER})).unwrap()["id"].as_u64().unwrap();
    apply_calendars(&mut s, &calendars_of(other, OTHER, Rights::Edit)).unwrap();
    let other_main = s.mailbox.calendars.iter().find(|c| c.origin.account == other && c.origin.id.is_empty()).map(|c| c.id).unwrap();
    apply_events(&mut s, &download(shared, SHARED, "", vec![event("again", "Again", NOW + 9_000)])).unwrap();
    let again = titled(&s, "Again");
    s.run("calendar.edit", &json!({"id": again.id, "calendar": other_main})).unwrap();
    assert_eq!(kinds(&s), [(EventChangeKind::Create, other)]);
    assert_eq!(event_upload(&s, other).unwrap().email, OTHER);
    assert!(ops(&s, shared).is_empty() && ops(&s, owner).is_empty());
}

#[test]
fn a_meeting_is_not_moved_to_another_mailbox_and_the_reason_says_what_to_do() {
    let Setup { mut s, owner, shared, shared_main, own_main } = setup();
    let invited = Attendee { address: Address::new("Bo", "bo@example.org"), ..Default::default() };
    let meeting = RemoteEvent { attendees: vec![invited.clone()], ..event("m-1", "Planning", NOW + 3_000) };
    let guest = RemoteEvent {
        organizer: Some(Address::new("Bo", "bo@example.org")),
        attendees: vec![invited],
        is_organizer: Some(false),
        ..event("g-1", "Board", NOW + 4_000)
    };
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600), meeting, guest])).unwrap();
    let snapshot = s.mailbox.clone();
    let why = "A meeting can't be moved to another mailbox's calendar: that would send everyone invited a cancellation, and their replies would no longer reach it. Forward the meeting, or create it again in that calendar.";
    for title in ["Planning", "Board"] {
        let id = titled(&s, title).id;
        assert_eq!(refused(&mut s, json!({"id": id, "calendar": shared_main})), why);
        assert_eq!(refused(&mut s, json!({"id": id, "calendar": shared_main, "send": true})), why);
        assert!(s.mailbox == snapshot, "{title}");
    }
    // Nor out of the shared mailbox.
    let sync = RemoteEvent {
        attendees: vec![Attendee { address: Address::new("Cy", "cy@example.org"), ..Default::default() }],
        ..event("t-1", "Sync", NOW + 5_000)
    };
    apply_events(&mut s, &download(shared, SHARED, "", vec![sync])).unwrap();
    let sync = titled(&s, "Sync").id;
    assert_eq!(refused(&mut s, json!({"id": sync, "calendar": own_main})), why);
    assert!(s.mailbox.event_changes.is_empty());
}

#[test]
fn the_rights_of_both_calendars_are_asked_for_and_what_would_not_arrive_whole_stays() {
    let Setup { mut s, owner, shared, shared_main, own_main } = setup();
    let mine = titled(&s, "Mine").id;
    let theirs = titled(&s, "Theirs").id;
    // Into a calendar that may only be read.
    let rota = s.mailbox.calendars.iter().find(|c| c.name == "Rota" && c.origin.account == shared).map(|c| c.id).unwrap();
    let said = refused(&mut s, json!({"id": mine, "calendar": rota}));
    assert!(said.contains("may only read the shared mailbox's calendar"), "{said}");
    // Out of one that may only be read.
    apply_calendars(&mut s, &calendars_of(shared, SHARED, Rights::Read)).unwrap();
    let said = refused(&mut s, json!({"id": theirs, "calendar": own_main}));
    assert!(said.contains("may only read the shared mailbox's calendar"), "{said}");
    let said = refused(&mut s, json!({"id": mine, "calendar": shared_main}));
    assert!(said.contains("may only read the shared mailbox's calendar"), "{said}");
    assert!(s.mailbox.event_changes.is_empty());
    apply_calendars(&mut s, &calendars_of(shared, SHARED, Rights::Edit)).unwrap();
    // A series and an item with files attached are not carried across.
    let weekly = RemoteEvent {
        kind: mailcraft_model::remote::RemoteEventKind::SeriesMaster,
        recurrence: Some(mailcraft_model::Recurrence::every(mailcraft_model::Freq::Weekly)),
        ..event("s-1", "Gym", NOW + 10_000)
    };
    let files = RemoteEvent { attachments: true, ..event("f-1", "Contract", NOW + 11_000) };
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600), weekly, files])).unwrap();
    let (gym, contract) = (titled(&s, "Gym").id, titled(&s, "Contract").id);
    assert!(refused(&mut s, json!({"id": gym, "calendar": shared_main})).starts_with("A series that the account's server keeps can't be moved"));
    assert!(refused(&mut s, json!({"id": contract, "calendar": shared_main})).starts_with("This item has files attached"));
    // Nothing of this is moved into a calendar that is only on this computer.
    let local = s.mailbox.alloc_id();
    s.mailbox.calendars.push(mailcraft_model::Calendar { id: local, name: "On this computer".into(), visible: true, ..Default::default() });
    assert!(refused(&mut s, json!({"id": mine, "calendar": local})).contains("not out of it"));
    assert!(s.mailbox.event_changes.is_empty());
}

#[test]
fn a_copy_the_server_refuses_leaves_the_original_where_it_was_and_untouched() {
    let Setup { mut s, owner, shared, shared_main, .. } = setup();
    let mine = titled(&s, "Mine");
    s.run("calendar.edit", &json!({"id": mine.id, "calendar": shared_main})).unwrap();
    // Can't be reached: it waits, and nothing is deleted at the owner's.
    let applied = push(&mut s, shared, EventResult::Failed, "no connection");
    assert_eq!(applied.failed, 1);
    assert!(ops(&s, owner).is_empty());
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600)])).unwrap();
    assert_eq!(count(&s, "Mine"), 1);
    s.mailbox.event_changes[0].retry_at = 0;
    // Refused for good: the appointment is back, as it was, in the calendar it was in.
    let applied = push(&mut s, shared, EventResult::Refused, "status 400");
    assert_eq!(applied.refused, 1);
    assert!(applied.messages[0].contains("wouldn't take \"Mine\" into the other calendar"), "{:?}", applied.messages);
    assert_eq!(s.mailbox.event(mine.id), Some(&mine));
    assert!(s.mailbox.event_changes.is_empty() && ops(&s, owner).is_empty());
}

#[test]
fn a_copy_that_is_never_seen_is_taken_back_and_the_original_is_never_deleted() {
    let Setup { mut s, owner, shared, shared_main, .. } = setup();
    let mine = titled(&s, "Mine");
    s.run("calendar.edit", &json!({"id": mine.id, "calendar": shared_main})).unwrap();
    push(&mut s, shared, EventResult::Done, "");
    for _ in 0..3 {
        assert!(ops(&s, owner).is_empty());
        apply_events(&mut s, &download(shared, SHARED, "", vec![event("theirs", "Theirs", NOW + 7_200)])).unwrap();
    }
    assert_eq!(s.mailbox.event(mine.id), Some(&mine), "back where it was");
    assert!(s.mailbox.event_changes.is_empty() && ops(&s, owner).is_empty());
}

#[test]
fn an_original_that_will_not_go_leaves_both_and_says_so() {
    let Setup { mut s, owner, shared, shared_main, .. } = setup();
    let mine = titled(&s, "Mine");
    s.run("calendar.edit", &json!({"id": mine.id, "calendar": shared_main})).unwrap();
    push(&mut s, shared, EventResult::Done, "");
    let copy = format!("made-{}", mine.id);
    let seen = vec![event("theirs", "Theirs", NOW + 7_200), RemoteEvent { change_key: "ck-new".into(), ..event(&copy, "Mine", NOW + 600) }];
    apply_events(&mut s, &download(shared, SHARED, "", seen.clone())).unwrap();
    // Can't be reached: the deletion waits and is tried again; both are on the servers.
    let applied = push(&mut s, owner, EventResult::Failed, "no connection");
    assert_eq!(applied.failed, 1);
    assert_eq!(kinds(&s), [(EventChangeKind::Delete, owner)]);
    s.mailbox.event_changes[0].retry_at = 0;
    // Refused: the copy stays (it is somebody else's mailbox), and the owner is told.
    let applied = push(&mut s, owner, EventResult::Refused, "status 409");
    assert_eq!(applied.refused, 1);
    assert_eq!(applied.messages, ["The server wouldn't take \"Mine\" out of the calendar it was in (status 409). It is in both calendars now."]);
    assert!(s.mailbox.event_changes.is_empty() && ops(&s, shared).is_empty(), "the copy is not taken out of the shared mailbox");
    // The next look at the owner's calendar shows the original again; both are here.
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600)])).unwrap();
    assert_eq!(count(&s, "Mine"), 2);
    apply_events(&mut s, &download(shared, SHARED, "", seen)).unwrap();
    assert_eq!(count(&s, "Mine"), 2);
}

#[test]
fn rights_that_shrink_hold_the_deletion_and_tell_rather_than_drop_it() {
    let Setup { mut s, owner, shared, own_main, .. } = setup();
    let theirs = titled(&s, "Theirs");
    s.run("calendar.edit", &json!({"id": theirs.id, "calendar": own_main})).unwrap();
    push(&mut s, owner, EventResult::Done, "");
    let copy = format!("made-{}", theirs.id);
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600), event(&copy, "Theirs", NOW + 7_200)])).unwrap();
    assert_eq!(ops(&s, shared).len(), 1);
    // The mailbox's owner takes the right to edit away before the deletion went.
    apply_calendars(&mut s, &calendars_of(shared, SHARED, Rights::Read)).unwrap();
    assert!(ops(&s, shared).is_empty());
    assert_eq!(kinds(&s), [(EventChangeKind::Delete, shared)], "kept, not dropped");
    assert_eq!(held_for_rights(&s.mailbox, shared).iter().map(|(_, name, n)| (name.as_str(), *n)).collect::<Vec<_>>(), [("Calendar", 1)]);
    apply_calendars(&mut s, &calendars_of(shared, SHARED, Rights::Edit)).unwrap();
    assert_eq!(ops(&s, shared).len(), 1);
}

#[test]
fn one_undo_puts_a_move_back_before_the_server_is_told_and_after_a_copy_is_made_it_is_taken_out_again() {
    let Setup { mut s, shared, owner, shared_main, .. } = setup();
    let mine = titled(&s, "Mine");
    s.run("calendar.edit", &json!({"id": mine.id, "calendar": shared_main})).unwrap();
    assert!(s.undo());
    assert_eq!(s.mailbox.event(mine.id), Some(&mine));
    assert!(s.mailbox.event_changes.is_empty());
    // Taken back while the copy was on its way: the copy goes again at the shared mailbox's
    // address; the original was never touched.
    s.run("calendar.edit", &json!({"id": mine.id, "calendar": shared_main})).unwrap();
    let upload = event_upload(&s, shared).unwrap();
    assert!(s.undo());
    apply_event_outcomes(&mut s, &said(&upload, EventResult::Done, "")).unwrap();
    let copy = format!("made-{}", mine.id);
    assert_eq!(ops(&s, shared).iter().map(|op| (op.kind, op.remote.clone())).collect::<Vec<_>>(), [(EventChangeKind::Delete, copy)]);
    assert!(ops(&s, owner).is_empty());
    assert_eq!(s.mailbox.event(mine.id).map(|e| (e.origin.account, e.origin.id.as_str())), Some((owner, "mine")));
}

#[test]
fn a_calendar_opened_by_address_takes_an_appointment_but_not_a_meeting() {
    let Setup { mut s, owner, own_main, .. } = setup();
    s.run("calendar.openShared", &json!({"address": "cy@example.org"})).unwrap();
    let cy = RemoteCalendar {
        id: opened_id("cy@example.org"),
        name: "Cy".into(),
        rights: Rights::Edit,
        owner: Some(CalendarOwner { address: "cy@example.org".into(), name: "Cy".into() }),
        ..Default::default()
    };
    let mut list = calendars_of(owner, OWNER, Rights::Edit);
    list.calendars.truncate(1);
    list.calendars.push(cy);
    apply_calendars(&mut s, &list).unwrap();
    let cy_calendar = s.mailbox.calendars.iter().find(|c| c.name == "Cy").map(|c| c.id).unwrap();
    let mine = titled(&s, "Mine");
    s.run("calendar.edit", &json!({"id": mine.id, "calendar": cy_calendar})).unwrap();
    // Made in Cy's calendar (asked for through it), under the owner's sign-in: nothing says /me.
    let upload = event_upload(&s, owner).unwrap();
    assert_eq!(
        upload.ops.iter().map(|op| (op.kind, op.calendar.as_str())).collect::<Vec<_>>(),
        [(EventChangeKind::Create, opened_id("cy@example.org").as_str())]
    );
    assert!(s.undo());
    // A meeting stays.
    let meeting = RemoteEvent {
        attendees: vec![Attendee { address: Address::new("Bo", "bo@example.org"), ..Default::default() }],
        ..event("m-1", "Planning", NOW + 3_000)
    };
    apply_events(&mut s, &download(owner, OWNER, "", vec![event("mine", "Mine", NOW + 600), meeting])).unwrap();
    let planning = titled(&s, "Planning").id;
    assert!(refused(&mut s, json!({"id": planning, "calendar": cy_calendar})).starts_with("A meeting can't be moved to another mailbox's calendar"));
    // And back from it into the owner's own calendar, a meeting still can't be.
    assert!(s.mailbox.event_changes.is_empty());
    let _ = own_main;
}
