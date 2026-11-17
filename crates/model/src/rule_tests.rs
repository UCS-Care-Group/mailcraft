// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::*;

#[test]
fn conditions_all_must_hold() {
    let m = Message {
        from: Address::new("Weekly Digest", "news@example.org"),
        to: vec![Address::new("", "me@example.org")],
        subject: "Your October digest".into(),
        attachments: vec![Attachment::new("a.pdf", "application/pdf", vec![1])],
        ..Default::default()
    };
    let mut rule = Rule {
        enabled: true,
        conditions: vec![Condition::FromContains("DIGEST".into()), Condition::HasAttachment],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    assert!(rule.matches(&m));
    rule.conditions.push(Condition::SubjectContains("invoice".into()));
    assert!(!rule.matches(&m));
    rule.conditions.pop();
    rule.enabled = false;
    assert!(!rule.matches(&m));
    // No conditions: matches nothing, not everything.
    assert!(!Rule { enabled: true, ..Default::default() }.matches(&m));
    assert!(Condition::ToContains("me@".into()).matches(&m) && !Condition::Importance(Importance::High).matches(&m));
    assert_eq!(
        Rule { conditions: vec![Condition::HasAttachment], actions: vec![Action::Flag, Action::MarkRead], ..Default::default() }.describe(),
        "When has an attachment: flag it, mark it as read"
    );
}

#[test]
fn rules_round_trip_as_json() {
    let rule = Rule {
        id: 4,
        name: "News".into(),
        enabled: true,
        conditions: vec![Condition::FromContains("news".into())],
        actions: vec![Action::MoveTo(9), Action::Categorize("Blue".into())],
        stop: true,
        ..Default::default()
    };
    let json = serde_json::to_string(&rule).unwrap();
    assert!(json.contains(r#""type":"moveTo","value":9"#), "{json}");
    assert_eq!(serde_json::from_str::<Rule>(&json).unwrap(), rule);
}

/// Tests that a rule loaded from an older version (with empty synced_* fields)
/// is not treated as changed until the next full read from server populates synced_*.
#[test]
fn old_server_rule_with_empty_synced_values_is_not_treated_as_changed() {
    let account = 1;
    // Create a server rule as it would be in an older mailbox (empty synced_* values)
    let rule = Rule {
        id: 1,
        name: "Old Rule".into(),
        enabled: true,
        conditions: vec![Condition::FromContains("test@example.com".into())],
        actions: vec![Action::MarkRead],
        stop: false,
        account,
        keep: crate::Keep::Server,
        server: Some(ServerRule {
            id: "rule-id".into(),
            sequence: 1,
            // Empty synced_* values (as in older saved mailboxes)
            synced_name: String::new(),
            synced_conditions: Vec::new(),
            synced_actions: Vec::new(),
            synced_stop: false,
            synced_enabled: false,
            synced_sequence: 0,
            ..Default::default()
        }),
        ..Default::default()
    };

    // synced_empty should return true for this rule
    assert!(rule.synced_empty(), "synced values should be empty");

    // changed_here should return false because synced values are empty
    assert!(!rule.changed_here(), "rule should not be treated as changed when synced values are empty");

    // Now update the rule locally and also update synced_* to simulate what was sent
    let mut rule = rule;
    rule.enabled = false; // Changed locally
    rule.name = "Updated Rule".into(); // Changed locally
    // Update synced_* to reflect what was sent (to simulate the state after send but before answer)
    if let Some(ref mut server) = rule.server {
        server.synced_name = "Old Rule".into();
        server.synced_enabled = true;
        server.synced_sequence = 1;
    }

    // Now changed_here should return true (we have synced values to compare against)
    assert!(rule.changed_here(), "rule should be treated as changed after local edits");
}

/// Test that a rule with no synced values can still be identified as a server rule
#[test]
fn server_rule_with_empty_synced_values_is_still_on_server() {
    let rule = Rule { id: 1, name: "Rule".into(), server: Some(ServerRule { id: "id".into(), ..Default::default() }), ..Default::default() };

    assert!(rule.on_server(), "rule should be on server even with empty synced values");
    assert!(rule.synced_empty(), "synced values are empty");
    assert!(!rule.changed_here(), "not changed when synced values are empty");
}
