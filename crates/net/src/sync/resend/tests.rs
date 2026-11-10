// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn listed(id: &str, created: &str) -> Listed {
    let (secs, millis) = instant(created).unwrap();
    Listed { id: id.into(), secs, millis }
}

#[test]
fn a_page_of_resends_list_is_read_and_what_isnt_usable_left_out() {
    let body = br#"{"object":"list","has_more":true,"data":[
        {"id":"a39999a6-88e3-48b1-888b-beaabcde1b33","created_at":"2026-10-09T14:37:40.951Z","subject":"Hello"},
        {"id":"../../evil","created_at":"2026-10-09T14:37:40Z"},
        {"id":"b1","created_at":"not a date"},
        {"created_at":"2026-10-09T14:37:40Z"}]}"#;
    let (page, more) = read_page(body).unwrap();
    assert!(more);
    assert_eq!(page, vec![listed("a39999a6-88e3-48b1-888b-beaabcde1b33", "2026-10-09T14:37:40.951Z")]);
    assert_eq!(page[0].millis, 951);
    for junk in [&b""[..], b"[]", b"{\"data\":7}", b"\xff"] {
        assert!(read_page(junk).is_none());
    }
    assert_eq!(instant("2026-10-09T14:37:40.9Z").unwrap().1, 900);
}

#[test]
fn uids_follow_when_mail_came_and_stay_the_same_from_run_to_run() {
    let first =
        numbered(vec![listed("c", "2026-10-09T14:37:41.000Z"), listed("b", "2026-10-09T14:37:40.951Z"), listed("a", "2026-10-09T14:37:40.9Z")]);
    let ids: Vec<&str> = first.iter().map(|(_, l)| l.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    let uids: Vec<u32> = first.iter().map(|(uid, _)| *uid).collect();
    assert!(uids[0] < uids[1] && uids[1] < uids[2], "{uids:?}");
    // A later run that lists one more in the same second numbers the others as before.
    let later = numbered(vec![
        listed("d", "2026-10-09T14:37:41.500Z"),
        listed("c", "2026-10-09T14:37:41.000Z"),
        listed("b", "2026-10-09T14:37:40.951Z"),
        listed("a", "2026-10-09T14:37:40.9Z"),
    ]);
    assert_eq!(&later[..3], &first[..]);
    assert!(later[3].0 > uids[2]);
    // Mail from before the UIDs' start, and a list that names one twice, never make 0 or two.
    let old = numbered(vec![listed("x", "2020-01-01T00:00:00Z"), listed("x", "2020-01-01T00:00:00Z")]);
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].0, 1);
}

#[test]
fn the_key_goes_to_resends_api_alone_and_the_bytes_come_from_resend() {
    let url = |text: &str| Url::parse(text).unwrap();
    assert!(may_go_to(&url("https://api.resend.com/emails/receiving")).is_ok());
    for elsewhere in ["https://api.resend.com.example/", "https://api.resend.com:8443/", "https://example.resend.com/", "http://127.0.0.1/"] {
        assert!(may_go_to(&url(elsewhere)).is_err(), "{elsewhere}");
    }
    assert!(raw_may_come_from(&url("https://inbound-cdn.resend.com/raw/1?Signature=x")).is_ok());
    for elsewhere in ["https://resend.com.example/raw", "https://evilresend.com/raw", "http://127.0.0.1/raw"] {
        assert!(raw_may_come_from(&url(elsewhere)).is_err(), "{elsewhere}");
    }
    assert!(is_id("4ef9a417-02e9-4d39-ad75-9611e0fcc33c") && !is_id("a/b") && !is_id("") && !is_id(&"a".repeat(65)));
    // A key that can't be one is never sent.
    assert!(matches!(get("", "/emails/receiving"), Err(NetError::Proto(ProtoError::Auth(_)))));
    assert!(matches!(get("re_a b", "/emails/receiving"), Err(NetError::Proto(ProtoError::Auth(_)))));
}
