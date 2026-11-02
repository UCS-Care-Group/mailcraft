// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pop Out (#286): `compose.popOut` names the message being written that the window moves into a
//! window of its own, and changes nothing of it or of the mailbox.

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

#[test]
fn pop_out_answers_the_message_it_is_for_and_changes_nothing() {
    let mut s = session();
    // Nothing being written: there is nothing to pop out.
    assert!(!s.enabled("compose.popOut"));
    assert!(s.run("compose.popOut", &json!({})).is_err());
    let first = run(&mut s, "mail.new", json!({"subject": "First"}))["compose"].as_u64().unwrap();
    run(&mut s, "compose.set", json!({"body": "Some words"}));
    let second = run(&mut s, "mail.new", json!({"subject": "Second"}))["compose"].as_u64().unwrap();
    assert!(s.enabled("compose.popOut"));
    let (rev, before) = (s.rev(), s.all_composing().cloned().collect::<Vec<_>>());
    // The current one, by default; the one named, by its id.
    assert_eq!(run(&mut s, "compose.popOut", json!({})), json!({"compose": second, "popOut": true}));
    assert_eq!(run(&mut s, "compose.popOut", json!({"compose": first})), json!({"compose": first, "popOut": true}));
    // Nothing of either changed, nor which is current, nor the mailbox, nor what Undo would undo.
    assert_eq!(s.all_composing().cloned().collect::<Vec<_>>(), before);
    assert_eq!((s.compose_id(), s.rev()), (Some(second), rev));
    assert!(!s.registry.get("compose.popOut").unwrap().mutates);
    // One that isn't open, or isn't an id, is refused.
    for bad in [json!({"compose": 999}), json!({"compose": "1"}), json!({"compose": -1}), json!({"compose": 1.5})] {
        assert!(s.run("compose.popOut", &bad).is_err(), "{bad}");
    }
}

#[test]
fn an_agent_names_the_message_to_pop_out_once_more_than_one_is_open() {
    let mut s = session();
    let first = run(&mut s, "mail.new", json!({}))["compose"].as_u64().unwrap();
    s.agent = Some(Vec::new());
    // One open: it is that one.
    assert_eq!(s.run("compose.popOut", &json!({})).unwrap()["compose"], first);
    s.agent = None;
    run(&mut s, "mail.new", json!({}));
    s.agent = Some(Vec::new());
    assert!(s.run("compose.popOut", &json!({})).is_err(), "ambiguous");
    assert_eq!(s.run("compose.popOut", &json!({"compose": first})).unwrap()["compose"], first);
    s.agent = None;
}

#[test]
fn pop_out_is_in_the_catalog_and_new_email_opens_in_a_window() {
    let rows: Vec<_> = crate::catalog::features().filter(|f| f.id == "compose.popOut" || f.name.contains("Window of Its Own")).collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|f| f.name == "Pop Out" && f.tab == "Message"));
    assert!(rows.iter().any(|f| f.id == "mail.new"));
}
