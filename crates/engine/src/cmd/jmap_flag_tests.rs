// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A follow-up in a JMAP account (M6). Its server keeps `$flagged` and nothing else: no due date
//! and no "complete". So whether the flag was changed here since the server last said is told by
//! that one bit, and a due date or a "complete" kept here neither holds the server's word off nor
//! is sent back against it. (The changes as they go up: `jmap_up_tests.rs`.)

use mailcraft_model::FlagStatus;
use mailcraft_model::sync::{JmapChange, JmapOutcome};
use mailcraft_model::time::Timestamp;
use serde_json::json;

use super::tests::{marks, run, window};
use super::up_tests::{answered, done, mail, refused, up};
use super::*;

/// A run that sends nothing up and reads the Inbox: `One` (E1) flagged there or not, `Two` as it was.
fn inbox_says(s: &mut Session, account: Id, flagged: bool) {
    run(s, account, vec![window("M1", vec![marks("E1", false, flagged), marks("E2", false, false)], true)], true);
}

/// The one change of `One`'s flag that would go up.
fn flag_goes(message: Id, flagged: bool) -> Vec<JmapChange> {
    vec![JmapChange::Keywords { message, id: "E1".into(), read: None, flagged: Some(flagged), answered: None, forwarded: None }]
}

/// Send up what waits, the server making all of it.
fn made(s: &mut Session, account: Id) {
    let changes = up(s, account);
    answered(s, account, changes.into_iter().map(|change| (change, done())).collect());
}

fn flag_of(s: &Session, id: Id) -> (FlagStatus, Option<Timestamp>) {
    s.mailbox.message(id).map(|m| (m.flag, m.flag_due)).unwrap()
}

#[test]
fn a_follow_up_with_a_due_date_cleared_on_another_device_is_cleared_here_and_not_flagged_there_again() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.flag", &json!({"ids": [one], "due": "2027-01-10"})).unwrap();
    assert!(flag_of(&s, one).1.is_some());
    assert_eq!(up(&s, account), flag_goes(one, true));
    made(&mut s, account);
    // On a phone the flag is cleared; the next run reads that.
    inbox_says(&mut s, account, false);
    assert_eq!(flag_of(&s, one), (FlagStatus::None, None));
    assert!(up(&s, account).is_empty(), "the server isn't flagged again: {:?}", up(&s, account));
    // Nor at the run after.
    inbox_says(&mut s, account, false);
    assert!(up(&s, account).is_empty());
}

#[test]
fn a_follow_up_marked_complete_flagged_again_on_another_device_is_flagged_here_and_not_cleared_there_again() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.flag", &json!({"ids": [one]})).unwrap();
    made(&mut s, account);
    s.run("mail.complete", &json!({"ids": [one]})).unwrap();
    assert_eq!(up(&s, account), flag_goes(one, false));
    made(&mut s, account);
    assert_eq!(flag_of(&s, one).0, FlagStatus::Complete);
    // On a phone it is flagged again; the next run reads that.
    inbox_says(&mut s, account, true);
    assert_eq!(flag_of(&s, one), (FlagStatus::Flagged, None));
    assert!(up(&s, account).is_empty(), "the server's flag isn't taken off again: {:?}", up(&s, account));
    inbox_says(&mut s, account, true);
    assert!(up(&s, account).is_empty());
}

#[test]
fn a_due_date_and_a_complete_stay_while_the_server_says_what_it_said() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.flag", &json!({"ids": [one], "due": "2027-01-10"})).unwrap();
    made(&mut s, account);
    let flagged = flag_of(&s, one);
    // Nothing changed elsewhere: flagged, with its due date, run after run.
    for _ in 0..2 {
        inbox_says(&mut s, account, true);
        assert_eq!(flag_of(&s, one), flagged);
        assert!(up(&s, account).is_empty());
    }
    // Complete, with the date it was due: so it stays while the server says "not flagged".
    s.run("mail.complete", &json!({"ids": [one]})).unwrap();
    made(&mut s, account);
    for _ in 0..2 {
        inbox_says(&mut s, account, false);
        assert_eq!(flag_of(&s, one), (FlagStatus::Complete, flagged.1));
        assert!(up(&s, account).is_empty());
    }
}

#[test]
fn a_complete_made_here_goes_up_once_though_a_window_says_flagged_before_it_went() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.flag", &json!({"ids": [one], "due": "2027-01-10"})).unwrap();
    made(&mut s, account);
    s.run("mail.complete", &json!({"ids": [one]})).unwrap();
    // A run that answered nothing of it (the connection went) still says flagged: changed here
    // since, it stays complete, and still waits to go.
    inbox_says(&mut s, account, true);
    assert_eq!(flag_of(&s, one).0, FlagStatus::Complete);
    assert_eq!(up(&s, account), flag_goes(one, false));
    // Gone up, it is settled: the windows after say "not flagged", and nothing goes again.
    made(&mut s, account);
    for _ in 0..2 {
        inbox_says(&mut s, account, false);
        assert_eq!(flag_of(&s, one).0, FlagStatus::Complete);
        assert!(up(&s, account).is_empty());
    }
}

#[test]
fn a_flag_changed_here_and_elsewhere_between_two_runs_ends_as_both_said_with_what_only_this_computer_keeps() {
    let (mut s, account, [one, ..]) = mail();
    // Flagged here with a due date, and on a phone, before a run: the server's bit is what was
    // meant here too, so nothing goes up, and the due date, which only this computer keeps, stays.
    s.run("mail.flag", &json!({"ids": [one], "due": "2027-01-10"})).unwrap();
    let flagged = flag_of(&s, one);
    inbox_says(&mut s, account, true);
    assert_eq!(flag_of(&s, one), flagged);
    assert!(up(&s, account).is_empty());
    // Marked complete here and cleared on the phone: not flagged on either side, complete here.
    s.run("mail.complete", &json!({"ids": [one]})).unwrap();
    inbox_says(&mut s, account, false);
    assert_eq!(flag_of(&s, one), (FlagStatus::Complete, flagged.1));
    assert!(up(&s, account).is_empty());
    // Flagged here, and the phone's flag unchanged: changed here alone, it goes up.
    s.run("mail.flag", &json!({"ids": [one]})).unwrap();
    inbox_says(&mut s, account, false);
    assert_eq!(flag_of(&s, one).0, FlagStatus::Flagged);
    assert_eq!(up(&s, account), flag_goes(one, true));
    made(&mut s, account);
    // Another due date here (the bit as the server has it) and cleared on the phone: the one
    // change a server can be told of was made there, so the flag goes, with its date.
    s.run("mail.flag", &json!({"ids": [one], "due": "2027-02-01"})).unwrap();
    assert!(up(&s, account).is_empty());
    inbox_says(&mut s, account, false);
    assert_eq!(flag_of(&s, one), (FlagStatus::None, None));
    assert!(up(&s, account).is_empty());
}

#[test]
fn a_refused_flag_with_a_due_date_or_a_refused_complete_is_put_back_and_not_sent_again() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.flag", &json!({"ids": [one], "due": "2027-01-10"})).unwrap();
    let flagged = flag_of(&s, one);
    // The server wouldn't flag it: not flagged here either, and no due date left behind.
    let changes = up(&s, account);
    answered(&mut s, account, changes.into_iter().map(|change| (change, refused("the mailbox is read-only"))).collect());
    assert_eq!(flag_of(&s, one), (FlagStatus::None, None));
    assert!(up(&s, account).is_empty());
    // Flagged for good now, then marked complete, which the server refuses: flagged again, as the
    // server has it, with the date it was due, and nothing is sent again.
    s.run("mail.flag", &json!({"ids": [one], "due": "2027-01-10"})).unwrap();
    made(&mut s, account);
    s.run("mail.complete", &json!({"ids": [one]})).unwrap();
    let changes = up(&s, account);
    assert_eq!(changes, flag_goes(one, false));
    answered(&mut s, account, changes.into_iter().map(|change| (change, refused("the mailbox is read-only"))).collect());
    assert_eq!(flag_of(&s, one), flagged);
    assert!(up(&s, account).is_empty());
    inbox_says(&mut s, account, true);
    assert_eq!(flag_of(&s, one), flagged);
    assert!(up(&s, account).is_empty());
    // A message the server no longer has: the same, and it is not asked about again.
    s.run("mail.complete", &json!({"ids": [one]})).unwrap();
    let changes = up(&s, account);
    answered(&mut s, account, changes.into_iter().map(|change| (change, JmapOutcome::Gone)).collect());
    assert_eq!(flag_of(&s, one), flagged);
    assert!(up(&s, account).is_empty());
}

#[test]
fn undo_of_a_follow_up_that_went_up_goes_up_once_and_one_cleared_elsewhere_has_nothing_to_undo_there() {
    let (mut s, account, [one, two, _]) = mail();
    s.run("mail.flag", &json!({"ids": [one], "due": "2027-01-10"})).unwrap();
    made(&mut s, account);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(flag_of(&s, one), (FlagStatus::None, None));
    assert_eq!(up(&s, account), flag_goes(one, false));
    made(&mut s, account);
    inbox_says(&mut s, account, false);
    assert!(up(&s, account).is_empty());
    // Redone, gone up, then cleared on a phone: undoing the follow-up finds it cleared already.
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(flag_of(&s, one).0, FlagStatus::Flagged);
    made(&mut s, account);
    inbox_says(&mut s, account, false);
    assert_eq!(flag_of(&s, one), (FlagStatus::None, None));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(flag_of(&s, one), (FlagStatus::None, None));
    assert!(up(&s, account).is_empty(), "{:?}", up(&s, account));
    assert_eq!(flag_of(&s, two), (FlagStatus::None, None));
}
