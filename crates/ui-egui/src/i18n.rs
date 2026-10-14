//! Interface text. Every string the interface shows goes through [`t`] (the `tl!` macro), keyed by
//! its English text; text that is only looked up later (a constant, a label in a table) is marked
//! where it is written with `tn!`, so that the list of what there is to translate can be read
//! from the source ([`keys`]).
//!
//! The translations are message catalogs, one plain text file per language under
//! `assets/i18n/` ([`catalog`] has the format), built into the program and read the first time a
//! language is used. Whatever a catalog doesn't have, or has with the wrong placeholders, is
//! shown in English. The language follows the system's unless File › Options names one
//! (`ui.language`), and the way dates, times and numbers are written follows it too
//! ([`locale`]). `pseudo` (a test language) proves that no string bypasses the lookup.
//!
//! The language is kept per thread: the window's thread is the one that draws, and each test
//! renders in its own language without disturbing the others.
//!
//! **Right to left.** Arabic and Hebrew are written right to left; egui lays out text left to
//! right and turns round each run of Hebrew or Arabic letters by itself. So the text of such a
//! language is handed out in the order it is drawn ([`html::bidi`](mailcraft_html::bidi)): a
//! translation without placeholders as it is looked up, one with placeholders by [`fmt`] once it
//! is filled in. Text that wraps onto several lines is ordered as one line and then wrapped, so a
//! long paragraph reads correctly only line by line (the catalogs of those languages translate
//! the short labels first). The window's layout is mirrored where egui allows it ([`rtl`],
//! [`start_panel`]).

pub mod catalog;
#[cfg(test)]
mod keys;
pub mod locale;
#[cfg(test)]
mod screens;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use mailcraft_html::bidi::{self, Direction};
use mailcraft_time::{Date, DateTime, Timestamp};

use locale::{Formats, Tag};

/// The interface language setting that follows the system.
pub const AUTO: &str = "auto";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    English,
    Spanish,
    French,
    German,
    /// Brazilian Portuguese (also what Portuguese of elsewhere gets, until it has its own).
    PortugueseBr,
    Arabic,
    Hebrew,
    /// Accented look-alikes of the English text, for finding strings that skip translation.
    Pseudo,
}

impl Lang {
    /// The languages File › Options offers, in the order it lists them.
    pub const ALL: [Lang; 7] = [Lang::English, Lang::Spanish, Lang::French, Lang::German, Lang::PortugueseBr, Lang::Arabic, Lang::Hebrew];

    /// The languages with a catalog.
    pub const TRANSLATED: [Lang; 6] = [Lang::Spanish, Lang::French, Lang::German, Lang::PortugueseBr, Lang::Arabic, Lang::Hebrew];

    pub fn code(self) -> &'static str {
        match self {
            Lang::English => "en",
            Lang::Spanish => "es",
            Lang::French => "fr",
            Lang::German => "de",
            Lang::PortugueseBr => "pt-BR",
            Lang::Arabic => "ar",
            Lang::Hebrew => "he",
            Lang::Pseudo => "pseudo",
        }
    }

    /// What the language calls itself, for the list in File › Options.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::Spanish => "Español",
            Lang::French => "Français",
            Lang::German => "Deutsch",
            Lang::PortugueseBr => "Português (Brasil)",
            Lang::Arabic => "العربية",
            Lang::Hebrew => "עברית",
            Lang::Pseudo => "Pséúdó",
        }
    }

    /// Written right to left.
    pub fn rtl(self) -> bool {
        matches!(self, Lang::Arabic | Lang::Hebrew)
    }

    /// The language of a locale's language part (`pt` for `pt-PT` too); `None` for one without a
    /// catalog.
    pub fn of_tag(tag: &Tag) -> Option<Lang> {
        Some(match tag.language.as_str() {
            "en" => Lang::English,
            "es" => Lang::Spanish,
            "fr" => Lang::French,
            "de" => Lang::German,
            "pt" => Lang::PortugueseBr,
            "ar" => Lang::Arabic,
            "he" | "iw" => Lang::Hebrew,
            _ => return None,
        })
    }

    /// The language a setting names (`de`, `pt-BR`, `pseudo`); `None` for `auto` or one unknown.
    pub fn from_code(code: &str) -> Option<Lang> {
        let code = code.trim();
        if code.eq_ignore_ascii_case("pseudo") {
            return Some(Lang::Pseudo);
        }
        Lang::of_tag(&Tag::parse(code))
    }

    /// The language for a saved setting: the one it names, else the system's, else English.
    pub fn from_pref(pref: &str) -> Lang {
        resolve(pref, &locale::system()).0
    }

    /// The catalog file built into the program.
    fn source(self) -> &'static str {
        match self {
            Lang::English | Lang::Pseudo => "",
            Lang::Spanish => include_str!("../../../assets/i18n/es.txt"),
            Lang::French => include_str!("../../../assets/i18n/fr.txt"),
            Lang::German => include_str!("../../../assets/i18n/de.txt"),
            Lang::PortugueseBr => include_str!("../../../assets/i18n/pt-BR.txt"),
            Lang::Arabic => include_str!("../../../assets/i18n/ar.txt"),
            Lang::Hebrew => include_str!("../../../assets/i18n/he.txt"),
        }
    }

    /// The locale a language is written in when the system's is of another language.
    fn default_tag(self) -> Tag {
        let (language, region) = match self {
            Lang::English | Lang::Pseudo => ("en", "US"),
            Lang::Spanish => ("es", "ES"),
            Lang::French => ("fr", "FR"),
            Lang::German => ("de", "DE"),
            Lang::PortugueseBr => ("pt", "BR"),
            Lang::Arabic => ("ar", "EG"),
            Lang::Hebrew => ("he", "IL"),
        };
        Tag { language: language.into(), region: region.into() }
    }
}

/// The language and the locale for a setting (`auto` or a language's code) on a system whose
/// locale is `system`. Dates are written as the system's region writes them when the language
/// is the system's, and as the language's home writes them otherwise.
pub fn resolve(pref: &str, system: &Tag) -> (Lang, Tag) {
    let named = if pref.trim().eq_ignore_ascii_case(AUTO) || pref.trim().is_empty() { None } else { Lang::from_code(pref) };
    match named {
        Some(Lang::Pseudo) => (Lang::Pseudo, Lang::English.default_tag()),
        Some(lang) => {
            let wanted = Tag::parse(pref);
            let tag = if !wanted.region.is_empty() {
                wanted
            } else if Lang::of_tag(system) == Some(lang) {
                system.clone()
            } else {
                lang.default_tag()
            };
            (lang, tag)
        }
        None => match Lang::of_tag(system) {
            Some(lang) => (lang, system.clone()),
            // A system language without a catalog: English words, the system's dates.
            None if !system.language.is_empty() => (Lang::English, system.clone()),
            None => (Lang::English, Lang::English.default_tag()),
        },
    }
}

/// One language's catalog as it is used.
struct Loaded {
    catalog: catalog::Catalog,
    /// For a right-to-left language: each translation without placeholders in drawn order.
    drawn: HashMap<String, String>,
    /// And back: the written text of each drawn one.
    written: HashMap<String, String>,
}

fn loaded(lang: Lang) -> &'static Loaded {
    static CATALOGS: [OnceLock<Loaded>; 6] = [const { OnceLock::new() }; 6];
    let index = Lang::TRANSLATED.iter().position(|l| *l == lang);
    static EMPTY: OnceLock<Loaded> = OnceLock::new();
    let Some(cell) = index.and_then(|i| CATALOGS.get(i)) else {
        return EMPTY.get_or_init(|| Loaded { catalog: catalog::Catalog::default(), drawn: HashMap::new(), written: HashMap::new() });
    };
    cell.get_or_init(|| {
        let (catalog, problems) = catalog::parse(lang.source());
        if !problems.is_empty() {
            log::debug!("catalog {}: {} line(s) not taken", lang.code(), problems.len());
        }
        let mut drawn = HashMap::new();
        let mut written = HashMap::new();
        if lang.rtl() {
            for value in catalog.entries.values() {
                if catalog::placeholders(value).is_empty() {
                    let shown = draw_order(value, Direction::Rtl);
                    written.insert(shown.clone(), value.clone());
                    drawn.insert(value.clone(), shown);
                }
            }
        }
        Loaded { catalog, drawn, written }
    })
}

/// A line of right-to-left text in the order egui draws it (see the module's notes).
fn draw_order(text: &str, base: Direction) -> String {
    if !text.chars().any(bidi::is_rtl_char) {
        return text.to_string();
    }
    // Each line on its own: a line break stays where it is.
    text.split('\n')
        .map(|line| {
            let (drawn, order) = bidi::visual_order(line, base);
            bidi::for_shaper(&drawn, &order).0
        })
        .collect::<Vec<_>>()
        .join("\n")
}

thread_local! {
    static LANG: Cell<Lang> = const { Cell::new(Lang::English) };
    static FORMATS: Cell<Formats> = const { Cell::new(Formats::US) };
    /// The locale [`FORMATS`] is of (`de-AT`).
    static TAG: RefCell<String> = const { RefCell::new(String::new()) };
    /// The setting [`apply`] last acted on.
    static APPLIED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Use `lang` (with its home's way of writing dates) on this thread.
pub fn set_current(lang: Lang) {
    LANG.with(|l| l.set(lang));
    FORMATS.with(|f| f.set(Formats::of(&lang.default_tag())));
    TAG.with(|t| *t.borrow_mut() = lang.default_tag().code());
    APPLIED.with(|a| *a.borrow_mut() = None);
}

/// Use the language and locale a setting (`auto` or a code) names on this thread. Says whether
/// anything changed (the window then sets the clock and the week to the locale's, where the owner
/// hasn't chosen otherwise).
pub fn apply(pref: &str) -> bool {
    let same = APPLIED.with(|a| a.borrow().as_deref() == Some(pref));
    if same {
        return false;
    }
    let (lang, tag) = resolve(pref, &locale::system());
    let before = (current(), formats());
    LANG.with(|l| l.set(lang));
    FORMATS.with(|f| f.set(Formats::of(&tag)));
    TAG.with(|t| *t.borrow_mut() = tag.code());
    APPLIED.with(|a| *a.borrow_mut() = Some(pref.to_string()));
    before != (lang, Formats::of(&tag))
}

pub fn current() -> Lang {
    LANG.with(Cell::get)
}

/// The current locale (`de-AT`; `en-US` before any is applied).
pub fn locale_code() -> String {
    let code = TAG.with(|t| t.borrow().clone());
    if code.is_empty() { Lang::English.default_tag().code() } else { code }
}

/// How the current locale writes dates, times and numbers.
pub fn formats() -> Formats {
    FORMATS.with(Cell::get)
}

/// The window is laid out right to left.
pub fn rtl() -> bool {
    current().rtl()
}

#[cfg(test)]
thread_local! {
    /// What [`t`] was asked for, while [`start_recording`] is on.
    static ASKED: RefCell<Option<std::collections::BTreeSet<String>>> = const { RefCell::new(None) };
}

/// Keep what [`t`] is asked for on this thread from now on.
#[cfg(test)]
pub(crate) fn start_recording() {
    ASKED.with(|a| *a.borrow_mut() = Some(Default::default()));
}

/// What [`t`] was asked for since [`start_recording`].
#[cfg(test)]
pub(crate) fn stop_recording() -> std::collections::BTreeSet<String> {
    ASKED.with(|a| a.borrow_mut().take()).unwrap_or_default()
}

/// `s` in the interface language.
pub fn t(s: &str) -> &str {
    #[cfg(test)]
    ASKED.with(|a| {
        if let Some(set) = a.borrow_mut().as_mut() {
            set.insert(s.to_string());
        }
    });
    match current() {
        Lang::English => s,
        Lang::Pseudo => pseudo(s),
        lang => {
            let loaded = loaded(lang);
            match loaded.catalog.entries.get(s) {
                Some(found) => loaded.drawn.get(found).unwrap_or(found).as_str(),
                None => s,
            }
        }
    }
}

/// Whether the current catalog has `s` (English and the test language have everything).
pub fn has(s: &str) -> bool {
    match current() {
        Lang::English | Lang::Pseudo => true,
        lang => loaded(lang).catalog.entries.contains_key(s),
    }
}

/// The pseudo-language form of `s`, interned so it can be handed out as `&'static str`.
fn pseudo(s: &str) -> &'static str {
    use std::sync::Mutex;
    static CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(done) = cache.get(s) {
        return done;
    }
    // `{placeholders}` stay as they are.
    let mut out = String::with_capacity(s.len() + 2);
    let mut inside = false;
    for c in s.chars() {
        match c {
            '{' => inside = true,
            '}' => inside = false,
            _ => {}
        }
        out.push(if inside {
            c
        } else {
            match c {
                'a' => 'á',
                'e' => 'é',
                'i' => 'í',
                'o' => 'ó',
                'u' => 'ú',
                'A' => 'Á',
                'E' => 'É',
                'O' => 'Ó',
                'U' => 'Ú',
                _ => c,
            }
        });
    }
    // The catalog is small and fixed (interface strings), so leaking each form once is bounded;
    // text that isn't a catalog string (built from data) is not kept.
    if cache.len() >= 20_000 {
        cache.clear();
    }
    let leaked: &'static str = Box::leak(out.into_boxed_str());
    cache.insert(s.to_string(), leaked);
    leaked
}

/// Fill `{name}` placeholders in a (translated) template, in one pass: a value with braces in it
/// is not filled again. In a right-to-left language the result is put in drawn order.
pub fn fmt(template: &str, args: &[(&str, &str)]) -> String {
    if !current().rtl() {
        return locale::fill(template, args);
    }
    // Back to written order, fill in, and put the whole in drawn order once.
    let loaded = loaded(current());
    let written = |s: &str| loaded.written.get(s).cloned().unwrap_or_else(|| s.to_string());
    let template = written(template);
    let values: Vec<(&str, String)> = args.iter().map(|(k, v)| (*k, written(v))).collect();
    let pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let filled = locale::fill(&template, &pairs);
    let base = bidi::direction(&template).unwrap_or(Direction::Rtl);
    draw_order(&filled, base)
}

/// Text put together from translated pieces (a date, a list) in the order it is drawn, for a
/// right-to-left language; as it is for any other.
pub fn shown(text: String) -> String {
    if current().rtl() { draw_order(&text, Direction::Rtl) } else { text }
}

/// What the folder pane calls a folder: one of the folders every account has, while it still
/// has the English name it was made with, in the interface language; any other as it is named.
pub fn folder_name(folder: &mailcraft_model::Folder) -> &str {
    if folder.kind != mailcraft_model::FolderKind::Custom && folder.name == folder.kind.default_name() { t(&folder.name) } else { &folder.name }
}

/// The prefix of a catalog key for a ribbon tab's name, where the same English word means
/// something else elsewhere ("Home" the tab, and "Home" a contact's home phone or address).
pub const TAB_CONTEXT: &str = "ribbon tab|";

/// A ribbon tab's name in the interface language: the catalog's `ribbon tab|<name>` when it has
/// one, else the word as [`t`] has it.
pub fn tab(name: &str) -> &str {
    if matches!(current(), Lang::English | Lang::Pseudo) {
        return t(name);
    }
    let loaded = loaded(current());
    let key = format!("{TAB_CONTEXT}{name}");
    match loaded.catalog.entries.get(key.as_str()) {
        Some(found) => loaded.drawn.get(found).unwrap_or(found).as_str(),
        None => t(name),
    }
}

/// A command's location (`Home › Respond`) in the interface language, part by part (the first
/// part is a ribbon tab: [`tab`]).
pub fn location(loc: &str) -> String {
    let parts: Vec<&str> = loc.split(" › ").enumerate().map(|(i, part)| if i == 0 { tab(part) } else { t(part) }).collect();
    if current().rtl() {
        // Read from the right: the last part drawn first.
        parts.into_iter().rev().collect::<Vec<_>>().join(" ‹ ")
    } else {
        parts.join(" › ")
    }
}

// ---- dates, times and numbers ------------------------------------------------------------------

/// A month's whole name in the interface language.
pub fn month_name(month: u8) -> &'static str {
    t(mailcraft_time::month_name(month))
}

/// A month's short name in the interface language.
pub fn month_short(month: u8) -> &'static str {
    t(mailcraft_time::month_short(month))
}

/// `Sunday, October 11, 2026`, as the locale writes it.
pub fn date_long(d: Date) -> String {
    shown(formats().long_with(d, written(t(d.weekday().name())), written(t(mailcraft_time::month_name(d.month)))))
}

/// `Oct 11, 2026`, as the locale writes it.
pub fn date_medium(d: Date) -> String {
    shown(formats().medium_with(d, written(t(mailcraft_time::month_short(d.month)))))
}

/// `10/11/2026`, as the locale writes it.
pub fn date_numeric(d: Date) -> String {
    formats().numeric(d)
}

/// `Oct 11`, a day of this year as the locale writes it.
pub fn day_month(d: Date) -> String {
    shown(formats().day_month_with(d, written(t(mailcraft_time::month_short(d.month)))))
}

/// `October 11, 2026`: a date without its weekday, the month's whole name, as the locale writes it.
pub fn date_full(d: Date) -> String {
    shown(formats().full_with(d, written(t(mailcraft_time::month_name(d.month)))))
}

/// `October 11`: a day of the year with the month's whole name, as the locale writes it.
pub fn day_month_full(d: Date) -> String {
    shown(formats().day_month_full_with(d, written(t(mailcraft_time::month_name(d.month)))))
}

/// A stretch of days with the months' whole names: `October 11 – 17, 2026`,
/// `September 28 – October 4, 2026`, `11.–17. Oktober 2026`, `11–17 octobre 2026`.
pub fn date_range(from: Date, to: Date) -> String {
    let f = formats();
    if from == to {
        date_full(from)
    } else if from.year != to.year {
        shown(format!("{} – {}", date_full(from), date_full(to)))
    } else if from.month != to.month {
        shown(format!("{} – {}", day_month_full(from), date_full(to)))
    } else if f.order == locale::Order::MonthDayYear {
        shown(format!("{} – {}, {}", day_month_full(from), to.day, to.year))
    } else {
        let first = if f.full.contains("{day}.") { format!("{}.", from.day) } else { from.day.to_string() };
        shown(format!("{first}–{}", date_full(to)))
    }
}

/// `October 2026`, as the locale writes it.
pub fn month_year(year: i32, month: u8) -> String {
    shown(formats().month_year_with(year, written(t(mailcraft_time::month_name(month)))))
}

/// A time of day: `9:30 AM`, or `09:30` on a 24-hour clock.
pub fn time(hour: u8, minute: u8, twelve_hour: bool) -> String {
    mailcraft_time::time_short(hour, minute, twelve_hour)
}

/// A drawn translation back in written order, to be put into a pattern.
fn written(s: &str) -> &str {
    if current().rtl() { loaded(current()).written.get(s).map_or(s, String::as_str) } else { s }
}

/// The date column of a message list: the time today, `Sat 3:05 PM` within the last week,
/// `Oct 3` this year, `10/3/2025` before that (each as the locale writes it).
pub fn list_label(ts: Timestamp, now: Timestamp, offset_minutes: i32, twelve_hour: bool) -> String {
    let (at, today) = (DateTime::from_timestamp(ts, offset_minutes), DateTime::from_timestamp(now, offset_minutes));
    let diff = at.date.days_until(today.date);
    if diff == 0 {
        at.time_short(twelve_hour)
    } else if (1..7).contains(&diff) {
        shown(format!("{} {}", written(t(at.date.weekday().short())), at.time_short(twelve_hour)))
    } else if at.date.year == today.date.year {
        day_month(at.date)
    } else {
        date_numeric(at.date)
    }
}

/// The date line of an open message: `Sun 10/11/2026 9:30 AM`, as the locale writes it.
pub fn header_label(ts: Timestamp, offset_minutes: i32, twelve_hour: bool) -> String {
    let at = DateTime::from_timestamp(ts, offset_minutes);
    shown(format!("{} {} {}", written(t(at.date.weekday().short())), date_numeric(at.date), at.time_short(twelve_hour)))
}

/// A length of time for people: `30 minutes`, `1 hour`, `1.5 hours`, `2 days`.
pub fn duration(minutes: i64) -> String {
    let m = minutes.max(0);
    let n = |template: &str, value: &str| fmt(t(template), &[("n", value)]);
    if m < 60 {
        if m == 1 { n(tn!("{n} minute"), "1") } else { n(tn!("{n} minutes"), &m.to_string()) }
    } else if m < 24 * 60 {
        match (m / 60, m % 60) {
            (1, 0) => n(tn!("{n} hour"), "1"),
            (h, 0) => n(tn!("{n} hours"), &h.to_string()),
            (h, 30) => n(tn!("{n} hours"), &format!("{h}{}5", formats().decimal)),
            (h, r) => fmt(t(tn!("{h} h {m} min")), &[("h", &h.to_string()), ("m", &r.to_string())]),
        }
    } else {
        let d = m / (24 * 60);
        if d == 1 { n(tn!("{n} day"), "1") } else { n(tn!("{n} days"), &d.to_string()) }
    }
}

/// A number with `decimals` digits after the locale's decimal separator.
pub fn number(value: f64, decimals: usize) -> String {
    formats().number(value, decimals)
}

// ---- layout ------------------------------------------------------------------------------------

/// A side panel on the side lines start from: the left, or the right in a right-to-left
/// language.
pub fn start_panel(id: impl Into<egui::Id>) -> egui::Panel {
    if rtl() { egui::Panel::right(id) } else { egui::Panel::left(id) }
}

/// A side panel on the side lines end on: the right, or the left in a right-to-left language.
pub fn end_panel(id: impl Into<egui::Id>) -> egui::Panel {
    if rtl() { egui::Panel::left(id) } else { egui::Panel::right(id) }
}

/// A row that runs the way the language reads, its items aligned at `align` across it.
pub fn row_layout(align: egui::Align) -> egui::Layout {
    if rtl() { egui::Layout::right_to_left(align) } else { egui::Layout::left_to_right(align) }
}

/// A row that runs against the way the language reads (what sits at the far end of a row).
pub fn end_layout(align: egui::Align) -> egui::Layout {
    if rtl() { egui::Layout::left_to_right(align) } else { egui::Layout::right_to_left(align) }
}

/// Where a line of text starts: the left edge, or the right in a right-to-left language.
pub fn start_align() -> egui::Align {
    if rtl() { egui::Align::Max } else { egui::Align::Min }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(code: &str) -> Tag {
        Tag::parse(code)
    }

    #[test]
    fn formatting_and_pseudo() {
        set_current(Lang::English);
        assert_eq!(fmt("{n} of {total} items", &[("n", "3"), ("total", "10")]), "3 of 10 items");
        // A value isn't filled again.
        assert_eq!(fmt("{a} and {b}", &[("a", "{b}"), ("b", "x")]), "{b} and x");
        assert_eq!(pseudo("Reply {name}"), "Réply {name}");
        assert_eq!(Lang::from_code("PSEUDO"), Some(Lang::Pseudo));
        assert_eq!(Lang::from_code("klingon"), None);
        assert_eq!(location("Home › Respond"), "Home › Respond");
    }

    #[test]
    fn settings_resolve_against_the_system() {
        // Following the system.
        assert_eq!(resolve("auto", &tag("de_AT.UTF-8")), (Lang::German, tag("de-AT")));
        assert_eq!(resolve("auto", &tag("pt-PT")), (Lang::PortugueseBr, tag("pt-PT")));
        assert_eq!(resolve("", &tag("he-IL")), (Lang::Hebrew, tag("he-IL")));
        // A language without a catalog: English, with the system's dates.
        assert_eq!(resolve("auto", &tag("nl-NL")), (Lang::English, tag("nl-NL")));
        assert_eq!(resolve("auto", &Tag::default()), (Lang::English, tag("en-US")));
        // Named: the system's region when it is the same language, else the language's home.
        assert_eq!(resolve("en", &tag("en-GB")), (Lang::English, tag("en-GB")));
        assert_eq!(resolve("fr", &tag("en-GB")), (Lang::French, tag("fr-FR")));
        assert_eq!(resolve("pt-BR", &tag("pt-PT")), (Lang::PortugueseBr, tag("pt-BR")));
        assert_eq!(resolve("es-MX", &tag("en-US")), (Lang::Spanish, tag("es-MX")));
        // Unknown: as `auto`.
        assert_eq!(resolve("tlh", &tag("fr-CA")), (Lang::French, tag("fr-CA")));
    }

    #[test]
    fn translations_and_fallback() {
        set_current(Lang::German);
        assert_ne!(t("Reply All"), "Reply All");
        // Not in any catalog: the English.
        assert_eq!(t("Zzyzx, not a string of the interface"), "Zzyzx, not a string of the interface");
        assert!(!has("Zzyzx, not a string of the interface"));
        assert_ne!(location("Home › Respond"), "Home › Respond");
        set_current(Lang::English);
        assert_eq!(t("Reply All"), "Reply All");
    }

    #[test]
    fn the_home_tab_is_not_a_contact_s_home() {
        // "Home" is a ribbon tab and a contact's home phone or address; each has its own word.
        set_current(Lang::German);
        assert_eq!(tab("Home"), "Start");
        assert!(location("Home › Respond").starts_with("Start"), "{}", location("Home › Respond"));
        assert_eq!(t("Home"), "Privat");
        // A tab with no word of its own is translated as usual.
        assert_eq!(tab("View"), t("View"));
        for lang in Lang::TRANSLATED {
            set_current(lang);
            assert_ne!(tab("Home"), "Home", "{lang:?} has a word for the Home tab");
        }
        set_current(Lang::English);
        assert_eq!(tab("Home"), "Home");
    }

    #[test]
    fn right_to_left_text_is_handed_out_in_drawn_order() {
        set_current(Lang::Hebrew);
        assert!(rtl());
        let shown_text = t("Reply All");
        assert_ne!(shown_text, "Reply All", "the Hebrew catalog translates Reply All");
        // Two Hebrew words come out with their order turned round (each word in written order,
        // for egui's shaper to turn round).
        let written_text = loaded(Lang::Hebrew).catalog.entries.get("Reply All").cloned().unwrap_or_default();
        if written_text.contains(' ') {
            let words: Vec<&str> = written_text.split(' ').collect();
            let reversed: Vec<&str> = words.iter().rev().copied().collect();
            assert_eq!(shown_text, reversed.join(" "));
        }
        // Filled templates are ordered once, with drawn values put back first.
        let line = fmt("{a} {b}", &[("a", "אב"), ("b", "גד")]);
        assert_eq!(line, "גד אב");
        set_current(Lang::English);
        assert!(!rtl());
    }

    #[test]
    fn dates_in_the_interface_language() {
        set_current(Lang::English);
        let day = Date::new(2026, 10, 11).unwrap_or_default();
        assert_eq!(date_long(day), "Sunday, October 11, 2026");
        assert_eq!(date_medium(day), "Oct 11, 2026");
        assert_eq!(duration(90), "1.5 hours");
        assert_eq!(duration(1), "1 minute");
        assert_eq!(duration(125), "2 h 5 min");
        assert_eq!(duration(3 * 24 * 60), "3 days");
        set_current(Lang::German);
        assert_eq!(date_numeric(day), "11.10.2026");
        assert!(date_long(day).starts_with("Sonntag, 11. Oktober 2026"), "{}", date_long(day));
        assert_eq!(duration(90), "1,5 Stunden");
        set_current(Lang::English);
        // `apply` follows a setting and says when it changed.
        assert!(apply("fr"));
        assert!(!apply("fr"));
        assert_eq!(current(), Lang::French);
        assert!(!formats().twelve_hour);
        set_current(Lang::English);
    }
}
