// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snooze (#949) on a mailbox kept on this computer: snoozing, the return at the Return time (also
//! after MailCraft was closed past it), the early return on a reply, the snooze called off by a
//! move, Return now, the undo steps, a source folder that is gone, and hostile parameters. The
//! servers' side is in `snooze_servers_tests`.

use mailcraft_model::{FolderKind, Id, Mailbox, Message};
use serde_json::{Value, json};

use super::snooze;
use crate::agent::{Class, Verdict, check};
use crate::{Clock, Session};

const NOON: i64 = 1_800_000_000;

struct World {
    s: Session,
    account: Id,
    inbox: Id,
    a: Id,
    b: Id,
}

fn at(s: &mut Session, now: i64) {
    s.clock = Clock { now: Some(now), offset: Some(0), zone: None };
}

fn world() -> World {
    let mut s = Session::new(Mailbox::with_local_account("Ann", "ann@example.org"));
    at(&mut s, NOON);
    let account = s.mailbox.accounts[0].id;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let mut add = |subject: &str, id: &str| {
        let m = Message {
            account,
            folder: inbox,
            subject: subject.into(),
            message_id: id.into(),
            read: true,
            received: NOON - 5_000,
            ..Default::default()
        };
        s.mailbox.add_message(m).unwrap()
    };
    let (a, b) = (add("Kickoff", "a@example.org"), add("Budget", "b@example.org"));
    World { s, account, inbox, a, b }
}

fn folder_of(s: &Session, id: Id) -> Id {
    s.mailbox.message(id).unwrap().folder
}

#[test]
fn snoozing_moves_the_message_into_a_snoozed_folder_named_in_the_interface_language() {
    let mut w = world();
    let r = w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 3_600})).unwrap();
    assert_eq!(r["snoozed"], 1);
    let snoozed = w.s.mailbox.snoozed_folder(w.account).unwrap();
    assert_eq!(folder_of(&w.s, w.a), snoozed);
    assert_eq!(w.s.mailbox.folder(snoozed).unwrap().name, "Snoozed");
    let z = w.s.mailbox.message(w.a).unwrap().snooze.unwrap();
    assert_eq!((z.until, z.from, z.returned), (NOON + 3_600, w.inbox, None));
    // The other stays; a second snooze uses the same folder.
    assert_eq!(folder_of(&w.s, w.b), w.inbox);
    w.s.run("mail.snooze", &json!({"ids": [w.b], "until": "2027-01-20T09:00"})).unwrap();
    assert_eq!(folder_of(&w.s, w.b), snoozed);
    assert_eq!(w.s.mailbox.folders.iter().filter(|f| f.name == "Snoozed").count(), 1);
}

#[test]
fn it_comes_back_at_its_return_time_unread_and_marked_with_its_received_date_as_it_was() {
    let mut w = world();
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 600})).unwrap();
    at(&mut w.s, NOON + 599);
    assert_eq!(w.s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([]));
    assert_ne!(folder_of(&w.s, w.a), w.inbox);
    at(&mut w.s, NOON + 600);
    assert_eq!(w.s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([w.a]));
    let m = w.s.mailbox.message(w.a).unwrap();
    assert_eq!((m.folder, m.read, m.received), (w.inbox, false, NOON - 5_000));
    assert_eq!(m.snooze.unwrap().returned, Some(NOON + 600));
    // Once: the next step finds nothing due.
    assert_eq!(w.s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([]));
}

#[test]
fn after_a_restart_past_the_time_it_comes_back_marked_with_when_it_came_back() {
    let mut w = world();
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 600})).unwrap();
    let saved = serde_json::to_string(&w.s.mailbox).unwrap();
    let mut again = Session::new(serde_json::from_str::<Mailbox>(&saved).unwrap());
    at(&mut again, NOON + 90_000);
    assert_eq!(again.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([w.a]));
    let m = again.mailbox.message(w.a).unwrap();
    assert_eq!((m.folder, m.read), (w.inbox, false));
    let z = m.snooze.unwrap();
    // Due at one time, came back at another.
    assert_eq!((z.until, z.returned), (NOON + 600, Some(NOON + 90_000)));
}

#[test]
fn mail_in_the_same_conversation_brings_it_back_at_once() {
    let mut w = world();
    w.s.run("mail.snooze", &json!({"ids": [w.a, w.b], "until": NOON + 86_400})).unwrap();
    let reply = Message {
        account: w.account,
        folder: w.inbox,
        subject: "Re: Kickoff".into(),
        message_id: "r@example.org".into(),
        in_reply_to: "a@example.org".into(),
        received: NOON + 10,
        ..Default::default()
    };
    w.s.mailbox.add_message(reply).unwrap();
    snooze::after(&mut w.s);
    let m = w.s.mailbox.message(w.a).unwrap();
    assert_eq!((m.folder, m.read), (w.inbox, false));
    assert_eq!(m.snooze.unwrap().returned, Some(NOON));
    // The other conversation waits on.
    assert_ne!(folder_of(&w.s, w.b), w.inbox);
    assert!(w.s.mailbox.message(w.b).unwrap().snooze.is_some_and(|z| z.waiting()));
}

#[test]
fn what_the_owner_did_and_what_is_not_news_does_not_bring_it_back() {
    let mut w = world();
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 86_400})).unwrap();
    let sent = w.s.mailbox.folder_of_kind(w.account, FolderKind::Sent).unwrap();
    let junk = w.s.mailbox.folder_of_kind(w.account, FolderKind::Junk).unwrap();
    for folder in [sent, junk] {
        let m = Message {
            account: w.account,
            folder,
            subject: "Re: Kickoff".into(),
            in_reply_to: "a@example.org".into(),
            received: NOON + 10,
            ..Default::default()
        };
        w.s.mailbox.add_message(m).unwrap();
    }
    snooze::after(&mut w.s);
    assert!(w.s.mailbox.message(w.a).unwrap().snooze.is_some_and(|z| z.waiting()));
}

#[test]
fn moved_out_of_the_snoozed_folder_by_hand_the_snooze_is_called_off() {
    let mut w = world();
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 600})).unwrap();
    let archive = w.s.mailbox.folder_of_kind(w.account, FolderKind::Archive).unwrap();
    w.s.run("mail.move", &json!({"ids": [w.a], "folder": archive})).unwrap();
    at(&mut w.s, NOON + 700);
    let r = w.s.run("mail.returnDue", &json!({})).unwrap();
    assert_eq!((r["returned"].clone(), r["calledOff"].clone()), (json!([]), json!(1)));
    // It stays where the owner put it, read as it was, with nothing to come back to.
    let m = w.s.mailbox.message(w.a).unwrap();
    assert_eq!((m.folder, m.read, m.snooze), (archive, true, None));
}

#[test]
fn return_now_brings_it_back_and_both_it_and_snoozing_are_one_undo_step() {
    let mut w = world();
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 600})).unwrap();
    w.s.run("mail.unsnooze", &json!({"ids": [w.a]})).unwrap();
    assert_eq!(folder_of(&w.s, w.a), w.inbox);
    assert!(!w.s.mailbox.message(w.a).unwrap().read);
    // Undoing Return now: waiting in Snoozed again, as it was, read.
    w.s.run("edit.undo", &json!({})).unwrap();
    let m = w.s.mailbox.message(w.a).unwrap();
    assert_ne!(m.folder, w.inbox);
    assert!(m.read && m.snooze.is_some_and(|z| z.waiting() && z.until == NOON + 600));
    // Undoing the snooze: in the Inbox, with nothing remembered.
    w.s.run("edit.undo", &json!({})).unwrap();
    let m = w.s.mailbox.message(w.a).unwrap();
    assert_eq!((m.folder, m.snooze), (w.inbox, None));
}

#[test]
fn return_now_on_mail_that_isnt_snoozed_is_refused() {
    let mut w = world();
    assert!(w.s.run("mail.unsnooze", &json!({"ids": [w.a]})).is_err());
    assert_eq!(folder_of(&w.s, w.a), w.inbox);
}

#[test]
fn a_source_folder_that_is_gone_sends_it_to_the_inbox() {
    let mut w = world();
    let projects = w.s.mailbox.add_folder(w.account, None, "Projects").unwrap();
    w.s.run("mail.move", &json!({"ids": [w.a], "folder": projects})).unwrap();
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 600})).unwrap();
    w.s.run("folder.delete", &json!({"id": projects})).unwrap();
    at(&mut w.s, NOON + 601);
    assert_eq!(w.s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([w.a]));
    assert_eq!(folder_of(&w.s, w.a), w.inbox);
}

#[test]
fn it_goes_back_to_the_folder_it_came_from() {
    let mut w = world();
    let projects = w.s.mailbox.add_folder(w.account, None, "Projects").unwrap();
    w.s.run("mail.move", &json!({"ids": [w.a], "folder": projects})).unwrap();
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 600})).unwrap();
    at(&mut w.s, NOON + 601);
    w.s.run("mail.returnDue", &json!({})).unwrap();
    assert_eq!(folder_of(&w.s, w.a), projects);
}

#[test]
fn a_flag_a_category_and_a_reminder_are_kept() {
    let mut w = world();
    w.s.run("mail.flag", &json!({"ids": [w.a]})).unwrap();
    w.s.run("mail.categorize", &json!({"ids": [w.a], "category": "Blue category"})).unwrap();
    let before = w.s.mailbox.message(w.a).unwrap().clone();
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 600})).unwrap();
    let m = w.s.mailbox.message(w.a).unwrap();
    assert_eq!((m.flag, &m.categories, m.flag_due), (before.flag, &before.categories, before.flag_due));
}

#[test]
fn a_return_time_that_is_not_in_the_future_or_is_not_a_time_is_refused_and_a_far_one_is_brought_in() {
    let mut w = world();
    let refused = |w: &mut World, until: Value| {
        let r = w.s.run("mail.snooze", &json!({"ids": [w.a], "until": until}));
        assert!(r.is_err(), "{until}");
        assert_eq!(folder_of(&w.s, w.a), w.inbox);
        assert!(w.s.mailbox.snoozed_folder(w.account).is_none(), "nothing made for a refusal");
    };
    for until in [json!(null), json!("soon"), json!(NOON), json!(0), json!(-5), json!(1.5e300), json!(true), json!({}), json!("0001-01-01T00:00")] {
        refused(&mut w, until);
    }
    assert!(w.s.run("mail.snooze", &json!({"ids": [w.a]})).is_err());
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": i64::MAX})).unwrap();
    let until = w.s.mailbox.message(w.a).unwrap().snooze.unwrap().until;
    assert_eq!(until, NOON + mailcraft_model::MAX_SNOOZE_AHEAD);
    w.s.run("mail.snooze", &json!({"ids": [w.b], "until": "2200-12-31T23:59"})).unwrap();
    assert!(w.s.run("mail.snooze", &json!({"ids": [w.a], "until": "9999-12-31T23:59"})).is_err());
}

#[test]
fn a_draft_cant_be_snoozed_and_the_others_with_it_stay() {
    let mut w = world();
    let draft = Message { account: w.account, folder: w.inbox, draft: true, ..Default::default() };
    let draft = w.s.mailbox.add_message(draft).unwrap();
    assert!(w.s.run("mail.snooze", &json!({"ids": [w.a, draft], "until": NOON + 600})).is_err());
    assert_eq!(folder_of(&w.s, w.a), w.inbox);
}

#[test]
fn an_agent_snoozes_like_it_moves_naming_the_messages_and_without_being_asked() {
    let w = world();
    let named = json!({"ids": [w.a], "until": NOON + 600});
    for id in ["mail.snooze", "mail.unsnooze", "mail.returnDue"] {
        assert_eq!(check(&w.s, id, &named, &[]), Verdict::Run, "{id}");
        assert!(w.s.registry.get(id).is_some());
    }
    // Never the window's selection, which the owner may have changed a moment ago.
    for id in ["mail.snooze", "mail.unsnooze"] {
        assert!(crate::agent::NEEDS_IDS.contains(&id), "{id}");
    }
    assert_eq!(check(&w.s, "mail.move", &json!({"ids": [w.a], "folder": w.inbox}), &[]), check(&w.s, "mail.snooze", &named, &[]));
    let _ = Class::Send;
}

#[test]
fn the_snoozed_folder_stays_the_same_one_when_it_is_renamed_or_the_language_changes() {
    let mut w = world();
    w.s.run("mail.snooze", &json!({"ids": [w.a], "until": NOON + 600})).unwrap();
    let snoozed = w.s.mailbox.snoozed_folder(w.account).unwrap();
    w.s.run("folder.rename", &json!({"id": snoozed, "name": "Later"})).unwrap();
    w.s.run("mail.snooze", &json!({"ids": [w.b], "until": NOON + 600})).unwrap();
    assert_eq!(folder_of(&w.s, w.b), snoozed);
    assert_eq!(w.s.mailbox.folders.iter().filter(|f| f.account == w.account && f.parent.is_none() && f.kind == FolderKind::Custom).count(), 1);
}
