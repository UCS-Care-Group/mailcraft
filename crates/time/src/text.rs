// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Timestamps as text: RFC 5322 (`Date:` headers) and ISO 8601.

use crate::{Date, DateTime, MINUTE, Timestamp, Weekday, month_short, parse_month};

/// `Tue, 01 Jul 2003 10:52:37 +0200` for an instant, written in a zone `offset_minutes` east of UTC.
pub fn format_rfc5322(ts: Timestamp, offset_minutes: i32) -> String {
    let off = offset_minutes.clamp(-(23 * 60 + 59), 23 * 60 + 59);
    let t = DateTime::from_timestamp(ts, off);
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.unsigned_abs();
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} {sign}{:02}{:02}",
        t.date.weekday().short(),
        t.date.day,
        month_short(t.date.month),
        t.date.year,
        t.hour,
        t.minute,
        t.second,
        a / 60,
        a % 60
    )
}

/// Parse an RFC 5322 date, leniently (missing weekday or seconds, two-digit years, obsolete zone
/// names, comments). Returns the instant and the zone offset in minutes east of UTC.
pub fn parse_rfc5322(s: &str) -> Option<(Timestamp, i32)> {
    let s = strip_comments(s);
    // The weekday, when present, ends at the first comma.
    let s = match s.split_once(',') {
        Some((day, rest)) if Weekday::parse(day).is_some() || day.trim().is_empty() => rest,
        _ => s.as_str(),
    };
    let mut it = s.split_whitespace();
    let day: u8 = it.next()?.parse().ok()?;
    let month = parse_month(it.next()?)?;
    let year: i32 = match it.next()? {
        y if y.len() <= 2 => {
            // RFC 5322 §4.3: two-digit years 00–49 are 2000–2049, 50–99 are 1950–1999.
            let y: i32 = y.parse().ok()?;
            if y < 50 { 2000 + y } else { 1900 + y }
        }
        y if y.len() == 3 => 1900 + y.parse::<i32>().ok()?,
        y => y.parse().ok()?,
    };
    let mut clock = it.next()?.split(':');
    let hour: u8 = clock.next()?.parse().ok()?;
    let minute: u8 = clock.next()?.parse().ok()?;
    // A leap second (60) is read as the last second of the minute.
    let second: u8 = clock.next().map_or(Some(0), |x| x.parse().ok())?.min(59);
    let offset = it.next().map_or(0, zone_offset);
    let t = DateTime::new(Date::new(year, month, day)?, hour, minute, second)?;
    Some((t.to_timestamp(offset), offset))
}

/// Minutes east of UTC for `+0200`, `-0500`, `Z`, `GMT`, `EST`…; unknown names are UTC.
fn zone_offset(z: &str) -> i32 {
    let z = z.trim();
    if let Some(digits) = z.strip_prefix(['+', '-']) {
        let digits: String = digits.chars().filter(char::is_ascii_digit).collect();
        let (h, m) = match digits.len() {
            4 => (digits.get(0..2), digits.get(2..4)),
            2 => (digits.get(0..2), Some("0")),
            _ => (None, None),
        };
        let (Some(h), Some(m)) = (h.and_then(|h| h.parse::<i32>().ok()), m.and_then(|m| m.parse::<i32>().ok())) else { return 0 };
        if h > 23 || m > 59 {
            return 0;
        }
        let total = h * 60 + m;
        return if z.starts_with('-') { -total } else { total };
    }
    match z.to_ascii_uppercase().as_str() {
        "EDT" => -4 * 60,
        "EST" | "CDT" => -5 * 60,
        "CST" | "MDT" => -6 * 60,
        "MST" | "PDT" => -7 * 60,
        "PST" => -8 * 60,
        _ => 0,
    }
}

/// Remove `(comments)`, which may nest.
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' => depth = depth.saturating_add(1),
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// `2026-10-11T09:30:00Z`.
pub fn format_iso_utc(ts: Timestamp) -> String {
    format!("{}Z", DateTime::from_timestamp(ts, 0).iso())
}

/// Parse ISO 8601: `2026-10-11`, `2026-10-11T09:30`, `2026-10-11 09:30:00.250`, with `Z` or
/// `+02:00` at the end. Returns the wall clock and, when the text names one, its offset in minutes.
pub fn parse_iso(s: &str) -> Option<(DateTime, Option<i32>)> {
    let s = s.trim();
    let (date_part, rest) = match s.find(['T', 't', ' ']) {
        Some(i) => (s.get(..i)?, s.get(i + 1..)?),
        None => (s, ""),
    };
    let date = Date::parse_iso(date_part)?;
    if rest.is_empty() {
        return Some((DateTime::at_midnight(date), None));
    }
    let (clock, offset) = if let Some(c) = rest.strip_suffix(['Z', 'z']) {
        (c, Some(0))
    } else if let Some(i) = rest.rfind(['+', '-']) {
        let z = rest.get(i..)?;
        let digits: String = z.chars().filter(char::is_ascii_digit).collect();
        let h: i32 = digits.get(0..2)?.parse().ok()?;
        let m: i32 = digits.get(2..4).map_or(Some(0), |m| m.parse().ok())?;
        if h > 23 || m > 59 {
            return None;
        }
        (rest.get(..i)?, Some(if z.starts_with('-') { -(h * 60 + m) } else { h * 60 + m }))
    } else {
        (rest, None)
    };
    // Fractions of a second are dropped.
    let clock = clock.split('.').next()?;
    let mut parts = clock.split(':');
    let hour: u8 = parts.next()?.trim().parse().ok()?;
    let minute: u8 = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    let second: u8 = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((DateTime::new(date, hour, minute, second.min(59))?, offset))
}

/// The instant an ISO 8601 text names; a text without an offset is read in `default_offset`.
pub fn iso_to_timestamp(s: &str, default_offset: i32) -> Option<Timestamp> {
    let (t, off) = parse_iso(s)?;
    Some(t.civil_seconds() - i64::from(off.unwrap_or(default_offset)) * MINUTE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc5322_round_trip() {
        let (ts, off) = parse_rfc5322("Tue, 1 Jul 2003 10:52:37 +0200").unwrap();
        assert_eq!((ts, off), (1_057_049_557, 120));
        assert_eq!(format_rfc5322(ts, off), "Tue, 01 Jul 2003 10:52:37 +0200");
        assert_eq!(format_rfc5322(ts, 0), "Tue, 01 Jul 2003 08:52:37 +0000");
        assert_eq!(format_rfc5322(0, -330), "Wed, 31 Dec 1969 18:30:00 -0530");
    }

    #[test]
    fn rfc5322_lenient_forms() {
        let want = 1_057_049_557 - 37;
        assert_eq!(parse_rfc5322("1 Jul 2003 10:52 +0200").map(|x| x.0), Some(want));
        assert_eq!(parse_rfc5322("Tue, 1 Jul 03 08:52 GMT").map(|x| x.0), Some(want));
        assert_eq!(parse_rfc5322("Tue, 1 Jul 2003 04:52:00 EDT (Eastern (daylight))").map(|x| x.0), Some(want));
        assert_eq!(parse_rfc5322("Tue,  1 Jul 2003 08:52:00 -0000").map(|x| x.0), Some(want));
        assert_eq!(parse_rfc5322("1 Jul 2003 08:52:00").map(|x| x.0), Some(want));
        // A leap second.
        assert!(parse_rfc5322("31 Dec 2016 23:59:60 +0000").is_some());
    }

    #[test]
    fn rfc5322_junk_is_none() {
        for s in [
            "",
            "yesterday",
            "32 Jul 2003 10:52:37 +0200",
            "1 Foo 2003 10:52 +0200",
            "1 Jul 2003",
            "1 Jul 2003 25:00",
            "((((",
            "1 Jul 99999999999 10:00",
        ] {
            assert_eq!(parse_rfc5322(s), None, "{s}");
        }
        // A junk zone reads as UTC rather than failing the whole date.
        assert_eq!(parse_rfc5322("1 Jul 2003 08:52:00 +9999").map(|x| x.1), Some(0));
    }

    #[test]
    fn iso_forms() {
        let d = Date::new(2026, 10, 11).unwrap();
        assert_eq!(parse_iso("2026-10-11"), Some((DateTime::at_midnight(d), None)));
        assert_eq!(parse_iso("2026-10-11T09:30"), Some((DateTime::new(d, 9, 30, 0).unwrap(), None)));
        assert_eq!(parse_iso("2026-10-11 09:30:15.250Z"), Some((DateTime::new(d, 9, 30, 15).unwrap(), Some(0))));
        assert_eq!(parse_iso("2026-10-11T09:30:00-05:00"), Some((DateTime::new(d, 9, 30, 0).unwrap(), Some(-300))));
        assert_eq!(parse_iso("2026-10-11T09:30:00+0530").and_then(|x| x.1), Some(330));
        assert_eq!(parse_iso("2026-13-11T09:30"), None);
        assert_eq!(parse_iso("2026-10-11T24:30"), None);
        assert_eq!(parse_iso("nope"), None);
        assert_eq!(format_iso_utc(1_760_082_785), "2025-10-10T07:53:05Z");
        assert_eq!(iso_to_timestamp("2025-10-10T07:53:05Z", 600), Some(1_760_082_785));
        assert_eq!(iso_to_timestamp("2025-10-10T09:53:05", 120), Some(1_760_082_785));
    }
}
