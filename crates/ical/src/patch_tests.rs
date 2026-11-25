// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An event changed here, written into the server's document: only what changed is replaced.
use super::*;
use proptest::prelude::*;

/// A server's document of one event, with what MailCraft doesn't read: an alarm, an attachment,
/// another program's property, a long folded line, and a zone.
const RAW: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Server//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:abc-1\r\nDTSTAMP:20261101T080000Z\r\nDTSTART;TZID=Europe/Berlin:20261201T100000\r\nDTEND;TZID=Europe/Berlin:20261201T110000\r\nSUMMARY:Alt\r\nLOCATION:Raum 1\r\nSEQUENCE:3\r\nX-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\nATTACH;FMTTYPE=application/pdf:https://example.org/a-very-long-address-that-is-folded-over\r\n -two-lines.pdf\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Erinnerung\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// What the writer makes of the event as it is now: another title and time, no location, and a
/// zone the server's document doesn't have.
const FRESH: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//MailCraft//MailCraft//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nEND:VTIMEZONE\r\nBEGIN:VTIMEZONE\r\nTZID:Asia/Bangkok\r\nBEGIN:STANDARD\r\nDTSTART:19700101T000000\r\nTZOFFSETFROM:+0700\r\nTZOFFSETTO:+0700\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:abc-1\r\nDTSTAMP:20261122T120000Z\r\nDTSTART;TZID=Asia/Bangkok:20261202T090000\r\nDTEND;TZID=Asia/Bangkok:20261202T100000\r\nSUMMARY:Neu\r\nDESCRIPTION:Mit Notiz\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[test]
fn only_what_changed_is_replaced_and_everything_else_stays_as_it_came() {
    let out = patch_event(RAW, FRESH).unwrap();
    for kept in [
        "X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC",
        "ATTACH;FMTTYPE=application/pdf:https://example.org/a-very-long-address-that-is-folded-over\r\n -two-lines.pdf",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Erinnerung\r\nTRIGGER:-PT15M\r\nEND:VALARM",
        "PRODID:-//Example//Server//EN",
        "UID:abc-1",
    ] {
        assert!(out.contains(kept), "{kept} kept:\n{out}");
    }
    assert!(out.contains("SUMMARY:Neu") && !out.contains("SUMMARY:Alt"));
    assert!(out.contains("DTSTART;TZID=Asia/Bangkok:20261202T090000") && !out.contains("20261201T100000"));
    assert!(out.contains("DESCRIPTION:Mit Notiz"), "the event's own description, not the alarm's");
    assert!(!out.contains("LOCATION:"), "a location taken away is gone");
    assert!(out.contains("SEQUENCE:4") && !out.contains("SEQUENCE:3"));
    assert!(out.contains("DTSTAMP:20261122T120000Z") && !out.contains("DTSTAMP:20261101"));
    // The zone it now names is added once, before the event; the one it had stays as it was.
    assert_eq!(out.matches("TZID:Asia/Bangkok").count(), 1);
    assert_eq!(out.matches("BEGIN:VTIMEZONE").count(), 2);
    assert!(out.contains("TZOFFSETFROM:+0200"), "the server's own zone, not the writer's empty one");
    assert!(out.find("TZID:Asia/Bangkok") < out.find("BEGIN:VEVENT"));
    // And it reads back as the changed event.
    let events = crate::parse(&out, &|_| 0).events;
    assert_eq!(events.len(), 1);
    assert_eq!(events.first().map(|e| e.title.as_str()), Some("Neu"));
}

#[test]
fn a_document_of_more_than_one_event_or_out_of_order_is_not_changed() {
    let two = RAW.replace(
        "END:VEVENT\r\nEND:VCALENDAR",
        "END:VEVENT\r\nBEGIN:VEVENT\r\nUID:abc-1\r\nRECURRENCE-ID:20261208T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR",
    );
    assert!(patch_event(&two, FRESH).is_none());
    assert!(patch_event("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VCALENDAR\r\n", FRESH).is_none());
    assert!(patch_event(" folded first\r\n", FRESH).is_none());
    assert!(patch_event(RAW, "").is_none());
}

proptest! {
    #[test]
    fn any_text_never_panics(raw in ".{0,400}", fresh in ".{0,400}") {
        let _ = patch_event(&raw, &fresh);
        let _ = patch_event(&raw, FRESH);
        let _ = patch_event(RAW, &fresh);
    }
}
