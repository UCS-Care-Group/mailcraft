// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashSet;

use mailcraft_model::Id;
use mailcraft_model::auto_replies::{AutoReplies, AutoReplyStatus};
use serde_json::json;

use super::auto_replies::*;
use super::{Busy, Permission, fingerprint};
use crate::Session;

const NOW: i64 = 1_790_000_000;
const MINUTE: f64 = 60_000.0;

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap();
    let id = added["id"].as_u64().unwrap();
    (s, id)
}

fn facts() -> RepliesFacts {
    RepliesFacts { reachable: true, running: false, busy: Busy::default() }
}

fn tokens(_: Id) -> Option<String> {
    Some("refresh".into())
}

fn away() -> AutoReplies {
    AutoReplies { status: Some(AutoReplyStatus::AlwaysEnabled), internal_reply_message: Some("<p>Away.</p>".into()), ..Default::default() }
}

fn answer(s: &mut Session, account: Id, now: f64, came: &RepliesCame<'_>) -> RepliesAnswer {
    s.auto_replies_answered(account, fingerprint("refresh"), now, came).unwrap()
}

#[test]
fn a_read_is_due_at_first_then_every_quarter_of_an_hour() {
    let (mut s, a) = session();
    assert_eq!(s.auto_replies_due(&facts(), tokens, 0.0).plans, vec![RepliesPlan { account: a, change: None }]);
    s.auto_replies_started(a, 0.0);
    assert!(s.auto_replies_due(&facts(), tokens, 14.0 * MINUTE).plans.is_empty());
    assert_eq!(s.auto_replies_due(&facts(), tokens, 15.0 * MINUTE).plans.len(), 1);
}

#[test]
fn nothing_is_due_while_the_host_cannot_or_something_else_goes_first() {
    let (mut s, _) = session();
    assert!(s.auto_replies_due(&RepliesFacts { reachable: false, ..facts() }, tokens, 0.0).plans.is_empty());
    assert!(s.auto_replies_due(&RepliesFacts { running: true, ..facts() }, tokens, 0.0).plans.is_empty());
    for busy in [
        Busy { signing_in: true, ..Default::default() },
        Busy { mail_run: true, ..Default::default() },
        Busy { graph: true, ..Default::default() },
        Busy { calendar: true, ..Default::default() },
        Busy { tasks: true, ..Default::default() },
        Busy { rules: true, ..Default::default() },
    ] {
        assert!(s.auto_replies_due(&RepliesFacts { busy, ..facts() }, tokens, 0.0).plans.is_empty(), "{busy:?}");
    }
    // A Send/Receive that is only waiting its turn doesn't hold it back.
    assert_eq!(s.auto_replies_due(&RepliesFacts { busy: Busy { queued: true, ..Default::default() }, ..facts() }, tokens, 0.0).plans.len(), 1);
}

#[test]
fn reads_wait_for_the_host_switch_but_a_change_the_owner_made_goes() {
    let (mut s, a) = session();
    s.driver.set_auto_sync(false);
    assert!(s.auto_replies_due(&facts(), tokens, 0.0).plans.is_empty());
    assert_eq!(s.auto_replies_change(a, away()), Change::Queued);
    let look = s.auto_replies_due(&facts(), tokens, 0.0);
    assert_eq!(look.plans, vec![RepliesPlan { account: a, change: Some(away()) }]);
    assert!(s.auto_replies_due(&facts(), tokens, 0.0).plans.is_empty(), "the change goes once");
}

#[test]
fn working_offline_drops_what_waited_and_says_so_once() {
    let (mut s, a) = session();
    assert_eq!(s.auto_replies_change(a, away()), Change::Queued);
    s.prefs.offline = true;
    let look = s.auto_replies_due(&facts(), tokens, 0.0);
    assert!(look.dropped && look.plans.is_empty());
    assert!(!s.auto_replies_due(&facts(), tokens, 1.0).dropped);
    assert_eq!(s.auto_replies_change(a, away()), Change::Offline);
    assert!(s.driver.auto_replies.waiting.is_none());
}

#[test]
fn a_read_that_keeps_failing_backs_off_and_a_success_ends_it() {
    let (mut s, a) = session();
    let failed = RepliesCame { error: "boom", ..Default::default() };
    let mut at = 0.0;
    for wait in [2.0, 4.0, 8.0] {
        assert_eq!(s.auto_replies_due(&facts(), tokens, at).plans.len(), 1, "due at {at}");
        s.auto_replies_started(a, at);
        let said = answer(&mut s, a, at, &failed);
        assert_eq!(said.trouble, Some(Trouble::Failed("boom".into())));
        at = s.driver.auto_replies.checked_at(a);
        assert!(s.auto_replies_due(&facts(), tokens, at + wait * MINUTE - 1.0).plans.is_empty(), "{wait} minutes");
        at += wait * MINUTE;
    }
    let ok = answer(&mut s, a, at, &RepliesCame { setting: Some(&away()), ..Default::default() });
    assert_eq!(ok.trouble, None);
    assert!(!s.driver.auto_replies.trouble.contains_key(&a));
    assert!(s.auto_replies.contains_key(&a), "what the server has was taken in");
    assert!(s.auto_replies_due(&facts(), tokens, at + 14.0 * MINUTE).plans.is_empty());
}

#[test]
fn a_refused_sign_in_waits_for_a_new_one() {
    let (mut s, a) = session();
    let said = answer(&mut s, a, 0.0, &RepliesCame { sign_in_refused: true, ..Default::default() });
    assert_eq!(said.trouble, Some(Trouble::SignInOver(fingerprint("refresh"))));
    assert!(s.auto_replies_blocked(a, Some("refresh")) && s.auto_replies_blocked(a, None));
    assert!(s.auto_replies_due(&facts(), tokens, 1e9).plans.is_empty());
    assert!(!s.auto_replies_blocked(a, Some("another")));
    let again = |_: Id| Some("another".to_string());
    assert_eq!(s.auto_replies_due(&facts(), again, 1e9).plans.len(), 1);
    assert!(!s.driver.auto_replies.trouble.contains_key(&a), "a read finds out where they stand");
    // (A renewed token is not a refusal.)
    let renewed = answer(&mut s, a, 0.0, &RepliesCame { sign_in_refused: true, new_tokens: true, ..Default::default() });
    assert_eq!(renewed.trouble, None);
}

#[test]
fn a_sign_in_that_reads_no_settings_waits_until_the_owner_signs_in_on_the_page() {
    let (mut s, a) = session();
    let said = answer(&mut s, a, 0.0, &RepliesCame { consent_needed: true, ..Default::default() });
    assert_eq!(said.trouble, Some(Trouble::SignInToRead));
    assert!(s.auto_replies_blocked(a, Some("anything")));
    s.driver.auto_replies.signed_in_on_page(a);
    assert!(!s.auto_replies_blocked(a, Some("anything")) && !s.driver.auto_replies.trouble.contains_key(&a));
    assert_eq!(s.auto_replies_due(&facts(), tokens, 0.0).plans.len(), 1);
    answer(&mut s, a, 0.0, &RepliesCame { consent_needed: true, ..Default::default() });
    s.driver.auto_replies.sign_in_again(a);
    assert!(s.driver.auto_replies.trouble.is_empty());
}

#[test]
fn a_change_that_finds_the_permission_missing_asks_the_owner_and_goes_once_he_allows_it() {
    let (mut s, a) = session();
    let said = answer(&mut s, a, 0.0, &RepliesCame { write_consent_needed: true, ..Default::default() });
    assert!(said.ask && said.trouble.is_none());
    assert_eq!(s.driver.auto_replies.perm.get(&a), Some(&Permission::Needed));
    // Another change meanwhile: the owner is to be asked, and the change held when he is.
    assert_eq!(s.auto_replies_change(a, away()), Change::MustAsk);
    assert!(s.driver.auto_replies.waiting.is_none());
    s.driver.auto_replies.asking(a);
    s.driver.auto_replies.hold(a, away());
    assert_eq!(s.auto_replies_change(a, AutoReplies::default()), Change::Held);
    assert!(s.driver.auto_replies.allowed(a), "a change waited");
    assert!(!s.driver.auto_replies.perm.contains_key(&a));
    assert_eq!(s.auto_replies_due(&facts(), tokens, 0.0).plans, vec![RepliesPlan { account: a, change: Some(AutoReplies::default()) }]);
    answer(&mut s, a, 0.0, &RepliesCame { changed: true, ..Default::default() });
    assert_eq!(s.driver.auto_replies.perm.get(&a), Some(&Permission::Given));
}

#[test]
fn a_question_that_went_away_unanswered_drops_the_change_that_waited_for_it() {
    let (mut s, a) = session();
    s.driver.auto_replies.asking(a);
    s.driver.auto_replies.hold(a, away());
    assert!(s.driver.auto_replies.unanswered(true).is_empty(), "still on screen");
    assert_eq!(s.driver.auto_replies.unanswered(false), vec![a]);
    assert_eq!(s.driver.auto_replies.perm.get(&a), Some(&Permission::Declined));
    assert!(s.driver.auto_replies.held.is_empty());
    assert!(s.driver.auto_replies.unanswered(false).is_empty(), "nothing is left to say");
    // A no from the page: the same standing, with the change dropped.
    s.driver.auto_replies.hold(a, away());
    s.driver.auto_replies.answered_no(a, Permission::Refused("no".into()));
    assert!(s.driver.auto_replies.drop_held(a) && !s.driver.auto_replies.drop_held(a));
    s.driver.auto_replies.allowed_for_rules(a);
    assert!(s.driver.auto_replies.perm.is_empty(), "the rules' permission is the same one");
}

#[test]
fn an_organisation_that_keeps_them_is_not_asked_and_the_change_still_goes() {
    let (mut s, a) = session();
    answer(&mut s, a, 0.0, &RepliesCame { write_denied: true, ..Default::default() });
    assert_eq!(s.driver.auto_replies.perm.get(&a), Some(&Permission::Denied));
    assert_eq!(s.auto_replies_change(a, away()), Change::Queued);
}

#[test]
fn an_answer_for_an_account_removed_meanwhile_changes_nothing() {
    let (mut s, a) = session();
    s.run("account.remove", &json!({"id": a})).unwrap();
    let before = format!("{:?}", s.driver);
    assert!(s.auto_replies_answered(a, 1, 0.0, &RepliesCame { write_consent_needed: true, ..Default::default() }).is_none());
    assert_eq!(format!("{:?}", s.driver), before);
    assert!(s.driver.auto_replies.perm.is_empty());
}

#[test]
fn removing_an_account_forgets_everything_kept_for_it() {
    let (mut s, a) = session();
    s.driver.auto_replies.asking(a);
    s.driver.auto_replies.hold(a, away());
    s.driver.auto_replies.waiting = Some((a, away()));
    s.driver.auto_replies.trouble.insert(a, Trouble::SignInToRead);
    s.auto_replies_started(a, 1.0);
    s.driver.auto_replies.forget_accounts(&HashSet::from([a]));
    assert_eq!(format!("{:?}", s.driver.auto_replies), format!("{:?}", AutoRepliesDriver::default()));
}
