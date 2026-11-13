// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message started from outside: hostile `mailto:` links and text shared by another app. Invented
//! people and `example.org` addresses only; nothing leaves the process.

use std::sync::Arc;

use mailcraft_model::Mailbox;

use crate::fake_tests::{FakePlatform, block_on, now};
use crate::{ComposeStart, ComposeView, Core};

fn core() -> Arc<Core> {
    Core::start(FakePlatform::new(), || {
        let mb = Mailbox::with_local_account("Avery", "avery@example.org");
        mailcraft_engine::Session::at(mb, now(), 0)
    })
    .unwrap()
}

fn mailto(core: &Core, link: &str) -> Option<ComposeView> {
    block_on(core.compose_start(ComposeStart::New { account: 0, mailto: Some(link.to_string()) })).ok()
}

fn shared(core: &Core, subject: Option<&str>, text: &str) -> ComposeView {
    block_on(core.compose_start(ComposeStart::Shared { account: 0, subject: subject.map(str::to_string), text: text.to_string() })).unwrap()
}

#[test]
fn a_mailto_link_cannot_carry_a_line_break_into_a_header() {
    let core = core();
    let v = mailto(&core, "mailto:ada@example.org%0D%0ABcc:evil@example.org?subject=Hi%0D%0ABcc:evil@example.org&cc=bo@example.org%0AX:y").unwrap();
    assert!(v.subject.chars().all(|c| !c.is_control()), "{:?}", v.subject);
    for p in v.to.iter().chain(&v.cc).chain(&v.bcc) {
        assert!(p.address.chars().all(|c| !c.is_control()), "{:?}", p.address);
    }
    assert!(v.bcc.is_empty());
}

#[test]
fn a_mailto_link_with_a_crowd_of_addresses_is_capped() {
    let core = core();
    // Under the link's own size cap, with far more addresses than a message may have.
    let crowd = (0..2_000).map(|i| format!("p{i}@example.org")).collect::<Vec<_>>().join(",");
    let link = format!("mailto:{}", crowd.get(..7_000).unwrap_or(&crowd));
    let v = mailto(&core, &link).unwrap();
    assert!(v.to.len() <= 200, "{}", v.to.len());
}

#[test]
fn a_mailto_link_with_a_long_subject_and_odd_escapes_stays_in_bounds() {
    let core = core();
    let long = "x".repeat(7_000);
    let v = mailto(&core, &format!("mailto:ada@example.org?subject={long}")).unwrap();
    assert!(v.subject.len() <= 998);
    for link in
        ["mailto:?", "mailto:%", "mailto:%zz%0", "mailto:a@example.org?&&&=&=", "MAILTO:%F0%28%8C%28?body=%FF%FE", "mailto:\u{202e}@x?subject=\u{0}"]
    {
        // Refused or opened, never a panic, and only ever a message in the making.
        if let Some(v) = mailto(&core, link) {
            assert!(v.files.is_empty(), "{link}");
            let _ = block_on(core.compose_close(v.compose, true));
        }
    }
}

#[test]
fn a_mailto_link_names_no_files_and_no_other_headers() {
    let core = core();
    let v = mailto(&core, "mailto:ada@example.org?attach=/etc/passwd&from=evil@example.org&reply-to=evil@example.org&in-reply-to=%3Cx%3E&x-evil=1")
        .unwrap();
    assert!(v.files.is_empty());
    assert!(v.bcc.is_empty());
    assert_eq!(v.to.len(), 1);
}

#[test]
fn shared_text_becomes_the_body_and_its_addresses_become_links() {
    let core = core();
    let v = shared(&core, Some("A page"), "Worth a read\nhttps://example.org/page?a=1, and http://example.org/b.");
    assert_eq!(v.subject, "A page");
    assert!(v.text.starts_with("Worth a read\nhttps://example.org/page?a=1, and http://example.org/b."), "{:?}", v.text);
    let links: Vec<_> = v.spans.iter().filter_map(|s| s.link.as_deref()).collect();
    assert_eq!(links, vec!["https://example.org/page?a=1", "http://example.org/b"], "{:?}", v.spans);
    assert!(v.to.is_empty() && v.files.is_empty());
}

#[test]
fn shared_text_after_an_emoji_links_the_right_range() {
    let core = core();
    let v = shared(&core, None, "\u{1F600} https://example.org/x");
    let span = v.spans.iter().find(|s| s.link.is_some()).unwrap();
    // The emoji is two units and the space one: the address starts at unit 3.
    assert_eq!((span.start, span.end), (3, 3 + "https://example.org/x".len() as u32), "{:?}", v.spans);
}

#[test]
fn shared_text_is_cleaned_and_capped() {
    let core = core();
    let v = shared(&core, Some("Line one\r\nBcc: evil@example.org"), "a\0b\u{202e}c\r\nd");
    assert!(v.subject.chars().all(|c| !c.is_control()), "{:?}", v.subject);
    assert!(v.bcc.is_empty());
    assert!(v.text.starts_with("abc\nd"), "{:?}", v.text);
    let huge = "word ".repeat(100_000);
    let v = shared(&core, Some(&"s".repeat(10_000)), &huge);
    assert!(v.text.chars().count() <= 100_100, "{}", v.text.chars().count());
    assert!(v.subject.chars().count() <= 500);
}

#[test]
fn shared_text_with_too_many_or_odd_addresses_links_only_web_ones() {
    let core = core();
    let many = (0..100).map(|i| format!("https://example.org/{i}")).collect::<Vec<_>>().join(" ");
    let v = shared(&core, None, &format!("{many} javascript:alert(1) file:///etc/passwd https:// http://"));
    let links: Vec<_> = v.spans.iter().filter_map(|s| s.link.as_deref()).collect();
    assert!(links.len() <= 20, "{}", links.len());
    assert!(links.iter().all(|l| l.starts_with("https://example.org/")));
    let v = shared(&core, None, "");
    assert!(v.spans.iter().all(|s| s.link.is_none()));
}

#[test]
fn shared_text_and_subject_carry_no_direction_overrides() {
    let core = core();
    let v = shared(&core, Some("Re\u{202e}port\u{2066}1\u{2069}"), "a\u{202a}b\u{202d}c\u{2067}d\u{202c}e");
    assert_eq!(v.subject, "Report1");
    assert!(v.text.starts_with("abcde"), "{:?}", v.text);
    for c in v.subject.chars().chain(v.text.chars()) {
        assert!(!matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'), "{c:?}");
    }
}
