// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::round::*;

fn w(account: u64, count: usize) -> Waiting {
    Waiting { account, count }
}

#[test]
fn a_round_is_due_by_the_owners_minutes_and_never_at_zero_or_with_auto_sync_off() {
    let mut r = RoundDriver::default();
    assert!(r.round_due(true, 5.0, 1.0), "the first one is due at once");
    assert!(!r.round_due(false, 5.0, 1.0), "host turned it off");
    assert!(!r.round_due(true, 0.0, 1.0), "0 minutes is never");
    r.begin_auto_round(&[1, 2], 1_000.0);
    assert!(!r.round_due(true, 5.0, 1_000.0 + 299_999.0));
    assert!(r.round_due(true, 5.0, 1_000.0 + 300_000.0));
    assert_eq!(r.round(), [1, 2]);
    assert!(r.queue.iter().all(|(_, quiet)| *quiet));
}

#[test]
fn the_line_goes_in_order_and_the_first_waits_when_blocked() {
    let mut r = RoundDriver::default();
    assert_eq!(r.next(|_| false), Next::Idle);
    r.begin_round(&[3, 4], false);
    assert_eq!(r.next(|a| a == 3), Next::Wait);
    assert_eq!(r.round(), [3, 4], "a wait takes nobody out of line");
    assert_eq!(r.next(|_| false), Next::Start { account: 3, quiet: false });
    assert_eq!(r.next(|_| false), Next::Start { account: 4, quiet: false });
    assert_eq!(r.next(|_| false), Next::Idle);
}

#[test]
fn looks_are_paced_and_off_when_auto_sync_is_off() {
    let mut r = RoundDriver::default();
    assert!(!r.push_look_due(false, 10_000.0));
    assert!(r.push_look_due(true, 10_000.0));
    assert!(!r.push_look_due(true, 11_499.0));
    assert!(r.push_look_due(true, 11_500.0));
    assert!(!r.run_look_due(true, 14_499.0));
    assert!(r.run_look_due(true, 14_500.0));
    assert!(!r.run_look_due(false, 99_000.0));
}

#[test]
fn what_waits_starts_a_run_when_it_changed_or_has_waited_long_and_not_for_a_refusal() {
    let mut r = RoundDriver::default();
    assert_eq!(r.pushes_due(&[w(1, 0), w(2, 2)], 100.0), [2]);
    r.note_pushed(2, 2, 100.0);
    assert!(r.pushes_due(&[w(2, 2)], 200.0).is_empty(), "a refusal keeps waiting");
    assert_eq!(r.pushes_due(&[w(2, 3)], 200.0), [2], "more of it");
    assert_eq!(r.pushes_due(&[w(2, 2)], 100.0 + 300_001.0), [2], "waited long");
    r.push_again_soon(2, 2, 500.0);
    assert_eq!(r.pushes_due(&[w(2, 2)], 600.0), [2]);
    r.push_again_soon(5, 0, 500.0);
    assert!(!r.push_seen.contains_key(&5), "nothing waits, nothing noted");
}

#[test]
fn queued_pushes_make_a_round_and_only_tell_the_server() {
    let mut r = RoundDriver::default();
    assert_eq!(r.queue_pushes(&[]), None);
    assert_eq!(r.queue_pushes(&[7, 8]), Some(vec![7, 8]));
    assert!(r.push_only_next.contains(&7) && r.push_only_next.contains(&8));
    assert!(r.skip(7));
    assert!(!r.skip(7));
    assert!(!r.push_only_next.contains(&7));
}

#[test]
fn put_back_keeps_one_place_and_forgetting_accounts_clears_all_of_theirs() {
    let mut r = RoundDriver::default();
    r.begin_round(&[1, 2], true);
    r.put_back(2, false);
    assert_eq!(r.round(), [1, 2], "already in line");
    r.put_back(3, false);
    assert_eq!(r.queue.front(), Some(&(3, false)));
    r.inbox_queue.push_back((1, true));
    r.note_pushed(1, 1, 1.0);
    r.push_only_next.insert(1);
    r.forget_accounts(&[1, 3].into_iter().collect());
    assert_eq!(r.round(), [2]);
    assert!(r.inbox_queue.is_empty() && r.push_seen.is_empty() && r.push_only_next.is_empty());
}

#[test]
fn a_new_mailbox_keeps_the_clock_and_drops_the_rest() {
    let mut r = RoundDriver::default();
    r.begin_auto_round(&[1], 9.0);
    r.push_checked = 4.0;
    r.note_pushed(1, 1, 1.0);
    r.reset();
    assert_eq!((r.last_auto, r.push_checked), (9.0, 4.0));
    assert!(r.queue.is_empty() && r.push_seen.is_empty());
}

#[test]
fn junk_times_do_not_panic() {
    let mut r = RoundDriver::default();
    for t in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        let _ = (r.push_look_due(true, t), r.run_look_due(true, t), r.round_due(true, 5.0, t), r.pushes_due(&[w(1, 1)], t));
    }
}
