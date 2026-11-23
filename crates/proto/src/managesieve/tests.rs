// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ManageSieve client against scripted servers.

use super::*;
use crate::script::{Broken, Script};

const GREETING: &str = "\"IMPLEMENTATION\" \"Example Sieve 1.0\"\r\n\"SASL\" \"PLAIN OAUTHBEARER xoauth2\"\r\n\"SIEVE\" \"fileinto Vacation imap4flags date relational copy\"\r\n\"STARTTLS\"\r\n\"VERSION\" \"1.0\"\r\n\"MAXREDIRECTS\" \"4\"\r\n\"OWNER\" \"ada@example.org\"\r\nOK \"Ready.\"\r\n";

/// A session past the greeting with `server`'s answers after it; the script, to look at what was sent.
fn session(server: &str, f: impl FnOnce(&mut ManageSieve<&mut Script>)) -> Script {
    let mut s = Script::new(format!("{GREETING}{server}"));
    let mut sieve = ManageSieve::connect(&mut s).unwrap();
    f(&mut sieve);
    drop(sieve);
    s
}

fn rejected<T: std::fmt::Debug>(r: Result<T, ProtoError>) -> String {
    match r {
        Err(ProtoError::Rejected(text)) => text.to_string(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn the_greeting_says_what_the_server_can_do() {
    let mut caps = Capabilities::default();
    session("", |sieve| caps = sieve.capabilities().clone());
    assert_eq!(
        caps,
        Capabilities {
            implementation: "Example Sieve 1.0".into(),
            sasl: vec!["PLAIN".into(), "OAUTHBEARER".into(), "XOAUTH2".into()],
            extensions: ["fileinto", "vacation", "imap4flags", "date", "relational", "copy"].map(String::from).to_vec(),
            starttls: true,
            version: Some("1.0".into()),
            max_redirects: Some(4),
        }
    );
    assert!(caps.has_extension("VACATION") && !caps.has_extension("include"));
    assert!(caps.offers("xoauth2") && !caps.offers("LOGIN"));
}

#[test]
fn a_greeting_that_isnt_one_is_refused() {
    for server in ["* OK IMAP4rev1 ready\r\n", "220 smtp.example.org ESMTP\r\n", "HELLO\r\n"] {
        let err = ManageSieve::connect(Script::new(server)).unwrap_err();
        assert_eq!(err, ProtoError::Protocol(said!("the server didn't greet like a ManageSieve server")), "{server:?}");
    }
    let err = ManageSieve::connect(Script::new("NO \"go away\"\r\n")).unwrap_err();
    assert!(matches!(&err, ProtoError::Protocol(why) if why.to_string().contains("go away")), "{err:?}");
    let err = ManageSieve::connect(Script::new("\"SASL\" \"PLAIN\"\r\nBYE \"too busy\"\r\n")).unwrap_err();
    assert!(matches!(&err, ProtoError::Io(why) if why.to_string().contains("too busy")), "{err:?}");
}

#[test]
fn starttls_hands_the_stream_back_and_the_capabilities_are_read_again() {
    let mut s = Script::new(format!("{GREETING}OK \"Begin TLS negotiation now.\"\r\n"));
    let sieve = ManageSieve::connect(&mut s).unwrap();
    let stream = sieve.starttls().unwrap();
    assert!(stream.sent().ends_with("STARTTLS\r\n"));
    // After the handshake: what the server says now is all that counts (no STARTTLS any more).
    let mut s = Script::new("\"SASL\" \"PLAIN\"\r\n\"SIEVE\" \"vacation\"\r\nOK\r\n");
    let sieve = ManageSieve::after_starttls(&mut s).unwrap();
    assert!(!sieve.capabilities().starttls);
    assert_eq!(sieve.capabilities().sasl, ["PLAIN"]);
}

#[test]
fn bytes_slipped_in_before_tls_are_refused() {
    let mut s = Script::whole(format!("{GREETING}OK \"go\"\r\n\"SASL\" \"PLAIN\"\r\n"));
    let sieve = ManageSieve::connect(&mut s).unwrap();
    assert_eq!(sieve.starttls().unwrap_err(), ProtoError::Protocol(said!("the server sent data before the secure connection started")));
}

#[test]
fn a_refused_starttls_is_an_error() {
    let mut s = Script::new(format!("{GREETING}NO \"not today\"\r\n"));
    let sieve = ManageSieve::connect(&mut s).unwrap();
    assert_eq!(rejected(sieve.starttls()), "not today");
}

#[test]
fn plain_sends_the_user_and_password_in_one_go() {
    let mut s = Script::new(format!("{GREETING}OK \"Logged in.\"\r\n"));
    let mut sieve = ManageSieve::connect(&mut s).unwrap();
    sieve.authenticate_plain("ada@example.org", "hunter2").unwrap();
    assert!(sieve.is_authenticated());
    let payload = STANDARD.encode("\0ada@example.org\0hunter2");
    assert!(s.sent().ends_with(&format!("AUTHENTICATE \"PLAIN\" \"{payload}\"\r\n")), "{}", s.sent());
}

#[test]
fn a_refused_password_is_a_sign_in_error_that_doesnt_repeat_it() {
    let payload = STANDARD.encode("\0ada@example.org\0hunter2");
    let mut s = Script::new(format!("{GREETING}NO (AUTH-TOO-WEAK) \"bad password hunter2 {payload}\"\r\n"));
    let mut sieve = ManageSieve::connect(&mut s).unwrap();
    let err = sieve.authenticate_plain("ada@example.org", "hunter2").unwrap_err();
    let said = err.to_string();
    assert!(matches!(err, ProtoError::Auth(_)), "{err:?}");
    assert!(said.contains("bad password") && !said.contains("hunter2") && !said.contains(&payload), "{said}");
}

#[test]
fn a_password_is_never_sent_to_a_server_that_doesnt_take_one() {
    let mut s = Script::new("\"SASL\" \"OAUTHBEARER\"\r\nOK\r\n");
    let mut sieve = ManageSieve::connect(&mut s).unwrap();
    assert_eq!(
        sieve.authenticate_plain("ada@example.org", "hunter2").unwrap_err(),
        ProtoError::Auth(said!("the server offers no way to sign in with a password"))
    );
    let mut s = Script::new("\"SASL\" \"PLAIN\"\r\nOK\r\n");
    let mut sieve = ManageSieve::connect(&mut s).unwrap();
    assert!(sieve.authenticate_token("ada@example.org", "tok").is_err());
    for (user, password) in [("", "pw"), ("ada", ""), ("a\0da", "pw"), ("ada", "p\0w")] {
        assert!(matches!(sieve.authenticate_plain(user, password), Err(ProtoError::Auth(_))), "{user:?}");
    }
    assert!(!s.sent().contains("AUTHENTICATE"), "{}", s.sent());
}

#[test]
fn a_question_after_plain_is_cancelled() {
    let mut s = Script::new(format!("{GREETING}\"cXVlc3Rpb24=\"\r\nNO \"cancelled\"\r\n"));
    let mut sieve = ManageSieve::connect(&mut s).unwrap();
    let err = sieve.authenticate_plain("ada@example.org", "hunter2").unwrap_err();
    assert_eq!(err, ProtoError::Auth(said!("the server asked for more than a user name and password")));
    assert!(s.sent().ends_with("\"*\"\r\n"), "{}", s.sent());
}

#[test]
fn oauthbearer_is_preferred_and_a_refusal_is_answered_with_control_a() {
    let mut s = Script::new(format!("{GREETING}{{8}}\r\neyJ9Ig==\r\nNO \"invalid token\"\r\n"));
    let mut sieve = ManageSieve::connect(&mut s).unwrap();
    let err = sieve.authenticate_token("ada@example.org", "tok.en").unwrap_err();
    assert!(matches!(&err, ProtoError::Auth(why) if why.to_string() == "invalid token"), "{err:?}");
    let sent = s.sent();
    let payload = STANDARD.encode("n,a=ada@example.org,\x01auth=Bearer tok.en\x01\x01");
    assert!(sent.contains(&format!("AUTHENTICATE \"OAUTHBEARER\" \"{payload}\"\r\n")), "{sent}");
    assert!(sent.ends_with("\"AQ==\"\r\n"), "{sent}");
}

#[test]
fn xoauth2_when_oauthbearer_isnt_offered() {
    let mut s = Script::new("\"SASL\" \"XOAUTH2\"\r\nOK\r\nOK (SASL \"ZG9uZQ==\") \"Logged in.\"\r\n");
    let mut sieve = ManageSieve::connect(&mut s).unwrap();
    sieve.authenticate_token("ada@example.org", "tok.en").unwrap();
    assert!(sieve.is_authenticated());
    let payload = xoauth2("ada@example.org", "tok.en").unwrap();
    assert!(s.sent().contains(&format!("AUTHENTICATE \"XOAUTH2\" \"{payload}\"")), "{}", s.sent());
}

#[test]
fn a_long_token_goes_as_a_literal() {
    let token = "t".repeat(2000);
    let mut s = Script::new("\"SASL\" \"XOAUTH2\"\r\nOK\r\nOK\r\n");
    let mut sieve = ManageSieve::connect(&mut s).unwrap();
    sieve.authenticate_xoauth2("ada@example.org", &token).unwrap();
    let payload = xoauth2("ada@example.org", &token).unwrap();
    assert!(s.sent().contains(&format!("AUTHENTICATE \"XOAUTH2\" {{{}+}}\r\n{payload}\r\n", payload.len())), "{}", s.sent());
}

#[test]
fn the_oauthbearer_header_escapes_the_user() {
    let decoded = |payload: String| String::from_utf8(STANDARD.decode(payload).unwrap()).unwrap();
    assert_eq!(decoded(oauthbearer("a,b=c@example.org", "t").unwrap()), "n,a=a=2Cb=3Dc@example.org,\x01auth=Bearer t\x01\x01");
    for (user, token) in [("ada\x01", "t"), ("ada", "t\r\n"), ("ada", "")] {
        assert!(matches!(oauthbearer(user, token), Err(ProtoError::Auth(_))), "{user:?} {token:?}");
    }
}

#[test]
fn scripts_are_listed_with_the_active_one() {
    let s = session("\"vacation\"\r\n{9}\r\nmailcraft ACTIVE\r\n\"old \\\"rules\\\"\"\r\nOK \"Listscripts completed.\"\r\n", |sieve| {
        assert_eq!(
            sieve.list_scripts().unwrap(),
            vec![
                ScriptEntry { name: "vacation".into(), active: false },
                ScriptEntry { name: "mailcraft".into(), active: true },
                ScriptEntry { name: "old \"rules\"".into(), active: false },
            ]
        );
    });
    assert_eq!(s.sent(), "LISTSCRIPTS\r\n");
}

#[test]
fn a_script_is_read_whole_from_its_literal() {
    let script = "require \"vacation\";\r\nvacation \"Away\";\r\n";
    let server =
        format!("{{{}}}\r\n{script}\r\nOK \"Getscript completed.\"\r\nNO (NONEXISTENT) \"There is no script by that name\"\r\n", script.len());
    let s = session(&server, |sieve| {
        assert_eq!(sieve.get_script("mailcraft").unwrap().as_deref(), Some(script));
        assert_eq!(sieve.get_script("gone").unwrap(), None);
    });
    assert_eq!(s.sent(), "GETSCRIPT \"mailcraft\"\r\nGETSCRIPT \"gone\"\r\n");
}

#[test]
fn a_getscript_without_a_script_is_an_error() {
    session("OK\r\n\"a\"\r\n\"b\"\r\nOK\r\n", |sieve| {
        for _ in 0..2 {
            assert_eq!(sieve.get_script("x").unwrap_err(), ProtoError::Protocol(said!("the server didn't send the script")));
        }
    });
}

#[test]
fn putscript_sends_the_script_as_a_literal_byte_for_byte() {
    let script = "require [\"fileinto\"];\r\n# \"quoted\" \\ and\r\n.\r\nfileinto \"Lists\";\r\n";
    let s = session("OK\r\nOK (WARNINGS) \"line 2: unused\"\r\nNO \"line 1: unknown extension\"\r\n", |sieve| {
        assert_eq!(sieve.put_script("mailcraft", script).unwrap(), None);
        assert_eq!(sieve.put_script("mailcraft", script).unwrap().as_deref(), Some("line 2: unused"));
        assert_eq!(rejected(sieve.put_script("mailcraft", "junk")), "line 1: unknown extension");
    });
    let sent = s.sent();
    assert!(sent.starts_with(&format!("PUTSCRIPT \"mailcraft\" {{{}+}}\r\n{script}\r\n", script.len())), "{sent}");
}

#[test]
fn checkscript_setactive_deletescript_and_havespace() {
    let server =
        "OK\r\nOK\r\nOK\r\nOK\r\nNO (ACTIVE) \"You may not delete an active script\"\r\nOK\r\nNO (QUOTA/MAXSIZE) \"Too big\"\r\nNO \"Huh\"\r\n";
    let s = session(server, |sieve| {
        assert_eq!(sieve.check_script("keep;").unwrap(), None);
        sieve.set_active("mailcraft").unwrap();
        sieve.deactivate().unwrap();
        sieve.delete_script("old").unwrap();
        assert_eq!(rejected(sieve.delete_script("mailcraft")), "You may not delete an active script");
        assert!(sieve.have_space("mailcraft", 100).unwrap());
        assert!(!sieve.have_space("mailcraft", 10_000_000).unwrap());
        assert_eq!(rejected(sieve.have_space("mailcraft", 1)), "Huh");
    });
    assert_eq!(
        s.sent(),
        "CHECKSCRIPT {5+}\r\nkeep;\r\nSETACTIVE \"mailcraft\"\r\nSETACTIVE \"\"\r\nDELETESCRIPT \"old\"\r\nDELETESCRIPT \"mailcraft\"\r\nHAVESPACE \"mailcraft\" 100\r\nHAVESPACE \"mailcraft\" 10000000\r\nHAVESPACE \"mailcraft\" 1\r\n"
    );
}

#[test]
fn a_refusal_without_words_says_its_code_or_that_it_was_refused() {
    session("NO (TRYLATER)\r\nNO\r\n", |sieve| {
        assert_eq!(rejected(sieve.set_active("a")), "TRYLATER");
        assert_eq!(sieve.set_active("a").unwrap_err(), ProtoError::Rejected(said!("the command was refused")));
    });
}

#[test]
fn a_referral_is_never_followed() {
    for answer in ["BYE (REFERRAL \"sieve://elsewhere.example.org\") \"Try Remote.\"\r\n", "NO (REFERRAL \"sieve://elsewhere.example.org\")\r\n"] {
        session(answer, |sieve| {
            let err = sieve.list_scripts().unwrap_err();
            assert_eq!(err, referral(), "{answer:?}");
            assert!(!err.to_string().contains("elsewhere"));
        });
    }
    // At sign-in too.
    session("BYE (REFERRAL \"sieve://elsewhere.example.org\")\r\n", |sieve| {
        assert_eq!(sieve.authenticate_plain("ada", "pw").unwrap_err(), referral());
    });
}

#[test]
fn bye_in_the_middle_ends_the_session() {
    session("BYE \"Shutting down\"\r\n", |sieve| {
        let err = sieve.list_scripts().unwrap_err();
        assert!(matches!(&err, ProtoError::Io(why) if why.to_string().contains("Shutting down")), "{err:?}");
    });
}

#[test]
fn names_that_cant_be_sent_are_refused_before_anything_is() {
    let long = "n".repeat(MAX_NAME + 1);
    let s = session("", |sieve| {
        for name in ["", "a\r\nDELETESCRIPT \"x\"", "a\nb", "a\0b", "tab\there", "\u{2028}", "\u{85}", long.as_str()] {
            assert!(matches!(sieve.get_script(name), Err(ProtoError::Protocol(_))), "{name:?}");
            assert!(matches!(sieve.put_script(name, "keep;"), Err(ProtoError::Protocol(_))), "{name:?}");
            assert!(matches!(sieve.set_active(name), Err(ProtoError::Protocol(_))), "{name:?}");
            assert!(matches!(sieve.delete_script(name), Err(ProtoError::Protocol(_))), "{name:?}");
            assert!(matches!(sieve.have_space(name, 1), Err(ProtoError::Protocol(_))), "{name:?}");
        }
    });
    assert_eq!(s.sent(), "");
    // Quotes and backslashes are escaped, other letters go as they are.
    assert_eq!(string("a \"b\" \\c ä".as_bytes()), "\"a \\\"b\\\" \\\\c ä\"".as_bytes());
}

#[test]
fn a_script_too_large_to_send_is_refused() {
    let big = "#".repeat(MAX_SCRIPT + 1);
    let s = session("", |sieve| {
        assert!(matches!(sieve.put_script("mailcraft", &big), Err(ProtoError::Protocol(_))));
        assert!(matches!(sieve.check_script(&big), Err(ProtoError::Protocol(_))));
    });
    assert_eq!(s.sent(), "");
}

#[test]
fn logout_takes_any_goodbye() {
    for answer in ["OK \"Logout completed.\"\r\n", "BYE\r\n", ""] {
        let mut s = Script::new(format!("{GREETING}{answer}"));
        ManageSieve::connect(&mut s).unwrap().logout().unwrap();
        assert_eq!(s.sent(), "LOGOUT\r\n");
    }
}

#[test]
fn a_broken_stream_is_a_connection_problem() {
    assert!(matches!(ManageSieve::connect(Broken(std::io::ErrorKind::ConnectionReset)), Err(ProtoError::Io(_))));
}

#[test]
fn capabilities_can_be_asked_for_again() {
    let s = session("\"SIEVE\" \"fileinto vacation include\"\r\n\"SASL\" \"\"\r\nOK\r\n", |sieve| {
        let caps = sieve.refresh_capabilities().unwrap();
        assert!(caps.has_extension("include"));
        assert!(caps.sasl.is_empty());
    });
    assert_eq!(s.sent(), "CAPABILITY\r\n");
}
