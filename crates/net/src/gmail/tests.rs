// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Moving messages between Gmail's categories through Gmail's API (#911), against the stand-in.

use mailcraft_model::GmailCategory;
use serde_json::{Value, json};

use super::standin::{StandIn, TOKEN};
use super::*;

/// Gmail's ids of the messages the tests move, as `X-GM-MSGID` gives them.
const ID: u64 = 0x1_8f3a_c0de_2bad;
const OTHER: u64 = 0x1_8f3a_c0de_2bae;

fn standin() -> StandIn {
    let standin = StandIn::start().unwrap();
    standin.state().add(ID, &["INBOX", "UNREAD", "CATEGORY_PROMOTIONS"]);
    standin.state().add(OTHER, &["INBOX", "CATEGORY_SOCIAL"]);
    standin
}

#[test]
fn a_batch_is_one_post_with_the_ids_in_hex_and_the_labels() {
    let standin = standin();
    let _routed = standin.route();
    assert_eq!(batch_modify(TOKEN, &[ID, OTHER], GmailCategory::Updates), Ok(()));
    let asked = standin.requests();
    assert_eq!(asked.len(), 1);
    let one = &asked[0];
    assert_eq!((one.method.as_str(), one.path.as_str(), one.status), ("POST", "/gmail/v1/users/me/messages/batchModify", 204));
    assert_eq!(one.header("authorization"), format!("Bearer {TOKEN}"));
    assert!(one.header("content-type").starts_with("application/json"));
    let body: Value = serde_json::from_str(&one.body).unwrap();
    assert_eq!(
        body,
        json!({"ids": ["18f3ac0de2bad", "18f3ac0de2bae"], "addLabelIds": ["CATEGORY_UPDATES"], "removeLabelIds": ["CATEGORY_PERSONAL", "CATEGORY_SOCIAL", "CATEGORY_PROMOTIONS", "CATEGORY_FORUMS"]})
    );
    // Gmail has them where they were moved, and their other labels as they were.
    assert_eq!(standin.state().messages[&ID], ["INBOX", "UNREAD", "CATEGORY_UPDATES"]);
    assert_eq!(standin.state().categories(OTHER), ["CATEGORY_UPDATES"]);
    // Primary is Gmail's `CATEGORY_PERSONAL`.
    assert_eq!(batch_modify(TOKEN, &[ID], GmailCategory::Primary), Ok(()));
    assert_eq!(standin.state().categories(ID), ["CATEGORY_PERSONAL"]);
    // Nothing but the ids and the labels went: no address, no subject, no folder.
    let sent = standin.requests().iter().map(|a| format!("{} {}", a.path, a.body)).collect::<String>();
    assert!(!sent.contains('@') && !sent.to_ascii_lowercase().contains("inbox"));
    // No ids: nothing sent.
    assert_eq!(batch_modify(TOKEN, &[], GmailCategory::Social), Ok(()));
    assert_eq!(standin.requests().len(), 2);
}

#[test]
fn a_message_gmail_no_longer_has_cant_be_told_apart() {
    // Gmail answers the batch as a whole: the next reading of the Inbox sets it right here.
    let standin = standin();
    let _routed = standin.route();
    assert_eq!(batch_modify(TOKEN, &[ID + 7, ID], GmailCategory::Forums), Ok(()));
    assert_eq!(standin.state().categories(ID), ["CATEGORY_FORUMS"]);
}

#[test]
fn every_refusal_is_an_error_that_says_what_and_never_the_token() {
    let standin = standin();
    let _routed = standin.route();
    standin.state().echo = true;
    for (status, kind) in [
        (401, "auth"),
        (403, "forbidden"),
        (429, "busy"),
        (500, "busy"),
        (502, "busy"),
        (503, "busy"),
        (400, "rejected"),
        (404, "rejected"),
        (409, "rejected"),
    ] {
        standin.state().faults.push_back(status);
        let e = batch_modify(TOKEN, &[ID, OTHER], GmailCategory::Forums).unwrap_err();
        let fits = match kind {
            "auth" => e.is_refused_sign_in(),
            "forbidden" => matches!(e, NetError::Forbidden(_)),
            "busy" => matches!(e, NetError::Busy { .. }),
            _ => matches!(e, NetError::Proto(ProtoError::Rejected(_))),
        };
        assert!(fits, "{status}: {e:?}");
        let words = format!("{e} {e:?}");
        assert!(
            !words.contains(TOKEN) && !words.contains("Bearer") && !words.contains("CATEGORY_") && !words.contains("18f3ac"),
            "{status}: {words}"
        );
    }
    // A token the stand-in doesn't take is a refused sign-in too.
    let e = batch_modify("another.token-0000", &[ID], GmailCategory::Forums).unwrap_err();
    assert!(e.is_refused_sign_in() && !e.to_string().contains("another.token"));
    // None of them moved anything.
    assert_eq!(standin.state().categories(ID), ["CATEGORY_PROMOTIONS"]);
    assert_eq!(standin.state().categories(OTHER), ["CATEGORY_SOCIAL"]);
}

#[test]
fn nothing_goes_without_ids_a_token_or_room() {
    let standin = standin();
    let _routed = standin.route();
    assert!(matches!(batch_modify(TOKEN, &[ID, 0], GmailCategory::Social), Err(NetError::Config(_))));
    let too_many: Vec<u64> = (1..=MAX_BATCH as u64 + 1).collect();
    assert!(matches!(batch_modify(TOKEN, &too_many, GmailCategory::Social), Err(NetError::Config(_))));
    for token in ["", "two words", "line\r\nX-Evil: 1", &"x".repeat(MAX_TOKEN + 1)] {
        let e = batch_modify(token, &[ID], GmailCategory::Social).unwrap_err();
        assert!(e.is_refused_sign_in(), "{e:?}");
    }
    assert!(standin.requests().is_empty());
    // A whole batch goes in one request.
    let full: Vec<u64> = (1..=MAX_BATCH as u64).collect();
    assert_eq!(batch_modify(TOKEN, &full, GmailCategory::Social), Ok(()));
    assert_eq!(standin.requests().len(), 1);
}

#[test]
fn switched_off_nothing_goes() {
    let standin = standin();
    let _routed = standin.route_switched_off();
    assert!(matches!(batch_modify(TOKEN, &[ID], GmailCategory::Social), Err(NetError::Config(_))));
    assert!(standin.requests().is_empty());
}

#[test]
fn a_google_token_goes_to_gmails_api_and_nowhere_else() {
    let ok = |text: &str| may_go_to(&Url::parse(text).unwrap()).is_ok();
    assert!(ok("https://gmail.googleapis.com"));
    assert!(ok("https://GMAIL.googleapis.com."));
    for elsewhere in [
        "https://gmail.googleapis.com:8443",
        "https://gmail.googleapis.com.attacker.example",
        "https://googleapis.com",
        "https://www.googleapis.com",
        "https://imap.gmail.com",
        "https://graph.microsoft.com",
        "https://attacker.example",
    ] {
        assert!(!ok(elsewhere), "{elsewhere}");
    }
    // Unencrypted to this machine only while a test has routed this thread to a stand-in.
    assert!(!ok("http://127.0.0.1:9"));
    let standin = standin();
    let _routed = standin.route();
    assert!(ok(&standin.url()));
    let plain = Url { host: "gmail.googleapis.com".into(), port: 0, secure: false, path: "/".into() };
    assert!(may_go_to(&plain).is_err(), "never unencrypted to the real one");
    // And anything else is refused before a connection is made.
    assert!(matches!(batch_modify_at("https://attacker.example", TOKEN, &[ID], GmailCategory::Social), Err(NetError::Config(_))));
}

#[test]
fn the_imap_and_smtp_check_is_not_widened_for_the_api() {
    let account = mailcraft_model::Account {
        oauth: Some(mailcraft_model::OAuth { provider: mailcraft_model::OAuthProvider::Google, ..Default::default() }),
        ..Default::default()
    };
    assert!(crate::google_destination(&account, "gmail.googleapis.com", "imap.gmail.com").is_err());
    assert!(crate::google_destination(&account, "gmail.googleapis.com", "smtp.gmail.com").is_err());
}
