// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{CorrespondentGroup, MAX_GROUP_ADDRESSES, Mailbox, ModelError};

#[test]
fn normalize_and_sanitize_reject_junk() {
    use crate::correspondents::{MAX_GROUP_NAME, normalize_address, sanitize_name};
    assert_eq!(normalize_address("  Ada@Example.ORG "), Some("ada@example.org".into()));
    assert_eq!(normalize_address(""), None);
    assert_eq!(normalize_address("ada"), None);
    assert_eq!(normalize_address("ada@"), None);
    assert_eq!(normalize_address("@example.org"), None);
    assert_eq!(normalize_address("a da@example.org"), None);
    assert_eq!(normalize_address("ada@example.org\n"), Some("ada@example.org".into()));
    assert_eq!(normalize_address("a\nd@example.org"), None);
    assert_eq!(normalize_address(&format!("{}@example.org", "a".repeat(320))), None);
    // Reject multiple `@` and malformed local/domain parts.
    assert_eq!(normalize_address("a@b@example.org"), None);
    assert_eq!(normalize_address("alice@localhost"), None);
    assert_eq!(normalize_address(".alice@example.org"), None);
    assert_eq!(normalize_address("alice@bad..example.org"), None);
    assert_eq!(normalize_address(&format!("{}@e.co", "ß".repeat(160))), None);

    assert_eq!(sanitize_name("  Ada\nLovelace\t "), "AdaLovelace");
    assert_eq!(sanitize_name(&"é".repeat(300)).chars().count(), MAX_GROUP_NAME);
}

#[test]
fn repair_groups_dedups_and_enforces_disjointness() {
    use crate::correspondents::repair_groups;
    let mut groups = vec![
        CorrespondentGroup {
            name: "  Ada\u{7} ".into(),
            addresses: vec!["Ada@Example.ORG".into(), "ada@example.org".into(), "ada.work@example.org".into(), "bad".into()],
        },
        // Overlaps `ada@example.org`, leaving only one address -> dropped.
        CorrespondentGroup { name: "Dup".into(), addresses: vec!["ada@example.org".into(), "other@example.org".into()] },
        // Single-address group -> dropped.
        CorrespondentGroup { name: "Single".into(), addresses: vec!["solo@example.org".into()] },
    ];
    assert!(repair_groups(&mut groups));
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "Ada");
    assert_eq!(groups[0].addresses, vec!["ada.work@example.org", "ada@example.org"]);
    // Already clean -> returns false.
    assert!(!repair_groups(&mut groups));
}

#[test]
fn mailbox_merge_and_unmerge_round_trip() {
    let mut mb = Mailbox::with_local_account("Me", "me@example.org");
    let group = mb.merge_correspondents(&["A@Example.org".into(), "b@example.org".into()], Some("Alice")).unwrap();
    assert_eq!(group.name, "Alice");
    assert_eq!(group.addresses, vec!["a@example.org", "b@example.org"]);

    // Merging a third address with one of the existing ones unions the group.
    let group2 = mb.merge_correspondents(&["B@Example.org".into(), "c@example.org".into()], None).unwrap();
    assert_eq!(group2.name, "Alice");
    assert_eq!(group2.addresses, vec!["a@example.org", "b@example.org", "c@example.org"]);
    assert_eq!(mb.correspondent_aliases.len(), 1);

    // Unmerging one keeps the remaining two.
    assert!(mb.unmerge_correspondent("b@example.org").unwrap());
    assert_eq!(mb.correspondent_aliases[0].addresses, vec!["a@example.org", "c@example.org"]);

    // Unmerging another leaves < 2 addresses, so the group is removed.
    assert!(mb.unmerge_correspondent("a@example.org").unwrap());
    assert!(mb.correspondent_aliases.is_empty());

    // Unmerging an address not in any group returns false.
    assert!(!mb.unmerge_correspondent("a@example.org").unwrap());

    // Need at least 2 distinct valid addresses to merge.
    assert!(matches!(mb.merge_correspondents(&["same@example.org".into(), "SAME@example.org".into()], None), Err(ModelError::Invalid(_))));
    let too_many: Vec<String> = (0..=MAX_GROUP_ADDRESSES).map(|i| format!("u{i}@example.org")).collect();
    assert!(matches!(mb.merge_correspondents(&too_many, None), Err(ModelError::Invalid(_))));

    // Failed union that exceeds MAX_GROUP_ADDRESSES leaves existing groups intact; exact 50 succeeds.
    let existing: Vec<String> = (0..30).map(|i| format!("g{i}@example.org")).collect();
    mb.merge_correspondents(&existing, Some("Big")).unwrap();
    let mut overflow: Vec<String> = (30..=50).map(|i| format!("g{i}@example.org")).collect();
    overflow.push("g0@example.org".into());
    assert!(matches!(mb.merge_correspondents(&overflow, None), Err(ModelError::Invalid(_))));
    assert_eq!(mb.correspondent_aliases.len(), 1);
    assert_eq!(mb.correspondent_aliases[0].addresses.len(), 30);

    let exact_twenty: Vec<String> = (30..50).map(|i| format!("g{i}@example.org")).collect();
    let mut at_limit = exact_twenty;
    at_limit.push("g0@example.org".into());
    let merged_50 = mb.merge_correspondents(&at_limit, None).unwrap();
    assert_eq!(merged_50.addresses.len(), MAX_GROUP_ADDRESSES);
}
