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
