// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send later and Undo Send (#950). Invented people and servers.

use mailcraft_model::{FolderKind, Id, Mailbox};
use serde_json::json;

use super::*;
use crate::cmd::account::{apply_sync, sync_request};

const T0: i64 = 1_800_000_000;

/// An IMAP account in a session whose clock stands at `T0`.
fn imap() -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), T0, 0);
    let a = s
        .run("account.add", &json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example", "port": 587, "security": "starttls"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    (s, a)
}

fn write(s: &mut Session, a: Id, subject: &str) {
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": subject, "body": "Hello", "account": a})).unwrap();
}

/// Writes and sends `subject` to be delivered `after` seconds from now: its id.
fn send_after(s: &mut Session, a: Id, subject: &str, after: i64) -> Id {
    write(s, a, subject);
    s.run("compose.sendLater", &json!({"at": T0 + after})).unwrap()["id"].as_u64().unwrap()
}

fn to_send(s: &Session, a: Id) -> Vec<Id> {
    sync_request(s, a).unwrap().outbox.iter().map(|m| m.id).collect()
}

fn at(s: &mut Session, now: i64) {
    s.clock.now = Some(now);
}

#[test]
fn a_delayed_message_waits_in_the_outbox_until_its_time() {
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "Later", 3600);
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().folder, outbox);
    assert_eq!(s.mailbox.message(id).unwrap().send_at, Some(T0 + 3600));
    assert_eq!(s.mailbox.message(id).unwrap().date, T0 + 3600, "dated the time it goes, so the same octets go at every run");
    assert!(to_send(&s, a).is_empty(), "not before its time");
    assert_eq!(crate::sync_driver::round::waiting_to_go(&s, a), 0, "and it doesn't ask for a run");
    at(&mut s, T0 + 3599);
    assert!(to_send(&s, a).is_empty());
    at(&mut s, T0 + 3600);
    assert_eq!(to_send(&s, a), vec![id], "due at its time");
    assert_eq!(crate::sync_driver::round::waiting_to_go(&s, a), 1);
}

#[test]
fn a_message_sent_without_a_time_goes_at_the_next_run() {
    let (mut s, a) = imap();
    write(&mut s, a, "Now");
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().send_at, None);
    assert_eq!(to_send(&s, a), vec![id]);
}

#[test]
fn delay_delivery_is_kept_on_the_message_being_written_and_sends_with_it() {
    let (mut s, a) = imap();
    write(&mut s, a, "Delayed");
    assert_eq!(s.run("compose.delay", &json!({"at": T0 + 600})).unwrap()["sendAt"], T0 + 600);
    assert_eq!(s.compose.as_ref().unwrap().send_at, Some(T0 + 600));
    // Kept with its draft, and read back from it.
    s.run("compose.saveDraft", &json!({})).unwrap();
    let draft = s.compose.as_ref().unwrap().draft.unwrap();
    assert_eq!(s.mailbox.message(draft).unwrap().send_at, Some(T0 + 600));
    s.run("compose.close", &json!({})).unwrap();
    s.run("mail.editDraft", &json!({"id": draft})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().send_at, Some(T0 + 600));
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().send_at, Some(T0 + 600));
    assert!(to_send(&s, a).is_empty());
    // Taking the delay off.
    write(&mut s, a, "Again");
    s.run("compose.delay", &json!({"at": T0 + 600})).unwrap();
    s.run("compose.delay", &json!({"at": null})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().send_at, None);
}

#[test]
fn a_time_that_has_passed_by_the_time_of_send_is_no_delay() {
    let (mut s, a) = imap();
    write(&mut s, a, "Stale");
    s.run("compose.delay", &json!({"at": T0 + 60})).unwrap();
    at(&mut s, T0 + 120);
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().send_at, None);
    assert_eq!(to_send(&s, a), vec![id]);
}

#[test]
fn a_cancelled_message_goes_back_to_drafts_and_never_goes() {
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "Cancel me", 600);
    let drafts = s.mailbox.folder_of_kind(a, FolderKind::Drafts).unwrap();
    assert_eq!(s.run("mail.cancelSend", &json!({"id": id})).unwrap()["draft"], true);
    let m = s.mailbox.message(id).unwrap();
    assert_eq!((m.folder, m.draft), (drafts, true));
    at(&mut s, T0 + 10_000);
    assert!(to_send(&s, a).is_empty(), "long past its time, nothing goes");
    // It isn't in the Outbox any more, so it can't be cancelled or changed again.
    assert!(s.run("mail.cancelSend", &json!({"id": id})).is_err());
    assert!(s.run("mail.setSendTime", &json!({"id": id, "at": T0 + 20_000})).is_err());
}

#[test]
fn changing_the_send_time_moves_it_and_null_lets_it_go() {
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "Move me", 600);
    s.run("mail.setSendTime", &json!({"id": id, "at": T0 + 7200})).unwrap();
    at(&mut s, T0 + 600);
    assert!(to_send(&s, a).is_empty(), "the old time no longer counts");
    assert_eq!(s.mailbox.message(id).unwrap().date, T0 + 7200);
    s.run("mail.setSendTime", &json!({"id": id, "at": null})).unwrap();
    assert_eq!(to_send(&s, a), vec![id]);
}

#[test]
fn changing_and_cancelling_are_undo_steps() {
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "Undo me", 600);
    s.run("mail.setSendTime", &json!({"id": id, "at": T0 + 900})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().send_at, Some(T0 + 600));
    s.run("mail.cancelSend", &json!({"id": id})).unwrap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!((m.folder, m.send_at), (outbox, Some(T0 + 600)), "back in the Outbox with its time");
    s.run("edit.redo", &json!({})).unwrap();
    assert_ne!(s.mailbox.message(id).unwrap().folder, outbox);
}

#[test]
fn a_delayed_message_survives_a_restart_and_goes_late_at_the_next_run() {
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "Across restarts", 600);
    // Saved to disk and opened again, as a restart does.
    let dir = std::env::temp_dir().join(format!("mc-send-later-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mail.mailcraft");
    crate::store::save(&path, &s.mailbox).unwrap();
    let opened = crate::store::open(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let mut back = Session::at(opened.mailbox, T0 + 30, 0);
    assert_eq!(back.mailbox.message(id).unwrap().send_at, Some(T0 + 600));
    assert!(to_send(&back, a).is_empty(), "still waiting after the restart");
    // MailCraft wasn't running at the time: the next run takes it, and the status bar says late.
    assert_eq!(back.send_later_note().unwrap(), SendLaterNote { next: Some(T0 + 600), waiting: 1, late: 0 });
    at(&mut back, T0 + 600 + 3 * 3600);
    assert_eq!(to_send(&back, a), vec![id]);
    assert_eq!(back.send_later_note().unwrap().late, 1);
    // A second after its time isn't late: due, and nothing to say.
    at(&mut back, T0 + 601);
    assert_eq!(back.send_later_note(), None);
}

#[test]
fn a_clock_set_back_never_sends_early_and_one_set_forward_sends_by_the_formula() {
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "Clocks", 600);
    at(&mut s, T0 + 700);
    assert_eq!(to_send(&s, a), vec![id]);
    // Set back before the time again (the run hasn't taken it): it waits.
    at(&mut s, T0 - 86_400);
    assert!(to_send(&s, a).is_empty());
    assert_eq!(s.send_later_note().unwrap().next, Some(T0 + 600));
    // Asleep through its time: due as soon as the clock says so, once.
    at(&mut s, T0 + 5 * 86_400);
    assert_eq!(to_send(&s, a), vec![id]);
}

#[test]
fn the_status_bar_note_names_the_next_time_and_the_count() {
    let (mut s, a) = imap();
    assert!(s.send_later_note().is_none());
    send_after(&mut s, a, "Second", 7200);
    send_after(&mut s, a, "First", 600);
    assert_eq!(s.send_later_note().unwrap(), SendLaterNote { next: Some(T0 + 600), waiting: 2, late: 0 });
}

#[test]
fn bad_times_are_refused() {
    let (mut s, a) = imap();
    write(&mut s, a, "Bad");
    for bad in [
        json!({}),
        json!({"at": "soon"}),
        json!({"at": T0}),
        json!({"at": T0 - 5}),
        json!({"at": T0 + MAX_AHEAD + 1}),
        json!({"at": 1.5}),
        json!({"at": T0 + 60, "x": 1}),
    ] {
        assert!(s.run("compose.delay", &bad).is_err(), "{bad}");
        assert!(s.run("compose.sendLater", &bad).is_err(), "{bad}");
    }
    assert!(s.compose.is_some(), "still being written");
    assert_eq!(s.mailbox.messages_in(s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap()).count(), 0);
}

#[test]
fn a_local_account_cant_wait_for_a_time() {
    let mut s = Session::at(Mailbox::with_local_account("Me", "me@local.example"), T0, 0);
    s.run("mail.new", &json!({"to": "someone@lumen.example", "subject": "Here"})).unwrap();
    s.run("compose.delay", &json!({"at": T0 + 600})).unwrap();
    assert!(s.run("compose.send", &json!({})).is_err());
    assert!(s.compose.is_some(), "the message is still being written");
}

#[test]
fn the_wait_before_sending_puts_the_message_in_the_outbox_with_a_time() {
    let (mut s, a) = imap();
    assert!(s.run("sync.waitBeforeSending", &json!({"seconds": 7})).is_err());
    assert_eq!(s.run("sync.waitBeforeSending", &json!({"seconds": 10})).unwrap()["seconds"], 10);
    write(&mut s, a, "Oops");
    let sent = s.run("compose.send", &json!({})).unwrap();
    assert_eq!(sent["wait"], true);
    let id = sent["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().send_at, Some(T0 + 10));
    assert_eq!(s.undo_send_wait(), Some((id, T0 + 10)));
    assert!(to_send(&s, a).is_empty(), "not within the wait");
    at(&mut s, T0 + 10);
    assert_eq!(to_send(&s, a), vec![id], "it goes when the wait is over");
    assert_eq!(s.undo_send_wait(), None, "and the bar is gone");
    // A message with a time of its own doesn't get the wait on top of it.
    at(&mut s, T0);
    let later = send_after(&mut s, a, "Own time", 3600);
    assert_eq!(s.mailbox.message(later).unwrap().send_at, Some(T0 + 3600));
    assert_eq!(s.undo_send_wait(), None);
}

#[test]
fn undo_send_returns_the_message_to_drafts_in_a_composer() {
    let (mut s, a) = imap();
    s.run("sync.waitBeforeSending", &json!({"seconds": 5})).unwrap();
    write(&mut s, a, "Wrong person");
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(s.compose.is_none());
    let back = s.run("compose.undoSend", &json!({})).unwrap();
    assert_eq!(back["composing"], true);
    assert_eq!(s.compose.as_ref().unwrap().subject, "Wrong person");
    assert_eq!(s.compose.as_ref().unwrap().draft, Some(id));
    assert_eq!(s.compose.as_ref().unwrap().send_at, None, "a wait is no Send time to keep");
    let drafts = s.mailbox.folder_of_kind(a, FolderKind::Drafts).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().folder, drafts);
    at(&mut s, T0 + 100);
    assert!(to_send(&s, a).is_empty(), "it never goes");
    assert!(s.run("compose.undoSend", &json!({})).is_err(), "once only");
}

#[test]
fn undo_send_is_refused_once_the_wait_is_over() {
    let (mut s, a) = imap();
    s.run("sync.waitBeforeSending", &json!({"seconds": 5})).unwrap();
    write(&mut s, a, "Too late");
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    at(&mut s, T0 + 5);
    assert!(s.run("compose.undoSend", &json!({})).is_err());
    assert_eq!(to_send(&s, a), vec![id]);
}

#[test]
fn a_message_being_sent_cant_be_cancelled_or_changed() {
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "On its way", 60);
    at(&mut s, T0 + 60);
    s.mailbox.message_mut(id).unwrap().sending = true;
    assert!(s.run("mail.cancelSend", &json!({"id": id})).is_err());
    assert!(s.run("mail.setSendTime", &json!({"id": id, "at": T0 + 9000})).is_err());
}

#[test]
fn open_draft_on_a_queued_message_takes_it_out_of_the_queue_with_its_time() {
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "Edit me", 600);
    s.run("mail.editDraft", &json!({"id": id})).unwrap();
    let c = s.compose.as_ref().unwrap();
    assert_eq!((c.subject.as_str(), c.send_at), ("Edit me", Some(T0 + 600)));
    at(&mut s, T0 + 10_000);
    assert!(to_send(&s, a).is_empty(), "taken out of the queue while it is written");
    // Send again: queued with its time (already passed here: it goes at the next run).
    let again = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(to_send(&s, a), vec![again]);
}

#[test]
fn hostile_params_are_refused_without_a_panic() {
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "Hostile", 600);
    let junk = [
        json!(null),
        json!([]),
        json!("x"),
        json!({"id": "x"}),
        json!({"id": null}),
        json!({"id": 0}),
        json!({"id": -1}),
        json!({"id": 1e30, "at": 1e30}),
        json!({"id": id, "at": "x"}),
        json!({"id": id, "at": i64::MAX}),
        json!({"id": id, "at": i64::MIN}),
        json!({"ids": [id, id]}),
        json!({"seconds": -1}),
        json!({"seconds": 1e9}),
        json!({"seconds": "5"}),
    ];
    for cmd in ["compose.delay", "compose.sendLater", "mail.setSendTime", "mail.cancelSend", "compose.undoSend", "sync.waitBeforeSending"] {
        for p in &junk {
            let _ = s.run(cmd, p);
        }
    }
    assert!(s.mailbox.message(id).is_some());
}

#[test]
fn apply_sync_sends_nothing_that_waits() {
    // A run that brings nothing back leaves a delayed message where it is.
    let (mut s, a) = imap();
    let id = send_after(&mut s, a, "Stays", 600);
    let result = mailcraft_model::sync::SyncResult { account: a, ..Default::default() };
    apply_sync(&mut s, result).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().send_at, Some(T0 + 600));
}

/// A delayed message of each kind of account that sends from an Outbox waits out of its runs'
/// requests until its time: Microsoft Graph, an Exchange server, JMAP (and IMAP above).
#[test]
fn a_delayed_message_is_in_no_kind_of_runs_request_before_its_time() {
    use mailcraft_model::{Account, AccountKind, JmapServer};
    let mut s = Session::at(Mailbox::with_local_account("", ""), T0, 0);
    let microsoft = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let exchange = s
        .run("account.add", &json!({"email": "ada@corp.example", "kind": "exchange", "ews": {"url": "https://mail.corp.example/EWS/Exchange.asmx", "username": "CORP\\ada"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.mailbox.account_mut(exchange).unwrap().ews.as_mut().unwrap().trusted = "ab".repeat(32);
    let server = JmapServer { session: "https://jmap.example/.well-known/jmap".into(), bearer: true };
    let jmap = s.mailbox.add_account(Account { email: "ada@jmap.example".into(), kind: AccountKind::Jmap, jmap: Some(server), ..Default::default() });
    for (account, name) in [(microsoft, "Microsoft"), (exchange, "Exchange"), (jmap, "JMAP")] {
        let id = send_after(&mut s, account, name, 600);
        at(&mut s, T0);
        let waits = |s: &Session| -> Vec<Id> {
            let request: Vec<Id> = match account {
                a if a == microsoft => crate::cmd::graph_mail::graph_mail_request(s, a).unwrap().sends.iter().map(|up| up.message).collect(),
                a if a == exchange => crate::cmd::ews_mail::ews_mail_request(s, a, false).unwrap().sends.iter().map(|up| up.message).collect(),
                a => crate::cmd::jmap_mail::jmap_mail_request(s, a, true).unwrap().sends.iter().map(|up| up.message).collect(),
            };
            assert_eq!(crate::cmd::graph_send::sends(s, account).iter().map(|up| up.message).collect::<Vec<_>>(), request, "{name}");
            request
        };
        assert!(waits(&s).is_empty(), "{name}: not before its time");
        assert_eq!(crate::sync_driver::round::waiting_to_go(&s, account), 0, "{name}: no run is asked for");
        at(&mut s, T0 + 599);
        assert!(waits(&s).is_empty(), "{name}: not a second early");
        at(&mut s, T0 + 600);
        assert_eq!(waits(&s), vec![id], "{name}: due");
        // Cancelled (back to Drafts): never offered.
        at(&mut s, T0);
        s.run("mail.cancelSend", &json!({"id": id})).unwrap();
        at(&mut s, T0 + 10_000);
        assert!(waits(&s).is_empty(), "{name}: a cancelled message never goes");
        at(&mut s, T0);
    }
}
