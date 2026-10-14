//! How dates, times and numbers are written where the interface's language is spoken, and which
//! language the system asks for.
//!
//! Everything the interface writes of a date, a time, a length of time or a number with a
//! fraction goes through here (the date patterns of [`Formats`]), so the order of day and month,
//! the separators, the clock and the first day of the week follow the language and the region
//! the system names (`en-GB` writes `11/10/2026` and `en-US` `10/11/2026`). The 24-hour clock and
//! the first day of the week are only defaults: File › Options changes either.

use mailcraft_time::{Date, Weekday};

/// A locale as the system or a setting names it, reduced to a language and a region:
/// `pt_BR.UTF-8` is `pt` and `BR`, `de-AT` is `de` and `AT`, `C` is nothing at all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tag {
    /// Lower case, two or three letters.
    pub language: String,
    /// Upper case, two letters or three digits; empty when none is named.
    pub region: String,
}

impl Tag {
    /// Read a locale name (BCP 47 `pt-BR`, POSIX `pt_BR.UTF-8@euro`, Windows `pt-BR`). Anything
    /// that isn't one is the empty tag.
    pub fn parse(text: &str) -> Tag {
        let text = text.trim();
        // POSIX: language[_territory][.codeset][@modifier].
        let text = text.split(['.', '@']).next().unwrap_or("");
        let mut parts = text.split(['-', '_']).filter(|p| !p.is_empty());
        let language = parts.next().unwrap_or("");
        let language_ok = (2..=3).contains(&language.len()) && language.chars().all(|c| c.is_ascii_alphabetic());
        if !language_ok {
            return Tag::default();
        }
        // The region is the first later part of two letters or three digits (a script such as
        // `Latn` comes before it).
        let region = parts
            .find(|p| (p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic())) || (p.len() == 3 && p.chars().all(|c| c.is_ascii_digit())))
            .unwrap_or("");
        Tag { language: language.to_ascii_lowercase(), region: region.to_ascii_uppercase() }
    }

    /// `pt-BR`, `de`, or empty.
    pub fn code(&self) -> String {
        match (self.language.is_empty(), self.region.is_empty()) {
            (true, _) => String::new(),
            (false, true) => self.language.clone(),
            (false, false) => format!("{}-{}", self.language, self.region),
        }
    }
}

/// The locale the system asks for, read once: from the operating system's settings on Windows
/// and macOS, from `LC_ALL`, `LC_MESSAGES` and `LANG` on Linux and the BSDs, and from the
/// browser's `navigator.language` on the web.
pub fn system() -> Tag {
    use std::sync::OnceLock;
    static SYSTEM: OnceLock<Tag> = OnceLock::new();
    SYSTEM
        .get_or_init(|| {
            // `MAILCRAFT_LOCALE` names one instead (and the tests are in American English unless
            // it does, whatever the machine running them speaks).
            if let Some(named) = std::env::var("MAILCRAFT_LOCALE").ok().filter(|v| !v.trim().is_empty()) {
                return Tag::parse(&named);
            }
            if cfg!(test) {
                return Tag { language: "en".into(), region: "US".into() };
            }
            sys_locale::get_locale().map(|name| Tag::parse(&name)).unwrap_or_default()
        })
        .clone()
}

/// Which comes first in a date written with numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// `10/11/2026`.
    MonthDayYear,
    /// `11/10/2026`, `11.10.2026`.
    DayMonthYear,
    /// `2026-10-11`.
    YearMonthDay,
}

/// How one locale writes dates, times and numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Formats {
    pub order: Order,
    /// Between the numbers of a date.
    pub date_sep: char,
    /// The clock people there read by default.
    pub twelve_hour: bool,
    /// The first day of the week there.
    pub week_start: Weekday,
    /// Before the fraction of a number.
    pub decimal: char,
    /// Between the thousands of a number.
    pub group: char,
    /// The long date: `{weekday}`, `{day}`, `{month}`, `{year}`.
    pub long: &'static str,
    /// The medium date, with the month's short name in `{month}`.
    pub medium: &'static str,
    /// A day of this year, with the month's short name in `{month}`.
    pub day_month: &'static str,
    /// `{month} {year}`, with the month's whole name.
    pub month_year: &'static str,
    /// A date with the month's whole name and no weekday (`October 11, 2026`).
    pub full: &'static str,
    /// A day of the year with the month's whole name (`October 11`).
    pub day_month_full: &'static str,
}

impl Formats {
    /// What the United States write (and what MailCraft wrote before it had any other).
    pub const US: Formats = Formats {
        order: Order::MonthDayYear,
        date_sep: '/',
        twelve_hour: true,
        week_start: Weekday::Sun,
        decimal: '.',
        group: ',',
        long: "{weekday}, {month} {day}, {year}",
        medium: "{month} {day}, {year}",
        day_month: "{month} {day}",
        month_year: "{month} {year}",
        full: "{month} {day}, {year}",
        day_month_full: "{month} {day}",
    };

    /// English outside North America: the day first, the 24-hour clock, Monday.
    const ENGLISH_WORLD: Formats = Formats {
        order: Order::DayMonthYear,
        twelve_hour: false,
        week_start: Weekday::Mon,
        long: "{weekday}, {day} {month} {year}",
        medium: "{day} {month} {year}",
        day_month: "{day} {month}",
        full: "{day} {month} {year}",
        day_month_full: "{day} {month}",
        ..Formats::US
    };

    const GERMAN: Formats = Formats {
        order: Order::DayMonthYear,
        date_sep: '.',
        twelve_hour: false,
        week_start: Weekday::Mon,
        decimal: ',',
        group: '.',
        long: "{weekday}, {day}. {month} {year}",
        medium: "{day}. {month} {year}",
        day_month: "{day}. {month}",
        month_year: "{month} {year}",
        full: "{day}. {month} {year}",
        day_month_full: "{day}. {month}",
    };

    const FRENCH: Formats = Formats {
        order: Order::DayMonthYear,
        date_sep: '/',
        twelve_hour: false,
        week_start: Weekday::Mon,
        decimal: ',',
        group: '\u{A0}',
        long: "{weekday} {day} {month} {year}",
        medium: "{day} {month} {year}",
        day_month: "{day} {month}",
        month_year: "{month} {year}",
        full: "{day} {month} {year}",
        day_month_full: "{day} {month}",
    };

    const SPANISH: Formats = Formats {
        order: Order::DayMonthYear,
        date_sep: '/',
        twelve_hour: false,
        week_start: Weekday::Mon,
        decimal: ',',
        group: '.',
        long: "{weekday}, {day} de {month} de {year}",
        medium: "{day} {month} {year}",
        day_month: "{day} {month}",
        month_year: "{month} de {year}",
        full: "{day} de {month} de {year}",
        day_month_full: "{day} de {month}",
    };

    const PORTUGUESE: Formats = Formats {
        order: Order::DayMonthYear,
        date_sep: '/',
        twelve_hour: false,
        week_start: Weekday::Sun,
        decimal: ',',
        group: '.',
        long: "{weekday}, {day} de {month} de {year}",
        medium: "{day} de {month} de {year}",
        day_month: "{day} de {month}",
        month_year: "{month} de {year}",
        full: "{day} de {month} de {year}",
        day_month_full: "{day} de {month}",
    };

    const ARABIC: Formats = Formats {
        order: Order::DayMonthYear,
        date_sep: '/',
        twelve_hour: true,
        week_start: Weekday::Sat,
        decimal: '.',
        group: ',',
        long: "{weekday}، {day} {month} {year}",
        medium: "{day} {month} {year}",
        day_month: "{day} {month}",
        month_year: "{month} {year}",
        full: "{day} {month} {year}",
        day_month_full: "{day} {month}",
    };

    const HEBREW: Formats = Formats {
        order: Order::DayMonthYear,
        date_sep: '.',
        twelve_hour: false,
        week_start: Weekday::Sun,
        decimal: '.',
        group: ',',
        long: "{weekday}, {day} ב{month} {year}",
        medium: "{day} ב{month} {year}",
        day_month: "{day} ב{month}",
        month_year: "{month} {year}",
        full: "{day} ב{month} {year}",
        day_month_full: "{day} ב{month}",
    };

    /// How `tag` writes them; a language this table doesn't know writes them as the United
    /// States do.
    pub fn of(tag: &Tag) -> Formats {
        let region = tag.region.as_str();
        match tag.language.as_str() {
            "en" => match region {
                "" | "US" | "PH" | "PR" | "UM" | "VI" | "AS" | "GU" => Formats::US,
                "CA" => Formats { order: Order::YearMonthDay, date_sep: '-', ..Formats::US },
                "AU" | "NZ" | "IN" => {
                    Formats { twelve_hour: true, week_start: if region == "IN" { Weekday::Sun } else { Weekday::Mon }, ..Formats::ENGLISH_WORLD }
                }
                _ => Formats::ENGLISH_WORLD,
            },
            "de" => Formats::GERMAN,
            "fr" => match region {
                "CA" => Formats { order: Order::YearMonthDay, date_sep: '-', week_start: Weekday::Sun, ..Formats::FRENCH },
                "CH" => Formats { date_sep: '.', group: '\'', decimal: '.', ..Formats::FRENCH },
                _ => Formats::FRENCH,
            },
            "es" => match region {
                "US" => {
                    Formats { order: Order::MonthDayYear, twelve_hour: true, week_start: Weekday::Sun, decimal: '.', group: ',', ..Formats::SPANISH }
                }
                "MX" => Formats { twelve_hour: true, week_start: Weekday::Sun, decimal: '.', group: ',', ..Formats::SPANISH },
                _ => Formats::SPANISH,
            },
            "pt" => match region {
                "" | "BR" => Formats::PORTUGUESE,
                _ => Formats { week_start: Weekday::Mon, group: '\u{A0}', ..Formats::PORTUGUESE },
            },
            "ar" => match region {
                "MA" | "DZ" | "TN" | "LB" => Formats { twelve_hour: false, week_start: Weekday::Mon, ..Formats::ARABIC },
                _ => Formats::ARABIC,
            },
            "he" | "iw" => Formats::HEBREW,
            _ => Formats::US,
        }
    }
}

/// `pattern` with `{name}` filled from `args`, in one pass (a value that contains braces stays as
/// it is).
pub(crate) fn fill(pattern: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(rest.get(..open).unwrap_or(""));
        let after = rest.get(open + 1..).unwrap_or("");
        let found = after.find('}').and_then(|close| {
            let name = after.get(..close)?;
            let value = args.iter().find(|(k, _)| *k == name)?.1;
            Some((value, close))
        });
        match found {
            Some((value, close)) => {
                out.push_str(value);
                rest = after.get(close + 1..).unwrap_or("");
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

impl Formats {
    /// `10/11/2026`, `11.10.2026`, `2026-10-11`.
    pub fn numeric(&self, d: Date) -> String {
        let s = self.date_sep;
        match self.order {
            Order::MonthDayYear => format!("{}{s}{}{s}{}", d.month, d.day, d.year),
            Order::DayMonthYear => format!("{:02}{s}{:02}{s}{}", d.day, d.month, d.year),
            Order::YearMonthDay => format!("{:04}{s}{:02}{s}{:02}", d.year, d.month, d.day),
        }
    }

    /// A day and month with numbers, no year: `10/11`, `11.10.`, `10-11`.
    pub fn numeric_day_month(&self, d: Date) -> String {
        let s = self.date_sep;
        match self.order {
            Order::MonthDayYear => format!("{}{s}{}", d.month, d.day),
            Order::DayMonthYear if s == '.' => format!("{}.{}.", d.day, d.month),
            Order::DayMonthYear => format!("{}{s}{}", d.day, d.month),
            Order::YearMonthDay => format!("{:02}{s}{:02}", d.month, d.day),
        }
    }

    /// The long date, with the names given (in the interface language).
    pub fn long_with(&self, d: Date, weekday: &str, month: &str) -> String {
        fill(self.long, &[("weekday", weekday), ("day", &d.day.to_string()), ("month", month), ("year", &d.year.to_string())])
    }

    /// The medium date, with the month's short name given.
    pub fn medium_with(&self, d: Date, month_short: &str) -> String {
        fill(self.medium, &[("day", &d.day.to_string()), ("month", month_short), ("year", &d.year.to_string())])
    }

    /// A day of the year without the year, with the month's short name given.
    pub fn day_month_with(&self, d: Date, month_short: &str) -> String {
        fill(self.day_month, &[("day", &d.day.to_string()), ("month", month_short)])
    }

    /// A date without its weekday, with the month's whole name given.
    pub fn full_with(&self, d: Date, month: &str) -> String {
        fill(self.full, &[("day", &d.day.to_string()), ("month", month), ("year", &d.year.to_string())])
    }

    /// A day of the year, with the month's whole name given.
    pub fn day_month_full_with(&self, d: Date, month: &str) -> String {
        fill(self.day_month_full, &[("day", &d.day.to_string()), ("month", month)])
    }

    /// The month and year, with the month's name given.
    pub fn month_year_with(&self, year: i32, month: &str) -> String {
        fill(self.month_year, &[("month", month), ("year", &year.to_string())])
    }

    /// A number with `decimals` digits after the separator (at most 6), its thousands grouped.
    /// What isn't a finite number is written as `0`.
    pub fn number(&self, value: f64, decimals: usize) -> String {
        let value = if value.is_finite() { value } else { 0.0 };
        let decimals = decimals.min(6);
        let text = format!("{:.*}", decimals, value.abs());
        let (whole, fraction) = text.split_once('.').unwrap_or((text.as_str(), ""));
        let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
        let digits: Vec<char> = whole.chars().collect();
        for (i, c) in digits.iter().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                grouped.push(self.group);
            }
            grouped.push(*c);
        }
        let negative = value < 0.0 && text.chars().any(|c| c.is_ascii_digit() && c != '0');
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        out.push_str(&grouped);
        if !fraction.is_empty() {
            out.push(self.decimal);
            out.push_str(fraction);
        }
        out
    }

    /// A whole number, its thousands grouped.
    pub fn integer(&self, value: i64) -> String {
        // An i64 is exact in this conversion up to 2^53, far beyond any count shown.
        #[allow(clippy::cast_precision_loss)]
        self.number(value as f64, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    #[test]
    fn tags_from_every_kind_of_name() {
        assert_eq!(Tag::parse("pt_BR.UTF-8"), Tag { language: "pt".into(), region: "BR".into() });
        assert_eq!(Tag::parse("de-AT"), Tag { language: "de".into(), region: "AT".into() });
        assert_eq!(Tag::parse("sr-Latn-RS").region, "RS");
        assert_eq!(Tag::parse("es-419").region, "419");
        assert_eq!(Tag::parse("fr_FR@euro").code(), "fr-FR");
        assert_eq!(Tag::parse("EN"), Tag { language: "en".into(), region: String::new() });
        for junk in ["", "C", "POSIX", "C.UTF-8", "1234", "-", "_", "@", "ŝ-é", "toolong-US"] {
            assert_eq!(Tag::parse(junk), Tag::default(), "{junk}");
        }
    }

    #[test]
    fn dates_per_locale() {
        let day = d(2026, 10, 11);
        let f = |code: &str| Formats::of(&Tag::parse(code));
        assert_eq!(f("en-US").numeric(day), "10/11/2026");
        assert_eq!(f("en-GB").numeric(day), "11/10/2026");
        assert_eq!(f("de-DE").numeric(day), "11.10.2026");
        assert_eq!(f("fr-CA").numeric(day), "2026-10-11");
        assert_eq!(f("pt-BR").numeric(d(2026, 3, 4)), "04/03/2026");
        assert_eq!(f("de").numeric_day_month(day), "11.10.");
        assert_eq!(f("en-US").long_with(day, "Sunday", "October"), "Sunday, October 11, 2026");
        assert_eq!(f("de").long_with(day, "Sonntag", "Oktober"), "Sonntag, 11. Oktober 2026");
        assert_eq!(f("es").long_with(day, "domingo", "octubre"), "domingo, 11 de octubre de 2026");
        assert_eq!(f("fr").medium_with(day, "oct."), "11 oct. 2026");
        assert_eq!(f("pt-BR").day_month_with(day, "out."), "11 de out.");
        assert_eq!(f("es").month_year_with(2026, "octubre"), "octubre de 2026");
        // A value with braces in it is not read as a pattern.
        assert_eq!(fill("{month} {day}", &[("month", "{day}"), ("day", "1")]), "{day} 1");
        assert_eq!(fill("{nope} {", &[]), "{nope} {");
    }

    #[test]
    fn clocks_weeks_and_numbers() {
        let f = |code: &str| Formats::of(&Tag::parse(code));
        assert!(f("en-US").twelve_hour && !f("de").twelve_hour && !f("en-GB").twelve_hour && f("es-MX").twelve_hour);
        assert_eq!(f("en-US").week_start, Weekday::Sun);
        assert_eq!(f("fr").week_start, Weekday::Mon);
        assert_eq!(f("pt-BR").week_start, Weekday::Sun);
        assert_eq!(f("ar-EG").week_start, Weekday::Sat);
        assert_eq!(f("en-US").number(1234567.891, 2), "1,234,567.89");
        assert_eq!(f("de").number(1234567.891, 2), "1.234.567,89");
        assert_eq!(f("fr").number(1234.5, 1), "1\u{A0}234,5");
        assert_eq!(f("de").number(-0.004, 2), "0,00");
        assert_eq!(f("de").number(-12.5, 1), "-12,5");
        assert_eq!(f("en").number(f64::NAN, 2), "0.00");
        assert_eq!(f("en").number(1.0e300, 0).len(), 301 + 100);
        assert_eq!(f("de").integer(1000), "1.000");
        assert_eq!(f("xx").numeric(d(2026, 1, 2)), "1/2/2026");
    }
}
