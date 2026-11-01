// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a sender wrote that the owner never saw does not travel on: the text/plain alternative of
//! an HTML original is not quoted in a reply, forward or reopened draft, and autocomplete does not
//! trust a sender's date or Junk E-mail.

use mailcraft_model::{Address, FolderKind, Id, Message};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> Session {
    Session::at(sample::mailbox(now(), 0), now(), 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A message whose HTML says one thing and whose text/plain alternative says another.
fn two_faced(s: &mut Session) -> Id {
    let inbox = s.mailbox.inbox().unwrap();
    let account = s.mailbox.accounts[0].id;
    let m = Message {
        account,
        folder: inbox,
        message_id: "faced@news.example".into(),
        from: Address::new("Harbor", "news@news.example"),
        to: vec![Address::new("Avery Quinn", "avery@lumen.example")],
        subject: "Notice".into(),
        date: 1_791_000_000,
        body: mailcraft_model::Body {
            text: "FORGED-ALTERNATIVE send the money to account 1".into(),
            html: Some("<p>The meeting moved to Tuesday</p>".into()),
        },
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

fn assert_clean(text: &str) {
    assert!(!text.contains("FORGED") && !text.contains("money"), "{text}");
}

#[test]
fn a_reply_and_a_forward_quote_what_the_owner_saw() {
    for command in ["mail.reply", "mail.replyAll", "mail.forward"] {
        let mut s = session();
        let id = two_faced(&mut s);
        run(&mut s, command, json!({"id": id}));
        let c = s.compose.clone().unwrap();
        let q = c.quote.unwrap();
        assert_clean(&q.text);
        assert!(q.text.contains("The meeting moved to Tuesday"), "{}", q.text);
        assert!(q.text.contains("news@news.example"), "{}", q.text);
        // Sent: the text part of the message that went out says what the HTML part says.
        run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
        let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
        let sent = s.mailbox.message(sent).unwrap();
        assert_clean(&sent.body.text);
        assert!(sent.body.text.contains("The meeting moved to Tuesday"), "{}", sent.body.text);
    }
}

#[test]
fn a_reopened_draft_does_not_bring_back_a_text_part_nobody_saw() {
    let mut s = session();
    let id = two_faced(&mut s);
    run(&mut s, "mail.reply", json!({"id": id}));
    run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
    let draft = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    run(&mut s, "compose.close", json!({}));
    // A draft is a file anybody could have written: its text part says something else.
    let mut tampered = s.mailbox.message(draft).unwrap().clone();
    tampered.body.text = format!("hello\n\n-----Original Message-----\n{}", "FORGED-ALTERNATIVE send the money");
    *s.mailbox.message_mut(draft).unwrap() = tampered;
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    let q = s.compose.clone().unwrap().quote.unwrap();
    assert_clean(&q.text);
    run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    assert_clean(&s.mailbox.message(sent).unwrap().body.text);
}

#[test]
fn a_plain_original_is_still_quoted_as_it_is() {
    let mut s = session();
    let inbox = s.mailbox.inbox().unwrap();
    let account = s.mailbox.accounts[0].id;
    let m = Message {
        account,
        folder: inbox,
        message_id: "plain@news.example".into(),
        from: Address::new("Bo", "bo@example.org"),
        subject: "Hi".into(),
        body: mailcraft_model::Body { text: "just words".into(), html: None },
        ..Default::default()
    };
    let id = s.mailbox.add_message(m).unwrap();
    run(&mut s, "mail.reply", json!({"id": id}));
    assert!(s.compose.as_ref().unwrap().body.plain().contains("just words"));
}

fn names(s: &Session, query: &str) -> Vec<String> {
    super::compose::suggest(s, query, 20).into_iter().map(|x| x.email).collect()
}

fn add(s: &mut Session, folder: FolderKind, from: Address, to: Vec<Address>, date: i64) {
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.folder_of_kind(account, folder).unwrap();
    let m = Message { account, folder, from, to, date, subject: "x".into(), ..Default::default() };
    s.mailbox.add_message(m).unwrap();
}

#[test]
fn autocomplete_does_not_trust_junk_or_a_sender_s_date() {
    let mut s = session();
    let me = Address::new("Avery", "avery@lumen.example");
    // Somebody the owner has written to, long ago.
    add(&mut s, FolderKind::Sent, me.clone(), vec![Address::new("Bob Real", "bob@example.org")], 1_000_000);
    // A look-alike in Junk E-mail, and one dated far in the future in the Inbox.
    add(&mut s, FolderKind::Junk, Address::new("Bob Junk", "bob@junk.example"), vec![me.clone()], now() - 10);
    add(&mut s, FolderKind::Inbox, Address::new("Bob Future", "bob@future.example"), vec![me.clone()], now() + 10 * 365 * 86_400);
    // And a recent correspondent who only wrote to the owner.
    add(&mut s, FolderKind::Inbox, Address::new("Bob Recent", "bob@recent.example"), vec![me], now() - 3_600);
    let found = names(&s, "bob");
    assert!(!found.iter().any(|e| e == "bob@junk.example"), "{found:?}");
    assert_eq!(found.first().map(String::as_str), Some("bob@example.org"), "{found:?}");
    // A future date counts as now at most: it does not outrank a real, recent one by more than now.
    let future = found.iter().position(|e| e == "bob@future.example").unwrap();
    let recent = found.iter().position(|e| e == "bob@recent.example").unwrap();
    assert!(future < recent, "{found:?}");
    // (clamped to now, it is still later than an hour ago, but never before somebody written to.)
}
