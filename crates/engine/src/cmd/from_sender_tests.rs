// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages from Sender (`mail.findFromSender`, #301).

use mailcraft_model::{FolderKind, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::*;

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn senders(s: &mut Session) -> Vec<String> {
    let rows = s.rows();
    rows.iter().map(|r| s.mailbox.message(r.id).unwrap().from.email.clone()).collect()
}

/// The sender of the selected message, or of the one `id` names, by address, in every mailbox.
#[test]
fn mail_from_the_sender_of_the_selected_message_or_of_the_one_named() {
    let mut s = session();
    let first_id = s.rows()[0].id;
    run(&mut s, "mail.select", json!({"id": first_id}));
    let sender = s.mailbox.message(first_id).unwrap().from.email.clone();
    let said = run(&mut s, "mail.findFromSender", json!({}));
    assert_eq!(said["search"], format!("from:{sender}"));
    assert_eq!(s.view.scope, crate::Scope::Everywhere);
    let found = senders(&mut s);
    assert!(!found.is_empty() && said["found"] == found.len());
    assert!(found.iter().all(|f| f.eq_ignore_ascii_case(&sender)));

    // Another message, named by its id; the selection is left as it is.
    run(&mut s, "view.clearSearch", json!({}));
    let other: Id = s.mailbox.messages.values().find(|m| !m.from.email.eq_ignore_ascii_case(&sender)).unwrap().id;
    let other_sender = s.mailbox.message(other).unwrap().from.email.clone();
    let said = run(&mut s, "mail.findFromSender", json!({"id": other}));
    assert_eq!(said["search"], format!("from:{other_sender}"));
    assert!(senders(&mut s).iter().all(|f| f.eq_ignore_ascii_case(&other_sender)));
    assert_eq!(s.sel.focus(), Some(first_id));

    // No such message, or one with no sender address: said, and the search is as it was.
    assert!(s.run("mail.findFromSender", &json!({"id": 999_999})).is_err());
    s.mailbox.message_mut(other).unwrap().from.email = "ada@[192.0.2.1]".into();
    let said = s.run("mail.findFromSender", &json!({"id": other})).unwrap_err().to_string();
    assert!(said.contains("as a whole address"), "{said}");
    s.mailbox.message_mut(other).unwrap().from.email.clear();
    let said = s.run("mail.findFromSender", &json!({"id": other})).unwrap_err().to_string();
    assert!(said.contains("no sender address"), "{said}");
    assert_eq!(s.view.search, format!("from:{other_sender}"));
}

/// A quoted name can hold spaces and quotes: it is written so that the search reads it back as
/// that one address, and nothing in it becomes a term of its own.
#[test]
fn a_quoted_address_is_searched_for_whole() {
    let mut s = session();
    let inbox = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Inbox).unwrap();
    let ids: Vec<Id> = s.mailbox.messages.values().filter(|m| m.folder == inbox).map(|m| m.id).take(4).collect();
    let quoted = r#""ada \"x\" from:eve@example.org"@example.org"#;
    for (id, email) in ids.iter().zip([quoted, quoted, "ada@example.org", "eve@example.org"]) {
        s.mailbox.message_mut(*id).unwrap().from.email = email.into();
    }
    let said = run(&mut s, "mail.findFromSender", json!({"id": ids[0]}));
    assert_eq!(said["search"], r#"from:"\"ada \\\"x\\\" from:eve@example.org\"@example.org""#);
    assert_eq!(senders(&mut s), [quoted, quoted]);
    // One the search can't read back (a control character in its name) is said instead.
    run(&mut s, "view.clearSearch", json!({}));
    s.mailbox.message_mut(ids[2]).unwrap().from.email = "\"a\u{1}b\"@example.org".into();
    assert!(s.run("mail.findFromSender", &json!({"id": ids[2]})).is_err());
    assert!(s.view.search.is_empty());
}

/// Find Related writes the subject so that the search reads it back as it is: a backslash at its
/// end (`Backup C:\`) doesn't take in what is typed after it, now that `\"` is a quote inside
/// quotes (#301).
#[test]
fn find_related_with_a_backslash_at_the_end_of_the_subject() {
    let mut s = session();
    let inbox = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Inbox).unwrap();
    let ids: Vec<Id> = s.mailbox.messages.values().filter(|m| m.folder == inbox).map(|m| m.id).take(2).collect();
    for id in &ids {
        s.mailbox.message_mut(*id).unwrap().subject = r"Backup C:\".into();
    }
    run(&mut s, "mail.select", json!({"id": ids[0]}));
    // (Selecting it read it.)
    for (id, read) in ids.iter().zip([false, true]) {
        s.mailbox.message_mut(*id).unwrap().read = read;
    }
    let said = run(&mut s, "mail.findRelated", json!({}));
    assert_eq!(said["search"], r#"subject:"Backup C:\\""#);
    assert_eq!(said["found"], 2);
    let narrowed = format!("{} is:unread", s.view.search);
    assert_eq!(run(&mut s, "view.search", json!({"query": narrowed, "scope": "everywhere"}))["found"], 1);
    let spans = mailcraft_model::search::keyword_spans(&narrowed);
    let keys: Vec<&str> = spans.iter().map(|k| &narrowed[k.key.clone()]).collect();
    assert_eq!(keys, ["subject:", "is:"]);
    assert_eq!(spans.last().map(|k| &narrowed[k.range.clone()]), Some("is:unread"));
}
