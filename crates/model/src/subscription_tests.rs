// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use proptest::prelude::*;

use super::*;
use crate::Calendar;

#[test]
fn https_and_webcal_addresses_are_taken_as_https() {
    let a = check_address("  webcal://Cal.Example.org/private/abc123/basic.ics ").unwrap();
    assert_eq!((a.url.as_str(), a.host.as_str()), ("https://Cal.Example.org/private/abc123/basic.ics", "cal.example.org"));
    assert_eq!(check_address("HTTPS://cal.example.org:8443/x.ics?t=1#top").unwrap().host, "cal.example.org");
    assert_eq!(check_address("webcals://[2001:db8::1]:443/x.ics").unwrap().host, "2001:db8::1");
    assert_eq!(check_address("https://cal.example.org./x.ics").unwrap().host, "cal.example.org");
}

#[test]
fn what_is_not_taken_says_why() {
    assert_eq!(check_address("  "), Err(AddressProblem::Empty));
    assert_eq!(check_address("http://cal.example.org/x.ics"), Err(AddressProblem::Plain));
    assert_eq!(check_address("HTTP://cal.example.org/x.ics"), Err(AddressProblem::Plain));
    assert_eq!(check_address("ftp://cal.example.org/x.ics"), Err(AddressProblem::NotWeb));
    assert_eq!(check_address("cal.example.org/x.ics"), Err(AddressProblem::NotWeb));
    assert_eq!(check_address("https://user:pw@cal.example.org/x.ics"), Err(AddressProblem::UserInfo));
    assert_eq!(check_address("https://user@cal.example.org/x.ics"), Err(AddressProblem::UserInfo));
    assert_eq!(check_address("https:///x.ics"), Err(AddressProblem::Malformed));
    assert_eq!(check_address("https://cal.example.org:0/x.ics"), Err(AddressProblem::Malformed));
    assert_eq!(check_address("https://cal.example.org:99999/x.ics"), Err(AddressProblem::Malformed));
    assert_eq!(check_address("https://cal.example.org:80a/x.ics"), Err(AddressProblem::Malformed));
    assert_eq!(check_address("https://cal.example.org/a b.ics"), Err(AddressProblem::Malformed));
    assert_eq!(check_address("https://cal.example.org/a\r\nHost: x"), Err(AddressProblem::Malformed));
    let long = format!("https://cal.example.org/{}", "a".repeat(MAX_ADDRESS));
    assert_eq!(check_address(&long), Err(AddressProblem::TooLong));
    let edge = format!("https://cal.example.org/{}", "a".repeat(MAX_ADDRESS - "https://cal.example.org/".len()));
    assert!(check_address(&edge).is_ok());
}

#[test]
fn a_refused_http_address_offers_https() {
    assert_eq!(try_https(" http://cal.example.org/x.ics?t=1 ").as_deref(), Some("https://cal.example.org/x.ics?t=1"));
    assert_eq!(try_https("https://cal.example.org/x.ics"), None);
    assert_eq!(try_https("webcal://cal.example.org/x.ics"), None);
}

#[test]
fn the_address_never_shows_in_debug_output() {
    let a = check_address("https://cal.example.org/private/SECRET-TOKEN/x.ics").unwrap();
    assert!(!format!("{a:?}").contains("SECRET"));
    let outcome = SubscriptionOutcome::Changed { text: "SECRET-TOKEN".into(), etag: String::new(), last_modified: String::new() };
    assert!(!format!("{outcome:?}").contains("SECRET"));
}

#[test]
fn the_interval_stays_within_bounds() {
    let mut s = Subscription::new("cal.example.org", 100);
    assert_eq!(s.interval(), 60);
    for (said, kept) in [(0, 60), (1, 15), (14, 15), (15, 15), (90, 90), (1440, 1440), (1441, 1440), (u32::MAX, 1440)] {
        s.interval_minutes = said;
        assert_eq!(s.interval(), kept, "{said}");
    }
}

#[test]
fn a_look_is_due_when_never_made_or_the_interval_has_gone_by() {
    let mut s = Subscription::new("cal.example.org", 100);
    assert!(s.due(1_000));
    s.checked = 1_000;
    assert!(!s.due(1_000 + 59 * 60));
    assert!(s.due(1_000 + 60 * 60));
    // A clock that went back is not a reason to wait for ever.
    assert!(s.due(500));
    s.interval_minutes = 15;
    assert!(s.due(1_000 + 15 * 60));
}

#[test]
fn failures_are_kept_as_codes_and_read_back() {
    for failure in [
        Failure::Address,
        Failure::NotPublic,
        Failure::TooManyRedirects,
        Failure::Status(404),
        Failure::TooLarge,
        Failure::TimedOut,
        Failure::Packed,
        Failure::Network,
        Failure::NotCalendar,
    ] {
        assert_eq!(Failure::parse(&failure.code()), failure);
        assert!(!failure.message().is_empty());
    }
    assert_eq!(Failure::parse("from-the-future"), Failure::Other);
    assert_eq!(Failure::parse("status:abc"), Failure::Other);
}

#[test]
fn the_credential_entry_is_named_for_the_calendar_and_its_host() {
    let s = Subscription::new("Cal.Example.org", 1234);
    assert_eq!(s.secret_name(7), "subscription:7|host=cal.example.org|since=1234");
    assert_ne!(s.secret_name(7), s.secret_name(8));
    assert!(!s.secret_name(7).contains("https"));
}

#[test]
fn a_subscribed_calendar_is_read_only_whatever_its_file_says() {
    let calendar = Calendar { id: 3, name: "Fixtures".into(), subscription: Some(Subscription::new("cal.example.org", 5)), ..Default::default() };
    assert!(!calendar.can_edit() && calendar.is_subscribed());
    let json = serde_json::to_string(&calendar).unwrap();
    assert!(json.contains("cal.example.org") && !json.contains("https"), "{json}");
    // A file that says Edit, or nothing about rights, still reads as read-only.
    let edit = json.replace("\"subscription\"", "\"rights\":\"edit\",\"subscription\"");
    for text in [json, edit] {
        let read: Calendar = serde_json::from_str(&text).unwrap();
        assert!(!read.can_edit(), "{text}");
        assert_eq!(read.subscription.as_ref().map(|s| s.host.as_str()), Some("cal.example.org"));
    }
    // Something odd in place of the subscription is no subscription.
    let odd: Calendar = serde_json::from_str(r#"{"id":1,"name":"x","subscription":42}"#).unwrap();
    assert!(odd.subscription.is_none());
}

#[test]
fn validators_are_printable_and_short() {
    assert_eq!(clean_validator(" \"abc-1\" "), "\"abc-1\"");
    assert_eq!(clean_validator("Tue, 03 Nov 2026 10:00:00 GMT"), "Tue, 03 Nov 2026 10:00:00 GMT");
    assert_eq!(clean_validator("a\r\nHost: evil"), "");
    assert_eq!(clean_validator("\u{e9}"), "");
    assert_eq!(clean_validator(&"a".repeat(MAX_VALIDATOR + 1)), "");
}

proptest! {
    #[test]
    fn any_address_text_is_judged_without_a_panic(text in "\\PC{0,3000}") {
        if let Ok(a) = check_address(&text) {
            prop_assert!(a.url.starts_with("https://") && !a.host.is_empty());
            prop_assert!(!a.url.contains(char::is_whitespace) && !a.url.contains(char::is_control));
        }
    }

    #[test]
    fn any_failure_code_reads_without_a_panic(code in "\\PC{0,40}") {
        let _ = Failure::parse(&code).message();
    }
}
