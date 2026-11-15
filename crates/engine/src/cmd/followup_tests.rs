// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Follow-up reminders (#948): at a time, and "if nobody replies"; the settle run that raises them
//! once (also late, after a restart), cancels them on a reply from a recipient, and ends them with
//! the flag; Snooze and Dismiss; undo; the composer's plan; what an agent may do; hostile
//! parameters.

use mailcraft_model::{FlagStatus, FolderKind, Id, Mailbox, Message};
use serde_json::{Value, json};

use crate::agent::{Class, Verdict, check};
use crate::{Clock, Session};

const NOON: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

struct World {
    s: Session,
    account: Id,
    inbox: Id,
    sent: Id,
    /// A message received.
    got: Id,
    /// A message the owner sent to bob@ and carol@.
    mine: Id,
}

fn at(s: &mut Session, now: i64) {
    s.clock = Clock { now: Some(now), offset: Some(0), zone: None };
}

fn world() -> World {
    let mut s = Session::new(Mailbox::with_local_account("Ann", "ann@example.org"));
    at(&mut s, NOON);
    let account = s.mailbox.accounts[0].id;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let sent = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    let got = s
        .mailbox
        .add_message(Message {
            account,
            folder: inbox,
            subject: "Kickoff".into(),
            message_id: "got@example.org".into(),
            from: mailcraft_model::Address::new("Dan", "dan@example.org"),
            read: true,
            ..Default::default()
        })
        .unwrap();
    let mine = s
        .mailbox
        .add_message(Message {
            account,
            folder: sent,
            subject: "Budget".into(),
            message_id: "mine@example.org".into(),
            from: mailcraft_model::Address::new("Ann", "ann@example.org"),
            to: vec![mailcraft_model::Address::new("Bob", "bob@example.org")],
            cc: vec![mailcraft_model::Address::new("Carol", "Carol@Example.org")],
            read: true,
            ..Default::default()
        })
        .unwrap();
    World { s, account, inbox, sent, got, mine }
}

impl World {
    fn m(&self, id: Id) -> &Message {
        self.s.mailbox.message(id).unwrap()
    }

    /// Mail arriving in the Inbox that answers `mine`, from `who`.
    fn reply_from(&mut self, who: &str) -> Id {
        let n = self.s.mailbox.messages.len();
        self.s
            .mailbox
            .add_message(Message {
                account: self.account,
                folder: self.inbox,
                subject: "Re: Budget".into(),
                message_id: format!("reply{n}@example.org"),
                in_reply_to: "mine@example.org".into(),
                references: vec!["mine@example.org".into()],
                from: mailcraft_model::Address::new("", who),
                ..Default::default()
            })
            .unwrap()
    }

    /// Mail from `who` answering the message `of` (by its Message-ID).
    fn reply_to(&mut self, of: Id, who: &str) -> Id {
        let (mid, account) = (self.m(of).message_id.clone(), self.m(of).account);
        let n = self.s.mailbox.messages.len();
        self.s
            .mailbox
            .add_message(Message {
                account,
                folder: self.inbox,
                subject: "Re".into(),
                message_id: format!("rt{n}@example.org"),
                in_reply_to: mid,
                from: mailcraft_model::Address::new("", who),
                ..Default::default()
            })
            .unwrap()
    }

    fn settle(&mut self, now: i64) -> Value {
        at(&mut self.s, now);
        self.s.run("mail.settleReminders", &json!({})).unwrap()
    }
}

/// The mailbox written to its file and read back, as a restart does.
fn reopen(w: &mut World) {
    let bytes = serde_json::to_vec(&w.s.mailbox).unwrap();
    w.s.mailbox = mailcraft_model::from_json(&bytes, 1_000_000).unwrap();
}

#[test]
fn a_reminder_flags_the_message_and_is_raised_at_its_time() {
    let mut w = world();
    let r = w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 3_600})).unwrap();
    assert_eq!(r["set"], 1);
    assert_eq!((w.m(w.got).flag, w.m(w.got).flag_due), (FlagStatus::Flagged, Some(NOON + 3_600)));
    // Not yet: nothing is raised, nothing shows.
    assert_eq!(w.settle(NOON + 3_599)["raised"], json!([]));
    assert_eq!(w.s.run("mail.dueReminders", &json!({})).unwrap()["reminders"], json!([]));
    // At its time.
    assert_eq!(w.settle(NOON + 3_600)["raised"], json!([w.got]));
    let due = w.s.run("mail.dueReminders", &json!({})).unwrap();
    assert_eq!(due["reminders"][0]["id"], w.got);
    assert_eq!(due["reminders"][0]["subject"], "Kickoff");
}

#[test]
fn it_is_raised_once_and_dismissing_ends_it() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    assert_eq!(w.settle(NOON + 100)["raised"], json!([w.got]));
    let raised = w.m(w.got).follow_up.unwrap().raised;
    // Later runs raise nothing more, and keep when it was.
    assert_eq!(w.settle(NOON + 200)["raised"], json!([]));
    assert_eq!(w.m(w.got).follow_up.unwrap().raised, raised);
    assert_eq!(w.s.mailbox.due_follow_ups(NOON + 200), vec![w.got]);
    assert_eq!(w.s.run("mail.dismissReminder", &json!({"ids": [w.got]})).unwrap()["dismissed"], 1);
    assert!(w.s.mailbox.due_follow_ups(NOON + 300).is_empty());
    assert_eq!(w.settle(NOON + 400)["raised"], json!([]));
    // Dismissed, it stays flagged until the flag is cleared.
    assert_eq!(w.m(w.got).flag, FlagStatus::Flagged);
}

#[test]
fn one_that_fell_due_while_closed_is_raised_once_at_the_next_start() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 3_600})).unwrap();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 2, "ifNoReply": true})).unwrap();
    reopen(&mut w);
    // Opened two weeks later.
    let r = w.settle(NOON + 14 * DAY);
    let mut raised: Vec<u64> = serde_json::from_value(r["raised"].clone()).unwrap();
    raised.sort_unstable();
    assert_eq!(raised, [w.got, w.mine]);
    assert_eq!(w.m(w.mine).flag, FlagStatus::Flagged);
    // Saved and opened again: not raised again, still showing until dismissed.
    reopen(&mut w);
    assert_eq!(w.settle(NOON + 14 * DAY + 60)["raised"], json!([]));
    assert_eq!(w.s.mailbox.due_follow_ups(NOON + 14 * DAY + 60).len(), 2);
}

#[test]
fn snooze_puts_it_off_and_survives_a_restart() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    w.settle(NOON + 60);
    at(&mut w.s, NOON + 120);
    assert_eq!(w.s.run("mail.postponeReminder", &json!({"ids": [w.got], "minutes": 10})).unwrap()["postponed"], 1);
    assert!(w.s.mailbox.due_follow_ups(NOON + 121).is_empty());
    reopen(&mut w);
    assert!(w.s.mailbox.due_follow_ups(NOON + 120 + 599).is_empty());
    // Back when the time is up, and not raised a second time for it.
    assert_eq!(w.s.mailbox.due_follow_ups(NOON + 120 + 600), vec![w.got]);
    assert_eq!(w.settle(NOON + 800)["raised"], json!([]));
    // Nothing to put off when nothing is showing.
    w.s.run("mail.dismissReminder", &json!({"ids": [w.got]})).unwrap();
    assert!(w.s.run("mail.postponeReminder", &json!({"ids": [w.got], "minutes": 5})).is_err());
}

#[test]
fn if_nobody_replies_flags_the_message_only_when_the_time_comes() {
    let mut w = world();
    let r = w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 3, "ifNoReply": true})).unwrap();
    assert_eq!(r["at"], NOON + 3 * DAY);
    // Not flagged while it waits.
    assert_eq!(w.m(w.mine).flag, FlagStatus::None);
    assert_eq!(w.settle(NOON + 3 * DAY - 1)["raised"], json!([]));
    assert_eq!(w.m(w.mine).flag, FlagStatus::None);
    assert_eq!(w.settle(NOON + 3 * DAY)["raised"], json!([w.mine]));
    assert_eq!(w.m(w.mine).flag, FlagStatus::Flagged);
    assert_eq!(w.s.mailbox.due_follow_ups(NOON + 3 * DAY), vec![w.mine]);
}

#[test]
fn a_reply_from_a_recipient_cancels_it() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 2, "ifNoReply": true})).unwrap();
    // Carol (Cc, in other letter case) answers on day one.
    w.reply_from("carol@example.org");
    let r = w.settle(NOON + 2 * DAY);
    assert_eq!((r["cancelled"].clone(), r["raised"].clone()), (json!([w.mine]), json!([])));
    // Cancelled, not hidden: nothing is kept, and the message is not flagged.
    assert!(w.m(w.mine).follow_up.is_none());
    assert_eq!(w.m(w.mine).flag, FlagStatus::None);
    assert!(!w.s.mailbox.has_follow_ups());
}

#[test]
fn a_reply_that_arrives_after_it_was_raised_changes_nothing() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    w.settle(NOON + DAY);
    w.reply_from("bob@example.org");
    assert_eq!(w.settle(NOON + 2 * DAY), json!({"raised": [], "cancelled": [], "ended": []}));
    assert!(w.m(w.mine).follow_up.is_some());
}

#[test]
fn a_reply_from_somebody_who_is_not_a_recipient_does_not_cancel_it() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    w.reply_from("dan@example.org");
    // Nor does the owner's own address (a copy to themselves, a list's echo).
    w.reply_from("ann@example.org");
    assert_eq!(w.settle(NOON + DAY)["raised"], json!([w.mine]));
}

#[test]
fn a_reply_from_before_the_reminder_was_set_does_not_cancel_it() {
    let mut w = world();
    w.reply_from("bob@example.org");
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    assert_eq!(w.settle(NOON + DAY)["raised"], json!([w.mine]));
}

#[test]
fn mail_of_another_conversation_does_not_cancel_it() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    // Bob writes about something else, linked to nothing.
    w.s.mailbox
        .add_message(Message {
            account: w.account,
            folder: w.inbox,
            subject: "Lunch?".into(),
            message_id: "lunch@example.org".into(),
            from: mailcraft_model::Address::new("", "bob@example.org"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(w.settle(NOON + DAY)["raised"], json!([w.mine]));
}

#[test]
fn if_nobody_replies_is_for_mail_you_sent_and_nothing_is_set_when_one_is_refused() {
    let mut w = world();
    let r = w.s.run("mail.reminder", &json!({"ids": [w.mine, w.got], "days": 1, "ifNoReply": true}));
    assert!(r.is_err());
    assert!(w.m(w.mine).follow_up.is_none() && w.m(w.got).follow_up.is_none());
    // A time in the past is refused too.
    assert!(w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON})).is_err());
    assert!(w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON - 5})).is_err());
    // A draft of the owner's isn't sent mail.
    let draft = w.s.mailbox.add_message(Message { account: w.account, folder: w.sent, draft: true, ..Default::default() }).unwrap();
    assert!(w.s.run("mail.reminder", &json!({"ids": [draft], "days": 1, "ifNoReply": true})).is_err());
}

#[test]
fn clearing_the_flag_or_completing_the_message_removes_the_reminder() {
    let mut w = world();
    for (command, flag) in [("mail.clearFlag", FlagStatus::None), ("mail.complete", FlagStatus::Complete), ("mail.toggleFlag", FlagStatus::None)] {
        w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
        w.s.run(command, &json!({"ids": [w.got]})).unwrap();
        assert_eq!((w.m(w.got).flag, w.m(w.got).follow_up), (flag, None), "{command}");
    }
    // Also the one still waiting on a message that wasn't flagged.
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 2, "ifNoReply": true})).unwrap();
    w.s.run("mail.clearFlag", &json!({"ids": [w.mine]})).unwrap();
    assert!(w.m(w.mine).follow_up.is_none());
    // Remove Reminder keeps the flag.
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    assert_eq!(w.s.run("mail.clearReminder", &json!({"ids": [w.got]})).unwrap()["cleared"], 1);
    assert_eq!((w.m(w.got).flag, w.m(w.got).follow_up), (FlagStatus::Flagged, None));
}

#[test]
fn a_flag_cleared_elsewhere_ends_a_reminder_that_is_set_or_raised() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    // A server's news: the flag of the first is gone.
    w.s.mailbox.message_mut(w.got).unwrap().flag = FlagStatus::None;
    let r = w.settle(NOON + 30);
    assert_eq!(r["ended"], json!([w.got]));
    assert!(w.m(w.got).follow_up.is_none());
    // The waiting "if nobody replies" one doesn't need the flag until it is raised.
    assert!(w.m(w.mine).follow_up.is_some());
    w.settle(NOON + DAY);
    w.s.mailbox.message_mut(w.mine).unwrap().flag = FlagStatus::Complete;
    assert_eq!(w.settle(NOON + DAY + 1)["ended"], json!([w.mine]));
}

#[test]
fn adding_and_removing_a_reminder_are_steps_of_the_history() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    w.s.run("mail.clearReminder", &json!({"ids": [w.got]})).unwrap();
    assert!(w.m(w.got).follow_up.is_none());
    w.s.run("edit.undo", &json!({})).unwrap();
    assert!(w.m(w.got).follow_up.is_some());
    w.s.run("edit.undo", &json!({})).unwrap();
    assert_eq!((w.m(w.got).follow_up, w.m(w.got).flag, w.m(w.got).flag_due), (None, FlagStatus::None, None));
    w.s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(w.m(w.got).flag, FlagStatus::Flagged);
}

#[test]
fn a_copy_has_no_reminder_and_a_draft_does_not_fire() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    let copy = w.s.mailbox.copy_message_marked(w.got, w.sent, 0).unwrap();
    assert!(w.m(copy).follow_up.is_none());
    let draft = w.s.mailbox.add_message(Message { account: w.account, folder: w.sent, draft: true, ..Default::default() }).unwrap();
    w.s.mailbox.message_mut(draft).unwrap().follow_up = Some(mailcraft_model::FollowUp::new(NOON + 1, true, 0));
    assert_eq!(w.settle(NOON + DAY)["raised"], json!([w.got]));
}

fn write_to_bob(w: &mut World) {
    w.s.run("mail.new", &json!({"to": "bob@example.org", "subject": "Plan", "body": "Hello"})).unwrap();
}

#[test]
fn the_composer_asks_for_a_reminder_that_the_sent_message_carries() {
    let mut w = world();
    write_to_bob(&mut w);
    assert_eq!(w.s.run("compose.followUp", &json!({"days": 5})).unwrap()["followUp"], json!({"days": 5}));
    assert_eq!(w.s.compose.as_ref().unwrap().follow_up, Some(super::followup::FollowUpPlan::Days(5)));
    let id = w.s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let f = w.m(id).follow_up.unwrap();
    assert_eq!((f.at, f.if_no_reply, f.days), (NOON + 5 * DAY, true, 5));
    assert!(f.after_id < id);
    assert_eq!(w.m(id).flag, FlagStatus::None);
    // Bob answers: cancelled, like any other.
    let n = w.s.mailbox.messages.len();
    w.s.mailbox
        .add_message(Message {
            account: w.account,
            folder: w.inbox,
            subject: "Re: Plan".into(),
            message_id: format!("r{n}@example.org"),
            in_reply_to: w.m(id).message_id.clone(),
            from: mailcraft_model::Address::new("", "bob@example.org"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(w.settle(NOON + 5 * DAY)["cancelled"], json!([id]));
}

#[test]
fn the_composer_plan_is_cleared_kept_in_a_draft_and_counted_from_sending() {
    let mut w = world();
    write_to_bob(&mut w);
    w.s.run("compose.followUp", &json!({"days": 2})).unwrap();
    w.s.run("compose.followUp", &json!({"clear": true})).unwrap();
    assert!(w.s.compose.as_ref().unwrap().follow_up.is_none());
    w.s.run("compose.followUp", &json!({"days": 2})).unwrap();
    let draft = w.s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert_eq!(w.m(draft).follow_up.map(|f| (f.days, f.if_no_reply)), Some((2, true)));
    w.s.run("compose.close", &json!({"discard": false})).ok();
    // Continued a week later and sent: two days from then.
    at(&mut w.s, NOON + 7 * DAY);
    w.s.run("mail.editDraft", &json!({"ids": [draft]})).unwrap();
    assert_eq!(w.s.compose.as_ref().unwrap().follow_up, Some(super::followup::FollowUpPlan::Days(2)));
    let id = w.s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(w.m(id).follow_up.unwrap().at, NOON + 9 * DAY);
    // Nothing set, nothing carried.
    write_to_bob(&mut w);
    let id = w.s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(w.m(id).follow_up.is_none());
}

#[test]
fn an_agent_sets_and_clears_reminders_but_never_dismisses_or_snoozes_one() {
    let w = world();
    let none: &[Class] = &[];
    for command in ["mail.reminder", "mail.clearReminder", "mail.settleReminders", "mail.dueReminders", "compose.followUp"] {
        assert!(matches!(check(&w.s, command, &json!({}), none), Verdict::Run), "{command}");
    }
    for command in ["mail.dismissReminder", "mail.postponeReminder"] {
        assert!(!matches!(check(&w.s, command, &json!({}), none), Verdict::Run), "{command}");
        // Not even with everything allowed in advance.
        let all = [Class::Send, Class::Delete, Class::Settings, Class::Accounts, Class::Mailbox];
        assert!(!matches!(check(&w.s, command, &json!({}), &all), Verdict::Run), "{command}");
    }
}

#[test]
fn hostile_parameters_are_refused_and_change_nothing() {
    let mut w = world();
    let before = w.s.mailbox.clone();
    for p in [
        json!({"ids": [w.got]}),
        json!({"ids": [w.got], "at": null}),
        json!({"ids": [w.got], "at": "x".repeat(10_000)}),
        json!({"ids": [w.got], "at": "9999-12-31T23:59"}),
        json!({"ids": [w.got], "at": 1e300}),
        json!({"ids": [w.got], "at": i64::MIN}),
        json!({"ids": [w.got], "at": NOON + 60, "days": 2}),
        json!({"ids": [w.got], "days": 0}),
        json!({"ids": [w.got], "days": 366}),
        json!({"ids": [w.got], "days": -1}),
        json!({"ids": [w.got], "days": "2"}),
        json!({"ids": [w.got], "days": 1, "ifNoReply": "yes"}),
        json!({"ids": [0, 18446744073709551615u64], "days": 1}),
        json!({"ids": "x", "days": 1}),
    ] {
        assert!(w.s.run("mail.reminder", &p).is_err(), "{p}");
    }
    assert_eq!(w.s.mailbox.messages, before.messages);
    // A time far ahead is brought to five years.
    let r = w.s.run("mail.reminder", &json!({"ids": [w.got], "at": i64::MAX})).unwrap();
    assert_eq!(r["at"], NOON + mailcraft_model::MAX_FOLLOW_UP_AHEAD);
    for p in [json!({"ids": [w.got], "minutes": 0}), json!({"ids": [w.got], "minutes": 10_081}), json!({"ids": [w.got], "minutes": 1e300})] {
        assert!(w.s.run("mail.postponeReminder", &p).is_err(), "{p}");
    }
}

// ---- Review fixes (#948) ----

#[test]
fn undo_of_an_unrelated_step_keeps_a_dismissed_reminder_dismissed() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    w.s.run("mail.markUnread", &json!({"ids": [w.got]})).unwrap();
    w.settle(NOON + 120);
    assert_eq!(w.s.mailbox.due_follow_ups(NOON + 120), vec![w.got]);
    w.s.run("mail.dismissReminder", &json!({"ids": [w.got]})).unwrap();
    w.s.run("edit.undo", &json!({})).unwrap();
    assert!(w.m(w.got).read, "the step before was undone");
    let f = w.m(w.got).follow_up.unwrap();
    assert!(f.dismissed && f.raised.is_some(), "still dismissed, not to be raised or alerted again");
    assert!(w.s.mailbox.due_follow_ups(NOON + 130).is_empty());
    assert_eq!(w.settle(NOON + 200)["raised"], json!([]));
    // A raised one that was put off stays put off, too.
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    w.s.run("mail.markUnread", &json!({"ids": [w.mine]})).unwrap();
    w.settle(NOON + 2 * DAY);
    at(&mut w.s, NOON + 2 * DAY);
    w.s.run("mail.postponeReminder", &json!({"ids": [w.mine], "minutes": 60})).unwrap();
    w.s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(w.m(w.mine).follow_up.unwrap().postponed_until, Some(NOON + 2 * DAY + 3_600));
}

#[test]
fn a_dismissed_reminder_is_over_and_makes_no_work() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    w.settle(NOON + 100);
    assert!(!w.s.mailbox.follow_up_work(NOON + 100), "raised, nothing more to do");
    w.s.run("mail.dismissReminder", &json!({"ids": [w.got]})).unwrap();
    assert!(!w.s.mailbox.has_follow_ups() && !w.s.mailbox.follow_up_work(NOON + 10_000));
    // Nothing to do: settling changes nothing, not even the revision.
    let rev = w.s.rev();
    w.settle(NOON + 20_000);
    assert_eq!(w.s.rev(), rev);
}

#[test]
fn a_reminder_that_is_due_is_work_and_one_that_is_not_is_not() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    assert!(!w.s.mailbox.follow_up_work(NOON + DAY - 1));
    assert!(w.s.mailbox.follow_up_work(NOON + DAY));
    // Waiting in the Outbox: not due, however late.
    let outbox = w.s.mailbox.folder_of_kind(w.account, FolderKind::Outbox).unwrap();
    w.s.mailbox.message_mut(w.mine).unwrap().folder = outbox;
    assert!(!w.s.mailbox.follow_up_work(NOON + 9 * DAY));
}

/// Replies are found by what links them, with nothing built over the account's mail: a thousand
/// messages after the reminder are looked through once, at its time, and not at all before.
#[test]
fn a_big_mailbox_is_looked_through_once_at_the_time_and_a_reply_is_found() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    for i in 0..20_000 {
        w.s.mailbox
            .add_message(Message {
                account: w.account,
                folder: w.inbox,
                subject: format!("Noise {i}"),
                message_id: format!("n{i}@example.org"),
                from: mailcraft_model::Address::new("", "noise@example.org"),
                ..Default::default()
            })
            .unwrap();
    }
    let reply = w.reply_from("bob@example.org");
    // Before its time nothing is looked through (and nothing is copied for a run).
    assert!(!w.s.mailbox.follow_up_work(NOON + DAY - 1));
    let started = std::time::Instant::now();
    assert_eq!(w.settle(NOON + DAY)["cancelled"], json!([w.mine]));
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    assert!(w.s.mailbox.message(reply).is_some());
}

#[test]
fn a_reply_with_another_subject_still_cancels_it_when_its_headers_link_it() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    let r = w.reply_from("bob@example.org");
    w.s.mailbox.message_mut(r).unwrap().subject = "Something else entirely".into();
    assert_eq!(w.settle(NOON + DAY)["cancelled"], json!([w.mine]));
}

#[test]
fn a_reply_to_a_reply_cancels_it_through_the_chain() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    // Dan (not a recipient) answers, and Carol answers Dan, naming only Dan's message.
    let dan = w.reply_from("dan@example.org");
    let dan_id = w.m(dan).message_id.clone();
    let carol = w.reply_from("carol@example.org");
    let c = w.s.mailbox.message_mut(carol).unwrap();
    (c.in_reply_to, c.references) = (dan_id, Vec::new());
    assert_eq!(w.settle(NOON + DAY)["cancelled"], json!([w.mine]));
}

#[test]
fn a_reply_in_another_of_the_owners_accounts_cancels_it() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    let other = w.s.mailbox.add_account(mailcraft_model::Account { name: "ann2".into(), email: "ann2@example.org".into(), ..Default::default() });
    let inbox = w.s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    w.s.mailbox
        .add_message(Message {
            account: other,
            folder: inbox,
            subject: "Re: Budget".into(),
            message_id: "elsewhere@example.org".into(),
            in_reply_to: "mine@example.org".into(),
            from: mailcraft_model::Address::new("", "bob@example.org"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(w.settle(NOON + DAY)["cancelled"], json!([w.mine]));
}

#[test]
fn a_reminder_ends_when_its_message_is_deleted() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    w.s.run("mail.delete", &json!({"ids": [w.got]})).unwrap();
    let r = w.settle(NOON + 120);
    assert_eq!((r["ended"].clone(), r["raised"].clone()), (json!([w.got]), json!([])));
    assert!(w.m(w.got).follow_up.is_none());
}

#[test]
fn follow_up_with_a_new_date_moves_the_reminder_or_ends_it() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    // A later day: the reminder goes to it (end of the working day), and the flag names the same time.
    w.s.run("mail.flag", &json!({"ids": [w.got], "due": "2027-03-01"})).unwrap();
    let m = w.m(w.got);
    let (f, due) = (m.follow_up.unwrap(), m.flag_due.unwrap());
    assert_eq!(f.at, due);
    assert!(f.at > NOON + 60 && f.raised.is_none());
    // A date with no time ahead (the end of today's working day has passed): it ends.
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    w.s.run("mail.flag", &json!({"ids": [w.got], "due": "2001-01-01"})).unwrap();
    assert!(w.m(w.got).follow_up.is_none());
    // "If nobody replies" doesn't follow the flag's date.
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 3, "ifNoReply": true})).unwrap();
    w.s.run("mail.flag", &json!({"ids": [w.mine], "due": "2027-03-01"})).unwrap();
    assert_eq!(w.m(w.mine).follow_up.unwrap().at, NOON + 3 * DAY);
}

#[test]
fn an_at_time_reminder_is_refused_on_a_draft_and_an_if_nobody_replies_plan_is_kept() {
    let mut w = world();
    let drafts = w.s.mailbox.folder_of_kind(w.account, FolderKind::Drafts).unwrap();
    let draft = w.s.mailbox.add_message(Message { account: w.account, folder: drafts, draft: true, ..Default::default() }).unwrap();
    let e = w.s.run("mail.reminder", &json!({"ids": [draft], "at": NOON + 60})).unwrap_err().to_string();
    assert!(e.contains("draft"), "{e}");
    assert!(w.m(draft).follow_up.is_none() && w.m(draft).flag == FlagStatus::None);
}

/// The reminder of a message sent from the composer goes with the message when the server files
/// its copy under another id: kept by Message-ID, counted from the moment it was sent.
fn sent_through_outbox(kind: mailcraft_model::AccountKind) -> (World, Id) {
    let mut w = world();
    w.s.mailbox.account_mut(w.account).unwrap().kind = kind;
    write_to_bob(&mut w);
    w.s.run("compose.followUp", &json!({"days": 4})).unwrap();
    let id = w.s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    (w, id)
}

#[test]
fn a_reminder_follows_a_sent_message_whose_copy_the_server_files_itself() {
    use mailcraft_model::AccountKind::{Exchange as Ews, Jmap, Microsoft};
    use mailcraft_model::sync::{SendDone, SendOutcome};
    for kind in [Microsoft, Ews, Jmap] {
        let (mut w, id) = sent_through_outbox(kind);
        let m = w.m(id).clone();
        let outbox = w.s.mailbox.folder_of_kind(w.account, FolderKind::Outbox).unwrap();
        assert_eq!(m.folder, outbox, "{kind:?}: queued");
        // Sent a day later: counted from then, not from the click.
        let sent_at = NOON + DAY;
        let done = SendDone { message: id, outcome: SendOutcome::Sent, draft: None, draft_left: String::new() };
        super::graph_send::send_done_at(&mut w.s.mailbox, w.account, &done, mailcraft_i18n::Lang::default(), &mut Vec::new(), Some(sent_at));
        assert!(w.s.mailbox.message(id).is_none(), "{kind:?}: forgotten, the server's copy comes down");
        // The copy comes down under an id of its own.
        let down = Message { id: 0, folder: w.sent, follow_up: None, ..m };
        let copy = w.s.mailbox.add_message(down).unwrap();
        let f = w.m(copy).follow_up.unwrap_or_else(|| panic!("{kind:?}: the copy has the reminder"));
        assert_eq!((f.at, f.if_no_reply, f.days), (sent_at + 4 * DAY, true, 4), "{kind:?}");
        assert!(w.s.mailbox.follow_ups_pending.is_empty(), "{kind:?}: taken once");
        // Bob's answer to it cancels, as for any other.
        w.reply_to(copy, "bob@example.org");
        assert_eq!(w.settle(sent_at + 4 * DAY)["cancelled"], json!([copy]), "{kind:?}");
    }
}

#[test]
fn a_reminder_of_an_imap_message_counts_from_when_the_server_took_it() {
    let (mut w, id) = sent_through_outbox(mailcraft_model::AccountKind::Imap);
    assert_eq!(w.m(id).follow_up.unwrap().at, NOON + 4 * DAY, "from the click while it waits");
    let result = mailcraft_model::sync::SyncResult { sent: vec![id], ..Default::default() };
    super::account::file_sent(&mut w.s.mailbox, w.account, &result, NOON + 2 * DAY);
    assert_eq!(w.m(id).folder, w.sent);
    assert_eq!(w.m(id).follow_up.unwrap().at, NOON + 6 * DAY);
    // A message that waits in the Outbox never fires, however late.
    let (mut w, id) = sent_through_outbox(mailcraft_model::AccountKind::Imap);
    assert_eq!(w.settle(NOON + 30 * DAY)["raised"], json!([]));
    assert!(w.m(id).follow_up.is_some());
}

#[test]
fn a_reminder_pending_for_a_copy_is_kept_in_the_file_and_let_go_of_with_time() {
    let (mut w, id) = sent_through_outbox(mailcraft_model::AccountKind::Microsoft);
    let m = w.m(id).clone();
    let done = mailcraft_model::sync::SendDone { message: id, outcome: mailcraft_model::sync::SendOutcome::Sent, ..Default::default() };
    super::graph_send::send_done_at(&mut w.s.mailbox, w.account, &done, mailcraft_i18n::Lang::default(), &mut Vec::new(), Some(NOON));
    assert_eq!(w.s.mailbox.follow_ups_pending.len(), 1);
    reopen(&mut w);
    assert_eq!(w.s.mailbox.follow_ups_pending.len(), 1);
    // A copy dated more than the keeping time after the send takes nothing, and stays pending.
    let late = Message { id: 0, folder: w.sent, follow_up: None, date: NOON + 15 * DAY, ..m.clone() };
    let copy = w.s.mailbox.add_message(late).unwrap();
    assert!(w.m(copy).follow_up.is_none());
    assert_eq!(w.s.mailbox.follow_ups_pending.len(), 1);
    // Keeping another one, long after, lets the old one go.
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    w.s.mailbox.keep_follow_up_for_copy(w.mine, NOON + 15 * DAY);
    let kept: Vec<&str> = w.s.mailbox.follow_ups_pending.iter().map(|p| p.message_id.as_str()).collect();
    assert_eq!(kept, ["mine@example.org"], "the old one went");
}

#[test]
fn only_the_owners_own_copy_in_sent_items_takes_a_kept_reminder() {
    let (mut w, id) = sent_through_outbox(mailcraft_model::AccountKind::Microsoft);
    let m = w.m(id).clone();
    let done = mailcraft_model::sync::SendDone { message: id, outcome: mailcraft_model::sync::SendOutcome::Sent, ..Default::default() };
    super::graph_send::send_done_at(&mut w.s.mailbox, w.account, &done, mailcraft_i18n::Lang::default(), &mut Vec::new(), Some(NOON));
    let down = |folder, from: &str| Message { id: 0, folder, follow_up: None, from: mailcraft_model::Address::new("X", from), ..m.clone() };
    // A self-Bcc or a list's echo in the Inbox, a forged copy from someone else in Sent Items:
    // none takes it.
    for (folder, from) in [(w.inbox, "ann@example.org"), (w.sent, "mallory@example.org")] {
        let c = w.s.mailbox.add_message(down(folder, from)).unwrap();
        assert!(w.m(c).follow_up.is_none(), "{from} in {folder}");
        assert_eq!(w.s.mailbox.follow_ups_pending.len(), 1);
    }
    let c = w.s.mailbox.add_message(down(w.sent, "Ann@Example.org")).unwrap();
    assert!(w.m(c).follow_up.is_some());
}

#[test]
fn undo_leaves_the_reminders_kept_for_a_sent_copy_as_they_are() {
    let (mut w, id) = sent_through_outbox(mailcraft_model::AccountKind::Microsoft);
    let before = w.s.mailbox.clone();
    let done = mailcraft_model::sync::SendDone { message: id, outcome: mailcraft_model::sync::SendOutcome::Sent, ..Default::default() };
    super::graph_send::send_done_at(&mut w.s.mailbox, w.account, &done, mailcraft_i18n::Lang::default(), &mut Vec::new(), Some(NOON));
    assert_eq!(w.s.mailbox.follow_ups_pending.len(), 1);
    let touched = crate::undo::touched(&before, &w.s.mailbox);
    let (out, _) = crate::undo::keep_in_step(before.clone(), &touched, &w.s.mailbox, mailcraft_i18n::Lang::default());
    assert_eq!(out.follow_ups_pending, w.s.mailbox.follow_ups_pending, "the entry stays");
    // And one taken meanwhile is not brought back.
    let mut now = w.s.mailbox.clone();
    now.follow_ups_pending.clear();
    let (out, _) = crate::undo::keep_in_step(w.s.mailbox.clone(), &touched, &now, mailcraft_i18n::Lang::default());
    assert!(out.follow_ups_pending.is_empty());
}

#[test]
fn undo_of_an_unrelated_step_does_not_bring_back_a_reminder_that_ended() {
    let mut w = world();
    w.s.run("mail.reminder", &json!({"ids": [w.got], "at": NOON + 60})).unwrap();
    w.s.run("mail.markUnread", &json!({"ids": [w.got]})).unwrap();
    // The flag is cleared elsewhere (a sync), and the reminder ends with it.
    w.s.mailbox.message_mut(w.got).unwrap().flag = FlagStatus::None;
    w.settle(NOON + 120);
    assert!(w.m(w.got).follow_up.is_none());
    w.s.run("edit.undo", &json!({})).unwrap();
    assert!(w.m(w.got).read, "the step was undone");
    assert!(w.m(w.got).follow_up.is_none(), "an ended reminder does not come back to fire twice");
    // The same for one a reply cancelled.
    w.s.run("mail.reminder", &json!({"ids": [w.mine], "days": 1, "ifNoReply": true})).unwrap();
    w.s.run("mail.markUnread", &json!({"ids": [w.mine]})).unwrap();
    w.reply_from("bob@example.org");
    w.settle(NOON + 2 * DAY);
    assert!(w.m(w.mine).follow_up.is_none());
    w.s.run("edit.undo", &json!({})).unwrap();
    assert!(w.m(w.mine).follow_up.is_none());
}

#[test]
fn a_file_with_too_many_or_odd_pending_reminders_is_cut_down_when_repaired() {
    let mut mb = Mailbox::with_local_account("Ann", "ann@example.org");
    let account = mb.accounts[0].id;
    let entry = |i: usize, id: String| mailcraft_model::PendingFollowUp {
        account,
        message_id: id,
        follow_up: mailcraft_model::FollowUp::default(),
        since: i as i64,
    };
    mb.follow_ups_pending = (0..5_000).map(|i| entry(i, format!("m{i}@example.org"))).collect();
    mb.follow_ups_pending.push(entry(9_999, "x".repeat(100_000)));
    mb.follow_ups_pending.push(entry(9_998, String::new()));
    assert!(!mb.repair().is_empty());
    assert_eq!(mb.follow_ups_pending.len(), 200);
    assert!(mb.follow_ups_pending.iter().all(|p| !p.message_id.is_empty() && p.message_id.len() < 1_000));
    // The newest are the ones kept.
    assert!(mb.follow_ups_pending.iter().any(|p| p.message_id == "m4999@example.org"));
}

/// A Microsoft account with one message its server has, as `graph_down_flag_tests` makes one.
fn microsoft() -> (Session, Id) {
    use mailcraft_model::sync::ServerMessage;
    use mailcraft_model::{AccountKind, ServerFolder};
    let mut mb = Mailbox::with_local_account("Avery", "avery@mail.example");
    let account = mb.accounts[0].id;
    mb.accounts[0].kind = AccountKind::Microsoft;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mb.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F".into(), ..Default::default() });
    let server = ServerMessage { id: "E1".into(), folder: "F".into(), ..Default::default() };
    let id = mb
        .add_message(Message { account, folder: inbox, server: Some(server), subject: "Follow up".into(), read: true, ..Default::default() })
        .unwrap();
    let mut s = Session::new(mb);
    at(&mut s, NOON);
    (s, id)
}

#[test]
fn undo_of_adding_or_removing_a_reminder_works_for_a_microsoft_account_too() {
    let (mut s, id) = microsoft();
    let f = |s: &Session| s.mailbox.message(id).unwrap().follow_up;
    s.run("mail.reminder", &json!({"ids": [id], "at": NOON + 60})).unwrap();
    assert!(f(&s).is_some());
    s.run("edit.undo", &json!({})).unwrap();
    assert!(f(&s).is_none(), "adding it is undone");
    s.run("edit.redo", &json!({})).unwrap();
    assert!(f(&s).is_some());
    s.run("mail.clearReminder", &json!({"ids": [id]})).unwrap();
    assert!(f(&s).is_none());
    s.run("edit.undo", &json!({})).unwrap();
    assert!(f(&s).is_some(), "removing it is undone");
}

#[test]
fn undo_of_an_unrelated_step_keeps_a_dismissed_reminder_dismissed_for_a_microsoft_account() {
    let (mut s, id) = microsoft();
    s.run("mail.reminder", &json!({"ids": [id], "at": NOON + 60})).unwrap();
    s.run("mail.markUnread", &json!({"ids": [id]})).unwrap();
    at(&mut s, NOON + 120);
    s.run("mail.settleReminders", &json!({})).unwrap();
    s.run("mail.dismissReminder", &json!({"ids": [id]})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    let f = s.mailbox.message(id).unwrap().follow_up.unwrap();
    assert!(f.dismissed && f.raised.is_some());
}
