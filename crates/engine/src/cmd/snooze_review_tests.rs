// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snooze (#949), the cases a review found: undo of a snooze and of Return now once the server has
//! the moves, a move back the server refused, Return times across a change of the clocks, early
//! return only by a known link, and a Snoozed folder the owner deleted.

use mailcraft_model::sync::{
    ChangeOutcome, EwsMailResult, FolderChangeDone, GraphFolder, GraphMailResult, MessageChange, MessageChangeDone, MessageOutcome, ServerMessage,
};
use mailcraft_model::{Account, AccountKind, Address, FolderKind, Id, Message, Snooze};
use serde_json::json;

use super::ews_mail::up_tests::{piece, setup as ews_setup, took};
use super::ews_mail::{apply_ews_mail, ews_mail_request};
use super::graph_mail::{apply_graph_mail, graph_mail_request};
use super::graph_up::tests::{NOW, setup as graph_setup};
use super::snooze;
use crate::{Clock, Session};

fn at(s: &mut Session, now: i64) {
    s.clock = Clock { now: Some(now), offset: Some(0), zone: None };
}

fn graph_answer(s: &mut Session, account: Id, outcomes: Vec<(usize, MessageOutcome)>, folder: Option<GraphFolder>) {
    let request = graph_mail_request(s, account).unwrap();
    let changes = folder
        .map(|folder| vec![FolderChangeDone { change: request.changes[0].clone(), outcome: ChangeOutcome::Done { folder } }])
        .unwrap_or_default();
    let messages = outcomes.into_iter().map(|(i, outcome)| MessageChangeDone { change: request.messages[i].clone(), outcome, there: None }).collect();
    apply_graph_mail(s, GraphMailResult { account, email: "owner@example.org".into(), changes, messages, ..Default::default() }).unwrap();
}

fn done(folder: &str) -> MessageOutcome {
    MessageOutcome::Done { categories: None, flag: None, folder: folder.into(), id: "M-1".into() }
}

/// Snoozed, and the server has the folder and the move.
fn graph_snoozed() -> (Session, Id, Id) {
    let (mut s, account, m, _) = graph_setup();
    s.run("mail.snooze", &json!({"ids": [m], "until": NOW + 600})).unwrap();
    let there = GraphFolder { id: "F-sn".into(), name: "Snoozed".into(), kind: FolderKind::Custom, ..Default::default() };
    graph_answer(&mut s, account, vec![(0, done("F-sn"))], Some(there));
    (s, account, m)
}

#[test]
fn graph_undo_of_return_now_after_the_server_moved_it_waits_in_snoozed_again() {
    let (mut s, account, m) = graph_snoozed();
    let snoozed = s.mailbox.snoozed_folder(account).unwrap();
    s.run("mail.unsnooze", &json!({"ids": [m]})).unwrap();
    graph_answer(&mut s, account, vec![(0, done("F-in"))], None);
    assert!(s.mailbox.message(m).unwrap().snooze.is_some_and(|z| z.returned.is_some()));
    s.run("edit.undo", &json!({})).unwrap();
    let here = s.mailbox.message(m).unwrap();
    assert_eq!(here.folder, snoozed);
    assert!(here.snooze.is_some_and(|z| z.waiting() && z.until == NOW + 600), "{:?}", here.snooze);
    // So it isn't stuck: it comes back at its time.
    at(&mut s, NOW + 601);
    assert_eq!(s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([m]));
}

#[test]
fn graph_undo_of_a_snooze_after_the_server_moved_it_leaves_no_snooze() {
    let (mut s, account, m) = graph_snoozed();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    let here = s.mailbox.message(m).unwrap();
    assert_eq!((here.folder, here.snooze), (inbox, None));
}

#[test]
fn graph_a_move_back_the_server_refused_is_tried_again() {
    let (mut s, account, m) = graph_snoozed();
    at(&mut s, NOW + 600);
    assert_eq!(s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([m]));
    graph_answer(&mut s, account, vec![(0, MessageOutcome::Refused { status: 400 })], None);
    let snoozed = s.mailbox.snoozed_folder(account).unwrap();
    assert_eq!(s.mailbox.message(m).unwrap().folder, snoozed, "the server's folder is where it is");
    // The next step takes it for waiting again and brings it back again.
    assert_eq!(s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([m]));
    assert!(matches!(graph_mail_request(&s, account).unwrap().messages.as_slice(), [MessageChange::Move { to, .. }] if to == "F-in"));
}

fn ews_snoozed() -> (Session, Id, Id, Id, i64) {
    let (mut s, account, a, b) = ews_setup();
    let now = s.now();
    at(&mut s, now);
    s.run("mail.snooze", &json!({"ids": [a], "until": now + 600})).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    let there = GraphFolder { id: "FS".into(), name: "Snoozed".into(), kind: FolderKind::Custom, ..Default::default() };
    let made = FolderChangeDone { change: request.changes[0].clone(), outcome: ChangeOutcome::Done { folder: there } };
    let moved = MessageChangeDone { change: request.messages[0].clone(), outcome: done_as("FS", "A1-moved"), there: None };
    took(&mut s, account, EwsMailResult { changes: vec![made], messages: vec![moved], partial: true, ..piece(account) });
    (s, account, a, b, now)
}

fn done_as(folder: &str, id: &str) -> MessageOutcome {
    MessageOutcome::Done { categories: None, flag: None, folder: folder.into(), id: id.into() }
}

#[test]
fn ews_undo_of_return_now_and_of_a_snooze_after_the_server_moved_it() {
    let (mut s, account, a, _, now) = ews_snoozed();
    let snoozed = s.mailbox.snoozed_folder(account).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.run("mail.unsnooze", &json!({"ids": [a]})).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    let moved = MessageChangeDone { change: request.messages[0].clone(), outcome: done_as("F1", "A1-back"), there: None };
    took(&mut s, account, EwsMailResult { messages: vec![moved], partial: true, ..piece(account) });
    s.run("edit.undo", &json!({})).unwrap();
    let here = s.mailbox.message(a).unwrap();
    assert_eq!(here.folder, snoozed);
    assert!(here.snooze.is_some_and(|z| z.waiting() && z.until == now + 600), "{:?}", here.snooze);
    s.run("edit.undo", &json!({})).unwrap();
    let here = s.mailbox.message(a).unwrap();
    assert_eq!((here.folder, here.snooze), (inbox, None));
    let _ = apply_ews_mail;
}

fn local() -> (Session, Id, Id, Id) {
    let mut s = Session::new(mailcraft_model::Mailbox::with_local_account("Ann", "ann@example.org"));
    at(&mut s, NOW);
    let account = s.mailbox.accounts[0].id;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let a = s
        .mailbox
        .add_message(Message {
            account,
            folder: inbox,
            subject: "Kickoff".into(),
            message_id: "a@example.org".into(),
            received: NOW - 50,
            ..Default::default()
        })
        .unwrap();
    (s, account, inbox, a)
}

fn waiting(s: &Session, id: Id) -> bool {
    s.mailbox.message(id).unwrap().snooze.is_some_and(|z| z.waiting())
}

fn arrive(s: &mut Session, account: Id, inbox: Id, f: impl FnOnce(&mut Message)) -> Id {
    let mut m = Message { account, folder: inbox, subject: "Re: Kickoff".into(), received: NOW + 10, ..Default::default() };
    f(&mut m);
    s.mailbox.add_message(m).unwrap()
}

#[test]
fn a_return_time_across_a_change_of_the_clocks_is_the_wall_clock_time_written() {
    let (mut s, _, _, a) = local();
    s.prefs.time_zone = "Europe/London".into();
    s.clock = Clock { now: Some(NOW), offset: None, zone: None };
    // Now is January (UTC+0); the clocks go forward on 2027-03-28, so 09:00 there is 08:00 UTC.
    s.run("mail.snooze", &json!({"ids": [a], "until": "2027-03-28T09:00"})).unwrap();
    let wall = mailcraft_time::parse_iso("2027-03-28T08:00").unwrap().0;
    let expected = mailcraft_time::Zone::fixed(0).unwrap().instant(wall);
    assert_eq!(s.mailbox.message(a).unwrap().snooze.unwrap().until, expected);
    // A time with its own offset is that offset's, and absurd text or years are refused.
    let (mut s, _, _, a) = local();
    s.run("mail.snooze", &json!({"ids": [a], "until": "2027-03-28T09:00+02:00"})).unwrap();
    for text in ["2027-03-28T09:00".repeat(10), "1800-01-01T00:00".into(), "9999-01-01T00:00".into()] {
        assert!(s.run("mail.snooze", &json!({"ids": [a], "until": text})).is_err());
    }
}

#[test]
fn a_reply_that_got_its_conversation_id_in_a_later_piece_brings_it_back_then() {
    let (mut s, account, inbox, a) = local();
    s.mailbox.message_mut(a).unwrap().server = Some(ServerMessage { thread: "C1".into(), ..Default::default() });
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 86_400})).unwrap();
    let reply = arrive(&mut s, account, inbox, |m| m.server = Some(ServerMessage { id: "R".into(), ..Default::default() }));
    snooze::after(&mut s);
    assert!(waiting(&s, a), "no conversation id yet, no link");
    // A later piece brings the conversation id, below any mark taken before the run.
    s.mailbox.message_mut(reply).unwrap().server.as_mut().unwrap().thread = "C1".into();
    snooze::after(&mut s);
    assert!(!waiting(&s, a));
}

#[test]
fn unrelated_mail_with_the_same_subject_does_not_bring_it_back() {
    let (mut s, account, inbox, a) = local();
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 86_400})).unwrap();
    arrive(&mut s, account, inbox, |m| m.subject = "Kickoff".into());
    arrive(&mut s, account, inbox, |m| (m.message_id, m.in_reply_to) = ("x@example.org".into(), "other@example.org".into()));
    arrive(&mut s, account, inbox, |m| m.references = vec!["elsewhere@example.org".into()]);
    snooze::after(&mut s);
    assert!(waiting(&s, a));
    // A reference to it, deep in the list, does.
    arrive(&mut s, account, inbox, |m| m.references = vec!["root@example.org".into(), "a@example.org".into()]);
    snooze::after(&mut s);
    assert!(!waiting(&s, a));
}

#[test]
fn mail_from_the_owners_own_address_and_mail_that_arrived_before_the_snooze_is_no_news() {
    let (mut s, account, inbox, a) = local();
    let before = arrive(&mut s, account, inbox, |m| (m.in_reply_to, m.received) = ("a@example.org".into(), NOW - 10));
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 86_400})).unwrap();
    arrive(&mut s, account, inbox, |m| (m.in_reply_to, m.from) = ("a@example.org".into(), Address::new("Ann", "ANN@example.org")));
    snooze::after(&mut s);
    assert!(waiting(&s, a));
    assert!(s.mailbox.message(before).is_some());
}

#[test]
fn after_marks_the_mailbox_changed_when_it_changes_a_snooze() {
    let (mut s, account, inbox, a) = local();
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 86_400})).unwrap();
    let rev = s.rev();
    snooze::after(&mut s);
    assert_eq!(s.rev(), rev, "nothing changed");
    arrive(&mut s, account, inbox, |m| m.in_reply_to = "a@example.org".into());
    snooze::after(&mut s);
    assert!(s.rev() > rev);
}

#[test]
fn a_snoozed_folder_the_owner_deleted_isnt_filed_into_again() {
    let (mut s, account, inbox, a) = local();
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 86_400})).unwrap();
    let old = s.mailbox.snoozed_folder(account).unwrap();
    s.run("folder.delete", &json!({"id": old})).unwrap();
    let b = arrive(&mut s, account, inbox, |_| {});
    s.run("mail.snooze", &json!({"ids": [b], "until": NOW + 86_400})).unwrap();
    let fresh = s.mailbox.snoozed_folder(account).unwrap();
    assert_ne!(fresh, old);
    assert_eq!(s.mailbox.folder(fresh).unwrap().parent, None);
    // Emptying Deleted Items takes the old one and nothing that waits.
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.run("folder.empty", &json!({"id": deleted})).unwrap();
    assert!(s.mailbox.message(b).is_some_and(|m| m.folder == fresh));
}

#[test]
fn mail_that_cant_come_back_does_not_starve_the_rest_of_the_due_ones() {
    let (mut s, _, _, a) = local();
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 600})).unwrap();
    let jmap = s.mailbox.add_account(Account { name: "Jay".into(), email: "jay@example.org".into(), kind: AccountKind::Jmap, ..Default::default() });
    let jmap_inbox = s.mailbox.folder_of_kind(jmap, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.add_message(Message { account: jmap, folder: jmap_inbox, ..Default::default() }).unwrap();
    s.mailbox.snooze_messages(&[theirs], NOW + 60, "Snoozed").unwrap();
    let snoozed = s.mailbox.snoozed_folder(jmap).unwrap();
    // The server's own mail waiting in a Snoozed folder (a file said so) can't move yet.
    for n in 0..(snooze::MOST_RETURNED_AT_ONCE + 5) {
        let server = ServerMessage { id: format!("J{n}"), folder: "mb".into(), ..Default::default() };
        let id = s.mailbox.add_message(Message { account: jmap, folder: snoozed, server: Some(server), ..Default::default() }).unwrap();
        s.mailbox.message_mut(id).unwrap().snooze = Some(Snooze { until: NOW + 1, after_id: 0, from: jmap_inbox, ..Default::default() });
    }
    at(&mut s, NOW + 700);
    let returned = s.run("mail.returnDue", &json!({})).unwrap()["returned"].clone();
    assert!(returned.as_array().unwrap().contains(&json!(a)), "{returned}");
    assert!(returned.as_array().unwrap().contains(&json!(theirs)));
}

#[test]
fn graph_a_move_back_the_server_keeps_refusing_is_given_up_after_three_and_said_once() {
    let (mut s, account, m) = graph_snoozed();
    at(&mut s, NOW + 600);
    let mut gave_up = Vec::new();
    for round in 1..=4 {
        let answer = s.run("mail.returnDue", &json!({})).unwrap();
        gave_up.extend(answer["gaveUp"].as_array().unwrap().clone());
        let returned = answer["returned"].as_array().unwrap().len();
        if returned == 0 {
            assert!(round > 3, "round {round}");
            break;
        }
        // The server refuses; the run's answer says so only when it gives up.
        let request = graph_mail_request(&s, account).unwrap();
        let change = MessageChangeDone { change: request.messages[0].clone(), outcome: MessageOutcome::Refused { status: 400 }, there: None };
        let said =
            apply_graph_mail(&mut s, GraphMailResult { account, email: "owner@example.org".into(), messages: vec![change], ..Default::default() })
                .unwrap();
        let errors = said["errors"].to_string();
        assert_eq!(errors.contains("no longer snoozed"), round == 3, "round {round}: {errors}");
    }
    assert_eq!(s.mailbox.message(m).unwrap().snooze, None);
    assert_eq!(s.mailbox.message(m).unwrap().folder, s.mailbox.snoozed_folder(account).unwrap());
    // Nothing is tried again.
    assert_eq!(s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([]));
    let _ = gave_up;
}

#[test]
fn return_due_tells_a_reset_from_a_call_off() {
    let (mut s, account, _, a) = local();
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 600})).unwrap();
    at(&mut s, NOW + 700);
    s.run("mail.returnDue", &json!({})).unwrap();
    // Refused as a server would: back in Snoozed with the return in flight.
    let snoozed = s.mailbox.snoozed_folder(account).unwrap();
    s.mailbox.message_mut(a).unwrap().folder = snoozed;
    let answer = s.run("mail.returnDue", &json!({})).unwrap();
    assert_eq!((answer["reset"].clone(), answer["calledOff"].clone(), answer["returned"].clone()), (json!(1), json!(0), json!([a])));
}

#[test]
fn a_message_that_came_back_and_was_filed_into_snoozed_by_hand_stays_there() {
    let (mut s, account, inbox, a) = local();
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 600})).unwrap();
    at(&mut s, NOW + 700);
    s.run("mail.returnDue", &json!({})).unwrap();
    s.run("mail.returnDue", &json!({})).unwrap();
    let snoozed = s.mailbox.snoozed_folder(account).unwrap();
    s.run("mail.move", &json!({"ids": [a], "folder": snoozed})).unwrap();
    let answer = s.run("mail.returnDue", &json!({})).unwrap();
    assert_eq!((answer["returned"].clone(), answer["reset"].clone(), answer["calledOff"].clone()), (json!([]), json!(0), json!(1)));
    let m = s.mailbox.message(a).unwrap();
    assert_eq!((m.folder, m.snooze), (snoozed, None));
    let _ = inbox;
}

#[test]
fn calling_off_the_last_snooze_after_a_run_marks_the_mailbox_changed() {
    let (mut s, account, _, a) = local();
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 86_400})).unwrap();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    s.mailbox.move_message(a, archive).unwrap();
    let rev = s.rev();
    snooze::after(&mut s);
    assert_eq!(s.mailbox.message(a).unwrap().snooze, None);
    assert!(s.rev() > rev);
}

#[test]
fn a_reply_delivered_late_with_an_old_date_still_brings_it_back() {
    let (mut s, account, inbox, a) = local();
    s.run("mail.snooze", &json!({"ids": [a], "until": NOW + 86_400})).unwrap();
    // The server dated it before the snooze (internal date, clock skew, a late delivery).
    arrive(&mut s, account, inbox, |m| (m.in_reply_to, m.received, m.date) = ("a@example.org".into(), NOW - 10_000, NOW - 10_000));
    snooze::after(&mut s);
    assert!(!waiting(&s, a));
}
