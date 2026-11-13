// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Older mail by month and year (#929): what Outlook calls Older is split into the months of this
//! year before last month and then the years before.

use super::*;

fn d(y: i32, m: u8, day: u8) -> Date {
    Date::new(y, m, day).unwrap()
}

#[test]
fn older_mail_goes_by_month_this_year_and_by_year_before() {
    let today = d(2026, 11, 12);
    let by_month = |day| DateGroup::of_by_month(day, today, Weekday::Mon);
    // What isn't Older is as it was.
    for day in [today, d(2026, 11, 11), d(2026, 11, 2), d(2026, 10, 3)] {
        assert_eq!(by_month(day), DateGroup::of(day, today, Weekday::Mon), "{day:?}");
    }
    assert_eq!(by_month(d(2026, 9, 30)), DateGroup::Month(9));
    assert_eq!(by_month(d(2026, 1, 1)), DateGroup::Month(1));
    assert_eq!(by_month(d(2025, 12, 31)), DateGroup::Year(2025));
    assert_eq!(by_month(d(1999, 6, 1)), DateGroup::Year(1999));
    // In January, last month is last year's December, and what is older is a year.
    let january = d(2027, 1, 20);
    assert_eq!(DateGroup::of_by_month(d(2026, 12, 2), january, Weekday::Mon), DateGroup::LastMonth);
    assert_eq!(DateGroup::of_by_month(d(2026, 11, 2), january, Weekday::Mon), DateGroup::Year(2026));
}

#[test]
fn the_headings_name_the_month_and_the_year() {
    assert_eq!(DateGroup::Month(9).heading(), "September");
    assert_eq!(DateGroup::Year(2025).heading(), "2025");
    assert_eq!(DateGroup::Older.heading(), "Older");
    assert_eq!(DateGroup::LastMonth.heading(), "Last Month");
    // A month that isn't one has no name, and nothing panics for it.
    assert_eq!(DateGroup::Month(0).heading(), "");
    assert_eq!(DateGroup::Month(13).heading(), "");
    // Newest first, as the list runs.
    assert!(DateGroup::LastMonth < DateGroup::Month(9) && DateGroup::Month(9) < DateGroup::Year(2025));
    // Newest first within the months and the years too, and the weekdays as before.
    assert!(DateGroup::Month(9) < DateGroup::Month(1) && DateGroup::Year(2025) < DateGroup::Year(2024));
    assert!(DateGroup::ThisWeek(Weekday::Mon) < DateGroup::ThisWeek(Weekday::Tue));
    let mut groups = vec![DateGroup::Year(2024), DateGroup::Month(2), DateGroup::Today, DateGroup::Year(2025), DateGroup::Month(8)];
    groups.sort();
    assert_eq!(groups, [DateGroup::Today, DateGroup::Month(8), DateGroup::Month(2), DateGroup::Year(2025), DateGroup::Year(2024)]);
}
