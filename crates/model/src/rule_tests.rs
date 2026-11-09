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
    assert!(r(Condition::SubjectMatches(r"order #\d+".to_string())).matches(&m));
    assert!(r(Condition::SubjectMatches(r"ORDER #\d+".to_string())).matches(&m));
    assert!(!r(Condition::SubjectMatches("order #9999".to_string())).matches(&m));
    assert!(!r(Condition::SubjectMatches("shipped.*order".to_string())).matches(&m));
    // In the body, and in the text a sender hid.
    let mut b = m.clone();
    b.body.text = "your order 2026-4411 is on its way".into();
    b.hidden_text = "order 2026-4411 also hidden".into();
    assert!(r(Condition::BodyMatches(r"order \d{4}-\d{4}".to_string())).matches(&b));
    // A pattern a reader doesn't take matches nothing, and says so in words.
    assert!(!r(Condition::SubjectMatches("order #d".to_string())).matches(&m));
    assert!(!r(Condition::BodyMatches("(unbalanced".to_string())).matches(&m));
    assert_eq!(r(Condition::SubjectMatches("order".to_string())).describe(), "When the subject matches a pattern: mark it as read");
    assert_eq!(r(Condition::BodyMatches("invoice".to_string())).describe(), "When the body matches a pattern: mark it as read");
    // A pattern a message carries is no pattern at all.
    let evil = r(Condition::SubjectMatches("a{100000}".to_string()));
    assert!(!evil.matches(&m) || !evil.matches(&Message { subject: "aaaa".into(), ..m }));
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
