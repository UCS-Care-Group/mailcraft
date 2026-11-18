// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's side of the **documented whole-series delete named by typed `items`**
//! (`calendar.delete {"items": [{"id": <series>}], "scope": "series"}`): the owner is asked about
//! the entire series before anything goes, the question keeps the set it showed, and Cancel
//! leaves the mailbox exactly as it was — including an appointment window with changes that
//! aren't saved, which such a delete must not lose unasked.
//!
//! One module of `agent_gate::calendar_batch_guard_tests`, so `use super::*` is that file's
//! fixtures (`app()`, `Ids`, `calendar()`, `d`, `at`, `NOW`,
//! `START`, `dirty_window`, `focus_elsewhere`, `agent_allowed`, `asked`, `answer`) and its
//! imports. Nothing here is production code and nothing here is a test hook: the window is the
//! real `MailApp`, and the question is answered with the real buttons of `Dialog::Confirm`
//! (dialogs.rs 2154-2231).
//!
//! What the source did when these were written (2026-11-15), and why each test holds it:
//! - `dialogs::set_items` (dialogs.rs:1103-1108) reads `calendar.delete` with
//!   `calendar::selected_items`, whose `Allow::Occurrences` refuses a day-less series
//!   (selection.rs:113, selection.rs:222-225). `calendar_question` therefore returns no question
//!   at all for the documented whole-series form (dialogs.rs:1303-1304), so the owner is never
//!   asked and the command is refused instead (test 1 fails at its first assertion).
//! - `dialogs::batch_question` (dialogs.rs:1155-1163) names an item by its own day or, for a
//!   whole series, by the master's start date: it never says the entire series goes, so the
//!   owner would approve a delete larger than what they were shown.
//! - `windows::takes_away` (windows.rs:558-560) reads the same strict `selected_items`, so an
//!   agent's whole-series delete finds no unsaved window: `as_the_window_has_it`
//!   (agent_gate.rs:158-185) puts no `unsaved` in the params, the batch is classed an ordinary
//!   owner delete, and with `--allow delete` it runs while the owner's changes are unsaved
//!   (test 2 fails: it is run, not asked, or in the current source refused before any question).
//!
//! Both tests use only the window's public API: `MailApp::run`,
//! `Agent`/`ControlRequest` through `agent_allowed`, `windows::of_event`, `windows::unsaved`,
//! `dialogs::Dialog::Confirm`, and `calendar::mail_of` (the fixture guard). Neither calls
//! `calendar::delete_items` directly.
//!
//! Fixtures only: synthetic people on `example.org`, events through the public
//! `Mailbox::add_event`, an account added through `account.add` whose "server" is a name nothing
//! contacts, and the window's fixed clock. No files, no server, no real account, no wall clock.

use egui::accesskit::Role;
use egui::{PointerButton, Rect};
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_model::{Address, Attendee, Origin};

use crate::dialogs::Dialog;
use crate::people_view::testing::{Window, click, dialog, window_on};

use super::*;

/// The account whose "server" keeps the synthetic series: an `example.org` name that is never
/// contacted (the same account `calendar_batch_dialog_tests::kept_mailbox` adds).
const KEPT_ACCOUNT: &str = "zoe@example.org";

/// A weekly series whose server keeps it, with one person invited: deleting the whole series
/// makes that server send them a cancellation (`Mail::Cancellation`), as the meetings of
/// `kept_mailbox` do in `calendar_batch_dialog_tests`.
struct KeptSeries {
    series: Id,
}

fn kept_series_mailbox() -> (Mailbox, KeptSeries) {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    session.mailbox.events.clear();
    let added = session
        .run("account.add", &json!({"email": KEPT_ACCOUNT, "name": "Zoe", "incoming": {"host": "imap.example.org"}}))
        .expect("an account is added");
    let account = added["id"].as_u64().expect("the account's id");
    session.mailbox.calendars[0].name = "Team Calendar".into();
    let calendar = session.mailbox.default_calendar().expect("the mailbox has a calendar");
    let start = at(d(2026, 11, 4), 9, 0);
    let series = session
        .mailbox
        .add_event(Event {
            title: "Weekly sync".into(),
            start,
            end: start.add_minutes(30),
            calendar,
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            organizer: Some(Address::new("Zoe Example", KEPT_ACCOUNT)),
            attendees: vec![Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Default::default() }],
            origin: Origin { account, id: "server-series".into(), ..Default::default() },
            ..Default::default()
        })
        .expect("the calendar takes a meeting");
    // A plain appointment around it: nothing the whole-series delete names may go with it.
    let den = at(d(2026, 11, 10), 14, 0);
    session
        .mailbox
        .add_event(Event { title: "Dentist".into(), start: den, end: den.add_minutes(30), calendar, ..Default::default() })
        .expect("the calendar takes an appointment");
    let params = json!({"items": [{"id": series}], "scope": "series"});
    assert!(
        matches!(
            mailcraft_engine::cmd::calendar::mail_of(&session, "calendar.delete", &params),
            Some(mailcraft_engine::cmd::calendar::Mail::Cancellation(n)) if n > 0
        ),
        "fixture: deleting the whole series makes the account's server tell the person invited"
    );
    (session.mailbox, KeptSeries { series })
}

fn kept_series_window() -> (Window, KeptSeries) {
    let (mailbox, kept) = kept_series_mailbox();
    let mut window = window_on(mailbox, "view.calendar");
    window.state_mut().session.driver.set_auto_sync(false);
    let _ = window.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    window.run_steps(3);
    (window, kept)
}

/// The open `Dialog::Confirm`: the command it will run, the targets it pinned, and the question.
fn confirm(window: &Window) -> (&str, &Value, &crate::dialogs::Question) {
    match &window.state().dialog {
        Some(Dialog::Confirm { command, params, ask }) => (command.as_str(), params, ask.as_ref()),
        _ => panic!("no question is open"),
    }
}

/// Whether `text` says the whole series goes, rather than one of its days.
fn names_whole_series(text: &str) -> bool {
    let text = text.to_lowercase();
    ["all of its occurrences", "all its occurrences", "entire series", "whole series"].iter().any(|said| text.contains(said))
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
    click(window, pos, PointerButton::Primary, 1);
    window.run_steps(3);
}

/// The typed whole-series delete asks about the series it would cancel — the entire series, not
/// one of its days — keeps the set it showed with the question, and Cancel changes nothing.
#[test]
fn a_typed_whole_series_delete_asks_about_the_series_before_it_goes() {
    let (mut window, kept) = kept_series_window();
    let before = window.state().session.mailbox.clone();
    let selection = window.state().session.sel.clone();
    let series_before = window.state().session.mailbox.event(kept.series).cloned().expect("fixture: the series is there");

    let said = window
        .state_mut()
        .run("calendar.delete", json!({"items": [{"id": kept.series}], "scope": "series"}))
        .expect("the owner's whole-series Delete waits for one question rather than failing to read the set");
    window.run_steps(3);
    assert_eq!(said["pending"], "confirm", "the whole series is asked about before anything goes: {said}");
    assert_eq!(dialog(&window), Some("confirm"), "one question is on screen");
    assert!(window.state().session.mailbox == before, "asking changes nothing of the mailbox");

    let (command, pinned, ask) = confirm(&window);
    assert_eq!(command, "calendar.delete", "the question is about the delete the owner asked for");
    // The question keeps the set it showed: the series by `items`, with the scope the delete
    // means — never a legacy `id`, and never a day of the series the owner did not name.
    assert_eq!(pinned["items"], json!([{"id": kept.series}]), "the question pins the series it showed: {pinned}");
    assert_eq!(pinned["scope"], "series", "and the scope the whole-series delete means: {pinned}");
    assert!(pinned.get("id").is_none() && pinned.get("day").is_none(), "never a legacy id or a day of its own: {pinned}");
    assert!(ask.calendar.is_some(), "the owner's yes is bound to the set the question showed");

    let text = ask.text.clone();
    assert!(text.contains("Weekly sync"), "the question names the series: {text}");
    assert!(names_whole_series(&text), "and says the entire series goes, not one of its days: {text}");
    assert!(!text.contains("is one of a series"), "a typed `scope: \"series\"` is not the which-of-the-series chooser: {text}");
    assert!(text.contains(KEPT_ACCOUNT), "and the account whose server tells the person invited: {text}");

    press(&mut window, "Cancel");
    window.run_steps(3);
    assert!(window.state().dialog.is_none(), "Cancel closes the question");
    assert!(window.state().session.mailbox == before, "Cancel leaves the mailbox byte for byte as it was");
    assert!(window.state().session.sel == selection, "and the selection too");
    assert_eq!(
        window.state().session.mailbox.event(kept.series),
        Some(&series_before),
        "the series, its identity and its exceptions included, is untouched"
    );
}

/// A whole-series delete that closes the window of a series with changes the owner hasn't saved
/// is asked about first, even when the agent was allowed everything: nothing of the series goes
/// while the question waits, and Cancel leaves the series and the unsaved changes exactly as
/// they were (U-1, #494) — the same guard the concrete-occurrence fixture already proves.
#[test]
fn a_typed_whole_series_delete_asks_before_it_loses_an_unsaved_series_window() {
    let (mut a, ids) = app();
    dirty_window(&mut a, ids.series);
    let window = a.windows.of_event(ids.series).expect("fixture: the series has a window of its own");
    focus_elsewhere(&mut a, &ids);
    let before = a.session.mailbox.clone();
    let series_before = a.session.mailbox.event(ids.series).cloned().expect("fixture: the series is there");

    let (now, rx) = agent_allowed(&mut a, "calendar.delete", json!({"items": [{"id": ids.series}], "scope": "series"}), Class::ALL.to_vec());
    assert!(
        now.is_none(),
        "an agent allowed everything must still be asked before a whole-series Delete closes the window with the owner's unsaved changes; it must not run, and must not be left to the engine to refuse: {now:?}"
    );
    let (class, what) = asked(&a).expect("the owner is asked about the whole-series Delete");
    assert_eq!(class, Class::Delete, "the question is the owner's-unsaved one, classed as a Delete: {what}");
    assert!(what.contains("Weekly sync"), "the question names the series whose changes would be lost: {what}");
    assert!(!what.contains("Kickoff"), "and no appointment the delete never names: {what}");
    assert!(what.to_lowercase().contains("without saving"), "and says what it would lose: {what}");
    assert!(a.session.mailbox == before, "nothing of the series was taken before the owner answered");

    // The window and what is unsaved in it stay as they were while the question waits.
    assert_eq!(a.windows.unsaved(), 1, "the series' window still has its unsaved changes while the question waits");
    assert_eq!(a.windows.of_event(ids.series), Some(window), "and it is still the series' own window");
    assert_eq!(a.session.mailbox.event(ids.series), Some(&series_before), "the series is untouched, identity and all");

    answer(&mut a, false);
    assert_eq!(rx.try_recv().expect("an answer")["ok"], false, "no leaves the series as it was");
    assert!(a.session.mailbox == before, "Cancel changes nothing");
    assert_eq!(a.windows.unsaved(), 1, "and the owner's unsaved changes are still there to save");
    assert_eq!(a.windows.of_event(ids.series), Some(window), "in the window of the series the question named");
    assert!(asked(&a).is_none(), "the question is answered and gone");
}
