// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sending a problem report, against a stand-in relay on this machine: each of the relay's
//! answers, an answer that never comes, and answers that aren't one. Nothing here reaches the
//! real relay or GitHub.

use std::time::Duration;

use mailcraft_model::report::{Field, NotSent, ProblemReport};
use proptest::prelude::*;
use serde_json::Value;

use crate::fake::{FakeHttp, json_answer};
use crate::report::*;

fn report() -> ProblemReport {
    ProblemReport::new("Search stops", "I searched and it stopped", "0.9.0", "testos testarch", "@octocat", Some("### MailCraft diagnostics\n"))
}

/// The stand-in relay, answering every request with `status` and `body`.
fn stand_in(status: &str, body: &str) -> FakeHttp {
    FakeHttp::answering(json_answer(status, body))
}

fn send(fake: &FakeHttp) -> Result<u64, NotSent> {
    send_to(&fake.url("/api/report"), &report())
}

#[test]
fn an_accepted_report_answers_its_reference_and_sends_the_report_alone() {
    let fake = stand_in("200 OK", r#"{"reference":42}"#);
    assert_eq!(send(&fake), Ok(42));
    assert_eq!(fake.asked(), 1);
    let request = fake.request(0);
    let (head, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("POST /api/report HTTP/1.1\r\n"), "{head}");
    assert!(head.contains("Content-Type: application/json\r\n"), "{head}");
    // No cookie, no credential, no referrer, and nothing of this machine in the User-Agent.
    for never in ["cookie:", "authorization:", "referer:", "x-forwarded"] {
        assert!(!head.to_ascii_lowercase().contains(never), "{never} in {head}");
    }
    assert!(head.contains("User-Agent: MailCraft\r\n"), "{head}");
    let json: Value = serde_json::from_str(body).unwrap();
    assert_eq!(json["title"], "Search stops");
    assert_eq!(json["text"], "I searched and it stopped");
    assert_eq!(json["version"], "0.9.0");
    assert_eq!(json["platform"], "testos testarch");
    assert_eq!(json["username"], "octocat");
    assert_eq!(json["diagnostics"], "### MailCraft diagnostics\n");
    assert_eq!(json["website"], "");
    assert_eq!(json.as_object().unwrap().len(), 7);
}

#[test]
fn each_refusal_of_the_relay_is_told_apart() {
    for (status, body, want) in [
        ("400 Bad Request", r#"{"error":"invalid","field":"username"}"#, NotSent::Refused(Field::Username)),
        ("400 Bad Request", r#"{"error":"invalid","field":"diagnostics"}"#, NotSent::Refused(Field::Diagnostics)),
        ("400 Bad Request", r#"{"error":"invalid","field":"nonsense"}"#, NotSent::Refused(Field::Request)),
        ("429 Too Many Requests", r#"{"error":"rate_limited"}"#, NotSent::RateLimited),
        ("503 Service Unavailable", r#"{"error":"disabled"}"#, NotSent::Disabled),
        ("503 Service Unavailable", r#"{"error":"unavailable"}"#, NotSent::Unavailable),
        ("502 Bad Gateway", r#"{"error":"failed"}"#, NotSent::Failed),
        ("500 Internal Server Error", "oops", NotSent::Unavailable),
        ("504 Gateway Timeout", "", NotSent::Unavailable),
        ("404 Not Found", r#"{"reference":7}"#, NotSent::Garbled),
        ("302 Found", "", NotSent::Garbled),
        ("200 OK", "not json", NotSent::Garbled),
        ("200 OK", r#"{"reference":"42"}"#, NotSent::Garbled),
        ("200 OK", r#"{"reference":0}"#, NotSent::Garbled),
        ("200 OK", r#"{"reference":-3}"#, NotSent::Garbled),
        ("200 OK", r#"[42]"#, NotSent::Garbled),
    ] {
        let fake = stand_in(status, body);
        assert_eq!(send(&fake), Err(want), "{status} {body}");
        // Sent once, never again by itself.
        assert_eq!(fake.asked(), 1, "{status} {body}");
    }
}

#[test]
fn an_answer_too_long_is_not_read() {
    let long = format!(r#"{{"reference":42,"pad":"{}"}}"#, "x".repeat(MAX_ANSWER * 4));
    let fake = stand_in("200 OK", &long);
    assert_eq!(send(&fake), Err(NotSent::Garbled));
}

#[test]
fn an_answer_that_never_comes_is_not_waited_for() {
    let fake = FakeHttp::start(|_, _| {
        std::thread::sleep(Duration::from_secs(3));
        json_answer("200 OK", r#"{"reference":42}"#)
    });
    let started = std::time::Instant::now();
    assert_eq!(send_within(&fake.url("/api/report"), &report(), Duration::from_millis(500)), Err(NotSent::TooSlow));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn a_relay_that_isnt_there_is_unreachable() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert_eq!(send_to(&format!("http://127.0.0.1:{port}/api/report"), &report()), Err(NotSent::Unreachable));
}

#[test]
fn only_https_leaves_this_machine_and_a_bad_report_sends_nothing() {
    // Plain http to anywhere but this machine isn't spoken.
    assert_eq!(send_to("http://relay.example/api/report", &report()), Err(NotSent::Unreachable));
    assert_eq!(send_to("ftp://relay.example/", &report()), Err(NotSent::Unreachable));
    // A report the relay would refuse never goes.
    let fake = stand_in("200 OK", r#"{"reference":42}"#);
    let mut bad = report();
    bad.username = Some("not a name".into());
    assert_eq!(send_to(&fake.url("/api/report"), &bad), Err(NotSent::Refused(Field::Username)));
    bad = report();
    bad.text.clear();
    assert_eq!(send_to(&fake.url("/api/report"), &bad), Err(NotSent::Refused(Field::Text)));
    assert_eq!(fake.asked(), 0);
}

#[test]
fn the_build_names_an_https_relay_or_none() {
    if let Some(url) = relay() {
        assert!(url.starts_with("https://"), "{url}");
    }
}

proptest! {
    #[test]
    fn whatever_the_relay_answers_reading_it_never_panics(status in any::<u16>(), body in proptest::collection::vec(any::<u8>(), 0..6000)) {
        let got = answer(status, &body);
        if let Ok(n) = got {
            prop_assert_eq!(status, 200);
            prop_assert!(n >= 1);
            prop_assert!(body.len() <= MAX_ANSWER);
        }
    }

    #[test]
    fn json_shaped_answers_never_panic(status in prop_oneof![Just(200u16), Just(400), Just(429), Just(502), Just(503)], key in "[a-z]{0,10}", value in any::<i64>(), text in ".{0,40}") {
        for body in [format!(r#"{{"{key}":{value}}}"#), format!(r#"{{"error":"{text}","field":"{text}"}}"#), format!(r#"{{"reference":{value}}}"#)] {
            let _ = answer(status, body.as_bytes());
        }
    }
}
