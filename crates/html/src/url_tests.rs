// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A web address is a link only when a URL parser accepts it with a host (AU2-V1).

use super::is_safe_url;

#[test]
fn web_addresses_the_parser_rejects_are_no_links() {
    for bad in ["http://[::1", "https://exa mple.org/", "http://", "https://", "https://:443/x", "http://exa<mple.org/", "http://[zz]/"] {
        assert!(!is_safe_url(bad), "{bad:?}");
    }
    for ok in ["http://example.org/", "https://[::1]:8080/a", "HTTP://EXAMPLE.org/x?y#z", "https://bücher.example/"] {
        assert!(is_safe_url(ok), "{ok:?}");
    }
}

#[test]
fn a_format_character_anywhere_makes_an_address_no_link() {
    let format: Vec<char> = (0..=0x10ffff).filter_map(char::from_u32).filter(|c| mailcraft_names::is_format(*c)).collect();
    assert_eq!(format.len(), 170);
    for c in format {
        for address in [format!("https://example.org/a{c}b"), format!("mailto:ada{c}@example.org"), format!("cid:part{c}1")] {
            assert!(!is_safe_url(&address), "{address:?}");
        }
    }
}
