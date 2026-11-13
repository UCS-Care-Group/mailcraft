// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Unsubscribe: which way a message offers, from its headers alone.
//!
//! The lists, hosts and addresses are invented.

use proptest::prelude::*;

use crate::unsubscribe::{MAX_ADDRESS, Way, addresses, mailto, way};
use crate::{Address, Message};

fn with(headers: &[(&str, &str)]) -> Message {
    Message {
        from: Address::new("Lumen News", "news@lumen.example"),
        headers: headers.iter().map(|(h, v)| ((*h).into(), (*v).into())).collect(),
        ..Message::default()
    }
}

const POST: (&str, &str) = ("List-Unsubscribe-Post", "List-Unsubscribe=One-Click");

#[test]
fn one_click_needs_the_post_header_and_an_https_address() {
    let m = with(&[("List-Unsubscribe", "<mailto:leave@lumen.example>, <https://lumen.example/u/7f3a>"), POST]);
    assert_eq!(way(&m), Some(Way::OneClick { url: "https://lumen.example/u/7f3a".into() }));
    // Without the post header the web address is a page to open, and mail comes first.
    let m = with(&[("List-Unsubscribe", "<https://lumen.example/u/7f3a>, <mailto:leave@lumen.example?subject=leave>")]);
    assert_eq!(way(&m), Some(Way::Mail { to: "leave@lumen.example".into(), subject: "leave".into(), body: String::new() }));
    let m = with(&[("List-Unsubscribe", "<https://lumen.example/u/7f3a>")]);
    assert_eq!(way(&m), Some(Way::Page { url: "https://lumen.example/u/7f3a".into() }));
    // An `http:` address is never posted to, only opened.
    let m = with(&[("List-Unsubscribe", "<http://lumen.example/u/7f3a>"), POST]);
    assert_eq!(way(&m), Some(Way::Page { url: "http://lumen.example/u/7f3a".into() }));
}

#[test]
fn the_post_header_must_say_one_click() {
    for value in ["List-Unsubscribe=One-Click", " list-unsubscribe = One-Click ", "LIST-UNSUBSCRIBE=One-Click"] {
        let m = with(&[("List-Unsubscribe", "<https://lumen.example/u>"), ("List-Unsubscribe-Post", value)]);
        assert_eq!(way(&m).map(|w| w.kind()), Some("oneClick"), "{value}");
    }
    for value in ["", "One-Click", "List-Unsubscribe=Yes", "List-Unsubscribe=one-click-please"] {
        let m = with(&[("List-Unsubscribe", "<https://lumen.example/u>"), ("List-Unsubscribe-Post", value)]);
        assert_eq!(way(&m).map(|w| w.kind()), Some("page"), "{value}");
    }
}

#[test]
fn no_header_or_nothing_usable_is_no_way() {
    assert_eq!(way(&with(&[])), None);
    for value in
        ["", "leave@lumen.example", "<javascript:alert(1)>", "<ftp://lumen.example/u>", "<mailto:>", "<mailto:not-an-address>", "<data:text/html,hi>"]
    {
        assert_eq!(way(&with(&[("List-Unsubscribe", value), POST])), None, "{value}");
    }
}

#[test]
fn a_folded_header_and_its_case_are_read() {
    let m = with(&[
        ("list-unsubscribe", "<https://lumen.example/u/\r\n 7f3a>,\r\n <mailto:leave@lumen.example>"),
        ("LIST-UNSUBSCRIBE-POST", "List-Unsubscribe=One-Click"),
    ]);
    assert_eq!(way(&m), Some(Way::OneClick { url: "https://lumen.example/u/7f3a".into() }));
}

#[test]
fn a_mailto_address_gives_one_mailbox_its_subject_and_its_body() {
    assert_eq!(
        mailto("mailto:leave%2Bnews@lumen.example?subject=Unsubscribe%20me&body=Please%0Aremove"),
        Some(Way::Mail { to: "leave+news@lumen.example".into(), subject: "Unsubscribe me".into(), body: "Please\nremove".into() })
    );
    // Only the first of several, and no header beyond subject and body.
    assert_eq!(mailto("MAILTO:a@lumen.example,b@lumen.example?cc=c@lumen.example&bcc=d@lumen.example").map(|w| w.kind()), Some("mail"));
    assert!(matches!(mailto("mailto:a@lumen.example,b@lumen.example?cc=x@lumen.example"), Some(Way::Mail { to, .. }) if to == "a@lumen.example"));
    // A subject can't carry a header of its own.
    assert!(
        matches!(mailto("mailto:a@lumen.example?subject=hi%0D%0ABcc:%20x@evil.example"), Some(Way::Mail { subject, .. }) if !subject.contains('\n') && !subject.contains('\r'))
    );
    for bad in [
        "mailto:",
        "mailto:@lumen.example",
        "mailto:a@",
        "mailto:a b@lumen.example",
        "mailto:\"a\"@lumen.example",
        "mailto:a@lumen",
        "https://lumen.example",
    ] {
        assert_eq!(mailto(bad), None, "{bad}");
    }
}

#[test]
fn addresses_are_bounded() {
    let many: String = (0..50).map(|n| format!("<mailto:a{n}@lumen.example>,")).collect();
    assert_eq!(addresses(&many).len(), 8);
    let long = format!("<https://lumen.example/{}>", "x".repeat(MAX_ADDRESS));
    assert!(addresses(&long).is_empty());
    assert!(addresses("<unclosed").is_empty());
}

proptest! {
    #[test]
    fn hostile_headers_never_panic(list in ".{0,600}", post in ".{0,80}", address in ".{0,300}") {
        let _ = way(&with(&[("List-Unsubscribe", list.as_str()), ("List-Unsubscribe-Post", post.as_str())]));
        let _ = mailto(&address);
        let _ = addresses(&list);
    }
}
