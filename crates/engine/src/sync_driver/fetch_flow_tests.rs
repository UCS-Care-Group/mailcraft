// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::sync::{GraphFetchResult, ServerMessage};
use mailcraft_model::{AccountKind, FolderKind, Id, Mailbox, Message};
use serde_json::json;

use super::fetch_flow::*;
use crate::Session;

/// A session with `n` unfetched messages of one account.
fn window(n: usize) -> (Session, Vec<Id>) {
    let mut mailbox = Mailbox::with_local_account("", "");
    let account = mailbox.accounts[0].id;
    let inbox = mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let ids = (0..n)
        .map(|i| {
            let server = ServerMessage { id: format!("M{i}"), folder: "in".into(), ..Default::default() };
            mailbox.add_message(Message { account, folder: inbox, unfetched: true, server: Some(server), ..Default::default() }).unwrap()
        })
        .collect();
    (Session::new(mailbox), ids)
}

#[test]
fn a_command_that_needs_its_message_whole_waits_and_runs_once_it_is_here() {
    let (mut s, ids) = window(1);
    let plan = s.fetch_first_plan("mail.reply", &json!({"id": ids[0]})).unwrap().unwrap();
    assert_eq!((plan.first, plan.messages.clone()), (ids[0], vec![ids[0]]));
    assert_eq!(s.fetch_waiting(plan), FirstNote::One);
    let opening = s.openings();
    // Not here after the answer: it says so and goes nowhere.
    let answered = s.fetch_now_answered(&ids, opening, GraphFetchResult::default());
    assert!(matches!(answered.next, NowNext::Note(NowNote::NotHere)));
    // Waiting again, the message comes by other means: it runs.
    let plan = s.fetch_first_plan("mail.reply", &json!({"id": ids[0]})).unwrap().unwrap();
    s.fetch_waiting(plan);
    s.mailbox.message_mut(ids[0]).unwrap().unfetched = false;
    let answered = s.fetch_now_answered(&ids, opening, GraphFetchResult::default());
    let NowNext::Run(waiting) = answered.next else { panic!("it runs") };
    assert_eq!((waiting.command.as_str(), waiting.tried), ("mail.reply", None));
    assert!(s.driver.fetch.then.is_none());
}

#[test]
fn an_answer_for_another_mailbox_is_nobodys_and_the_command_that_waited_is_let_go_of() {
    let (mut s, ids) = window(1);
    let plan = s.fetch_first_plan("mail.reply", &json!({"id": ids[0]})).unwrap().unwrap();
    s.fetch_waiting(plan);
    let answered = s.fetch_now_answered(&ids, s.openings() + 1, GraphFetchResult::default());
    assert!(matches!(answered.next, NowNext::Nothing) && answered.refused.is_none());
    s.driver.fetch.forget();
    assert!(s.driver.fetch.then.is_none());
}

#[test]
fn a_command_that_needs_nothing_fetched_waits_for_nothing() {
    let (mut s, ids) = window(1);
    s.mailbox.message_mut(ids[0]).unwrap().unfetched = false;
    assert!(s.fetch_first_plan("mail.reply", &json!({"id": ids[0]})).unwrap().is_none());
    assert!(s.fetch_first_plan("mail.flag", &json!({"id": ids[0]})).unwrap().is_none());
}

#[test]
fn another_command_taking_the_place_is_said_and_a_held_one_is_let_go_of() {
    let (mut s, ids) = window(2);
    let first = s.fetch_first_plan("mail.reply", &json!({"id": ids[0]})).unwrap().unwrap();
    s.fetch_waiting(first);
    let held = s.driver.fetch.then.clone().unwrap();
    s.driver.fetch.then = None;
    s.fetch_hold(held);
    let second = s.fetch_first_plan("mail.reply", &json!({"id": ids[1]})).unwrap().unwrap();
    assert_eq!(s.fetch_waiting(second), FirstNote::HeldCalledOff);
    assert!(s.fetch_take_held().is_none());
}

#[test]
fn a_held_command_comes_back_in_its_own_mailbox_only() {
    let (mut s, ids) = window(1);
    let plan = s.fetch_first_plan("mail.reply", &json!({"id": ids[0]})).unwrap().unwrap();
    s.fetch_waiting(plan);
    let waiting = s.driver.fetch.then.take().unwrap();
    s.fetch_hold(waiting.clone());
    assert_eq!(s.fetch_take_held(), Some(waiting.clone()));
    s.fetch_hold(Waiting { opening: s.openings() + 1, ..waiting });
    assert!(s.fetch_take_held().is_none());
}

#[test]
fn the_message_shown_is_asked_for_once_and_after_the_batch_under_way() {
    let (mut s, ids) = window(2);
    s.sel.messages = vec![ids[0]];
    let account = s.mailbox.message(ids[0]).unwrap().account;
    assert_eq!(s.fetch_shown(false), Some((account, ids[0])));
    assert_eq!(s.fetch_shown(false), None, "not asked again while it stays open");
    s.sel.messages = vec![ids[1]];
    assert_eq!(s.fetch_shown(true), None, "after the batch under way");
    assert!(s.fetch_after_batch(ids[1], true));
    assert_eq!(s.fetch_shown(false), Some((account, ids[1])));
    s.fetch_replaced(&[ids[1]], &[ids[0]]);
    assert_eq!(s.driver.fetch.opened, None, "asked for again once the others are here");
}

#[test]
fn the_background_looks_at_the_accounts_that_are_not_waiting_and_rests_when_stopped() {
    let (mut s, _) = window(1);
    let account = s.mailbox.accounts[0].id;
    let all = |_: AccountKind| true;
    assert_eq!(s.fetch_background_accounts(all, 0.0), vec![account]);
    assert!(s.fetch_accounts(|_| false).is_empty());
    s.driver.fetch.stuck.insert(account, (1, 10_000.0));
    assert!(s.fetch_background_accounts(all, 5_000.0).is_empty());
    assert!(s.fetch_background_due(5_000.0));
    s.driver.fetch.found_nothing(5_000.0);
    assert!(!s.fetch_background_due(5_001.0));
    s.sync_progress.full_paused = true;
    assert!(!s.fetch_background_due(1e12));
}
