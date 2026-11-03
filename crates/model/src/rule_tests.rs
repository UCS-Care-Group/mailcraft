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
    assert!(json.contains(r#"{"type":"moveTo","value":9}"#), "{json}");
    assert_eq!(serde_json::from_str::<Rule>(&json).unwrap(), rule);
}
