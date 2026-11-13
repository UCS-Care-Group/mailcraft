// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Unsubscribe: which way a message offers, from its headers alone.
//!
//! The lists, hosts and addresses are invented.

use proptest::prelude::*;

use crate::unsubscribe::{MAX_ADDRESS, Way, addresses, mailto, signed_for_one_click, way};
use crate::{Account, AccountKind, Address, Message, Security, Server};

/// Avery's account, whose server writes the `mx.lumen.example` results.
fn account() -> Account {
    Account {
        kind: AccountKind::Imap,
        email: "avery@lumen.example".into(),
        incoming: Some(Server { host: "imap.lumen.example".into(), port: 993, security: Security::Tls }),
        ..Account::default()
    }
}

/// What the server writes for a list message whose DKIM signature passed.
const PASSED: &str = "mx.lumen.example; dkim=pass header.d=news.example; dmarc=pass header.from=news.example";
/// A signature of the list's that covers both unsubscribe headers.
const SIGNED: &str = "v=1; a=rsa-sha256; d=news.example; s=s1;\r\n h=From:To:Subject:List-Unsubscribe:\r\n List-Unsubscribe-Post; bh=AAAA; b=BBBB";

/// A message from the list, on the server, with `headers`, its server's verdict and its signature.
fn with(headers: &[(&str, &str)]) -> Message {
    let mut m = bare(headers);
    m.headers.push(("Authentication-Results".into(), PASSED.into()));
    m.headers.push(("DKIM-Signature".into(), SIGNED.into()));
    m
}

/// The same with `headers` alone.
fn bare(headers: &[(&str, &str)]) -> Message {
    Message {
        from: Address::new("Lumen News", "letter@news.example"),
        uid: 1,
        headers: headers.iter().map(|(h, v)| ((*h).into(), (*v).into())).collect(),
        ..Message::default()
    }
}

fn of(m: &Message) -> Option<Way> {
    way(Some(&account()), m)
}

const POST: (&str, &str) = ("List-Unsubscribe-Post", "List-Unsubscribe=One-Click");

#[test]
fn one_click_needs_the_post_header_and_an_https_address() {
    let m = with(&[("List-Unsubscribe", "<mailto:leave@lumen.example>, <https://lumen.example/u/7f3a>"), POST]);
    assert_eq!(of(&m), Some(Way::OneClick { url: "https://lumen.example/u/7f3a".into() }));
    // Without the post header the web address is a page to open, and mail comes first.
    let m = with(&[("List-Unsubscribe", "<https://lumen.example/u/7f3a>, <mailto:leave@lumen.example?subject=leave>")]);
    assert_eq!(of(&m), Some(Way::Mail { to: "leave@lumen.example".into(), subject: "leave".into(), body: String::new() }));
    let m = with(&[("List-Unsubscribe", "<https://lumen.example/u/7f3a>")]);
    assert_eq!(of(&m), Some(Way::Page { url: "https://lumen.example/u/7f3a".into() }));
    // An `http:` address is never posted to, only opened.
    let m = with(&[("List-Unsubscribe", "<http://lumen.example/u/7f3a>"), POST]);
    assert_eq!(of(&m), Some(Way::Page { url: "http://lumen.example/u/7f3a".into() }));
}

#[test]
fn one_click_needs_a_passing_signature_that_covers_both_headers() {
    let list = ("List-Unsubscribe", "<https://news.example/u/7f3a>");
    let page = Some(Way::Page { url: "https://news.example/u/7f3a".into() });
    // RFC 8058, 4: anybody can add the headers to a message signed without them.
    let unsigned_headers = "v=1; a=rsa-sha256; d=news.example; s=s1; h=From:To:Subject; bh=AAAA; b=BBBB";
    let only_one = "v=1; d=news.example; h=from:list-unsubscribe; b=BBBB";
    for signature in [unsigned_headers, only_one] {
        let mut m = bare(&[list, POST, ("Authentication-Results", PASSED)]);
        m.headers.push(("DKIM-Signature".into(), signature.into()));
        assert_eq!(of(&m), page, "{signature}");
    }
    // A DMARC pass that rests on SPF alone: no signature passed at all.
    let spf_only = "mx.lumen.example; spf=pass smtp.mailfrom=news.example; dmarc=pass header.from=news.example";
    let mut m = bare(&[list, POST, ("Authentication-Results", spf_only)]);
    m.headers.push(("DKIM-Signature".into(), SIGNED.into()));
    assert!(!signed_for_one_click(Some(&account()), &m));
    assert_eq!(of(&m), page);
    // A passing signature, but by another domain than the one covering the headers.
    let other = "mx.lumen.example; dkim=pass header.d=elsewhere.example; dmarc=pass header.from=news.example";
    let mut m = bare(&[list, POST, ("Authentication-Results", other)]);
    m.headers.push(("DKIM-Signature".into(), SIGNED.into()));
    assert_eq!(of(&m), page);
    // A verdict written by another server than the account's, or no account to say whose it is.
    let foreign = "mx.news.example; dkim=pass header.d=news.example";
    let mut m = bare(&[list, POST, ("Authentication-Results", foreign)]);
    m.headers.push(("DKIM-Signature".into(), SIGNED.into()));
    assert_eq!(of(&m), page);
    assert_eq!(way(None, &with(&[list, POST])), page);
    // Signed as it should be: one click.
    assert_eq!(of(&with(&[list, POST])), Some(Way::OneClick { url: "https://news.example/u/7f3a".into() }));
}

#[test]
fn the_post_header_must_say_one_click() {
    for value in ["List-Unsubscribe=One-Click", " list-unsubscribe = One-Click ", "LIST-UNSUBSCRIBE=One-Click"] {
        let m = with(&[("List-Unsubscribe", "<https://lumen.example/u>"), ("List-Unsubscribe-Post", value)]);
        assert_eq!(of(&m).map(|w| w.kind()), Some("oneClick"), "{value}");
    }
    for value in ["", "One-Click", "List-Unsubscribe=Yes", "List-Unsubscribe=one-click-please"] {
        let m = with(&[("List-Unsubscribe", "<https://lumen.example/u>"), ("List-Unsubscribe-Post", value)]);
        assert_eq!(of(&m).map(|w| w.kind()), Some("page"), "{value}");
    }
}

#[test]
fn no_header_or_nothing_usable_is_no_way() {
    assert_eq!(of(&with(&[])), None);
    for value in
        ["", "leave@lumen.example", "<javascript:alert(1)>", "<ftp://lumen.example/u>", "<mailto:>", "<mailto:not-an-address>", "<data:text/html,hi>"]
    {
        assert_eq!(of(&with(&[("List-Unsubscribe", value), POST])), None, "{value}");
    }
}

#[test]
fn a_folded_header_and_its_case_are_read() {
    let m = with(&[
        ("list-unsubscribe", "<https://lumen.example/u/\r\n 7f3a>,\r\n <mailto:leave@lumen.example>"),
        ("LIST-UNSUBSCRIBE-POST", "List-Unsubscribe=One-Click"),
    ]);
    assert_eq!(of(&m), Some(Way::OneClick { url: "https://lumen.example/u/7f3a".into() }));
}

#[test]
fn a_mailto_address_gives_one_mailbox_its_subject_and_its_body() {
    assert_eq!(
        mailto("mailto:leave%2Bletter@news.example?subject=Unsubscribe%20me&body=Please%0Aremove"),
        Some(Way::Mail { to: "leave+letter@news.example".into(), subject: "Unsubscribe me".into(), body: "Please\nremove".into() })
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
        let _ = of(&with(&[("List-Unsubscribe", list.as_str()), ("List-Unsubscribe-Post", post.as_str())]));
        let _ = mailto(&address);
        let _ = addresses(&list);
    }
}
