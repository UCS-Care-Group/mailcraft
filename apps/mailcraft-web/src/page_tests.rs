// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::page::*;

const MAX_LINK: usize = 8192;

#[test]
fn only_web_mail_and_phone_links_are_opened() {
    for ok in ["https://example.org/a?b=1", "http://example.org", "mailto:ada@example.org", "tel:+15550100"] {
        assert_eq!(web_link(ok).as_deref(), Some(ok));
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
fn a_sample_or_empty_start_is_a_look_around() {
    // AU2-V10: such a start never writes the mailbox kept in the browser.
    for q in ["?sample", "?empty", "?webgl&sample", "?empty&webgl", "?sample=1"] {
        assert!(is_ephemeral(q), "{q}");
    }
    for q in ["", "?", "?webgl", "?samples", "?x=sample", "?resample"] {
        assert!(!is_ephemeral(q), "{q}");
    }
}
