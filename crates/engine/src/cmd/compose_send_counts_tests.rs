// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The send counts behind recipient suggestions are kept per mailbox revision (#1803): a send
//! made, undone or deleted between two key presses is counted as it stands, and the answers are
//! the ones a count made afresh gives.

use mailcraft_model::{Address, FolderKind, Message};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::Session;

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> Session {
    Session::at(mailcraft_model::Mailbox::with_local_account("Avery Quinn", "avery@lumen.example"), now(), 0)
}

fn post(s: &mut Session, folder: FolderKind, to: &[(&str, &str)], cc: &[(&str, &str)], date: i64) {
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.folder_of_kind(account, folder).unwrap();
    let list = |v: &[(&str, &str)]| v.iter().map(|(n, e)| Address::new(*n, *e)).collect::<Vec<_>>();
    let m = Message {
        account,
        folder,
        from: Address::new("Avery Quinn", "avery@lumen.example"),
        to: list(to),
        cc: list(cc),
        date,
        subject: "x".into(),
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap();
}

fn found(s: &Session, q: &str) -> Vec<String> {
    super::compose::suggest(s, q, 20).into_iter().map(|x| x.email).collect()
}

/// A fresh session over the same mailbox: nothing kept, so what it says is a count made afresh.
fn fresh(s: &Session) -> Session {
    Session::at(s.mailbox.clone(), now(), 0)
}

#[test]
fn kept_counts_give_the_answers_of_a_fresh_count() {
    let mut s = session();
    let day = 86_400;
    // Rae: three sends (one with To and Cc both), Ron: one, Rue: none (only mail from her).
    post(&mut s, FolderKind::Sent, &[("Rae", "rae@example.org")], &[("Rae", "RAE@example.org")], now() - day);
    post(&mut s, FolderKind::Sent, &[("Rae", "rae@example.org"), ("Ron", "ron@example.org")], &[], now() - 2 * day);
    post(&mut s, FolderKind::Outbox, &[("Rae", "rae@example.org")], &[], now() - 3 * day);
    post(&mut s, FolderKind::Junk, &[("Rue", "rue@example.org")], &[], now() - 10);
    post(&mut s, FolderKind::Drafts, &[("Rue", "rue@example.org")], &[], now() - 10);
    for q in ["r", "ra", "ro", "ru", "example", "rae@", "zzz", ""] {
        let first = found(&s, q);
        // A second press, from what was kept, and the same asked of a session with nothing kept.
        assert_eq!(found(&s, q), first, "{q}");
        assert_eq!(found(&fresh(&s), q), first, "{q}");
    }
    assert_eq!(found(&s, "r").first().map(String::as_str), Some("rae@example.org"));
}

#[test]
fn a_send_between_key_presses_is_counted() {
    let mut s = session();
    post(&mut s, FolderKind::Sent, &[("Rae", "rae@example.org")], &[], now() - 100);
    post(&mut s, FolderKind::Sent, &[("Ron", "ron@example.org")], &[], now() - 200);
    assert_eq!(found(&s, "r"), ["rae@example.org", "ron@example.org"]);
    // Two sends to Ron through the commands, as the owner would.
    for _ in 0..2 {
        s.run("mail.new", &json!({"to": "ron@example.org", "subject": "Hi"})).unwrap();
        s.run("compose.send", &json!({})).unwrap();
    }
    assert_eq!(found(&s, "r"), ["ron@example.org", "rae@example.org"]);
}

#[test]
fn undoing_a_send_uncounts_it() {
    let mut s = session();
    post(&mut s, FolderKind::Sent, &[("Rae", "rae@example.org")], &[], now() - 100);
    post(&mut s, FolderKind::Sent, &[("Ron", "ron@example.org")], &[], now() - 200);
    s.run("mail.new", &json!({"to": "ron@example.org", "subject": "Hi"})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    assert_eq!(found(&s, "r"), ["ron@example.org", "rae@example.org"]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(found(&s, "r"), ["rae@example.org", "ron@example.org"]);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(found(&s, "r"), ["ron@example.org", "rae@example.org"]);
}

#[test]
fn a_mailbox_changed_from_outside_or_replaced_is_counted_afresh() {
    let mut s = session();
    post(&mut s, FolderKind::Sent, &[("Rae", "rae@example.org")], &[], now() - 100);
    assert_eq!(found(&s, "r"), ["rae@example.org"]);
    // Mail that arrived from a server (a send to Ron seen in Sent Items), then another mailbox.
    post(&mut s, FolderKind::Sent, &[("Ron", "ron@example.org")], &[], now() - 50);
    s.mailbox_changed();
    assert_eq!(found(&s, "r"), ["ron@example.org", "rae@example.org"]);
    let mut other = mailcraft_model::Mailbox::with_local_account("Avery Quinn", "avery@lumen.example");
    let account = other.accounts[0].id;
    let sent = other.folder_of_kind(account, FolderKind::Sent).unwrap();
    other.add_message(Message { account, folder: sent, to: vec![Address::new("Rue", "rue@example.org")], date: 5, ..Default::default() }).unwrap();
    s.replace_mailbox(other);
    assert_eq!(found(&s, "r"), ["rue@example.org"]);
}

#[test]
fn the_kept_counts_stay_capped() {
    let mut s = session();
    let n = super::compose::MAX_SUGGEST_TRACKED.saturating_add(10);
    for i in 0..n {
        post(&mut s, FolderKind::Sent, &[("", &format!("s{i}@lists.example"))], &[], 1_500);
    }
    assert!(found(&s, "s1").len() <= 20);
    let kept = s.send_counts.lock().unwrap();
    let (_, counts) = kept.as_ref().unwrap();
    assert_eq!(counts.len(), super::compose::MAX_SUGGEST_TRACKED);
}
