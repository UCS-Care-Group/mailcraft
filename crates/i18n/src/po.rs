// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! gettext `.po` files: a reader of our own (the part of the format translators' tools write:
//! `msgctxt`, `msgid`, `msgid_plural`, `msgstr`, `msgstr[n]`, the comments and the `fuzzy` flag,
//! C escapes, strings over several lines, obsolete `#~` entries) and the writer `cargo xtask i18n`
//! uses. A file is treated as hostile: whatever it holds, reading it never panics, and sizes are
//! capped. What can't be read is skipped and reported as a [`Problem`].

use std::fmt::Write;

/// Bytes of a file read; anything after is left out (and reported).
pub const MAX_INPUT: usize = 8 << 20;
/// Entries read from one file.
pub const MAX_ENTRIES: usize = 50_000;
/// Characters in one string (a `msgid`, a `msgstr`…), however many lines it spans.
pub const MAX_STRING: usize = 64 << 10;
/// Plural forms a `msgstr[n]` may name (`n` below this).
pub const MAX_FORMS: usize = 8;
/// Problems reported for one file.
pub const MAX_PROBLEMS: usize = 200;

/// One entry of a catalogue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// `# …`: the translators' own comments.
    pub comments: Vec<String>,
    /// `#. …`: comments for translators from the program.
    pub extracted: Vec<String>,
    /// `#: file:line`: where the text is used.
    pub references: Vec<String>,
    /// `#, fuzzy, …`: the flags.
    pub flags: Vec<String>,
    /// `#| …`: what the English was before (kept as written).
    pub previous: Vec<String>,
    pub context: Option<String>,
    pub id: String,
    pub id_plural: Option<String>,
    /// `msgstr`, or `msgstr[0]`, `msgstr[1]`… for a plural.
    pub strs: Vec<String>,
    /// `#~`: an entry no longer used, kept for the translators.
    pub obsolete: bool,
    /// The line its `msgid` is on (1-based; 0 for an entry made in code).
    pub line: usize,
}

impl Entry {
    /// Marked `fuzzy`: a draft nobody has reviewed (still used).
    pub fn fuzzy(&self) -> bool {
        self.flags.iter().any(|f| f == "fuzzy")
    }

    /// The header (`msgid ""`), which holds the catalogue's settings.
    pub fn is_header(&self) -> bool {
        self.id.is_empty() && self.context.is_none()
    }

    /// Translated: every form it has is filled in.
    pub fn translated(&self) -> bool {
        !self.strs.is_empty() && self.strs.iter().all(|s| !s.is_empty())
    }

    /// The key a catalogue looks it up by: the context, then `\u{4}`, then the English (as gettext
    /// does).
    pub fn key(&self) -> String {
        key(self.context.as_deref(), &self.id)
    }

    /// The value of the header field `name` (`Plural-Forms`), when this is the header.
    pub fn header_field(&self, name: &str) -> Option<&str> {
        let text = self.strs.first()?;
        text.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then_some(v.trim())
        })
    }
}

/// The lookup key of `id` in `context`.
pub fn key(context: Option<&str>, id: &str) -> String {
    match context {
        Some(c) => format!("{c}\u{4}{id}"),
        None => id.to_string(),
    }
}

/// Something in a file that couldn't be read as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// 1-based.
    pub line: usize,
    pub what: String,
}

/// A file read: its entries (the header first, when it has one) and what was wrong with it.
#[derive(Clone, Debug, Default)]
pub struct File {
    pub entries: Vec<Entry>,
    pub problems: Vec<Problem>,
}

impl File {
    pub fn header(&self) -> Option<&Entry> {
        self.entries.iter().find(|e| e.is_header() && !e.obsolete)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Context,
    Id,
    IdPlural,
    Str(usize),
}

struct Reader {
    file: File,
    cur: Entry,
    /// The current entry has a keyword line (it isn't only comments).
    started: bool,
    field: Option<Field>,
    has_str: bool,
    has_id: bool,
}

impl Reader {
    fn problem(&mut self, line: usize, what: impl Into<String>) {
        if self.file.problems.len() < MAX_PROBLEMS {
            self.file.problems.push(Problem { line, what: what.into() });
        }
    }

    /// End the entry being read.
    fn finish(&mut self, line: usize) {
        let entry = std::mem::take(&mut self.cur);
        let (started, has_id, has_str) = (self.started, self.has_id, self.has_str);
        self.started = false;
        self.has_id = false;
        self.has_str = false;
        self.field = None;
        if !started {
            return;
        }
        if !has_id {
            self.problem(line, "an entry without a msgid");
            return;
        }
        if !has_str {
            self.problem(entry.line, "an entry without a msgstr");
            return;
        }
        if entry.id_plural.is_some() && entry.strs.is_empty() {
            self.problem(entry.line, "a plural entry without msgstr[0]");
            return;
        }
        if self.file.entries.len() >= MAX_ENTRIES {
            self.problem(entry.line, format!("more than {MAX_ENTRIES} entries: the rest is left out"));
            return;
        }
        self.file.entries.push(entry);
    }

    fn append(&mut self, line: usize, text: String) {
        let Some(field) = self.field else {
            self.problem(line, "a string that belongs to no keyword");
            return;
        };
        let target = match field {
            Field::Context => self.cur.context.get_or_insert_with(String::new),
            Field::Id => &mut self.cur.id,
            Field::IdPlural => self.cur.id_plural.get_or_insert_with(String::new),
            Field::Str(n) => match self.cur.strs.get_mut(n) {
                Some(s) => s,
                None => return,
            },
        };
        let had = target.chars().count();
        if had.saturating_add(text.chars().count()) > MAX_STRING {
            target.extend(text.chars().take(MAX_STRING.saturating_sub(had)));
            self.problem(line, format!("a string longer than {MAX_STRING} characters: cut short"));
        } else {
            target.push_str(&text);
        }
    }

    fn keyword(&mut self, line: usize, rest: &str, obsolete: bool) {
        let (word, value) = match rest.find(|c: char| c.is_whitespace() || c == '"') {
            Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("").trim_start()),
            None => (rest, ""),
        };
        let field = match word {
            "msgctxt" => Field::Context,
            "msgid" => Field::Id,
            "msgid_plural" => Field::IdPlural,
            "msgstr" => Field::Str(0),
            _ => match word.strip_prefix("msgstr[").and_then(|w| w.strip_suffix(']')).map(|n| n.trim().parse::<usize>()) {
                Some(Ok(n)) if n < MAX_FORMS => Field::Str(n),
                Some(_) => {
                    self.problem(line, format!("a plural form this reader doesn't take: {}", clip(word)));
                    self.field = None;
                    return;
                }
                None => {
                    self.problem(line, format!("not a keyword of the format: {}", clip(word)));
                    self.field = None;
                    return;
                }
            },
        };
        // A msgctxt or msgid after a msgstr starts the next entry (entries needn't be separated
        // by blank lines).
        if matches!(field, Field::Context | Field::Id) && self.has_str {
            self.finish(line);
        }
        if !self.started {
            self.started = true;
            self.cur.obsolete = obsolete;
        }
        match field {
            Field::Context if self.has_id => {
                self.problem(line, "msgctxt after msgid");
                self.field = None;
                return;
            }
            Field::Id if self.has_id => {
                self.problem(line, "a second msgid in one entry");
                self.field = None;
                return;
            }
            Field::Id => {
                self.has_id = true;
                self.cur.line = line;
            }
            Field::IdPlural | Field::Str(_) if !self.has_id => {
                self.problem(line, "a msgid_plural or msgstr before the msgid");
                self.field = None;
                return;
            }
            Field::Str(n) => {
                self.has_str = true;
                if self.cur.strs.len() <= n {
                    self.cur.strs.resize(n + 1, String::new());
                }
            }
            _ => {}
        }
        self.field = Some(field);
        match unquote(value) {
            Ok(text) => self.append(line, text),
            Err(e) => self.problem(line, e),
        }
    }
}

fn clip(s: &str) -> String {
    s.chars().take(40).collect()
}

/// Read a `.po` (or `.pot`) file.
pub fn parse(text: &str) -> File {
    let mut r = Reader { file: File::default(), cur: Entry::default(), started: false, field: None, has_str: false, has_id: false };
    let text = if text.len() > MAX_INPUT {
        r.problem(0, format!("longer than {MAX_INPUT} bytes: the rest is left out"));
        let mut end = MAX_INPUT;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text.get(..end).unwrap_or("")
    } else {
        text
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut last = 0;
    for (i, raw) in text.lines().enumerate() {
        let n = i.saturating_add(1);
        last = n;
        let line = raw.trim();
        if line.is_empty() {
            r.finish(n);
            continue;
        }
        let (line, obsolete) = match line.strip_prefix("#~") {
            Some(rest) => (rest.trim_start(), true),
            None => (line, false),
        };
        if obsolete && line.starts_with('|') {
            // `#~|`: the previous English of an obsolete entry; not needed.
            continue;
        }
        if let Some(rest) = line.strip_prefix('#').filter(|_| !obsolete) {
            // A comment after a keyword line starts the next entry.
            if r.has_str {
                r.finish(n);
            }
            let (kind, body) = match rest.chars().next() {
                Some(c @ ('.' | ':' | ',' | '|')) => (Some(c), rest.get(1..).unwrap_or("").trim()),
                _ => (None, rest.trim()),
            };
            match kind {
                Some('.') => r.cur.extracted.push(clip_long(body)),
                Some(':') => r.cur.references.extend(body.split_whitespace().take(64).map(str::to_string)),
                Some(',') => r.cur.flags.extend(body.split(',').map(str::trim).filter(|f| !f.is_empty()).take(16).map(str::to_string)),
                Some('|') => r.cur.previous.push(clip_long(body)),
                _ => r.cur.comments.push(clip_long(body)),
            }
            // Comments don't make an entry by themselves, but are capped all the same.
            if r.cur.comments.len() + r.cur.extracted.len() + r.cur.previous.len() > 256 {
                r.problem(n, "too many comment lines in one entry");
                r.cur.comments.truncate(64);
                r.cur.extracted.truncate(64);
                r.cur.previous.truncate(64);
            }
            continue;
        }
        if line.starts_with('"') {
            match unquote(line) {
                Ok(text) => r.append(n, text),
                Err(e) => r.problem(n, e),
            }
            continue;
        }
        r.keyword(n, line, obsolete);
    }
    r.finish(last.saturating_add(1));
    r.file
}

fn clip_long(s: &str) -> String {
    s.chars().take(1024).collect()
}

/// The text of one quoted string (`"…"`, with C escapes), nothing but blanks after it.
fn unquote(s: &str) -> Result<String, String> {
    let mut chars = s.chars();
    if chars.next() != Some('"') {
        return Err("a string that doesn't start with a quotation mark".into());
    }
    let mut out = String::new();
    loop {
        let Some(c) = chars.next() else {
            return Err("a string that doesn't end".into());
        };
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('a') => out.push('\u{7}'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('v') => out.push('\u{b}'),
                Some(c @ ('"' | '\\' | '\'' | '?')) => out.push(c),
                Some(d @ '0'..='7') => {
                    let mut v = d.to_digit(8).unwrap_or(0);
                    for _ in 0..2 {
                        match chars.clone().next().and_then(|c| c.to_digit(8)) {
                            Some(x) => {
                                v = v * 8 + x;
                                chars.next();
                            }
                            None => break,
                        }
                    }
                    out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                }
                Some('x') => {
                    let mut v: u32 = 0;
                    let mut digits = 0;
                    while digits < 2 {
                        match chars.clone().next().and_then(|c| c.to_digit(16)) {
                            Some(x) => {
                                v = v * 16 + x;
                                chars.next();
                                digits += 1;
                            }
                            None => break,
                        }
                    }
                    if digits == 0 {
                        return Err("\\x without a hexadecimal digit".into());
                    }
                    out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                }
                Some(other) => return Err(format!("an escape this reader doesn't know: \\{other}")),
                None => return Err("a string that doesn't end".into()),
            },
            c => out.push(c),
        }
        if out.len() > MAX_STRING.saturating_mul(4) {
            return Err("a string far too long".into());
        }
    }
    if !chars.as_str().trim().is_empty() {
        return Err("something after a string's closing quotation mark".into());
    }
    Ok(out)
}

/// `s` quoted for a `.po` file.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\{:03o}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One keyword and its string: on one line, or, when the text holds line breaks, split after each
/// as translators' tools do.
fn field(out: &mut String, prefix: &str, keyword: &str, s: &str) {
    let pieces: Vec<&str> = s.split_inclusive('\n').collect();
    if pieces.len() > 1 {
        let _ = writeln!(out, "{prefix}{keyword} \"\"");
        for p in pieces {
            let _ = writeln!(out, "{prefix}{}", quote(p));
        }
    } else {
        let _ = writeln!(out, "{prefix}{keyword} {}", quote(s));
    }
}

/// Write `entry` (and the blank line before the next).
pub fn write_entry(out: &mut String, entry: &Entry) {
    for c in &entry.comments {
        let _ = writeln!(out, "#{}{c}", if c.is_empty() { "" } else { " " });
    }
    for c in &entry.extracted {
        let _ = writeln!(out, "#. {c}");
    }
    for r in &entry.references {
        let _ = writeln!(out, "#: {r}");
    }
    if !entry.flags.is_empty() {
        let _ = writeln!(out, "#, {}", entry.flags.join(", "));
    }
    let prefix = if entry.obsolete { "#~ " } else { "" };
    if !entry.obsolete {
        for p in &entry.previous {
            let _ = writeln!(out, "#| {p}");
        }
    }
    if let Some(c) = &entry.context {
        field(out, prefix, "msgctxt", c);
    }
    field(out, prefix, "msgid", &entry.id);
    match &entry.id_plural {
        Some(plural) => {
            field(out, prefix, "msgid_plural", plural);
            let forms = entry.strs.len().max(1);
            for i in 0..forms {
                field(out, prefix, &format!("msgstr[{i}]"), entry.strs.get(i).map_or("", String::as_str));
            }
        }
        None => field(out, prefix, "msgstr", entry.strs.first().map_or("", String::as_str)),
    }
    out.push('\n');
}

/// A whole file.
pub fn write(entries: &[Entry]) -> String {
    let mut out = String::new();
    for e in entries {
        write_entry(&mut out, e);
    }
    out
}

/// The `{name}` placeholders in `s`, sorted, each once.
pub fn placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('{') {
        let after = rest.get(start + 1..).unwrap_or("");
        match after.find('}') {
            Some(end) => {
                let name = after.get(..end).unwrap_or("");
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.push(name.to_string());
                }
                rest = after.get(end + 1..).unwrap_or("");
            }
            None => break,
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# French translation.
msgid ""
msgstr ""
"Language: fr\n"
"Plural-Forms: nplurals=2; plural=(n > 1);\n"

#. On the ribbon.
#: crates/ui-egui/src/ribbon.rs:10
#, fuzzy
msgid "Reply"
msgstr "Répondre"

msgctxt "verb"
msgid "Open"
msgstr "Ouvrir"
msgid "{n} message"
msgid_plural "{n} messages"
msgstr[0] "{n} message"
msgstr[1] "{n} messages"

msgid ""
"Two\n"
"lines \"quoted\" \\ \t\101"
msgstr "Deux\nlignes"

#~ msgid "Gone"
#~ msgstr "Parti"
"#;

    #[test]
    fn reads_what_translators_tools_write() {
        let f = parse(SAMPLE);
        assert!(f.problems.is_empty(), "{:?}", f.problems);
        assert_eq!(f.entries.len(), 6);
        let h = f.header().unwrap();
        assert_eq!(h.header_field("plural-forms"), Some("nplurals=2; plural=(n > 1);"));
        let reply = &f.entries[1];
        assert!(reply.fuzzy() && reply.translated());
        assert_eq!(reply.references, ["crates/ui-egui/src/ribbon.rs:10"]);
        assert_eq!(reply.extracted, ["On the ribbon."]);
        assert_eq!(f.entries[2].key(), "verb\u{4}Open");
        assert_eq!(f.entries[3].strs.len(), 2);
        assert_eq!(f.entries[4].id, "Two\nlines \"quoted\" \\ \tA");
        assert!(f.entries[5].obsolete && f.entries[5].id == "Gone");
    }

    #[test]
    fn what_is_wrong_is_reported_and_skipped() {
        let f = parse("msgid \"a\"\nmsgstr \"b\" junk\nmsgstr[99] \"x\"\nwhat\n\nmsgstr \"orphan\"\n\nmsgid \"ok\"\nmsgstr \"fine\"\n");
        assert!(f.problems.len() >= 3, "{:?}", f.problems);
        assert!(f.entries.iter().any(|e| e.id == "ok" && e.strs == ["fine"]));
        assert!(!parse("msgid \"unterminated\nmsgstr \"\"\n").problems.is_empty());
    }

    #[test]
    fn writes_what_it_reads() {
        let f = parse(SAMPLE);
        let again = parse(&write(&f.entries));
        assert!(again.problems.is_empty(), "{:?}", again.problems);
        let strip = |es: &[Entry]| es.iter().map(|e| Entry { line: 0, ..e.clone() }).collect::<Vec<_>>();
        assert_eq!(strip(&again.entries), strip(&f.entries));
    }

    #[test]
    fn placeholders_are_found() {
        assert_eq!(placeholders("{n} of {total} {n} {not one} {}"), ["n", "total"]);
        assert!(placeholders("no { brace").is_empty());
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2048)) {
            let text = String::from_utf8_lossy(&bytes);
            let f = parse(&text);
            let _ = write(&f.entries);
            proptest::prop_assert!(f.problems.len() <= MAX_PROBLEMS);
        }

        #[test]
        fn po_shaped_text_never_panics(lines in proptest::collection::vec("(msgid|msgstr|msgstr\\[[0-9]{1,3}\\]|msgctxt|msgid_plural|#,|#~|#|\"|)[ ]?\"?[a-z\\\\\"{}\\n]{0,8}\"?", 0..40)) {
            let text = lines.join("\n");
            let f = parse(&text);
            for e in &f.entries {
                proptest::prop_assert!(e.strs.len() <= MAX_FORMS);
            }
        }

        #[test]
        fn strings_survive_a_round_trip(id in "\\PC{0,40}", s in "[\\PC\\n\\t\"\\\\]{0,40}") {
            let e = Entry { id: id.clone(), strs: vec![s.clone()], ..Entry::default() };
            let f = parse(&write(std::slice::from_ref(&e)));
            proptest::prop_assert_eq!(f.entries.len(), 1);
            proptest::prop_assert_eq!(&f.entries[0].id, &id);
            proptest::prop_assert_eq!(&f.entries[0].strs[0], &s);
        }
    }
}
