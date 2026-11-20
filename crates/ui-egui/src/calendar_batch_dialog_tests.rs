// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's side of a batch of calendar items: what the window asks before a set is deleted
//! or cut and pasted, what that question names, what the owner's yes reaches, and what the window
//! says after Select All. The engine already takes a typed set (`calendar::selected_items`,
//! `delete_selected`, `paste`, `select_all`); these hold the window to naming that same set.
//!
//! One module of `agent_gate::calendar_batch_guard_tests`, so `super::*` is that file's fixtures
//! — `app()`, `Ids`, `calendar()`, `d`, `at`, `NOW` and
//! `START` — and this file adds only the fixtures the owner's questions need. Nothing here is
//! production code and nothing here is a test hook: the window is the real `MailApp`, the answers
//! are the buttons the owner presses in `Dialog::Confirm` (`dialogs.rs` 1922–1981 → `answered` →
//! `go_ahead`), and every appointment is synthetic (`example.org` people, `Mailbox::add_event`),
//! with no file, server, real account or wall clock.
//!
//! What the source did when these were written (2026-11-15), and why each test holds it:
//! - `dialogs::confirmation` (430–716) does not list `calendar.paste` at all, so a cut that needs
//!   the people invited to be told is refused by the engine as unsent instead of asked about.
//! - `dialogs::calendar_question` (1058–1177) reads only the legacy `id`/selection focus
//!   (1077–1083) and asks the which-of-the-series question for whatever `sel.event_day` maps
//!   (1084–1113), even when the command carries a typed `items` list: a batch Delete of a typed
//!   set whose focus is an occurrence is asked as if the whole series were meant.
//! - `dialogs::pin_targets` (852–865) pins only `people.delete`, `folder.empty` and
//!   `folder.delete`; a batch Delete's `items` are never written into the question, so the
//!   selection at answer time — not the set the owner was shown — is what a yes reaches.
//! - `dialogs::go_ahead` (990) has no calendar-member validation: a member that went or changed
//!   while the question was up is not compared with what was shown.
//! - `lib::feedback` (5622) has no `calendar.selectAll` arm, though the engine answers
//!   `{"selected": n, "capped": bool}` (`selection.rs` 330–365), so Select All's cap is silent.
//!
//! `windows::takes_away` (554–560) reading only `id`/focus is the separate unsaved-window gap
//! already held by `calendar_batch_guard_tests`; it is not repeated here. Neither is any native
//! clear: this file is component-level UI only.

use std::collections::HashSet;

use egui::accesskit::Role;
use egui::{Modifiers, PointerButton, Pos2, Rect};
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::CalView;
use mailcraft_model::{Address, Attendee, Origin};
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::people_view::testing::{Window, dialog, window_on};

use super::*;

/// The second account's address: the server that would tell the people invited, named in the
/// question that asks about writing to them.
const KEPT_ACCOUNT: &str = "zoe@example.org";

/// The two appointments a server keeps, the local series a batch can also name a day of, and one
/// appointment no batch names (what the owner selects while a question is up).
struct Kept {
    calendar_name: String,
    first: Id,
    second: Id,
    series: Id,
    outsider: Id,
}

/// A mailbox whose meetings an account's server keeps, built only from the model the engine's own
/// batch tests use (`crates/engine/tests/calendar_batch_review_tests.rs`: `account.add`,
/// `attendee()`, `Origin { account, id }`): no server, service or real account is involved.
///
/// `Kickoff` is 2026-11-09 09:00–10:00 and `Review` 2026-11-11 10:15–11:00, each with one person
/// invited and kept by `zoe@example.org`; a cut of either writes one update. The calendar is
/// renamed so that a question naming where the items go cannot be satisfied by the word
/// "Calendar" appearing for another reason.
fn kept_mailbox() -> (Mailbox, Kept) {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    session.mailbox.events.clear();
    let added = session
        .run("account.add", &json!({"email": KEPT_ACCOUNT, "name": "Zoe", "incoming": {"host": "imap.example.org"}}))
        .expect("an account is added");
    let account = added["id"].as_u64().expect("the account's id");
    session.mailbox.calendars[0].name = "Team Calendar".into();
    let calendar = session.mailbox.default_calendar().expect("the mailbox has a calendar");
    let attendee = || Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Default::default() };
    let kept = |session: &mut Session, title: &str, start: DateTime, minutes: i64, remote: &str| -> Id {
        session
            .mailbox
            .add_event(Event {
                title: title.into(),
                start,
                end: start.add_minutes(minutes),
                calendar,
                organizer: Some(Address::new("Zoe Example", KEPT_ACCOUNT)),
                attendees: vec![attendee()],
                origin: Origin { account, id: remote.into(), ..Default::default() },
                ..Default::default()
            })
            .expect("the calendar takes a meeting")
    };
    let first = kept(&mut session, "Kickoff", at(d(2026, 11, 9), 9, 0), 60, "server-1");
    let second = kept(&mut session, "Review", at(d(2026, 11, 11), 10, 15), 45, "server-2");
    let series_start = at(d(2026, 11, 4), 9, 0);
    let series = session
        .mailbox
        .add_event(Event {
            title: "Weekly sync".into(),
            start: series_start,
            end: series_start.add_minutes(30),
            calendar,
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            ..Default::default()
        })
        .expect("the calendar takes a series");
    let den = at(d(2026, 11, 10), 14, 0);
    let outsider = session
        .mailbox
        .add_event(Event { title: "Dentist".into(), start: den, end: den.add_minutes(30), calendar, ..Default::default() })
        .expect("the calendar takes an appointment");
    let deleting = json!({"items": [{"id": first}, {"id": second}]});
    assert!(
        matches!(mailcraft_engine::cmd::calendar::mail_of(&session, "calendar.delete", &deleting), Some(mailcraft_engine::cmd::calendar::Mail::Cancellation(n)) if n > 0),
        "fixture: both synthetic meetings require cancellation consent"
    );
    let cutting = json!({"items": [{"id": first}, {"id": second}], "start": START, "cut": true});
    assert!(
        matches!(mailcraft_engine::cmd::calendar::mail_of(&session, "calendar.paste", &cutting), Some(mailcraft_engine::cmd::calendar::Mail::Update(n)) if n > 0),
        "fixture: the synthetic cut requires update consent"
    );
    (session.mailbox, Kept { calendar_name: "Team Calendar".into(), first, second, series, outsider })
}

fn kept_app() -> (MailApp, Kept) {
    let (mailbox, kept) = kept_mailbox();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    (app, kept)
}

fn kept_window() -> (Window, Kept) {
    let (mailbox, kept) = kept_mailbox();
    let mut window = window_on(mailbox, "view.calendar");
    window.state_mut().session.driver.set_auto_sync(false);
    let _ = window.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    window.run_steps(3);
    (window, kept)
}

fn local_window() -> (Window, Ids) {
    let (mailbox, ids) = calendar();
    let mut window = window_on(mailbox, "view.calendar");
    window.state_mut().session.driver.set_auto_sync(false);
    let _ = window.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    window.run_steps(3);
    (window, ids)
}

/// Whether `text` names `day` as the window writes days (`dates.rs`: the long, medium and
/// weekday-medium forms) or as an ISO day: every one is the day the item is named by, never one
/// it happens to be shown on.
fn names_day(text: &str, day: Date) -> bool {
    let long = crate::dates::long(day);
    let medium = crate::dates::medium(day);
    let weekday = crate::dates::weekday_medium(day);
    let iso = day.iso();
    text.contains(iso.as_str()) || text.contains(long.as_str()) || text.contains(medium.as_str()) || text.contains(weekday.as_str())
}

/// The open question's own text, as `Dialog::Confirm` holds it.
fn owner_question_text(window: &Window) -> String {
    match &window.state().dialog {
        Some(Dialog::Confirm { ask, .. }) => ask.text.clone(),
        _ => panic!("no question is open"),
    }
}

/// The open question's list of items, as `Dialog::Confirm` holds it.
fn owner_question_listing(window: &Window) -> crate::dialogs::Listing {
    match &window.state().dialog {
        Some(Dialog::Confirm { ask, .. }) => ask.listing.clone().expect("a batch question lists its items"),
        _ => panic!("no question is open"),
    }
}

/// The open question's own action button label, as `Dialog::Confirm` holds it: what the test
/// presses without taking the question out of the window (the binding it carries stays with it).
fn owner_question_action(window: &Window) -> String {
    match &window.state().dialog {
        Some(Dialog::Confirm { ask, .. }) => ask.action.clone(),
        _ => panic!("no question is open"),
    }
}

/// One pointer click at `pos`, a moment after whatever came before.
fn click(window: &mut Window, pos: Pos2) {
    window.run_steps(45);
    window.input_mut().events.push(egui::Event::PointerMoved(pos));
    window.step();
    for pressed in [true, false] {
        window.input_mut().events.push(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        window.step();
    }
    window.run_steps(2);
}

/// Press the one button called `label`. The question is drawn over the window, so where the
/// ribbon has a button of the same name the dialog's is the lower one.
fn press(window: &mut Window, label: &str) {
    let mut at: Vec<Rect> = window
        .query_all_by_label_contains(label)
        .filter(|node| node.accesskit_node().role() == Role::Button && node.accesskit_node().label().is_some_and(|said| said.trim() == label))
        .map(|node| node.rect())
        .collect();
    assert!(!at.is_empty(), "no button of the window says {label:?}");
    at.sort_by(|a, b| b.min.y.total_cmp(&a.min.y));
    let pos = at[0].center();
    click(window, pos);
    window.run_steps(3);
}

/// Select the two meetings as a typed set and open Delete's question, through the owner's own
/// `MailApp::run` (not `execute`): only the run path asks.
fn pending_delete_of_both(window: &mut Window, kept: &Kept) {
    window
        .state_mut()
        .execute("calendar.select", json!({ "items": [{ "id": kept.first }, { "id": kept.second }] }))
        .expect("the two meetings are selected");
    assert_eq!(window.state().session.sel.events.len(), 2, "fixture: a typed set of two");
    let said = window.state_mut().run("calendar.delete", json!({})).expect("a batch Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "fixture: the owner is asked, not run over: {said}");
    window.run_steps(3);
    assert_eq!(dialog(window), Some("confirm"), "fixture: one question is on screen");
}

/// Press the open question's action, as the owner does. The question stays in `app.dialog` until
/// the window's own `Dialog::Confirm` code reads it, so whatever binding the question carries is
/// the one the answer is checked against.
fn answer_question(window: &mut Window) {
    let action = owner_question_action(window);
    press(window, &action);
    window.run_steps(3);
}

/// A typed set that names one whole appointment and one occurrence of a series is deleted as
/// those items: the focus, which is the occurrence, never brings back the which-of-the-series
/// question, and the same holds when the items are given explicitly while the focus names another
/// day of the series. A legacy `id` keeps the old chooser (the next test).
#[test]
fn a_typed_batch_delete_is_its_items_and_not_the_series_in_focus() {
    // The selected set: an ordinary appointment and one occurrence of a weekly series, which is
    // last and so has the focus (`calendar.select` makes the last item the focus).
    let (mut app, ids) = super::app();
    let day = d(2026, 11, 18);
    app.execute("calendar.select", json!({ "items": [{ "id": ids.kickoff }, { "id": ids.series, "day": day.iso() }] }))
        .expect("the typed set is selected");
    assert_eq!(app.session.sel.event, Some(ids.series), "fixture: the occurrence of the series is in focus");
    assert_eq!(app.session.sel.event_day, Some(day), "fixture: so `calendar_question` can find a day for it");

    let said = app.run("calendar.delete", json!({})).expect("the owner's Delete runs");
    assert!(
        app.dialog.is_none(),
        "a typed set is deleted as the items name it: the one series its focus names must not ask which of the series is meant"
    );
    assert!(app.session.mailbox.event(ids.kickoff).is_none(), "the ordinary appointment of the set went");
    assert_eq!(
        app.session.mailbox.event(ids.series).map(|series| series.exceptions.clone()),
        Some(vec![day]),
        "and only the occurrence the set named left the series"
    );
    assert_eq!(said["deleted"], json!(2), "both items were deleted, not one: {said}");

    // The same items given explicitly, while the legacy focus names another day of that series:
    // the focus must not decide what is deleted or what is asked.
    let (mut app, ids) = super::app();
    let shown = d(2026, 11, 18);
    let named = d(2026, 11, 25);
    app.execute("calendar.select", json!({ "id": ids.series, "day": shown.iso() })).expect("the legacy focus is a day of the series");
    assert_eq!((app.session.sel.event, app.session.sel.event_day), (Some(ids.series), Some(shown)), "fixture: the focus is a day no item names");
    let said = app
        .run("calendar.delete", json!({ "items": [{ "id": ids.kickoff }, { "id": ids.series, "day": named.iso() }] }))
        .expect("the owner's Delete runs");
    assert!(
        app.dialog.is_none(),
        "explicit items are answered from the items: the focus's day of a series must not open the which-of-the-series question"
    );
    assert!(app.session.mailbox.event(ids.kickoff).is_none(), "the appointment the items named went");
    let series = app.session.mailbox.event(ids.series).expect("the series still stands");
    assert_eq!(series.exceptions, vec![named], "the day the items named is the one that went");
    assert!(!series.exceptions.contains(&shown), "the day the focus named, but no item did, still happens");
    assert_eq!(said["deleted"], json!(2), "both items were deleted: {said}");
}

/// A single event named by `id`, still, is the old question: Just This One or the whole series.
/// The batch rules must not take that choice away from the owner.
#[test]
fn a_legacy_single_recurring_delete_still_asks_which_of_the_series() {
    let (mut window, ids) = local_window();
    let day = d(2026, 11, 18);
    window.state_mut().execute("calendar.select", json!({ "id": ids.series, "day": day.iso() })).expect("the owner selects the occurrence");
    window.state_mut().run("calendar.delete", json!({ "id": ids.series })).expect("the question is asked before anything goes");
    window.run_steps(3);

    assert_eq!(dialog(&window), Some("confirm"), "one appointment of a series is still asked about");
    match &window.state().dialog {
        Some(Dialog::Confirm { command, ask, .. }) => {
            assert_eq!(command.as_str(), "calendar.delete");
            assert_eq!(ask.title.as_str(), "Delete Recurring Item", "the old chooser's own title");
            assert_eq!(ask.action.as_str(), "Just This One", "and its one-occurrence way");
            assert!(ask.more.iter().any(|(label, _)| label.as_str() == "The Entire Series"), "and the whole-series way: {:?}", ask.more);
        }
        _ => panic!("no question is open"),
    }

    press(&mut window, "Just This One");
    let series = window.state().session.mailbox.event(ids.series).cloned().expect("the series is still there");
    assert_eq!(series.exceptions, vec![day], "the answer took the one the owner chose, and nothing else");
}

/// What a batch Delete's question carries, and what its yes may reach: every typed item, each
/// with its own day where it has one, never a legacy `id` taken from the focus. A target the
/// payload names itself — even a malformed one, or `null` — is never quietly replaced by the
/// selection or the focus; and when the owner changes the selection under the question, the yes
/// still deletes the two meetings it showed.
#[test]
fn a_batch_delete_question_pins_the_set_it_showed_and_never_retargets() {
    let (mut app, kept) = kept_app();
    let day = d(2026, 11, 18);
    app.execute("calendar.select", json!({ "items": [{ "id": kept.first }, { "id": kept.series, "day": day.iso() }] }))
        .expect("the typed set is selected");
    assert_eq!(app.session.sel.event, Some(kept.series), "fixture: the focus is the occurrence");

    let pinned = crate::dialogs::pin_targets(&app, "calendar.delete", json!({}));
    assert!(pinned.get("id").is_none(), "the question carries the set, never one legacy id from the focus: {pinned}");
    let items = pinned.get("items").and_then(Value::as_array).expect("the question carries the items it counted");
    assert_eq!(items.len(), 2, "every item of the set, once: {pinned}");
    let got: HashSet<(Id, Option<String>)> = items
        .iter()
        .map(|item| (item["id"].as_u64().expect("an item id"), item.get("day").and_then(Value::as_str).map(|day| day.to_string())))
        .collect();
    let want: HashSet<(Id, Option<String>)> = [(kept.first, None), (kept.series, Some(day.iso()))].into_iter().collect();
    assert_eq!(got, want, "a whole event has no day; an occurrence has the day the series names it by: {pinned}");

    // A valid explicit list is the command's own target and stays exactly as given.
    let explicit = json!({ "items": [{ "id": kept.first }] });
    assert_eq!(
        crate::dialogs::pin_targets(&app, "calendar.delete", explicit.clone()),
        explicit,
        "explicit items are never rewritten from the selection"
    );
    // No list, or one that names itself invalid, is the command's to refuse: it is never replaced
    // from the focus or the selection (the fail-open this pins is a yes landing on another item).
    for bad in [
        json!({ "items": null }),
        json!({ "items": [] }),
        json!({ "items": "not an item" }),
        json!({ "items": [{ "id": 0 }] }),
        json!({ "items": [{ "id": kept.first }], "id": kept.outsider }),
        json!({ "items": [{ "id": kept.first }], "day": day.iso() }),
        json!({ "items": null, "id": null }),
        json!({ "id": null }),
        json!({ "id": kept.outsider }),
    ] {
        assert_eq!(
            crate::dialogs::pin_targets(&app, "calendar.delete", bad.clone()),
            bad,
            "a target the payload names (even badly) is never replaced from the focus or the selection"
        );
    }

    // And through the window: the owner changes the selection while the batch Delete's question
    // waits (a click in the grid), and the yes must still reach the two meetings it showed.
    let (mut window, kept) = kept_window();
    pending_delete_of_both(&mut window, &kept);
    window.state_mut().execute("calendar.select", json!({ "id": kept.outsider })).expect("the owner selects another appointment");
    window.run_steps(2);
    assert_eq!(window.state().session.sel.event, Some(kept.outsider), "fixture: the selection is no longer the set the question showed");

    answer_question(&mut window);
    let mailbox = &window.state().session.mailbox;
    assert!(mailbox.event(kept.first).is_none(), "the first meeting the question counted went");
    assert!(mailbox.event(kept.second).is_none(), "and the second");
    assert!(mailbox.event(kept.outsider).is_some(), "the appointment selected while the question was up is not what the owner's yes reaches");
    assert!(window.state().dialog.is_none(), "the yes is not asked about again");
}

/// Cutting two meetings that an account's server keeps must reach the people invited, so it asks
/// once: one question naming every item, its own date, and where they go (the time asked for, the
/// calendar, the account), before anything moves. The yes moves both, and neither is dropped.
#[test]
fn a_cut_that_must_tell_people_asks_once_naming_every_item_and_where_they_go() {
    let (mut window, kept) = kept_window();
    let before = window.state().session.mailbox.clone();
    let said = window
        .state_mut()
        .run("calendar.paste", json!({ "items": [{ "id": kept.first }, { "id": kept.second }], "start": START, "cut": true }))
        .expect("the owner's cut waits for one question");
    window.run_steps(3);
    assert_eq!(said["pending"], "confirm", "a cut that writes to people is asked about, not refused as unasked: {said}");
    assert_eq!(dialog(&window), Some("confirm"), "one question is on screen");

    let text = owner_question_text(&window);
    for (title, day) in [("Kickoff", d(2026, 11, 9)), ("Review", d(2026, 11, 11))] {
        assert!(text.contains(title), "the one question names {title}: {text}");
        assert!(names_day(&text, day), "and the date {title} is named by: {text}");
    }
    let start = at(d(2026, 12, 2), 9, 0);
    assert!(names_day(&text, start.date), "and where they go: {text}");
    let clock = crate::dates::clock(start, window.state().session.prefs.twelve_hour);
    assert!(text.contains(clock.as_str()), "the time asked for, {clock}: {text}");
    assert!(text.contains(kept.calendar_name.as_str()), "the calendar they go to: {text}");
    assert!(text.contains(KEPT_ACCOUNT), "and the account whose server tells the people: {text}");

    assert!(
        window.state().session.mailbox == before,
        "nothing moves while the question waits: no item is silently dropped and no part of the cut is done"
    );

    answer_question(&mut window);
    let now = window.state().session.mailbox.clone();
    assert!(now.event(kept.first).is_some() && now.event(kept.second).is_some(), "the yes moves both items, not one of them");
    assert_eq!(
        now.event(kept.first).map(|event| (event.start, event.end)),
        Some((at(d(2026, 12, 2), 9, 0), at(d(2026, 12, 2), 10, 0))),
        "the earliest item lands where the time was asked for"
    );
    assert_eq!(
        now.event(kept.second).map(|event| (event.start, event.end)),
        Some((at(d(2026, 12, 4), 10, 15), at(d(2026, 12, 4), 11, 0))),
        "and the other keeps its day and time-of-day offset: an item that would be dropped is missing here"
    );
    assert_eq!(now.events.len(), before.events.len(), "a cut moves the appointments themselves: none is copied or dropped");
    assert!(window.state().dialog.is_none(), "the yes is not asked about again");
}

/// What the owner's yes may run on, after the question was shown, is the set it showed — not a
/// smaller one and not whatever now holds an id. A member that went, one that changed, and
/// another mailbox in view must leave the whole batch untouched.
#[test]
fn the_owner_question_refuses_the_whole_batch_when_a_member_went_or_changed() {
    // A member gone while the owner was being asked: another command takes it away.
    let (mut window, kept) = kept_window();
    pending_delete_of_both(&mut window, &kept);
    window.state_mut().execute("calendar.delete", json!({ "id": kept.second, "send": true })).expect("the other delete runs at once");
    window.run_steps(2);
    assert!(window.state().session.mailbox.event(kept.second).is_none(), "fixture: the second meeting is gone");
    let before = window.state().session.mailbox.clone();
    answer_question(&mut window);
    assert!(
        window.state().session.mailbox == before,
        "a member gone while the owner was asked refuses the whole batch: the other one must not be deleted on its own"
    );
    assert!(window.state().session.mailbox.event(kept.first).is_some(), "the member still there is kept, whole batch and all");

    // A member changed while the owner was being asked (an update from another device keeps the
    // id): the question showed the old one, so the whole batch is refused, never the new one.
    let (mut window, kept) = kept_window();
    pending_delete_of_both(&mut window, &kept);
    {
        let event = window.state_mut().session.mailbox.event_mut(kept.second).expect("the second meeting is there");
        event.title.push_str(" (changed elsewhere)");
        event.start = at(d(2026, 11, 11), 11, 15);
        event.end = at(d(2026, 11, 11), 12, 0);
    }
    window.run_steps(2);
    let before = window.state().session.mailbox.clone();
    answer_question(&mut window);
    assert!(
        window.state().session.mailbox == before,
        "a member changed while the owner was asked refuses the whole batch, rather than deleting what holds its id now"
    );

    // Another mailbox takes this one's place while the question is up. The window already drops a
    // question about the mailbox that was open (`lib.rs` 3666–3676); if a question is still there
    // and is answered, it must refuse. What it may never do is reach an appointment of the new
    // mailbox because the ids happen to be the same.
    let (mut window, kept) = kept_window();
    pending_delete_of_both(&mut window, &kept);
    let copy = window.state().session.mailbox.clone();
    window.state_mut().session.replace_mailbox(copy);
    window.run_steps(3);
    let before = window.state().session.mailbox.clone();
    if matches!(&window.state().dialog, Some(Dialog::Confirm { .. })) {
        answer_question(&mut window);
        let said = window.state().status_msg.as_ref().map(|(said, _)| said.to_lowercase()).unwrap_or_default();
        assert!(
            said.contains("mailbox") || said.contains("nothing"),
            "an answer to a question about another mailbox is refused, not run silently: {said:?}"
        );
    }
    assert!(window.state().session.mailbox == before, "nothing of the mailbox that replaced the one the question was about is changed");
    assert!(
        window.state().session.mailbox.event(kept.first).is_some() && window.state().session.mailbox.event(kept.second).is_some(),
        "the lookalike appointments of the new mailbox stay as they were"
    );
}

/// Select All's answer is what the owner is told: two hundred were selected and the range holds
/// more. The status bar never says that everything was selected.
#[test]
fn select_all_tells_the_owner_the_two_hundred_it_took_and_that_more_remain() {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    session.mailbox.events.clear();
    let calendar = session.mailbox.default_calendar().expect("the mailbox has a calendar");
    // More than a selection holds, all in the month in view.
    let first = at(d(2026, 10, 1), 0, 0);
    for n in 0..250i64 {
        let start = first.add_minutes(n.saturating_mul(90));
        session
            .mailbox
            .add_event(Event { title: format!("Slot {n}"), start, end: start.add_minutes(30), calendar, ..Default::default() })
            .expect("the calendar takes an appointment");
    }
    session.view.calendar = CalView::Month;
    session.view.date = d(2026, 10, 14);
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);

    let said = app.run("calendar.selectAll", json!({})).expect("the owner's Select All runs");
    assert_eq!(said["selected"], json!(200), "a selection holds two hundred: {said}");
    assert_eq!(said["capped"], json!(true), "and the range holds more: {said}");
    assert_eq!(app.session.sel.events.len(), 200, "fixture and behaviour: two hundred are selected");

    let status = app.status_msg.as_ref().map(|(said, _)| said.clone()).expect("the owner is told what Select All took");
    let lower = status.to_lowercase();
    assert!(lower.contains("200"), "the count the owner is told: {status:?}");
    assert!(
        lower.contains("more") || lower.contains("only") || lower.contains("first"),
        "and that the range holds more than the two hundred: {status:?}"
    );
    assert!(!lower.contains("all selected") && !lower.contains("selected all"), "never claims everything was selected: {status:?}");
}

/// The accounts a batch Delete's question names as telling people are those whose servers will
/// write to somebody: an ordinary appointment another account's server keeps sends nothing, and
/// that account is not named as telling anybody (CodeRabbit on #1665, dialogs.rs:1429).
#[test]
fn a_batch_question_names_only_the_accounts_whose_servers_tell_somebody() {
    let (mailbox, kept) = kept_mailbox();
    let mut session = Session::at(mailbox, NOW, 0);
    let added = session
        .run("account.add", &json!({"email": "yan@example.org", "name": "Yan", "incoming": {"host": "imap.example.net"}}))
        .expect("a second account is added");
    let account = added["id"].as_u64().expect("the account's id");
    let calendar = session.mailbox.default_calendar().expect("the mailbox has a calendar");
    let start = at(d(2026, 11, 12), 14, 0);
    let plain = session
        .mailbox
        .add_event(Event {
            title: "Plain appointment".into(),
            start,
            end: start.add_minutes(30),
            calendar,
            origin: Origin { account, id: "server-yan".into(), ..Default::default() },
            ..Default::default()
        })
        .expect("the calendar takes an appointment");
    assert!(
        mailcraft_engine::cmd::remote::keeper(&session.mailbox, &session.mailbox.event(plain).expect("it").origin).is_some(),
        "fixture: a server keeps it"
    );
    let mut window = window_on(session.mailbox, "view.calendar");
    window.state_mut().session.driver.set_auto_sync(false);
    let _ = window.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    window.run_steps(3);
    window.state_mut().execute("calendar.select", json!({ "items": [{ "id": kept.first }, { "id": plain }] })).expect("both are selected");
    let said = window.state_mut().run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    window.run_steps(3);
    let text = owner_question_text(&window);
    assert!(text.contains(KEPT_ACCOUNT), "the account whose server sends the cancellation is named: {text}");
    assert!(!text.contains("yan@example.org"), "an account whose server tells nobody is not named as telling: {text}");
}

/// The batch question names, for each item that writes to somebody, the account whose server sends
/// it and the people it goes to, from the plan the command counts by (owner decision D1; CodeRabbit
/// on #1665, dialogs.rs:1429): a count alone doesn't say who is written to. The items are grouped
/// under the one account that writes, each with its day and time; somebody without an address is
/// counted, and said to be one.
#[test]
fn a_batch_question_names_each_items_people_and_the_account_that_writes() {
    use mailcraft_engine::cmd::calendar::{Mail, delete_notices, mail_of, recipients};
    let (mut window, kept) = kept_window();
    {
        let review = window.state_mut().session.mailbox.event_mut(kept.second).expect("the review is there");
        review.attendees.push(Attendee { address: Address::new("Cy Example", "cy@example.org"), ..Default::default() });
        review.attendees.push(Attendee { address: Address::new("Room", ""), ..Default::default() });
    }
    window.run_steps(2);
    window.state_mut().execute("calendar.select", json!({ "items": [{ "id": kept.first }, { "id": kept.second }] })).expect("both are selected");
    let said = window.state_mut().run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    window.run_steps(3);
    let text = owner_question_text(&window);
    let listing = owner_question_listing(&window);
    assert_eq!(listing.groups.len(), 1, "one account writes, and every item of the set writes to somebody: {text}");
    let group = &listing.groups[0];
    assert!(group.heading.contains(KEPT_ACCOUNT) && group.heading.contains("Cancellations"), "the account and what it sends: {}", group.heading);
    let entry = |title: &str| {
        group
            .items
            .iter()
            .find(|item| item.what.contains(&format!("\"{title}\"")))
            .cloned()
            .unwrap_or_else(|| panic!("a line of its own for {title}: {text}"))
    };
    let kickoff = entry("Kickoff");
    let clock = crate::dates::clock(at(d(2026, 11, 9), 9, 0), window.state().session.prefs.twelve_hour);
    assert!(names_day(&kickoff.what, d(2026, 11, 9)) && kickoff.what.contains(clock.as_str()), "with its day and time: {}", kickoff.what);
    let to = kickoff.to.unwrap_or_default();
    assert!(to.contains("bo@example.org") && !to.contains("cy@example.org"), "Kickoff's person, and only Kickoff's: {to}");
    let review = entry("Review");
    let to = review.to.unwrap_or_default();
    assert!(to.contains("bo@example.org") && to.contains("cy@example.org"), "Review's people: {to}");
    assert!(to.contains("1 person without an address"), "and the room, which has none: {to}");
    assert_eq!(text.matches(KEPT_ACCOUNT).count(), 2, "the account is named in the summary and its heading, not on every line: {text}");

    let session = &window.state().session;
    let plan = delete_notices(session, &json!({})).expect("the selected set is a set");
    assert_eq!(recipients(&plan), ["bo@example.org", "cy@example.org"], "the plan writes to Bo and Cy by address, once each");
    assert_eq!(mail_of(session, "calendar.delete", &json!({})), Some(Mail::Cancellation(3)), "and to the room: the command counts the plan's people");
    assert!(text.contains("3 people invited"), "and the question says that count: {text}");
}

/// A large set's question stays one that can be read: every item is named, the first items that
/// write to people with the first few people of each, and the rest with how many.
#[test]
fn a_large_batch_question_lists_the_first_few_and_counts_the_rest() {
    let (mut mailbox, _) = kept_mailbox();
    let account = mailbox.accounts.iter().find(|account| account.email == KEPT_ACCOUNT).map(|account| account.id).expect("the kept account");
    let calendar = mailbox.default_calendar().expect("the mailbox has a calendar");
    let mut items: Vec<Value> = Vec::new();
    for n in 0..20i64 {
        let start = at(d(2026, 11, 10), 8, 0).add_minutes(n * 20);
        let attendees =
            (0..10).map(|k| Attendee { address: Address::new("Guest", format!("guest{n}-{k}@example.org")), ..Default::default() }).collect();
        let id = mailbox
            .add_event(Event {
                title: format!("Meeting {n}"),
                start,
                end: start.add_minutes(15),
                calendar,
                attendees,
                origin: Origin { account, id: format!("server-many-{n}"), ..Default::default() },
                ..Default::default()
            })
            .expect("the calendar takes a meeting");
        items.push(json!({ "id": id }));
    }
    let mut window = window_on(mailbox, "view.calendar");
    window.state_mut().session.driver.set_auto_sync(false);
    let _ = window.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    window.run_steps(3);
    window.state_mut().execute("calendar.select", json!({ "items": items })).expect("the set is selected");
    let said = window.state_mut().run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    window.run_steps(3);
    let text = owner_question_text(&window);
    let listing = owner_question_listing(&window);
    assert_eq!(listing.groups.len(), 1, "under the one account that writes: {text}");
    let items = &listing.groups[0].items;
    assert_eq!(items.len(), 20, "every item is named: {text}");
    let named = items.iter().filter(|item| item.to.as_deref().is_some_and(|to| to.contains('@'))).count();
    assert_eq!(named, 12, "the first twelve with their people: {text}");
    assert!(items.iter().skip(12).all(|item| item.to.as_deref() == Some("to 10 people")), "and the rest with how many: {text}");
    assert!(text.contains("guest0-5@example.org") && !text.contains("guest0-6@example.org"), "six people of each are named: {text}");
    assert!(text.contains("and 4 more"), "and the rest of them counted: {text}");
    assert!(text.contains("200 people invited"), "the count is everybody: {text}");
}

/// Two days of one series are told apart in the question, each by its own day, and a cut says
/// where each item goes (review of #1665: an item's line named its title alone).
#[test]
fn a_batch_question_tells_two_days_of_a_series_apart_and_says_where_a_cut_goes() {
    let (mut window, kept) = kept_window();
    window.state_mut().session.mailbox.event_mut(kept.first).expect("Kickoff is there").recurrence = Some(Recurrence::every(Freq::Weekly));
    window.run_steps(2);
    let (monday, next) = (d(2026, 11, 9), d(2026, 11, 16));
    window
        .state_mut()
        .execute("calendar.select", json!({ "items": [{ "id": kept.first, "day": monday.iso() }, { "id": kept.first, "day": next.iso() }] }))
        .expect("two days of the kept series are selected");
    let said = window.state_mut().run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    window.run_steps(3);
    let text = owner_question_text(&window);
    let listing = owner_question_listing(&window);
    let lines: Vec<&str> = listing.groups.iter().flat_map(|group| group.items.iter()).map(|item| item.what.as_str()).collect();
    assert_eq!(lines.len(), 2, "a line for each day: {text}");
    assert!(names_day(lines[0], monday) && names_day(lines[1], next), "each names its own day: {lines:?}");
    assert_ne!(lines[0], lines[1], "so the two are told apart: {text}");
    let _ = window.state_mut().execute("ui.closeDialog", json!({}));
    window.run_steps(2);

    // A cut of the two meetings: each line says where it goes.
    let (mut window, kept) = kept_window();
    let said = window
        .state_mut()
        .run("calendar.paste", json!({ "items": [{ "id": kept.first }, { "id": kept.second }], "start": START, "cut": true }))
        .expect("a cut that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    window.run_steps(3);
    let text = owner_question_text(&window);
    let listing = owner_question_listing(&window);
    let line = |title: &str| {
        listing
            .groups
            .iter()
            .flat_map(|group| group.items.iter())
            .find(|item| item.what.contains(&format!("\"{title}\"")))
            .map(|item| item.what.clone())
            .unwrap_or_else(|| panic!("a line for {title}: {text}"))
    };
    // The earliest, Kickoff, lands at the time asked for; Review keeps its distance from it.
    let kickoff = line("Kickoff");
    assert!(names_day(&kickoff, d(2026, 11, 9)) && names_day(&kickoff, d(2026, 12, 2)), "where it is and where it goes: {kickoff}");
    let review = line("Review");
    assert!(names_day(&review, d(2026, 11, 11)) && names_day(&review, d(2026, 12, 4)), "and the same for Review: {review}");
    let clock = crate::dates::clock(at(d(2026, 12, 4), 10, 15), window.state().session.prefs.twelve_hour);
    // (Read after "moves to": Review's own time is 10:15 too, which mustn't stand for the new one.)
    let to = review.split("moves to").nth(1).unwrap_or_default();
    assert!(names_day(to, d(2026, 12, 4)) && to.contains(clock.as_str()), "with its new day and time: {review}");
}

/// A set of meetings whose people have no address is asked about in the window as one meeting is,
/// and the owner's yes deletes them for good: the server is told, and nothing is held back to come
/// back into the calendar (review of #1665: the batch asked nothing, and the host undid it).
#[test]
fn a_set_whose_people_have_no_address_is_asked_about_and_the_yes_goes_through() {
    let (mut window, kept) = kept_window();
    for id in [kept.first, kept.second] {
        window.state_mut().session.mailbox.event_mut(id).expect("the meeting is there").attendees =
            vec![Attendee { address: Address::new("Room four", ""), ..Default::default() }];
    }
    window.run_steps(2);
    pending_delete_of_both(&mut window, &kept);
    let text = owner_question_text(&window);
    let listing = owner_question_listing(&window);
    let people: Vec<&str> = listing.groups.iter().flat_map(|group| group.items.iter()).filter_map(|item| item.to.as_deref()).collect();
    assert_eq!(people, ["to 1 person without an address", "to 1 person without an address"], "{text}");
    assert!(text.contains("2 people invited"), "and the count: {text}");
    answer_question(&mut window);
    let session = &window.state().session;
    assert!(session.mailbox.event(kept.first).is_none() && session.mailbox.event(kept.second).is_none(), "the yes deleted both");
    let account = session.mailbox.accounts.iter().find(|account| account.email == KEPT_ACCOUNT).map(|account| account.id).expect("the kept account");
    let upload = mailcraft_engine::cmd::remote::event_upload(session, account).expect("an upload can be built");
    let mut cancels: Vec<&str> = upload.ops.iter().filter(|op| op.cancel).map(|op| op.remote.as_str()).collect();
    cancels.sort_unstable();
    assert_eq!(cancels, ["server-1", "server-2"], "and the server is told of both: nothing is held back");
}

/// The lines of the open question's list, in order, as `Dialog::Confirm` holds them in `app`.
fn app_listing(app: &MailApp) -> crate::dialogs::Listing {
    match &app.dialog {
        Some(Dialog::Confirm { ask, .. }) => ask.listing.clone().expect("a batch question lists its items"),
        _ => panic!("no question is open"),
    }
}

/// Every item line of `listing`, in order.
fn item_lines(listing: &crate::dialogs::Listing) -> Vec<String> {
    listing.groups.iter().flat_map(|group| group.items.iter()).map(|item| item.what.clone()).collect()
}

/// A window shown by London's clock (`Session::in_zone`, as `calendar_multiselect_edge_tests`
/// makes one) on [`kept_mailbox`], and a daily meeting its server keeps by `series_zone`'s clock,
/// first at `start` by the clock here, with Bo invited.
fn zoned_app(series_zone: &str, start: DateTime) -> (MailApp, Id) {
    let (mailbox, _) = kept_mailbox();
    let zone = mailcraft_time::Zone::named("Europe/London").expect("a zone of the bundled database");
    let mut app = MailApp::new(Session::in_zone(mailbox, NOW, zone), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    // (The first command settles the mailbox into the window's zone; the series is written after
    // it, by the clock here.)
    app.execute("view.calendar", json!({})).expect("the calendar view opens");
    let account =
        app.session.mailbox.accounts.iter().find(|account| account.email == KEPT_ACCOUNT).map(|account| account.id).expect("the kept account");
    let calendar = app.session.mailbox.default_calendar().expect("the mailbox has a calendar");
    let series = app
        .session
        .mailbox
        .add_event(Event {
            title: "Stand-up".into(),
            start,
            end: start.add_minutes(30),
            start_zone: series_zone.into(),
            end_zone: series_zone.into(),
            calendar,
            recurrence: Some(Recurrence::every(Freq::Daily)),
            organizer: Some(Address::new("Zoe Example", KEPT_ACCOUNT)),
            attendees: vec![Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Default::default() }],
            origin: Origin { account, id: "server-zoned".into(), ..Default::default() },
            ..Default::default()
        })
        .expect("the calendar takes a series");
    (app, series)
}

/// When the series' own day `day` starts by the clock here, as the engine places it on the grid.
fn starts_here(app: &MailApp, series: Id, day: Date) -> DateTime {
    let event = app.session.mailbox.event(series).expect("the series is there");
    event.occurrence_times(day, &|t| app.session.offset_at(t)).0
}

/// Whether `line` names `at` as a batch question says a time: its long day and its clock.
fn names_time(app: &MailApp, line: &str, at: DateTime) -> bool {
    line.contains(crate::dates::long(at.date).as_str()) && line.contains(crate::dates::clock(at, app.session.prefs.twelve_hour).as_str())
}

/// A day of a series that keeps another zone's clock is named in the question at the hour and on
/// the date the calendar shows it here, for a Delete and for a cut alike (review of #1665: the line
/// paired the series' day with its first start's clock, which named an hour, and a weekday, at
/// which the item is not).
#[test]
fn a_batch_question_names_a_day_of_a_zoned_series_when_it_happens_here() {
    // 00:00 in London on October 1 (summer time) is 08:00 in Tokyo, which keeps no summer time;
    // after London's clocks go back on October 25, 08:00 in Tokyo is 23:00 the day before here.
    let (mut app, series) = zoned_app("Asia/Tokyo", at(d(2026, 10, 1), 0, 0));
    let (first, second) = (d(2026, 10, 27), d(2026, 10, 28));
    let shown = [at(d(2026, 10, 26), 23, 0), at(d(2026, 10, 27), 23, 0)];
    assert_eq!([starts_here(&app, series, first), starts_here(&app, series, second)], shown, "fixture: where the grid shows the two days");
    let items = json!({ "items": [{ "id": series, "day": first.iso() }, { "id": series, "day": second.iso() }] });
    app.execute("calendar.select", items.clone()).expect("two days of the series are selected");
    let said = app.run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let lines = item_lines(&app_listing(&app));
    assert_eq!(lines.len(), 2, "a line for each day: {lines:?}");
    let first_clock = crate::dates::clock(at(d(2026, 10, 1), 0, 0), app.session.prefs.twelve_hour);
    for (line, at) in lines.iter().zip(shown) {
        assert!(names_time(&app, line, at), "names {at:?} as the grid shows it: {line}");
        assert!(!line.contains(first_clock.as_str()), "and never the series' first clock: {line}");
    }
    assert!(!lines[0].contains(crate::dates::long(first).as_str()), "the first day is on the day before here: {}", lines[0]);

    // A cut of the same two days says where each is now as the grid does, and where it goes.
    app.dialog = None;
    let mut cut = items;
    cut["start"] = json!(START);
    cut["cut"] = json!(true);
    let said = app.run("calendar.paste", cut).expect("a cut that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let lines = item_lines(&app_listing(&app));
    assert_eq!(lines.len(), 2, "a line for each day: {lines:?}");
    assert!(names_time(&app, &lines[0], shown[0]) && names_time(&app, &lines[0], at(d(2026, 12, 2), 9, 0)), "from and to: {}", lines[0]);
    assert!(names_time(&app, &lines[1], shown[1]), "from: {}", lines[1]);
}

/// A day of a series whose zone changes its clocks on another date than the window's is named at
/// the hour it is here that day, not at the hour its first day was (review of #1665).
#[test]
fn a_batch_question_names_a_series_day_by_the_clock_here_across_a_change_of_the_clocks() {
    // 14:00 in London on October 1 is 09:00 in New York; London's clocks go back on October 25 and
    // New York's on November 1, so on October 27 09:00 there is 13:00 here.
    let (mut app, series) = zoned_app("America/New_York", at(d(2026, 10, 1), 14, 0));
    let (before, between) = (d(2026, 10, 2), d(2026, 10, 27));
    let shown = [at(before, 14, 0), at(between, 13, 0)];
    assert_eq!([starts_here(&app, series, before), starts_here(&app, series, between)], shown, "fixture: where the grid shows the two days");
    app.execute("calendar.select", json!({ "items": [{ "id": series, "day": before.iso() }, { "id": series, "day": between.iso() }] }))
        .expect("two days of the series are selected");
    let said = app.run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let lines = item_lines(&app_listing(&app));
    assert_eq!(lines.len(), 2, "a line for each day: {lines:?}");
    assert!(names_time(&app, &lines[0], shown[0]), "{}", lines[0]);
    assert!(names_time(&app, &lines[1], shown[1]), "at 13:00 here: {}", lines[1]);
    assert!(!names_time(&app, &lines[1], at(between, 14, 0)), "never at the first day's 14:00: {}", lines[1]);
}

/// The second account whose server writes in [`two_accounts_app`].
const OTHER_ACCOUNT: &str = "yan@example.net";

/// [`kept_app`] with a second account whose server keeps a meeting of its own, "Planning" on
/// 2026-11-10 at 11:00 with Cy invited: a set of the three asks about mail from two servers.
fn two_accounts_app() -> (MailApp, Kept, Id) {
    let (mut app, kept) = kept_app();
    let added = app
        .session
        .run("account.add", &json!({"email": OTHER_ACCOUNT, "name": "Yan", "incoming": {"host": "imap.example.net"}}))
        .expect("a second account is added");
    let account = added["id"].as_u64().expect("the account's id");
    let calendar = app.session.mailbox.default_calendar().expect("the mailbox has a calendar");
    let start = at(d(2026, 11, 10), 11, 0);
    let planning = app
        .session
        .mailbox
        .add_event(Event {
            title: "Planning".into(),
            start,
            end: start.add_minutes(30),
            calendar,
            organizer: Some(Address::new("Yan Example", OTHER_ACCOUNT)),
            attendees: vec![Attendee { address: Address::new("Cy Example", "cy@example.net"), ..Default::default() }],
            origin: Origin { account, id: "server-yan".into(), ..Default::default() },
            ..Default::default()
        })
        .expect("the calendar takes a meeting");
    app.execute("calendar.select", json!({ "items": [{ "id": kept.first }, { "id": planning }, { "id": kept.second }] }))
        .expect("the three meetings are selected");
    (app, kept, planning)
}

/// A set whose meetings two accounts' servers write about lists each item under its own account's
/// heading, and no item under both (review of #1665).
#[test]
fn a_batch_question_with_two_writing_accounts_lists_each_item_under_its_own() {
    let (mut app, _, _) = two_accounts_app();
    let said = app.run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let listing = app_listing(&app);
    assert_eq!(listing.groups.len(), 2, "one heading for each account that writes: {listing:?}");
    let titles = |group: &crate::dialogs::ListGroup| -> Vec<String> {
        group.items.iter().filter_map(|item| item.what.split('"').nth(1).map(str::to_string)).collect()
    };
    let zoe = listing.groups.iter().find(|group| group.heading.contains(KEPT_ACCOUNT)).expect("a heading for Zoe's server");
    let yan = listing.groups.iter().find(|group| group.heading.contains(OTHER_ACCOUNT)).expect("a heading for Yan's server");
    assert!(!zoe.heading.contains(OTHER_ACCOUNT) && !yan.heading.contains(KEPT_ACCOUNT), "each heading names its own account alone");
    assert_eq!(titles(zoe), ["Kickoff", "Review"], "Zoe's server writes about Kickoff and Review: {zoe:?}");
    assert_eq!(titles(yan), ["Planning"], "and Yan's about Planning: {yan:?}");
    assert!(yan.items[0].to.as_deref().is_some_and(|to| to.contains("cy@example.net") && !to.contains("bo@")), "to Cy alone: {yan:?}");
}

/// With two accounts' servers writing, the question's first lines say "servers" and "tell", once
/// for both, for a Delete and for a cut (review of #1665: "The server of A, B tells …").
#[test]
fn a_batch_summary_with_two_writing_accounts_names_both_servers_in_the_plural() {
    let (mut app, kept, planning) = two_accounts_app();
    let said = app.run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let summary = app_listing(&app).summary;
    assert!(summary.contains("The servers of the accounts named below tell the 2 people invited, so that they know it is cancelled."), "{summary}");
    assert!(!summary.contains("The server of"), "{summary}");

    app.dialog = None;
    let cut = json!({ "items": [{ "id": kept.first }, { "id": planning }, { "id": kept.second }], "start": START, "cut": true });
    let said = app.run("calendar.paste", cut).expect("a cut that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let summary = app_listing(&app).summary;
    assert!(summary.contains("The servers of the accounts named below tell the 2 people invited of the new time."), "{summary}");
}

/// A title, or an address, with format characters (a right-to-left override, a zero-width space)
/// is shown without them in a batch question, as names are shown elsewhere (`mailcraft_names`;
/// review of #1665).
#[test]
fn a_batch_question_shows_titles_and_people_without_format_characters() {
    let (mut app, kept) = kept_app();
    {
        let kickoff = app.session.mailbox.event_mut(kept.first).expect("Kickoff is there");
        kickoff.title = "Kick\u{202e}off\u{200b}".into();
        kickoff.attendees.push(Attendee { address: Address::new("Dee", "dee\u{202e}@example.org"), ..Default::default() });
    }
    app.execute("calendar.select", json!({ "items": [{ "id": kept.first }, { "id": kept.second }] })).expect("both are selected");
    let said = app.run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let listing = app_listing(&app);
    let text = listing.plain();
    assert!(!text.chars().any(mailcraft_names::is_format), "no format character is shown: {text:?}");
    assert!(text.contains("\"Kickoff\""), "the title, as it reads: {text}");
    assert!(text.contains("dee@example.org"), "and the address: {text}");
}

// ---- CodeRabbit's second pass over #1665, and review-05's follow-ups ----

const THIRD_ACCOUNT: &str = "xan@example.com";

/// The open question, as `Dialog::Confirm` holds it in `app`.
fn app_question(app: &MailApp) -> &crate::dialogs::Question {
    match &app.dialog {
        Some(Dialog::Confirm { ask, .. }) => ask,
        _ => panic!("no question is open"),
    }
}

/// The account `email` names.
fn account_of(app: &MailApp, email: &str) -> Id {
    app.session.mailbox.accounts.iter().find(|account| account.email == email).map(|account| account.id).expect("the fixture's account")
}

/// A meeting `account`'s server keeps, in the mailbox's calendar, with `invited` invited.
fn served_meeting(app: &mut MailApp, account: Id, title: &str, start: DateTime, remote: &str, invited: &str) -> Id {
    let calendar = app.session.mailbox.default_calendar().expect("the mailbox has a calendar");
    app.session
        .mailbox
        .add_event(Event {
            title: title.into(),
            start,
            end: start.add_minutes(30),
            calendar,
            attendees: vec![Attendee { address: Address::new("", invited), ..Default::default() }],
            origin: Origin { account, id: remote.into(), ..Default::default() },
            ..Default::default()
        })
        .expect("the calendar takes a meeting")
}

/// Three accounts' servers write: the summary says so without naming them in a list, whose
/// joining words aren't every language's and whose length would grow with the set (review-05
/// NB2 and NB4; CodeRabbit, dialogs.rs:2809); each account has its heading in the list.
#[test]
fn a_batch_summary_with_three_writing_accounts_names_them_in_the_headings_alone() {
    let (mut app, kept, planning) = two_accounts_app();
    let added = app
        .session
        .run("account.add", &json!({"email": THIRD_ACCOUNT, "name": "Xan", "incoming": {"host": "imap.example.com"}}))
        .expect("a third account is added");
    let account = added["id"].as_u64().expect("the account's id");
    let retro = served_meeting(&mut app, account, "Retro", at(d(2026, 11, 12), 15, 0), "server-xan", "di@example.com");
    let set = json!({ "items": [{ "id": kept.first }, { "id": planning }, { "id": kept.second }, { "id": retro }] });
    app.execute("calendar.select", set).expect("the four meetings are selected");
    let said = app.run("calendar.delete", json!({})).expect("a Delete that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let listing = app_listing(&app);
    assert!(
        listing.summary.contains("The servers of the accounts named below tell the 3 people invited, so that they know it is cancelled."),
        "{}",
        listing.summary
    );
    assert!(!listing.summary.contains('@'), "no address in the summary: {}", listing.summary);
    for email in [KEPT_ACCOUNT, OTHER_ACCOUNT, THIRD_ACCOUNT] {
        assert_eq!(listing.groups.iter().filter(|group| group.heading.contains(email)).count(), 1, "one heading names {email}: {listing:?}");
    }
}

/// The calendar a cut goes to is named without format characters, as every other name of the
/// question is (review-05 NB3).
#[test]
fn a_cut_summary_names_its_calendar_without_format_characters() {
    let (mut app, kept) = kept_app();
    let calendar = app.session.mailbox.default_calendar().expect("the mailbox has a calendar");
    if let Some(named) = app.session.mailbox.calendars.iter_mut().find(|named| named.id == calendar) {
        named.name = "Te\u{202e}am\u{200b} Calendar".into();
    }
    let cut = json!({ "items": [{ "id": kept.first }, { "id": kept.second }], "start": START, "cut": true, "calendar": calendar });
    let said = app.run("calendar.paste", cut).expect("a cut that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let summary = app_listing(&app).summary;
    assert!(!summary.chars().any(mailcraft_names::is_format), "no format character: {summary:?}");
    assert!(summary.contains("\"Team Calendar\""), "the name as it reads: {summary}");
}

/// Appointments of this computer's with people in them, cut into a calendar a server keeps, make
/// that server send invitations: the button says so (CodeRabbit, dialogs.rs:1518).
#[test]
fn a_cut_that_sends_invitations_says_so_on_its_button() {
    let (mut app, _) = kept_app();
    let account = account_of(&app, KEPT_ACCOUNT);
    let served = app.session.mailbox.alloc_id();
    app.session.mailbox.calendars.push(mailcraft_model::Calendar {
        id: served,
        name: "Zoe's team".into(),
        visible: true,
        origin: Origin { account, id: "server-team".into(), ..Default::default() },
        ..Default::default()
    });
    let calendar = app.session.mailbox.default_calendar().expect("the mailbox has a calendar");
    let mut local = |title: &str, start: DateTime| {
        let invited = vec![Attendee { address: Address::new("", "di@example.org"), ..Default::default() }];
        let event = Event { title: title.into(), start, end: start.add_minutes(30), calendar, attendees: invited, ..Default::default() };
        app.session.mailbox.add_event(event).expect("the calendar takes a meeting")
    };
    let (talk, club) = (local("Lunch talk", at(d(2026, 11, 12), 12, 0)), local("Book club", at(d(2026, 11, 13), 18, 0)));
    let cut = json!({ "items": [{ "id": talk }, { "id": club }], "start": START, "cut": true, "calendar": served });
    assert!(
        matches!(
            mailcraft_engine::cmd::calendar::mail_of(&app.session, "calendar.paste", &cut),
            Some(mailcraft_engine::cmd::calendar::Mail::Invitation(_))
        ),
        "fixture: the cut sends invitations"
    );
    let said = app.run("calendar.paste", cut).expect("a cut that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    assert_eq!(app_question(&app).action, "Move and Send Invitations");
}

/// A cut of a whole day and a meeting with a time of day: the time asked for is the meeting's,
/// and the summary says so; a cut of whole days alone names the day without a time (CodeRabbit,
/// dialogs.rs:1409).
#[test]
fn a_cut_summary_says_which_item_takes_the_time_asked_for() {
    let (mut app, kept) = kept_app();
    let account = account_of(&app, KEPT_ACCOUNT);
    let calendar = app.session.mailbox.default_calendar().expect("the mailbox has a calendar");
    let mut whole = |title: &str, on: Date, remote: &str| {
        let event = Event {
            title: title.into(),
            start: DateTime::at_midnight(on),
            end: DateTime::at_midnight(on.add_days(1)),
            all_day: true,
            calendar,
            attendees: vec![Attendee { address: Address::new("", "bo@example.org"), ..Default::default() }],
            origin: Origin { account, id: remote.into(), ..Default::default() },
            ..Default::default()
        };
        app.session.mailbox.add_event(event).expect("the calendar takes a whole-day meeting")
    };
    let (offsite, retreat) = (whole("Offsite", d(2026, 11, 8), "server-off"), whole("Retreat", d(2026, 11, 10), "server-ret"));
    let mixed = json!({ "items": [{ "id": offsite }, { "id": kept.first }], "start": START, "cut": true });
    let said = app.run("calendar.paste", mixed).expect("a cut that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let summary = app_listing(&app).summary;
    assert!(summary.contains("the earliest with a time of day to"), "{summary}");

    app.dialog = None;
    let days = json!({ "items": [{ "id": offsite }, { "id": retreat }], "start": START, "cut": true });
    let said = app.run("calendar.paste", days).expect("a cut that writes to people asks first");
    assert_eq!(said["pending"], "confirm", "{said}");
    let summary = app_listing(&app).summary;
    let clock = crate::dates::clock(at(d(2026, 12, 2), 9, 0), app.session.prefs.twelve_hour);
    assert!(summary.contains(&crate::dates::long(d(2026, 12, 2))) && !summary.contains(clock.as_str()), "the day alone: {summary}");
}

/// Somebody else's meetings in a Delete whose question can decline them: their organizers, one
/// each, are named under a heading that says when they are told, never under what writes to
/// nobody (CodeRabbit's reply on dialogs.rs, 4630368257). The heading names the question's own
/// button that tells them, so that it never stands over "Decline without Telling" as if that
/// told anybody (W2 on #1665).
#[test]
fn a_batch_delete_names_the_organizers_a_decline_would_tell() {
    let (mut app, kept) = kept_app();
    let account = account_of(&app, KEPT_ACCOUNT);
    let calendar = app.session.mailbox.default_calendar().expect("the mailbox has a calendar");
    let mut theirs = |title: &str, start: DateTime, organizer: &str, remote: &str| {
        let event = Event {
            title: title.into(),
            start,
            end: start.add_minutes(30),
            calendar,
            organizer: Some(Address::new("", organizer)),
            attendees: vec![Attendee { address: Address::new("Zoe Example", KEPT_ACCOUNT), ..Default::default() }],
            origin: Origin { account, id: remote.into(), guest: true, ..Default::default() },
            ..Default::default()
        };
        app.session.mailbox.add_event(event).expect("the calendar takes somebody else's meeting")
    };
    let call = theirs("Their call", at(d(2026, 11, 12), 11, 0), "org@example.net", "server-call");
    let demo = theirs("Vendor demo", at(d(2026, 11, 13), 14, 0), "sales@example.com", "server-demo");
    for (set, choice) in
        [(vec![kept.first, call, demo], "Decline the Others and Tell Their Organizers"), (vec![call, demo], "Decline and Tell the Organizer")]
    {
        app.dialog = None;
        let items: Vec<Value> = set.iter().map(|id| json!({ "id": id })).collect();
        app.execute("calendar.select", json!({ "items": items })).expect("the set is selected");
        let said = app.run("calendar.delete", json!({})).expect("a Delete with somebody else's meetings asks first");
        assert_eq!(said["pending"], "confirm", "{said}");
        let listing = app_listing(&app);
        let heading = format!("If you choose \"{choice}\", their organizers are told:");
        let declined = listing.groups.iter().find(|group| group.heading == heading).unwrap_or_else(|| panic!("{heading} is a heading: {listing:?}"));
        let buttons: Vec<&str> = match &app.dialog {
            Some(crate::dialogs::Dialog::Confirm { ask, .. }) => {
                std::iter::once(ask.action.as_str()).chain(ask.more.iter().map(|(label, _)| label.as_str())).collect()
            }
            _ => Vec::new(),
        };
        assert!(buttons.contains(&choice), "the heading names a button of this question: {buttons:?}");
        for (title, organizer) in [("Their call", "org@example.net"), ("Vendor demo", "sales@example.com")] {
            let line = declined.items.iter().find(|item| item.what.contains(title)).unwrap_or_else(|| panic!("{title} is under it: {listing:?}"));
            assert!(line.to.as_deref().is_some_and(|to| to.contains(organizer)), "to its own organizer: {line:?}");
        }
        assert!(
            !listing.groups.iter().any(|group| group.heading.contains("Nobody") && group.items.iter().any(|item| item.what.contains("Their call"))),
            "a meeting a decline answers is not said to write to nobody: {listing:?}"
        );
    }
}

/// An address list uses its translated punctuation, not the English comma (W4). The pattern
/// remains one catalogue entry: punctuation alone is deliberately not extracted by xtask.
#[test]
fn a_batch_questions_addresses_use_the_languages_list_punctuation() {
    let _restore = crate::i18n::RestoreLanguage::hold();
    let (mut app, kept) = kept_app();
    let event = app.session.mailbox.events.get_mut(&kept.first).expect("the fixture's meeting");
    std::sync::Arc::make_mut(event).attendees = ["a@example.org", "b@example.org", "c@example.org"]
        .into_iter()
        .map(|email| Attendee { address: Address::new("", email), ..Default::default() })
        .collect();
    for (language, expected) in [
        (crate::i18n::Lang::English, "a@example.org, b@example.org, c@example.org"),
        (crate::i18n::Lang::ChineseSimplified, "a@example.org\u{3001}b@example.org\u{3001}c@example.org"),
    ] {
        crate::i18n::set_current(language);
        app.dialog = None;
        let said = app.run("calendar.delete", json!({ "items": [{ "id": kept.first }, { "id": kept.second }] })).expect("the owner's Delete asks");
        assert_eq!(said["pending"], "confirm");
        let listing = app_listing(&app);
        let item =
            listing.groups.iter().flat_map(|group| &group.items).find(|item| item.what.contains("Kickoff")).expect("the question names the meeting");
        assert!(item.to.as_deref().is_some_and(|to| to.contains(expected)), "{language:?}: {item:?}");
    }
}
