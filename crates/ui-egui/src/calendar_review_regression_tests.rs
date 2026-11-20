// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The two regressions a source review of the integration found, held against the window itself:
//! a set copied or cut off an overlaid calendar in the same block must keep its own calendar
//! (never the block's front one), and a set of somebody else's meetings must be asked about with
//! the same three choices one meeting gets before any of it is taken out. One more holds the
//! smallest such set — a single kept meeting selected as a typed item — to the shared reader the
//! question is built from.
//!
//! One module of `calendar_selection_tests`, which is itself a child of `calendar_view`, so
//! `super::*` is that file's fixtures (`click`, `chord`, `selected`, `drawn`, `event`,
//! `titled`, `after_noon`, `date`, `at`, `COMMAND` and the `egui` types they import), and
//! `super::super` is `calendar_view`'s own `Clip`, `CLIP_ID`, `Slot`, `load_slot`, `other_block`
//! and `paste_calendar`. Nothing here is production code and nothing here is a test hook: the
//! window is the real one, its events are synthetic (invented people on `example.org`), the clock
//! is fixed, and every act is a pointer event, a key, a command or a button the owner would
//! press. No file, server, real account, native window or wall clock is used.
//!
//! WHAT IS HELD (source read 2026-11-15), and why each part is a source reading:
//! - L-1, `calendar_view::paste_calendar`: it returns `Some(slot.calendar)` whenever the pick names
//!   a calendar and every member resolves. `slot.calendar` is the block's *front* calendar, so in
//!   a block that holds several calendars over one another a copy made from a time picked there is
//!   written into the front calendar even though every member already lives in the block. A cut of
//!   one occurrence in the same block is then refused whole as a move to another calendar. The
//!   contract retargets only a pick in a *separate* calendar block ("an explicitly chosen
//!   separate calendar block"); `paste_single`, still used for one whole event, already
//!   keeps the event in its own calendar on a same-block pick through `other_block`.
//! - L-2, `dialogs::batch_question`: a typed Delete takes the batch branch before the legacy guest
//!   branches, and the batch question starts from `mail_of(...)?`, which returns `None` for a set
//!   of guest meetings (`calendar::invited` is 0 for `origin.guest`). The owner is asked nothing
//!   and `delete_selected` removes each copy without a word to its organizer.
//! - S-9, `calendar::selection::items_mail_of`: with no `items` in the payload it returns `None`
//!   when `sel.events.len() <= 1`, so a single kept meeting selected as a typed item is never
//!   asked about and the engine refuses the Delete as unsent; `calendar.delete` then dispatched
//!   any non-empty typed selection as a set, one item included. (Since then a selection of one
//!   item that has the focus is the older one-event Delete, which asks about that meeting as it
//!   always did; the test still pins that it is asked about.)
//!
//! Every assertion is a behaviour the contract requires, or a fixture fact checked before the
//! behaviour (the pick names the block's front calendar while the item's calendar is in the same
//! block; the shared reader counts one kept meeting). No colour, pixel or sentence beyond the
//! names, dates and organizer wording the contract requires is asserted.

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;
use mailcraft_model::remote::EventChangeKind;
use mailcraft_model::{Address, Attendee, Calendar, Event, Freq, Id, Origin, Recurrence, Response, Rights};
use serde_json::{Value, json};

use super::super::{CLIP_ID, Clip, Slot, load_slot, other_block, paste_calendar};
use super::*;
use crate::dialogs::Dialog;
use crate::people_view::testing::{now, window_on};

/// The account whose server keeps the meetings of these fixtures: an invented mailbox of this
/// computer's own, on `example.org`, with no server ever reached (`account.add` only records it).
const ACCOUNT: &str = "zoe@example.org";

/// The window's reminder windows out of the way, the Week view in front, and nothing reaching a
/// server by itself.
fn ready(w: &mut Window) {
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
}

/// The blocks of the calendar as the window arranges them (`calendar_settings::panes`): one block
/// when calendars lie over one another.
fn blocks(w: &Window) -> Vec<mailcraft_engine::cmd::calendar_settings::Pane> {
    mailcraft_engine::cmd::calendar_settings::panes(&w.state().session)
}

/// What Copy or Cut put on the calendar's clipboard, as `calendar_view::clip` put it there.
fn clipboard(w: &Window) -> Option<Clip> {
    let id = egui::Id::unique(CLIP_ID);
    w.ctx.data(|data| data.get_temp::<Clip>(id))
}

/// The time the pointer picked, as the grid stored it: its own calendar is the block's front one.
fn picked(w: &Window) -> Slot {
    load_slot(&w.ctx).expect("the click picked a time in the calendar")
}

/// Select `ids` as a typed set through the engine's own command, the last one focused, as a
/// Command click of each leaves the window. Every event of these fixtures is whole (no series).
fn select_set(w: &mut Window, ids: &[Id]) {
    let items: Vec<Value> = ids.iter().map(|id| json!({ "id": *id })).collect();
    w.state_mut().execute("calendar.select", json!({ "items": items })).expect("the window takes the typed set");
    w.run_steps(2);
}

/// A block that holds two calendars over one another, with `behind` drawn under the block's front
/// one: the sample's calendar, plus `Overlay` on `example.org` with two whole appointments of its
/// own on the sample's Wednesday. The blocks are read back from the window's own arrangement, so
/// the front calendar is whatever the window actually works in.
fn overlay_window() -> (Window, Id, Id, Event, Event) {
    let mut mailbox = mailcraft_engine::sample::mailbox(now(), 0);
    let behind = 901;
    mailbox.calendars.push(Calendar { id: behind, name: "Overlay".into(), visible: true, rights: Rights::Edit, ..Calendar::default() });
    let day = date(10, 14);
    let (nine, ten) = (at(day, 9, 0), at(day, 10, 0));
    let one = mailbox
        .add_event(Event { title: "Overlay one".into(), start: nine, end: nine.add_minutes(30), calendar: behind, ..Event::default() })
        .expect("the overlaid calendar takes an appointment");
    let two = mailbox
        .add_event(Event { title: "Overlay two".into(), start: ten, end: ten.add_minutes(30), calendar: behind, ..Event::default() })
        .expect("the overlaid calendar takes a second appointment");
    let (one, two) =
        (mailbox.event(one).cloned().expect("the first appointment is there"), mailbox.event(two).cloned().expect("the second is there"));
    let mut w = window_on(mailbox, "view.calendar");
    ready(&mut w);
    // Every shown calendar lies over one another, the one worked in in front (`view.overlay`).
    w.state_mut().execute("view.overlay", json!({})).expect("the shown calendars lie over one another");
    w.run_steps(3);
    let panes = blocks(&w);
    assert_eq!(panes.len(), 1, "fixture: the two calendars are drawn as one block");
    let pane = panes.first().expect("one block");
    assert!(pane.calendars.contains(&behind), "fixture: the overlaid calendar is drawn in that block");
    assert_ne!(pane.front, behind, "fixture: another calendar is in front of it");
    (w, pane.front, behind, one, two)
}

/// The same block, with one occurrence of a weekly series in the overlaid calendar: Wednesday
/// 09:00–09:30, the day the sample's week is on.
fn overlay_series() -> (Window, Id, Id, Event, mailcraft_time::Date) {
    let mut mailbox = mailcraft_engine::sample::mailbox(now(), 0);
    let behind = 902;
    mailbox.calendars.push(Calendar { id: behind, name: "Overlay series".into(), visible: true, rights: Rights::Edit, ..Calendar::default() });
    let day = date(10, 14);
    let nine = at(day, 9, 0);
    let series = mailbox
        .add_event(Event {
            title: "Overlay stand-up".into(),
            start: nine,
            end: nine.add_minutes(30),
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            calendar: behind,
            ..Event::default()
        })
        .expect("the overlaid calendar takes a series");
    let before = mailbox.event(series).cloned().expect("the series is there");
    let mut w = window_on(mailbox, "view.calendar");
    ready(&mut w);
    w.state_mut().execute("view.overlay", json!({})).expect("the shown calendars lie over one another");
    w.run_steps(3);
    let panes = blocks(&w);
    assert_eq!(panes.len(), 1, "fixture: the two calendars are drawn as one block");
    let pane = panes.first().expect("one block");
    assert!(pane.calendars.contains(&behind), "fixture: the series' calendar is drawn in that block");
    assert_ne!(pane.front, behind, "fixture: another calendar is in front of it");
    (w, pane.front, behind, before, day)
}

/// The fixture facts a same-block pick rests on, checked before what that pick does: the item is
/// drawn in the block the time was picked in, the pick names that block's front calendar (not the
/// item's), and `other_block` — the rule a one-event paste already uses — names no destination.
fn assert_picked_in_the_blocks_front(w: &Window, front: Id, member: &Event, slot: &Slot) {
    let panes = blocks(w);
    assert_eq!(panes.len(), 1, "the calendars lie over one another in one block");
    let pane = panes.first().expect("one block");
    assert!(pane.calendars.contains(&member.calendar), "the item is drawn in the block the time was picked in");
    assert_ne!(pane.front, member.calendar, "the item's calendar is behind the block's front one");
    assert_eq!(pane.front, front, "the block's front calendar is the one it was when the calendars were laid over one another");
    assert_eq!(slot.calendar, front, "the pick names the block's front calendar");
    assert_eq!(
        other_block(w.state(), member, slot),
        None,
        "the item's calendar is not another block, so picking a time there names no other calendar"
    );
}

/// Two appointments of an overlaid calendar, copied from a time picked in the same block, stay in
/// the calendar they were copied from: both copies, with their own times of day, and neither in
/// the block's front calendar; the sources do not move. The block's front calendar is a separate
/// destination only when the pick is in a separate block (the two-block edge test keeps that).
///
/// The bounded guards first: the pick names the front calendar while the items' calendar is in
/// that same block, and `other_block` (what `paste_single` uses for one whole event) names none.
/// Then the paste, as the owner makes it.
#[test]
fn a_copy_of_an_overlaid_calendars_items_stays_in_their_own_calendar() {
    let (mut w, front, behind, one, two) = overlay_window();
    // (A click first: the calendar is what the keys are for.)
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    select_set(&mut w, &[one.id, two.id]);
    assert_eq!(selected(&w), [(one.id, None), (two.id, None)], "fixture: the two appointments are a typed set");

    // Copy: both members are on the clipboard.
    chord(&mut w, COMMAND, egui::Key::C);
    let clip = clipboard(&w).expect("Copy puts the set on the calendar's clipboard");
    assert!(!clip.cut, "fixture: a copy, not a cut");
    assert_eq!(clip.items.len(), 2, "fixture: both members of the set are on it");

    // A time picked in the same block, and Paste.
    let somewhere = after_noon(&w, 3.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    let slot = picked(&w);
    assert_picked_in_the_blocks_front(&w, front, &one, &slot);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);

    // Both copies are in the calendar they were copied from, and none came into the front one.
    let firsts = titled(&w, "Overlay one");
    assert_eq!(
        firsts.iter().filter(|event| event.calendar == behind).count(),
        2,
        "the appointment and its copy stay in the calendar the time was picked beside, not the block's front calendar: {firsts:?}"
    );
    assert_eq!(
        firsts.iter().filter(|event| event.calendar == front).count(),
        0,
        "a same-block pick names no destination, so nothing may be re-homed into the front calendar: {firsts:?}"
    );
    let mut came: Vec<Event> = firsts.into_iter().filter(|event| event.calendar == behind).collect();
    came.sort_by_key(|event| event.start);
    assert_eq!(
        came.iter().map(|event| (event.start, event.end)).collect::<Vec<_>>(),
        vec![(at(date(10, 14), 9, 0), at(date(10, 14), 9, 30)), (at(date(10, 14), 15, 0), at(date(10, 14), 15, 30))],
        "the copy lands where the time was picked, in the calendar it was copied from"
    );
    let seconds = titled(&w, "Overlay two");
    assert_eq!(
        seconds.iter().filter(|event| event.calendar == behind).count(),
        2,
        "the second appointment of the set came too, into its own calendar: {seconds:?}"
    );
    assert_eq!(seconds.iter().filter(|event| event.calendar == front).count(), 0, "and not into the front calendar: {seconds:?}");

    // The sources are where they were.
    let source = w.state().session.mailbox.event(one.id).cloned().expect("the appointment is still there");
    assert_eq!((source.start, source.end, source.calendar), (one.start, one.end, behind), "a copy leaves its source where it was");
    assert_eq!(w.state().session.mailbox.event(two.id).map(|event| event.start), Some(two.start), "and the second source too");

    // The mechanism the behaviour rests on: a pick in the block the items are already in names no
    // calendar of its own (`paste_calendar`), as `paste_single` has it through `other_block`.
    assert_eq!(
        paste_calendar(w.state(), &clip, &slot),
        None,
        "a pick inside the block that already holds the copied items is not a separate calendar block, so it names no destination"
    );
}

/// One occurrence of a series, cut from a time picked in the same block, moves only that day: the
/// series, its next week and the week before are unchanged, the moved day is an event of its own
/// at the time picked, in the series' own calendar. It is not refused as a move to another
/// calendar, because the pick named the block's front calendar and not a different block.
#[test]
fn a_cut_occurrence_of_an_overlaid_calendar_moves_only_its_own_day() {
    let (mut w, front, behind, series, day) = overlay_series();
    let (series_id, start) = (series.id, series.start);
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    w.state_mut()
        .execute("calendar.select", json!({ "items": [{ "id": series_id, "day": day.iso() }] }))
        .expect("the one occurrence is selected as a typed item");
    w.run_steps(2);
    assert_eq!(selected(&w), [(series_id, Some(day))], "fixture: the occurrence is named by its own day");
    chord(&mut w, COMMAND, egui::Key::X);
    assert_eq!(w.state().session.mailbox.event(series_id).map(|event| event.start), Some(start), "a cut moves nothing until it is pasted");
    let (was, neighbour) = {
        let here = |instant: mailcraft_time::Timestamp| w.state().session.offset_at(instant);
        (series.occurrence_times(day, &here).0, series.occurrence_times(day.add_days(7), &here).0)
    };
    assert_eq!(was, at(day, 9, 0), "fixture: the occurrence starts at 09:00");
    assert_eq!(neighbour, at(day.add_days(7), 9, 0), "fixture: the next week's is a week later, at 09:00");

    // A time picked in the same block, and Paste.
    let somewhere = after_noon(&w, 3.0);
    click(&mut w, somewhere, Modifiers::NONE, PointerButton::Primary);
    let slot = picked(&w);
    assert_picked_in_the_blocks_front(&w, front, &series, &slot);
    chord(&mut w, COMMAND, egui::Key::V);
    w.run_steps(3);
    // A day of a series cut alone and pasted on that day is asked about as Cut and Paste always
    // asked (benandez on #1665): just this one is the day that was cut.
    let asked = open_question(&w, "a same-day paste of one cut day of a series asks which of the series is meant");
    assert_eq!(asked.command, "calendar.move", "the question is the move's");
    press(&mut w, "Just This One");

    // The cut day is an event of its own at the time picked, in the series' calendar.
    let moved: Vec<Event> = titled(&w, "Overlay stand-up").into_iter().filter(|event| event.recurrence.is_none()).collect();
    assert_eq!(
        moved.len(),
        1,
        "a cut of one occurrence in the block it is already in moves that day; it is not refused as a move to another calendar ({moved:?})"
    );
    let moved = moved.first().expect("the occurrence on its own");
    assert_ne!(moved.id, series_id, "with an identity of its own");
    assert_eq!((moved.start, moved.end), (at(day, 15, 0), at(day, 15, 30)), "it lands at the time picked and keeps its length");
    assert_eq!(moved.calendar, behind, "and stays in the calendar of its series");

    // The series itself, the day that left it, and the week after it are unchanged.
    let after = w.state().session.mailbox.event(series_id).cloned().expect("the series is still there");
    assert!(after.recurrence.is_some(), "the series still repeats");
    assert_eq!(after.start, series.start, "and starts where it did");
    assert_eq!(after.exceptions, vec![day], "only the day that was cut left it, named by its own day");
    let after_neighbour = {
        let here = |instant: mailcraft_time::Timestamp| w.state().session.offset_at(instant);
        after.occurrence_times(day.add_days(7), &here).0
    };
    assert_eq!(after_neighbour, neighbour, "the next week's occurrence is where it was");
}

/// A window on the sample whose `Lunch with Dev` and `Dentist` are somebody else's meetings the
/// account's server keeps: `origin.guest`, a remote id, an organizer and one person invited. A
/// Delete of them is the owner's copy going, or an answer to the organizer — the owner's choice.
fn guest_window() -> (Window, Event, Event) {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(now(), 0), now(), 0);
    let added = session
        .run("account.add", &json!({ "email": ACCOUNT, "name": "Zoe", "incoming": { "host": "imap.example.org" } }))
        .expect("an account is added");
    let account = added["id"].as_u64().expect("the account's id");
    let mut marked: Vec<Event> = Vec::new();
    for title in ["Lunch with Dev", "Dentist"] {
        let id = session.mailbox.events.values().find(|event| event.title == title).map(|event| event.id).expect("the sample has it");
        let event = session.mailbox.event_mut(id).expect("the event is there");
        event.origin = Origin { account, id: format!("guest-{id}"), guest: true, ..Origin::default() };
        event.organizer = Some(Address::new("Dev Example", "dev@example.org"));
        event.attendees = vec![Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Attendee::default() }];
        marked.push(event.clone());
    }
    assert_eq!(marked.len(), 2, "fixture: the two sample meetings");
    for event in &marked {
        assert_eq!(event.origin.account, account, "fixture: kept by the account's server");
        assert!(event.origin.guest, "fixture: somebody else's meeting, which the owner is invited to");
        assert!(event.organizer.is_some() && !event.attendees.is_empty(), "fixture: an organizer and people invited");
    }
    let mut w = window_on(session.mailbox, "view.calendar");
    ready(&mut w);
    (w, marked.remove(0), marked.remove(0))
}

/// The question that is open, read as the window holds it: the command, the three answers it
/// offers (the label and the parameters each adds), the button that goes nowhere, and the body.
/// `why` says what was expected, so that a question that is missing fails with the expectation.
struct Asked {
    command: String,
    action: String,
    with: Value,
    more: Vec<(String, Value)>,
    cancel: String,
    text: String,
}

impl Asked {
    /// Every way of going ahead, the action first, as the dialog draws them.
    fn choices(&self) -> Vec<(String, Value)> {
        std::iter::once((self.action.clone(), self.with.clone())).chain(self.more.iter().cloned()).collect()
    }
}

fn open_question(w: &Window, why: &str) -> Asked {
    match &w.state().dialog {
        Some(Dialog::Confirm { command, ask, .. }) => Asked {
            command: command.clone(),
            action: ask.action.clone(),
            with: ask.with.clone(),
            more: ask.more.clone(),
            cancel: ask.cancel.clone().unwrap_or_else(|| "Cancel".to_string()),
            text: ask.text.clone(),
        },
        Some(other) => panic!("{why}: the window is asking something else ({})", other.name()),
        None => panic!("{why}: no question is open"),
    }
}

/// The lowest button of the window that says exactly `label` (the dialog is drawn over the
/// window, so where the ribbon has a button of the same name the question's is the lower one).
fn press(w: &mut Window, label: &str) {
    let mut found: Vec<Rect> = w
        .query_all_by_label_contains(label)
        .filter(|node| node.accesskit_node().role() == Role::Button)
        .filter(|node| node.accesskit_node().label().is_some_and(|said| said.trim() == label))
        .map(|node| node.rect())
        .collect();
    found.sort_by(|a, b| b.min.y.total_cmp(&a.min.y));
    let at = found.first().copied().unwrap_or_else(|| panic!("no button of the question says {label:?}")).center();
    click(w, at, Modifiers::NONE, PointerButton::Primary);
    w.run_steps(3);
}

/// The answer among `choices` that declines (`decline`) and, when it says, sends (`send`):
/// `None` for the send flag means the parameters say nothing of sending at all.
fn answer(choices: &[(String, Value)], decline: bool, send: Option<bool>) -> Option<String> {
    choices
        .iter()
        .find(|(_, with)| {
            with.get("decline").and_then(Value::as_bool) == Some(decline)
                && match send {
                    Some(send) => with.get("send").and_then(Value::as_bool) == Some(send),
                    None => with.get("send").and_then(Value::as_bool) != Some(true),
                }
        })
        .map(|(label, _)| label.clone())
}

/// The answers queued for the account's server about `ids`, as the mailbox holds them.
fn answers_queued<'a>(w: &'a Window, ids: &[Id]) -> Vec<&'a mailcraft_model::remote::EventChange> {
    w.state().session.mailbox.event_changes.iter().filter(|change| change.kind == EventChangeKind::Respond && ids.contains(&change.event)).collect()
}

/// A Delete of a typed set of guest meetings asks first, with the three choices one meeting gets:
/// delete the owner's copy and tell nobody; decline and tell the organizer; decline without
/// telling. Nothing is removed while it waits, Cancel changes and queues nothing, and each answer
/// does what it says — the owner's copy gone, an answer queued with `send` exactly when the
/// organizer is told, and no cancellation mail at all.
#[test]
fn a_guest_set_is_asked_about_with_three_choices_before_anything_is_removed() {
    let (mut w, lunch, dentist) = guest_window();
    let ids = [lunch.id, dentist.id];
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    select_set(&mut w, &ids);
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "fixture: the guest meetings are a typed set");

    let before = w.state().session.mailbox.clone();
    chord(&mut w, Modifiers::NONE, egui::Key::Delete);
    w.run_steps(3);
    let asked = open_question(&w, "a Delete of a typed set of guest meetings must ask before taking any of them out");
    assert_eq!(asked.command, "calendar.delete", "the question is about the Delete");
    assert!(w.state().session.mailbox == before, "nothing is removed while the question waits");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len(), "all the events are still there");
    for title in ["Lunch with Dev", "Dentist"] {
        assert!(asked.text.contains(title), "the one question names {title}: {}", asked.text);
    }
    assert!(
        asked.text.to_lowercase().contains("organizer"),
        "the question says what happens to the organizer (the legacy one says they are not told): {}",
        asked.text
    );

    // The three ways of going ahead, by what each does rather than by its wording.
    let choices = asked.choices();
    let keep_quiet = answer(&choices, false, None).expect("one answer deletes the owner's copies and tells nobody");
    let tell = answer(&choices, true, Some(true)).expect("one answer declines and tells the organizer");
    let decline_quiet = answer(&choices, true, Some(false)).expect("one answer declines without telling the organizer");
    assert_eq!(
        choices.len(),
        3,
        "exactly the three choices one guest meeting gets, no more: {:?}",
        choices.iter().map(|(label, _)| label).collect::<Vec<_>>()
    );

    // Cancel: nothing goes, nothing is queued.
    press(&mut w, &asked.cancel);
    assert!(w.state().dialog.is_none(), "Cancel closes the question");
    assert!(w.state().session.mailbox == before, "Cancel removes nothing and changes nothing");
    assert!(w.state().session.mailbox.event_changes.is_empty(), "and queues no answer: nobody is written to");

    // Decline and tell the organizer: both copies go, and both answers are queued to be sent.
    chord(&mut w, Modifiers::NONE, egui::Key::Delete);
    w.run_steps(3);
    let _asked = open_question(&w, "the set is still there, so a second Delete asks again");
    press(&mut w, &tell);
    assert!(w.state().session.mailbox.event(lunch.id).is_none(), "the first guest meeting's copy went");
    assert!(w.state().session.mailbox.event(dentist.id).is_none(), "and the second's");
    let queued = answers_queued(&w, &ids);
    assert_eq!(queued.len(), 2, "both meetings have an answer queued for their organizer");
    assert!(
        queued.iter().all(|change| change.send && change.response == Response::Declined),
        "each answer is a decline the organizer is told: {:?}",
        queued.iter().map(|change| (change.send, change.response)).collect::<Vec<_>>()
    );
    assert!(
        w.state().session.mailbox.event_changes.iter().all(|change| !change.send || change.kind == EventChangeKind::Respond),
        "no cancellation mail goes besides the answers the owner chose"
    );

    // One step back, and Delete my copy: nothing is answered, and nothing is sent.
    assert!(w.state_mut().session.undo(), "a Delete of the set is one step");
    w.run_steps(2);
    assert!(
        w.state().session.mailbox.event(lunch.id).is_some() && w.state().session.mailbox.event(dentist.id).is_some(),
        "one step brings both back"
    );
    select_set(&mut w, &ids);
    chord(&mut w, Modifiers::NONE, egui::Key::Delete);
    w.run_steps(3);
    let _asked = open_question(&w, "a second Delete asks again");
    press(&mut w, &keep_quiet);
    assert!(w.state().session.mailbox.event(lunch.id).is_none(), "Delete My Copy takes the first copy out");
    assert!(w.state().session.mailbox.event(dentist.id).is_none(), "and the second");
    assert!(answers_queued(&w, &ids).is_empty(), "the organizer is told nothing");
    assert!(w.state().session.mailbox.event_changes.iter().all(|change| !change.send), "and no mail is queued at all");

    // One step back again, and decline without telling: the answer is kept here, not sent.
    assert!(w.state_mut().session.undo(), "a Delete of the set is one step");
    w.run_steps(2);
    select_set(&mut w, &ids);
    chord(&mut w, Modifiers::NONE, egui::Key::Delete);
    w.run_steps(3);
    let _asked = open_question(&w, "a third Delete asks again");
    press(&mut w, &decline_quiet);
    assert!(w.state().session.mailbox.event(lunch.id).is_none(), "the first declined copy went");
    assert!(w.state().session.mailbox.event(dentist.id).is_none(), "and the second");
    let queued = answers_queued(&w, &ids);
    assert_eq!(queued.len(), 2, "both declines are kept for the account's server");
    assert!(
        queued.iter().all(|change| !change.send && change.response == Response::Declined),
        "and neither answer is sent to the organizer: {:?}",
        queued.iter().map(|change| (change.send, change.response)).collect::<Vec<_>>()
    );
    assert!(w.state().session.mailbox.event_changes.iter().all(|change| !change.send), "and nothing of any kind is queued to be sent");
}

/// One appointment the account's server keeps, with its organizer and one person invited: the
/// owner's own meeting, not one they were invited to.
fn kept_single() -> (mailcraft_model::Mailbox, Event) {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(now(), 0), now(), 0);
    let added = session
        .run("account.add", &json!({ "email": ACCOUNT, "name": "Zoe", "incoming": { "host": "imap.example.org" } }))
        .expect("an account is added");
    let account = added["id"].as_u64().expect("the account's id");
    let id = session.mailbox.events.values().find(|event| event.title == "Lunch with Dev").map(|event| event.id).expect("the sample has it");
    let event = session.mailbox.event_mut(id).expect("the event is there");
    event.origin = Origin { account, id: format!("server-{id}"), ..Origin::default() };
    event.organizer = Some(Address::new("Dev Example", "dev@example.org"));
    event.attendees = vec![Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Attendee::default() }];
    let marked = event.clone();
    assert!(!marked.origin.guest, "fixture: the owner's own meeting");
    (session.mailbox, marked)
}

/// One kept meeting, selected as a typed item, is a cancellation the owner must be asked about:
/// the shared reader counts it, the window asks about it, and the yes deletes exactly it and
/// queues the one cancellation the question promised. It is not refused as unsent, and the
/// keyboard's unnamed Delete is the same act as the item's own menu.
///
/// The shared reader is checked first. When this test was written, `items_mail_of` returned `None`
/// for a typed selection of one, so the question was never built; the window part after it is the
/// behaviour that failure hid.
#[test]
fn a_single_typed_kept_meeting_is_asked_about_as_one_cancellation() {
    let (mailbox, meeting) = kept_single();
    let mut session = Session::at(mailbox, now(), 0);
    session.run("calendar.select", &json!({ "items": [{ "id": meeting.id }] })).expect("the one meeting is selected as a typed item");
    assert_eq!(session.sel.events.len(), 1, "fixture: a typed selection of exactly one");
    let mail = mailcraft_engine::cmd::calendar::mail_of(&session, "calendar.delete", &json!({}));
    assert_eq!(
        mail,
        Some(mailcraft_engine::cmd::calendar::Mail::Cancellation(1)),
        "the shared reader must count one kept meeting selected as a typed item as one cancellation, so that the window asks about it rather than refusing the Delete as unsent"
    );

    // The same selection through the window, and the Delete key.
    let mut w = window_on(session.mailbox, "view.calendar");
    ready(&mut w);
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    let other = event(&w, "Design review").id;
    select_set(&mut w, &[meeting.id]);
    let before = w.state().session.mailbox.clone();
    chord(&mut w, Modifiers::NONE, egui::Key::Delete);
    w.run_steps(3);
    let asked = open_question(&w, "a Delete of one kept meeting selected as a typed item must ask about its cancellation, not refuse it as unsent");
    assert_eq!(asked.command, "calendar.delete", "the question is about the Delete");
    // (A set of one in focus is the one meeting in focus, benandez on #1665: asked about as Delete
    // asked before sets, with its cancellation.)
    assert!(asked.text.contains("Send the cancellation"), "and it asks about its cancellation: {}", asked.text);
    assert!(w.state().session.mailbox == before, "nothing is deleted while the question waits");

    // The yes: exactly that meeting goes, with the cancellation queued as mail.
    press(&mut w, &asked.action);
    assert!(w.state().session.mailbox.event(meeting.id).is_none(), "the yes deletes the one meeting");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len() - 1, "nothing else is deleted");
    assert!(w.state().session.mailbox.event(other).is_some(), "and no unrelated appointment went");
    let queued = w.state().session.mailbox.event_changes.iter().find(|change| change.kind == EventChangeKind::Delete && change.event == meeting.id);
    let queued = queued.expect("the cancellation is queued for the account's server");
    assert!(queued.send, "and it is queued as mail to the person invited, as the question said");
}

#[path = "calendar_mixed_guest_tests.rs"]
mod mixed_guest_tests;
