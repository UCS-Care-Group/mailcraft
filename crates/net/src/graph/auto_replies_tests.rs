// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The automatic replies read from and written to a fake Graph: what is asked, what is sent,
//! and what a hostile answer comes to.

use mailcraft_model::auto_replies::{AutoReplies, AutoReplyStatus, ExternalAudience, MAX_REPLY_HTML, ZonedTime};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::auto_replies::{automatic_replies_at, replies_of, set_automatic_replies_at};
use crate::NetError;
use crate::fake::{FakeHttp, json_answer};

const TOKEN: &str = "graph.acc-ess_tok.en-0123456789";

fn setting() -> Value {
    json!({
        "@odata.context": "https://graph.example.org/v1.0/$metadata#users('ada')/mailboxSettings/automaticRepliesSetting",
        "status": "scheduled",
        "externalAudience": "contactsOnly",
        "scheduledStartDateTime": {"dateTime": "2026-11-02T08:00:00.0000000", "timeZone": "UTC"},
        "scheduledEndDateTime": {"dateTime": "2026-11-09T17:00:00.0000000", "timeZone": "Pacific Standard Time"},
        "internalReplyMessage": "<html><body><p>Away until Monday.</p><script>alert(1)</script></body></html>",
        "externalReplyMessage": "<p>Out of the office.</p>",
    })
}

#[test]
fn the_setting_is_read_as_the_mailbox_has_it() {
    let server = FakeHttp::answering(json_answer("200 OK", &setting().to_string()));
    let read = automatic_replies_at(&server.url(""), TOKEN).unwrap();
    // Only the automatic replies are asked for, with the token, and only read.
    let request = server.request(0);
    assert!(request.starts_with("GET /v1.0/me/mailboxSettings/automaticRepliesSetting HTTP/1.1\r\n"), "{request}");
    assert!(request.contains(&format!("Authorization: Bearer {TOKEN}\r\n")) && server.asked() == 1);
    assert_eq!(read.status, Some(AutoReplyStatus::Scheduled));
    assert_eq!(read.external_audience, Some(ExternalAudience::ContactsOnly));
    let end = read.scheduled_end_date_time.clone().unwrap();
    assert_eq!((end.date_time.as_str(), end.time_zone.as_str()), ("2026-11-09T17:00:00.0000000", "Pacific Standard Time"));
    // The replies come as the server has them, HTML and all: the engine makes text of them.
    assert!(read.internal_reply_message.as_deref().unwrap().contains("<script>"));
    // All of the settings where the setting was asked for are read the same.
    let all = json!({"timeZone": "UTC", "automaticRepliesSetting": setting(), "workingHours": {}});
    assert_eq!(replies_of(&all), read);
}

#[test]
fn a_change_says_what_changes_and_nothing_else_of_the_settings() {
    let server = FakeHttp::answering(json_answer("200 OK", &json!({"automaticRepliesSetting": setting()}).to_string()));
    let change = AutoReplies {
        status: Some(AutoReplyStatus::AlwaysEnabled),
        internal_reply_message: Some("<p>Back on the 9th &amp; not before.</p>".into()),
        external_audience: Some(ExternalAudience::None),
        ..Default::default()
    };
    set_automatic_replies_at(&server.url(""), TOKEN, &change).unwrap();
    let request = server.request(0);
    assert!(request.starts_with("PATCH /v1.0/me/mailboxSettings HTTP/1.1\r\n"), "{request}");
    assert!(request.contains("Content-Type: application/json\r\n") && request.contains(&format!("Authorization: Bearer {TOKEN}\r\n")));
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"automaticRepliesSetting": {"status": "alwaysEnabled", "internalReplyMessage": "<p>Back on the 9th &amp; not before.</p>", "externalAudience": "none"}})
    );
    // Never the categories, the working hours or the zone of the mailbox.
    for other in ["workingHours", "masterCategories", "timeZone", "language", "dateFormat"] {
        assert!(!request.contains(other), "{other}");
    }
    // 204 is a yes too; nothing to change asks nothing.
    let server = FakeHttp::answering(json_answer("204 No Content", ""));
    set_automatic_replies_at(&server.url(""), TOKEN, &change).unwrap();
    set_automatic_replies_at(&server.url(""), TOKEN, &AutoReplies::default()).unwrap();
    assert_eq!(server.asked(), 1);
    // A reply too long to be one goes cut.
    let long = AutoReplies { external_reply_message: Some("x".repeat(MAX_REPLY_HTML + 50)), ..Default::default() };
    let server = FakeHttp::answering(json_answer("200 OK", "{}"));
    set_automatic_replies_at(&server.url(""), TOKEN, &long).unwrap();
    assert!(server.request(0).len() < MAX_REPLY_HTML + 2_000);
}

#[test]
fn what_graph_says_when_the_replies_are_not_to_be_had_or_changed() {
    let error = |code: &str| json!({"error": {"code": code, "message": "Access is denied. Check credentials and try again."}}).to_string();
    // A sign-in that reads the settings and may not change them: said as what it is, for the
    // window to ask the owner to sign in again.
    let server = FakeHttp::answering(json_answer("403 Forbidden", &error("ErrorAccessDenied")));
    let change = AutoReplies { status: Some(AutoReplyStatus::Disabled), ..Default::default() };
    let refused = set_automatic_replies_at(&server.url(""), TOKEN, &change).unwrap_err();
    assert!(matches!(refused, NetError::Forbidden(_)), "{refused}");
    assert_eq!(
        refused.to_string(),
        NetError::Forbidden("this account may not change its automatic replies through Microsoft Graph (ErrorAccessDenied)".into()).to_string()
    );
    let refused = automatic_replies_at(&server.url(""), TOKEN).unwrap_err();
    assert!(refused.to_string().contains("may not read its automatic replies"), "{refused}");
    // A sign-in that is no longer taken, a Graph that is busy.
    let server = FakeHttp::answering(json_answer("401 Unauthorized", &error("InvalidAuthenticationToken")));
    assert!(automatic_replies_at(&server.url(""), TOKEN).unwrap_err().is_refused_sign_in());
    assert!(set_automatic_replies_at(&server.url(""), TOKEN, &change).unwrap_err().is_refused_sign_in());
    let server = FakeHttp::answering(json_answer("503 Service Unavailable", "{}"));
    assert!(matches!(automatic_replies_at(&server.url(""), TOKEN), Err(NetError::Busy { .. })));
    // Nothing of the token or of the reply sent in any of it, even when a careless server writes
    // them back.
    let secret = AutoReplies { internal_reply_message: Some("<p>my-private-reply-text-0123</p>".into()), ..Default::default() };
    for status in ["403 Forbidden", "400 Bad Request", "500 Internal Server Error"] {
        let server = FakeHttp::answering(json_answer(status, &error(&format!("{TOKEN} my-private-reply-text-0123"))));
        let said = set_automatic_replies_at(&server.url(""), TOKEN, &secret).unwrap_err();
        let shown = format!("{said} {said:?}");
        assert!((0..TOKEN.len() - 8).all(|from| !shown.contains(&TOKEN[from..from + 8])), "{shown}");
        assert!(!shown.contains("private-reply"), "{shown}");
    }
    // An answer that isn't JSON or isn't an object is an error; one far too long is too.
    for body in ["", "not json", "[]", "7", "\"status\""] {
        let server = FakeHttp::answering(json_answer("200 OK", body));
        assert!(automatic_replies_at(&server.url(""), TOKEN).is_err(), "{body}");
    }
    let huge = json!({"status": "alwaysEnabled", "internalReplyMessage": "x".repeat(4 * 1024 * 1024)});
    let server = FakeHttp::answering(json_answer("200 OK", &huge.to_string()));
    assert!(automatic_replies_at(&server.url(""), TOKEN).is_err());
    // Without a token nothing is asked.
    let server = FakeHttp::answering(json_answer("200 OK", "{}"));
    assert!(automatic_replies_at(&server.url(""), "").is_err());
    assert!(set_automatic_replies_at(&server.url(""), "", &change).is_err());
    assert_eq!(server.asked(), 0);
}

#[test]
fn hostile_settings_are_cut_or_left_out() {
    for odd in [
        json!({}),
        json!({"status": "sometimes", "externalAudience": "everybody"}),
        json!({"status": 3, "externalAudience": null, "internalReplyMessage": 7, "externalReplyMessage": ["<p>"]}),
        json!({"scheduledStartDateTime": "2026-11-02", "scheduledEndDateTime": {"dateTime": "", "timeZone": "UTC"}}),
        json!({"automaticRepliesSetting": "on"}),
    ] {
        let read = replies_of(&odd);
        assert_eq!(
            (read.status, read.external_audience, read.internal_reply_message.clone(), read.external_reply_message.clone()),
            (None, None, None, None),
            "{odd}"
        );
        assert!(read.scheduled_start_date_time.is_none() && read.scheduled_end_date_time.is_none(), "{odd}");
    }
    let long = replies_of(
        &json!({"internalReplyMessage": "é".repeat(MAX_REPLY_HTML), "scheduledStartDateTime": {"dateTime": format!("2026-11-02T08:00:00\r\n{}", "0".repeat(500)), "timeZone": "Z\nZ"}}),
    );
    assert!(long.internal_reply_message.unwrap().len() <= MAX_REPLY_HTML);
    let start = long.scheduled_start_date_time.unwrap();
    assert!(!format!("{}{}", start.date_time, start.time_zone).chars().any(char::is_control));
    assert_eq!(start, ZonedTime { date_time: start.date_time.clone(), time_zone: "ZZ".into() });
}

proptest! {
    #[test]
    fn arbitrary_answers_give_a_setting(status in "\\PC{0,20}", audience in "\\PC{0,20}", reply in "\\PC{0,200}", at in "\\PC{0,60}") {
        let answer = json!({"status": status, "externalAudience": audience, "internalReplyMessage": reply, "scheduledEndDateTime": {"dateTime": at, "timeZone": at}});
        let read = replies_of(&answer);
        if let Some(end) = read.scheduled_end_date_time {
            prop_assert!(end.date_time.chars().count() <= 40 && !end.date_time.chars().any(char::is_control));
        }
        prop_assert!(read.internal_reply_message.is_some_and(|reply| reply.len() <= MAX_REPLY_HTML));
    }
}
