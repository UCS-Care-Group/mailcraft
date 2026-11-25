// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The wait before sending, with Undo Send (#1799): the rules it keeps, one test each (the
//! module's own docs say them). Invented people and servers.

use mailcraft_model::{Attachment, FolderKind, Id, Importance, Mailbox};
use serde_json::json;

use super::*;
use crate::cmd::account::sync_request;

const T0: i64 = 1_800_000_000;

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

/// Sent with the wait on: its id.
fn send(s: &mut Session, a: Id, subject: &str) -> Id {
    write(s, a, subject);
    let sent = s.run("compose.send", &json!({})).unwrap();
    assert_eq!(sent["wait"], true, "{subject}: waits");
    sent["id"].as_u64().unwrap()
}

fn to_send(s: &Session, a: Id) -> Vec<Id> {
    sync_request(s, a).unwrap().outbox.iter().map(|m| m.id).collect()
}

fn at(s: &mut Session, now: i64) {
    s.clock.now = Some(now);
}

fn in_folder(s: &Session, a: Id, kind: FolderKind) -> Vec<Id> {
    let f = s.mailbox.folder_of_kind(a, kind).unwrap();
    s.mailbox.messages_in(f).map(|m| m.id).collect()
}

#[test]
fn minutes_are_offered_and_a_settings_file_is_read_to_a_choice() {
    let (mut s, _) = imap();
    for n in [0u8, 5, 10, 15, 20, 30, 60, 120] {
        assert_eq!(s.run("sync.waitBeforeSending", &json!({"seconds": n})).unwrap()["seconds"], n);
        assert_eq!(s.prefs.undo_send(), n);
    }
    for bad in [json!(7), json!(90), json!(121), json!(600), json!(-5), json!("60")] {
        assert!(s.run("sync.waitBeforeSending", &json!({"seconds": bad})).is_err(), "{bad}");
    }
    // A settings file may say anything: the next choice up, never more than the most offered.
    for (kept, read) in [(0u8, 0u8), (31, 60), (60, 60), (61, 120), (200, 120), (255, 120)] {
        let p = Prefs { undo_send_secs: kept, ..Prefs::default() };
        assert_eq!(p.undo_send(), read, "{kept}");
    }
    assert_eq!(Prefs::default().undo_send(), 0, "off until the owner turns it on");
}

/// Send/Receive within the wait: the message is in no run's request, whatever the wait.
#[test]
fn a_two_minute_wait_holds_the_message_out_of_every_run_and_undo_works_to_its_last_second() {
    let (mut s, a) = imap();
    s.run("sync.waitBeforeSending", &json!({"seconds": 120})).unwrap();
    let id = send(&mut s, a, "Two minutes");
    assert_eq!(s.mailbox.message(id).unwrap().send_at, Some(T0 + 120));
    at(&mut s, T0 + 119);
    assert!(to_send(&s, a).is_empty(), "Send/Receive a second before: not taken");
    assert_eq!(crate::sync_driver::round::waiting_to_go(&s, a), 0, "and no run is asked for");
    assert_eq!(s.undo_send_wait(), Some((id, T0 + 120)));
    at(&mut s, T0 + 120);
    assert_eq!(to_send(&s, a), vec![id], "goes at the end of the wait");
    assert!(s.run("compose.undoSend", &json!({})).is_err(), "and can't be taken back then");
}

/// At most once: what a run took (marked as being sent) is never taken back, not even with the
/// clock set back; and taking back then sending again leaves one message, not two.
#[test]
fn a_message_is_sent_at_most_once() {
    let (mut s, a) = imap();
    s.run("sync.waitBeforeSending", &json!({"seconds": 30})).unwrap();
    let id = send(&mut s, a, "Once");
    // Taken back and sent again: one message waits, nothing is left in Drafts.
    s.run("compose.undoSend", &json!({})).unwrap();
    let again = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(in_folder(&s, a, FolderKind::Outbox), vec![again]);
    assert!(in_folder(&s, a, FolderKind::Drafts).is_empty(), "the draft became the message");
    assert!(id == again || s.mailbox.message(id).is_none(), "the first is not left behind");
    // The run takes it at its time and marks it; the clock is then set back inside the wait.
    at(&mut s, T0 + 30);
    assert_eq!(to_send(&s, a), vec![again]);
    s.send_marking(a, &[again]).unwrap();
    at(&mut s, T0 + 1);
    assert_eq!(s.undo_send_wait(), None, "on its way: no Undo");
    assert!(s.run("compose.undoSend", &json!({})).is_err());
    assert!(s.run("mail.cancelSend", &json!({"id": again})).is_err());
    assert!(s.run("mail.editDraft", &json!({"id": again})).is_err(), "never written again while it goes");
    assert_eq!(in_folder(&s, a, FolderKind::Outbox), vec![again], "one message, still in the Outbox until the run answers");
}

/// Never lost: quit (or a crash after the save) within the wait leaves the message in the
/// Outbox with its time. After the restart it goes at its time, or at the next run when that has
/// passed; the bar is gone (it was this run's), and Cancel Send still takes it back.
#[test]
fn a_quit_within_the_wait_keeps_the_message_in_the_outbox() {
    let (mut s, a) = imap();
    s.run("sync.waitBeforeSending", &json!({"seconds": 60})).unwrap();
    let id = send(&mut s, a, "Across a quit");
    let dir = std::env::temp_dir().join(format!("mc-send-delay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mail.mailcraft");
    crate::store::save(&path, &s.mailbox).unwrap();
    let opened = crate::store::open(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    // Back within the wait.
    let mut back = Session::at(opened.mailbox, T0 + 20, 0);
    back.prefs = s.prefs.clone();
    assert_eq!(in_folder(&back, a, FolderKind::Outbox), vec![id], "still in the Outbox");
    assert_eq!(back.mailbox.message(id).unwrap().send_at, Some(T0 + 60));
    assert!(to_send(&back, a).is_empty(), "not before its time");
    assert_eq!(back.undo_send_wait(), None, "the bar was the last run's");
    assert!(back.run("compose.undoSend", &json!({})).is_err());
    // Back after it: the next run takes it, once.
    let late = Session::at(back.mailbox.clone(), T0 + 3600, 0);
    assert_eq!(to_send(&late, a), vec![id]);
    // And within the wait, Cancel Send still takes it back to Drafts.
    back.run("mail.cancelSend", &json!({"id": id})).unwrap();
    assert_eq!(in_folder(&back, a, FolderKind::Drafts), vec![id]);
}

/// Work Offline: a message whose wait is over stays in the Outbox, no run is asked for, Cancel
/// Send still takes it back; once online it goes with the next run.
#[test]
fn working_offline_holds_the_message_after_its_wait() {
    let (mut s, a) = imap();
    s.run("sync.waitBeforeSending", &json!({"seconds": 10})).unwrap();
    s.run("sync.offline", &json!({"value": true})).unwrap();
    let id = send(&mut s, a, "Offline");
    at(&mut s, T0 + 600);
    assert!(s.run("sync.request", &json!({"account": a})).is_err(), "no run while offline");
    assert_eq!(in_folder(&s, a, FolderKind::Outbox), vec![id]);
    assert_eq!(s.undo_send_wait(), None, "the wait is over: no bar");
    assert!(s.run("compose.undoSend", &json!({})).is_err());
    s.run("sync.offline", &json!({"value": false})).unwrap();
    assert_eq!(to_send(&s, a), vec![id], "online: it goes");
    // Taken back while offline instead: never goes.
    s.run("sync.offline", &json!({"value": true})).unwrap();
    s.run("mail.cancelSend", &json!({"id": id})).unwrap();
    s.run("sync.offline", &json!({"value": false})).unwrap();
    assert!(to_send(&s, a).is_empty());
}

/// An agent's Send that the owner approved waits too: the wait only ever holds mail back, and the
/// owner keeps the same Undo. The agent's answer says so.
#[test]
fn an_approved_agent_send_waits_and_the_owner_can_take_it_back() {
    let (mut s, a) = imap();
    s.run("sync.waitBeforeSending", &json!({"seconds": 20})).unwrap();
    write(&mut s, a, "From an agent");
    s.agent = Some(vec![]);
    assert!(s.run("compose.send", &json!({})).is_err(), "not approved: nothing goes");
    s.agent = Some(vec![crate::agent::Class::Send]);
    let sent = s.run("compose.send", &json!({})).unwrap();
    s.agent = None;
    assert_eq!((sent["wait"].clone(), sent["sendAt"].clone()), (json!(true), json!(T0 + 20)));
    let id = sent["id"].as_u64().unwrap();
    assert!(to_send(&s, a).is_empty());
    assert_eq!(s.undo_send_wait(), Some((id, T0 + 20)));
    s.run("compose.undoSend", &json!({})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().subject, "From an agent");
}

/// Undo Send gives back the message as it was written: the same lines, text, importance and files.
#[test]
fn undo_send_gives_the_message_back_unchanged() {
    let (mut s, a) = imap();
    s.run("sync.waitBeforeSending", &json!({"seconds": 15})).unwrap();
    write(&mut s, a, "Check this");
    s.run("compose.set", &json!({"cc": "kit@lumen.example", "bcc": "rue@lumen.example", "importance": "high", "body": "Line one\nLine two"}))
        .unwrap();
    s.compose.as_mut().unwrap().attachments.push(Attachment::new("plan.txt", "text/plain", b"steps".to_vec()));
    let before = s.compose.clone().unwrap();
    s.run("compose.send", &json!({})).unwrap();
    s.run("compose.undoSend", &json!({})).unwrap();
    let c = s.compose.as_ref().unwrap();
    assert_eq!((&c.to, &c.cc, &c.bcc, &c.subject), (&before.to, &before.cc, &before.bcc, &before.subject));
    assert_eq!(c.body.plain(), before.body.plain());
    assert_eq!(c.importance, Importance::High);
    assert_eq!(
        c.attachments.iter().map(|x| (&x.name, &x.data)).collect::<Vec<_>>(),
        before.attachments.iter().map(|x| (&x.name, &x.data)).collect::<Vec<_>>()
    );
    assert_eq!(c.account, before.account);
}
