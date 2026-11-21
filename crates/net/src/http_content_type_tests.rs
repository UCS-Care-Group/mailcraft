// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An answer's `Content-Type`, as `read_reply` keeps it (a JMAP answer must be JSON).

use std::time::{Duration, Instant};

use super::*;

/// The `Content-Type` kept of an answer with these header lines.
fn kept(headers: &str) -> Option<String> {
    let answer = format!("HTTP/1.1 200 OK\r\n{headers}Content-Length: 2\r\n\r\n{{}}");
    read_reply(&mut answer.as_bytes(), Instant::now(), Duration::from_secs(5), MAX_BODY).unwrap().content_type
}

#[test]
fn the_content_type_is_kept() {
    assert_eq!(kept("Content-Type: application/json; charset=utf-8\r\n").as_deref(), Some("application/json; charset=utf-8"));
    assert_eq!(kept("content-type:application/json\r\n").as_deref(), Some("application/json"));
    assert_eq!(kept("X-Other: 1\r\n"), None);
    // The first one counts.
    assert_eq!(kept("Content-Type: application/json\r\nContent-Type: text/html\r\n").as_deref(), Some("application/json"));
    // One too long to be a media type is none, not a cut one that could read as another.
    assert_eq!(kept(&format!("Content-Type: application/json{}\r\n", "x".repeat(300))), None);
}

#[test]
fn a_content_type_too_long_leaves_the_answer_without_one_whatever_follows() {
    let long = format!("Content-Type: text/html{}\r\n", "x".repeat(300));
    // A second field after it is not taken in its place.
    assert_eq!(kept(&format!("{long}Content-Type: application/json\r\n")), None);
    // One exactly at the limit is still kept, and still the only one.
    let at_limit = format!("application/json;{}", "x".repeat(MAX_CONTENT_TYPE - "application/json;".len()));
    assert_eq!(kept(&format!("Content-Type: {at_limit}\r\nContent-Type: text/html\r\n")), Some(at_limit));
}
