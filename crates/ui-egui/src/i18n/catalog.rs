//! Message catalogs: the interface text of one language, as a plain text file of pairs.
//!
//! ```text
//! # A comment.
//! @language de
//! @name Deutsch
//! @direction ltr
//!
//! Reply All
//! = Allen antworten
//! ```
//!
//! A pair is a line of English (the text exactly as the program has it in `tl!`) and, on the next
//! line, `= ` and its translation. `\n`, `\t` and `\\` stand for a line break, a tab and a
//! backslash on either line; a translation left empty (`=` alone) is not translated yet, and the
//! English is shown. Blank lines and lines starting with `#` are ignored; `@` lines say what the
//! file is. A catalog comes from a file that anybody can edit, so nothing in it is trusted:
//! lines are capped, a translation whose `{placeholders}` aren't those of its English is left
//! out (the English is shown), and anything not understood is reported and skipped, never a
//! panic.

use std::collections::{BTreeSet, HashMap};

/// The most bytes of one catalog file that are read.
pub const MAX_FILE: usize = 4 * 1024 * 1024;
/// The longest line taken, in bytes.
pub const MAX_LINE: usize = 8 * 1024;
/// The most pairs one catalog holds.
pub const MAX_ENTRIES: usize = 20_000;

/// One language's interface text.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    /// What the `@language` line says (`de`, `pt-BR`).
    pub language: String,
    /// What the language calls itself (`Deutsch`).
    pub name: String,
    /// Written right to left (`@direction rtl`).
    pub rtl: bool,
    /// English → translation, for the pairs that passed the checks.
    pub entries: HashMap<String, String>,
    /// Pairs whose translation is still empty.
    pub untranslated: Vec<String>,
}

/// Something in a catalog file that was not taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The line it is on, from 1.
    pub line: usize,
    pub kind: ProblemKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// The file is longer than [`MAX_FILE`]; the rest was not read.
    TooLong,
    /// A line longer than [`MAX_LINE`].
    LongLine,
    /// A `= ` line with no English before it.
    Orphan,
    /// English with no `= ` line after it.
    NoTranslation(String),
    /// The same English twice; the first is kept.
    Duplicate(String),
    /// The translation's placeholders aren't those of the English.
    Placeholders { key: String, missing: Vec<String>, invented: Vec<String> },
    /// An `@` line that isn't one of `language`, `name`, `direction`.
    UnknownHeader(String),
    /// More than [`MAX_ENTRIES`] pairs; the rest were not read.
    TooMany,
}

/// The `{name}` placeholders in a text: a brace, a letter or `_`, letters, digits and `_`, a
/// brace. (Other braces are text.)
pub fn placeholders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = rest.get(open + 1..).unwrap_or("");
        let name_len = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len());
        let name = after.get(..name_len).unwrap_or("");
        let starts_well = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if starts_well && after.get(name_len..).is_some_and(|r| r.starts_with('}')) {
            out.insert(name.to_string());
            rest = after.get(name_len + 1..).unwrap_or("");
        } else {
            rest = after;
        }
    }
    out
}

/// What `translation` drops of `english`'s placeholders, and what it invents.
pub fn placeholder_mismatch(english: &str, translation: &str) -> Option<(Vec<String>, Vec<String>)> {
    let (want, have) = (placeholders(english), placeholders(translation));
    if want == have {
        return None;
    }
    Some((want.difference(&have).cloned().collect(), have.difference(&want).cloned().collect()))
}

/// A line's text with its escapes undone.
pub fn unescape(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(c @ ('=' | '#' | '@')) => out.push(c),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A text as a catalog line writes it (a line of English that would start with `=`, `#` or `@`
/// starts with a `\\` before it).
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1);
    if text.starts_with(['=', '#', '@']) {
        out.push('\\');
    }
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Read a catalog file. What can't be taken is in the problems; the rest is in the catalog.
pub fn parse(text: &str) -> (Catalog, Vec<Problem>) {
    let mut catalog = Catalog::default();
    let mut problems = Vec::new();
    let text = if text.len() > MAX_FILE {
        problems.push(Problem { line: 0, kind: ProblemKind::TooLong });
        let mut end = MAX_FILE;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text.get(..end).unwrap_or("")
    } else {
        text
    };
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    // The English waiting for its translation, and its line.
    let mut pending: Option<(String, usize)> = None;
    let mut count = 0usize;
    for (index, raw) in text.lines().enumerate() {
        let line_no = index.saturating_add(1);
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.len() > MAX_LINE {
            problems.push(Problem { line: line_no, kind: ProblemKind::LongLine });
            pending = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix('=') {
            let Some((key, _)) = pending.take() else {
                problems.push(Problem { line: line_no, kind: ProblemKind::Orphan });
                continue;
            };
            let value = unescape(rest.strip_prefix(' ').unwrap_or(rest));
            if count >= MAX_ENTRIES {
                problems.push(Problem { line: line_no, kind: ProblemKind::TooMany });
                break;
            }
            count += 1;
            if catalog.entries.contains_key(&key) || catalog.untranslated.contains(&key) {
                problems.push(Problem { line: line_no, kind: ProblemKind::Duplicate(key) });
                continue;
            }
            if value.trim().is_empty() {
                catalog.untranslated.push(key);
                continue;
            }
            if let Some((missing, invented)) = placeholder_mismatch(&key, &value) {
                problems.push(Problem { line: line_no, kind: ProblemKind::Placeholders { key, missing, invented } });
                continue;
            }
            catalog.entries.insert(key, value);
            continue;
        }
        if let Some((key, at)) = pending.take() {
            problems.push(Problem { line: at, kind: ProblemKind::NoTranslation(key) });
        }
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('@') {
            let (name, value) = header.split_once(char::is_whitespace).unwrap_or((header, ""));
            let value = value.trim();
            match name {
                "language" => catalog.language = value.chars().take(32).collect(),
                "name" => catalog.name = value.chars().take(64).collect(),
                "direction" => catalog.rtl = value.eq_ignore_ascii_case("rtl"),
                other => problems.push(Problem { line: line_no, kind: ProblemKind::UnknownHeader(other.chars().take(64).collect()) }),
            }
            continue;
        }
        pending = Some((unescape(line), line_no));
    }
    if let Some((key, at)) = pending {
        problems.push(Problem { line: at, kind: ProblemKind::NoTranslation(key) });
    }
    (catalog, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const SAMPLE: &str = "# German\n@language de\n@name Deutsch\n\nReply All\n= Allen antworten\n\n{n} unread\n= {n} ungelesen\n\nNew Folder\n=\n";

    #[test]
    fn pairs_headers_and_untranslated() {
        let (c, problems) = parse(SAMPLE);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!((c.language.as_str(), c.name.as_str(), c.rtl), ("de", "Deutsch", false));
        assert_eq!(c.entries.get("Reply All").map(String::as_str), Some("Allen antworten"));
        assert_eq!(c.entries.get("{n} unread").map(String::as_str), Some("{n} ungelesen"));
        assert_eq!(c.untranslated, vec!["New Folder".to_string()]);
    }

    #[test]
    fn placeholders_must_match() {
        let (c, problems) = parse("{n} of {total}\n= {n} von {gesamt}\n\nHi {name}\n= Hallo\n");
        assert!(c.entries.is_empty());
        assert_eq!(problems.len(), 2);
        assert_eq!(
            problems.first().map(|p| &p.kind),
            Some(&ProblemKind::Placeholders { key: "{n} of {total}".into(), missing: vec!["total".into()], invented: vec!["gesamt".into()] })
        );
        // Braces that aren't a placeholder are text.
        assert!(placeholders("a list of {path, kind} and {} and {1x}").is_empty());
        assert_eq!(placeholders("{a}{b_2} {a}").into_iter().collect::<Vec<_>>(), vec!["a".to_string(), "b_2".into()]);
    }

    #[test]
    fn escapes_round_trip_and_problems_are_reported() {
        assert_eq!(unescape(r"one\ntwo\\three\t\q\"), "one\ntwo\\three\t\\q\\");
        for s in ["a\nb", "back\\slash", "tab\there", "plain", "# not a comment", "=x"] {
            assert_eq!(unescape(&escape(s)), s);
        }
        let (c, problems) = parse("= orphan\nKey\nKey2\n= v2\nKey2\n= again\n@colour red\nLast");
        assert_eq!(c.entries.len(), 1);
        let kinds: Vec<_> = problems.into_iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ProblemKind::Orphan,
                ProblemKind::NoTranslation("Key".into()),
                ProblemKind::Duplicate("Key2".into()),
                ProblemKind::UnknownHeader("colour".into()),
                ProblemKind::NoTranslation("Last".into()),
            ]
        );
    }

    #[test]
    fn crlf_bom_and_rtl() {
        let (c, problems) = parse("\u{FEFF}@direction RTL\r\nInbox\r\n= דואר נכנס\r\n");
        assert!(problems.is_empty());
        assert!(c.rtl);
        assert_eq!(c.entries.get("Inbox").map(String::as_str), Some("דואר נכנס"));
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let text = String::from_utf8_lossy(&bytes);
            let (catalog, _) = parse(&text);
            for (k, v) in &catalog.entries {
                prop_assert!(placeholder_mismatch(k, v).is_none());
            }
        }

        #[test]
        fn arbitrary_lines_never_panic(lines in proptest::collection::vec("[=@#{}a-z\\\\ \\n\u{05D0}-\u{05EA}]{0,40}", 0..60)) {
            let _ = parse(&lines.join("\n"));
        }
    }
}
