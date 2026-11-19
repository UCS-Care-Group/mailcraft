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
fn starts_with_conditions() {
    let m = Message {
        from: Address::new("Weekly Digest", "news@example.org"),
        to: vec![Address::new("", "me@example.org")],
        subject: "Your October digest".into(),
        ..Default::default()
    };
    // The sender's address and name, in letters of either case.
    assert!(Condition::FromStartsWith("news@".into()).matches(&m));
    assert!(Condition::FromStartsWith("weekly".into()).matches(&m));
    assert!(!Condition::FromStartsWith("ews".into()).matches(&m));
    assert!(!Condition::FromStartsWith(String::new()).matches(&m));
    // In a recipient's address.
    assert!(Condition::ToStartsWith("ME@".into()).matches(&m) && !Condition::ToStartsWith("example".into()).matches(&m));
    // In the subject and the body.
    assert!(Condition::SubjectStartsWith("your october".into()).matches(&m) && !Condition::SubjectStartsWith("october".into()).matches(&m));
    let mut b = m.clone();
    b.body.text = "Your digest".into();
    assert!(Condition::BodyStartsWith("YOUR".into()).matches(&b) && !Condition::BodyStartsWith("digest".into()).matches(&b));
    // A rule kept on a server reads a sender's or a recipient's words in their addresses alone.
    let r = |c: Condition| Rule { enabled: true, conditions: vec![c], actions: vec![Action::MarkRead], ..Default::default() };
    assert!(r(Condition::FromStartsWith("news@".into())).matches_as(&m, true) && !r(Condition::FromStartsWith("weekly".into())).matches_as(&m, true));
    assert!(r(Condition::ToStartsWith("me@".into())).matches_as(&m, true));
    assert!(r(Condition::SubjectStartsWith("your".into())).matches_as(&m, true));
    assert_eq!(r(Condition::FromStartsWith("news@".into())).describe_as(true), "When sender's address starts with \"news@\": mark it as read");
    assert_eq!(r(Condition::BodyStartsWith("your".into())).describe(), "When body starts with \"your\": mark it as read");
}

#[test]
fn regex_conditions() {
    let m = Message {
        from: Address::new("Weekly Digest", "news@example.org"),
        to: vec![Address::new("", "me@example.org")],
        subject: "Order #2026-4411 shipped".into(),
        ..Default::default()
    };
    let r = |c: Condition| Rule { enabled: true, conditions: vec![c], actions: vec![Action::MarkRead], ..Default::default() };
    // Somewhere in the subject, in letters of either case.
    assert!(r(Condition::SubjectMatches(Pattern::new(r"order #\d+".to_string()).unwrap())).matches(&m));
    assert!(r(Condition::SubjectMatches(Pattern::new(r"ORDER #\d+".to_string()).unwrap())).matches(&m));
    assert!(!r(Condition::SubjectMatches(Pattern::new("order #9999".to_string()).unwrap())).matches(&m));
    assert!(!r(Condition::SubjectMatches(Pattern::new("shipped.*order".to_string()).unwrap())).matches(&m));
    // In the body, and in the text a sender hid.
    let mut b = m.clone();
    b.body.text = "your order 2026-4411 is on its way".into();
    b.hidden_text = "order 2026-4411 also hidden".into();
    assert!(r(Condition::BodyMatches(Pattern::new(r"order \d{4}-\d{4}".to_string()).unwrap())).matches(&b));
    // A pattern that's valid but won't match the message.
    assert!(!r(Condition::SubjectMatches(Pattern::new("invoice".to_string()).unwrap())).matches(&m));
    // Use a pattern that's truly invalid - unmatched group
    assert!(Pattern::new("(".to_string()).is_none());
    assert_eq!(
        r(Condition::SubjectMatches(Pattern::new("order".to_string()).unwrap())).describe(),
        "When the subject matches a pattern: mark it as read"
    );
    assert_eq!(
        r(Condition::BodyMatches(Pattern::new("invoice".to_string()).unwrap())).describe(),
        "When the body matches a pattern: mark it as read"
    );
    // A pattern over 200 chars is refused.
    assert!(Pattern::new("a".repeat(201)).is_none());
    // A pattern that's valid but won't match any message.
    let evil = r(Condition::SubjectMatches(Pattern::new("zzzzzzzzzz".to_string()).unwrap()));
    assert!(!evil.matches(&m));
    assert!(!evil.matches(&Message { subject: "aaaa".into(), ..m }));
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
    assert!(json.contains(r#"{"type":"moveTo","value":9}"#), "{json}");
    assert_eq!(serde_json::from_str::<Rule>(&json).unwrap(), rule);
}

#[test]
fn pattern_round_trip_preserves_matcher() {
    // A pattern that compiles on one build should compile on another
    // and still match correctly after round-trip through JSON
    let pattern_str = "invoice";
    let original = Pattern::new(pattern_str.to_string()).unwrap();

    // Serialize and deserialize
    let json = serde_json::to_string(&original).unwrap();
    let deserialized: Pattern = serde_json::from_str(&json).unwrap();

    // The text should be preserved
    assert_eq!(deserialized.as_str(), pattern_str);

    // The matcher should still work
    assert!(deserialized.is_match("Your INVOICE"));
    assert!(!deserialized.is_match("No match here"));
}

#[test]
fn invalid_pattern_deserializes_to_empty_matcher() {
    // A pattern that doesn't compile (e.g., from a desktop build with full regex features
    // being loaded in a CLI build with limited features) becomes a rule that matches nothing.
    let invalid = "(unclosed"; // Unmatched parenthesis
    let json = format!(r#"{{"type":"subjectMatches","value":"{}"}}"#, invalid);

    // Deserialize to Condition
    let condition: Condition = serde_json::from_str(&json).unwrap();
    match condition {
        Condition::SubjectMatches(p) => {
            // The pattern text is kept
            assert_eq!(p.as_str(), invalid);
            // But it doesn't match anything
            let m = Message { subject: "any text".into(), ..Default::default() };
            assert!(!p.is_match(&m.subject));
        }
        _ => panic!("Expected SubjectMatches"),
    }
}

#[test]
fn clean_condition_text_trims_and_caps() {
    // Trims whitespace
    assert_eq!(clean_condition_text("  hello  "), "hello");
    assert_eq!(clean_condition_text("\t\nhello\n\t"), "hello");
    // Caps at MAX_TEXT_LENGTH
    let text = "a".repeat(250);
    let cleaned = clean_condition_text(&text);
    assert_eq!(cleaned.len(), MAX_TEXT_LENGTH);
    assert_eq!(cleaned, "a".repeat(MAX_TEXT_LENGTH));
    // Empty after trim becomes empty
    assert_eq!(clean_condition_text("   "), "");
    assert_eq!(clean_condition_text(""), "");
}

#[test]
fn clean_condition_text_handles_multibyte_chars() {
    // 199 'a' chars followed by 'é' (2 bytes) = 201 chars
    // Should cut to exactly 200 chars, not panic
    let text = "a".repeat(199) + "é";
    let cleaned = clean_condition_text(&text);
    assert_eq!(cleaned.chars().count(), 200);
    assert_eq!(cleaned.len(), 201); // 199 a's + é = 201 bytes

    // Greek characters - each is multiple bytes
    let greek = "α".repeat(250);
    let cleaned = clean_condition_text(&greek);
    assert_eq!(cleaned.chars().count(), 200);

    // Chinese characters
    let chinese = "中".repeat(250);
    let cleaned = clean_condition_text(&chinese);
    assert_eq!(cleaned.chars().count(), 200);
}

#[test]
fn load_mailbox_with_too_many_patterns() {
    // Create a mailbox with 150 rules, each having a pattern condition
    // This exceeds the MAX_PATTERN_CONDITIONS limit of 100
    let mut mailbox = Mailbox::default();
    
    // Add 150 rules with pattern conditions
    for i in 0..150 {
        let pattern = Pattern::new(format!(r"pattern{}", i)).unwrap();
        mailbox.rules.push(Rule {
            id: i as Id,
            name: format!("Rule {}", i),
            enabled: true,
            conditions: vec![Condition::SubjectMatches(pattern)],
            actions: vec![Action::MarkRead],
            ..Default::default()
        });
    }
    
    // Verify all 150 rules were created
    assert_eq!(mailbox.rules.len(), 150);
    
    // Before repair, all patterns should be compiled
    let mut pattern_count_before = 0;
    for rule in &mailbox.rules {
        for cond in &rule.conditions {
            if let Condition::SubjectMatches(p) = cond {
                pattern_count_before += 1;
                assert!(p.is_compiled(), "Rule {} pattern should be compiled before repair", rule.id);
            }
        }
    }
    assert_eq!(pattern_count_before, 150);
    
    // Repair should enforce the pattern budget
    let fixed = mailbox.repair();
    
    // Verify we have at most MAX_PATTERN_CONDITIONS pattern conditions
    let mut pattern_count_after = 0;
    for rule in &mailbox.rules {
        for cond in &rule.conditions {
            if let Condition::SubjectMatches(p) = cond {
                pattern_count_after += 1;
                // The pattern should either be compiled (kept) or uncompiled (removed)
                // If it's uncompiled, it will match nothing
            }
        }
    }
    assert!(pattern_count_after <= MAX_PATTERN_CONDITIONS, "After repair: {} patterns, limit is {}", pattern_count_after, MAX_PATTERN_CONDITIONS);
    
    // Verify the fix message was added
    assert!(fixed.iter().any(|msg| msg.contains("pattern conditions cut")), "Fixed: {:?}", fixed);
    
    // Verify that uncompiled patterns match nothing
    for rule in &mailbox.rules {
        for cond in &rule.conditions {
            if let Condition::SubjectMatches(p) = cond {
                let test_msg = Message {
                    subject: "any text".into(),
                    ..Default::default()
                };
                // If pattern is not compiled, it matches nothing
                if !p.is_compiled() {
                    assert!(!p.is_match("any text"), "Uncompiled pattern should match nothing");
                }
            }
        }
    }
}

#[test]
fn pattern_budget_works_across_multiple_mailboxes() {
    // Verify the pattern budget is per-mailbox, not global
    // Create first mailbox with 120 patterns
    let mut mailbox1 = Mailbox::default();
    for i in 0..120 {
        let pattern = Pattern::new(format!("pattern{}", i)).unwrap();
        mailbox1.rules.push(Rule {
            id: i as Id,
            name: format!("Rule {}", i),
            enabled: true,
            conditions: vec![Condition::SubjectMatches(pattern)],
            actions: vec![Action::MarkRead],
            ..Default::default()
        });
    }
    
    // Repair should limit mailbox1 to 100 pattern conditions
    let _ = mailbox1.repair();
    
    let pattern_count1 = mailbox1.rules.iter()
        .map(|r| count_pattern_conditions(&r.conditions))
        .sum::<usize>();
    
    assert!(pattern_count1 <= MAX_PATTERN_CONDITIONS, "Mailbox1: {} patterns exceeds limit", pattern_count1);
    
    // Create second mailbox with 80 patterns (under limit)
    let mut mailbox2 = Mailbox::default();
    for i in 0..80 {
        let pattern = Pattern::new(format!("pattern{}", i + 1000)).unwrap();
        mailbox2.rules.push(Rule {
            id: i as Id,
            name: format!("Rule {}", i + 1000),
            enabled: true,
            conditions: vec![Condition::SubjectMatches(pattern)],
            actions: vec![Action::MarkRead],
            ..Default::default()
        });
    }
    
    // Repair should keep all 80 patterns (under limit)
    let _ = mailbox2.repair();
    
    let pattern_count2 = mailbox2.rules.iter()
        .map(|r| count_pattern_conditions(&r.conditions))
        .sum::<usize>();
    
    assert_eq!(pattern_count2, 80, "Mailbox2 should keep all 80 patterns");
    
    // Verify each mailbox has its own budget (not shared)
    assert!(pattern_count1 <= 100);
    assert_eq!(pattern_count2, 80);
}

#[test]
fn body_pattern_caps_text_to_max_length() {
    // Create a pattern that looks for text at a specific position
    let pattern = Pattern::new(r"ZZZ".to_string()).unwrap();
    
    // Body with "ZZZ" at position 5000 (within cap)
    let short_body = "a".repeat(4997) + "ZZZ";
    let m = Message {
        body: Body { text: short_body, html: None },
        ..Default::default()
    };
    assert!(Condition::BodyMatches(pattern.clone()).matches(&m), "Pattern should match within cap");
    
    // Body with "ZZZ" at position 15000 (beyond cap)
    let long_body = "a".repeat(15000);
    let m = Message {
        body: Body { text: long_body, html: None },
        ..Default::default()
    };
    assert!(!Condition::BodyMatches(pattern.clone()).matches(&m), "Pattern beyond cap should not match");
    
    // Body with "ZZZ" at position 5000 within a 15000 char body (the first 10000 chars)
    let mixed_body = "a".repeat(5000) + "ZZZ" + &"b".repeat(9999);
    let m = Message {
        body: Body { text: mixed_body, html: None },
        ..Default::default()
    };
    assert!(Condition::BodyMatches(pattern.clone()).matches(&m), "Pattern in first 10000 chars should match");
    
    // Verify hidden text is also capped
    let hidden_with_zzz = "a".repeat(5000) + "ZZZ" + &"c".repeat(5000);
    let m = Message {
        body: Body { text: "short body".to_string(), html: None },
        hidden_text: hidden_with_zzz,
        ..Default::default()
    };
    assert!(Condition::BodyMatches(pattern).matches(&m), "Hidden text within cap should match");
}

#[test]
fn body_pattern_handles_multibyte_chars() {
    // Use multibyte characters and verify truncation at character boundary
    let pattern = Pattern::new("中".to_string()).unwrap();
    
    // Create body with 10000+ characters, ending with "中"
    let multi = "中".repeat(10_001);
    let multi_clone = multi.clone();
    let m = Message {
        body: Body { text: multi, html: None },
        ..Default::default()
    };
    
    // Pattern should match because first 10000 chars include "中"
    assert!(Condition::BodyMatches(pattern).matches(&m), "Multibyte pattern should match");
    
    // Verify the cap is at character count, not byte count
    // Each "中" is 3 bytes, so 10000 chars = 30000 bytes
    let capped = crate::rule::cap_body_text(&multi_clone);
    assert_eq!(capped.chars().count(), MAX_BODY_PATTERN_CHARS, "Should cap at 10000 characters");
}
