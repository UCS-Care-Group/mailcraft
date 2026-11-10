// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent doesn't change what the owner is writing, nor empty it and save that over its draft
//! (#577): once the owner has written in a message, it is theirs until it is closed (#494). A
//! message only the agent wrote stays the agent's to edit (#442).

use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// Run as an agent's command runs (`Session::agent`), allowed nothing in advance.
fn as_agent(s: &mut Session, id: &str, params: Value) -> Result<Value, String> {
    s.agent = Some(Vec::new());
    let r = s.run(id, &params);
    s.agent = None;
    r.map_err(|e| e.to_string())
}

/// The owner has written a message and saved it to Drafts.
fn owner_wrote() -> Session {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"to": "ada@example.org", "subject": "Mine", "body": "Half a thought"}));
    run(&mut s, "compose.type", json!({"text": " and more"}));
    run(&mut s, "compose.saveDraft", json!({}));
    assert!(s.compose.as_ref().unwrap().owner_changed);
    s
}

fn body(s: &Session) -> String {
    s.compose.as_ref().unwrap().body.plain()
}

#[test]
fn an_agent_cannot_empty_the_owners_message_and_save_it_over_the_draft() {
    let mut s = owner_wrote();
    let draft = s.compose.as_ref().unwrap().draft.unwrap();
    let written = body(&s);
    assert!(written.contains("Half a thought"));
    let changes = [
        ("compose.set", json!({"body": ""})),
        ("compose.set", json!({"to": "", "subject": ""})),
        ("compose.delete", json!({"from": 0, "to": 5})),
        ("edit.cut", json!({"from": 0, "to": 5})),
        ("compose.undo", json!({})),
        ("compose.type", json!({"text": "x"})),
        ("compose.highImportance", json!({})),
    ];
    for (id, params) in changes {
        let e = as_agent(&mut s, id, params.clone()).expect_err(&format!("{id} {params}"));
        assert!(e.contains("the message the owner is writing"), "{id}: {e}");
        assert_eq!(body(&s), written, "{id}");
        assert_eq!(s.compose.as_ref().unwrap().subject, "Mine", "{id}");
    }
    // Saving it saves only what the owner wrote: the draft keeps their text.
    as_agent(&mut s, "compose.saveDraft", json!({})).unwrap();
    assert_eq!(s.mailbox.message(draft).unwrap().subject, "Mine");
    let saved = s.loaded_message(draft).unwrap();
    assert!(saved.body.text.contains("Half a thought"), "{}", saved.body.text);
    // Moving the cursor isn't writing in it.
    as_agent(&mut s, "compose.select", json!({"from": 0, "to": 3})).unwrap();
}

#[test]
fn a_message_only_the_agent_wrote_is_still_its_own_to_edit() {
    let mut s = session();
    as_agent(&mut s, "mail.new", json!({})).unwrap();
    as_agent(&mut s, "compose.set", json!({"subject": "The agent's", "body": "Written by the agent"})).unwrap();
    as_agent(&mut s, "compose.set", json!({"body": ""})).unwrap();
    as_agent(&mut s, "compose.saveDraft", json!({})).unwrap();
    assert_eq!(body(&s), "");
}

#[test]
fn the_owner_still_changes_their_own_message() {
    let mut s = owner_wrote();
    run(&mut s, "compose.set", json!({"body": ""}));
    assert_eq!(body(&s), "");
}
