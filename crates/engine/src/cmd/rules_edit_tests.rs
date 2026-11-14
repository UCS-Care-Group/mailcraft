// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Basic tests for rules.edit command.

use mailcraft_model::{Action, Condition, FolderKind, Keep, Rule, ServerRule};
use serde_json::json;

use crate::Session;

const EMAIL: &str = "owner@example.org";

fn session_with_microsoft_account() -> (Session, u64) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": EMAIL, "kind": "microsoft"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    (s, account)
}

fn mark_folder_on_server(s: &mut Session, folder_id: u64, server_id: &str) {
    s.mailbox.folder_mut(folder_id).unwrap().server =
        Some(mailcraft_model::ServerFolder { id: server_id.into(), name: server_id.into(), ..Default::default() });
}

#[test]
fn rules_edit_applies_all_fields_before_server_change() {
    // Test that rules.edit applies all edits before handling server location change.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mark_folder_on_server(&mut s, inbox, "in");
    let news = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    mark_folder_on_server(&mut s, news, "arc");

    // Add a rule on this computer first
    let rule_id = s
        .run(
            "rules.add",
            &json!({
                "name": "Test Rule",
                "account": account,
                "conditions": [{"type": "fromContains", "value": "test@example.com"}],
                "actions": [{"type": "moveTo", "value": inbox}],
                "server": false
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();

    // Edit name, conditions, actions AND change server location in one call
    let result = s.run(
        "rules.edit",
        &json!({
            "id": rule_id,
            "name": "Updated name",
            "conditions": [{"type": "fromContains", "value": "new@example.com"}],
            "actions": [{"type": "moveTo", "value": news}],
            "stop": true,
            "server": true  // Move to server
        }),
    );

    assert!(result.is_ok(), "rules.edit failed: {:?}", result);
    let updated = s.mailbox.rules.iter().find(|rule| rule.id == rule_id).unwrap();
    assert_eq!(updated.name, "Updated name");
    assert_eq!(updated.conditions.len(), 1);
    assert_eq!(updated.conditions[0], Condition::FromContains("new@example.com".to_string()));
    assert!(updated.stop);
    assert_eq!(updated.keep, Keep::Server, "Rule should be on server now");
}

#[test]
fn rules_edit_rejects_non_bool_server() {
    // Test that rules.edit fails with non-bool server value.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mark_folder_on_server(&mut s, inbox, "in");

    // Add a rule
    let rule_id = s
        .run(
            "rules.add",
            &json!({
                "name": "Test Rule",
                "account": account,
                "conditions": [{"type": "fromContains", "value": "test@example.com"}],
                "actions": [{"type": "moveTo", "value": inbox}],
                "server": false
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();

    // Try with a string instead of bool
    let result = s.run(
        "rules.edit",
        &json!({
            "id": rule_id,
            "name": "Should fail",
            "server": "false"  // String, not bool
        }),
    );

    // Should be rejected because server: "false" (string) can't be parsed as bool
    assert!(result.is_err(), "Should reject non-bool server value");
}

#[test]
fn rules_edit_moves_here_to_server() {
    // Test that rules.edit handles server: true correctly for rules on this computer.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mark_folder_on_server(&mut s, inbox, "in");

    // Add a rule on this computer
    let rule_id = s
        .run(
            "rules.add",
            &json!({
                "name": "Test Rule",
                "account": account,
                "conditions": [{"type": "fromContains", "value": "test@example.com"}],
                "actions": [{"type": "moveTo", "value": inbox}],
                "server": false
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();

    // Make sure it's here now
    assert_eq!(s.mailbox.rules.iter().find(|r| r.id == rule_id).unwrap().keep, Keep::Here);

    // Now try to move it to server with an edit
    let result = s.run(
        "rules.edit",
        &json!({
            "id": rule_id,
            "name": "Moved to server",
            "conditions": [{"type": "fromContains", "value": "server@example.com"}],
            "actions": [{"type": "moveTo", "value": inbox}],
            "server": true
        }),
    );

    assert!(result.is_ok(), "rules.edit failed: {:?}", result);
    let updated = s.mailbox.rules.iter().find(|rule| rule.id == rule_id).unwrap();
    assert_eq!(updated.name, "Moved to server");
    assert_eq!(updated.keep, Keep::Server, "Rule should be on server now");
}

#[test]
fn rules_edit_with_no_server_param_updates_in_place() {
    // Test that rules.edit handles empty params (no server change) correctly.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mark_folder_on_server(&mut s, inbox, "in");

    // Add a rule
    let rule_id = s
        .run(
            "rules.add",
            &json!({
                "name": "Test Rule",
                "account": account,
                "conditions": [{"type": "fromContains", "value": "test@example.com"}],
                "actions": [{"type": "moveTo", "value": inbox}],
                "server": false
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();

    let result = s.run(
        "rules.edit",
        &json!({
            "id": rule_id,
            "name": "Name changed"
        }),
    );

    assert!(result.is_ok(), "rules.edit failed: {:?}", result);
    let updated = s.mailbox.rules.iter().find(|rule| rule.id == rule_id).unwrap();
    assert_eq!(updated.name, "Name changed");
    assert_eq!(updated.keep, Keep::Here, "Server location should not change");
}

#[test]
fn rules_edit_server_false_deletes_from_server() {
    // Test that rules.edit with server: false tells server to delete the rule.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mark_folder_on_server(&mut s, inbox, "in");

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
        server: Some(ServerRule { id: "rule-1".to_string(), sequence: 1, ..Default::default() }),
        ..Default::default()
    };
    s.mailbox.rules.push(rule);

    // Verify rule is on server
    let r = s.mailbox.rules.iter().find(|x| x.id == 1).unwrap();
    assert_eq!(r.keep, Keep::Server);
    assert!(r.on_server());

    // Edit with server: false to move it to this computer
    let result = s.run(
        "rules.edit",
        &json!({
            "id": 1,
            "name": "Local Rule",
            "server": false
        }),
    );

    assert!(result.is_ok(), "rules.edit failed: {:?}", result);

    // Verify rule is now local
    let r = s.mailbox.rules.iter().find(|x| x.id == 1).unwrap();
    assert_eq!(r.keep, Keep::Here, "Rule should be on this computer now");
    assert!(r.server.is_none(), "Server should be cleared");

    // Verify server is told to delete the rule
    assert_eq!(s.mailbox.rule_gone.len(), 1, "Server should be told to delete the rule");
    assert_eq!(s.mailbox.rule_gone[0].id, "rule-1", "Server rule id should match");
}

#[test]
fn rules_edit_server_rule_with_unknown_parts_refused() {
    // Server rules with unknown parts can only be switched on/off, moved or deleted - not rewritten.
    let (mut s, account) = session_with_microsoft_account();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mark_folder_on_server(&mut s, inbox, "in");

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
        server: Some(ServerRule { id: "rule-1".to_string(), sequence: 1, unknown: vec!["forwardTo".to_string()], ..Default::default() }),
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
