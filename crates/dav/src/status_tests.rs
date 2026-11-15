// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn r(code: u16) -> HttpResponse {
    HttpResponse::new(code, Vec::new())
}
fn o() -> Origin {
    Origin::parse("https://dav.example.org").unwrap()
}

#[test]
fn the_table_of_statuses() {
    assert_eq!(classify(&r(207), 0), Verdict::Multistatus);
    for c in [200, 201, 204] {
        assert_eq!(classify(&r(c), 0), Verdict::Done);
    }
    assert_eq!(classify(&r(304), 0), Verdict::NotModified);
    assert_eq!(classify(&r(401), 0), Verdict::Unauthorized);
    assert_eq!(classify(&r(403), 0), Verdict::Forbidden);
    assert_eq!(classify(&r(404), 0), Verdict::Gone);
    assert_eq!(classify(&r(410), 0), Verdict::Gone);
    assert_eq!(classify(&r(412), 0), Verdict::PreconditionFailed);
    assert_eq!(classify(&r(428), 0), Verdict::PreconditionRequired);
    assert_eq!(classify(&r(507), 0), Verdict::InsufficientStorage);
    assert_eq!(classify(&r(302), 0), Verdict::Redirect(302));
    assert_eq!(classify(&r(500), 0), Verdict::Other(500));
}

#[test]
fn an_invalid_sync_token_is_403_or_409_with_the_precondition() {
    let body = b"<d:error xmlns:d=\"DAV:\"><d:valid-sync-token/></d:error>".to_vec();
    for c in [403, 409] {
        assert_eq!(classify(&HttpResponse::new(c, body.clone()), 0), Verdict::SyncTokenInvalid);
    }
    assert_eq!(classify(&HttpResponse::new(403, b"nope".to_vec()), 0), Verdict::Forbidden);
    assert_eq!(classify(&HttpResponse::new(409, b"nope".to_vec()), 0), Verdict::Other(409));
}

#[test]
fn waits_are_capped_and_read_as_seconds_or_dates() {
    assert_eq!(classify(&r(503).with_field("Retry-After", "30"), 0), Verdict::Wait(30));
    assert_eq!(classify(&r(429).with_field("Retry-After", "99999"), 0), Verdict::Wait(MAX_WAIT));
    assert_eq!(classify(&r(429), 0), Verdict::Wait(60));
    assert_eq!(classify(&r(429).with_field("Retry-After", "soon"), 0), Verdict::Wait(60));
    assert_eq!(classify(&r(429).with_field("Retry-After", "99999999999999999999"), 0), Verdict::Wait(60));
    assert_eq!(http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
    assert_eq!(retry_after(Some("Sun, 06 Nov 1994 08:49:37 GMT"), 784_111_777 - 20), 20);
    assert_eq!(retry_after(Some("Sun, 06 Nov 1994 08:49:37 GMT"), 784_111_777 + 20), 0);
    for bad in [
        "",
        "Sun 06 Nov 1994",
        "Sun, 32 Nov 1994 08:49:37 GMT",
        "Sun, 06 Foo 1994 08:49:37 GMT",
        "Sun, 06 Nov 1994 25:49:37 GMT",
        "Sun, 06 Nov 1994 08:49:37 PST",
    ] {
        assert_eq!(http_date(bad), None, "{bad:?}");
    }
}

#[test]
fn a_bare_401_is_never_evidence() {
    let o = o();
    let bare = r(401).with_field("WWW-Authenticate", "Basic realm=\"x\"");
    assert!(!is_dav_evidence(&bare, &o, "https://dav.example.org/"));
    let plain_dav = bare.clone().with_field("DAV", "1, 2");
    assert!(!is_dav_evidence(&plain_dav, &o, "https://dav.example.org/"));
    for header in ["1, calendar-access", "1, 2, addressbook", "1,CALENDAR-ACCESS"] {
        assert!(is_dav_evidence(&bare.clone().with_field("DAV", header), &o, "https://dav.example.org/"), "{header}");
    }
    let good = HttpResponse::new(207, b"<d:multistatus xmlns:d=\"DAV:\"/>".to_vec());
    assert!(is_dav_evidence(&good, &o, "https://dav.example.org/"));
    assert!(!is_dav_evidence(&HttpResponse::new(207, b"<html/>".to_vec()), &o, "https://dav.example.org/"));
    assert!(!is_dav_evidence(&r(200).with_field("DAV", "calendar-access"), &o, "https://dav.example.org/"));
}

#[test]
fn challenges_and_classes_are_lowercased_and_capped() {
    let resp = r(401)
        .with_field("WWW-Authenticate", "Basic realm=\"a\"")
        .with_field("WWW-Authenticate", "Digest nonce=\"b\"")
        .with_field("DAV", "1, 2, Calendar-Access");
    assert_eq!(challenges(&resp), ["basic", "digest"]);
    assert_eq!(dav_classes(&resp), ["1", "2", "calendar-access"]);
}

#[test]
fn multistatus_wants_a_207() {
    assert!(multistatus(&r(200), &o(), "https://dav.example.org/").is_err());
}

#[test]
fn challenges_reads_every_scheme_in_one_header() {
    let resp = r(401).with_field("WWW-Authenticate", "Negotiate, Basic realm=\"a, b\", charset=\"UTF-8\", Digest realm=\"x\", nonce=\"n\"");
    assert_eq!(challenges(&resp), ["negotiate", "basic", "digest"]);
    assert_eq!(challenges(&r(401).with_field("WWW-Authenticate", "Basic")), ["basic"]);
}

#[test]
fn a_hostile_http_date_never_overflows() {
    for bad in [
        "Sun, 06 Nov 1994 -9223372036854775808:00:00 GMT",
        "Sun, 06 Nov 1994 08:-1:00 GMT",
        "Sun, 06 Nov 1994 +8:49:37 GMT",
        "Sun, 06 Nov 99999999999999999999 08:49:37 GMT",
        "Sun, 06 Nov 9999999999999999 08:49:37 GMT",
        "Sun, 06 Nov 1994 8:49:37 GMT",
        "Sun, 06 Nov 1994 08:49:37:00 GMT",
        "Sun, 06 Nov 1994 08:49:37 GMT extra",
        "Sun, +6 Nov 1994 08:49:37 GMT",
        "Sun, 06 Nov -999 08:49:37 GMT",
    ] {
        assert_eq!(http_date(bad), None, "{bad:?}");
        assert_eq!(retry_after(Some(bad), 0), 60);
        assert_eq!(classify(&r(429).with_field("Retry-After", bad), i64::MIN), Verdict::Wait(60));
    }
    assert_eq!(retry_after(Some("Sun, 06 Nov 9999 08:49:37 GMT"), i64::MIN), MAX_WAIT);
    assert_eq!(http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
    assert_eq!(status_code_digits(), None);
}

fn status_code_digits() -> Option<u16> {
    crate::answer::status_code("HTTP/1.1 +20 OK")
}

#[test]
fn a_303_is_a_redirect() {
    assert_eq!(classify(&r(303), 0), Verdict::Redirect(303));
}

#[test]
fn only_a_dav_error_with_the_precondition_is_an_invalid_token() {
    for body in [
        "valid-sync-token",
        "<html>valid-sync-token</html>",
        "<d:error xmlns:d=\"DAV:\"><d:other/></d:error>",
        "<x:error xmlns:x=\"urn:x\"><x:valid-sync-token/></x:error>",
        "<d:error xmlns:d=\"DAV:\"><d:valid-sync-token/>",
    ] {
        assert_eq!(classify(&HttpResponse::new(403, body.as_bytes().to_vec()), 0), Verdict::Forbidden, "{body}");
    }
}

#[test]
fn civil_seconds_is_checked_at_the_extremes() {
    for year in [i64::MIN, i64::MIN + 1, i64::MAX, i64::MAX - 1, i64::MAX / 2, i64::MIN / 2] {
        for month in [1, 2, 3, 12] {
            let _ = civil_seconds(year, month, 31, i64::MAX);
            let _ = civil_seconds(year, month, 1, i64::MIN);
        }
    }
    assert_eq!(civil_seconds(i64::MIN, 6, 1, 0), None);
    assert_eq!(civil_seconds(1970, 1, 1, 5), Some(5));
}
