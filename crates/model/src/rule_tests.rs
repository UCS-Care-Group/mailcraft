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
    // A pattern over 1000 chars is refused.
    assert!(Pattern::new("a".repeat(1001)).is_none());
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
