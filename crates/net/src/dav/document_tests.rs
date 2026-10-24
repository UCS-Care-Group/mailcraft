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

#[test]
fn a_moved_event_never_carries_both_dtend_and_duration() {
    // The server's copy gives the length; the model writes an end.
    let original = EVENT.replace("SUMMARY:Old", "SUMMARY:Old\r\nDTSTART:20261024T120000Z\r\nDURATION:PT1H");
    let before = EVENT.replace("SUMMARY:Old", "SUMMARY:Old\r\nDTSTART:20261024T120000Z\r\nDTEND:20261024T130000Z");
    let after = before.replace("T120000Z", "T140000Z").replace("T130000Z", "T150000Z");
    // Refusing is fine; RFC 5545 forbids writing both.
    if let Ok(got) = patch(&original, &before, &after, Component::Event) {
        assert!(!(got.contains("\nDTEND") && got.contains("\nDURATION")), "{got}");
    }
}

#[test]
fn editing_one_phone_keeps_the_other_phones_bytes() {
    let original = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:stable\r\nFN:A\r\nTEL;VALUE=uri;TYPE=\"voice,cell\";PREF=1:tel:+1-555-0101\r\nTEL;TYPE=work:+1-555-0102\r\nEND:VCARD\r\n";
    // What a 3.0 writer makes of the same card, before and after the second number changes.
    let before = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:stable\r\nFN:A\r\nTEL;TYPE=CELL:+1-555-0101\r\nTEL;TYPE=WORK:+1-555-0102\r\nEND:VCARD\r\n";
    let after = before.replace("0102", "0199");
    if let Ok(got) = patch(original, before, &after, Component::Card) {
        assert!(got.contains("TEL;VALUE=uri;TYPE=\"voice,cell\";PREF=1:tel:+1-555-0101\r\n"), "{got}");
        assert!(!got.contains("TYPE=CELL"), "3.0 lines spliced into a 4.0 card: {got}");
    }
}

#[test]
fn an_email_reminder_is_not_a_meeting() {
    let alarm = "BEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER:-PT15M\r\nSUMMARY:Reminder\r\nDESCRIPTION:Reminder\r\nATTENDEE:mailto:me@example.org\r\nEND:VALARM\r\n";
    let original = EVENT.replace("END:VEVENT", &format!("{alarm}END:VEVENT"));
    assert!(patch(&original, EVENT, &EVENT.replace("Old", "New"), Component::Event).is_ok());
}

#[test]
fn editing_a_repeated_property_keeps_untouched_folds_and_duplicate_counts() {
    let first = "TEL;TYPE=CELL;PREF=1:+1-555-\r\n 0101\r\nX-BETWEEN:opaque\r\n";
    let original = CARD.replace("FN:Old\r\n", &format!("FN:Old\r\n{first}TEL;TYPE=WORK:+1-555-0102\r\nTEL;TYPE=WORK:+1-555-0102\r\n"));
    let before = original.replace("+1-555-\r\n 0101", "+1-555-0101");
    let after = before.replacen("TEL;TYPE=WORK:+1-555-0102", "TEL;TYPE=WORK:+1-555-0199", 1);
    let got = patch(&original, &before, &after, Component::Card).unwrap();
    assert!(got.contains(first));
    assert_eq!(got.matches("TEL;TYPE=WORK:+1-555-0102\r\n").count(), 1);
    assert_eq!(got.matches("TEL;TYPE=WORK:+1-555-0199\r\n").count(), 1);
    let after = before.replacen("TEL;TYPE=WORK:+1-555-0102\r\n", "", 1);
    let got = patch(&original, &before, &after, Component::Card).unwrap();
    assert!(got.contains(first));
    assert_eq!(got.matches("TEL;TYPE=WORK:+1-555-0102\r\n").count(), 1);
}

#[test]
fn a_same_version_writer_cannot_discard_unmodelled_phone_parameters() {
    let original = CARD.replace("FN:Old", "FN:Old\r\nTEL;PREF=1;TYPE=CELL:123\r\nTEL;TYPE=WORK:456");
    let before = original.replace(";PREF=1", "");
    assert!(patch(&original, &before, &before.replace("456", "789"), Component::Card).is_err());
}

#[test]
fn a_recurring_events_time_form_and_zone_are_not_changed() {
    let fields = "SUMMARY:Old\r\nRRULE:FREQ=WEEKLY;COUNT=3\r\nDTSTART;TZID=Europe/London:20261024T120000\r\nDTEND;TZID=Europe/London:20261024T130000";
    let before = EVENT.replace("SUMMARY:Old", fields);
    let zone = "BEGIN:VTIMEZONE\r\nTZID:Europe/London\r\nEND:VTIMEZONE\r\n";
    let source = before.replace("BEGIN:VEVENT", &format!("{zone}BEGIN:VEVENT"));
    for (key, time) in [("DTSTART", "120000"), ("DTEND", "130000")] {
        for replacement in [
            format!("{key}:20261024T{time}Z"),
            format!("{key}:20261024T{time}"),
            format!("{key};VALUE=DATE:20261024"),
            format!("{key};TZID=Europe/Paris:20261024T{time}"),
        ] {
            let after = before.replace(&format!("{key};TZID=Europe/London:20261024T{time}"), &replacement);
            assert!(patch(&source, &before, &after, Component::Event).is_err(), "{replacement}");
        }
    }
    let after = before.replace("T120000", "T140000").replace("T130000", "T150000");
    assert!(patch(&source, &before, &after, Component::Event).unwrap().contains(zone));
}

#[test]
fn a_zone_reference_without_its_definition_is_not_enough_for_a_time_edit() {
    let before = EVENT.replace("SUMMARY:Old", "SUMMARY:Old\r\nDTSTART;TZID=Europe/London:20261024T120000");
    assert!(patch(&before, &before, &before.replace("T120000", "T130000"), Component::Event).is_err());
    // An unrelated edit leaves the existing zone representation alone.
    assert!(patch(&before, &before, &before.replace("SUMMARY:Old", "SUMMARY:New"), Component::Event).is_ok());
}

#[test]
fn real_ical_writer_duration_conversion_is_refused_before_it_can_mix_end_forms() {
    let source = EVENT.replace("SUMMARY:Old", "SUMMARY:Old\r\nDTSTART:20261024T120000Z\r\nDURATION:PT1H");
    let mut file = mailcraft_ical::parse(&source, &|_| 0);
    let event = file.events.first_mut().unwrap();
    let options = mailcraft_ical::WriteOptions::default();
    let before = mailcraft_ical::write_in(&[event], &[], &options, Some(mailcraft_time::Zone::UTC));
    assert!(before.contains("DTEND:"));
    assert!(!before.contains("DURATION:"));
    event.start = event.start.add_minutes(120);
    event.end = event.end.add_minutes(120);
    let after = mailcraft_ical::write_in(&[event], &[], &options, Some(mailcraft_time::Zone::UTC));
    assert!(patch(&source, &before, &after, Component::Event).is_err());
}

#[test]
fn real_vcard_writer_version_conversion_is_refused() {
    let source = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:stable\r\nFN:A\r\nTEL;VALUE=uri;TYPE=\"voice,cell\";PREF=1:tel:+1-555-0101\r\nTEL;TYPE=work:+1-555-0102\r\nEND:VCARD\r\n";
    let mut contacts = mailcraft_vcard::parse(source);
    let contact = contacts.first_mut().unwrap();
    assert_eq!(contact.phones.len(), 2);
    let before = mailcraft_vcard::write(&[contact]);
    assert!(before.contains("VERSION:3.0"));
    contact.phones.get_mut(1).unwrap().value = "+1-555-0199".into();
    let after = mailcraft_vcard::write(&[contact]);
    assert!(patch(source, &before, &after, Component::Card).is_err());
}

#[test]
fn real_vcard_writer_edits_a_canonical_card_without_rewriting_the_other_phone() {
    let seed = CARD.replace("FN:Old", "FN:Old\r\nTEL;TYPE=CELL:+1-555-0101\r\nTEL;TYPE=WORK:+1-555-0102");
    let contacts = mailcraft_vcard::parse(&seed);
    let canonical = mailcraft_vcard::write(&[contacts.first().unwrap()]);
    let preserved = "TEL;TYPE=CELL:+1-555-\r\n 0101\r\nX-BETWEEN:opaque\r\n";
    let source = canonical.replace("TEL;TYPE=CELL:+1-555-0101\r\n", preserved);
    assert!(source.contains(preserved));
    let mut contacts = mailcraft_vcard::parse(&source);
    let contact = contacts.first_mut().unwrap();
    let before = mailcraft_vcard::write(&[contact]);
    contact.phones.get_mut(1).unwrap().value = "+1-555-0199".into();
    let after = mailcraft_vcard::write(&[contact]);
    let got = patch(&source, &before, &after, Component::Card).unwrap();
    assert!(got.contains(preserved));
    let contacts = mailcraft_vcard::parse(&got);
    assert_eq!(contacts.first().unwrap().phones.get(1).unwrap().value, "+1-555-0199");
}

#[test]
fn real_ical_writer_title_edit_preserves_an_email_alarm() {
    let alarm = "BEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER:-PT15M\r\nSUMMARY:Reminder\r\nDESCRIPTION:Reminder\r\nATTENDEE:mailto:me@example.org\r\nEND:VALARM\r\n";
    let source = EVENT
        .replace("SUMMARY:Old", "SUMMARY:Old\r\nDTSTART:20261024T120000Z\r\nDTEND:20261024T130000Z")
        .replace("END:VEVENT", &format!("{alarm}END:VEVENT"));
    let mut file = mailcraft_ical::parse(&source, &|_| 0);
    let event = file.events.first_mut().unwrap();
    let opts = mailcraft_ical::WriteOptions::default();
    let before = mailcraft_ical::write_in(&[event], &[], &opts, Some(mailcraft_time::Zone::UTC));
    event.title = "New".into();
    let after = mailcraft_ical::write_in(&[event], &[], &opts, Some(mailcraft_time::Zone::UTC));
    let got = patch(&source, &before, &after, Component::Event).unwrap();
    assert!(got.contains(alarm));
    assert!(got.contains("SUMMARY:New\r\n"));
}
