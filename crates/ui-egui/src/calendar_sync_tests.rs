// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar kept in step with Microsoft Graph in the window. Hosts are fakes; every address
//! is invented.

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::remote::{EventChangeKind, EventResult};
use serde_json::json;

use super::testing::{Job, Server, event, microsoft_account, serve};
use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SECOND: f64 = 1_000.0;

/// The sample mailbox with a Microsoft account, Mail in front, and a stand-in for Graph that
/// has one event today.
fn app() -> (MailApp, Arc<Mutex<Server>>, Id) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let server = serve(&mut app);
    let account = microsoft_account(&mut app);
    // (The wide stretch and the contacts, due at start-up, are not what these look at.)
    app.graph_checked.insert(account, f64::MAX);
    server.lock().unwrap().events = vec![event("e-1", "Budget review", NOW - 3 * 3600)];
    (app, server, account)
}

fn jobs(server: &Arc<Mutex<Server>>) -> usize {
    server.lock().unwrap().jobs.len()
}

/// Let the job in flight answer.
fn settle(app: &mut MailApp) {
    for _ in 0..4 {
        app.poll_calendar();
    }
}

#[test]
fn the_calendar_is_asked_for_when_it_is_due_and_no_more_often() {
    let (mut app, server, account) = app();
    let t0 = now_ms();
    // Mail is in front. The calendar is asked for once to begin with: the days on screen,
    // nothing else, with the token that is at hand.
    assert!(app.calendar_tick_at(true, t0));
    assert!(app.calendar_syncing());
    // One job at a time.
    assert!(!app.calendar_tick_at(true, t0 + SECOND));
    settle(&mut app);
    assert!(!app.calendar_syncing());
    let first = server.lock().unwrap().jobs[0].clone();
    assert_eq!((first.ops.len(), first.directory, first.had_token), (0, false, true));
    let (from, to) = first.calendar.expect("the days on screen were asked for");
    // The week of the 14th (the 11th to the 17th) and a week either side.
    assert_eq!((from, to), (NOW - 16 * 3600 - 10 * 86_400, NOW - 16 * 3600 + 11 * 86_400));
    // What the server has is in the mailbox, in a calendar under the account.
    assert!(app.session.mailbox.events.values().any(|e| e.title == "Budget review" && e.origin.account == account));
    // Behind the mail, every five minutes will do.
    assert!(!app.calendar_tick_at(true, t0 + 60.0 * SECOND));
    assert!(!app.calendar_tick_at(true, t0 + 299.0 * SECOND));
    assert!(app.calendar_tick_at(true, t0 + 301.0 * SECOND));
    settle(&mut app);
    assert_eq!(jobs(&server), 2);

    // The owner switches to the calendar: at once.
    let t1 = t0 + 310.0 * SECOND;
    app.execute("view.calendar", json!({})).unwrap();
    assert!(app.calendar_tick_at(true, t1));
    settle(&mut app);
    assert_eq!(jobs(&server), 3);
    // In front, about every half minute.
    assert!(!app.calendar_tick_at(true, t1 + SECOND));
    assert!(!app.calendar_tick_at(true, t1 + 29.0 * SECOND));
    assert!(app.calendar_tick_at(true, t1 + 31.0 * SECOND));
    settle(&mut app);
    assert_eq!(jobs(&server), 4);
    // The window goes behind another and comes back: at once.
    let t2 = t1 + 40.0 * SECOND;
    assert!(!app.calendar_tick_at(false, t2));
    assert!(app.calendar_tick_at(true, t2 + 2.0 * SECOND));
    settle(&mut app);
    assert_eq!(jobs(&server), 5);
    // And not again for looking at it: only the half minute brings the next.
    assert!(!app.calendar_tick_at(true, t2 + 3.0 * SECOND));
    // The File page is in front of the calendar: it is behind, like any other module.
    app.execute("ui.backstage", json!({"value": true})).unwrap();
    assert!(!app.calendar_tick_at(true, t2 + 40.0 * SECOND));
    app.execute("ui.backstage", json!({"value": false})).unwrap();
    assert!(app.calendar_tick_at(true, t2 + 41.0 * SECOND));
    settle(&mut app);

    // What was made in Outlook meanwhile is here after the next look.
    server.lock().unwrap().events.push(event("e-2", "Made in Outlook", NOW + 3600));
    assert!(app.calendar_tick_at(true, t2 + 80.0 * SECOND));
    settle(&mut app);
    assert!(app.session.mailbox.events.values().any(|e| e.title == "Made in Outlook"));
    // Not while a Send/Receive is asking Graph, or is about to; and not when nothing may
    // start by itself.
    let t3 = t2 + 200.0 * SECOND;
    app.sync_queue.push_back((account, true));
    assert!(!app.calendar_tick_at(true, t3));
    app.sync_queue.clear();
    app.auto_sync = false;
    assert!(!app.calendar_tick_at(true, t3));
    app.auto_sync = true;
    assert!(app.calendar_tick_at(true, t3));
    settle(&mut app);
}

#[test]
fn a_change_goes_at_once_and_the_calendar_comes_back_with_it() {
    let (mut app, server, account) = app();
    app.execute("view.calendar", json!({})).unwrap();
    let t0 = now_ms();
    assert!(app.calendar_tick_at(true, t0));
    settle(&mut app);
    let review = app.session.mailbox.events.values().find(|e| e.title == "Budget review").map(|e| e.id).unwrap();
    // Nothing is due for half a minute; a change made here doesn't wait for that.
    assert!(!app.calendar_tick_at(true, t0 + SECOND));
    app.execute("calendar.move", json!({"id": review, "start": "2026-10-15T09:00"})).unwrap();
    assert_eq!(app.session.mailbox.event_changes.len(), 1);
    assert!(app.calendar_tick_at(true, t0 + 2.0 * SECOND));
    settle(&mut app);
    // One job: the change, and then the days on screen, which have the event where it is now.
    let job: Job = server.lock().unwrap().jobs.last().cloned().unwrap();
    assert_eq!(job.ops.iter().map(|op| (op.kind, op.remote.as_str())).collect::<Vec<_>>(), [(EventChangeKind::Update, "e-1")]);
    assert!(job.calendar.is_some() && !job.directory);
    assert!(app.session.mailbox.event_changes.is_empty());
    let event = app.session.mailbox.event(review).unwrap();
    assert_eq!((event.start.iso(), event.origin.change_key.as_str()), ("2026-10-15T09:00:00".to_string(), "ck-2"));
    assert_eq!(server.lock().unwrap().events[0].start, NOW - 7 * 3600 + 86_400);
    // A new event, typed or otherwise: made on the server, and here once, not twice.
    app.execute("calendar.new", json!({"title": "Lunch", "start": "2026-10-15T12:00"})).unwrap();
    assert!(app.calendar_tick_at(true, t0 + 3.0 * SECOND));
    settle(&mut app);
    assert_eq!(app.session.mailbox.events.values().filter(|e| e.title == "Lunch").count(), 1);
    let lunch = app.session.mailbox.events.values().find(|e| e.title == "Lunch").unwrap();
    assert_eq!((lunch.origin.id.as_str(), lunch.origin.account), ("made-1", account));
    assert!(app.session.mailbox.event_changes.is_empty() && !app.has_sync_problems());

    // The server refuses a change for good: it is put back, and the status bar says why.
    server.lock().unwrap().say = Some(EventResult::Refused);
    app.execute("calendar.move", json!({"id": review, "start": "2026-10-16T09:00"})).unwrap();
    assert!(app.calendar_tick_at(true, t0 + 4.0 * SECOND));
    settle(&mut app);
    assert_eq!(app.session.mailbox.event(review).map(|e| e.start.iso()), Some("2026-10-15T09:00:00".to_string()));
    let said = app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert_eq!(said, "The server wouldn't take the change to \"Budget review\" (the stand-in said so). It was put back as it was.");
    assert!(app.session.mailbox.event_changes.is_empty() && app.has_sync_problems());
}

#[test]
fn a_server_that_does_not_answer_is_not_hammered() {
    let (mut app, server, account) = app();
    app.execute("view.calendar", json!({})).unwrap();
    server.lock().unwrap().down = true;
    let t0 = now_ms();
    assert!(app.calendar_tick_at(true, t0));
    settle(&mut app);
    assert!(app.has_sync_problems());
    // In front, with the window coming back again and again: not before half a minute is up.
    for seconds in [1.0, 5.0, 29.0] {
        assert!(!app.calendar_tick_at(false, t0 + seconds * SECOND - 500.0));
        assert!(!app.calendar_tick_at(true, t0 + seconds * SECOND), "{seconds}");
    }
    assert!(app.calendar_tick_at(true, t0 + 31.0 * SECOND));
    settle(&mut app);
    // Twice as long after every failure, ten minutes at most.
    let mut at = t0 + 31.0 * SECOND;
    for wait in [60.0, 120.0, 240.0, 480.0, 600.0, 600.0] {
        assert!(!app.calendar_tick_at(true, at + (wait - 1.0) * SECOND), "{wait}");
        at += (wait + 1.0) * SECOND;
        assert!(app.calendar_tick_at(true, at), "{wait}");
        settle(&mut app);
    }
    assert_eq!(jobs(&server), 8);
    // What is made meanwhile is made all the same (here: on this computer, as no calendar of
    // the server's has come down yet), and doesn't cut the waiting short.
    let made = app.execute("calendar.new", json!({"title": "Offline", "start": "2026-10-15T12:00"}));
    assert!(made.is_ok() && !app.calendar_tick_at(true, at + SECOND));
    // The server is back: the next look brings the calendar, and the waiting is over.
    server.lock().unwrap().down = false;
    at += 601.0 * SECOND;
    assert!(app.calendar_tick_at(true, at));
    settle(&mut app);
    assert!(app.session.mailbox.event_changes.is_empty());
    assert!(app.calendar_tick_at(true, at + 31.0 * SECOND));
    settle(&mut app);

    // No permission: the owner is told once, and Graph is left alone until it is given.
    server.lock().unwrap().consent_needed = true;
    at += 100.0 * SECOND;
    assert!(app.calendar_tick_at(true, at));
    settle(&mut app);
    assert!(app.graph_consent_needed(account));
    let asked = jobs(&server);
    for seconds in [31.0, 301.0, 3_600.0] {
        assert!(!app.calendar_tick_at(true, at + seconds * SECOND));
    }
    assert_eq!(jobs(&server), asked);
}

#[test]
fn a_change_the_account_may_not_make_asks_for_permission_and_waits() {
    let (mut app, server, account) = app();
    app.execute("view.calendar", json!({})).unwrap();
    let t0 = now_ms();
    assert!(app.calendar_tick_at(true, t0));
    settle(&mut app);
    let review = app.session.mailbox.events.values().find(|e| e.title == "Budget review").map(|e| e.id).unwrap();
    // The sign-in is from before the calendar was written to: the server says 403.
    server.lock().unwrap().say = Some(EventResult::Denied);
    app.execute("calendar.move", json!({"id": review, "start": "2026-10-15T09:00"})).unwrap();
    assert!(app.calendar_tick_at(true, t0 + SECOND));
    settle(&mut app);
    // The change is kept, the owner is asked to connect, and nothing more is asked of Graph.
    assert!(app.graph_consent_needed(account));
    assert_eq!(app.session.mailbox.event(review).map(|e| e.start.iso()), Some("2026-10-15T09:00:00".to_string()));
    assert_eq!(app.session.mailbox.event_changes.len(), 1);
    let said = app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert_eq!(said, "Changing the calendar needs your permission: File › Info › Connect calendar and contacts");
    assert!(!app.calendar_tick_at(true, t0 + 400.0 * SECOND));
    // "Connect calendar and contacts": the permission is given, and the change goes.
    server.lock().unwrap().say = None;
    app.graph_consent_needed.remove(&account);
    app.calendar_connected(account);
    assert!(app.calendar_tick_at(true, t0 + 401.0 * SECOND));
    settle(&mut app);
    assert!(app.session.mailbox.event_changes.is_empty());
    assert_eq!(server.lock().unwrap().events[0].start, NOW - 7 * 3600 + 86_400);
}

use crate::dialogs::{Dialog, Question};
use mailcraft_model::remote::{RemoteEvent, RemoteEventKind};
use mailcraft_model::{Address, Attendee, Freq, Recurrence, Response};

/// The app with the calendar in front and what the stand-in has taken in. `events` are what
/// the stand-in has beside the one of [`app`].
fn calendar(events: Vec<RemoteEvent>) -> (MailApp, Arc<Mutex<Server>>, Id, f64) {
    let (mut app, server, account) = app();
    server.lock().unwrap().events.extend(events);
    app.execute("view.calendar", json!({})).unwrap();
    let t0 = now_ms();
    assert!(app.calendar_tick_at(true, t0));
    settle(&mut app);
    (app, server, account, t0)
}

/// Let what waits go to the stand-in, and take in what it says.
fn sync(app: &mut MailApp, at: &mut f64) {
    *at += 31.0 * SECOND;
    app.calendar_tick_at(true, *at);
    settle(app);
}

/// The question that is being asked: its title, the button that goes ahead, and the others.
fn asked(app: &MailApp) -> Option<(String, String, Vec<String>)> {
    match &app.dialog {
        Some(Dialog::Confirm { ask, .. }) => Some((ask.title.clone(), ask.action.clone(), ask.more.iter().map(|(label, _)| label.clone()).collect())),
        _ => None,
    }
}

/// Answer the question that is being asked with its button `label`, and with a note when
/// one is written.
fn answer(app: &mut MailApp, label: &str, note: &str) {
    let Some(Dialog::Confirm { command, params, ask }) = app.dialog.take() else { panic!("no question is being asked") };
    let Question { action, with, more, note: field, .. } = ask;
    let with = if action == label {
        with
    } else {
        more.into_iter().find(|(other, _)| other == label).map(|(_, with)| with).unwrap_or_else(|| panic!("no button {label}"))
    };
    let params = crate::dialogs::answered(&params, &with, field.as_ref().map(|(key, _, _)| (*key, note)));
    crate::dialogs::go_ahead(app, &command, params);
}

fn mail(server: &Arc<Mutex<Server>>) -> Vec<String> {
    server.lock().unwrap().mail.clone()
}

fn titled(app: &MailApp, title: &str) -> Vec<mailcraft_model::Event> {
    let mut events: Vec<_> = app.session.mailbox.events.values().filter(|e| e.title == title).map(|e| e.as_ref().clone()).collect();
    events.sort_by_key(|e| (e.start, e.id));
    events
}

#[test]
fn nobody_is_written_to_until_the_owner_presses_send() {
    let (mut app, server, account, mut at) = calendar(Vec::new());
    let sync_meeting = json!({"title": "Sync", "start": "2026-10-15T10:00", "attendees": "bo@example.org; cy@example.org"});
    // A new meeting in the account's calendar: the invitations are asked about first, and
    // until the answer nothing is made and nothing is sent.
    assert_eq!(app.run("calendar.newMeeting", sync_meeting.clone()).unwrap()["pending"], "confirm");
    let question = asked(&app).unwrap();
    assert_eq!((question.0.as_str(), question.1.as_str()), ("Send This Invitation?", "Send"));
    assert_eq!(question.2, ["Don't Send, Keep on This Computer"]);
    sync(&mut app, &mut at);
    assert!(titled(&app, "Sync").is_empty() && mail(&server).is_empty());
    // Not sent: it is kept on this computer, and nobody hears of it.
    answer(&mut app, "Don't Send, Keep on This Computer", "");
    sync(&mut app, &mut at);
    let kept = titled(&app, "Sync");
    assert!(kept.len() == 1 && kept[0].origin.is_local() && mail(&server).is_empty());
    app.execute("calendar.delete", json!({"id": kept[0].id})).unwrap();
    // Cancel (the dialog is closed): as if it had never been asked for.
    app.run("calendar.newMeeting", sync_meeting.clone()).unwrap();
    app.dialog = None;
    sync(&mut app, &mut at);
    assert!(titled(&app, "Sync").is_empty() && mail(&server).is_empty());
    // Send: the meeting is made on the server, which invites the two.
    app.run("calendar.newMeeting", sync_meeting.clone()).unwrap();
    answer(&mut app, "Send", "");
    assert_eq!(asked(&app), None);
    sync(&mut app, &mut at);
    let sent = titled(&app, "Sync");
    assert_eq!((sent.len(), sent[0].origin.account, sent[0].origin.id.as_str()), (1, account, "made-1"));
    assert_eq!(mail(&server), ["invitation:Sync:2"]);
    let id = sent[0].id;

    // Moving it is an update in two people's mail: asked about, and left alone on Cancel.
    assert_eq!(app.run("calendar.move", json!({"id": id, "start": "2026-10-15T11:00"})).unwrap()["pending"], "confirm");
    assert_eq!(asked(&app).map(|q| (q.0, q.1)), Some(("Change This Meeting?".to_string(), "Send Update".to_string())));
    app.dialog = None;
    sync(&mut app, &mut at);
    assert_eq!((titled(&app, "Sync")[0].start.hour, mail(&server).len()), (10, 1));
    app.run("calendar.move", json!({"id": id, "start": "2026-10-15T11:00"})).unwrap();
    answer(&mut app, "Send Update", "");
    sync(&mut app, &mut at);
    assert_eq!((titled(&app, "Sync")[0].start.hour, mail(&server).last().cloned()), (11, Some("update:Sync:2".to_string())));
    // How it shows in the owner's own calendar is asked about by nobody, and told to nobody.
    assert!(app.run("calendar.showAs", json!({"id": id, "value": "tentative"})).unwrap().get("pending").is_none());
    assert!(app.run("calendar.categorize", json!({"id": id, "category": "Blue category"})).unwrap().get("pending").is_none());
    sync(&mut app, &mut at);
    assert_eq!(mail(&server).len(), 2);
    assert!(app.session.mailbox.event_changes.is_empty());
    // A script that doesn't say "send" changes nothing, and is told why.
    let why = app.execute("calendar.move", json!({"id": id, "start": "2026-10-15T12:00"})).unwrap_err();
    assert!(why.contains("Say that this is meant with \"send\": true."), "{why}");
    sync(&mut app, &mut at);
    assert_eq!((titled(&app, "Sync")[0].start.hour, mail(&server).len()), (11, 2));

    // Deleting it is a cancellation, with a note for the people invited.
    app.run("calendar.delete", json!({"id": id})).unwrap();
    let question = asked(&app).unwrap();
    assert_eq!((question.0.as_str(), question.1.as_str(), question.2.len()), ("Cancel This Meeting?", "Send Cancellation", 0));
    assert!(app.session.mailbox.event(id).is_some());
    answer(&mut app, "Send Cancellation", " Sorry, something came up. ");
    assert!(app.session.mailbox.event(id).is_none());
    sync(&mut app, &mut at);
    assert_eq!(mail(&server).last().map(String::as_str), Some("cancellation:Sync:2:Sorry, something came up."));
    assert!(server.lock().unwrap().events.iter().all(|e| e.title != "Sync") && !app.has_sync_problems());
}

#[test]
fn a_series_is_asked_about_and_one_day_or_all_of_it_changes_on_the_server() {
    // Monday, Wednesday and Friday at one o'clock, from Monday the 5th.
    let rule = Recurrence {
        by_day: vec![mailcraft_time::Weekday::Mon, mailcraft_time::Weekday::Wed, mailcraft_time::Weekday::Fri],
        ..Recurrence::every(Freq::Weekly)
    };
    let series = RemoteEvent {
        kind: RemoteEventKind::SeriesMaster,
        recurrence: Some(rule),
        end: NOW - 9 * 86_400 - 3 * 3600 + 1800,
        ..event("s-1", "Stand-up", NOW - 9 * 86_400 - 3 * 3600)
    };
    let (mut app, server, _, mut at) = calendar(vec![series]);
    // One event here, which repeats; the week in view has it on its three days.
    let all = titled(&app, "Stand-up");
    assert_eq!((all.len(), all[0].recurrence.is_some(), all[0].exceptions.len()), (1, true, 0));
    let id = all[0].id;
    let day = |d: u8| mailcraft_time::Date::new(2026, 10, d).unwrap();
    let on: Vec<u8> = all[0].occurrences(day(11), day(17)).iter().map(|o| o.start.date.day).collect();
    assert_eq!(on, [12, 14, 16]);

    // Friday's is dragged an hour down: which is meant is asked first.
    app.execute("calendar.select", json!({"id": id, "day": "2026-10-16"})).unwrap();
    assert_eq!(app.run("calendar.move", json!({"id": id, "start": "2026-10-05T14:00"})).unwrap()["pending"], "confirm");
    let question = asked(&app).unwrap();
    assert_eq!((question.0.as_str(), question.1.as_str()), ("Move Recurring Item", "Just This One"));
    assert_eq!(question.2, ["The Entire Series"]);
    let Some(Dialog::Confirm { ask, .. }) = &app.dialog else { panic!() };
    assert_eq!(ask.text, "\"Stand-up\" is one of a series. Do you mean just the one on Friday, October 16, 2026, or the entire series?");
    // Just this one: Friday's is an event of its own at two, and the server has it so.
    answer(&mut app, "Just This One", "");
    let all = titled(&app, "Stand-up");
    assert_eq!(all.len(), 2);
    assert_eq!((all[0].exceptions.as_slice(), all[1].start.date, all[1].start.hour), (&[day(16)][..], day(16), 14));
    sync(&mut app, &mut at);
    let all = titled(&app, "Stand-up");
    assert_eq!((all.len(), all[1].origin.id.as_str(), all[1].origin.series.as_str()), (2, "x-1", "s-1"));
    assert_eq!(all[0].exceptions, [day(16)]);
    assert!(app.session.mailbox.event_changes.is_empty() && !app.has_sync_problems());
    let moved = server.lock().unwrap().events.iter().find(|e| e.id == "x-1").map(|e| (e.start, e.original)).unwrap();
    assert_eq!(moved, (NOW + 2 * 86_400 - 2 * 3600, Some(NOW + 2 * 86_400 - 3 * 3600)));

    // Monday's is deleted: only that one, when that is the answer.
    app.execute("calendar.select", json!({"id": id, "day": "2026-10-12"})).unwrap();
    app.run("calendar.delete", json!({"id": id})).unwrap();
    assert_eq!(asked(&app).map(|q| q.0), Some("Delete Recurring Item".to_string()));
    answer(&mut app, "Just This One", "");
    sync(&mut app, &mut at);
    let all = titled(&app, "Stand-up");
    assert_eq!((all.len(), all[0].exceptions.as_slice()), (2, &[day(12), day(16)][..]));
    assert!(server.lock().unwrap().taken.contains(&("s-1".to_string(), (NOW - 2 * 86_400).div_euclid(86_400))));

    // Wednesday's is dragged half an hour down, and the entire series is meant: every day
    // of it is at half past one from now on, on the server too.
    app.execute("calendar.select", json!({"id": id, "day": "2026-10-14"})).unwrap();
    app.run("calendar.move", json!({"id": id, "start": "2026-10-05T13:30"})).unwrap();
    answer(&mut app, "The Entire Series", "");
    assert_eq!(asked(&app), None);
    sync(&mut app, &mut at);
    let all = titled(&app, "Stand-up");
    assert_eq!((all[0].start.hour, all[0].start.minute, all[0].recurrence.is_some()), (13, 30, true));
    assert_eq!(server.lock().unwrap().events.iter().find(|e| e.id == "s-1").map(|e| e.start), Some(NOW - 9 * 86_400 - 3 * 3600 + 1800));
    assert!(app.session.mailbox.event_changes.is_empty() && !app.has_sync_problems());

    // Opened: which is meant is asked before the form, and what the form saves is saved for
    // that, without asking again.
    app.execute("calendar.select", json!({"id": id, "day": "2026-10-21"})).unwrap();
    assert_eq!(app.run("calendar.edit", json!({})).unwrap()["pending"], "editEvent");
    assert_eq!(asked(&app).map(|q| (q.0, q.1)), Some(("Open Recurring Item".to_string(), "Just This One".to_string())));
    assert!(crate::dialogs::confirmation(&app, "ui.openWindow", &json!({"event": id})).is_some_and(|ask| ask.title == "Open Recurring Item"));
    answer(&mut app, "Just This One", "");
    assert!(matches!(app.dialog, Some(Dialog::Form(_))) || app.windows.of_event(id).is_some(), "the form opens once the question is answered");
    assert_eq!(app.session.driver.calendar.opened, Some((id, Some(day(21)))));
    app.dialog = None;
    // The form saves as forms do: asked first when there is a question, and here there is
    // always something to say (which of the series), whatever is selected by now.
    app.execute("calendar.select", json!({"id": null})).unwrap();
    let fields = json!({"id": id, "title": "Stand-up (long)", "start": "2026-10-05T13:30", "end": "2026-10-05T15:00"});
    assert!(crate::dialogs::confirmation(&app, "calendar.edit", &fields).is_some());
    let saved = app.run("calendar.edit", fields).unwrap();
    assert_eq!((saved["occurrence"].as_str(), asked(&app), app.session.driver.calendar.opened), (Some("2026-10-21"), None, None));
    let long = titled(&app, "Stand-up (long)");
    assert_eq!((long.len(), long[0].start.date, long[0].duration_minutes()), (1, day(21), 90));
    // A script that says which is meant is asked nothing.
    assert!(crate::dialogs::confirmation(&app, "calendar.delete", &json!({"id": id, "scope": "series"})).is_none());
    assert!(crate::dialogs::confirmation(&app, "calendar.delete", &json!({"id": id, "occurrence": "2026-10-23"})).is_none());

    // The series is deleted on the server: asked about by name, it is said to be gone, and
    // goes here with the days that were changed.
    server.lock().unwrap().events.retain(|e| e.id != "s-1" && e.series != "s-1");
    sync(&mut app, &mut at);
    assert!(server.lock().unwrap().jobs.last().unwrap().series.contains(&"s-1".to_string()));
    assert!(app.session.mailbox.events.values().all(|e| e.origin.id != "s-1" && e.origin.series != "s-1"));
}

#[test]
fn an_invitation_is_answered_and_declined_from_the_calendar() {
    let invited = RemoteEvent {
        organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
        attendees: vec![Attendee { address: Address::new("Avery", "avery@mail.example"), ..Default::default() }],
        is_organizer: Some(false),
        ..event("g-1", "Board", NOW + 86_400)
    };
    let (mut app, server, _, mut at) = calendar(vec![invited]);
    let id = titled(&app, "Board")[0].id;
    // Accept: send the answer now (with a note), or keep it to the calendar.
    assert_eq!(app.run("calendar.respond", json!({"id": id, "response": "accept"})).unwrap()["pending"], "confirm");
    let question = asked(&app).unwrap();
    assert_eq!((question.0.as_str(), question.1.as_str()), ("Send a Response?", "Send the Response Now"));
    assert_eq!(question.2, ["Don't Send a Response"]);
    answer(&mut app, "Send the Response Now", "See you there");
    assert_eq!(titled(&app, "Board")[0].attendees[0].response, Response::Accepted);
    sync(&mut app, &mut at);
    assert_eq!(mail(&server), ["response:Board:Accepted:See you there"]);
    assert_eq!(server.lock().unwrap().events.iter().find(|e| e.id == "g-1").map(|e| e.response), Some(Response::Accepted));
    // Tentatively, and nobody told: the server has the answer, and wrote to nobody.
    app.run("calendar.respond", json!({"id": id, "response": "tentative"})).unwrap();
    answer(&mut app, "Don't Send a Response", "a note nobody reads");
    sync(&mut app, &mut at);
    assert_eq!(mail(&server).len(), 1);
    assert_eq!(server.lock().unwrap().events.iter().find(|e| e.id == "g-1").map(|e| e.response), Some(Response::Tentative));

    // Deleting the invitation: only the owner's copy, or declining it, told or not.
    app.run("calendar.delete", json!({"id": id})).unwrap();
    let question = asked(&app).unwrap();
    assert_eq!((question.0.as_str(), question.1.as_str()), ("Delete Your Copy of This Meeting?", "Delete My Copy"));
    assert_eq!(question.2, ["Decline and Tell the Organizer", "Decline without Telling"]);
    answer(&mut app, "Decline and Tell the Organizer", "Can't make it");
    assert!(app.session.mailbox.event(id).is_none());
    sync(&mut app, &mut at);
    assert_eq!(mail(&server).last().map(String::as_str), Some("response:Board:Declined:Can't make it"));
    assert!(server.lock().unwrap().events.iter().all(|e| e.id != "g-1"));
    assert!(app.session.mailbox.event_changes.is_empty() && !app.has_sync_problems());
}

#[test]
fn the_owner_can_have_the_calendar_connection_checked() {
    use crate::{CalendarCheckRequest, CalendarCheckResult, CalendarCheckStep};
    let (mut app, server, _) = app();
    // A build that can't reach a server can't check one.
    assert_eq!(app.execute("ui.checkCalendar", json!({"confirmed": true})).unwrap_err(), "This build can't reach the account's calendar.");
    // A host that reports three steps, of which the reading didn't come back as it was sent.
    let calls: Arc<Mutex<Vec<(String, bool)>>> = Default::default();
    let seen = calls.clone();
    app.services.check_calendar = Some(Box::new(move |request: CalendarCheckRequest| {
        seen.lock().unwrap().push((request.account.email.clone(), !request.access.is_empty()));
        let step = |what: &str, ok: bool, status: u16, said: &str| CalendarCheckStep { what: what.into(), ok, status, said: said.into() };
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send(CalendarCheckResult {
            account: request.account.id,
            steps: vec![
                step("Make an appointment", true, 201, "sent: A to B"),
                step("Read it back", false, 200, "sent: A to B; the server has: C to D (the start and the end differ)"),
                step("Delete it", true, 204, "deleted"),
            ],
            ok: false,
            cleaned_up: true,
            ..Default::default()
        });
        rx
    }));
    // The owner is asked first, and told exactly what is about to happen on the real account.
    assert_eq!(app.run("ui.checkCalendar", json!({})).unwrap()["pending"], "confirm");
    let Some(Dialog::Confirm { ask, .. }) = &app.dialog else { panic!("no question is being asked") };
    assert_eq!((ask.title.as_str(), ask.action.as_str(), ask.more.len()), ("Check Calendar Connection", "Check Now", 0));
    for said in [
        "\"MailCraft connection check\"",
        "avery@mail.example",
        "a year from today",
        "deletes it again",
        "Nothing else in your calendar is read or changed",
    ] {
        assert!(ask.text.contains(said), "{said}: {}", ask.text);
    }
    assert!(calls.lock().unwrap().is_empty() && !app.checking_calendar());
    // A script has to say that it means it; closing the question does nothing at all.
    assert!(app.execute("ui.checkCalendar", json!({})).unwrap_err().contains("\"confirmed\": true"));
    app.dialog = None;
    assert!(calls.lock().unwrap().is_empty());
    // Yes: the host gets the job, with the token that is at hand, and one check at a time.
    app.run("ui.checkCalendar", json!({})).unwrap();
    answer(&mut app, "Check Now", "");
    assert_eq!(*calls.lock().unwrap(), [("avery@mail.example".to_string(), true)]);
    assert!(app.checking_calendar());
    assert_eq!(app.execute("ui.checkCalendar", json!({"confirmed": true})).unwrap_err(), "The calendar connection is being checked already.");
    // The report: every step with the server's status and what it said, to read and to copy.
    app.poll_calendar_check();
    assert!(!app.checking_calendar());
    let Some(Dialog::Report { name, title, lines }) = &app.dialog else { panic!("the report is shown") };
    assert_eq!((*name, title.as_str()), ("calendarCheck", "Calendar Connection Check"));
    assert_eq!(
        lines,
        &[
            "avery@mail.example: Something did not go through, or did not arrive as it was sent. The steps below say what.",
            "1. Make an appointment: OK (the server answered 201). sent: A to B",
            "2. Read it back: FAILED (the server answered 200). sent: A to B; the server has: C to D (the start and the end differ)",
            "3. Delete it: OK (the server answered 204). deleted",
            "The test appointment is gone again. Nothing else in the calendar was read or changed.",
        ]
    );
    assert_eq!(app.calendar_check_report(), lines.as_slice());
    // It asked the calendar's own stand-in for nothing: the check is the host's business.
    assert_eq!(jobs(&server), 0);
}

#[test]
fn a_change_that_cannot_be_sent_is_given_up_out_loud() {
    let (mut app, server, _, mut at) = calendar(Vec::new());
    // A meeting the owner called, on the server.
    app.run("calendar.newMeeting", json!({"title": "Sync", "start": "2026-10-15T10:00", "attendees": "bo@example.org"})).unwrap();
    answer(&mut app, "Send", "");
    sync(&mut app, &mut at);
    let id = titled(&app, "Sync")[0].id;
    assert_eq!(mail(&server), ["invitation:Sync:1"]);
    // A change to it that waits without the owner's yes to the mail it makes (only a
    // mailbox file from elsewhere holds such a thing): it is not sent, and it doesn't just
    // vanish either. The event is as the server has it again, and the owner is told.
    app.execute("calendar.move", json!({"id": id, "start": "2026-10-15T11:00", "send": true})).unwrap();
    app.session.mailbox.event_changes[0].send = false;
    app.session.mailbox_changed();
    sync(&mut app, &mut at);
    assert_eq!((titled(&app, "Sync")[0].start.hour, mail(&server).len()), (10, 1));
    assert!(app.session.mailbox.event_changes.is_empty());
    let line = "The change to \"Sync\" was not sent: it would have made the server write to the people invited, and nobody said to send that. It was put back to what the server has.";
    assert_eq!(app.status_msg.as_ref().map(|(text, _)| text.as_str()), Some(line));
    assert!(app.has_sync_problems() && app.sync_problem_lines().iter().any(|said| said.trim() == line));
}

#[test]
fn a_change_to_what_is_only_on_this_computer_says_that_it_stays_here() {
    let (mut app, _, _, _) = calendar(Vec::new());
    // The sample's own appointments are on this computer; the account's calendar is on its server.
    let lunch = app.session.mailbox.events.values().find(|e| e.origin.is_local() && e.attendees.is_empty()).map(|e| e.id).unwrap();
    app.status_msg = None;
    app.run("calendar.showAs", json!({"id": lunch, "value": "tentative"})).unwrap();
    let said = app.status_msg.as_ref().map(|(text, _)| text.clone());
    assert_eq!(said.as_deref(), Some("Saved on this computer only: this item is not in your Outlook calendar"));
    assert!(app.session.mailbox.event_changes.is_empty());
    // One of the server's says nothing of the kind: it goes there.
    let review = titled(&app, "Budget review")[0].id;
    app.status_msg = None;
    app.run("calendar.showAs", json!({"id": review, "value": "tentative"})).unwrap();
    assert!(app.status_msg.is_none() && app.session.mailbox.event_changes.len() == 1);
}

#[test]
fn a_sign_in_without_the_newest_permission_is_no_disconnected_calendar() {
    let (mut app, server, account) = app();
    server.lock().unwrap().extras_missing = true;
    app.execute("view.calendar", json!({})).unwrap();
    let t0 = now_ms();
    assert!(app.calendar_tick_at(true, t0));
    settle(&mut app);
    // The calendar came, and keeps coming; what is missing is noted quietly.
    assert!(app.graph_extras_missing(account) && !app.graph_consent_needed(account) && !app.has_sync_problems());
    assert!(app.session.mailbox.events.values().any(|e| e.title == "Budget review"));
    assert!(app.calendar_tick_at(true, t0 + 31.0 * SECOND));
    settle(&mut app);
    assert_eq!(jobs(&server), 2);
    // Once the categories come (the owner agreed to it), nothing is missing any more.
    let categories = mailcraft_model::remote::CategoriesDownload {
        account,
        email: "avery@mail.example".into(),
        categories: vec![mailcraft_model::Category { name: "Project X".into(), color: mailcraft_model::Rgb(1, 2, 3) }],
        complete: true,
    };
    let mut errors = Vec::new();
    app.take_in_calendar_list(account, None, Some(&categories), false, &mut errors);
    assert!(!app.graph_extras_missing(account) && errors.is_empty());
    assert!(app.session.mailbox.category("Project X").is_some());
}

/// A job that takes long to come to nothing waits its whole wait from when it ended, not from
/// when it began (#661).
#[test]
fn a_server_that_answers_late_is_waited_for_from_its_answer() {
    let (mut app, server, _) = app();
    app.execute("view.calendar", json!({})).unwrap();
    server.lock().unwrap().down = true;
    let t0 = now_ms();
    assert!(app.calendar_tick_at(true, t0));
    // The answer comes 40 seconds later.
    assert!(!app.calendar_tick_at(true, t0 + 40.0 * SECOND));
    settle(&mut app);
    assert!(app.has_sync_problems());
    assert!(!app.calendar_tick_at(true, t0 + 41.0 * SECOND), "not at once");
    assert!(!app.calendar_tick_at(true, t0 + 69.0 * SECOND));
    assert!(app.calendar_tick_at(true, t0 + 71.0 * SECOND), "after the wait");
    assert_eq!(jobs(&server), 2);
}

#[test]
fn the_calendar_waits_while_the_categories_are_told_to_the_server() {
    let (mut app, server, account) = app();
    // A categories job under way, its answer not taken in yet.
    let (_tx, rx) = std::sync::mpsc::channel();
    app.category_sync.rx = Some((account, 0.0, 0, rx));
    assert!(!app.calendar_tick_at(true, now_ms()), "a list read now could be from before the change");
    assert_eq!(jobs(&server), 0);
    app.category_sync.rx = None;
    assert!(app.calendar_tick_at(true, now_ms()));
}
