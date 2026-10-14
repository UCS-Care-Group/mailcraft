//! Time zones: the rules of every zone of the IANA Time Zone Database (with their history, so a
//! date in 1995 gets the daylight-saving rule of 1995), the names Windows and Microsoft's services
//! call zones by, and this computer's own zone.
//!
//! The database is compiled in (the `jiff` crate with its bundled copy), so a zone means the same
//! on every platform and in the browser, and nothing here reads a file or asks a server. No other
//! crate of the workspace depends on `jiff`: [`Zone`] is the whole interface.
//!
//! Everything from outside is hostile: a name is looked at only when it is short, an unknown
//! name is `None` (never a guess), and no number of any size makes a conversion panic.

mod windows;

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

use crate::{DAY, DateTime, HOUR, MINUTE, Timestamp};

/// Longest name of a zone that is looked up, in bytes.
pub const MAX_ZONE_NAME: usize = 128;
/// The furthest from UTC a fixed offset may be (minutes): more is not an offset anywhere.
const MAX_OFFSET: i32 = 18 * 60;
/// The instants the database is asked about: years 1 to 9999, less the day at the very end that
/// it doesn't reach.
const FIRST_INSTANT: Timestamp = -62_135_596_800;
const LAST_INSTANT: Timestamp = 253_402_207_200;
/// Most transitions [`Zone::transitions`] returns.
const MAX_TRANSITIONS: usize = 600;
/// For how many seconds [`Zone::system`] goes by what the system said last.
const SYSTEM_KEPT: i64 = 5;
/// What [`Zone::system`] was told last, and when: 0 for nothing yet, 1 for no name, or a zone's
/// place in the list of names and 2.
static SYSTEM_SAID: AtomicU32 = AtomicU32::new(0);
static SYSTEM_ASKED: AtomicI64 = AtomicI64::new(i64::MIN);

/// The names UTC goes by.
const UTC_NAMES: [&str; 12] =
    ["UTC", "Z", "GMT", "Zulu", "UCT", "Universal", "Etc/UTC", "Etc/GMT", "Etc/UCT", "Etc/Universal", "Etc/Zulu", "tzone://Microsoft/Utc"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    /// This computer's zone when the system can't say which one it is.
    Local,
    /// Always this many minutes east of UTC.
    Fixed(i32),
    /// A zone of the database, by its place in the list of names.
    Named(u16),
}

/// A time zone: one of the IANA database (`Europe/Berlin`), a fixed offset from UTC, or this
/// computer's own when the system can't name it ([`Zone::LOCAL`]).
///
/// A zone turns an instant into the wall-clock time there ([`Zone::wall`]) and a wall-clock time
/// into its instant ([`Zone::instant`]). Its [`id`](Zone::id) is what [`Zone::named`] reads back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Zone(Kind);

/// A change of a zone's offset: at `at` the clocks there go from `before` to `after` (seconds
/// east of UTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub at: Timestamp,
    pub before: i32,
    pub after: i32,
}

/// The database's names, and the zones that were asked for so far.
struct Db {
    /// As the database spells them, sorted by `lower`.
    names: Vec<String>,
    lower: Vec<String>,
    zones: Vec<OnceLock<Option<jiff::tz::TimeZone>>>,
    /// For each name, the row of [`windows::WINDOWS_ZONES`] that calls it.
    windows: Vec<Option<u16>>,
}

fn db() -> &'static Db {
    static DB: OnceLock<Db> = OnceLock::new();
    DB.get_or_init(|| {
        let mut names: Vec<(String, String)> =
            jiff::tz::db().available().map(|name| (name.as_str().to_ascii_lowercase(), name.as_str().to_string())).collect();
        names.sort();
        names.dedup_by(|a, b| a.0 == b.0);
        names.truncate(usize::from(u16::MAX));
        let (lower, names): (Vec<String>, Vec<String>) = names.into_iter().unzip();
        let find = |name: &str| lower.binary_search(&name.to_ascii_lowercase()).ok();
        let mut called: Vec<Option<u16>> = vec![None; names.len()];
        for (row, (_, iana)) in windows::WINDOWS_ZONES.iter().enumerate() {
            // The first row of a zone is the name it goes by.
            if let Some(slot) = find(iana).and_then(|at| called.get_mut(at))
                && slot.is_none()
            {
                *slot = u16::try_from(row).ok();
            }
        }
        for (alias, iana) in windows::ALIASES {
            let row = find(iana).and_then(|at| called.get(at).copied().flatten());
            if let Some(slot) = find(alias).and_then(|at| called.get_mut(at))
                && slot.is_none()
            {
                *slot = row;
            }
        }
        let zones = names.iter().map(|_| OnceLock::new()).collect();
        Db { names, lower, zones, windows: called }
    })
}

impl Db {
    fn index(&self, name: &str) -> Option<u16> {
        self.lower.binary_search(&name.to_ascii_lowercase()).ok().and_then(|at| u16::try_from(at).ok())
    }

    fn name(&self, index: u16) -> &str {
        self.names.get(usize::from(index)).map_or("", String::as_str)
    }

    fn zone(&self, index: u16) -> Option<&jiff::tz::TimeZone> {
        let slot = self.zones.get(usize::from(index))?;
        slot.get_or_init(|| jiff::tz::db().get(self.name(index)).ok()).as_ref()
    }
}

/// The offset (seconds east of UTC) a database zone has at an instant; 0 when it can't say.
fn offset_in(zone: &jiff::tz::TimeZone, instant: Timestamp) -> i64 {
    match jiff::Timestamp::from_second(instant.clamp(FIRST_INSTANT, LAST_INSTANT)) {
        Ok(stamp) => i64::from(zone.to_offset(stamp).seconds()),
        Err(_) => 0,
    }
}

/// The instant of a wall-clock time in a database zone. A time the clocks skip (the hour lost in
/// spring) is read with the offset from before the change, so it lands as much after the change
/// as it was written after it; a time they show twice (the hour repeated in autumn) is the first
/// of the two. That is how iCalendar (RFC 5545 §3.3.5) reads such times.
fn instant_in(zone: &jiff::tz::TimeZone, wall: DateTime) -> Timestamp {
    let fields = (
        i16::try_from(wall.date.year),
        i8::try_from(wall.date.month),
        i8::try_from(wall.date.day),
        i8::try_from(wall.hour),
        i8::try_from(wall.minute),
        i8::try_from(wall.second),
    );
    let exact = match fields {
        (Ok(year), Ok(month), Ok(day), Ok(hour), Ok(minute), Ok(second)) => jiff::civil::DateTime::new(year, month, day, hour, minute, second, 0)
            .ok()
            .and_then(|civil| zone.to_ambiguous_timestamp(civil).compatible().ok())
            .map(|stamp| stamp.as_second()),
        _ => None,
    };
    exact.unwrap_or_else(|| {
        // A date the database has no way of taking (it doesn't exist, or lies at the very edge
        // of the calendar): by the offset of its own moment, asked twice.
        let civil = wall.civil_seconds();
        let guess = civil.saturating_sub(offset_in(zone, civil));
        civil.saturating_sub(offset_in(zone, guess))
    })
}

/// This computer's zone as the system has it, named or not.
#[cfg(not(target_arch = "wasm32"))]
fn local_offset_seconds(instant: Timestamp) -> i64 {
    match jiff::tz::TimeZone::try_system() {
        Ok(zone) => offset_in(&zone, instant),
        Err(_) => 0,
    }
}

/// The browser's zone, as its own clock has it.
#[cfg(target_arch = "wasm32")]
fn local_offset_seconds(instant: Timestamp) -> i64 {
    let date = js_sys::Date::new_0();
    date.set_time(instant as f64 * 1000.0);
    let minutes = date.get_timezone_offset();
    if minutes.is_finite() && minutes.abs() <= 24.0 * 60.0 { -(minutes as i64) * MINUTE } else { 0 }
}

#[cfg(not(target_arch = "wasm32"))]
fn local_instant(wall: DateTime) -> Timestamp {
    match jiff::tz::TimeZone::try_system() {
        Ok(zone) => instant_in(&zone, wall),
        Err(_) => wall.civil_seconds(),
    }
}

#[cfg(target_arch = "wasm32")]
fn local_instant(wall: DateTime) -> Timestamp {
    let civil = wall.civil_seconds();
    let guess = civil.saturating_sub(local_offset_seconds(civil));
    civil.saturating_sub(local_offset_seconds(guess))
}

/// `+05:30`, `-0800`, `+2`, `-03`: minutes east of UTC; `None` for anything else, and for more
/// than a zone can be away from UTC.
fn signed_offset(text: &str) -> Option<i32> {
    let (sign, digits) = match (text.strip_prefix('+'), text.strip_prefix(['-', '\u{2212}'])) {
        (Some(rest), _) => (1, rest),
        (None, Some(rest)) => (-1, rest),
        (None, None) => return None,
    };
    let (hours, minutes) = match digits.split_once([':', '.']) {
        Some((hours, minutes)) => (hours, minutes),
        None if digits.len() > 2 => digits.split_at_checked(digits.len().saturating_sub(2))?,
        None => (digits, "0"),
    };
    let number =
        |part: &str| if !part.is_empty() && part.len() <= 2 && part.bytes().all(|b| b.is_ascii_digit()) { part.parse::<i32>().ok() } else { None };
    let (hours, minutes) = (number(hours)?, number(minutes)?);
    let total = hours.checked_mul(60)?.checked_add(minutes)?;
    (minutes < 60 && total <= MAX_OFFSET).then_some(sign * total)
}

impl Zone {
    /// UTC.
    pub const UTC: Zone = Zone(Kind::Fixed(0));

    /// This computer's zone as the system reckons it, for when the system can't say which zone
    /// of the database it is: it has offsets, and no name ([`Zone::id`] is empty).
    pub const LOCAL: Zone = Zone(Kind::Local);

    /// The zone that is always `minutes` east of UTC; `None` for more than eighteen hours.
    pub fn fixed(minutes: i32) -> Option<Zone> {
        (-MAX_OFFSET..=MAX_OFFSET).contains(&minutes).then_some(Zone(Kind::Fixed(minutes)))
    }

    /// The zone a name stands for: a name of the IANA database (`Europe/Berlin`, in any case, also
    /// behind the prefix some programs put before it: `/example.org/2024/Europe/Berlin`), a name
    /// Windows and Microsoft's services use (`W. Europe Standard Time`), one of the names of UTC,
    /// or an offset (`UTC+05:30`, `GMT-8`, `+0200`). `None` for anything else: an unknown zone is
    /// never guessed at.
    pub fn named(name: &str) -> Option<Zone> {
        if name.len() > MAX_ZONE_NAME {
            return None;
        }
        let name = name.trim().trim_matches('"').trim();
        if name.is_empty() {
            return None;
        }
        if UTC_NAMES.iter().any(|utc| utc.eq_ignore_ascii_case(name)) {
            return Some(Zone::UTC);
        }
        let db = db();
        let known = |name: &str| {
            let canonical = windows::ALIASES.iter().find(|(alias, _)| alias.eq_ignore_ascii_case(name)).map_or(name, |(_, iana)| *iana);
            if UTC_NAMES.iter().any(|utc| utc.eq_ignore_ascii_case(canonical)) {
                return Some(Zone::UTC);
            }
            db.index(canonical).or_else(|| db.index(name)).map(|index| Zone(Kind::Named(index)))
        };
        if let Some(zone) = known(name) {
            return Some(zone);
        }
        if let Some((_, iana)) = windows::WINDOWS_ZONES.iter().find(|(windows, _)| windows.eq_ignore_ascii_case(name)) {
            return known(iana);
        }
        // `/mozilla.org/20070129_1/Europe/Berlin`: the longest tail that is a zone.
        if name.starts_with('/') {
            let tails = name.char_indices().filter(|(_, c)| *c == '/').filter_map(|(at, _)| name.get(at.saturating_add(1)..));
            if let Some(zone) = tails.filter(|tail| tail.contains('/')).find_map(known) {
                return Some(zone);
            }
        }
        let offset = ["UTC", "GMT", "utc", "gmt", "Utc", "Gmt"].iter().find_map(|prefix| name.strip_prefix(prefix)).unwrap_or(name).trim();
        signed_offset(offset).and_then(Zone::fixed)
    }

    /// The offset a name mentions when it is not the name of a zone, as in
    /// `(UTC-05:00) Somewhere` or `GMT+0100`: all that can be told of such a zone.
    pub fn offset_in_name(name: &str) -> Option<i32> {
        if name.len() > MAX_ZONE_NAME {
            return None;
        }
        let upper = name.to_ascii_uppercase();
        let at = upper.find("UTC").or_else(|| upper.find("GMT"))?;
        let rest = name.get(at.saturating_add(3)..)?.trim_start();
        let end = rest
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_digit() || matches!(c, '+' | '-' | ':' | '.' | '\u{2212}')))
            .map_or(rest.len(), |(at, _)| at);
        signed_offset(rest.get(..end)?)
    }

    /// This computer's zone (the browser's, on the web): the zone of the database the system
    /// names, or [`Zone::LOCAL`] when it names none that is known. What the system said is kept
    /// for a few seconds ([`SYSTEM_KEPT`]), and then asked again where that is cheap (the `TZ`
    /// variable, the link `/etc/localtime` is on Linux, macOS and the BSDs, the browser's own
    /// setting): a zone that changes while the program runs is noticed within seconds there. On
    /// Windows the database's own look at the system is used, which it keeps for some minutes;
    /// [`Zone::refresh_system`] looks past it, and a front end calls that when its window comes
    /// back to the front or the computer wakes.
    pub fn system() -> Zone {
        // Asked for with every message of a list and every frame of a window.
        let now = crate::now();
        let kept = SYSTEM_SAID.load(Ordering::Relaxed);
        if kept != 0 && (0..SYSTEM_KEPT).contains(&now.saturating_sub(SYSTEM_ASKED.load(Ordering::Relaxed))) {
            return u16::try_from(kept.saturating_sub(2)).ok().filter(|_| kept >= 2).map_or(Zone::LOCAL, |index| Zone(Kind::Named(index)));
        }
        let zone = system_named(false).unwrap_or_else(system_of_database);
        keep_system(zone, now);
        zone
    }

    /// This computer's zone, asked of the system now, past every cache (Windows' included, where
    /// that means asking the system's `tzutil`, without a window): for the moment the program's
    /// window comes back to the front or the computer wakes, when it may have been carried to
    /// another zone. [`Zone::system`] says the same from then on.
    pub fn refresh_system() -> Zone {
        let zone = system_named(true).unwrap_or_else(system_of_database);
        keep_system(zone, crate::now());
        zone
    }

    /// Whether it is [`Zone::LOCAL`]: the system's zone, which has no name.
    pub fn is_local(self) -> bool {
        self.0 == Kind::Local
    }

    /// What the zone is called, and what [`Zone::named`] reads back: the IANA name, `UTC`,
    /// `UTC+05:30`; empty for [`Zone::LOCAL`].
    pub fn id(self) -> String {
        match self.0 {
            Kind::Local => String::new(),
            Kind::Fixed(0) => "UTC".into(),
            Kind::Fixed(minutes) => offset_text(i64::from(minutes) * MINUTE),
            Kind::Named(index) => db().name(index).to_string(),
        }
    }

    /// Seconds east of UTC at an instant (zones had offsets with seconds before railways).
    pub fn offset_seconds_at(self, instant: Timestamp) -> i64 {
        match self.0 {
            Kind::Local => local_offset_seconds(instant),
            Kind::Fixed(minutes) => i64::from(minutes) * MINUTE,
            Kind::Named(index) => db().zone(index).map_or(0, |zone| offset_in(zone, instant)),
        }
    }

    /// Minutes east of UTC at an instant, daylight saving included.
    pub fn offset_at(self, instant: Timestamp) -> i32 {
        i32::try_from(self.offset_seconds_at(instant).div_euclid(MINUTE)).unwrap_or(0)
    }

    /// The wall-clock time there at an instant.
    pub fn wall(self, instant: Timestamp) -> DateTime {
        DateTime::from_civil_seconds(instant.saturating_add(self.offset_seconds_at(instant)))
    }

    /// The instant of a wall-clock time there. A time the clocks skip is taken as much after the
    /// change as it is written after it; a time they show twice is the first of the two.
    pub fn instant(self, wall: DateTime) -> Timestamp {
        match self.0 {
            Kind::Local => local_instant(wall),
            Kind::Fixed(minutes) => wall.civil_seconds().saturating_sub(i64::from(minutes) * MINUTE),
            Kind::Named(index) => match db().zone(index) {
                Some(zone) => instant_in(zone, wall),
                None => wall.civil_seconds(),
            },
        }
    }

    /// A wall-clock time there as the wall-clock time of the same instant in `other`.
    pub fn wall_in(self, wall: DateTime, other: Zone) -> DateTime {
        if self == other { wall } else { other.wall(self.instant(wall)) }
    }

    /// The name Windows and Microsoft's services have for the zone (`Eastern Standard Time`),
    /// when they have one.
    pub fn windows_name(self) -> Option<&'static str> {
        match self.0 {
            Kind::Fixed(0) => Some("UTC"),
            Kind::Named(index) => {
                let row = db().windows.get(usize::from(index)).copied().flatten()?;
                windows::WINDOWS_ZONES.get(usize::from(row)).map(|(windows, _)| *windows)
            }
            _ => None,
        }
    }

    /// What the zone is called in a line of text: `Eastern Time`, `W. Europe Time`, `UTC`; for a
    /// zone Windows has no name for, its place (`Detroit`); for an offset, the offset.
    pub fn label(self) -> String {
        match (self.0, self.windows_name()) {
            (Kind::Local, _) => "Local Time".into(),
            (_, Some(windows)) => windows.replacen(" Standard Time", " Time", 1),
            (Kind::Named(index), None) if db().name(index).starts_with("Etc/") => offset_text(self.offset_seconds_at(0)),
            (Kind::Named(_), None) => self.place(),
            (Kind::Fixed(minutes), None) => offset_text(i64::from(minutes) * MINUTE),
        }
    }

    /// The place a zone of the database is named after (`New York`, `Buenos Aires`); empty for
    /// the others.
    pub fn place(self) -> String {
        match self.0 {
            Kind::Named(index) => {
                let name = db().name(index);
                if name.starts_with("Etc/") { String::new() } else { name.rsplit('/').next().unwrap_or("").replace('_', " ") }
            }
            _ => String::new(),
        }
    }

    /// What the zone is called in full, for a list to choose from or a tooltip:
    /// `(UTC-05:00) Eastern Time — New York, Americas`, `(UTC+01:00) West Europe Time — Berlin,
    /// Europe`, `(UTC-05:00) Indianapolis — Indiana, Americas`. Made of the zone's identifiers
    /// alone: Windows' name for it with its abbreviations spelled out (and the place where it is
    /// an acronym that says nothing to a reader), and the place and part of the world the
    /// database names it after. Nothing of any program's own list of zone names.
    pub fn long_name(self, instant: Timestamp) -> String {
        let offset = self.offset_label(instant);
        let label = self.readable_label();
        let (place, around) = match self.0 {
            Kind::Named(index) => places(db().name(index)),
            _ => (String::new(), Vec::new()),
        };
        let mut said = format!("({offset}) {label}");
        let mut more: Vec<String> = Vec::new();
        if !place.is_empty() && !label.contains(&place) {
            more.push(place);
        }
        more.extend(around);
        if label == offset || (self.0 != Kind::Local && label.is_empty()) {
            said = format!("({offset})");
        }
        if !more.is_empty() {
            said = format!("{said} — {}", more.join(", "));
        }
        said
    }

    /// [`Zone::label`] with the abbreviations of Windows' names spelled out (`W. Europe Time` is
    /// `West Europe Time`, `SA Pacific Time` is `South America Pacific Time`); a name made of an
    /// acronym that says nothing to a reader (`GTB Time`) is the place's (`Bucharest Time`).
    pub fn readable_label(self) -> String {
        let label = self.label();
        if self.windows_name().is_none() || self.0 == Kind::Fixed(0) {
            return label;
        }
        let words: Vec<&str> = label.split(' ').collect();
        let opaque = words.iter().any(|word| word.len() >= 3 && word.chars().all(|c| c.is_ascii_uppercase()) && !matches!(*word, "UTC" | "AUS"));
        if opaque {
            let place = self.place();
            if !place.is_empty() {
                return format!("{place} Time");
            }
        }
        words
            .iter()
            .map(|word| match *word {
                "W." => "West",
                "E." => "East",
                "N." => "North",
                "S." => "South",
                "SE" => "Southeast",
                "NE" => "Northeast",
                "SA" => "South America",
                "AUS" => "Australia",
                "Cen." => "Central",
                other => other,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `UTC-05:00`, `UTC+05:45`, `UTC`: how far the zone is from UTC at an instant.
    pub fn offset_label(self, instant: Timestamp) -> String {
        offset_text(self.offset_seconds_at(instant))
    }

    /// Whether the two zones have the same clock around an instant: the same offset then and
    /// through the year after it. (`Europe/Berlin` and `Europe/Paris` are different zones with
    /// the same clock.)
    pub fn same_clock(self, other: Zone, instant: Timestamp) -> bool {
        self == other
            || (0..12).all(|month| {
                let at = instant.saturating_add(month * 31 * DAY);
                self.offset_seconds_at(at) == other.offset_seconds_at(at)
            })
    }

    /// The changes of the zone's offset from `from` up to `to`, in order (daylight saving
    /// beginning and ending, and the rare change of a zone's own time). At most a few hundred.
    pub fn transitions(self, from: Timestamp, to: Timestamp) -> Vec<Transition> {
        let Kind::Named(index) = self.0 else { return Vec::new() };
        let Some(zone) = db().zone(index) else { return Vec::new() };
        let (from, to) = (from.clamp(FIRST_INSTANT, LAST_INSTANT), to.clamp(FIRST_INSTANT, LAST_INSTANT));
        let Ok(start) = jiff::Timestamp::from_second(from) else { return Vec::new() };
        let mut before = zone.to_offset(start).seconds();
        let mut out = Vec::new();
        // The database lists changes of a zone's abbreviation too: only the offset counts here.
        for change in zone.following(start).take(MAX_TRANSITIONS * 4) {
            let at = change.timestamp().as_second();
            if at >= to || out.len() >= MAX_TRANSITIONS {
                break;
            }
            let after = change.offset().seconds();
            if after != before {
                out.push(Transition { at, before, after });
                before = after;
            }
        }
        out
    }

    /// The zones to choose from: one for each name Windows has, west to east. (Every zone of the
    /// database can be had by name: [`Zone::search`].)
    pub fn choices() -> Vec<Zone> {
        let mut out: Vec<Zone> = Vec::new();
        for (_, iana) in windows::WINDOWS_ZONES {
            if let Some(zone) = Zone::named(iana)
                && !out.contains(&zone)
            {
                out.push(zone);
            }
        }
        out
    }

    /// The zones whose name, place, Windows name or offset now (`+05:30`) has `query` in it, the
    /// zones of [`Zone::choices`] first and at most `limit` of them. Everything for an empty
    /// query.
    pub fn search(query: &str, now: Timestamp, limit: usize) -> Vec<Zone> {
        let query: String = query.trim().chars().take(MAX_ZONE_NAME).collect::<String>().to_lowercase();
        let fits = |zone: &Zone| {
            query.is_empty()
                || zone.label().to_lowercase().contains(&query)
                || zone.id().to_lowercase().replace('_', " ").contains(&query.replace('_', " "))
                || zone.windows_name().is_some_and(|windows| windows.to_lowercase().contains(&query))
                || zone.offset_label(now).to_lowercase().contains(&query)
        };
        let mut out: Vec<Zone> = Zone::choices().into_iter().filter(fits).collect();
        if !query.is_empty() {
            // What is called just that comes first, then what begins with it, shortest first.
            out.sort_by_key(|zone| {
                let label = zone.label().to_lowercase();
                if label == query {
                    (0, 0)
                } else if label.starts_with(&query) {
                    (1, label.len())
                } else {
                    (2, 0)
                }
            });
        }
        out.truncate(limit);
        if !query.is_empty() {
            let rest = (0..db().names.len()).filter_map(|index| u16::try_from(index).ok()).map(|index| Zone(Kind::Named(index)));
            for zone in rest.filter(fits) {
                if out.len() >= limit {
                    break;
                }
                // The same zone under a name of old is listed once, under the name it has now.
                let zone = Zone::named(&zone.id()).unwrap_or(zone);
                if !out.contains(&zone) {
                    out.push(zone);
                }
            }
        }
        out
    }
}

/// `UTC+05:30` for a number of seconds east of UTC; seconds are dropped.
fn offset_text(seconds: i64) -> String {
    let minutes = seconds.clamp(-48 * HOUR, 48 * HOUR).div_euclid(MINUTE);
    if minutes == 0 {
        return "UTC".into();
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    format!("UTC{sign}{:02}:{:02}", minutes.abs() / 60, minutes.abs() % 60)
}

/// The place a zone of the database is named after, and what its name says around it (a state,
/// the part of the world), in words: `America/Indiana/Indianapolis` is `Indianapolis` in
/// `Indiana, Americas`. Nothing for `Etc/…`.
fn places(name: &str) -> (String, Vec<String>) {
    if name.starts_with("Etc/") || !name.contains('/') {
        return (String::new(), Vec::new());
    }
    let parts: Vec<String> = name.split('/').take(4).map(|part| part.replace('_', " ")).collect();
    let Some((place, rest)) = parts.split_last() else { return (String::new(), Vec::new()) };
    let mut around: Vec<String> = rest.iter().skip(1).rev().cloned().collect();
    let world = match rest.first().map(String::as_str) {
        Some("America") => "Americas",
        Some("Indian") => "Indian Ocean",
        Some("Atlantic") => "Atlantic Ocean",
        Some("Pacific") => "Pacific Ocean",
        Some(other) => other,
        None => "",
    };
    if !world.is_empty() && world != place {
        around.push(world.to_string());
    }
    (place.clone(), around)
}

/// Keep what the system said of its zone, as of `now`. UTC and offsets are not kept: they are
/// rare for a computer, and cheap to find again.
fn keep_system(zone: Zone, now: Timestamp) {
    if let Kind::Named(index) = zone.0 {
        SYSTEM_SAID.store(u32::from(index) + 2, Ordering::Relaxed);
        SYSTEM_ASKED.store(now, Ordering::Relaxed);
    } else if zone.is_local() {
        SYSTEM_SAID.store(1, Ordering::Relaxed);
        SYSTEM_ASKED.store(now, Ordering::Relaxed);
    }
}

/// The system's zone as the database sees it (which keeps it for some minutes).
fn system_of_database() -> Zone {
    match jiff::tz::TimeZone::try_system() {
        Ok(zone) => zone.iana_name().and_then(Zone::named).unwrap_or(Zone::LOCAL),
        Err(_) => Zone::LOCAL,
    }
}

/// The zone a name the system gives is (`Europe/Berlin`, `:Europe/Berlin`, a path that ends in
/// `zoneinfo/Europe/Berlin`); `None` for anything else.
fn zone_of_system_name(name: &str) -> Option<Zone> {
    let name = name.trim().trim_start_matches(':');
    if name.is_empty() || name.len() > 4 * MAX_ZONE_NAME {
        return None;
    }
    let name = name.rsplit_once("zoneinfo/").map_or(name, |(_, rest)| rest);
    Zone::named(name).filter(|zone| !zone.is_local())
}

/// The system's zone, asked of the system itself where that is cheap and needs no cache: the
/// `TZ` variable; the link `/etc/localtime` is (Linux, macOS, the BSDs); the browser's setting.
/// With `thorough`, on Windows also its `tzutil` (a program run without a window), which is too
/// slow for every few seconds. `None` when none of that says a zone that is known.
fn system_named(thorough: bool) -> Option<Zone> {
    if let Some(zone) = std::env::var("TZ").ok().as_deref().and_then(zone_of_system_name) {
        return Some(zone);
    }
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let _ = thorough;
        let link = std::fs::read_link("/etc/localtime").ok()?;
        zone_of_system_name(&link.to_string_lossy())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = thorough;
        let options = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new()).resolved_options();
        let named = js_sys::Reflect::get(&options, &"timeZone".into()).ok()?.as_string()?;
        zone_of_system_name(&named)
    }
    #[cfg(windows)]
    {
        if !thorough {
            return None;
        }
        use std::os::windows::process::CommandExt;
        // (CREATE_NO_WINDOW: no console flashes up.)
        let out = std::process::Command::new("tzutil").arg("/g").creation_flags(0x0800_0000).output().ok()?;
        let said = String::from_utf8_lossy(out.stdout.get(..out.stdout.len().min(256))?).into_owned();
        zone_of_system_name(&said)
    }
    #[cfg(not(any(unix, windows, target_arch = "wasm32")))]
    {
        let _ = thorough;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Date;

    fn zone(name: &str) -> Zone {
        Zone::named(name).unwrap_or_else(|| panic!("no zone named {name}"))
    }

    fn at(y: i32, m: u8, d: u8, h: u8, min: u8) -> DateTime {
        DateTime::new(Date::new(y, m, d).unwrap(), h, min, 0).unwrap()
    }

    fn utc(y: i32, m: u8, d: u8, h: u8, min: u8) -> Timestamp {
        at(y, m, d, h, min).to_timestamp(0)
    }

    #[test]
    fn long_names_are_made_of_the_identifiers_and_read_as_words() {
        let october = utc(2026, 10, 12, 12, 0);
        assert_eq!(zone("America/New_York").long_name(october), "(UTC-04:00) Eastern Time — New York, Americas");
        assert_eq!(zone("Europe/Berlin").long_name(october), "(UTC+02:00) West Europe Time — Berlin, Europe");
        assert_eq!(zone("America/Bogota").readable_label(), "South America Pacific Time");
        assert_eq!(zone("America/Indiana/Indianapolis").long_name(october), "(UTC-04:00) US Eastern Time — Indianapolis, Indiana, Americas");
        // A zone Windows has no name for is its place; an acronym is the place's time.
        assert_eq!(zone("America/Detroit").long_name(october), "(UTC-04:00) Detroit — Americas");
        assert_eq!(zone("Europe/Bucharest").readable_label(), "Bucharest Time");
        assert_eq!(zone("Asia/Kolkata").long_name(october), "(UTC+05:30) India Time — Kolkata, Asia");
        assert_eq!(zone("Indian/Maldives").long_name(october).split(" — ").nth(1), Some("Indian Ocean"));
        // UTC, an offset, and the computer's zone without a name.
        assert_eq!(Zone::UTC.long_name(october), "(UTC)");
        assert_eq!(Zone::fixed(330).unwrap().long_name(october), "(UTC+05:30)");
        assert_eq!(zone("Etc/GMT+12").long_name(october), "(UTC-12:00) Dateline Time");
        assert!(Zone::LOCAL.long_name(october).ends_with("Local Time"));
        // Every zone there is has one, of one line.
        for zone in Zone::choices() {
            let name = zone.long_name(october);
            assert!(name.starts_with("(UTC") && !name.chars().any(char::is_control) && name.len() < 140, "{name}");
        }
    }

    #[test]
    fn what_the_system_calls_its_zone_is_read_as_a_name_or_not_at_all() {
        assert_eq!(zone_of_system_name("Europe/Berlin"), Some(zone("Europe/Berlin")));
        assert_eq!(zone_of_system_name(":America/New_York\\n"), Zone::named("America/New_York\\n").or(None));
        assert_eq!(zone_of_system_name(" :Asia/Tokyo "), Some(zone("Asia/Tokyo")));
        assert_eq!(zone_of_system_name("../usr/share/zoneinfo/Europe/Paris"), Some(zone("Europe/Paris")));
        assert_eq!(zone_of_system_name("/var/db/timezone/zoneinfo/Australia/Sydney"), Some(zone("Australia/Sydney")));
        assert_eq!(zone_of_system_name("Eastern Standard Time\\r\\n".trim_end_matches("\\r\\n")), Some(zone("America/New_York")));
        for nonsense in ["", ":", "/etc/localtime", "EST5EDT,M3.2.0,M11.1.0x", "zoneinfo/", "Nowhere/Never"] {
            assert_eq!(zone_of_system_name(nonsense), None, "{nonsense}");
        }
        assert_eq!(zone_of_system_name(&"a/".repeat(10_000)), None);
        // Asked past every cache, the system's zone is what it is asked for afterwards too.
        let fresh = Zone::refresh_system();
        assert_eq!(Zone::system(), fresh);
    }

    #[test]
    fn names_of_every_kind_are_read() {
        let berlin = zone("Europe/Berlin");
        assert_eq!(berlin.id(), "Europe/Berlin");
        for name in [
            "europe/berlin",
            " EUROPE/BERLIN ",
            "\"Europe/Berlin\"",
            "W. Europe Standard Time",
            "w. europe standard time",
            "/example.org/20070129_1/Europe/Berlin",
        ] {
            assert_eq!(Zone::named(name), Some(berlin), "{name}");
        }
        // A spelling of old is the zone it became.
        assert_eq!(zone("Asia/Calcutta"), zone("Asia/Kolkata"));
        assert_eq!(zone("Asia/Calcutta").id(), "Asia/Kolkata");
        assert_eq!(zone("India Standard Time"), zone("Asia/Kolkata"));
        assert_eq!(zone("US/Eastern"), zone("America/New_York"));
        for name in ["UTC", "utc", "Z", "GMT", "Etc/UTC", "Zulu", "tzone://Microsoft/Utc", "Etc/Universal"] {
            assert_eq!(Zone::named(name), Some(Zone::UTC), "{name}");
        }
        assert_eq!(Zone::UTC.id(), "UTC");
        for (name, minutes) in
            [("UTC+05:30", 330), ("GMT-8", -480), ("+0200", 120), ("-03", -180), ("UTC+5", 300), ("utc-09:30", -570), ("UTC\u{2212}05:00", -300)]
        {
            let fixed = zone(name);
            assert_eq!((fixed, fixed.offset_at(0)), (Zone::fixed(minutes).unwrap(), minutes), "{name}");
            assert_eq!(Zone::named(&fixed.id()), Some(fixed), "{name}");
        }
    }

    #[test]
    fn nonsense_names_are_no_zone() {
        let long = "Europe/Berlin".repeat(100_000);
        for name in [
            "",
            "   ",
            "Mars/Olympus_Mons",
            "Customized Time Zone",
            "Eastern",
            "Europe/",
            "/",
            "////",
            "Europe/Berlin/Extra",
            "UTC+99",
            "UTC+18:01",
            "+2400",
            "UTC+1:99",
            "UTC+",
            "+",
            "UTC+0530+0530",
            "\u{0}",
            "é/ü",
            long.as_str(),
        ] {
            assert_eq!(Zone::named(name), None, "{:?}", name.get(..name.len().min(40)));
        }
        assert_eq!(Zone::fixed(18 * 60 + 1), None);
        assert_eq!(Zone::fixed(i32::MIN), None);
        // What can still be told of a zone nobody knows: the offset its name mentions.
        assert_eq!(Zone::offset_in_name("(UTC-05:00) Somewhere Else"), Some(-300));
        assert_eq!(Zone::offset_in_name("(GMT+05.30) Nowhere"), Some(330));
        assert_eq!(Zone::offset_in_name("GMT+0100"), Some(60));
        assert_eq!(Zone::offset_in_name("Nowhere Standard Time"), None);
        assert_eq!(Zone::offset_in_name("(UTC) Coordinated"), None);
        assert_eq!(Zone::offset_in_name("(UTC+72:00) Days Away"), None);
        assert_eq!(Zone::offset_in_name(&long), None);
    }

    #[test]
    fn every_windows_name_is_a_zone_of_the_database() {
        let mut seen = std::collections::HashSet::new();
        for (windows, iana) in windows::WINDOWS_ZONES {
            assert!(seen.insert(windows.to_ascii_lowercase()), "{windows} is listed twice");
            let zone = Zone::named(windows).unwrap_or_else(|| panic!("{windows} names no zone"));
            assert_eq!(Some(zone), Zone::named(iana), "{windows}");
            // The table spells IANA names as the database does today.
            assert!(zone.id() == *iana || *iana == "Etc/UTC", "{windows}: {iana} is {} in the database", zone.id());
            // And the zone is called by a Windows name again (its own, or the one that took over).
            let back = zone.windows_name().unwrap_or_else(|| panic!("{iana} has no Windows name"));
            assert_eq!(Zone::named(back), Some(zone), "{windows}");
        }
        for (alias, iana) in windows::ALIASES {
            assert_eq!(Zone::named(alias), Zone::named(iana), "{alias}");
            assert!(Zone::named(alias).is_some(), "{alias}");
        }
        assert_eq!(zone("America/New_York").windows_name(), Some("Eastern Standard Time"));
        assert_eq!(zone("Asia/Kamchatka").windows_name(), Some("Russia Time Zone 11"));
        assert_eq!(zone("America/Detroit").windows_name(), None);
        assert_eq!(Zone::UTC.windows_name(), Some("UTC"));
        assert!(Zone::choices().len() > 130);
    }

    #[test]
    fn daylight_saving_in_the_north_and_in_the_south() {
        let new_york = zone("America/New_York");
        // 2026: clocks go forward on March 8 at 2:00 (7:00 UTC) and back on November 1 at 2:00 (6:00 UTC).
        assert_eq!(new_york.offset_at(utc(2026, 3, 8, 6, 59)), -300);
        assert_eq!(new_york.offset_at(utc(2026, 3, 8, 7, 0)), -240);
        assert_eq!(new_york.offset_at(utc(2026, 11, 1, 5, 59)), -240);
        assert_eq!(new_york.offset_at(utc(2026, 11, 1, 6, 0)), -300);
        assert_eq!(new_york.wall(utc(2026, 7, 4, 16, 0)), at(2026, 7, 4, 12, 0));
        assert_eq!(new_york.instant(at(2026, 1, 15, 9, 0)), utc(2026, 1, 15, 14, 0));
        // Sydney is the other way round: summer time until April 5 at 3:00, again from October 4 at 2:00.
        let sydney = zone("Australia/Sydney");
        assert_eq!(sydney.offset_at(utc(2026, 1, 15, 0, 0)), 660);
        assert_eq!(sydney.offset_at(utc(2026, 4, 4, 15, 59)), 660);
        assert_eq!(sydney.offset_at(utc(2026, 4, 4, 16, 0)), 600);
        assert_eq!(sydney.offset_at(utc(2026, 10, 3, 15, 59)), 600);
        assert_eq!(sydney.offset_at(utc(2026, 10, 3, 16, 0)), 660);
        assert_eq!(sydney.instant(at(2026, 7, 1, 9, 0)), utc(2026, 6, 30, 23, 0));
        // Nine in New York seen from Sydney, in each half of the year.
        assert_eq!(new_york.wall_in(at(2026, 1, 15, 9, 0), sydney), at(2026, 1, 16, 1, 0));
        assert_eq!(new_york.wall_in(at(2026, 7, 15, 9, 0), sydney), at(2026, 7, 15, 23, 0));
    }

    #[test]
    fn the_hour_that_is_skipped_and_the_hour_that_comes_twice() {
        let new_york = zone("America/New_York");
        // 2:30 on March 8 doesn't exist: half an hour after the change, which is 3:30.
        let skipped = new_york.instant(at(2026, 3, 8, 2, 30));
        assert_eq!(skipped, utc(2026, 3, 8, 7, 30));
        assert_eq!(new_york.wall(skipped), at(2026, 3, 8, 3, 30));
        // 1:30 on November 1 comes twice: the first one, still in summer time.
        let twice = new_york.instant(at(2026, 11, 1, 1, 30));
        assert_eq!(twice, utc(2026, 11, 1, 5, 30));
        assert_eq!(new_york.wall(twice), at(2026, 11, 1, 1, 30));
        assert_eq!(new_york.wall(twice + 3600), at(2026, 11, 1, 1, 30));
        // Lord Howe Island changes by half an hour.
        let lord_howe = zone("Australia/Lord_Howe");
        assert_eq!((lord_howe.offset_at(utc(2026, 1, 15, 0, 0)), lord_howe.offset_at(utc(2026, 7, 15, 0, 0))), (660, 630));
    }

    #[test]
    fn zones_off_the_hour() {
        for (name, minutes) in
            [("Asia/Kolkata", 330), ("Asia/Kathmandu", 345), ("Australia/Eucla", 525), ("Pacific/Marquesas", -570), ("Asia/Kabul", 270)]
        {
            let zone = zone(name);
            let noon = utc(2026, 6, 15, 12, 0);
            assert_eq!(zone.offset_at(noon), minutes, "{name}");
            assert_eq!(zone.instant(zone.wall(noon)), noon, "{name}");
        }
        assert_eq!(zone("Pacific/Chatham").offset_at(utc(2026, 1, 15, 0, 0)), 13 * 60 + 45);
        assert_eq!(zone("Asia/Kathmandu").wall(utc(2026, 6, 15, 12, 0)), at(2026, 6, 15, 17, 45));
        assert_eq!(zone("Asia/Kathmandu").offset_label(0), "UTC+05:30");
        assert_eq!(zone("Asia/Kathmandu").offset_label(utc(2026, 1, 1, 0, 0)), "UTC+05:45");
    }

    #[test]
    fn rules_that_changed_are_remembered() {
        // The United States began daylight saving in April until 2006, in March since 2007.
        let new_york = zone("America/New_York");
        assert_eq!(new_york.offset_at(utc(2006, 3, 20, 12, 0)), -300);
        assert_eq!(new_york.offset_at(utc(2007, 3, 20, 12, 0)), -240);
        // Russia gave up changing its clocks in 2011, and went back an hour for good in 2014.
        let moscow = zone("Europe/Moscow");
        assert_eq!(moscow.offset_at(utc(2010, 1, 15, 12, 0)), 180);
        assert_eq!(moscow.offset_at(utc(2010, 7, 15, 12, 0)), 240);
        assert_eq!(moscow.offset_at(utc(2013, 1, 15, 12, 0)), 240);
        assert_eq!(moscow.offset_at(utc(2026, 7, 15, 12, 0)), 180);
        // Samoa crossed the date line at the end of 2011.
        let apia = zone("Pacific/Apia");
        assert_eq!(apia.wall(utc(2011, 12, 30, 9, 59)).date, Date::new(2011, 12, 29).unwrap());
        assert_eq!(apia.wall(utc(2011, 12, 30, 10, 0)).date, Date::new(2011, 12, 31).unwrap());
    }

    #[test]
    fn dates_far_away_and_hostile_numbers() {
        let new_york = zone("America/New_York");
        // Before standard time: local mean time, with its seconds, and back again exactly.
        let early = at(1850, 6, 1, 12, 0);
        assert_eq!(new_york.offset_seconds_at(new_york.instant(early)), -17_762);
        assert_eq!(new_york.wall(new_york.instant(early)), early);
        // Far ahead: today's rule goes on.
        assert_eq!(new_york.offset_at(utc(2400, 7, 1, 12, 0)), -240);
        assert_eq!(new_york.offset_at(utc(2400, 1, 1, 12, 0)), -300);
        assert_eq!(new_york.offset_at(utc(9999, 12, 31, 23, 59)), -300);
        for zone in [new_york, Zone::UTC, Zone::LOCAL, Zone::fixed(-720).unwrap(), zone("Pacific/Kiritimati")] {
            for instant in [i64::MIN, -1, 0, 1, i64::MAX, FIRST_INSTANT - 1, LAST_INSTANT + 1] {
                let wall = zone.wall(instant);
                let _ = (zone.offset_at(instant), zone.instant(wall), zone.offset_label(instant), zone.transitions(instant, i64::MAX));
            }
            for date in [Date { year: 0, month: 0, day: 0 }, Date { year: i32::MAX, month: 255, day: 255 }, Date { year: 2026, month: 2, day: 30 }] {
                let _ = zone.instant(DateTime { date, hour: 99, minute: 99, second: 99 });
            }
            let _ = (zone.label(), zone.place(), zone.id(), zone.windows_name());
        }
    }

    #[test]
    fn labels() {
        assert_eq!(zone("America/New_York").label(), "Eastern Time");
        assert_eq!(zone("America/New_York").place(), "New York");
        assert_eq!(zone("America/Tijuana").label(), "Pacific Time (Mexico)");
        assert_eq!(zone("America/Argentina/Buenos_Aires").place(), "Buenos Aires");
        assert_eq!(zone("America/Detroit").label(), "Detroit");
        assert_eq!(Zone::UTC.label(), "UTC");
        assert_eq!(zone("Etc/GMT+12").label(), "Dateline Time");
        assert_eq!(zone("Etc/GMT+5").label(), "UTC-05:00");
        assert_eq!(Zone::fixed(345).unwrap().label(), "UTC+05:45");
        assert_eq!(Zone::LOCAL.id(), "");
        assert_eq!(zone("America/New_York").offset_label(utc(2026, 7, 1, 0, 0)), "UTC-04:00");
        assert_eq!(Zone::UTC.offset_label(0), "UTC");
        // Two zones with one clock, and two without.
        let now = utc(2026, 10, 12, 0, 0);
        assert!(zone("Europe/Berlin").same_clock(zone("Europe/Paris"), now));
        assert!(!zone("Europe/Berlin").same_clock(zone("Europe/London"), now));
        assert!(!zone("America/Phoenix").same_clock(zone("America/Denver"), now));
    }

    #[test]
    fn transitions_are_listed() {
        let changes = zone("Europe/Berlin").transitions(utc(2026, 1, 1, 0, 0), utc(2027, 1, 1, 0, 0));
        assert_eq!(
            changes,
            [
                Transition { at: utc(2026, 3, 29, 1, 0), before: 3600, after: 7200 },
                Transition { at: utc(2026, 10, 25, 1, 0), before: 7200, after: 3600 },
            ]
        );
        assert!(zone("Asia/Kolkata").transitions(utc(2026, 1, 1, 0, 0), utc(2027, 1, 1, 0, 0)).is_empty());
        assert!(Zone::UTC.transitions(0, i64::MAX).is_empty());
        assert!(zone("Europe/Berlin").transitions(0, i64::MAX).len() <= MAX_TRANSITIONS);
        assert!(zone("Europe/Berlin").transitions(100, 0).is_empty());
    }

    #[test]
    fn zones_are_found_by_what_they_are_called() {
        let now = utc(2026, 10, 12, 0, 0);
        let ids = |query: &str| Zone::search(query, now, 5).into_iter().map(Zone::id).collect::<Vec<_>>();
        assert_eq!(ids("eastern time")[0], "America/New_York");
        assert_eq!(ids("Eastern Time (Mexico)"), ["America/Cancun"]);
        assert_eq!(ids("kolkata"), ["Asia/Kolkata"]);
        assert_eq!(ids("Buenos Aires"), ["America/Argentina/Buenos_Aires"]);
        assert!(ids("detroit").contains(&"America/Detroit".to_string()));
        assert!(ids("+05:45").contains(&"Asia/Kathmandu".to_string()));
        assert!(ids("no such place anywhere").is_empty());
        assert_eq!(Zone::search("", now, 1000).len(), Zone::choices().len());
        assert_eq!(Zone::search(&"x".repeat(1_000_000), now, 5), []);
    }

    #[test]
    fn the_system_has_a_zone() {
        let system = Zone::system();
        let now = crate::now();
        assert!(system.offset_at(now).abs() <= 18 * 60);
        assert_eq!(system.offset_at(now), crate::local_offset_minutes(now));
        // Named or not, it reads its own wall-clock times back.
        assert_eq!(system.instant(system.wall(now)), now);
    }

    proptest::proptest! {
        #[test]
        fn an_instant_reads_back_through_any_zone(index in 0usize..600, instant in FIRST_INSTANT + 2 * DAY..LAST_INSTANT - 2 * DAY) {
            let names = &db().names;
            let zone = Zone::named(&names[index % names.len()]).unwrap();
            // The wall-clock time of an instant names that instant again, or (in an hour that
            // comes twice; once, in Alaska, a day) the first instant with that wall-clock time.
            let wall = zone.wall(instant);
            let back = zone.instant(wall);
            proptest::prop_assert_eq!(zone.wall(back), wall);
            proptest::prop_assert!(back <= instant && instant - back <= 26 * 3600, "{} at {}", zone.id(), instant);
        }

        #[test]
        fn a_wall_clock_time_is_an_instant_in_any_zone(index in 0usize..600, civil in FIRST_INSTANT + 2 * DAY..LAST_INSTANT - 2 * DAY) {
            let names = &db().names;
            let zone = Zone::named(&names[index % names.len()]).unwrap();
            // A time on the clock there is that time again, unless the clocks skipped it: then
            // it is as much later as the clocks jumped.
            let wall = DateTime::from_civil_seconds(civil);
            let shown = zone.wall(zone.instant(wall));
            let later = shown.civil_seconds() - wall.civil_seconds();
            proptest::prop_assert!((0..=26 * 3600).contains(&later), "{} at {}: {}", zone.id(), wall.iso(), shown.iso());
        }

        #[test]
        fn any_text_is_a_zone_or_none(name in proptest::prelude::any::<String>(), instant in proptest::num::i64::ANY) {
            if let Some(zone) = Zone::named(&name) {
                let _ = (zone.wall(instant), zone.id(), zone.label());
                proptest::prop_assert_eq!(Zone::named(&zone.id()), Some(zone));
            }
            let _ = Zone::offset_in_name(&name);
            let _ = Zone::search(&name, instant, 3);
        }
    }
}
