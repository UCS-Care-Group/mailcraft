// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Series with a count are not walked from their first day: the answer is the same, found at once.

use std::time::{Duration, Instant};

use mailcraft_time::{Date, Weekday};

use crate::{Freq, Recurrence};

fn d(y: i32, m: u8, day: u8) -> Date {
    Date::new(y, m, day).unwrap()
}

fn rules() -> Vec<Recurrence> {
    let mut out = Vec::new();
    for freq in [Freq::Daily, Freq::Weekly, Freq::Monthly, Freq::Yearly] {
        for interval in [1, 2, 3] {
            for (by_day, nth) in [
                (vec![], None),
                (vec![Weekday::Mon, Weekday::Fri], None),
                (vec![Weekday::Wed], Some(2)),
                (vec![Weekday::Wed, Weekday::Sun, Weekday::Wed], Some(-1)),
                (vec![Weekday::Fri], Some(5)),
            ] {
                out.push(Recurrence { freq, interval, by_day, nth, count: Some(150), until: None });
            }
        }
    }
    out
}

#[test]
fn a_count_series_gives_the_same_days_as_walking_from_the_start() {
    let start = d(2020, 2, 29);
    for rule in rules() {
        let all = rule.dates(start, start, d(2200, 1, 1), &[]);
        assert!(all.len() <= 150);
        for from in [d(2020, 3, 1), d(2021, 6, 15), d(2024, 2, 29), d(2030, 1, 1), d(2035, 7, 7), d(2100, 1, 1), d(2150, 1, 1)] {
            let to = from.add_days(400);
            let expected: Vec<Date> = all.iter().copied().filter(|day| *day >= from && *day <= to).collect();
            assert_eq!(rule.dates(start, from, to, &[]), expected, "{rule:?} from {from:?}");
        }
    }
}

#[test]
fn a_huge_count_far_in_the_past_costs_almost_nothing() {
    let rule = Recurrence { freq: Freq::Daily, interval: 1, count: Some(4_000_000_000), ..Default::default() };
    let started = Instant::now();
    for _ in 0..2_000 {
        assert!(!rule.dates(d(1970, 1, 1), d(2026, 10, 1), d(2026, 10, 31), &[]).is_empty());
    }
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
}

#[test]
fn many_excluded_days_are_cheap() {
    let rule = Recurrence { freq: Freq::Daily, interval: 1, ..Default::default() };
    let excluded: Vec<Date> = (0..5_000).map(|i| d(2000, 1, 1).add_days(i)).collect();
    let started = Instant::now();
    for _ in 0..200 {
        rule.dates(d(2000, 1, 1), d(2000, 1, 1), d(2020, 1, 1), &excluded);
    }
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}
