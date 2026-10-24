// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use proptest::prelude::*;

const CARD: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:stable\r\nFN:Old\r\nEND:VCARD\r\n";
const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:stable\r\nSUMMARY:Old\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[test]
fn an_edit_preserves_unknown_bytes_photos_folding_and_nested_components() {
    let extras = "PHOTO;ENCODING=b:abc\r\n def\r\nX-PRIVATE:opaque\r\n";
    let original = CARD.replace("FN:Old\r\n", &format!("FN:Old\r\n{extras}"));
    let got = patch(&original, CARD, &CARD.replace("FN:Old", "FN:New"), Component::Card).unwrap();
    assert!(got.contains(extras));
    assert!(got.contains("FN:New\r\n"));
    assert!(!got.contains("FN:Old"));
    let extras = "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Keep\r\nEND:VALARM\r\n";
    let original = EVENT.replace("END:VEVENT", &format!("{extras}END:VEVENT"));
    let got = patch(&original, EVENT, &EVENT.replace("Old", "New"), Component::Event).unwrap();
    assert!(got.contains(extras));
}

#[test]
fn changed_properties_can_be_added_removed_or_repeated() {
    let before = CARD.replace("FN:Old", "FN:Old\r\nEMAIL:one@example.org\r\nEMAIL:two@example.org");
    let after = CARD.replace("FN:Old", "FN:New\r\nTEL:123");
    let got = patch(&before, &before, &after, Component::Card).unwrap();
    assert!(!got.contains("EMAIL:"));
    assert!(got.contains("TEL:123\r\n"));
    assert_eq!(got.matches("FN:").count(), 1);
}

#[test]
fn a_noop_is_byte_identical_but_not_a_bypass_for_malformed_input() {
    assert_eq!(patch(CARD, CARD, CARD, Component::Card).unwrap(), CARD);
    assert!(patch("junk", CARD, CARD, Component::Card).is_err());
    assert!(patch(&(CARD.to_owned() + CARD), CARD, CARD, Component::Card).is_err());
}

#[test]
fn meetings_identity_changes_ambiguous_groups_and_detached_occurrences_are_refused() {
    for field in ["ORGANIZER:mailto:a@example.org", "ATTENDEE;CN=Guest:mailto:a@example.org"] {
        let original = EVENT.replace("SUMMARY:Old", &format!("SUMMARY:Old\r\n{field}"));
        assert!(patch(&original, EVENT, &EVENT.replace("Old", "New"), Component::Event).is_err());
    }
    assert!(patch(CARD, CARD, &CARD.replace("UID:stable", "UID:other"), Component::Card).is_err());
    let grouped = CARD.replace("FN:Old", "FN:Old\r\nitem1.TEL:123\r\nitem1.X-ABLabel:Home");
    assert!(patch(&grouped, &grouped, &grouped.replace("123", "456"), Component::Card).is_err());
    let detached = EVENT.replace("END:VCALENDAR", "BEGIN:VEVENT\r\nUID:stable\r\nRECURRENCE-ID:20261024T120000\r\nEND:VEVENT\r\nEND:VCALENDAR");
    assert!(patch(&detached, EVENT, &EVENT.replace("Old", "New"), Component::Event).is_err());
}

#[test]
fn malformed_and_over_budget_documents_are_refused() {
    for text in
        ["BEGIN:VCARD\r\nFN:Missing end\r\n", "BEGIN:VCARD\r\nEND:VEVENT\r\n", " FN:Orphan fold\r\n", "BEGIN:VCARD\r\nFN:no\0\r\nEND:VCARD\r\n"]
    {
        assert!(patch(text, CARD, CARD, Component::Card).is_err());
    }
    assert!(patch(&"x".repeat(MAX_BYTES + 1), CARD, CARD, Component::Card).is_err());
    assert!(patch(&CARD.replace("Old", &"x".repeat(MAX_LINE)), CARD, CARD, Component::Card).is_err());
}

#[test]
fn timezone_definitions_are_preserved_and_new_zones_are_refused() {
    let before = EVENT.replace("SUMMARY:Old", "SUMMARY:Old\r\nDTSTART;TZID=Europe/London:20261024T120000");
    let timezone = "BEGIN:VTIMEZONE\r\nTZID:Europe/London\r\nX-OPAQUE:kept\r\nEND:VTIMEZONE\r\n";
    let original = before.replace("BEGIN:VEVENT", &format!("{timezone}BEGIN:VEVENT"));
    let after = before.replace("T120000", "T130000");
    let got = patch(&original, &before, &after, Component::Event).unwrap();
    assert!(got.contains(timezone));
    assert!(got.contains("T130000"));
    assert!(patch(&original, &before, &after.replace("Europe/London", "Europe/Paris"), Component::Event).is_err());
}

#[test]
fn nested_component_edits_are_refused_rather_than_silently_ignored() {
    let before = EVENT.replace("END:VEVENT", "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Old\r\nEND:VALARM\r\nEND:VEVENT");
    let after = before.replace("DESCRIPTION:Old", "DESCRIPTION:New");
    assert!(patch(&before, &before, &after, Component::Event).is_err());
}

#[test]
fn quoted_parameter_colons_do_not_split_the_property_name() {
    let before = CARD.replace("FN:Old", "FN;X-LABEL=\"label:one\":Old");
    let after = before.replace(":Old", ":New");
    assert!(patch(&before, &before, &after, Component::Card).unwrap().contains("FN;X-LABEL=\"label:one\":New"));
}

proptest! {
    #[test]
    fn arbitrary_text_never_panics(text in ".{0,4096}") {
        let _ = patch(&text, CARD, CARD, Component::Card);
        let _ = patch(&text, EVENT, EVENT, Component::Event);
    }
}

#[test]
fn unrelated_group_labels_do_not_prevent_a_name_edit() {
    let original = CARD.replace("FN:Old\r\n", "FN:Old\r\nitem1.PHOTO:data\r\nitem1.X-ABLabel:Portrait\r\n");
    let got = patch(&original, CARD, &CARD.replace("FN:Old", "FN:New"), Component::Card).unwrap();
    assert!(got.contains("item1.PHOTO:data\r\nitem1.X-ABLabel:Portrait\r\n"));
    assert!(got.contains("FN:New"));
}
