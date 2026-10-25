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
