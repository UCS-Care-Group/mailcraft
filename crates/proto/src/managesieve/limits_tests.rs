// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A hostile ManageSieve server: limits, junk, and arbitrary bytes.

use proptest::prelude::*;

use super::parse::{MAX_RESPONSE, MAX_TOKENS, scan};
use super::*;
use crate::script::Script;

const GREETING: &str = "\"SASL\" \"PLAIN XOAUTH2 OAUTHBEARER\"\r\n\"SIEVE\" \"vacation\"\r\nOK\r\n";

/// Every command, against whatever the server says; nothing may panic or hang.
fn drive(server: &[u8]) {
    let secret = String::from_utf8_lossy(server.get(..12).unwrap_or(server)).into_owned();
    let Ok(mut sieve) = ManageSieve::connect(Script::new(server)) else { return };
    let _ = sieve.refresh_capabilities();
    let _ = sieve.authenticate_plain("ada@example.org", &secret);
    let _ = sieve.authenticate_token("ada@example.org", &secret);
    let _ = sieve.list_scripts();
    let _ = sieve.get_script("mailcraft");
    let _ = sieve.have_space("mailcraft", 10);
    let _ = sieve.check_script("keep;");
    let _ = sieve.put_script("mailcraft", "keep;");
    let _ = sieve.set_active("mailcraft");
    let _ = sieve.deactivate();
    let _ = sieve.delete_script("mailcraft");
    let _ = sieve.logout();
}

#[test]
fn a_literal_larger_than_a_script_is_refused_before_it_is_read() {
    let err = ManageSieve::connect(Script::new(format!("{{{}}}\r\n", MAX_SCRIPT + 1))).unwrap_err();
    assert!(matches!(&err, ProtoError::Protocol(why) if why.to_string().contains("bytes in one piece")), "{err:?}");
    // A count too long to be a number at all.
    assert!(ManageSieve::connect(Script::new("{99999999999999999999}\r\n")).is_err());
    // A literal of exactly the limit is read.
    let script = "#".repeat(MAX_SCRIPT);
    let mut server = format!("{GREETING}{{{}}}\r\n", script.len()).into_bytes();
    server.extend_from_slice(script.as_bytes());
    server.extend_from_slice(b"\r\nOK\r\n");
    let mut sieve = ManageSieve::connect(Script::whole(server)).unwrap();
    assert_eq!(sieve.get_script("big").unwrap().map(|s| s.len()), Some(MAX_SCRIPT));
}

#[test]
fn too_many_parts_or_literals_in_one_response_are_refused() {
    let many = format!("\"a\"{}\r\n", " x".repeat(MAX_TOKENS + 1));
    assert!(matches!(ManageSieve::connect(Script::whole(many)), Err(ProtoError::Protocol(_))));
    let literals = "\"a\" {1}\r\nb {1}\r\nc {1}\r\nd {1}\r\ne {1}\r\nf\r\nOK\r\n";
    assert!(matches!(ManageSieve::connect(Script::new(literals)), Err(ProtoError::Protocol(_))));
}

#[test]
fn a_listing_without_end_is_cut_off() {
    let mut server = String::from(GREETING);
    for n in 0..=MAX_DATA_LINES {
        server.push_str(&format!("\"script {n}\"\r\n"));
    }
    server.push_str("OK\r\n");
    let mut sieve = ManageSieve::connect(Script::whole(server)).unwrap();
    assert_eq!(sieve.list_scripts().unwrap_err(), ProtoError::Protocol(said!("the server sent too many responses to one command")));
    // Capabilities without end likewise.
    let endless = "\"X\" \"y\"\r\n".repeat(MAX_DATA_LINES + 1);
    assert!(matches!(ManageSieve::connect(Script::whole(endless)), Err(ProtoError::Protocol(_))));
}

/// A listing MailCraft can't keep whole is refused, never shortened: the script left out could be
/// the active one, or MailCraft's own, and a write would take the listing for the whole (#2058).
#[test]
fn a_listing_with_more_scripts_than_kept_or_a_name_that_cant_be_used_is_refused_whole() {
    // The active script after the cap: an error, not a listing without it.
    let mut server = String::from(GREETING);
    for n in 0..MAX_LISTED {
        server.push_str(&format!("\"script {n}\"\r\n"));
    }
    server.push_str("\"theirs\" ACTIVE\r\nOK\r\n");
    let mut sieve = ManageSieve::connect(Script::whole(server)).unwrap();
    let err = sieve.list_scripts().unwrap_err();
    assert!(matches!(&err, ProtoError::Protocol(why) if why.to_string().contains("more Sieve scripts")), "{err:?}");
    // Up to the cap, with the active one last, every script is kept.
    let mut server = String::from(GREETING);
    for n in 0..MAX_LISTED - 1 {
        server.push_str(&format!("\"script {n}\"\r\n"));
    }
    server.push_str("\"theirs\" ACTIVE\r\nOK\r\n");
    let mut sieve = ManageSieve::connect(Script::whole(server)).unwrap();
    let scripts = sieve.list_scripts().unwrap();
    assert_eq!(scripts.len(), MAX_LISTED);
    assert!(scripts.last().is_some_and(|s| s.active && s.name == "theirs"));
    // A name that can't be sent back, first in the listing and well under the cap: refused, not
    // left out (it could be the active one).
    for bad in ["\"bad\u{85}name\" ACTIVE".as_bytes(), b"\"\"", b"{3}\r\nb\xffd"] {
        let mut server = GREETING.as_bytes().to_vec();
        server.extend_from_slice(bad);
        server.extend_from_slice(b"\r\n\"mailcraft\"\r\nOK\r\n");
        let mut sieve = ManageSieve::connect(Script::whole(server)).unwrap();
        let err = sieve.list_scripts().unwrap_err();
        assert!(matches!(&err, ProtoError::Protocol(why) if why.to_string().contains("name MailCraft can't use")), "{bad:?}: {err:?}");
    }
}

/// What one command keeps is bounded as a whole, not only response by response: a server can't
/// make one LISTSCRIPTS hold a thousand responses of a megabyte each (#2058).
#[test]
fn what_one_command_keeps_is_bounded_across_its_responses() {
    let piece = MAX_SCRIPT / 2;
    let mut server = GREETING.as_bytes().to_vec();
    for _ in 0..3 {
        server.extend_from_slice(format!("{{{piece}}}\r\n").as_bytes());
        server.extend(std::iter::repeat_n(b'x', piece));
        server.extend_from_slice(b"\r\n");
    }
    server.extend_from_slice(b"OK\r\n");
    let mut sieve = ManageSieve::connect(Script::whole(server)).unwrap();
    let err = sieve.list_scripts().unwrap_err();
    assert!(matches!(&err, ProtoError::Protocol(why) if why.to_string().contains("too long")), "{err:?}");
}

/// A script's text that isn't UTF-8 is refused, not mended: written back, it would change.
#[test]
fn a_script_that_isnt_utf8_is_refused_not_mended() {
    let mut server = format!("{GREETING}{{5}}\r\n").into_bytes();
    server.extend_from_slice(b"k\xe9ep");
    server.extend_from_slice(b"\r\nOK\r\n");
    let mut sieve = ManageSieve::connect(Script::whole(server)).unwrap();
    let err = sieve.get_script("mine").unwrap_err();
    assert!(matches!(&err, ProtoError::Protocol(why) if why.to_string().contains("UTF-8")), "{err:?}");
}

/// Exactly as many capabilities as a command may bring, then OK: taken.
#[test]
fn capabilities_up_to_the_limit_are_taken() {
    let mut server = "\"X\" \"y\"\r\n".repeat(MAX_DATA_LINES);
    server.push_str("OK\r\n");
    assert!(ManageSieve::connect(Script::whole(server)).is_ok());
}

#[test]
fn malformed_responses_are_errors() {
    for junk in [
        "\"unterminated\r\nOK\r\n",
        "OK (\r\n",
        "OK (CODE (NESTED)) \"x\"\r\n",
        "OK ()\r\n",
        "{3} trailing\r\nabc\r\nOK\r\n",
        "{}\r\nOK\r\n",
        "{-1}\r\n",
        "\x01\r\n",
        "\r\n",
        "WHAT \"is this\"\r\n",
        "\"a\" {x}\r\n",
        "\"nul\0\"\r\n",
    ] {
        let server = format!("{GREETING}{junk}");
        let mut sieve = ManageSieve::connect(Script::new(server)).unwrap();
        assert!(matches!(sieve.list_scripts(), Err(ProtoError::Protocol(_) | ProtoError::Io(_))), "{junk:?}");
    }
}

#[test]
fn a_response_over_its_size_is_refused() {
    // Lines up to the limit of each, joined by literals: the whole stays bounded.
    let mut server = format!("{GREETING}\"a\" {{{}}}\r\n", MAX_SCRIPT).into_bytes();
    server.extend(std::iter::repeat_n(b'x', MAX_SCRIPT));
    server.extend_from_slice(format!(" {{{}}}\r\n", MAX_RESPONSE - MAX_SCRIPT).as_bytes());
    let mut sieve = ManageSieve::connect(Script::whole(server)).unwrap();
    assert!(matches!(sieve.list_scripts(), Err(ProtoError::Protocol(_))));
}

#[test]
fn server_text_with_secrets_or_control_characters_is_tidied() {
    let server = format!("{GREETING}NO \"pw hunter2\\r\\nis\x07wrong\"\r\n");
    let mut sieve = ManageSieve::connect(Script::new(server)).unwrap();
    let err = sieve.authenticate_plain("ada", "hunter2").unwrap_err().to_string();
    assert!(!err.contains("hunter2") && !err.contains('\x07'), "{err}");
}

proptest! {
    #[test]
    fn arbitrary_server_bytes_never_panic_or_hang(server in proptest::collection::vec(any::<u8>(), 0..800)) {
        drive(&server);
        let mut with_greeting = GREETING.as_bytes().to_vec();
        with_greeting.extend_from_slice(&server);
        drive(&with_greeting);
    }

    #[test]
    fn response_shaped_noise_never_panics(lines in proptest::collection::vec(
        (prop_oneof![Just("OK"), Just("NO"), Just("BYE"), Just("\"x\""), Just("{3}"), Just("(")], "[ -~]{0,30}"),
        0..30,
    )) {
        let server: String = lines.iter().map(|(start, rest)| format!("{start} {rest}\r\n")).collect();
        let mut with_greeting = String::from(GREETING);
        with_greeting.push_str(&server);
        drive(with_greeting.as_bytes());
    }

    /// Whatever the bytes of a name, a token or a script, what is sent of them reads back as one
    /// string and nothing more: no quote, backslash or line break in them can end it early and
    /// put a command of their own on the line.
    #[test]
    fn a_string_sent_is_one_string_and_nothing_else(bytes in proptest::collection::vec(prop_oneof![
        Just(b'"'), Just(b'\\'), Just(b'\r'), Just(b'\n'), Just(b'{'), Just(b'}'), Just(b'+'), Just(b' '), any::<u8>(),
    ], 0..1100)) {
        let sent = string(&bytes);
        let mut tokens = Vec::new();
        // Up to the end of the first line: a quoted string, or the count of a literal.
        let end = sent.iter().position(|b| *b == b'\n').map_or(sent.len(), |at| at + 1);
        let first = sent[..end].strip_suffix(b"\r\n").unwrap_or(&sent[..end]);
        match scan(first, &mut tokens) {
            Ok(None) => {
                prop_assert_eq!(end, sent.len());
                prop_assert_eq!(tokens, vec![Token::Str(bytes)]);
            }
            Ok(Some(n)) => {
                prop_assert!(tokens.is_empty());
                prop_assert_eq!(n, bytes.len());
                prop_assert_eq!(&sent[end..], &bytes[..]);
            }
            Err(e) => prop_assert!(false, "{e:?}"),
        }
    }
}
