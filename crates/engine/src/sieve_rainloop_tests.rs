// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The filter blocks of a webmail of the RainLoop kind (#9999): their names, whether they are on,
//! and where they are, read within caps; hostile ones never panic and show nothing they shouldn't.

use proptest::prelude::*;
use serde_json::json;

use crate::cmd::base64_encode;
use crate::sieve::lex;
use crate::sieve_rainloop::{Filter, MAX_JSON, MAX_NAME, filters, settings, signed};

/// The comment a filter block starts with: its settings in base64, in lines of 74.
pub(crate) fn header(id: &str, settings: &str) -> String {
    let b64 = base64_encode(settings.as_bytes());
    let lines: Vec<String> = b64.as_bytes().chunks(74).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
    format!("/*\r\nBEGIN:FILTER:{id}\r\nBEGIN:HEADER\r\n{}\r\nEND:HEADER\r\n*/", lines.join("\r\n"))
}

fn read(script: &str) -> (bool, Vec<Filter>) {
    let comments = lex(script).unwrap().comments;
    (signed(script, &comments), filters(script, &comments))
}

#[test]
fn a_filter_blocks_name_switch_and_place_are_read() {
    let on = json!({"ID": "f1", "Enabled": true, "Name": "Spam", "Conditions": []}).to_string();
    let off = json!({"ID": "f2", "Enabled": false, "Name": "Old"}).to_string();
    let script = format!(
        "require \"fileinto\";\r\n# RAINLOOP:SIEVE\r\n\r\n{}\r\n\r\nif true {{ stop; }}\r\n\r\n/* END:FILTER */\r\n\r\n{}\r\n/* END:FILTER */\r\n",
        header("f1", &on),
        header("f2", &off)
    );
    let (yes, f) = read(&script);
    assert!(yes);
    assert_eq!(f.len(), 2);
    assert_eq!((f[0].id.as_str(), f[0].name.as_deref(), f[0].enabled), ("f1", Some("Spam"), Some(true)));
    assert_eq!((f[1].id.as_str(), f[1].name.as_deref(), f[1].enabled), ("f2", Some("Old"), Some(false)));
    for one in &f {
        let begin = script.get(one.header.clone()).unwrap();
        assert!(begin.starts_with("/*") && begin.ends_with("*/") && begin.contains(&format!("BEGIN:FILTER:{}", one.id)));
        assert_eq!(script.get(one.end.clone().unwrap()), Some("/* END:FILTER */"));
    }
}

#[test]
fn only_the_exact_line_says_a_script_is_one() {
    for (script, yes) in [
        ("# RAINLOOP:SIEVE\r\nkeep;", true),
        ("# RAINLOOP:SIEVE", true),
        ("#RAINLOOP:SIEVE\r\nkeep;", false),
        ("# RAINLOOP:SIEVE too\r\nkeep;", false),
        ("# rainloop:sieve\r\nkeep;", false),
        ("/* # RAINLOOP:SIEVE */ keep;", false),
        ("if header :is \"x\" \"\\n# RAINLOOP:SIEVE\\n\" { keep; }", false),
        ("if header :is \"x\" text:\r\n# RAINLOOP:SIEVE\r\n.\r\n { keep; }", false),
    ] {
        assert_eq!(read(script).0, yes, "{script:?}");
    }
}

#[test]
fn hostile_blocks_give_no_name_and_never_panic() {
    let block = |b64: &str| format!("/*\nBEGIN:FILTER:x\nBEGIN:HEADER\n{b64}\nEND:HEADER\n*/");
    let name_of = |script: &str| read(script).1.first().and_then(|f| f.name.clone());
    // Broken base64, base64 of what isn't JSON, JSON that isn't an object, a name that isn't text.
    for b64 in ["!!!not base64!!!", "eyJOYW1lIjoiU3BhbSJ", &base64_encode(b"Name: Spam"), &base64_encode(b"[\"Spam\"]")] {
        assert_eq!(name_of(&block(b64)), None, "{b64}");
    }
    assert_eq!(name_of(&block(&base64_encode(br#"{"Name": 7}"#))), None);
    // No END:HEADER: nothing is read.
    assert_eq!(name_of(&format!("/*\nBEGIN:FILTER:x\nBEGIN:HEADER\n{}\n*/", base64_encode(br#"{"Name":"Spam"}"#))), None);
    // Huge JSON: past the cap, not read.
    let huge = json!({"Name": "Spam", "Pad": "x".repeat(MAX_JSON)}).to_string();
    assert_eq!(name_of(&block(&base64_encode(huge.as_bytes()))), None);
    // JSON deeper than serde_json reads: not read, no overflow.
    let deep = format!("{{\"Name\":\"Spam\",\"a\":{}{}}}", "[".repeat(5_000), "]".repeat(5_000));
    assert_eq!(settings(&base64_encode(deep.as_bytes())), None);
    // A name of control, format and right-to-left characters: one line, cleaned, capped.
    let rtl = json!({"Name": "\u{202e}Spam\u{0}\r\nnext\u{200b}\u{1b}[31m"}).to_string();
    assert_eq!(name_of(&block(&base64_encode(rtl.as_bytes()))).as_deref(), Some("Spam next[31m"));
    let long = json!({"Name": "é".repeat(MAX_NAME * 3)}).to_string();
    assert_eq!(name_of(&block(&base64_encode(long.as_bytes()))).map(|n| n.chars().count()), Some(MAX_NAME));
    let blank = json!({"Name": "\u{202e}\u{200b} \t"}).to_string();
    assert_eq!(name_of(&block(&base64_encode(blank.as_bytes()))), None);
    // A comment inside a comment ends at the first `*/`: the rest isn't Sieve.
    assert!(lex(&format!("/* {} */ keep;", block("eyJ9"))).is_none());
    // An END:FILTER with no filter open, or two, ends nothing.
    let (_, f) = read(&format!("/* END:FILTER */ {} /* END:FILTER */ /* END:FILTER */", block("e30=")));
    assert_eq!(f.len(), 1);
    assert!(f[0].end.is_some());
}

proptest! {
    #[test]
    fn arbitrary_blocks_never_panic_and_stay_in_the_script(
        inner in ".{0,200}",
        bytes in proptest::collection::vec(any::<u8>(), 0..300),
        name in ".{0,300}",
    ) {
        let b64 = base64_encode(&bytes);
        let named = base64_encode(json!({"Name": name, "Enabled": false}).to_string().as_bytes());
        for script in [
            format!("# RAINLOOP:SIEVE\n/*\nBEGIN:FILTER:{inner}\nBEGIN:HEADER\n{b64}\nEND:HEADER\n*/ keep; /* END:FILTER */"),
            format!("/*\nBEGIN:FILTER:1\nBEGIN:HEADER\n{inner}\nEND:HEADER\n*/"),
            format!("/*\nBEGIN:FILTER:2\nBEGIN:HEADER\n{named}\nEND:HEADER\n*/"),
            format!("/* {inner} */"),
        ] {
            let Some(lexed) = lex(&script) else { continue };
            let _ = signed(&script, &lexed.comments);
            for f in filters(&script, &lexed.comments) {
                prop_assert!(script.get(f.header.clone()).is_some());
                prop_assert!(f.end.clone().is_none_or(|e| script.get(e).is_some()));
                if let Some(n) = &f.name {
                    prop_assert!(n.chars().count() <= MAX_NAME && !n.chars().any(|c| c.is_control() || mailcraft_names::is_format(c)));
                }
            }
        }
        let _ = settings(&b64);
        let _ = settings(&inner);
    }
}
