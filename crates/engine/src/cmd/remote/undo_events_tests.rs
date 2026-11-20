// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Undo and redo put back the events their step changed and nothing else (#1674): what a
//! download brought for the other events since the step stays, and the server is told nothing
//! of them. The server's side is written out by hand, as `mailcraft-net` brings it from
//! Microsoft Graph, and goes in through `sync.applyEvents` as a host's download does.

use mailcraft_model::remote::{EventChangeKind, EventsDownload, RemoteEvent};
use mailcraft_model::{Address, Attendee, Event, Id};
use mailcraft_time::{Date, DateTime, Timestamp, Zone};
use serde_json::{Value, json};

use super::event_upload;
use crate::Session;
use crate::agent::{Class, Verdict};

/// An instant, by the clock in UTC.
fn at(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> Timestamp {
    clock(year, month, day, hour, minute).to_timestamp(0)
}

/// A wall-clock time.
fn clock(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> DateTime {
    DateTime::new(Date::new(year, month, day).unwrap(), hour, minute, 0).unwrap()
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// An event of `minutes` as a server has it, at its first version.
fn remote(id: &str, title: &str, start: Timestamp, minutes: i64) -> RemoteEvent {
    RemoteEvent { id: id.into(), change_key: format!("ck-{id}-1"), title: title.into(), start, end: start + minutes * 60, ..Default::default() }
}

/// All of October 2026 for `account`, complete.
fn october(account: Id, events: Vec<RemoteEvent>) -> EventsDownload {
    EventsDownload {
        account,
        email: "ada@example.org".into(),
        from: at(2026, 10, 1, 0, 0),
        to: at(2026, 11, 1, 0, 0),
        complete: true,
        events,
        ..Default::default()
    }
}

/// A download taken in as a host hands it over; answers what it did, as the command says it.
fn download(s: &mut Session, events: EventsDownload) -> (u64, u64, u64, u64, u64) {
    let merged = run(s, "sync.applyEvents", json!({ "download": events }));
    let n = |name: &str| merged[name].as_u64().unwrap();
    (n("added"), n("updated"), n("removed"), n("unchanged"), n("skipped"))
}

/// The sample mailbox on Wednesday 2026-10-14, 16:00 UTC, four hours behind, with an account
/// that signs in with Microsoft and whose calendar has two events: "Budget review" (`e-1`) and
/// "Dentist" (`e-2`). Answers the session, the account and the two events' ids here.
fn served() -> (Session, Id, Id, Id) {
    let now = at(2026, 10, 14, 16, 0);
    served_by(Session::at(crate::sample::mailbox(now, -240), now, -240))
}

/// [`served`] with this computer's clock in New York (four hours behind UTC that day) as the
/// program runs, rather than a fixed offset: its zone moves the way a computer's does
/// (`Clock::zone`, settled by the next command or frame).
fn served_in_new_york() -> (Session, Id, Id, Id) {
    let now = at(2026, 10, 14, 16, 0);
    let mut s = Session::in_zone(crate::sample::mailbox(now, -240), now, Zone::named("America/New_York").unwrap());
    s.settle_zone();
    assert_eq!(s.mailbox.zone, "America/New_York");
    served_by(s)
}

fn served_by(mut s: Session) -> (Session, Id, Id, Id) {
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    download(&mut s, october(account, vec![budget(), dentist()]));
    let (first, second) = (called(&s, account, "e-1").id, called(&s, account, "e-2").id);
    // What was there before the owner did anything is not a step to undo.
    s.forget_history();
    (s, account, first, second)
}

fn budget() -> RemoteEvent {
    remote("e-1", "Budget review", at(2026, 10, 12, 13, 0), 60)
}

fn dentist() -> RemoteEvent {
    remote("e-2", "Dentist", at(2026, 10, 13, 18, 0), 30)
}

/// "Dentist" as the server has it after it was moved there (on another device), at version `n`.
fn dentist_moved(n: u32) -> RemoteEvent {
    RemoteEvent { change_key: format!("ck-e-2-{n}"), ..remote("e-2", "Dentist (moved)", at(2026, 10, 13, 19, 0), 30) }
}

/// The event that `account`'s server calls `remote`.
fn called(s: &Session, account: Id, remote: &str) -> Event {
    let found = s.mailbox.events.values().find(|e| e.origin.account == account && e.origin.id == remote);
    found.map(|e| e.as_ref().clone()).unwrap_or_else(|| panic!("no event {remote}"))
}

fn title(s: &Session, id: Id) -> Option<String> {
    s.mailbox.event(id).map(|e| e.title.clone())
}

/// What the server is to be told now: each change's kind and the server's name of its event.
fn ops(s: &Session, account: Id) -> Vec<(EventChangeKind, String)> {
    event_upload(s, account).unwrap().ops.into_iter().map(|op| (op.kind, op.remote)).collect()
}

/// The title, start and change key of the event the server calls `remote`.
fn shown(s: &Session, account: Id, remote: &str) -> (String, DateTime, String) {
    let e = called(s, account, remote);
    (e.title, e.start, e.origin.change_key)
}

#[test]
fn undo_keeps_what_a_download_changed_in_another_event() {
    let (mut s, account, first, second) = served();
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    assert_eq!(ops(&s, account), [(EventChangeKind::Update, "e-1".to_string())]);
    // The server moved "Dentist" meanwhile; the change waiting for "Budget review" keeps it here.
    assert_eq!(download(&mut s, october(account, vec![budget(), dentist_moved(2)])), (0, 1, 0, 1, 0));
    assert_eq!(shown(&s, account, "e-2"), ("Dentist (moved)".to_string(), clock(2026, 10, 13, 15, 0), "ck-e-2-2".to_string()));
    assert_eq!(s.undo_label(), Some("Edit Event"));

    assert!(s.undo());
    // The edit is undone, and with it the note for the server.
    assert_eq!(title(&s, first).as_deref(), Some("Budget review"));
    // "Dentist" is as the download left it, with the server's change key: nothing to tell.
    assert_eq!(s.mailbox.event(second).map(|e| e.id), Some(second));
    assert_eq!(shown(&s, account, "e-2"), ("Dentist (moved)".to_string(), clock(2026, 10, 13, 15, 0), "ck-e-2-2".to_string()));
    assert!(ops(&s, account).is_empty() && s.mailbox.event_changes.is_empty());
    // The next download finds everything as the server has it.
    assert_eq!(download(&mut s, october(account, vec![budget(), dentist_moved(2)])), (0, 0, 0, 2, 0));
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn undo_keeps_an_event_a_download_added_and_leaves_out_one_it_removed() {
    let (mut s, account, first, second) = served();
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    // The server has a new event, "Standup", and no longer has "Dentist".
    let standup = remote("e-3", "Standup", at(2026, 10, 15, 13, 0), 15);
    assert_eq!(download(&mut s, october(account, vec![budget(), standup.clone()])), (1, 0, 1, 1, 0));
    let made = called(&s, account, "e-3").id;

    assert!(s.undo());
    assert_eq!(title(&s, first).as_deref(), Some("Budget review"));
    // What came down stays; what the server took away stays away, and nothing is sent for either.
    assert_eq!(s.mailbox.event(made).map(|e| (e.title.as_str(), e.origin.change_key.as_str())), Some(("Standup", "ck-e-3-1")));
    assert!(s.mailbox.event(second).is_none());
    assert!(s.mailbox.events.values().all(|e| e.origin.id != "e-2"));
    assert!(ops(&s, account).is_empty() && s.mailbox.event_changes.is_empty());
    // Nothing is there twice, and nothing comes back.
    assert_eq!(download(&mut s, october(account, vec![budget(), standup])), (0, 0, 0, 2, 0));
    assert_eq!(s.mailbox.events.values().filter(|e| e.origin.account == account).count(), 2);
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn undo_of_a_deletion_brings_back_only_that_event() {
    let (mut s, account, first, second) = served();
    run(&mut s, "calendar.delete", json!({"id": first}));
    assert_eq!(ops(&s, account), [(EventChangeKind::Delete, "e-1".to_string())]);
    download(&mut s, october(account, vec![budget(), dentist_moved(2)]));
    assert!(s.mailbox.event(first).is_none());

    assert!(s.undo());
    assert_eq!(shown(&s, account, "e-1"), ("Budget review".to_string(), clock(2026, 10, 12, 9, 0), "ck-e-1-1".to_string()));
    assert_eq!(s.mailbox.event(second).map(|e| e.origin.change_key.as_str()), Some("ck-e-2-2"));
    assert!(ops(&s, account).is_empty() && s.mailbox.event_changes.is_empty());
}

#[test]
fn redo_after_such_an_undo_does_the_change_again_and_keeps_the_downloads() {
    let (mut s, account, first, second) = served();
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    download(&mut s, october(account, vec![budget(), dentist_moved(2)]));
    assert!(s.undo());
    // The server moves "Dentist" again before the owner redoes the edit.
    let later = RemoteEvent { title: "Dentist (moved twice)".into(), ..dentist_moved(3) };
    assert_eq!(download(&mut s, october(account, vec![budget(), later.clone()])), (0, 1, 0, 1, 0));

    assert!(s.redo());
    assert_eq!(title(&s, first).as_deref(), Some("Mine"));
    assert_eq!(s.mailbox.event(second).map(|e| (e.title.as_str(), e.origin.change_key.as_str())), Some(("Dentist (moved twice)", "ck-e-2-3")));
    // The edit is to go to the server again, and only the edit.
    assert_eq!(ops(&s, account), [(EventChangeKind::Update, "e-1".to_string())]);
    // And undone again, the downloads still stay.
    assert!(s.undo());
    assert_eq!(title(&s, first).as_deref(), Some("Budget review"));
    assert_eq!(title(&s, second).as_deref(), Some("Dentist (moved twice)"));
    assert!(ops(&s, account).is_empty());
    assert_eq!(download(&mut s, october(account, vec![budget(), later])), (0, 0, 0, 2, 0));
}

#[test]
fn steps_undone_one_by_one_each_keep_the_downloads_between_them() {
    let (mut s, account, first, second) = served();
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    download(&mut s, october(account, vec![budget(), dentist_moved(2)]));
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-12T11:00"}));
    let later = RemoteEvent { title: "Dentist (moved twice)".into(), ..dentist_moved(3) };
    download(&mut s, october(account, vec![budget(), later]));

    assert_eq!(s.undo_label(), Some("Move Event"));
    assert!(s.undo());
    assert_eq!(s.mailbox.event(first).map(|e| (e.title.as_str(), e.start)), Some(("Mine", clock(2026, 10, 12, 9, 0))));
    assert_eq!(ops(&s, account), [(EventChangeKind::Update, "e-1".to_string())]);
    assert!(s.undo());
    assert_eq!(s.mailbox.event(first).map(|e| (e.title.as_str(), e.start)), Some(("Budget review", clock(2026, 10, 12, 9, 0))));
    assert_eq!(s.mailbox.event(second).map(|e| (e.title.as_str(), e.origin.change_key.as_str())), Some(("Dentist (moved twice)", "ck-e-2-3")));
    assert!(ops(&s, account).is_empty() && !s.can_undo());
}

#[test]
fn a_change_waiting_for_another_event_stays_through_the_undo() {
    let (mut s, account, first, second) = served();
    run(&mut s, "calendar.edit", json!({"id": second, "title": "Dentist, mine"}));
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    download(&mut s, october(account, vec![budget(), dentist()]));

    assert!(s.undo());
    assert_eq!(title(&s, first).as_deref(), Some("Budget review"));
    assert_eq!(title(&s, second).as_deref(), Some("Dentist, mine"));
    assert_eq!(ops(&s, account), [(EventChangeKind::Update, "e-2".to_string())]);
    assert!(s.undo());
    assert_eq!(title(&s, second).as_deref(), Some("Dentist"));
    assert!(ops(&s, account).is_empty());
}

#[test]
fn what_a_download_confirmed_is_not_asked_for_again_after_an_undo() {
    let (mut s, account, first, second) = served();
    // "Dentist" moved here and told: the server said yes, and a download is to show it.
    run(&mut s, "calendar.move", json!({"id": second, "start": "2026-10-14T09:00"}));
    let upload = event_upload(&s, account).unwrap();
    let outcomes = mailcraft_model::remote::EventsUploaded {
        account,
        email: "ada@example.org".into(),
        outcomes: upload
            .ops
            .iter()
            .map(|op| mailcraft_model::remote::EventOutcome {
                op: op.clone(),
                result: mailcraft_model::remote::EventResult::Done,
                remote: op.remote.clone(),
                change_key: "ck-e-2-2".into(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    run(&mut s, "sync.applyEventChanges", json!({ "result": outcomes }));
    assert_eq!(s.mailbox.event_checks.iter().map(|c| c.event).collect::<Vec<_>>(), [second]);
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    // The download shows "Dentist" as it was sent: nothing is left to check.
    let told = RemoteEvent { change_key: "ck-e-2-2".into(), ..remote("e-2", "Dentist", at(2026, 10, 14, 13, 0), 30) };
    download(&mut s, october(account, vec![budget(), told.clone()]));
    assert!(s.mailbox.event_checks.is_empty());

    assert!(s.undo());
    assert_eq!(title(&s, first).as_deref(), Some("Budget review"));
    assert!(s.mailbox.event_checks.is_empty());
    assert_eq!(s.mailbox.event(second).map(|e| (e.start, e.origin.change_key.as_str())), Some((clock(2026, 10, 14, 9, 0), "ck-e-2-2")));
    assert!(ops(&s, account).is_empty());
    assert_eq!(download(&mut s, october(account, vec![budget(), told])), (0, 0, 0, 2, 0));
}

#[test]
fn a_calendar_of_this_computers_undoes_as_ever_beside_a_download() {
    let (mut s, account, _, second) = served();
    let local = s.mailbox.events.values().find(|e| e.origin.is_local() && e.recurrence.is_none() && e.attendees.is_empty()).map(|e| e.id).unwrap();
    let was = s.mailbox.event(local).cloned().unwrap();
    run(&mut s, "calendar.edit", json!({"id": local, "title": "Mine here"}));
    let made =
        run(&mut s, "calendar.new", json!({"title": "Lunch here", "start": "2026-10-15T12:00", "calendar": was.calendar}))["id"].as_u64().unwrap();
    assert!(s.mailbox.event(made).is_some_and(|e| e.origin.is_local()));
    download(&mut s, october(account, vec![budget(), dentist_moved(2)]));

    assert!(s.undo());
    assert!(s.mailbox.event(made).is_none());
    assert!(s.undo());
    assert_eq!(s.mailbox.event(local), Some(&was));
    assert_eq!(s.mailbox.event(second).map(|e| e.origin.change_key.as_str()), Some("ck-e-2-2"));
    assert!(ops(&s, account).is_empty() && s.mailbox.event_changes.is_empty());
    // Redo brings both back, as they were made.
    assert!(s.redo() && s.redo());
    assert_eq!(title(&s, local).as_deref(), Some("Mine here"));
    assert_eq!(title(&s, made).as_deref(), Some("Lunch here"));
    assert!(s.mailbox.event_changes.is_empty());
}

#[test]
fn undo_of_a_forward_takes_back_the_forward_alone() {
    // A forward changes nothing here: only what the server is to be told.
    let (mut s, account, _, second) = served();
    let invite = |name: &str, email: &str| Attendee { address: Address::new(name, email), ..Default::default() };
    let board = RemoteEvent {
        organizer: Some(Address::new("Bo", "bo@example.org")),
        attendees: vec![invite("Ada", "ada@example.org"), invite("Cy", "cy@example.org")],
        is_organizer: Some(false),
        uid: "uid-g-1".into(),
        ..remote("g-1", "Board", at(2026, 10, 15, 14, 0), 60)
    };
    download(&mut s, october(account, vec![budget(), dentist(), board.clone()]));
    let meeting = called(&s, account, "g-1").id;
    run(&mut s, "calendar.forward", json!({"id": meeting, "to": "dana@example.org", "send": true}));
    assert_eq!(ops(&s, account), [(EventChangeKind::Forward, "g-1".to_string())]);
    download(&mut s, october(account, vec![budget(), dentist_moved(2), board]));

    assert_eq!(s.undo_label(), Some("Forward"));
    assert!(s.undo());
    // Nobody is written to, and "Dentist" is as the server has it.
    assert!(ops(&s, account).is_empty() && s.mailbox.event_changes.is_empty());
    assert_eq!(s.mailbox.event(second).map(|e| e.origin.change_key.as_str()), Some("ck-e-2-2"));
    // Redone, it is to go again.
    assert!(s.redo());
    assert_eq!(ops(&s, account), [(EventChangeKind::Forward, "g-1".to_string())]);
}

#[test]
fn undoing_the_account_takes_its_events_away_with_it() {
    let now = at(2026, 10, 14, 16, 0);
    let mut s = Session::at(crate::sample::mailbox(now, -240), now, -240);
    let local: Vec<Event> = s.mailbox.events.values().map(|e| e.as_ref().clone()).collect();
    let added = run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}));
    let account = added["id"].as_u64().unwrap();
    // Its calendar comes down after the account was added, and then a change of the owner's.
    download(&mut s, october(account, vec![budget(), dentist()]));
    let first = called(&s, account, "e-1").id;
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    download(&mut s, october(account, vec![budget(), dentist_moved(2)]));

    assert!(s.undo());
    assert_eq!(title(&s, first).as_deref(), Some("Budget review"));
    assert_eq!(s.undo_label(), Some("Add Account"));
    // Its events go with it, though the step made none of them: an agent's undo says so.
    assert!(s.undo_removes(false, first));
    assert!(s.undo());
    // Nothing of the account is left: no event of its calendar ends up in another.
    assert!(s.mailbox.account(account).is_none());
    assert!(s.mailbox.events.values().all(|e| e.origin.account != account));
    assert_eq!(s.mailbox.events.values().map(|e| e.as_ref().clone()).collect::<Vec<_>>(), local);
    assert!(s.mailbox.event_changes.is_empty() && s.mailbox.event_checks.is_empty());
    assert!(s.mailbox.clone().repair().is_empty());
}

/// The waiting changes as a list: each one's event and kind, in order.
fn order(s: &Session) -> Vec<(Id, EventChangeKind)> {
    s.mailbox.event_changes.iter().map(|c| (c.event, c.kind)).collect()
}

/// A second change waiting for the event of the first, after the others (as a forward or an
/// occurrence's change would be).
fn noted_again(s: &mut Session) {
    let event = s.mailbox.event_changes.first().map(|c| c.event).unwrap();
    noted_for(s, event);
}

/// A second change waiting for `event`, after the others, as [`noted_again`] notes one.
fn noted_for(s: &mut Session, event: Id) {
    let mut again = s.mailbox.event_changes.iter().find(|c| c.event == event).cloned().unwrap();
    again.kind = EventChangeKind::Forward;
    s.mailbox.event_changes.push(again);
    s.forget_history();
}

/// A calendar event of this computer's alone, with nobody invited and no series.
fn local_event(s: &Session) -> Id {
    s.mailbox.events.values().find(|e| e.origin.is_local() && e.recurrence.is_none() && e.attendees.is_empty()).map(|e| e.id).unwrap()
}

#[test]
fn the_changes_waiting_for_other_events_keep_their_order_through_an_undo() {
    let (mut s, _, first, second) = served();
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    run(&mut s, "calendar.edit", json!({"id": second, "title": "Dentist, mine"}));
    noted_again(&mut s);
    let was = order(&s);
    assert_eq!(was, [(first, EventChangeKind::Update), (second, EventChangeKind::Update), (first, EventChangeKind::Forward)]);
    let local = local_event(&s);
    run(&mut s, "calendar.edit", json!({"id": local, "title": "Mine here"}));

    assert!(s.undo());
    // Not grouped by event: they go to the server in the order they were made.
    assert_eq!(order(&s), was);
}

#[test]
fn the_changes_an_undo_puts_back_go_where_they_stood() {
    let (mut s, _, first, second) = served();
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    run(&mut s, "calendar.edit", json!({"id": second, "title": "Dentist, mine"}));
    noted_again(&mut s);
    let was = order(&s);
    // The change waiting for "Dentist" is changed by the step, between the two of "Budget review".
    run(&mut s, "calendar.edit", json!({"id": second, "title": "Dentist, mine again"}));

    assert!(s.undo());
    assert_eq!(title(&s, second).as_deref(), Some("Dentist, mine"));
    assert_eq!(order(&s), was);
    // Redone and undone again, the same.
    assert!(s.redo());
    assert_eq!(title(&s, second).as_deref(), Some("Dentist, mine again"));
    assert!(s.undo());
    assert_eq!(order(&s), was);
}

#[test]
fn each_change_an_undo_puts_back_goes_where_it_stood_among_several() {
    // Two changes of the event the step touches, each with another event's before it, and one
    // of the other's after both: each goes back after as many of the others' as stood before it.
    let (mut s, _, first, second) = served();
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    run(&mut s, "calendar.edit", json!({"id": second, "title": "Dentist, mine"}));
    noted_for(&mut s, first);
    noted_for(&mut s, second);
    noted_for(&mut s, first);
    let was = order(&s);
    let (update, forward) = (EventChangeKind::Update, EventChangeKind::Forward);
    assert_eq!(was, [(first, update), (second, update), (first, forward), (second, forward), (first, forward)]);
    run(&mut s, "calendar.edit", json!({"id": second, "title": "Dentist, mine again"}));

    assert!(s.undo());
    assert_eq!(title(&s, second).as_deref(), Some("Dentist, mine"));
    assert_eq!(order(&s), was);
    assert!(s.redo() && s.undo());
    assert_eq!(order(&s), was);
}

#[test]
fn an_undo_is_said_to_take_away_only_the_events_it_takes_away() {
    let (mut s, account, first, _) = served();
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    let standup = remote("e-3", "Standup", at(2026, 10, 15, 13, 0), 15);
    download(&mut s, october(account, vec![budget(), dentist(), standup.clone()]));
    let brought = called(&s, account, "e-3").id;
    // What the agent's question about unsaved windows goes by (#494): the undo keeps "Standup",
    // and puts "Budget review" back rather than taking it away.
    assert!(!s.undo_removes(false, brought));
    assert!(!s.undo_removes(false, first));
    assert!(s.undo());
    assert!(s.mailbox.event(brought).is_some());
    // Nor would the redo take away what a download brings after the undo.
    let retro = remote("e-4", "Retro", at(2026, 10, 16, 13, 0), 30);
    download(&mut s, october(account, vec![budget(), dentist(), standup.clone(), retro.clone()]));
    let later = called(&s, account, "e-4").id;
    assert!(!s.undo_removes(true, later));
    assert!(s.redo());
    assert!(s.mailbox.event(later).is_some());

    // An event the step made is taken away by its undo, download or not, and said to be.
    let calendar = s.mailbox.event(local_event(&s)).map(|e| e.calendar).unwrap();
    let made = run(&mut s, "calendar.new", json!({"title": "Lunch here", "start": "2026-10-15T12:00", "calendar": calendar}))["id"].as_u64().unwrap();
    download(&mut s, october(account, vec![budget(), dentist(), standup, retro]));
    assert!(s.undo_removes(false, made));
    assert!(!s.undo_removes(false, brought));
    assert!(s.undo());
    assert!(s.mailbox.event(made).is_none());
    // And a redo that deletes one again says so.
    let local = local_event(&s);
    run(&mut s, "calendar.delete", json!({"id": local}));
    assert!(s.undo());
    assert!(s.undo_removes(true, local));
    assert!(!s.undo_removes(true, later));
}

#[test]
fn a_local_event_deleted_comes_back_beside_a_download() {
    // Deleted on this computer alone: no change waits for a server to show the step took it.
    let (mut s, account, _, second) = served();
    let local = local_event(&s);
    let was = s.mailbox.event(local).cloned().unwrap();
    run(&mut s, "calendar.delete", json!({"id": local}));
    assert!(s.mailbox.event_changes.is_empty());
    download(&mut s, october(account, vec![budget(), dentist_moved(2)]));

    assert!(s.undo());
    assert_eq!(s.mailbox.event(local), Some(&was));
    assert_eq!(s.mailbox.event(second).map(|e| e.origin.change_key.as_str()), Some("ck-e-2-2"));
    assert!(s.mailbox.event_changes.is_empty());
}

#[test]
fn after_a_move_to_another_time_zone_an_undo_is_said_to_take_away_what_it_does() {
    // Settled by a command or a frame since, or not yet: what is said is what the undo does,
    // which settles it first (`Session::run`).
    for settled in [true, false] {
        let (mut s, account, first, _) = served_in_new_york();
        run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
        download(&mut s, october(account, vec![budget(), dentist(), remote("e-3", "Standup", at(2026, 10, 15, 13, 0), 15)]));
        let brought = called(&s, account, "e-3").id;
        // The computer is carried to London. The times are moved to its clock since the step
        // (or are about to be): the undo puts every event back as it was then, and "Standup",
        // which came after it, goes with it.
        s.clock.zone = Zone::named("Europe/London");
        if settled {
            assert!(s.settle_zone());
        }
        assert_eq!(s.mailbox.zone, if settled { "Europe/London" } else { "America/New_York" });
        assert!(s.undo_removes(false, brought), "settled: {settled}");
        assert!(s.undo_deletes(false), "settled: {settled}");
        assert!(s.undo_removed_items(false).contains(&format!("event {brought}")), "settled: {settled}");
        run(&mut s, "edit.undo", json!({}));
        assert!(s.mailbox.event(brought).is_none(), "settled: {settled}");
        assert_eq!(title(&s, first).as_deref(), Some("Budget review"));
        assert_eq!(s.mailbox.zone, "Europe/London");
    }
}

#[test]
fn an_agent_allowed_nothing_is_asked_before_an_undo_after_an_unsettled_zone_move_takes_an_event() {
    let (mut s, account, first, _) = served_in_new_york();
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
    download(&mut s, october(account, vec![budget(), dentist(), remote("e-3", "Standup", at(2026, 10, 15, 13, 0), 15)]));
    let brought = called(&s, account, "e-3").id;
    // In its zone, the undo keeps "Standup", and nothing is asked.
    assert_eq!(crate::agent::check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    // Carried to London, with no command or frame since: the gate is asked first, as
    // `mailcraft-cli mcp` and the window ask it, and the undo would take "Standup" away.
    s.clock.zone = Zone::named("Europe/London");
    assert!(s.undo_removes(false, brought));
    match crate::agent::check(&s, "edit.undo", &json!({}), &[]) {
        Verdict::Ask(Class::Delete, _) => {}
        other => panic!("not asked for delete: {other:?}"),
    }
    // Asking changed nothing: the event and the step are as they were.
    assert!(s.mailbox.event(brought).is_some());
    assert_eq!(s.undo_label(), Some("Edit Event"));
}

#[test]
fn an_undo_that_a_zone_move_after_the_gate_makes_take_an_event_is_refused_to_an_agent_without_delete() {
    // The gate is asked in New York, where the undo keeps "Standup"; the computer is carried to
    // London before the command runs, which settles the zone first (`Session::run`): the undo
    // would now take "Standup" away for good, and it checks that again itself.
    let setup = || {
        let (mut s, account, first, _) = served_in_new_york();
        run(&mut s, "calendar.edit", json!({"id": first, "title": "Mine"}));
        download(&mut s, october(account, vec![budget(), dentist(), remote("e-3", "Standup", at(2026, 10, 15, 13, 0), 15)]));
        let brought = called(&s, account, "e-3").id;
        assert_eq!(crate::agent::check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
        s.clock.zone = Zone::named("Europe/London");
        (s, first, brought)
    };
    // Refused to an agent allowed nothing, and to one allowed another class but not Delete (the
    // shape of a yes in the window to something else).
    for allow in [Vec::new(), vec![Class::Settings]] {
        let (mut s, first, brought) = setup();
        s.agent = Some(allow.clone());
        let refused = s.run("edit.undo", &json!({}));
        s.agent = None;
        let why = refused.unwrap_err().to_string();
        assert!(
            why.contains("`edit.undo` needs the owner")
                && why.contains("take away contacts, events, tasks, calendars or categories made since, for good"),
            "{allow:?}: {why}"
        );
        // Nothing was done: the event, the step and the change are as they were.
        assert!(s.mailbox.event(brought).is_some(), "{allow:?}");
        assert_eq!(title(&s, first).as_deref(), Some("Mine"), "{allow:?}");
        assert_eq!(s.undo_label(), Some("Edit Event"), "{allow:?}");
    }
    // An agent the owner allowed to delete, and the owner, undo it as ever.
    for allow in [Some(vec![Class::Delete]), None] {
        let (mut s, first, brought) = setup();
        s.agent = allow.clone();
        let done = s.run("edit.undo", &json!({}));
        s.agent = None;
        assert!(done.is_ok(), "{allow:?}: {done:?}");
        assert!(s.mailbox.event(brought).is_none(), "{allow:?}");
        assert_eq!(title(&s, first).as_deref(), Some("Budget review"), "{allow:?}");
    }
}
