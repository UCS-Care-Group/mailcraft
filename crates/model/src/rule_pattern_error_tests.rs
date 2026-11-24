// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::rule::Rule;
use crate::{Action, Condition, Mailbox, Pattern};

/// Test that invalid regex patterns are reported during mailbox repair
#[test]
fn invalid_regex_patterns_are_reported() {
    // Create a mailbox with an invalid regex pattern
    let mut mailbox = Mailbox::default();

    mailbox.rules.push(Rule {
        id: 1,
        name: "Invalid pattern rule".to_string(),
        enabled: true,
        conditions: vec![Condition::SubjectMatches(Pattern::uncompiled("invalid regex (".to_string()))],
        actions: vec![Action::MarkRead],
        ..Default::default()
    });

    // Before repair, the pattern should not be compiled
    if let Some(rule) = mailbox.rules.first()
        && let Condition::SubjectMatches(p) = &rule.conditions[0]
    {
        assert!(!p.is_compiled(), "Pattern should not be compiled before repair");
    }

    // Repair should report the invalid pattern
    let fixed = mailbox.repair();

    // Verify that an error message about the invalid pattern was added
    assert!(fixed.iter().any(|msg| msg.contains("patterns could not be compiled")), "Fixed messages: {:?}", fixed);
}

/// Test that patterns at the exact MAX_PATTERN_LENGTH limit are accepted
#[test]
fn pattern_at_exactly_max_length_is_accepted() {
    let max_pattern = "a".repeat(200); // Exactly MAX_PATTERN_LENGTH

    // This should succeed
    let pattern = Pattern::new(max_pattern.clone());
    assert!(pattern.is_some(), "Pattern at exactly 200 chars should be accepted");
}

/// Test that patterns over MAX_PATTERN_LENGTH are rejected
#[test]
fn pattern_over_max_length_is_rejected() {
    let over_max = "a".repeat(201); // One char over MAX_PATTERN_LENGTH

    // This should fail
    let pattern = Pattern::new(over_max);
    assert!(pattern.is_none(), "Pattern over 200 chars should be rejected");
}

/// Test that valid patterns are compiled and invalid ones are reported
#[test]
fn mixed_valid_invalid_patterns_report_errors() {
    let mut mailbox = Mailbox::default();

    // Add a valid pattern
    mailbox.rules.push(Rule {
        id: 1,
        name: "Valid pattern rule".to_string(),
        enabled: true,
        conditions: vec![Condition::SubjectMatches(Pattern::uncompiled("test".to_string()))],
        actions: vec![Action::MarkRead],
        ..Default::default()
    });

    // Add an invalid pattern
    mailbox.rules.push(Rule {
        id: 2,
        name: "Invalid pattern rule".to_string(),
        enabled: true,
        conditions: vec![Condition::SubjectMatches(Pattern::uncompiled("invalid (".to_string()))],
        actions: vec![Action::MarkRead],
        ..Default::default()
    });

    // Repair should compile the valid one and report the invalid one
    let fixed = mailbox.repair();

    // Check that valid pattern is compiled
    if let Some(rule) = mailbox.rules.first()
        && let Condition::SubjectMatches(p) = &rule.conditions[0]
    {
        assert!(p.is_compiled(), "Valid pattern should be compiled");
    }

    // Check that invalid pattern is not compiled
    if let Some(rule) = mailbox.rules.get(1)
        && let Condition::SubjectMatches(p) = &rule.conditions[0]
    {
        assert!(!p.is_compiled(), "Invalid pattern should not be compiled");
    }

    // Verify error message was added
    assert!(fixed.iter().any(|msg| msg.contains("patterns could not be compiled")), "Fixed messages: {:?}", fixed);
}
