// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Basic credentials: RFC 7617's example, refusals that say nothing of the input, and no secret
//! in `Debug`.

use super::*;

#[test]
fn base64_matches_rfc_4648() {
    for (plain, coded) in
        [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
    {
        assert_eq!(base64(plain.as_bytes()), coded);
    }
}

#[test]
fn the_value_is_rfc_7617s_example() {
    // RFC 7617 section 2: Aladdin and "open sesame".
    assert_eq!(Secret::basic("Aladdin", "open sesame").unwrap().expose(), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    // UTF-8, a domain's backslash and a UPN's at sign are fine; a password may hold a colon.
    assert!(Secret::basic("EXAMPLE\\ada", "pässwörd:1").unwrap().expose().starts_with("Basic "));
    assert!(Secret::basic("ada@example.org", "").is_ok());
}

#[test]
fn bad_names_are_refused_without_saying_them() {
    assert_eq!(Secret::basic("", "x"), Err(CredentialError::NoUserName));
    assert_eq!(Secret::basic("a:b", "x"), Err(CredentialError::ColonInUserName));
    assert_eq!(Secret::basic("ada\r\nX y", "x"), Err(CredentialError::Unusable));
    assert_eq!(Secret::basic("ada", "x\0"), Err(CredentialError::Unusable));
    assert_eq!(Secret::basic(&"a".repeat(5_000), "x"), Err(CredentialError::Unusable));
    for error in [CredentialError::NoUserName, CredentialError::ColonInUserName, CredentialError::Unusable] {
        assert!(!error.to_string().contains("hunter2"));
    }
}

#[test]
fn debug_shows_nothing() {
    let secret = Secret::basic("ada@example.org", "hunter2").unwrap();
    let shown = format!("{secret:?}");
    assert!(!shown.contains("hunter2") && !shown.contains("Basic") && !shown.contains("YWRh"), "{shown}");
}

#[test]
fn a_basic_value_goes_where_the_stand_in_wants_it() {
    // The stand-in's own credential is "standin:password".
    assert_eq!(Secret::basic("standin", "password").unwrap().expose(), crate::standin_autodiscover::BASIC);
}
