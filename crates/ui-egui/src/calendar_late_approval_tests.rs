// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The two last seams of a batch of calendar items, held against the window itself: a cut whose
//! delivery the owner confirms *after* the paste was asked about must be taken off the clipboard
//! by that yes (and by nothing else), and the largest set a selection holds — two hundred items —
//! must still be asked about in one question whose answers can be pressed in a window at the
//! least size it can be made.
//!
//! One module of `calendar_selection_tests`, which is itself a child of `calendar_view`, so
//! `super::*` is that file's fixtures (among them `click`, `chord`, `selected`, `drawn`,
//! `after_noon`, `date` and `at`) and `super::super` is `calendar_view`'s own `Clip` and
//! `CLIP_ID`. Nothing here is production code and nothing here is a test hook: the window is the
//! real one (`people_view::testing::{window_on, now, dialog}`), its events are synthetic (invented
//! people on `example.org`, `Mailbox::add_event`), the clock is fixed, and every act is a pointer
//! event, a key or a button the owner would press — no file, server, real account, native window
//! or wall clock.
//!
//! What the source did when these were written (2026-11-15), and why each test holds it:
//! - `dialogs::confirmation` (430–716) lists `calendar.delete` but not `calendar.paste`, so a cut
//!   of meetings an account's server keeps is refused by the engine as unsent (`calendar.paste`
//!   needs `"send": true`; `selection::paste`, 475–478) instead of being asked about. The first
//!   test's "one question is asked" assertion fails there, and everything after it is the seam
//!   the question opens.
//! - `dialogs::go_ahead` (990) runs the answered command directly. `calendar_view::paste`
//!   (3209–3249) consumes a cut only when the command answered `done` at the moment of the paste;
//!   a paste that waited for the owner answers `{"pending": "confirm"}` (lib.rs 2246–2253), so the
//!   `Clip` stays, and the yes that moves the items never takes it off. That is the first test's
//!   last assertion.
//! - `dialogs::calendar_question` (1058–1177) reads the legacy `id`/`sel.event` only, so a Delete
//!   of a typed set of two hundred has no `id` to find and asks nothing; the question, its
//!   wording and the geometry of its buttons are the second test's seam.
//!
//! The first test reads the `Clip` the window itself stored (`calendar_view::clip`), presses the
//! question's own buttons where access kit says they are drawn, and reads the mailbox back. No
//! assertion here predicts a colour, a pixel or the wording of a sentence beyond the item titles
//! and dates a question must name.

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;
use mailcraft_model::{Address, Attendee, Event, Origin};
use serde_json::{Value, json};

use super::super::{CLIP_ID, Clip};
use super::*;
use crate::dialogs::Dialog;
use crate::people_view::testing::{dialog, now, window_on};

/// The account whose server keeps the meetings of the first test: an invented mailbox of this
/// computer's own, on `example.org`, with no server ever reached (`account.add` only records it).
const KEPT_ACCOUNT: &str = "zoe@example.org";

/// The `Clip` on the calendar's clipboard, as `calendar_view::clip` put it there: the items, the
/// cut flag, what the system's clipboard says and the mailbox it was taken from.
fn clipboard(w: &Window) -> Option<Clip> {
    let id = egui::Id::unique(CLIP_ID);
    w.ctx.data(|data| data.get_temp::<Clip>(id))
}

/// Where `id` starts now (and whether it is still there at all).
fn starts_at(w: &Window, id: Id) -> Option<DateTime> {
    w.state().session.mailbox.event(id).map(|event| event.start)
}

/// The open question, as the window holds it: the command it is about, the label of the button
/// that goes ahead, the label of the button that goes nowhere (`Cancel` when the question doesn't
/// say otherwise), and the body of what the owner is told. `why` says what was expected, so that a
/// question that is missing fails with the expectation rather than with a bare panic.
fn open_question(w: &Window, why: &str) -> (String, String, String, String) {
    match &w.state().dialog {
        Some(Dialog::Confirm { command, ask, .. }) => {
            (command.clone(), ask.action.clone(), ask.cancel.clone().unwrap_or_else(|| "Cancel".to_string()), ask.text.clone())
        }
        Some(other) => panic!("{why}: the window is asking something else ({})", other.name()),
        None => panic!("{why}: no question is open"),
    }
}

/// The lowest button of the window that says exactly `label`: the dialog is drawn over the
/// window, so where the ribbon has a button of the same name the question's is the lower one.
fn button_rect(w: &Window, label: &str) -> Option<Rect> {
    let mut found: Vec<Rect> = w
        .query_all_by_label_contains(label)
        .filter(|node| node.accesskit_node().role() == Role::Button)
        .filter(|node| node.accesskit_node().label().is_some_and(|said| said.trim() == label))
        .map(|node| node.rect())
        .collect();
    found.sort_by(|a, b| b.min.y.total_cmp(&a.min.y));
    found.first().copied()
}

/// A window on the sample mailbox whose `Lunch with Dev` and `Dentist` an account's server keeps:
/// each is marked with an origin of that account (a remote id of its own, and not somebody else's
/// meeting), an organizer and one person invited, through the model's own fields. A cut of them is
/// then a move the server must tell those people about — `calendar::mail_of` says `Update`, never a
/// simulated answer — and the meeting itself is what moves, keeping its identity.
///
/// The two events are returned as they were before anything was cut, for what the paste must keep.
fn kept_window() -> (Window, Event, Event) {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(now(), 0), now(), 0);
    let added = session
        .run("account.add", &json!({ "email": KEPT_ACCOUNT, "name": "Zoe", "incoming": { "host": "imap.example.org" } }))
        .expect("an account is added");
    let account = added["id"].as_u64().expect("the account's id");
    let mut marked: Vec<Event> = Vec::new();
    for title in ["Lunch with Dev", "Dentist"] {
        let id = session.mailbox.events.values().find(|event| event.title == title).map(|event| event.id).expect("the sample has it");
        let event = session.mailbox.event_mut(id).expect("the event is there");
        event.origin = Origin { account, id: format!("server-{id}"), ..Origin::default() };
        event.organizer = Some(Address::new("Dev Example", "dev@example.org"));
        event.attendees = vec![Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Attendee::default() }];
        marked.push(event.clone());
    }
    let mut w = window_on(session.mailbox, "view.calendar");
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
    // (Each meeting is kept by that account under a remote id of its own, and is nobody else's
    // meeting: the model's own words for it.)
    assert_eq!(marked.len(), 2, "fixture: the two sample meetings");
    for event in &marked {
        assert_eq!(event.origin.account, account, "fixture: kept by the account's server");
        assert_eq!(event.origin.id, format!("server-{}", event.id), "fixture: under a remote id of its own");
        assert!(!event.origin.guest, "fixture: the owner's own meeting, not one they were invited to");
        assert!(event.organizer.is_some() && !event.attendees.is_empty(), "fixture: people the server would tell");
    }
    (w, marked[0].clone(), marked[1].clone())
}

/// A cut of two meetings an account's server keeps waits for one question that names both; the
/// clipboard keeps the cut through that question being cancelled, and takes it off only when the
/// confirmed paste actually moved the meetings. A second Paste after the yes must then find
/// nothing to move, and one Undo must put both back where they were.
///
/// The two steps that make it a seam: the question is the one a cut of owner-kept meetings must
/// ask (nothing moves while it waits, and Cancel or a refusal keeps the cut pasteable), and the
/// successful yes is the *only* thing that consumes the clipboard. Consuming it when the question
/// opens, or never consuming it, both leave the owner able to paste one cut twice.
#[test]
fn a_cut_is_taken_off_the_clipboard_only_by_the_confirmed_paste_that_moved_it() {
    let (mut w, lunch, dentist) = kept_window();
    // (A click first: the calendar is what the keys are for.)
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    w.state_mut()
        .execute("calendar.select", json!({ "items": [{ "id": lunch.id }, { "id": dentist.id }] }))
        .expect("the two meetings are selected as a typed set");
    w.run_steps(2);
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "fixture: a typed set of the two meetings");

    // Ctrl+X: the set goes on the clipboard as a cut, and stays where it is.
    chord(&mut w, COMMAND, egui::Key::X);
    let cut = clipboard(&w).expect("Cut puts the set on the calendar's clipboard");
    assert!(cut.cut, "the clipboard holds a cut, not a copy");
    assert_eq!(cut.items.len(), 2, "both members of the set are on it");
    assert_eq!((starts_at(&w, lunch.id), starts_at(&w, dentist.id)), (Some(lunch.start), Some(dentist.start)), "nothing moves until it is pasted");

    // A time picked, and Paste: one question about the cut, and nothing moved while it waits.
    let before = w.state().session.mailbox.clone();
    let picked = after_noon(&w, 3.0);
    click(&mut w, picked, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    let (command, _, cancel, text) = open_question(&w, "a cut of two meetings an account's server keeps must be asked about once");
    assert_eq!(command, "calendar.paste", "the question is about the paste, not another command");
    for title in ["Lunch with Dev", "Dentist"] {
        assert!(text.contains(title), "the one question names {title}: {text}");
    }
    let clock = crate::dates::clock(at(date(10, 14), 15, 0), w.state().session.prefs.twelve_hour);
    assert!(text.contains(clock.as_str()), "and where they go, {clock}: {text}");
    assert!(
        w.state().session.mailbox == before,
        "a paste that is still waiting for the owner moves nothing: no member of the cut is moved and none is dropped"
    );
    assert!(clipboard(&w).is_some(), "a cut whose question is still open stays on the clipboard");

    // Cancel: the question goes, and so does nothing else.
    let nowhere = button_rect(&w, &cancel).unwrap_or_else(|| panic!("the question has a button that says {cancel:?}"));
    click(&mut w, nowhere.center(), Modifiers::NONE, PointerButton::Primary);
    w.run_steps(3);
    assert_eq!(dialog(&w), None, "Cancel closes the question");
    assert!(w.state().session.mailbox == before, "Cancel moves nothing and deletes nothing");
    assert!(clipboard(&w).is_some(), "a cancelled cut stays on the clipboard, to be pasted again");

    // Paste again — the click on the time is a second or so of window time, so the paste is not
    // the earlier click's repeated delivery (the duplicate guard in `paste`, which is left as it
    // is: a real second Paste is what this is) — and this time say yes.
    let picked = after_noon(&w, 3.0);
    click(&mut w, picked, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    let (again, action, _, _) = open_question(&w, "the cut is still on the clipboard, so a second Paste asks again");
    assert_eq!(again, "calendar.paste");
    assert!(w.state().session.mailbox == before, "the second question moves nothing either, until it is answered");
    assert!(clipboard(&w).is_some(), "a question that is still open keeps the cut");

    let go = button_rect(&w, &action).unwrap_or_else(|| panic!("the question goes ahead with a button that says {action:?}"));
    click(&mut w, go.center(), Modifiers::NONE, PointerButton::Primary);
    w.run_steps(3);
    assert_eq!(dialog(&w), None, "the yes is not asked about again");

    // Both meetings moved — the meetings themselves, in one step, keeping their identity, their
    // lengths and the time of day between them.
    let moved = w.state().session.mailbox.event(lunch.id).expect("a cut moves the meeting itself, not a copy of it");
    assert_eq!(
        (moved.start, moved.end),
        (at(date(10, 14), 15, 0), at(date(10, 14), 16, 0)),
        "the earliest of the set lands where the time was picked, and keeps its length"
    );
    let other = w.state().session.mailbox.event(dentist.id).expect("the second meeting is the same event, not a copy");
    assert_eq!(
        moved.start.minutes_until(other.start),
        lunch.start.minutes_until(dentist.start),
        "the second keeps the wall-clock time between it and the one that was anchored"
    );
    assert_eq!(other.duration_minutes(), dentist.duration_minutes(), "and keeps its own length");
    assert_ne!(other.start, dentist.start, "and it did move");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len(), "a cut moves them: none is copied, none is dropped");

    // The confirmed paste is the paste that consumed the cut: what is left on the clipboard is
    // nothing at all, so a further Paste can't move the same meetings a second time.
    assert!(
        clipboard(&w).is_none(),
        "a cut is used up by the paste that went through: the yes that moved the meetings must take it off the clipboard"
    );
    let moved_to = (starts_at(&w, lunch.id), starts_at(&w, dentist.id));
    let later = after_noon(&w, 5.0);
    click(&mut w, later, Modifiers::NONE, PointerButton::Primary);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    assert_eq!(
        (starts_at(&w, lunch.id), starts_at(&w, dentist.id)),
        moved_to,
        "the spent cut is not pasted again: the meetings stay where the confirmed paste put them"
    );

    // One step back puts both where they were.
    assert!(w.state_mut().session.undo(), "a cut and paste of a set is one step");
    w.run_steps(2);
    assert_eq!((starts_at(&w, lunch.id), starts_at(&w, dentist.id)), (Some(lunch.start), Some(dentist.start)), "one step takes both back");
}

/// Two hundred meetings an account's server keeps: the largest set a selection holds
/// (`MAX_SELECTED_EVENTS`), each an appointment of its own (its own remote id and title), with the
/// organizer and one person invited, through the model's own fields. The mailbox is emptied first,
/// so that every item the question has to name is one of these.
///
/// Returned in selection order, with the day of the first and of the last.
fn two_hundred_kept() -> (mailcraft_model::Mailbox, Vec<Id>, Date, Date) {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(now(), 0), now(), 0);
    session.mailbox.events.clear();
    let added = session
        .run("account.add", &json!({ "email": KEPT_ACCOUNT, "name": "Zoe", "incoming": { "host": "imap.example.org" } }))
        .expect("an account is added");
    let account = added["id"].as_u64().expect("the account's id");
    let calendar = session.mailbox.default_calendar().expect("the mailbox has a calendar");
    let first = at(date(10, 1), 9, 0);
    let mut ids = Vec::with_capacity(200);
    for n in 0..200i64 {
        let start = first.add_minutes(n.saturating_mul(90));
        let made = session
            .mailbox
            .add_event(Event {
                title: format!("Batch {n:03}"),
                start,
                end: start.add_minutes(30),
                calendar,
                organizer: Some(Address::new("Zoe Example", KEPT_ACCOUNT)),
                // (Somebody else to each: a batch counts each person once.)
                attendees: vec![Attendee { address: Address::new("Bo Example", format!("bo{n}@example.org")), ..Attendee::default() }],
                origin: Origin { account, id: format!("server-batch-{n}"), ..Origin::default() },
                ..Event::default()
            })
            .expect("the calendar takes a meeting");
        ids.push(made);
    }
    assert_eq!(ids.len(), 200, "fixture: two hundred meetings, the largest set a selection holds");
    // Deleting them would make the account's server tell two hundred people: the question that
    // has to be asked is the one about a batch that writes to them.
    let items: Vec<Value> = ids.iter().map(|id| json!({ "id": *id })).collect();
    assert!(
        matches!(
            mailcraft_engine::cmd::calendar::mail_of(&session, "calendar.delete", &json!({ "items": items })),
            Some(mailcraft_engine::cmd::calendar::Mail::Cancellation(200))
        ),
        "fixture: deleting the set requires the whole batch's cancellation consent"
    );
    let head = ids.first().and_then(|id| session.mailbox.event(*id)).map(|event| event.start.date).expect("the first meeting is there");
    let tail = ids.last().and_then(|id| session.mailbox.event(*id)).map(|event| event.start.date).expect("the last meeting is there");
    (session.mailbox, ids, head, tail)
}

/// The largest set a selection holds is asked about in one question, in a window at the least size
/// it can be made, in either palette: the question names the first and the last of the two hundred
/// with their dates and the count itself, nothing is deleted while it waits, and the two buttons
/// that answer it are drawn whole inside the window and apart from each other — so that Cancel can
/// be pressed, and it deletes none of them.
///
/// The least size is the one the program's own window is built with
/// (`apps/mailcraft/src/main.rs`: `with_min_inner_size([760.0, 480.0])`), which a window can't be
/// made smaller than; the harness is that window, laid out offscreen, in process.
#[test]
fn two_hundred_items_are_asked_about_once_with_their_buttons_in_reach_at_the_least_size() {
    for theme in ["light", "dark"] {
        let (mailbox, ids, first_day, last_day) = two_hundred_kept();
        let mut w = window_on(mailbox, "view.calendar");
        w.state_mut().execute("ui.theme", json!({ "value": theme })).expect("the window takes the theme");
        w.state_mut().session.driver.set_auto_sync(false);
        let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
        let _ = w.state_mut().execute("view.week", json!({}));
        w.set_size(egui::vec2(760.0, 480.0));
        w.run_steps(3);

        // The largest set a selection holds, named item by item through `calendar.select` (which
        // pins exactly these, in this order) rather than through Select All, whose answer depends
        // on the range in view — a seam of its own, held by the edge tests.
        let items: Vec<Value> = ids.iter().map(|id| json!({ "id": *id })).collect();
        w.state_mut().execute("calendar.select", json!({ "items": items })).expect("all two hundred are selected");
        w.run_steps(2);
        assert_eq!(w.state().session.sel.events.len(), 200, "{theme}: fixture: a selection of the largest size");

        // Delete: one question about the whole set, before anything goes.
        let before = w.state().session.mailbox.clone();
        chord(&mut w, Modifiers::NONE, egui::Key::Delete);
        w.run_steps(3);
        let (command, action, cancel, text) =
            open_question(&w, &format!("{theme}: Delete of two hundred meetings must be asked about once, as that batch"));
        assert_eq!(command, "calendar.delete", "{theme}: the question is about the Delete");
        assert!(text.contains("Batch 000"), "{theme}: the question names the first of the set: {text}");
        assert!(text.contains("Batch 199"), "{theme}: and the last of it: {text}");
        assert!(text.contains(crate::dates::long(first_day).as_str()), "{theme}: with the first one's own date: {text}");
        assert!(text.contains(crate::dates::long(last_day).as_str()), "{theme}: and the last one's: {text}");
        assert!(text.contains("200"), "{theme}: and the count the question is about: {text}");
        assert!(w.state().session.mailbox == before, "{theme}: nothing is deleted while the question waits");
        assert_eq!(w.state().session.mailbox.events.len(), 200, "{theme}: all two hundred are still there");

        // Both answers are drawn whole inside the window, and apart.
        let viewport = w.ctx.content_rect();
        let go = button_rect(&w, &action).unwrap_or_else(|| panic!("{theme}: the question goes ahead with a button that says {action:?}"));
        let nowhere = button_rect(&w, &cancel).unwrap_or_else(|| panic!("{theme}: the question has a button that says {cancel:?}"));
        assert!(viewport.contains_rect(go), "{theme}: the way of going ahead is drawn whole inside the window ({go:?} in {viewport:?})");
        assert!(viewport.contains_rect(nowhere), "{theme}: and so is the way of going nowhere ({nowhere:?} in {viewport:?})");
        assert!(!go.intersects(nowhere), "{theme}: the two answers do not overlap ({go:?} and {nowhere:?})");

        // Cancel, pressed where it is drawn: none of the two hundred goes.
        click(&mut w, nowhere.center(), Modifiers::NONE, PointerButton::Primary);
        w.run_steps(3);
        assert_eq!(dialog(&w), None, "{theme}: Cancel closes the question");
        assert!(w.state().session.mailbox == before, "{theme}: Cancel deletes none of the two hundred, and changes nothing else");
        assert_eq!(w.state().session.mailbox.events.len(), 200, "{theme}: all two hundred are still there");
    }
}

#[path = "legacy_single_stale_tests.rs"]
mod legacy_single_stale_tests;

#[path = "unchanged_single_cut_tests.rs"]
mod unchanged_single_cut_tests;
