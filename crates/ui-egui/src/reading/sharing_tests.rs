// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which messages are taken for calendar sharing invitations (#70): only those with the sharing
//! headers of a calendar; and the sender's address is checked before it is used.

use mailcraft_model::{Address, Message};

use super::sharing::{is_calendar_share, sharer};

fn with(headers: &[(&str, &str)], from: &str) -> Message {
    Message {
        from: Address::new("Cy Colleague", from),
        headers: headers.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect(),
        ..Default::default()
    }
}

#[test]
fn a_message_with_the_sharing_headers_of_a_calendar_is_an_invitation() {
    let guid = ("X-Sharing-Provider-Guid", "{AEF00F00-0000-0000-C000-000000000046}");
    assert!(is_calendar_share(&with(&[guid, ("X-Sharing-Remote-Type", "IPF.Appointment")], "cy@example.org")));
    assert!(is_calendar_share(&with(&[guid, ("x-sharing-local-type", " ipf.appointment ")], "cy@example.org")));
}

#[test]
fn nothing_else_is_taken_for_one() {
    let guid = ("X-Sharing-Provider-Guid", "{AEF00F00-0000-0000-C000-000000000046}");
    // A folder that isn't a calendar; no provider; no type; words alone.
    assert!(!is_calendar_share(&with(&[guid, ("X-Sharing-Remote-Type", "IPF.Contact")], "cy@example.org")));
    assert!(!is_calendar_share(&with(&[("X-Sharing-Remote-Type", "IPF.Appointment")], "cy@example.org")));
    assert!(!is_calendar_share(&with(&[guid], "cy@example.org")));
    assert!(!is_calendar_share(&with(&[], "cy@example.org")));
    let mut words = with(&[], "cy@example.org");
    words.subject = "Cy Colleague shared a calendar with you".into();
    assert!(!is_calendar_share(&words));
    assert!(!is_calendar_share(&with(&[guid, ("X-Sharing-Remote-Type", &"IPF.Appointment".repeat(100))], "cy@example.org")));
}

#[test]
fn the_sender_is_used_only_when_it_could_name_a_mailbox() {
    assert_eq!(sharer(&with(&[], "cy@example.org")).as_deref(), Some("cy@example.org"));
    for bad in ["", "cy", "cy@example.org/../x", "a b@example.org", "cy@example.org\r\nBcc: x@example.org", "é@example.org"] {
        assert_eq!(sharer(&with(&[], bad)), None, "{bad:?}");
    }
}
