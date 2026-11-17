// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::page::*;

const MAX_LINK: usize = 8192;

#[test]
fn only_web_mail_and_phone_links_are_opened() {
    for (ok, opened) in [
        ("https://example.org/a?b=1", "https://example.org/a?b=1"),
        ("http://example.org", "http://example.org/"),
        ("mailto:ada@example.org", "mailto:ada@example.org"),
        ("tel:+15550100", "tel:+15550100"),
    ] {
        assert_eq!(web_link(ok).as_deref(), Some(opened));
    }
    for bad in [
        "javascript:alert(1)",
        "data:text/html,x",
        "file:///etc/passwd",
        "cid:logo@example.org",
        "https://",
        "http://[::1",
        "",
        "//example.org",
        "blob:https://x/y",
    ] {
        assert_eq!(web_link(bad), None, "{bad:?}");
    }
    assert_eq!(web_link(&format!("https://example.org/{}", "a".repeat(MAX_LINK))), None);
}

#[test]
fn a_web_link_opens_the_host_its_tip_shows() {
    // A full-width letter is mapped by the browser (ｅxample.org is example.org), never by the tip,
    // so the page opens the address as mailcraft-link writes it, the host the tip shows.
    for href in ["https://\u{ff45}xample.org/", "https://\u{2488}example.org/", "HTTPS://Example.ORG/a"] {
        let shown = mailcraft_link::WebLink::parse(href).map(mailcraft_link::WebLink::into_string);
        assert_eq!(web_link(href), shown, "{href:?}");
    }
    assert!(!web_link("https://\u{ff45}xample.org/").is_some_and(|u| u.contains("example.org")));
}

#[test]
fn a_sample_or_empty_start_is_a_look_around() {
    // AU2-V10: such a start never writes the mailbox kept in the browser.
    for q in ["?sample", "?empty", "?webgl&sample", "?empty&webgl", "?sample=1"] {
        assert!(is_ephemeral(q), "{q}");
    }
    for q in ["", "?", "?webgl", "?samples", "?x=sample", "?resample"] {
        assert!(!is_ephemeral(q), "{q}");
    }
}

/// Georgian is the one language whose face the browser build fetches; every other code, a
/// region-tagged `ka-GE` and the empty code included, fetches nothing.
#[test]
fn only_georgian_has_a_face_to_fetch() {
    assert_eq!(face_for("ka"), Some("NotoSansGeorgian-Regular.ttf"));
    for code in ["en", "de", "nl", "pseudo", "ka-GE", ""] {
        assert_eq!(face_for(code), None);
    }
}

/// A fetched face is capped at 1 MB, to the byte.
#[test]
fn a_fetched_face_is_capped_at_one_megabyte() {
    assert!(face_size_ok(35_300));
    assert!(face_size_ok(1_000_000));
    assert!(!face_size_ok(1_000_001));
}
