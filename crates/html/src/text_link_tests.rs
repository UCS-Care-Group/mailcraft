// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plain text is linked only where the safety check accepts the address.

use super::{is_safe_url, text_blocks};

#[test]
fn an_address_the_safety_check_refuses_is_no_link() {
    // Found by the proptest: `www.a@.` made a link `is_safe_url` refuses.
    for text in ["www.a@.", "see www.a@. now", "http://", "www.", "a@."] {
        for block in text_blocks(text) {
            for span in &block.spans {
                assert!(span.link.as_deref().is_none_or(is_safe_url), "{text:?}: {:?}", span.link);
            }
        }
    }
}

#[test]
fn good_addresses_stay_links() {
    let blocks = text_blocks("go to www.example.org or ada@example.org");
    let links: Vec<_> = blocks.iter().flat_map(|b| &b.spans).filter_map(|s| s.link.as_deref()).collect();
    assert_eq!(links, ["https://www.example.org", "mailto:ada@example.org"]);
}
