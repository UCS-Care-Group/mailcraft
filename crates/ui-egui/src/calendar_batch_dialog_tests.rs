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
