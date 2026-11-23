// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Regressions for the window's side of a batch of calendar items: an agent's batch may
//! not throw away changes the owner hasn't saved in an appointment's window, the owner's yes
//! reaches the items the question pinned and nothing that was selected since, a member that went
//! while the owner was being asked refuses the whole batch, and a question about another mailbox
//! is never answered for this one (#494, U-1, U-2, C-1).
//!
//! Test-only: the state of the source these were written against (2026-11-15):
//! - `windows::takes_away` reads `calendar.delete`'s one `id` (else the selection), so a batch
//!   that names `items` has no unsaved appointment found: `as_the_window_has_it` puts no
//!   `unsaved` in its params, the batch is classified as an ordinary owner delete, and with
//!   `--allow delete` it runs while the owner's changes are unsaved. That is what the first test
//!   holds against, and it is the one case that would lose the owner's work today.
//! - `calendar.paste` is not classified at all, so a cut is refused rather than asked about:
//!   the paste tests fail on their own "the owner is asked" assertion.
//!
//! Fixtures only: the sample mailbox plus synthetic appointments on `example.org`, the fixed
//! clock the window's tests use, and the owner's own appointment window filled in through the
//! form (no file, server, clock or real account).
//!
//! `What::Event` keeps an appointment's id and nothing of which occurrence the window shows, so
//! protecting every occurrence of a series whose window has unsaved changes is the rule these
//! tests expect; nothing here asks for a narrower one.

use super::as_the_window_has_it;

use std::sync::mpsc::Receiver;

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_engine::model::{Event, Freq, Id, Mailbox, Recurrence};
use mailcraft_engine::time::{Date, DateTime};
use serde_json::{Value, json};

use crate::agent_gate::answer;
use crate::control::{ControlRequest, Outcome};
use crate::forms::Form;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC: the fixed clock the window's tests use.
const NOW: i64 = 1_791_993_600;
/// Where a cut is asked to put its items.
const START: &str = "2026-12-02T09:00";

fn d(year: i32, month: u8, day: u8) -> Date {
    Date::new(year, month, day).expect("a date that exists")
}

fn at(day: Date, hour: u8, minute: u8) -> DateTime {
    DateTime::new(day, hour, minute, 0).expect("a time of day that exists")
}

struct Ids {
    /// Wednesday 2026-11-04 09:00–09:30, weekly: 11-04, 11-11, 11-18 …
    series: Id,
    kickoff: Id,
    review: Id,
    /// The appointment the owner selects while a question is up: no batch names it.
    unrelated: Id,
}

fn calendar() -> (Mailbox, Ids) {
    let mut mb = mailcraft_engine::sample::mailbox(NOW, 0);
    let local = mb.default_calendar().expect("the sample mailbox has a local calendar");
    let (series, kickoff, review, unrelated) = {
        let mut add = |title: &str, start: DateTime, minutes: i64, recurrence: Option<Recurrence>| -> Id {
            let event = Event { title: title.to_string(), start, end: start.add_minutes(minutes), calendar: local, recurrence, ..Default::default() };
            mb.add_event(event).expect("a local calendar takes an event")
        };
        (
            add("Weekly sync", at(d(2026, 11, 4), 9, 0), 30, Some(Recurrence::every(Freq::Weekly))),
            add("Kickoff", at(d(2026, 11, 9), 9, 0), 45, None),
            add("Review", at(d(2026, 11, 11), 10, 15), 45, None),
            add("Unrelated dental", at(d(2026, 11, 10), 14, 0), 30, None),
        )
    };
    (mb, Ids { series, kickoff, review, unrelated })
}

fn app() -> (MailApp, Ids) {
    let (mailbox, ids) = calendar();
    let mut a = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    (a, ids)
}

/// `command` sent by an agent allowed nothing in advance: its answer, when it has one already.
fn agent(a: &mut MailApp, command: &str, params: Value) -> (Option<Value>, Receiver<Value>) {
    agent_allowed(a, command, params, Vec::new())
}

/// `command` sent by an agent the owner allowed `allow` in advance (`--allow`).
fn agent_allowed(a: &mut MailApp, command: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let out = crate::control::handle(a, &egui::Context::default(), &req.from_agent(allow));
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

/// The question on screen: its class and what it says the agent wants.
fn asked(a: &MailApp) -> Option<(Class, String)> {
    a.asking.front().map(|q| (q.class, q.what.clone()))
}

/// Whether a refusal says the owner's unsaved changes are why.
fn says_unsaved(said: &Value) -> bool {
    said["error"].as_str().is_some_and(|why| why.to_lowercase().contains("sav"))
}

/// The owner has the appointment `event` open in a window of its own and put another time into
/// its fields without saving: a series' window is opened as the series, as its first day.
fn dirty_appointment_window(a: &mut MailApp, event: Id) {
    let opened = a.execute("ui.openWindow", json!({"event": event})).expect("an appointment opens in a window of its own");
    let window = opened["window"].as_u64().expect("a window number");
    {
        let form = a.windows.form_mut(window).and_then(Form::event_mut).expect("the window shows the appointment's form");
        form.take_proposal(at(d(2026, 11, 6), 10, 0), at(d(2026, 11, 6), 11, 0), false);
    }
    assert_eq!(a.windows.unsaved(), 1, "fixture: the appointment's window has a change that isn't saved");
}

/// Nothing the owner allowed in advance lets an agent take an occurrence away while that series'
/// window has changes the owner hasn't saved.
#[test]
fn a_batch_delete_touching_an_unsaved_appointment_window_is_never_run_unasked() {
    let (mut a, ids) = app();
    dirty_appointment_window(&mut a, ids.series);
    let before = a.session.mailbox.clone();
    let (now, rx) = agent_allowed(&mut a, "calendar.delete", json!({"items": [{"id": ids.series, "day": "2026-11-18"}]}), Class::ALL.to_vec());
    let protected = match &now {
        None => true,
        Some(said) => said["ok"] == false && says_unsaved(said),
    };
    assert!(protected, "a batch delete that takes an occurrence of a series whose window has unsaved changes must ask the owner, or refuse: {now:?}");
    assert!(a.session.mailbox == before, "the batch ran: it took an occurrence away while the owner's changes were unsaved");
    if let Some((_, what)) = asked(&a) {
        assert!(what.contains("Weekly sync"), "the question names the appointment whose changes would be lost: {what}");
        answer(&mut a, false);
        assert_eq!(rx.try_recv().expect("an answer")["ok"], false, "no leaves the series as it was");
        assert!(a.session.mailbox == before);
    }
}

/// A cut is a move of what the owner has open: unsaved changes in that series' window keep it
/// from running unasked, as they do for a delete.
#[test]
fn a_cut_of_a_series_with_an_unsaved_window_is_protected() {
    let (mut a, ids) = app();
    dirty_appointment_window(&mut a, ids.series);
    let before = a.session.mailbox.clone();
    let (now, _rx) = agent_allowed(
        &mut a,
        "calendar.paste",
        json!({"items": [{"id": ids.series, "day": "2026-11-18"}], "start": START, "cut": true}),
        Class::ALL.to_vec(),
    );
    let protected = match &now {
        None => true,
        Some(said) => said["ok"] == false && says_unsaved(said),
    };
    assert!(protected, "cutting an occurrence of a series whose window has unsaved changes must ask the owner, or refuse: {now:?}");
    assert!(a.session.mailbox == before, "the cut ran: it moved an occurrence away while the owner's changes were unsaved");
}

/// The owner's yes reaches the items the question pinned: what is selected afterwards is not
/// what the yes moves, and nothing the question didn't show goes.
#[test]
fn the_owners_yes_reaches_only_the_items_the_question_showed() {
    let (mut a, ids) = app();
    let (now, rx) = agent(&mut a, "calendar.paste", json!({"items": [{"id": ids.kickoff}, {"id": ids.review}], "start": START, "cut": true}));
    assert!(now.is_none(), "a cut waits for the owner: {now:?}");
    let (_, what) = asked(&a).expect("the owner is asked about the cut");
    assert!(what.contains("Kickoff") && what.contains("Review"), "the question names both appointments it would move: {what}");
    let before: Vec<Id> = a.session.mailbox.events.keys().copied().collect();
    let unrelated = a.session.mailbox.event(ids.unrelated).cloned().expect("the unrelated item exists");
    let kickoff_before = a.session.mailbox.event(ids.kickoff).cloned().expect("the first item exists");
    let review_before = a.session.mailbox.event(ids.review).cloned().expect("the second item exists");
    // The owner selects another appointment while the question is up.
    a.execute("calendar.select", json!({"id": ids.unrelated})).expect("the owner selects another appointment");
    answer(&mut a, true);
    let said = rx.try_recv().expect("an answer");
    assert_eq!(said["ok"], true, "the owner's yes must actually complete the named move: {said}");
    assert!(a.session.mailbox.event(ids.unrelated).is_some(), "the appointment selected while the question was up is not what a yes reaches: {said}");
    assert_eq!(a.session.mailbox.event(ids.unrelated), Some(&unrelated), "the new focus is not moved or edited either");
    let kickoff_now = a.session.mailbox.event(ids.kickoff).expect("cut preserves the first whole event's identity");
    let review_now = a.session.mailbox.event(ids.review).expect("cut preserves the second whole event's identity");
    assert_eq!(kickoff_now.start, at(d(2026, 12, 2), 9, 0));
    assert_eq!(kickoff_now.start.minutes_until(review_now.start), kickoff_before.start.minutes_until(review_before.start));
    assert_eq!(kickoff_now.duration_minutes(), kickoff_before.duration_minutes());
    assert_eq!(review_now.duration_minutes(), review_before.duration_minutes());
    let went: Vec<Id> = before.iter().copied().filter(|id| a.session.mailbox.event(*id).is_none()).collect();
    assert!(went.iter().all(|id| *id == ids.kickoff || *id == ids.review), "the yes took away an appointment the question didn't show: {went:?}");
}

/// A batch is one step: a member that went while the owner was being asked refuses all of it,
/// rather than the rest moving on its own.
#[test]
fn a_member_gone_while_the_owner_is_asked_refuses_the_whole_batch() {
    let (mut a, ids) = app();
    let (_, rx) = agent(&mut a, "calendar.paste", json!({"items": [{"id": ids.kickoff}, {"id": ids.review}], "start": START, "cut": true}));
    assert!(asked(&a).is_some(), "a cut waits for the owner");
    // Another agent, allowed to delete, takes one of the two away meanwhile.
    let (said, _) = agent_allowed(&mut a, "calendar.delete", json!({"id": ids.review}), vec![Class::Delete]);
    assert_eq!(said.expect("the other agent's delete answers at once")["ok"], true, "the other command runs at once");
    assert!(a.session.mailbox.event(ids.review).is_none());
    let kickoff = a.session.mailbox.event(ids.kickoff).cloned();
    answer(&mut a, true);
    let said = rx.try_recv().expect("an answer");
    assert_eq!(said["ok"], false, "a member that is gone refuses the whole batch: {said}");
    assert_eq!(a.session.mailbox.event(ids.kickoff).cloned(), kickoff, "the member that is still there was not moved on its own");
}

/// A question names the mailbox it was asked about: another one in view makes the yes do
/// nothing, whatever the ids mean in the new one.
#[test]
fn a_question_about_another_mailbox_is_never_answered_for_this_one() {
    let (mut a, ids) = app();
    let (_, rx) = agent(&mut a, "calendar.delete", json!({"items": [{"id": ids.kickoff}]}));
    assert!(asked(&a).is_some(), "deleting an appointment is asked about");
    // Another mailbox takes this one's place while the owner is being asked.
    a.execute("file.new", json!({})).expect("File › New opens an empty mailbox");
    answer(&mut a, true);
    let said = rx.try_recv().expect("an answer");
    assert_eq!(said["ok"], false, "a question about the mailbox that was open isn't answered for another: {said}");
    assert!(said["error"].as_str().is_some_and(|why| why.contains("another mailbox")), "{said}");
    assert!(a.session.mailbox.events.is_empty(), "nothing of the new mailbox was taken, and nothing of the old one came with it");
}

/// The owner has `event` open in a window of its own with a change it hasn't saved; how many
/// windows are like that is left to the caller.
fn dirty_window(a: &mut MailApp, event: Id) {
    let opened = a.execute("ui.openWindow", json!({"event": event})).expect("an appointment opens in a window of its own");
    let window = opened["window"].as_u64().expect("a window number");
    {
        let form = a.windows.form_mut(window).and_then(Form::event_mut).expect("the window shows the appointment's form");
        form.take_proposal(at(d(2026, 11, 6), 10, 0), at(d(2026, 11, 6), 11, 0), false);
    }
}

/// The owner has `ids.unrelated` in focus and no batch below names it: a guard that read the
/// focus instead of the batch's `items` would find nothing in these tests.
fn focus_elsewhere(a: &mut MailApp, ids: &Ids) {
    a.execute("calendar.select", json!({"id": ids.unrelated})).expect("the owner selects another appointment");
    assert_eq!(a.session.sel.event, Some(ids.unrelated), "fixture: the focus is an appointment no batch names");
}

/// An explicit batch Delete that names an appointment — a whole event, and an occurrence of a
/// series that isn't the day its window opened as — closes the windows of those appointments:
/// what the owner has unsaved in them must be found, as the one-item Delete finds it (#494, U-1).
#[test]
fn an_explicit_batch_delete_is_seen_by_the_unsaved_window_guard() {
    let (mut a, ids) = app();
    dirty_window(&mut a, ids.series);
    dirty_window(&mut a, ids.kickoff);
    assert_eq!(a.windows.unsaved(), 2, "fixture: both windows have a change that isn't saved");
    focus_elsewhere(&mut a, &ids);
    // A whole event the batch names, whose window has unsaved changes.
    let kickoff = crate::windows::removing_unsaved(&a, "calendar.delete", &json!({"items": [{"id": ids.kickoff}]}));
    assert_eq!(
        kickoff.as_ref().map(|lost| lost["title"].clone()),
        Some(json!("Kickoff")),
        "a batch Delete of a whole event closes its window and loses what is unsaved in it: {kickoff:?}"
    );
    // An occurrence of a series, on days its window wasn't opened as and no focus names: the
    // window of the series is the one that loses what it has unsaved, whichever day is taken
    // (U-1: the whole series is protected while its window has changes).
    for day in ["2026-11-18", "2026-11-25"] {
        let series = crate::windows::removing_unsaved(&a, "calendar.delete", &json!({"items": [{"id": ids.series, "day": day}]}));
        assert_eq!(
            series.as_ref().map(|lost| lost["title"].clone()),
            Some(json!("Weekly sync")),
            "a batch Delete of the occurrence on {day} closes the series' window and loses its unsaved changes: {series:?}"
        );
    }
    // Both windows at once: how many more would go with the one named is counted, and which of
    // the two a question names is the window's to choose.
    let both =
        crate::windows::removing_unsaved(&a, "calendar.delete", &json!({"items": [{"id": ids.kickoff}, {"id": ids.series, "day": "2026-11-18"}]}))
            .expect("a batch Delete of both appointments loses what is unsaved in both windows");
    assert!(matches!(both["title"].as_str(), Some("Kickoff" | "Weekly sync")), "the loss names one of the windows the batch closes: {both}");
    assert!(both["more"].as_u64().is_none_or(|more| more == 1), "neither window is left out of the count, and none is invented: {both}");
    let elsewhere = crate::windows::removing_unsaved(&a, "calendar.delete", &json!({"items": [{"id": ids.unrelated}]}));
    assert!(elsewhere.is_none(), "an appointment no batch names loses nothing, and the open windows stay: {elsewhere:?}");
}

/// A cut takes the items away as a Delete does, so a series' window with unsaved changes is
/// protected from it too; a copy of the same occurrence reads it and takes nothing away
/// (#494, U-1).
#[test]
fn an_explicit_cut_is_seen_by_the_unsaved_window_guard() {
    let (mut a, ids) = app();
    dirty_window(&mut a, ids.series);
    focus_elsewhere(&mut a, &ids);
    let moving = json!({"items": [{"id": ids.series, "day": "2026-11-18"}], "start": START, "cut": true});
    let lost = crate::windows::removing_unsaved(&a, "calendar.paste", &moving).expect("a cut that closes the series' window loses the changes in it");
    assert_eq!(lost["title"], "Weekly sync", "the loss names the series whose window has unsaved changes: {lost}");
    let copying = json!({"items": [{"id": ids.series, "day": "2026-11-18"}], "start": START});
    assert!(
        crate::windows::removing_unsaved(&a, "calendar.paste", &copying).is_none(),
        "a copy of the same occurrence takes nothing away and loses nothing"
    );
}

/// What the owner allowed in advance (`--allow delete`) doesn't answer for them: a batch Delete
/// that would close a window with unsaved changes is asked about, as the one-item Delete is. A
/// refusal from the engine after the batch began is not what must happen (#494, U-1).
#[test]
fn an_allowed_agent_is_asked_before_a_batch_delete_closes_an_unsaved_window() {
    let (mut a, ids) = app();
    dirty_window(&mut a, ids.series);
    focus_elsewhere(&mut a, &ids);
    let before = a.session.mailbox.clone();
    let (now, _rx) = agent_allowed(&mut a, "calendar.delete", json!({"items": [{"id": ids.series, "day": "2026-11-18"}]}), Class::ALL.to_vec());
    assert!(
        now.is_none(),
        "an agent allowed to delete must still be asked before a batch Delete loses the owner's unsaved changes; it must not run, and must not be left to the engine to refuse: {now:?}"
    );
    let (class, what) = asked(&a).expect("the owner is asked about the batch Delete");
    assert_eq!(class, Class::Delete, "the question is the owner's-unsaved one, classed as a Delete: {what}");
    assert!(what.contains("Weekly sync"), "the question names the appointment whose changes would be lost: {what}");
    assert!(a.session.mailbox == before, "nothing of the batch was done before the owner answered");
}

/// The same for a cut: an agent allowed `delete` is asked before it closes a series' window with
/// unsaved changes, rather than the cut running and the engine refusing afterwards (#494, U-1).
#[test]
fn an_allowed_agent_is_asked_before_a_cut_closes_an_unsaved_window() {
    let (mut a, ids) = app();
    dirty_window(&mut a, ids.series);
    focus_elsewhere(&mut a, &ids);
    let before = a.session.mailbox.clone();
    let (now, _rx) = agent_allowed(
        &mut a,
        "calendar.paste",
        json!({"items": [{"id": ids.series, "day": "2026-11-18"}], "start": START, "cut": true}),
        Class::ALL.to_vec(),
    );
    assert!(
        now.is_none(),
        "an agent allowed to delete must still be asked before a cut loses the owner's unsaved changes; it must not run, and must not be left to the engine to refuse: {now:?}"
    );
    let what = match asked(&a) {
        Some((_, what)) => what,
        None => String::new(),
    };
    assert!(what.contains("Weekly sync"), "the question names the series whose changes would be lost: {what}");
    assert!(a.session.mailbox == before, "nothing of the cut was done before the owner answered");
}

/// The window says what it would lose, never the agent (#494): an agent that says nothing is
/// unsaved doesn't hide a window with changes, and one that names a loss of its own invention
/// doesn't invent one.
#[test]
fn the_window_says_what_it_would_lose_whatever_the_agent_says() {
    let (mut a, ids) = app();
    dirty_window(&mut a, ids.series);
    focus_elsewhere(&mut a, &ids);
    let said = as_the_window_has_it(&a, "calendar.delete", json!({"items": [{"id": ids.series, "day": "2026-11-18"}], "unsaved": false}));
    let lost =
        said.get("unsaved").filter(|unsaved| !unsaved.is_null()).expect("a window with unsaved changes is said to be lost, whatever the agent said");
    assert_eq!(lost["title"], "Weekly sync", "the loss names the window as the window has it: {lost}");
    // The other way: an agent's word that something would be lost isn't believed where none is.
    let (mut clean, clean_ids) = app();
    clean.execute("calendar.select", json!({"id": clean_ids.unrelated})).expect("the owner selects the appointment");
    let claimed =
        as_the_window_has_it(&clean, "calendar.delete", json!({"id": clean_ids.unrelated, "unsaved": {"form": "appointment", "title": "Nope"}}));
    assert!(claimed.get("unsaved").is_none_or(Value::is_null), "the window doesn't take an agent's word for what is unsaved: {claimed}");
}

/// The plain cases must not ask: a copy reads an appointment and closes nothing, and a Delete of
/// an appointment no open window is about takes nothing the owner is changing (#494).
#[test]
fn copying_or_deleting_another_appointment_never_asks_about_the_unsaved_window() {
    let (mut a, ids) = app();
    dirty_window(&mut a, ids.series);
    focus_elsewhere(&mut a, &ids);
    let (copied, _rx) = agent_allowed(&mut a, "calendar.paste", json!({"items": [{"id": ids.kickoff}], "start": START}), Class::ALL.to_vec());
    assert!(asked(&a).is_none(), "copying an appointment isn't asked about, whatever a window has unsaved: {:?}", asked(&a));
    assert_eq!(copied.expect("the allowed copy answers")["ok"], true, "the copy runs");
    let (deleted, _rx) = agent_allowed(&mut a, "calendar.delete", json!({"items": [{"id": ids.unrelated}]}), Class::ALL.to_vec());
    assert!(asked(&a).is_none(), "deleting an appointment no window of the owner's is about isn't asked about: {:?}", asked(&a));
    assert_eq!(deleted.expect("the allowed Delete answers")["ok"], true, "the Delete runs");
    assert!(a.session.mailbox.event(ids.unrelated).is_none(), "the appointment the batch named is gone");
    assert_eq!(a.windows.unsaved(), 1, "the window the owner is changing is still open, untouched");
    assert!(a.session.mailbox.event(ids.series).is_some(), "the series behind that window wasn't touched");
}

/// An agent's move of an appointment whose window has unsaved changes, through the window, is
/// refused as the owner chose to keep it (CAL-D3), in the words of a move: it is open with changes
/// that aren't saved, to be saved or closed first (benandez on #1665). Nothing moves.
#[test]
fn an_agents_move_of_an_appointment_with_unsaved_changes_in_its_window_is_refused_as_a_move() {
    let (mut a, ids) = app();
    dirty_appointment_window(&mut a, ids.kickoff);
    let before = a.session.mailbox.clone();
    let (moved, _rx) = agent_allowed(&mut a, "calendar.move", json!({"id": ids.kickoff, "start": START}), Class::ALL.to_vec());
    let moved = moved.expect("the move is answered at once");
    assert_eq!(
        moved["error"].as_str(),
        Some(
            "`calendar.move` would move an appointment that is open with changes the owner hasn't saved (to another time or another calendar), and the owner wasn't asked about that; nothing was changed. Try again once the owner has saved or closed its window"
        ),
        "{moved}"
    );
    assert!(a.session.mailbox == before, "nothing moved");
    assert_eq!(a.windows.unsaved(), 1, "the owner's changes are still there");
}

#[path = "calendar_batch_dialog_tests.rs"]
mod dialog_tests;

#[path = "calendar_batch_delete_scope_tests.rs"]
mod delete_scope_tests;
