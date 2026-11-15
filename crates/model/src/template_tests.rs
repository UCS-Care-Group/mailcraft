// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;

use crate::time::Timestamp;
use crate::{
    MAX_TEMPLATE_BODY, MAX_TEMPLATE_NAME, MAX_TEMPLATE_SUBJECT, MAX_TEMPLATES, MAX_TEMPLATES_TOTAL_BYTES, Mailbox, Template, TemplateFields,
    expand_text,
};

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
    assert!(mb.add_template(t1.clone()).is_ok());
    assert!(mb.add_template(t2.clone()).is_ok());

    // Duplicate names (case-insensitive, trimmed) are rejected and name the existing template.
    let dup = mb.add_template(Template::new(3, " ALPHA ", "Other", "Body", 30)).unwrap_err().to_string();
    assert!(dup.contains("alpha"), "{dup}");

    let sorted = mb.templates_sorted();
    assert_eq!(sorted.iter().map(|t| t.id).collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(mb.template_by_name("ALPHA").map(|t| t.id), Some(2));
    assert!(mb.remove_template(1));
    assert_eq!(mb.templates().len(), 1);
    assert!(!mb.remove_template(1));

    // Hostile/corrupt template inserted directly into map is sanitized by repair.
    mb.templates.insert(
        99,
        Arc::new(Template { id: 99, name: "Bad\x00Name".into(), subject: "Sub\r\nLine".into(), body: "Body\x07Text".into(), created: 0 }),
    );
    // Empty template is dropped by repair.
    mb.templates.insert(100, Arc::new(Template::default()));
    let fixed = mb.repair();
    assert!(fixed.iter().any(|f| f.contains("templates tidied")));
    assert_eq!(mb.template(99).unwrap().name, "BadName");
    assert_eq!(mb.template(99).unwrap().subject, "Sub Line");
    assert_eq!(mb.template(99).unwrap().body, "BodyText");
    assert!(mb.template(100).is_none());

    // Count cap enforcement.
    let mut cap_mb = Mailbox::default();
    for i in 1..=(MAX_TEMPLATES as u64) {
        assert!(cap_mb.add_template(Template::new(i, &format!("T{i}"), "", "x", 0)).is_ok());
    }
    assert!(cap_mb.add_template(Template::new(9999, "Extra", "", "x", 0)).is_err());
}

#[test]
fn template_total_byte_cap_enforced_and_snapshots_share_arcs() {
    let mut mb = Mailbox::default();
    let big_body = "😀".repeat(MAX_TEMPLATE_BODY);
    let mut id = 1u64;
    loop {
        let candidate = Template::new(id, &format!("Big{id}"), "", &big_body, 0);
        if mb.templates_total_bytes() + candidate.byte_size() > MAX_TEMPLATES_TOTAL_BYTES {
            break;
        }
        assert!(mb.add_template(candidate).is_ok());
        id += 1;
    }
    let count_before = mb.templates().len();
    let err = mb.add_template(Template::new(id, "Overflow", "", &big_body, 0)).unwrap_err().to_string();
    assert!(err.contains("size limit"), "{err}");
    assert_eq!(mb.templates().len(), count_before);

    // Snapshot / clone shares template Arc pointers without copying bodies.
    let snap = mb.clone();
    for (k, v) in mb.templates() {
        assert!(Arc::ptr_eq(v, snap.templates().get(k).unwrap()));
    }

    // Adding a new template or repairing a clean mailbox keeps existing Arc pointers shared.
    mb.remove_template(1);
    assert!(mb.add_template(Template::new(9000, "Small", "", "hello", 0)).is_ok());
    for (k, v) in mb.templates() {
        if let Some(prev) = snap.templates().get(k) {
            assert!(Arc::ptr_eq(v, prev));
        }
    }

    // Hand-edited mailbox over MAX_TEMPLATES_TOTAL_BYTES is trimmed by both loading and repair.
    let mut bloated = Mailbox::default();
    for i in 1..=30u64 {
        bloated.templates.insert(i, Arc::new(Template::new(i, &format!("B{i}"), "", &big_body, 0)));
    }
    assert!(bloated.templates_total_bytes() > MAX_TEMPLATES_TOTAL_BYTES);
    let json = serde_json::to_vec(&bloated).unwrap();
    let loaded = crate::load_limit::from_json(&json, 10_000).unwrap();
    assert!(loaded.templates_total_bytes() <= MAX_TEMPLATES_TOTAL_BYTES);
    assert!(!loaded.templates().is_empty());

    let fixed = bloated.repair();
    assert!(bloated.templates_total_bytes() <= MAX_TEMPLATES_TOTAL_BYTES);
    assert!(fixed.iter().any(|f| f.contains("templates tidied")));
}

#[test]
fn repair_accounts_for_template_ids_and_rehomes_ids_over_max() {
    // Stale nextId: a template id >= nextId advances nextId so alloc_id never collides.
    let mut raw = serde_json::to_value(Mailbox::default()).unwrap();
    raw["nextId"] = serde_json::json!(5);
    raw["templates"] = serde_json::json!({
        "50": { "id": 50, "name": "Existing", "subject": "Sub", "body": "Body" }
    });
    let mut mb: Mailbox = crate::load_limit::from_json(&serde_json::to_vec(&raw).unwrap(), 1000).unwrap();
    mb.repair();
    let fresh = mb.alloc_id();
    assert!(fresh > 50, "expected alloc_id > 50, got {fresh}");
    assert!(mb.add_template(Template::new(fresh, "Second", "Sub2", "Body2", 0)).is_ok());
    assert_eq!(mb.template(50).unwrap().name, "Existing");

    // Template id > MAX_ID (1 << 53) is given a fresh id instead of being dropped.
    let huge: u64 = (1u64 << 53) + 42;
    let mut mb2 = Mailbox::default();
    mb2.templates.insert(huge, Arc::new(Template::new(huge, "Rescued", "Sub", "Keep me", 0)));
    let fixed = mb2.repair();
    assert!(fixed.iter().any(|f| f.contains("id(s) corrected")), "{fixed:?}");
    assert!(!mb2.templates().contains_key(&huge));
    assert_eq!(mb2.templates().len(), 1);
    let (&new_id, tpl) = mb2.templates().iter().next().unwrap();
    assert!(new_id <= (1u64 << 53));
    assert_eq!(tpl.id, new_id);
    assert_eq!(tpl.name, "Rescued");
    assert!(mb2.alloc_id() > new_id);
}
