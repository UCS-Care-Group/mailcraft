// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn basic_matches_rfc_7617() {
    assert_eq!(Secret::basic("Aladdin", "open sesame").unwrap().expose(), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    assert_eq!(Secret::basic("a", "b").unwrap().expose(), "Basic YTpi");
    assert_eq!(Secret::basic("ab", "c").unwrap().expose(), "Basic YWI6Yw==");
}

#[test]
fn secrets_and_addresses_stay_out_of_debug() {
    let s = Secret::basic("user", "hunter2").unwrap();
    assert!(!format!("{s:?}").contains("hunter2"));
    let r = HttpRequest::new(Method::Put, "https://dav.example.org/secret-path", &[("Authorization", s.expose())], b"BODY".to_vec()).unwrap();
    let shown = format!("{r:?}");
    assert!(!shown.contains("secret-path") && !shown.contains("Basic") && !shown.contains("BODY"));
    assert!(!r.is_repeatable());
}

#[test]
fn only_https_or_loopback_and_no_user_info() {
    assert!(check_endpoint("https://dav.example.org/").is_ok());
    assert_eq!(check_endpoint("http://dav.example.org/"), Err(EndpointError::NotEncrypted));
    assert!(check_endpoint("http://localhost:8080/").is_ok());
    assert_eq!(check_endpoint("https://u:p@dav.example.org/"), Err(EndpointError::Credentials));
    assert_eq!(check_endpoint("ftp://dav.example.org/"), Err(EndpointError::Malformed));
}

#[test]
fn header_values_lose_control_characters_and_responses_cap_fields() {
    let r = HttpRequest::new(Method::Get, "https://dav.example.org/", &[("X-A", "a\r\nInjected: 1"), ("bad name", "x")], Vec::new()).unwrap();
    assert_eq!(r.field("x-a"), Some("aInjected: 1"));
    assert_eq!(r.fields().count(), 1);
    let mut resp = HttpResponse::new(200, Vec::new());
    for i in 0..100 {
        resp = resp.with_field(&format!("x-{i}"), "v");
    }
    resp = resp.with_field("Retry-After", "5").with_field("DAV", "1");
    assert_eq!(resp.header("retry-after"), Some("5"));
    assert_eq!(resp.header("dav"), Some("1"));
    assert!(resp.with_field("x-long", &"a".repeat(MAX_FIELD_VALUE_LEN + 1)).header("x-long").is_none());
}

#[test]
fn basic_refuses_a_colon_in_the_user_name() {
    assert_eq!(Secret::basic("a:b", "pw"), Err(CredentialError));
    assert!(Secret::basic("ab", "p:w").is_ok());
}
