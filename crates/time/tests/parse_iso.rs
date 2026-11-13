// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `Date::parse_iso` takes only `YYYYMMDD` and `YYYY-MM-DD`.

use mailcraft_time::Date;

#[test]
fn accepts_only_the_two_layouts() {
    let want = Date::new(2026, 10, 12);
    assert_eq!(Date::parse_iso("20261012"), want);
    assert_eq!(Date::parse_iso(" 2026-10-12 "), want);
    for bad in ["2026-1-012", "20-26-10-12", "2026-10-1-2", "2026-1012", "202610-12", "2026-10-12-", "-20261012", "2026--10-12", ""] {
        assert_eq!(Date::parse_iso(bad), None, "{bad}");
    }
}
