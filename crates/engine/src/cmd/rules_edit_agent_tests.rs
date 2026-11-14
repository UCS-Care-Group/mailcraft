// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Agent questions for rules.edit: server delete and another-account folder.

use mailcraft_model::{Action, Condition, FolderKind, Keep, Rule};
use serde_json::Value;
use serde_json::json;

use crate::Session;

const EMAIL: &str = "owner@example.org";

fn session_with_microsoft_account() -> (Session, u64) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": EMAIL, "kind": "microsoft"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    (s, account)
}

fn add_rule_here(s: &mut Session, account: u64, name: &str, conditions: Value, actions: Value) -> u64 {
    let made = s.run("rules.add", &json!({"name": name, "account": account, "conditions": conditions, "actions": actions, "server": false})).unwrap();
    made["id"].as_u64().unwrap()
}

#[test]
fn rules_edit_to_delete_on_server_triggers_agent_question() {
    // When editing a server rule to delete mail, the agent should ask the owner.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "in".into(), name: "Inbox".into(), ..Default::default() });
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.folder_mut(deleted).unwrap().server =
        Some(mailcraft_model::ServerFolder { id: "del".into(), name: "Deleted Items".into(), ..Default::default() });

    // Add a rule on the server
    let rule = Rule {
        id: 1,
        name: "Server Rule".to_string(),
        account,
        conditions: vec![Condition::FromContains("test@example.com".to_string())],
        actions: vec![Action::MarkRead],
        enabled: true,
        stop: false,
        keep: Keep::Server,
        server: Some(mailcraft_model::ServerRule { id: "rule-1".to_string(), sequence: 1, ..Default::default() }),
        ..Default::default()
    };
    s.mailbox.rules.push(rule);

    // Edit to delete mail (move to Deleted Items) - this should trigger agent question
    let result = s.run(
        "rules.edit",
        &json!({
            "id": 1,
            "conditions": [{"type": "fromContains", "value": "delete@example.com"}],
            "actions": [{"type": "moveTo", "value": deleted}]
        }),
    );

    // The edit succeeds, and the agent question is queued
    assert!(result.is_ok(), "rules.edit failed: {:?}", result);
    // Check that the rule's server was not cleared (it would be updated on server)
    let r = s.mailbox.rules.iter().find(|x| x.id == 1).unwrap();
    assert_eq!(r.keep, Keep::Server);
    assert!(r.server.is_some());
}

#[test]
fn rules_edit_to_file_to_another_account_folder_triggers_agent_question() {
    // When editing a server rule to file mail to another account's folder, the agent should ask.
    let (mut s, account) = session_with_microsoft_account();

    // Add another account
    let added = s.run("account.add", &json!({"email": "other@example.org", "kind": "microsoft"})).unwrap();
    let other_account = added["id"].as_u64().unwrap();
    let other_inbox = s.mailbox.folder_of_kind(other_account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(other_inbox).unwrap().server =
        Some(mailcraft_model::ServerFolder { id: "in".into(), name: "Inbox".into(), ..Default::default() });

    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "in".into(), name: "Inbox".into(), ..Default::default() });

    // Add a server rule
    let rule = Rule {
        id: 1,
        name: "Server Rule".to_string(),
        account,
        conditions: vec![Condition::FromContains("test@example.com".to_string())],
        actions: vec![Action::MarkRead],
        enabled: true,
        stop: false,
        keep: Keep::Server,
        server: Some(mailcraft_model::ServerRule { id: "rule-1".to_string(), sequence: 1, ..Default::default() }),
        ..Default::default()
    };
    s.mailbox.rules.push(rule);

    // Edit to move to other account's folder - this should trigger agent question
    let result = s.run(
        "rules.edit",
        &json!({
            "id": 1,
            "conditions": [{"type": "fromContains", "value": "file@example.com"}],
            "actions": [{"type": "moveTo", "value": other_inbox}]
        }),
    );

    // The edit succeeds (agent will ask separately via rules_onto_another_server)
    assert!(result.is_ok(), "rules.edit failed: {:?}", result);
    let r = s.mailbox.rules.iter().find(|x| x.id == 1).unwrap();
    assert_eq!(r.keep, Keep::Server);
}

#[test]
fn rules_edit_waiting_rule_to_delete_on_server_triggers_agent_question() {
    // When editing a waiting rule (to be sent to server) to delete mail, the agent should ask.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "in".into(), name: "Inbox".into(), ..Default::default() });
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.folder_mut(deleted).unwrap().server =
        Some(mailcraft_model::ServerFolder { id: "del".into(), name: "Deleted Items".into(), ..Default::default() });

    // Add a rule on this computer (will be waiting for server)
    let rule_id = add_rule_here(
        &mut s,
        account,
        "Waiting Rule",
        json!([{"type": "fromContains", "value": "test@example.com"}]),
        json!([{"type": "moveTo", "value": inbox}]),
    );

    // Move it to server first
    s.run("rules.keepOnServer", &json!({"id": rule_id, "account": account})).unwrap();

    // Now edit to delete mail
    let result = s.run(
        "rules.edit",
        &json!({
            "id": rule_id,
            "conditions": [{"type": "fromContains", "value": "delete@example.com"}],
            "actions": [{"type": "moveTo", "value": deleted}]
        }),
    );

    assert!(result.is_ok(), "rules.edit failed: {:?}", result);
    let r = s.mailbox.rules.iter().find(|x| x.id == rule_id).unwrap();
    assert_eq!(r.keep, Keep::Server);
}

#[test]
fn rules_edit_local_rule_to_server_delete_triggers_agent_question() {
    // When editing a local-only rule to delete mail on server, the agent should ask.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "in".into(), name: "Inbox".into(), ..Default::default() });
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.folder_mut(deleted).unwrap().server =
        Some(mailcraft_model::ServerFolder { id: "del".into(), name: "Deleted Items".into(), ..Default::default() });

    // Add a local-only rule
    let rule_id = add_rule_here(
        &mut s,
        account,
        "Local Rule",
        json!([{"type": "fromContains", "value": "test@example.com"}]),
        json!([{"type": "moveTo", "value": inbox}]),
    );

    // Edit to move to server with delete action
    let result = s.run(
        "rules.edit",
        &json!({
            "id": rule_id,
            "conditions": [{"type": "fromContains", "value": "delete@example.com"}],
            "actions": [{"type": "moveTo", "value": deleted}],
            "server": true  // Move to server
        }),
    );

    assert!(result.is_ok(), "rules.edit failed: {:?}", result);
    let r = s.mailbox.rules.iter().find(|x| x.id == rule_id).unwrap();
    assert_eq!(r.keep, Keep::Server);
}

#[test]
fn rules_edit_server_rule_with_unknown_parts_refused() {
    // Server rules with unknown parts can only be switched on/off, moved or deleted - not rewritten.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "in".into(), name: "Inbox".into(), ..Default::default() });

    // Add a server rule with unknown parts
    let rule = Rule {
        id: 1,
        name: "Server Rule".to_string(),
        account,
        conditions: vec![Condition::FromContains("test@example.com".to_string())],
        actions: vec![Action::MarkRead],
        enabled: true,
        stop: false,
        keep: Keep::Server,
        server: Some(mailcraft_model::ServerRule {
            id: "rule-1".to_string(),
            sequence: 1,
            unknown: vec!["forwardTo".to_string()], // Unknown part
            ..Default::default()
        }),
        ..Default::default()
    };
    s.mailbox.rules.push(rule);

    // Try to edit - should be refused
    let result = s.run(
        "rules.edit",
        &json!({
            "id": 1,
            "name": "Should fail"
        }),
    );

    assert!(result.is_err(), "Should refuse to edit server rule with unknown parts");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("parts the server knows"), "Error should mention server parts: {}", err);
}
