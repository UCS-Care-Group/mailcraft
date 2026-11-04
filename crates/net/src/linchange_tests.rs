// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Linchange browser approval: requests, answers and cancellation.

use super::*;
use crate::fake::{FakeHttp, json_answer};
use std::sync::{Arc, Mutex};

#[test]
fn an_approval_is_started_and_read() {
    let origin = Arc::new(Mutex::new(String::new()));
    let answer_origin = origin.clone();
    let server = FakeHttp::start(move |_, _| {
        let origin = answer_origin.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        json_answer(
            "200 OK",
            &format!(
                r#"{{"device_code":"device-secret","user_code":"ABCD-EFGH","verification_url":"{origin}/app-auth?code=ABCD-EFGH","expires_in":600,"interval":5}}"#
            ),
        )
    });
    *origin.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = server.url("/").trim_end_matches('/').to_string();
    let grant = start(&server.url("/"), "MailCraft on a laptop").unwrap();
    assert_eq!(grant.device_code, "device-secret");
    assert_eq!(grant.user_code, "ABCD-EFGH");
    assert!(grant.verification_url.ends_with("/app-auth?code=ABCD-EFGH"));
    let request = server.request(0);
    assert!(request.starts_with("POST /api/v1/auth/app-token HTTP/1.1\r\n"), "{request}");
    assert!(request.contains("MailCraft on a laptop"));
}

#[test]
fn an_approval_page_on_another_server_is_refused() {
    let server = FakeHttp::answering(json_answer(
        "200 OK",
        r#"{"device_code":"device-secret","user_code":"ABCD-EFGH","verification_url":"https://elsewhere.example/app-auth","expires_in":600,"interval":5}"#,
    ));
    assert!(matches!(start(&server.url("/"), "MailCraft on a laptop"), Err(NetError::Proto(_))));
}

#[test]
fn an_approval_page_that_is_not_a_web_address_is_refused() {
    let server = FakeHttp::answering(json_answer(
        "200 OK",
        r#"{"device_code":"device-secret","user_code":"ABCD-EFGH","verification_url":"mailto:approve@example.org","expires_in":600,"interval":5}"#,
    ));
    assert!(start(&server.url("/"), "MailCraft on a laptop").is_err());
}

#[test]
fn a_pending_approval_is_polled_until_the_password_comes() {
    let server = FakeHttp::start(|n, _| {
        if n == 0 {
            json_answer("400 Bad Request", r#"{"error":"authorization_pending"}"#)
        } else {
            json_answer("200 OK", r#"{"token":"lcapp_prefix_secret","username":"ada@example.org"}"#)
        }
    });
    let grant = AppTokenGrant {
        device_code: "device-secret".into(),
        user_code: "ABCD-EFGH".into(),
        verification_url: "https://mail.example.org/app-auth?code=ABCD-EFGH".into(),
        expires_in: 60,
        interval: 1,
    };
    let cancel = AtomicBool::new(false);
    let password = finish(&server.url("/"), &grant, &cancel).unwrap();
    assert_eq!(password, "lcapp_prefix_secret");
    assert_eq!(server.asked(), 2);
    assert!(server.request(0).contains("\"device_code\":\"device-secret\""));
}

#[test]
fn cancelling_the_wait_stops_the_polling() {
    let server = FakeHttp::answering(json_answer("400 Bad Request", r#"{"error":"authorization_pending"}"#));
    let grant = AppTokenGrant {
        device_code: "device-secret".into(),
        user_code: "ABCD-EFGH".into(),
        verification_url: "https://mail.example.org/app-auth?code=ABCD-EFGH".into(),
        expires_in: 60,
        interval: 1,
    };
    let cancel = AtomicBool::new(true);
    assert!(matches!(finish(&server.url("/"), &grant, &cancel), Err(NetError::SignInCancelled)));
    assert_eq!(server.asked(), 0);
}
