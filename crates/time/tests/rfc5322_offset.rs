// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A written `Date:` header reads back as the same instant, whatever offset the caller passes.

use mailcraft_time::{format_rfc5322, parse_rfc5322};

#[test]
fn extreme_offsets_round_trip() {
    for off in [1440, 100_000, -1440, -100_000, i32::MAX, i32::MIN] {
        let text = format_rfc5322(0, off);
        assert_eq!(parse_rfc5322(&text).map(|x| x.0), Some(0), "{text}");
    }
}
