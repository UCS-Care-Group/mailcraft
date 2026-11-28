// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! SASL OAUTHBEARER (RFC 7628) over IMAP and SMTP, against scripted servers.

use proptest::prelude::*;

use super::*;
use crate::imap::Imap;
use crate::script::Script;
use crate::smtp::Smtp;

const TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJhbGljZSJ9.c2ln";

fn decoded(payload: &str) -> String {
    String::from_utf8(STANDARD.decode(payload).unwrap()).unwrap()
}

/// What goes out for alice at mail.example.org:993.
fn message() -> String {
    oauthbearer("alice@example.org", "mail.example.org", 993, TOKEN).unwrap()
}

/// The reasons a server gives, in base64.
fn reasons(json: &str) -> String {
    STANDARD.encode(json)
}

#[test]
fn the_message_is_the_gs2_header_then_host_port_and_token() {
    assert_eq!(decoded(&message()), format!("n,a=alice@example.org,\x01host=mail.example.org\x01port=993\x01auth=Bearer {TOKEN}\x01\x01"));
    // A name with a comma or an equals sign is written as RFC 5801 has it.
    let odd = oauthbearer("a,b=c", "mail.example.org", 587, TOKEN).unwrap();
    assert!(decoded(&odd).starts_with("n,a=a=2Cb=3Dc,\x01"), "{}", decoded(&odd));
}

#[test]
fn what_would_break_out_of_the_message_is_refused() {
    for (user, host, token) in [
        ("alice\x01", "mail.example.org", TOKEN),
        ("alice", "mail.example.org", "tok\x01en"),
        ("alice\r\n", "mail.example.org", TOKEN),
        ("alice", "mail.example.org\r\nA2 LOGOUT", TOKEN),
        ("alice", "mail example.org", TOKEN),
        ("alice", "mail.example.org", "tok en"),
        ("alice", "mail.example.org", "tok,en"),
        ("alice", "mail.example.org", ""),
    ] {
        assert!(matches!(oauthbearer(user, host, 993, token), Err(ProtoError::Auth(_))), "{user:?} {host:?} {token:?}");
    }
    assert!(oauthbearer("alice", &"h".repeat(300), 993, TOKEN).is_err());
    assert!(oauthbearer("alice", "mail.example.org", 993, &"t".repeat(40_000)).is_err());
}

#[test]
fn imap_sends_the_token_with_the_command_when_the_server_takes_it_that_way() {
    let mut s =
        Script::new("* OK [CAPABILITY IMAP4rev1 SASL-IR AUTH=OAUTHBEARER AUTH=XOAUTH2] ready\r\nA0001 OK [CAPABILITY IMAP4rev1 IDLE] welcome\r\n");
    let mut imap = Imap::connect(&mut s).unwrap();
    assert!(imap.offers_mechanism("oauthbearer") && imap.offers_mechanism("XOAUTH2") && !imap.offers_mechanism("PLAIN"));
    imap.authenticate_oauthbearer("alice@example.org", "mail.example.org", 993, TOKEN).unwrap();
    assert!(imap.is_authenticated());
    assert_eq!(s.sent(), format!("A0001 AUTHENTICATE OAUTHBEARER {}\r\n", message()));
    assert!(s.finished());
}

#[test]
fn imap_waits_for_the_go_ahead_otherwise() {
    let mut s = Script::new("* OK [CAPABILITY IMAP4rev1 AUTH=OAUTHBEARER] ready\r\n+ \r\nA0001 OK welcome\r\n");
    let mut imap = Imap::connect(&mut s).unwrap();
    imap.authenticate_oauthbearer("alice@example.org", "mail.example.org", 993, TOKEN).unwrap();
    // (Then what the server can do once signed in is asked for, as after any sign-in.)
    assert!(s.sent().starts_with(&format!("A0001 AUTHENTICATE OAUTHBEARER\r\n{}\r\n", message())), "{}", s.sent());
}

#[test]
fn imap_answers_the_reasons_with_0x01_and_keeps_them() {
    let why = reasons(
        r#"{"status":"invalid_token","scope":"openid mail","openid-configuration":"https://id.example.org/.well-known/openid-configuration"}"#,
    );
    let mut s = Script::new(format!(
        "* OK [CAPABILITY IMAP4rev1 SASL-IR AUTH=OAUTHBEARER] ready\r\n+ {why}\r\nA0001 NO [AUTHENTICATIONFAILED] {TOKEN} expired\r\n"
    ));
    let mut imap = Imap::connect(&mut s).unwrap();
    let err = imap.authenticate_oauthbearer("alice@example.org", "mail.example.org", 993, TOKEN).unwrap_err();
    assert!(matches!(&err, ProtoError::Auth(_)));
    assert!(!format!("{err} {err:?} {imap:?}").contains(TOKEN));
    let failure = imap.sasl_failure().unwrap().clone();
    drop(imap);
    assert_eq!(s.sent(), format!("A0001 AUTHENTICATE OAUTHBEARER {}\r\nAQ==\r\n", message()));
    assert!(failure.token_rejected() && !failure.scope_missing());
    assert_eq!(
        (failure.scope.as_str(), failure.openid_configuration.as_str()),
        ("openid mail", "https://id.example.org/.well-known/openid-configuration")
    );
}

#[test]
fn imap_refused_without_reasons_or_with_a_server_that_keeps_asking() {
    let mut s = Script::new("+ \r\nA0001 NO AUTHENTICATE failed.\r\n");
    let mut imap = Imap::resume(&mut s);
    assert_eq!(imap.authenticate_oauthbearer("alice", "mail.example.org", 993, TOKEN), Err(ProtoError::Auth("AUTHENTICATE failed.".into())));
    assert!(imap.sasl_failure().is_none());
    // Reasons that can't be read are no reasons; a third question ends the exchange.
    let mut s = Script::new("+ \r\n+ !!!not base64\r\n+ again\r\nA0001 NO cancelled\r\n");
    let mut imap = Imap::resume(&mut s);
    assert!(matches!(imap.authenticate_oauthbearer("alice", "mail.example.org", 993, TOKEN), Err(ProtoError::Auth(_))));
    assert!(imap.sasl_failure().is_none());
    assert!(s.sent().ends_with("AQ==\r\n*\r\n"), "{}", s.sent());
    // A fourth is more than a sign-in has.
    let mut s = Script::new("+ \r\n+ x\r\n+ y\r\n+ z\r\n");
    assert!(matches!(Imap::resume(&mut s).authenticate_oauthbearer("alice", "mail.example.org", 993, TOKEN), Err(ProtoError::Protocol(_))));
}

#[test]
fn smtp_sees_the_offer_and_signs_in() {
    let mut s = Script::new("250-mail.example.org\r\n250 AUTH PLAIN OAUTHBEARER\r\n334 \r\n235 2.7.0 ok\r\n");
    let mut smtp = Smtp::resume(&mut s);
    let ext = smtp.hello("localhost").unwrap();
    assert!(ext.auth_oauthbearer && !ext.auth_xoauth2);
    smtp.auth_oauthbearer("alice@example.org", "mail.example.org", 587, TOKEN).unwrap();
    let sent = s.sent();
    let lines: Vec<&str> = sent.lines().collect();
    let payload =
        lines.iter().find_map(|l| l.strip_prefix("AUTH OAUTHBEARER ")).map(str::to_string).or_else(|| lines.get(2).map(|l| l.to_string())).unwrap();
    assert!(decoded(&payload).contains("host=mail.example.org\x01port=587\x01auth=Bearer "));
}

#[test]
fn smtp_answers_the_reasons_with_0x01_then_takes_the_refusal() {
    let why = reasons(r#"{"status":"insufficient_scope","scope":"mail"}"#);
    let mut s = Script::new(format!("250-x\r\n250 AUTH OAUTHBEARER\r\n334 {why}\r\n535 5.7.8 no {TOKEN}\r\n"));
    let mut smtp = Smtp::resume(&mut s);
    smtp.hello("localhost").unwrap();
    let err = smtp.auth_oauthbearer("alice@example.org", "mail.example.org", 587, TOKEN).unwrap_err();
    assert!(matches!(&err, ProtoError::Auth(_)));
    assert!(!format!("{err} {err:?}").contains(TOKEN));
    let failure = smtp.sasl_failure().unwrap().clone();
    drop(smtp);
    assert!(s.sent().ends_with("AQ==\r\n"), "{}", s.sent());
    assert!(failure.scope_missing());
    assert_eq!(failure.scope, "mail");
}

#[test]
fn smtp_never_shows_the_token_to_a_server_that_doesnt_offer_it() {
    let mut s = Script::new("250-x\r\n250 AUTH PLAIN XOAUTH2\r\n");
    let mut smtp = Smtp::resume(&mut s);
    smtp.hello("localhost").unwrap();
    assert!(matches!(smtp.auth_oauthbearer("alice", "mail.example.org", 587, TOKEN), Err(ProtoError::Auth(_))));
    assert!(!s.sent().contains("AUTH"));
}

#[test]
fn reasons_are_read_as_hostile() {
    let read = |json: &str| SaslFailure::read(reasons(json).as_bytes());
    // Only what each field may be is kept.
    let odd = read(r#"{"status":"invalid token\r\n","scope":"a\"b","openid-configuration":"http://id.example.org/","extra":"x"}"#).unwrap();
    assert_eq!(odd, SaslFailure::default());
    let odd = read(r#"{"status":7,"scope":["mail"],"openid-configuration":"https://id.example.org/a b"}"#).unwrap();
    assert_eq!(odd, SaslFailure::default());
    // Not JSON, not an object, not base64, too long: nothing.
    assert!(read("[1,2]").is_none() && read("not json").is_none());
    assert!(SaslFailure::read(b"%%%").is_none());
    assert!(SaslFailure::read(reasons(&format!(r#"{{"status":"{}"}}"#, "a".repeat(5_000))).as_bytes()).is_none());
    // A field longer than any is dropped, the others kept.
    let long = read(&format!(r#"{{"status":"invalid_token","scope":"{}"}}"#, "s".repeat(600))).unwrap();
    assert_eq!((long.status.as_str(), long.scope.as_str()), ("invalid_token", ""));
}

proptest! {
    #[test]
    fn the_message_never_breaks_a_line_and_carries_what_it_was_given(user in "[ -~]{0,40}", host in "[a-z0-9.-]{0,40}", port in any::<u16>(), token in "[A-Za-z0-9._~+/-]{1,80}") {
        if let Ok(payload) = oauthbearer(&user, &host, port, &token) {
            prop_assert!(!payload.contains(['\r', '\n', '\0']));
            let text = decoded(&payload);
            let header = text.split('\x01').next().unwrap();
            let name = header.strip_prefix("n,a=").and_then(|rest| rest.strip_suffix(',')).unwrap();
            prop_assert_eq!(name.replace("=2C", ",").replace("=3D", "="), user);
            let auth = format!("auth=Bearer {token}");
            prop_assert!(text.split('\x01').any(|field| field == auth));
        }
    }

    #[test]
    fn reading_reasons_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2_000)) {
        let _ = SaslFailure::read(&bytes);
        let _ = SaslFailure::read(STANDARD.encode(&bytes).as_bytes());
    }
}
