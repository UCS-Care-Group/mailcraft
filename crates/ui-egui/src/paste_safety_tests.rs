// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The two refusals the programmatic Calendar Paste must make before it touches anything: a cut
//! whose move would make an account's server write to people (which `execute` cannot ask the
//! owner about), and a cut that would close an appointment window with changes the owner hasn't
//! saved. Each test makes the cut with the real `ui.calendarCut`, picks a destination with a real
//! pointer, asserts the clipboard still points at the very events it cut, and then calls
//! `MailApp::execute("ui.calendarPaste", {})`: it must answer `Err` with no dialog, no queued
//! question and no change at all — the same cut, mailbox, queued server changes, selection,
//! mailbox opening and undo history.
//!
//! One module of `calendar_selection_tests`, which is itself a child of `calendar_view`, so
//! `super::*` is that file's fixtures (`calendar`, `event`, `select`, `selected`, `click`,
//! `after_noon`, `date`, `at`, `Event`, `Id`, `DateTime`, `Window`, `Modifiers`,
//! `PointerButton`), and `super::super` is `calendar_view`'s own private `Clip`, `CLIP_ID`, `Slot`
//! and `load_slot` — read from the module that defines them, as `calendar_review_regression_tests`,
//! `calendar_late_approval_tests` and `paste_command_tests` do. Nothing here widens a private
//! helper, adds a test hook or invents a fixture API.
//!
//! WHICH PREFLIGHT THE COMMAND MUST USE, AND WHY IT IS NOT ENOUGH FOR ONE ITEM. The window's
//! unsaved guard is `crate::windows::removing_unsaved(app, id, params)` (windows.rs:547), which
//! keeps for `takes_away` (windows.rs:553) only the backend ids it names: `calendar.delete`, a
//! **cut** `calendar.paste` (its `cut` parameter must be `true`), `calendar.removeCalendar`,
//! `people.delete`, `tasks.delete` and `edit.undo`/`edit.redo`. It knows neither the UI id
//! `ui.calendarPaste` nor the empty object `{}` a caller passes, and it has no arm for the legacy
//! one-event commands `calendar.move` and `calendar.edit`. So the command must preflight the
//! backend command and the parameters it actually built (`calendar.paste`, `items`, `start`,
//! `cut: true`), before `session.run`; and the one-whole-event path (`paste_single`,
//! calendar_view.rs:3455, which runs `calendar.move`, or `calendar.edit` when the destination is
//! another calendar) needs the same guard, which `takes_away` does not provide.

use std::sync::Arc;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::calendar::{Mail, mail_of};
use mailcraft_engine::cmd::remote::keeper;
use mailcraft_model::{Address, Attendee, Mailbox, Origin};
use serde_json::json;

use super::super::{CLIP_ID, Clip, Slot, load_slot};
use super::*;
use crate::forms::Form;
use crate::people_view::testing::{now, window_on};

/// The `Clip` the window keeps on the calendar's clipboard, read where `calendar_view` put it:
/// the items, the cut flag, the text, the mailbox it was taken from and its source pointers.
fn clipboard(w: &Window) -> Option<Clip> {
    w.ctx.data(|data| data.get_temp::<Clip>(egui::Id::unique(CLIP_ID)))
}

/// The time the pointer picked, as the grid stored the pick: the destination a Paste must obey
/// rather than any start a caller could name.
#[track_caller]
fn picked(w: &Window) -> Slot {
    load_slot(&w.ctx).expect("the click picked a time in the calendar")
}

/// Where `id` starts now, or `None` when it is no longer there.
fn starts_at(w: &Window, id: Id) -> Option<DateTime> {
    w.state().session.mailbox.event(id).map(|event| event.start)
}

/// What Undo and Redo would do, and how many steps have moved: the history a refused command must
/// leave exactly as it found it (reading it never changes it).
fn undo_state(w: &Window) -> (bool, Option<&'static str>, Option<&'static str>, u64) {
    let s = &w.state().session;
    (s.can_undo(), s.undo_label(), s.redo_label(), s.undo_steps_moved())
}

/// A start in the `YYYY-MM-DDTHH:MM` spelling a command takes: what the window's own builder
/// (`iso_minute`) makes of the pick.
fn minute_iso(t: DateTime) -> String {
    format!("{}T{:02}:{:02}", t.date.iso(), t.hour, t.minute)
}

/// Nothing at all happened: no question is on screen, and the mailbox (events and the queued
/// server changes), the selection, the mailbox opening, the undo history and the calendar's
/// clipboard are the ones the refusal found — the clipboard still holding the very events it cut.
fn assert_the_refusal_touched_nothing(
    w: &Window,
    before: &Mailbox,
    opening: u64,
    selection: Vec<(Id, Option<Date>)>,
    undo: (bool, Option<&'static str>, Option<&'static str>, u64),
    cut: &Clip,
) {
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "the refusal asks nothing: no dialog and no queued owner question");
    assert!(&w.state().session.mailbox == before, "the refused paste did not change the mailbox: no event moved, none was copied, none was dropped");
    assert_eq!(&w.state().session.mailbox.event_changes, &before.event_changes, "and it queued no server change");
    assert_eq!(selected(w), selection, "and it left the selection as it found it");
    assert_eq!(w.state().session.openings(), opening, "and it opened no other mailbox");
    assert_eq!(undo_state(w), undo, "and it added no undo step, nor moved the redo one");
    let kept = clipboard(w).expect("a refused paste keeps the cut on the calendar's clipboard, to be pasted again");
    assert!(cut.is(&kept), "the very cut it found is still there, not a copy or a later cut");
    assert_eq!(kept.items, cut.items, "with the same items, in the same selection order");
    assert_eq!((kept.cut, &kept.text, kept.opening), (cut.cut, &cut.text, cut.opening), "and the same cut flag, clipboard text and mailbox");
    assert_eq!(kept.sources.len(), cut.sources.len(), "and one source pointer for each item");
    assert!(
        kept.sources.iter().zip(&cut.sources).all(|(now, was)| Arc::ptr_eq(now, was)),
        "the cut is still of the very events it captured, not later replacements under their ids"
    );
}

/// A cut of two meetings an account's server keeps: moving them makes that server tell the people
/// invited. `MailApp::execute` can never ask the owner, so `ui.calendarPaste {}` must refuse
/// before it moves anything — and keep the cut, the mailbox, its queue, the selection, the
/// opening and the undo history exactly as they were.
#[test]
fn ui_calendar_paste_of_a_cut_that_must_tell_people_is_refused_before_it_moves_anything() {
    // The meetings `calendar_late_approval_tests::kept_window` makes, by the same supported
    // means: an invented account on `example.org` (recorded only; no server is reached), and the
    // model's own `origin`, `organizer` and `attendees` fields. `mail_of` is then the real
    // notification predicate, not a count this test invents.
    let mut session = Session::at(mailcraft_engine::sample::mailbox(now(), 0), now(), 0);
    let added = session
        .run("account.add", &json!({ "email": "zoe@example.org", "name": "Zoe", "incoming": { "host": "imap.example.org" } }))
        .expect("an account is added");
    let account = added["id"].as_u64().expect("the account's id");
    for title in ["Lunch with Dev", "Dentist"] {
        let id = session.mailbox.events.values().find(|event| event.title == title).map(|event| event.id).expect("the sample has it");
        let event = session.mailbox.event_mut(id).expect("the event is there");
        event.origin = Origin { account, id: format!("server-{id}"), ..Origin::default() };
        event.organizer = Some(Address::new("Dev Example", "dev@example.org"));
        // (Somebody else to each, so that the cut writes to two people: a batch counts each once.)
        event.attendees = vec![Attendee { address: Address::new("Bo Example", format!("bo-{id}@example.org")), ..Attendee::default() }];
    }
    let mut w = window_on(session.mailbox, "view.calendar");
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);

    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    assert_ne!(lunch.id, dentist.id, "fixture: the two meetings are distinct");
    for item in [&lunch, &dentist] {
        assert!(item.recurrence.is_none(), "fixture: \"{}\" is one whole meeting, not a series", item.title);
        assert!(!item.origin.guest, "fixture: \"{}\" is the owner's own meeting, not one they were invited to", item.title);
        assert_eq!(item.origin.account, account, "fixture: the account's server keeps \"{}\"", item.title);
        assert_eq!(item.origin.id, format!("server-{}", item.id), "fixture: under a remote id of its own");
        assert!(
            keeper(&w.state().session.mailbox, &item.origin).is_some(),
            "fixture: the server that keeps \"{}\" is the one that would write",
            item.title
        );
        assert!(item.organizer.is_some() && !item.attendees.is_empty(), "fixture: \"{}\" has people the server would tell", item.title);
    }
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "fixture: the typed set is the two meetings");

    let before = w.state().session.mailbox.clone();
    let opening = w.state().session.openings();

    // The real Cut: it takes the set onto the calendar's clipboard and moves nothing.
    let cut_answer = w.state_mut().execute("ui.calendarCut", json!({})).expect("ui.calendarCut is a Calendar command");
    assert_eq!(cut_answer, json!({ "items": 2, "cut": true }), "Cut takes the set and answers only how many, and that it was a cut");
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "a cut asks nothing");
    w.run_steps(2);

    let cut = clipboard(&w).expect("Cut puts the set on the calendar's clipboard");
    assert!(cut.cut, "the clipboard holds a cut, not a copy");
    assert_eq!(cut.items.iter().map(|item| item.id).collect::<Vec<_>>(), vec![lunch.id, dentist.id], "both meetings, in selection order");
    assert_eq!(cut.sources.len(), 2, "with the source pointers it captured");
    assert_eq!(cut.opening, opening, "of the mailbox it was taken from");
    for (source, id) in cut.sources.iter().zip([lunch.id, dentist.id]) {
        let held = before.events.get(&id).expect("the mailbox still holds the meeting the cut captured");
        assert!(Arc::ptr_eq(source, held), "the cut is of that meeting itself, not a later replacement under its id");
    }
    assert_eq!((starts_at(&w, lunch.id), starts_at(&w, dentist.id)), (Some(lunch.start), Some(dentist.start)), "a cut leaves its sources in place");

    // The destination, picked by the real pointer in the meetings' own block and read back as the
    // grid stored it.
    let somewhere = after_noon(&w, 3.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    let slot = picked(&w);
    assert!(!slot.all_day && slot.text.is_none(), "fixture: the click picked empty hours, not a whole day or a field being typed into");
    assert_eq!(slot.start, at(date(10, 14), 15, 0), "fixture: the click picked Wednesday's 15:00, as `after_noon(3.0)` means");

    // Restore the selection synchronously, without painting another frame: the calendar normally
    // clears a picked slot when it paints a focused event. This covers two script commands in one
    // frame, not a stable painted selection-plus-pick. Both original preconditions remain asserted.
    w.state_mut()
        .execute("calendar.select", json!({"items": [{"id": lunch.id}, {"id": dentist.id}]}))
        .expect("select the cut set without advancing a frame");
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "fixture: the set the cut is about is selected");
    assert_eq!(picked(&w).start, slot.start, "fixture: selecting the set left the picked time where it was");

    // The real notification precondition: this is a cut of two meetings a server keeps, moved in
    // their own calendar, so that server would tell both people invited. (The picked block is the
    // meetings' own, so the payload names no other destination.)
    let payload = json!({
        "items": [{ "id": lunch.id }, { "id": dentist.id }],
        "start": minute_iso(slot.start),
        "cut": true,
    });
    assert_eq!(
        mail_of(&w.state().session, "calendar.paste", &payload),
        Some(Mail::Update(2)),
        "fixture: moving this cut writes to the two people invited, so the owner's yes is required"
    );
    assert!(cut.is(&clipboard(&w).expect("the cut survived picking a time")), "fixture: the same cut is still on the clipboard");

    let selection = selected(&w);
    let undo = undo_state(&w);
    assert_eq!(&w.state().session.mailbox, &before, "fixture: the cut and the pick changed nothing");

    // The command under test: no parameters, so no caller can add a `send` or an owner flag for
    // the owner. It must refuse before it runs anything.
    let why = w
        .state_mut()
        .execute("ui.calendarPaste", json!({}))
        .expect_err("the programmatic Paste must refuse a cut it cannot ask the owner to send, not move it");
    assert!(!why.contains("unknown command"), "ui.calendarPaste must be the Calendar's own command, refusing for its own reason: {why}");
    assert!(
        !why.contains("\"send\""),
        "the no-parameter route must refuse through the owner's guard, not tell a caller to add a `send` parameter it takes no part in: {why}"
    );
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "the refusal opens no question");
    w.run_steps(2);

    assert_the_refusal_touched_nothing(&w, &before, opening, selection, undo, &cut);
    assert_eq!(
        (starts_at(&w, lunch.id), starts_at(&w, dentist.id)),
        (Some(lunch.start), Some(dentist.start)),
        "both meetings are still where they were cut from"
    );
}

/// A cut of two local appointments, one of them open in a window with a time put into its fields
/// and not saved: pasting the cut would close that window and lose the owner's change. The command
/// cannot ask, so `ui.calendarPaste {}` must refuse before it moves anything — and keep the cut,
/// the mailbox, its queue, the selection, the opening and the undo history exactly as they were.
#[test]
fn ui_calendar_paste_of_a_cut_with_an_unsaved_appointment_window_is_refused_before_it_moves_anything() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    for item in [&lunch, &dentist] {
        assert!(item.recurrence.is_none(), "fixture: \"{}\" is one whole appointment", item.title);
        assert!(item.origin.is_local(), "fixture: \"{}\" lives in this mailbox only", item.title);
        assert!(!item.origin.guest, "fixture: \"{}\" is the owner's own", item.title);
        assert!(item.organizer.is_none() && item.attendees.is_empty(), "fixture: \"{}\" tells nobody", item.title);
    }
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "fixture: the typed set is the two appointments");

    let before = w.state().session.mailbox.clone();
    let opening = w.state().session.openings();

    // The real Cut: it takes the set onto the calendar's clipboard and moves nothing.
    let cut_answer = w.state_mut().execute("ui.calendarCut", json!({})).expect("ui.calendarCut is a Calendar command");
    assert_eq!(cut_answer, json!({ "items": 2, "cut": true }), "Cut takes the set and answers only how many, and that it was a cut");
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "a cut asks nothing");
    w.run_steps(2);

    let cut = clipboard(&w).expect("Cut puts the set on the calendar's clipboard");
    assert!(cut.cut, "the clipboard holds a cut, not a copy");
    assert_eq!(cut.items.iter().map(|item| item.id).collect::<Vec<_>>(), vec![lunch.id, dentist.id], "both appointments, in selection order");
    assert_eq!(cut.sources.len(), 2, "with the source pointers it captured");
    assert_eq!(cut.opening, opening, "of the mailbox it was taken from");
    for (source, id) in cut.sources.iter().zip([lunch.id, dentist.id]) {
        let held = before.events.get(&id).expect("the mailbox still holds the appointment the cut captured");
        assert!(Arc::ptr_eq(source, held), "the cut is of that appointment itself, not a later replacement under its id");
    }
    assert_eq!((starts_at(&w, lunch.id), starts_at(&w, dentist.id)), (Some(lunch.start), Some(dentist.start)), "a cut leaves its sources in place");

    // The destination, picked by the real pointer before the owner's window opens (so the click
    // is the grid's) and read back as the grid stored it.
    let somewhere = after_noon(&w, 3.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    let slot = picked(&w);
    assert!(!slot.all_day && slot.text.is_none(), "fixture: the click picked empty hours, not a whole day or a field being typed into");
    assert_eq!(slot.start, at(date(10, 14), 15, 0), "fixture: the click picked Wednesday's 15:00, as `after_noon(3.0)` means");

    // Keep the same synchronous script interleaving as the notification case: selecting through
    // the command does not paint a frame that would intentionally clear the grid's pick.
    w.state_mut()
        .execute("calendar.select", json!({"items": [{"id": lunch.id}, {"id": dentist.id}]}))
        .expect("select the cut set without advancing a frame");
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "fixture: the set the cut is about is selected");
    assert_eq!(picked(&w).start, slot.start, "fixture: selecting the set left the picked time where it was");

    // The owner's own appointment window, through `ui.openWindow` and the form's own fields, as
    // `calendar_batch_guard_tests::dirty_window` makes it: a proposed time put into the fields and
    // not saved.
    let opened = w.state_mut().execute("ui.openWindow", json!({ "event": lunch.id })).expect("the appointment opens in a window of its own");
    assert_eq!(opened["event"], json!(lunch.id), "fixture: the window is about the appointment that was cut");
    let window = opened["window"].as_u64().expect("a window number");
    {
        let form = w.state_mut().windows.form_mut(window).and_then(Form::event_mut).expect("the window shows the appointment's form");
        form.take_proposal(at(date(11, 6), 10, 0), at(date(11, 6), 11, 0), false);
    }
    assert_eq!(w.state().windows.unsaved(), 1, "fixture: the appointment's window has a change that isn't saved");
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "fixture: opening the window didn't change the set the cut is about");
    assert!(cut.is(&clipboard(&w).expect("the cut survived the appointment window opening")), "fixture: the same cut is still on the clipboard");

    // The real unsaved guard: moving this cut would close that window, and the guard names it.
    // The same payload tells nobody (these are local uninvited appointments), so this is the
    // unsaved seam alone.
    let payload = json!({
        "items": [{ "id": lunch.id }, { "id": dentist.id }],
        "start": minute_iso(slot.start),
        "cut": true,
    });
    assert_eq!(
        mail_of(&w.state().session, "calendar.paste", &payload),
        None,
        "fixture: moving these two local appointments writes to nobody, so no notification is being refused here"
    );
    let lost = crate::windows::removing_unsaved(w.state(), "calendar.paste", &payload)
        .expect("fixture: the cut would close the appointment window with unsaved changes");
    assert_eq!(lost["title"], json!("Lunch with Dev"), "fixture: the guard names the window whose change would be lost");

    let selection = selected(&w);
    let undo = undo_state(&w);

    // The command under test: no parameters, so nothing can tell it which window to lose. It must
    // refuse before it runs anything.
    let why = w
        .state_mut()
        .execute("ui.calendarPaste", json!({}))
        .expect_err("the programmatic Paste must refuse a cut that would close an unsaved appointment window, not move it");
    assert!(!why.contains("unknown command"), "ui.calendarPaste must be the Calendar's own command, refusing for its own reason: {why}");
    assert!(w.state().dialog.is_none() && w.state().asking.is_empty(), "the refusal opens no question");
    w.run_steps(2);

    assert_the_refusal_touched_nothing(&w, &before, opening, selection, undo, &cut);
    assert_eq!(
        (starts_at(&w, lunch.id), starts_at(&w, dentist.id)),
        (Some(lunch.start), Some(dentist.start)),
        "both appointments are still where they were cut from"
    );
    assert_eq!(w.state().windows.unsaved(), 1, "the owner's window is still open with the change it had");
}
