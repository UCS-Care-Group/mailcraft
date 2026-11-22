// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One mixed set of a whole appointment the owner keeps on the account's server and one whole
//! appointment somebody else organizes, held against the window itself: the Delete key asks once
//! before anything goes, the question names both and tells the owner's cancellation apart from
//! the guest copy, the way ahead it offers is the owner's cancellation (`send`, not `decline`),
//! and the yes removes both while queueing mail only for the owner's meeting. One Undo puts
//! both back.
//!
//! One module of `calendar_selection_tests::review_regressions`, so `super::*` is that file's
//! own fixtures and helpers (`guest_window`, `open_question`, `press`, `select_set`, `Asked`,
//! `ACCOUNT`), and `super::super` is `calendar_selection_tests`'s (`click`, `chord`, `selected`,
//! `drawn`, `date`, `at`, `COMMAND`).
//! Nothing here is production code and nothing here is a test hook: the window is the real one,
//! its events are synthetic (invented people on `example.org`), the clock is fixed, and every act
//! is a key, a command or a button the owner would press. No file, server, real account, native
//! window or wall clock is used.
//!
//! WHAT IS HELD (source read 2026-11-15):
//! - `calendar::selection::delete_selected` takes `send` for a batch that invites anybody only
//!   as `Some(true)`; `answering` is `decline && kept && guest`, so a guest copy deleted with
//!   `decline:false` goes through `note_deleted` (a `Delete` change with `send` false) and never
//!   queues a `Respond`; `approve_mail` is reached only for an item with `send` true,
//!   `!remote_id.is_empty()` and `invited > 0`, which is the owner's own meeting alone.
//! - `calendar::selection::items_mail_of` adds `invited` only for a kept item that is not a
//!   guest, so this set is `Mail::Cancellation(1)`.
//! - `dialogs::batch_question` asks about the set once; its one question must therefore say what
//!   is cancelled and what is only removed from the owner's calendar.
//! - `cmd::remote::approve_mail` marks the owned meeting's queued deletion as mail, so the
//!   owner's cancellation is the only outgoing change.
//!
//! The fixture facts are checked first (one role each, both server ids and attendees kept), then
//! the engine reader, then the real key, the real question and the real buttons. No colour,
//! pixel, translated caption or network delivery outcome is asserted; labels come from the
//! question itself and are pressed through the existing helper.

use egui::{Modifiers, PointerButton};
use mailcraft_engine::cmd::calendar::{Mail, mail_of};
use mailcraft_model::Event;
use mailcraft_model::remote::EventChangeKind;
use serde_json::{Value, json};

use super::super::{chord, click, drawn, selected};
use super::*;
use crate::people_view::testing::Window;

/// A window on the sample whose `Lunch with Dev` and `Dentist` are both kept by the added
/// account's server. One is turned into the owner's own meeting (`origin.guest` false) before
/// anything is selected, keeping its account, its server id, its organizer and the person
/// invited; the other stays somebody else's meeting the owner is invited to.
fn mixed_window() -> (Window, Event, Event) {
    let (mut w, lunch, dentist) = guest_window();
    {
        let owned = w.state_mut().session.mailbox.event_mut(lunch.id).expect("the sample's Lunch with Dev is there");
        owned.origin.guest = false;
    }
    let owned = w.state().session.mailbox.event(lunch.id).cloned().expect("the owner's meeting is still there");
    let guest = w.state().session.mailbox.event(dentist.id).cloned().expect("the guest meeting is still there");

    assert_eq!(owned.title, "Lunch with Dev", "fixture: the first sample appointment");
    assert_eq!(guest.title, "Dentist", "fixture: the second sample appointment");
    assert!(!owned.origin.guest, "fixture: the first is now the owner's own meeting");
    assert!(guest.origin.guest, "fixture: the second is still somebody else's, which the owner is invited to");
    assert_ne!(owned.origin.guest, lunch.origin.guest, "fixture: the owner's meeting is the one whose role changed");
    assert_eq!(guest.origin.guest, dentist.origin.guest, "fixture: the guest meeting keeps its role");
    assert_eq!(owned.origin.account, lunch.origin.account, "fixture: the owner's meeting keeps its account");
    assert_eq!(guest.origin.account, dentist.origin.account, "fixture: the guest meeting keeps its account");
    assert_eq!(owned.origin.id, lunch.origin.id, "fixture: the owner's meeting keeps the server's id");
    assert_eq!(guest.origin.id, dentist.origin.id, "fixture: the guest meeting keeps the server's id");
    assert!(!owned.origin.id.is_empty() && !guest.origin.id.is_empty(), "fixture: both are on the account's server");
    assert_eq!(owned.attendees, lunch.attendees, "fixture: the owner's meeting keeps the person invited");
    assert_eq!(guest.attendees, dentist.attendees, "fixture: the guest meeting keeps the person invited");
    assert_eq!(owned.attendees.len(), 1, "fixture: one person is invited to the owner's meeting");
    assert_eq!(guest.attendees.len(), 1, "fixture: one person is invited to the guest meeting");
    assert!(owned.organizer.is_some() && guest.organizer.is_some(), "fixture: both have an organizer");
    (w, owned, guest)
}

/// A Delete of a typed set that holds the owner's own whole meeting and one whole meeting
/// somebody else organizes:
/// - the engine reader counts one cancellation (the owner's one invited person) and no answer
///   for the guest copy;
/// - the Delete key asks before anything is removed, and the question names both meetings and
///   says the owner's meeting is cancelled while the guest copy is removed without answering its
///   organizer;
/// - every offered way ahead can do what it says: none asks for `send:false`, which the engine
///   refuses for the owner's meeting; the primary one is `send` and not `decline`;
/// - that primary action removes both, queues the owner's deletion as mail (`send` true) and the
///   guest copy's deletion without mail (`send` false), and queues no answer at all;
/// - one Undo puts both back exactly, with no queued change and the set selected again.
#[test]
fn a_mixed_owned_and_guest_set_is_one_cancellation_and_one_quiet_deletion() {
    let (mut w, owned, guest) = mixed_window();
    let ids = [owned.id, guest.id];

    // The reader the question is built from: one person is told, on the owner's meeting.
    let named = json!([{ "id": owned.id }, { "id": guest.id }]);
    assert_eq!(
        mail_of(&w.state().session, "calendar.delete", &json!({ "items": named })),
        Some(Mail::Cancellation(1)),
        "the reader counts the owner's one invited person as one cancellation and asks for no answer to the guest meeting's organizer"
    );

    // The set, through the window's own command, after a click has made the calendar the keys' own.
    let on_review = drawn(&w, "Design review").center();
    click(&mut w, on_review, Modifiers::NONE, PointerButton::Primary);
    select_set(&mut w, &ids);
    assert_eq!(selected(&w), [(owned.id, None), (guest.id, None)], "fixture: the mixed pair is a typed set of two");
    assert_eq!(w.state().session.sel.events.len(), 2, "fixture: a typed selection of exactly two");

    // Delete: the question waits, and nothing is removed while it does.
    let before = w.state().session.mailbox.clone();
    chord(&mut w, Modifiers::NONE, egui::Key::Delete);
    w.run_steps(3);
    let asked = open_question(&w, "a Delete of a typed set holding the owner's meeting and a guest meeting must ask before taking either out");
    assert_eq!(asked.command, "calendar.delete", "the question is about the Delete");
    assert!(w.state().session.mailbox == before, "nothing is removed while the question waits");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len(), "all the events are still there");
    assert!(w.state().session.mailbox.event(owned.id).is_some(), "the owner's meeting is still there");
    assert!(w.state().session.mailbox.event(guest.id).is_some(), "and the guest copy too");

    // Cancel goes nowhere: the dialog closes and the mailbox is exactly as it was.
    press(&mut w, &asked.cancel);
    assert!(w.state().dialog.is_none(), "Cancel closes the question");
    assert!(w.state().session.mailbox == before, "Cancel removes nothing and queues nothing");

    // A second Delete asks again; the question is the same set.
    chord(&mut w, Modifiers::NONE, egui::Key::Delete);
    w.run_steps(3);
    let asked = open_question(&w, "the mixed set is still there, so a second Delete asks again");

    // The question says what is cancelled and what is only the owner's copy going.
    assert!(asked.text.contains("Lunch with Dev"), "the one question names the owner's meeting: {}", asked.text);
    assert!(asked.text.contains("Dentist"), "and the guest meeting: {}", asked.text);
    let body = asked.text.to_lowercase();
    assert!(body.contains(ACCOUNT), "it says whose server sends the owner's cancellation: {}", asked.text);
    assert!(body.contains("cancel"), "it says the owner's meeting is cancelled: {}", asked.text);
    assert!(
        ["not told", "isn't told", "won't be told", "is not told", "not automatically", "no answer", "not answered"]
            .iter()
            .any(|said| body.contains(said))
            && (body.contains("organizer") || body.contains("invited")),
        "it says the guest copy is removed without answering its organizer, unlike the owner's cancellation: {}",
        asked.text
    );

    // Every way ahead must be one the engine can carry out for the owner's meeting.
    for (label, with) in asked.choices() {
        assert_eq!(
            with.get("send").and_then(Value::as_bool),
            Some(true),
            "the way {label:?} cannot succeed for the owner's meeting: a batch that invites anybody and says send:false is refused as unsent ({with})"
        );
    }
    assert_eq!(asked.with.get("send").and_then(Value::as_bool), Some(true), "the primary action sends the owner's cancellation");
    assert_ne!(
        asked.with.get("decline").and_then(Value::as_bool),
        Some(true),
        "the primary action deletes the owner's meeting and sends its cancellation; it is not a decline of somebody else's meeting"
    );

    // The primary action: both go, one queued as mail, one without.
    let primary = asked.action.clone();
    press(&mut w, &primary);
    assert!(w.state().session.mailbox.event(owned.id).is_none(), "the owner's meeting went");
    assert!(w.state().session.mailbox.event(guest.id).is_none(), "and the guest copy");
    assert_eq!(w.state().session.mailbox.events.len(), before.events.len() - 2, "nothing else was removed");
    {
        let changes: Vec<_> = w.state().session.mailbox.event_changes.iter().filter(|change| ids.contains(&change.event)).collect();
        assert_eq!(changes.len(), 2, "one deletion is queued for each meeting, and nothing else");
        assert!(
            changes.iter().all(|change| change.kind == EventChangeKind::Delete),
            "both are deletions: {:?}",
            changes.iter().map(|change| change.kind).collect::<Vec<_>>()
        );
        let owned_change = changes.iter().find(|change| change.event == owned.id).expect("the owner's deletion is queued");
        assert!(owned_change.send, "the owner's deletion is queued as the cancellation the question promised");
        assert_eq!(owned_change.remote.as_str(), owned.origin.id.as_str(), "it names the owner's meeting on the server");
        let guest_change = changes.iter().find(|change| change.event == guest.id).expect("the guest copy's deletion is queued");
        assert!(!guest_change.send, "the guest copy's deletion is queued without mail");
        assert_eq!(guest_change.remote.as_str(), guest.origin.id.as_str(), "and it names the guest meeting on the server");
    }
    assert!(
        w.state().session.mailbox.event_changes.iter().all(|change| change.kind == EventChangeKind::Delete),
        "no answer or other change is queued: {:?}",
        w.state().session.mailbox.event_changes.iter().map(|change| change.kind).collect::<Vec<_>>()
    );

    // One Undo: both back exactly, nothing left queued, and the set selected again.
    assert!(w.state_mut().session.undo(), "a Delete of the mixed set is one step");
    w.run_steps(2);
    assert_eq!(w.state().session.mailbox.event(owned.id), Some(&owned), "one Undo brings the owner's meeting back exactly");
    assert_eq!(w.state().session.mailbox.event(guest.id), Some(&guest), "and the guest copy exactly");
    assert!(w.state().session.mailbox.event_changes.is_empty(), "and takes back both queued deletions, so nothing is sent");
    assert_eq!(selected(&w), [(owned.id, None), (guest.id, None)], "and the mixed pair is selected again");
}

#[path = "mixed_layout_tests.rs"]
mod mixed_layout_tests;
