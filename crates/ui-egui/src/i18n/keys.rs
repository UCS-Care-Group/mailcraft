//! What there is to translate, read from the source, and how much of it each catalog has.
//!
//! The keys are the English strings of `tl!("…")` and `tn!("…")` in this crate's source, and the
//! strings that reach `tl!` from elsewhere: the engine's command names, places in the ribbon and
//! the reasons a command can't run, the window's own commands, and the names of months, days,
//! modules, views and the like that the lower crates hand up.
//!
//! `cargo test -p mailcraft-ui-egui --lib i18n::keys` checks every catalog: it fails on a pair
//! the file format doesn't allow, on a translation whose placeholders aren't its English's, and
//! on English that isn't (or is no longer) in the program; it reports, and doesn't fail on, how
//! much of the program each catalog covers. `cargo xtask i18n [lang]` lists what a catalog still
//! lacks, as pairs ready to be filled in.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use super::{Lang, catalog};

/// Files whose strings are tests' own.
fn is_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name == "tests.rs" || name.ends_with("_tests.rs") || path.ends_with("i18n/keys.rs")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") && !is_test_file(&path) {
            out.push(path);
        }
    }
}

/// A Rust string literal starting at `s[0]` (`"…"` or `r#"…"#`): its value and the bytes it takes.
pub(crate) fn literal(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if bytes.first() == Some(&b'r') {
        let hashes = s.get(1..)?.bytes().take_while(|b| *b == b'#').count();
        let open = 1 + hashes;
        if bytes.get(open) != Some(&b'"') {
            return None;
        }
        let close = format!("\"{}", "#".repeat(hashes));
        let body_start = open + 1;
        let end = s.get(body_start..)?.find(&close)?;
        return Some((s.get(body_start..body_start + end)?.to_string(), body_start + end + close.len()));
    }
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, i + 1)),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                'u' => {
                    let mut hex = String::new();
                    for (_, h) in chars.by_ref() {
                        match h {
                            '{' => {}
                            '}' => break,
                            h => hex.push(h),
                        }
                    }
                    out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)?);
                }
                '\n' => {
                    // A line continuation: the break and the next line's leading blanks go.
                    while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                        chars.next();
                    }
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// The literals in `tl!(…)` and `tn!(…)` in one file's text, in order.
pub(crate) fn literals_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["tl!(", "tn!("] {
        let mut from = 0;
        while let Some(at) = text.get(from..).and_then(|r| r.find(marker)) {
            let start = from + at + marker.len();
            from = start;
            // Not part of a longer name (`xtl!(`).
            let before = text.get(..start - marker.len()).and_then(|b| b.chars().next_back());
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            // Not in a comment.
            let line_start = text.get(..start).and_then(|b| b.rfind('\n')).map_or(0, |n| n + 1);
            if text.get(line_start..start).is_some_and(|l| l.contains("//")) {
                continue;
            }
            let rest = text.get(start..).unwrap_or("");
            let trimmed = rest.trim_start();
            if let Some((value, _)) = literal(trimmed) {
                out.push((start, value));
            }
        }
    }
    out.sort_by_key(|(at, _)| *at);
    out.into_iter().map(|(_, v)| v).collect()
}

/// Every English string of the interface: from the source, then the ones handed up from below,
/// first appearance first.
pub fn all() -> Vec<String> {
    with_origin().into_iter().map(|(k, _)| k).collect()
}

/// [`all`], each with the file it is first in (`engine` for those handed up).
pub fn with_origin() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut add = |s: &str, origin: &str, out: &mut Vec<(String, String)>| {
        if !s.trim().is_empty() && seen.insert(s.to_string()) {
            out.push((s.to_string(), origin.to_string()));
        }
    };
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let origin = path.strip_prefix(&root).map(|p| p.display().to_string()).unwrap_or_default();
        for s in literals_in(&text) {
            add(&s, &origin, &mut out);
        }
    }
    for s in handed_up() {
        add(&s, "engine", &mut out);
    }
    out
}

/// The strings that reach `tl!` from the engine and the crates under it.
fn handed_up() -> Vec<String> {
    use mailcraft_engine::{CalView, Filter, Module, Session, SortBy, TaskFilter};
    use mailcraft_model::{OnlineProvider, Response, ShowAs};
    use mailcraft_time::{DateGroup, Weekday};
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| out.push(s.to_string());
    let empty = Session::new(mailcraft_model::Mailbox::default());
    let sample = Session::new(mailcraft_engine::sample::mailbox(1_791_000_000, 0));
    for spec in empty.registry.all() {
        push(spec.label);
        for part in spec.location.split(" › ") {
            push(part);
        }
        for session in [&empty, &sample] {
            if let Some(why) = (spec.enabled)(session) {
                push(why);
            }
        }
    }
    // A tab whose English name means something else elsewhere has a key of its own.
    push(&format!("{}Home", crate::i18n::TAB_CONTEXT));
    for (_, label, location, _) in crate::UI_COMMANDS {
        push(label);
        for part in location.split(" › ") {
            push(part);
        }
    }
    for m in 1..=12 {
        push(mailcraft_time::month_name(m));
        push(mailcraft_time::month_short(m));
    }
    for d in Weekday::ALL {
        push(d.name());
        push(d.short());
    }
    for g in [
        DateGroup::Future,
        DateGroup::Today,
        DateGroup::Yesterday,
        DateGroup::LastWeek,
        DateGroup::TwoWeeksAgo,
        DateGroup::ThreeWeeksAgo,
        DateGroup::LastMonth,
        DateGroup::Older,
    ] {
        push(g.label());
    }
    for m in Module::ALL {
        push(m.label());
    }
    for s in SortBy::ALL {
        push(s.label());
    }
    for f in Filter::ALL {
        push(f.label());
    }
    for v in CalView::ALL {
        push(v.label());
    }
    for f in TaskFilter::ALL {
        push(f.label());
    }
    for s in ShowAs::ALL {
        push(s.label());
    }
    for r in [Response::None, Response::Accepted, Response::Tentative, Response::Declined] {
        push(r.label());
    }
    for p in [OnlineProvider::Teams, OnlineProvider::SkypeForBusiness, OnlineProvider::Skype, OnlineProvider::Other] {
        push(p.label());
    }
    // What account setup says of a provider.
    push(mailcraft_model::provider::microsoft(false).note);
    if let Some(gmail) = mailcraft_model::provider::by_domain("gmail.com") {
        push(gmail.note);
    }
    for calendar in mailcraft_engine::cmd::calendar_settings::CALENDAR_COLORS {
        push(calendar.0);
    }
    // The message list's groups and previews, and the calendar list's groups.
    for s in ["Pinned", "Encrypted message", "Signed message", "High", "Normal", "Low", "On this computer"] {
        push(s);
    }
    push(mailcraft_engine::cmd::calendar_settings::MY_CALENDARS);
    push(mailcraft_engine::cmd::calendar_settings::OTHER_CALENDARS);
    for kind in mailcraft_model::FolderKind::STANDARD {
        push(kind.default_name());
    }
    // What a task's due date is called (`Task::due_label`), and the undo of nothing.
    for s in ["No date", "Tomorrow", "nothing to undo", "nothing to redo"] {
        push(s);
    }
    out
}

/// The catalog file of a language, as it is in the tree.
fn catalog_path(lang: Lang) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/i18n").join(format!("{}.txt", lang.code()))
}

/// How much of `keys` a catalog translates, in percent.
fn coverage(catalog: &catalog::Catalog, keys: &[String]) -> f64 {
    if keys.is_empty() {
        return 100.0;
    }
    let have = keys.iter().filter(|k| catalog.entries.contains_key(*k)).count();
    #[allow(clippy::cast_precision_loss)]
    let pct = have as f64 * 100.0 / keys.len() as f64;
    pct
}

#[test]
fn the_source_is_read() {
    let keys = all();
    for expected in ["Reply All", "Options", "Interface theme", "January", "Mon", "Calendar", "Out of Office", "{n} minutes"] {
        assert!(keys.iter().any(|k| k == expected), "{expected} is not among the keys");
    }
    assert!(keys.len() > 1000, "{} keys", keys.len());
}

#[test]
fn literals_are_read_like_rust() {
    assert_eq!(literal(r#""a\"b\n\u{e9}" rest"#), Some(("a\"b\né".to_string(), 14)));
    assert_eq!(literal("r#\"x \"y\"\"# z"), Some(("x \"y\"".to_string(), 10)));
    assert_eq!(literal("\"one \\\n      two\""), Some(("one two".to_string(), 17)));
    assert_eq!(literal("\"open"), None);
    assert_eq!(literal("x"), None);
    let text = "let a = tl!(\"First\"); let b = tn!(\n  \"Second\"); xtl!(\"no\"); tl!(name);";
    assert_eq!(literals_in(text), vec!["First".to_string(), "Second".into()]);
}

/// Every catalog is well formed, has only English the program has, and says how much it covers.
#[test]
fn catalogs_are_sound_and_report_coverage() {
    let keys = all();
    let known: HashSet<&str> = keys.iter().map(String::as_str).collect();
    let mut report = Vec::new();
    let mut failures = Vec::new();
    for lang in Lang::TRANSLATED {
        let (catalog, problems) = catalog::parse(lang.source());
        for p in problems {
            failures.push(format!("{}.txt line {}: {:?}", lang.code(), p.line, p.kind));
        }
        if catalog.language != lang.code() {
            failures.push(format!("{}.txt says it is `@language {}`", lang.code(), catalog.language));
        }
        if catalog.rtl != lang.rtl() {
            failures.push(format!("{}.txt has the wrong @direction", lang.code()));
        }
        let mut unknown: Vec<&String> = catalog.entries.keys().chain(catalog.untranslated.iter()).filter(|k| !known.contains(k.as_str())).collect();
        unknown.sort();
        for k in unknown {
            failures.push(format!("{}.txt: not a string of the program: {k:?}", lang.code()));
        }
        report.push(format!("{} {:5.1}% ({} of {})", lang.code(), coverage(&catalog, &keys), catalog.entries.len(), keys.len()));
    }
    println!("catalog coverage:\n  {}", report.join("\n  "));
    assert!(failures.is_empty(), "{} problem(s) in the catalogs:\n{}", failures.len(), failures.join("\n"));
}

/// `cargo xtask i18n [lang]`: what a catalog lacks, as pairs to fill in (written to the file
/// `MAILCRAFT_I18N_OUT` names, else printed).
#[test]
#[ignore = "a report, run by `cargo xtask i18n`"]
fn untranslated() {
    let keys = with_origin();
    let wanted = std::env::var("MAILCRAFT_I18N_LANG").unwrap_or_default();
    let mut out = String::new();
    let mut counts = BTreeMap::new();
    for lang in Lang::TRANSLATED {
        if !wanted.is_empty() && !wanted.eq_ignore_ascii_case(lang.code()) {
            continue;
        }
        let text = std::fs::read_to_string(catalog_path(lang)).unwrap_or_default();
        let (catalog, _) = catalog::parse(&text);
        let missing: Vec<&(String, String)> = keys.iter().filter(|(k, _)| !catalog.entries.contains_key(k)).collect();
        counts.insert(lang.code(), missing.len());
        out.push_str(&format!("# {}: {} untranslated\n\n", lang.code(), missing.len()));
        let mut last = "";
        for (k, origin) in missing {
            if origin != last {
                out.push_str(&format!("# {origin}\n"));
                last = origin;
            }
            out.push_str(&catalog::escape(k));
            out.push_str("\n=\n\n");
        }
    }
    match std::env::var("MAILCRAFT_I18N_OUT") {
        Ok(path) if !path.is_empty() => {
            std::fs::write(&path, out).unwrap();
            println!("untranslated: {counts:?}, written to {path}");
        }
        _ => println!("{out}"),
    }
}
