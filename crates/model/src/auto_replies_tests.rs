// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The automatic replies as data: Graph's names, and what is too long or not one line cut.

use serde_json::json;

use crate::auto_replies::{AutoReplies, AutoReplyStatus, ExternalAudience, MAX_REPLY_HTML, ZonedTime};

#[test]
fn graph_names_come_and_go_as_they_are() {
    let graph = json!({
        "status": "scheduled",
        "externalAudience": "contactsOnly",
        "scheduledStartDateTime": {"dateTime": "2026-11-02T08:00:00.0000000", "timeZone": "UTC"},
        "scheduledEndDateTime": {"dateTime": "2026-11-09T08:00:00.0000000", "timeZone": "Pacific Standard Time"},
        "internalReplyMessage": "<p>Away</p>",
        "externalReplyMessage": "<p>Out</p>",
    });
    let read: AutoReplies = serde_json::from_value(graph.clone()).unwrap();
    assert_eq!(read.status, Some(AutoReplyStatus::Scheduled));
    assert_eq!(read.external_audience, Some(ExternalAudience::ContactsOnly));
    assert_eq!(read.scheduled_end_date_time.as_ref().map(|t| t.time_zone.as_str()), Some("Pacific Standard Time"));
    assert_eq!(serde_json::to_value(&read).unwrap(), graph);
    // A change says only what changes.
    let change = AutoReplies { status: Some(AutoReplyStatus::Disabled), ..Default::default() };
    assert_eq!(serde_json::to_value(&change).unwrap(), json!({"status": "disabled"}));
    assert!(AutoReplies::default().is_empty() && !change.is_empty());
    for (name, status) in [("ALWAYSENABLED", AutoReplyStatus::AlwaysEnabled), (" disabled ", AutoReplyStatus::Disabled)] {
        assert_eq!(AutoReplyStatus::parse(name), Some(status));
    }
    assert_eq!(AutoReplyStatus::parse("sometimes"), None);
    assert_eq!(ExternalAudience::parse("ALL"), Some(ExternalAudience::All));
    assert_eq!(ExternalAudience::parse(""), None);
}

#[test]
fn what_is_too_long_is_cut_and_times_are_one_line() {
    let mut odd = AutoReplies {
        internal_reply_message: Some("é".repeat(MAX_REPLY_HTML)),
        external_reply_message: Some("x".repeat(10)),
        scheduled_start_date_time: Some(ZonedTime {
            date_time: format!("2026-11-02T08:00:00\r\nBcc: x{}", "9".repeat(1000)),
            time_zone: "Z".repeat(10_000),
        }),
        ..Default::default()
    };
    odd.tidy();
    let internal = odd.internal_reply_message.unwrap();
    assert!(internal.len() <= MAX_REPLY_HTML && internal.chars().all(|c| c == 'é'));
    assert_eq!(odd.external_reply_message.as_deref(), Some("xxxxxxxxxx"));
    let start = odd.scheduled_start_date_time.unwrap();
    assert!(!start.date_time.chars().any(char::is_control) && start.date_time.chars().count() <= 40);
    assert_eq!(start.time_zone.chars().count(), 100);
}
