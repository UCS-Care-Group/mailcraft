// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::time::Timestamp;
use crate::{MAX_TEMPLATE_BODY, MAX_TEMPLATE_NAME, MAX_TEMPLATE_SUBJECT, MAX_TEMPLATES, Mailbox, Template, TemplateFields, expand_text};

#[test]
fn template_creation_and_sanitizing() {
    let tpl = Template::new(
        1,
        "  Welcome   Note  ",
        "Hello\r\nWorld",
        "Hi {recipient.name},\r\n\r\nWelcome from {recipient.company} on {date}.\tBest!",
        1_700_000_000,
    );
    assert_eq!(tpl.id, 1);
    assert_eq!(tpl.name, "Welcome Note");
    assert_eq!(tpl.subject, "Hello World");
    assert_eq!(tpl.body, "Hi {recipient.name},\n\nWelcome from {recipient.company} on {date}.\tBest!");
}

#[test]
fn template_control_characters_stripped() {
    let raw_name = "Follow\x00\x07\x1bUp\u{202e}Name";
    let raw_subject = "Subj\x08\r\nBcc: evil@example.org";
    let raw_body = "Line1\x00\x0c\x1b\nLine2\tIndented";
    let tpl = Template::new(1, raw_name, raw_subject, raw_body, Timestamp::default());
    assert_eq!(tpl.name, "FollowUpName");
    assert_eq!(tpl.subject, "Subj Bcc: evil@example.org");
    assert_eq!(tpl.body, "Line1\nLine2\tIndented");
}

#[test]
fn template_defaults_empty_name_to_subject_or_body() {
    let from_subject = Template::new(1, "", "Monthly Status Report", "Body text", Timestamp::default());
    assert_eq!(from_subject.name, "Monthly Status Report");

    let from_body = Template::new(2, "   ", "", "\n\nFirst non-empty line\nSecond line", Timestamp::default());
    assert_eq!(from_body.name, "First non-empty line");
}

#[test]
fn template_caps_at_char_boundaries_multibyte() {
    let long_name = "🦀".repeat(MAX_TEMPLATE_NAME + 20);
    let long_subject = "日".repeat(MAX_TEMPLATE_SUBJECT + 50);
    let long_body = "é".repeat(MAX_TEMPLATE_BODY + 50);

    let tpl = Template::new(1, &long_name, &long_subject, &long_body, Timestamp::default());
    assert!(tpl.name.chars().count() <= MAX_TEMPLATE_NAME);
    assert!(tpl.name.len() <= crate::template::MAX_TEMPLATE_NAME_BYTES);
    assert!(tpl.subject.chars().count() <= MAX_TEMPLATE_SUBJECT);
    assert!(tpl.subject.len() <= crate::template::MAX_TEMPLATE_SUBJECT_BYTES);
    assert!(tpl.body.chars().count() <= MAX_TEMPLATE_BODY);
    assert!(tpl.body.len() <= crate::template::MAX_TEMPLATE_BODY_BYTES);
}

#[test]
fn field_expansion_fills_known_and_preserves_unknown_without_re_evaluating() {
    let fields = TemplateFields {
        // Recursive-looking value must NOT be expanded again.
        recipient_name: "Ada {date}".into(),
        recipient_company: "Analytical\r\nEngines\x00".into(),
        date: "October 14, 2026".into(),
    };
    let tpl = Template::new(
        1,
        "Intro",
        "Hello {recipient.name} ({unknown})",
        "Dear {recipient.name} at {recipient.company},\nToday is {date}.\nKeep: {unknown} {1+1} {{date}} {unclosed\nNext {date}.",
        0,
    );
    let (subj, body) = tpl.expand(&fields);
    assert_eq!(subj, "Hello Ada {date} ({unknown})");
    assert_eq!(
        body,
        "Dear Ada {date} at Analytical Engines,\nToday is October 14, 2026.\nKeep: {unknown} {1+1} {October 14, 2026} {unclosed\nNext October 14, 2026."
    );
    // Unclosed brace at the end never panics.
    assert_eq!(expand_text("trail {", &fields), "trail {");
}

#[test]
fn mailbox_templates_crud_and_repair() {
    let mut mb = Mailbox::default();
    assert!(mb.templates().is_empty());

    let t1 = Template::new(1, "Beta", "Sub B", "Body B", 10);
    let t2 = Template::new(2, "alpha", "Sub A", "Body A", 20);
    assert!(mb.add_template(t1.clone()));
    assert!(mb.add_template(t2.clone()));

    let sorted = mb.templates_sorted();
    assert_eq!(sorted.iter().map(|t| t.id).collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(mb.template_by_name("ALPHA").map(|t| t.id), Some(2));
    assert!(mb.remove_template(1));
    assert_eq!(mb.templates().len(), 1);
    assert!(!mb.remove_template(1));

    // Hostile/corrupt template inserted directly into map is sanitized by repair.
    mb.templates.insert(99, Template { id: 99, name: "Bad\x00Name".into(), subject: "Sub\r\nLine".into(), body: "Body\x07Text".into(), created: 0 });
    // Empty template is dropped by repair.
    mb.templates.insert(100, Template::default());
    let fixed = mb.repair();
    assert!(fixed.iter().any(|f| f.contains("templates tidied")));
    assert_eq!(mb.template(99).unwrap().name, "BadName");
    assert_eq!(mb.template(99).unwrap().subject, "Sub Line");
    assert_eq!(mb.template(99).unwrap().body, "BodyText");
    assert!(mb.template(100).is_none());

    // Cap enforcement.
    let mut cap_mb = Mailbox::default();
    for i in 1..=(MAX_TEMPLATES as u64) {
        assert!(cap_mb.add_template(Template::new(i, &format!("T{i}"), "", "x", 0)));
    }
    assert!(!cap_mb.add_template(Template::new(9999, "Extra", "", "x", 0)));
}
