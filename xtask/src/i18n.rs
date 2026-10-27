// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask i18n [--check]`: the interface's English text, gathered from the source into
//! `crates/i18n/po/messages.pot`, and merged into each language's catalogue (`fr.po`, …).
//!
//! What is gathered: every literal in `tl!("…")` and `tln!(n, "…", "…")` in the window
//! (`crates/ui-egui`); a literal passed where a function of the window puts that parameter through
//! `tl!` (`mi(ui, app, "New Message", …)`); the window's own commands (`UI_COMMANDS`); the engine's
//! command labels, the parts of their ribbon locations and the reasons a command can't run; the
//! names the window shows through `tl!(x.label())` (`fn label`/`name`/`title`… returning a
//! `&'static str`); the words and patterns of dates and times (`crates/i18n/src/date.rs`); and
//! what the engine and the model say to the owner ([`engine_says`]): why a command failed
//! (`fail("…")`, `failed!("… {name} …")`), the words of a synchronisation (`lang.t("…")`), and
//! the reasons put into those.
//!
//! `--check` (a `cargo xtask ci` step) changes nothing and fails when the template or a catalogue
//! is out of date. Either way, it fails on a catalogue it can't read, or on a translation whose
//! `{placeholders}` aren't the English's.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mailcraft_i18n::plural::Rule;
use mailcraft_i18n::po::{self, Entry};

/// Functions whose `&'static str` is a name the window shows through `tl!`.
const NAME_FNS: &[&str] = &["label", "title", "detail", "about", "default_name", "notice", "said", "what", "unit"];

/// Window constants that reach `tl!` through a variable (`tl!(error)`), not by name.
const SHOWN_CONSTS: &[&str] = &["NEEDS_APPROVAL", "REFUSED_BY_ORGANISATION", "REFUSED_FOR_ACCOUNT", "FAVORITES_NOW_GRAPH"];

pub fn run(root: &Path, check: bool) -> Result<(), String> {
    let found = gather(root)?;
    let pot = template(&found);
    let dir = root.join("crates/i18n/po");
    let pot_path = dir.join("messages.pot");
    let mut stale = Vec::new();
    if std::fs::read_to_string(&pot_path).ok().as_deref() != Some(pot.as_str()) {
        stale.push(pot_path.clone());
        if !check {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            std::fs::write(&pot_path, &pot).map_err(|e| format!("{}: {e}", pot_path.display()))?;
        }
    }
    println!("{} texts in {}", found.len(), rel(root, &pot_path));
    let mut problems = Vec::new();
    for path in catalogues(&dir)? {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file = po::parse(&text);
        for p in &file.problems {
            problems.push(format!("{}:{}: {}", rel(root, &path), p.line, p.what));
        }
        let merged = merge(&file.entries, &found);
        for e in merged.iter().filter(|e| !e.obsolete && !e.is_header()) {
            if let Some(why) = mismatch(e) {
                problems.push(format!("{}:{}: {why}", rel(root, &path), e.line));
            }
        }
        let (translated, fuzzy) = coverage(&merged);
        let total = found.len().max(1);
        println!(
            "{}: {translated} of {} translated ({}%), {fuzzy} of them fuzzy",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("?"),
            found.len(),
            translated * 100 / total
        );
        let out = po::write(&merged);
        if out != text {
            stale.push(path.clone());
            if !check {
                std::fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
    }
    if !problems.is_empty() {
        return Err(format!("{} problem(s) in the catalogues:\n  {}", problems.len(), problems.join("\n  ")));
    }
    if check && !stale.is_empty() {
        let names: Vec<String> = stale.iter().map(|p| rel(root, p)).collect();
        return Err(format!("out of date: {} (run `cargo xtask i18n`)", names.join(", ")));
    }
    if !check {
        for p in &stale {
            println!("wrote {}", rel(root, p));
        }
    }
    Ok(())
}

fn rel(root: &Path, p: &Path) -> String {
    // The same template on every system: `/` between path parts, also on Windows.
    p.strip_prefix(root).unwrap_or(p).display().to_string().replace('\\', "/")
}

fn catalogues(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(Vec::new()) };
    let mut out: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()) == Some("po")).collect();
    out.sort();
    Ok(out)
}

/// One English text the interface shows.
#[derive(Default, Debug)]
pub struct Found {
    plural: Option<String>,
    /// Where it is used: files, relative to the root.
    refs: BTreeSet<String>,
    /// What it is, for the translator ("command label", …).
    kinds: BTreeSet<&'static str>,
}

/// By the key a catalogue looks a text up by: its English, after its context and `\u{4}` when it
/// has one ([`po::key`]).
type Texts = BTreeMap<String, Found>;

fn add(texts: &mut Texts, id: &str, plural: Option<&str>, file: &str, kind: &'static str) {
    add_in(texts, None, id, plural, file, kind);
}

/// A text in a context (`msgctxt`): `tlc!("folder", "Archive")`.
fn add_in(texts: &mut Texts, context: Option<&str>, id: &str, plural: Option<&str>, file: &str, kind: &'static str) {
    // Nothing to translate in a text without a letter (`·`, `{n}%`, ``).
    if !id.chars().any(char::is_alphabetic) {
        return;
    }
    let f = texts.entry(po::key(context, id)).or_default();
    if let Some(p) = plural {
        f.plural = Some(p.to_string());
    }
    f.refs.insert(file.to_string());
    f.kinds.insert(kind);
}

fn gather(root: &Path) -> Result<Texts, String> {
    let mut texts = Texts::new();
    // The window: first which functions put a parameter through `tl!`, then everything.
    let ui: Vec<(String, Vec<Tok>)> = sources(root, "crates/ui-egui/src")?;
    let mut through: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for (_, toks) in &ui {
        translating_params(toks, &mut through);
    }
    // `tl!(NAME)` for a `const NAME: &str = "…";` of the window.
    let mut ui_consts = BTreeMap::new();
    for (_, toks) in &ui {
        constants(toks, &mut ui_consts);
    }
    for name in SHOWN_CONSTS {
        if let Some(s) = ui_consts.get(*name) {
            add(&mut texts, s, None, "crates/ui-egui/src", "interface");
        }
    }
    for (file, toks) in &ui {
        literals(toks, file, &through, &ui_consts, &mut texts);
        if file.ends_with("ui-egui/src/lib.rs") {
            window_commands(toks, file, &mut texts);
        }
        if file.ends_with("ui-egui/src/ribbon.rs") {
            ribbon(toks, file, &mut texts);
        }
        names(toks, file, &mut texts);
    }
    // The engine and the model: which of their functions say a parameter, and their constants.
    let mut core = sources(root, "crates/engine/src")?;
    core.extend(sources(root, "crates/model/src")?);
    let mut says: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut consts = BTreeMap::new();
    for (_, toks) in &core {
        translating_params(toks, &mut says);
        constants(toks, &mut consts);
    }
    for (file, toks) in &core {
        if file.starts_with("crates/engine/") {
            commands(toks, file, &mut texts);
            reasons(toks, file, &mut texts);
        }
        names(toks, file, &mut texts);
        engine_says(toks, file, &says, &consts, &mut texts);
    }
    for (file, toks) in sources(root, "crates/time/src")? {
        names(&toks, &file, &mut texts);
    }
    for (file, toks) in sources(root, "crates/i18n/src")? {
        if file.ends_with("/date.rs") {
            dates(&toks, &file, &mut texts);
        } else if file.ends_with("/lib.rs") {
            // The language picker's "(preview)", which each language says in its own words.
            engine_says(&toks, &file, &BTreeMap::new(), &BTreeMap::new(), &mut texts);
        }
    }
    Ok(texts)
}

/// Every `.rs` file under `dir` but tests (`tests.rs`, `*_tests.rs`, `#[cfg(test)] mod tests`),
/// lexed, sorted by path.
fn sources(root: &Path, dir: &str) -> Result<Vec<(String, Vec<Tok>)>, String> {
    let mut files = Vec::new();
    walk(&root.join(dir), &mut files)?;
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "tests.rs" || name.ends_with("_tests.rs") || path.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        out.push((rel(root, &path), without_tests(lex(&text.replace("\r\n", "\n")))));
    }
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    Ok(())
}

// ---- A small Rust lexer: enough to find string literals and what is around them. ----

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Str(String),
    Ident(String),
    Punct(char),
}

fn lex(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
    while i < chars.len() {
        let c = at(i);
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && at(i + 1) == '/' {
            while i < chars.len() && at(i) != '\n' {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == '*' {
            let mut depth = 0usize;
            while i < chars.len() {
                if at(i) == '/' && at(i + 1) == '*' {
                    depth += 1;
                    i += 2;
                } else if at(i) == '*' && at(i + 1) == '/' {
                    depth = depth.saturating_sub(1);
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == '"' {
            let (s, next) = string(&chars, i + 1);
            out.push(Tok::Str(s));
            i = next;
        } else if c == '\'' {
            // A character ('a', '\n', '\u{7f}') or a lifetime ('static).
            if at(i + 1) == '\\' {
                i += 2;
                while i < chars.len() && at(i) != '\'' {
                    i += 1;
                }
                i += 1;
            } else if at(i + 2) == '\'' {
                i += 3;
            } else {
                let start = i;
                i += 1;
                while at(i).is_alphanumeric() || at(i) == '_' {
                    i += 1;
                }
                out.push(Tok::Ident(chars.get(start..i).unwrap_or_default().iter().collect()));
            }
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while at(i).is_alphanumeric() || at(i) == '_' {
                i += 1;
            }
            let word: String = chars.get(start..i).unwrap_or_default().iter().collect();
            if (word == "r" || word == "br") && (at(i) == '"' || at(i) == '#') {
                let mut hashes = 0;
                while at(i) == '#' {
                    hashes += 1;
                    i += 1;
                }
                i += 1; // the opening quote
                let start = i;
                loop {
                    if i >= chars.len() {
                        break;
                    }
                    if at(i) == '"' && (1..=hashes).all(|k| at(i + k) == '#') {
                        break;
                    }
                    i += 1;
                }
                let s: String = chars.get(start..i.min(chars.len())).unwrap_or_default().iter().collect();
                out.push(Tok::Str(s));
                i += 1 + hashes;
            } else if word == "b" && at(i) == '"' {
                let (_, next) = string(&chars, i + 1);
                i = next;
            } else {
                out.push(Tok::Ident(word));
            }
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

/// A string's text from just after its opening quote; and where the lexer goes on.
fn string(chars: &[char], mut i: usize) -> (String, usize) {
    let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
    let mut s = String::new();
    while i < chars.len() {
        match at(i) {
            '"' => return (s, i + 1),
            '\\' => {
                let e = at(i + 1);
                i += 2;
                match e {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    'r' => s.push('\r'),
                    '0' => s.push('\0'),
                    '\n' => {
                        while at(i).is_whitespace() {
                            i += 1;
                        }
                    }
                    'x' => {
                        let hex: String = chars.get(i..i + 2).unwrap_or_default().iter().collect();
                        s.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                        i += 2;
                    }
                    'u' => {
                        let start = i + 1;
                        while i < chars.len() && at(i) != '}' {
                            i += 1;
                        }
                        let hex: String = chars.get(start..i).unwrap_or_default().iter().collect();
                        s.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                        i += 1;
                    }
                    other => s.push(other),
                }
            }
            c => {
                s.push(c);
                i += 1;
            }
        }
    }
    (s, i)
}

/// The tokens without `mod tests { … }`.
fn without_tests(toks: Vec<Tok>) -> Vec<Tok> {
    let mut out = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        if toks.get(i) == Some(&Tok::Ident("mod".into()))
            && toks.get(i + 1) == Some(&Tok::Ident("tests".into()))
            && toks.get(i + 2) == Some(&Tok::Punct('{'))
        {
            i = close(&toks, i + 2) + 1;
            continue;
        }
        if let Some(t) = toks.get(i) {
            out.push(t.clone());
        }
        i += 1;
    }
    out
}

/// The index of the bracket that closes the one at `open`.
fn close(toks: &[Tok], open: usize) -> usize {
    let (o, c) = match toks.get(open) {
        Some(Tok::Punct('(')) => ('(', ')'),
        Some(Tok::Punct('[')) => ('[', ']'),
        _ => ('{', '}'),
    };
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        if *t == Tok::Punct(o) {
            depth += 1;
        } else if *t == Tok::Punct(c) {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return i;
            }
        }
    }
    toks.len()
}

/// The arguments between the bracket at `open` and its partner, split at the commas between them.
fn args(toks: &[Tok], open: usize) -> Vec<&[Tok]> {
    let end = close(toks, open);
    let inner = toks.get(open + 1..end).unwrap_or_default();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, t) in inner.iter().enumerate() {
        match t {
            Tok::Punct('(' | '[' | '{') => depth += 1,
            Tok::Punct(')' | ']' | '}') => depth = depth.saturating_sub(1),
            Tok::Punct(',') if depth == 0 => {
                out.push(inner.get(start..i).unwrap_or_default());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < inner.len() {
        out.push(inner.get(start..).unwrap_or_default());
    }
    out
}

fn lit(arg: &[Tok]) -> Option<&str> {
    match arg {
        [Tok::Str(s)] => Some(s),
        _ => None,
    }
}

fn is(toks: &[Tok], i: usize, want: &[&str]) -> bool {
    want.iter().enumerate().all(|(k, w)| match toks.get(i + k) {
        Some(Tok::Ident(s)) => s == w,
        Some(Tok::Punct(c)) => w.len() == 1 && w.starts_with(*c),
        _ => false,
    })
}

/// The ribbon's controls, `Item::big(icon, "Label", …)` and the other constructors (the label, and
/// the tooltip's name where it has one), also through a closure that hands its parameters on
/// (`let due = |icon, label, tip, …| Item::small(icon, label, tip, …)`), a label chosen by an `if`
/// included; and its groups' titles (`g("Respond", …)`). A large button's label keeps its line
/// break (`"Follow\nUp"`): the translation says where it breaks.
fn ribbon(toks: &[Tok], file: &str, texts: &mut Texts) {
    // Which arguments of each constructor are a label or a tooltip's name.
    const AT: [(&str, &[usize]); 7] =
        [("big", &[1]), ("small", &[1, 2]), ("big_menu", &[2, 3]), ("small_menu", &[2, 3]), ("gather", &[2]), ("new", &[2]), ("pending", &[1])];
    let ctor = |i: usize| AT.iter().find(|(f, _)| is(toks, i, &["Item", ":", ":", f, "("])).map(|(_, at)| (i + 4, *at));
    // The closures that make a control: which of their parameters end up a label.
    let mut closures: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for i in 0..toks.len() {
        let (Some(Tok::Ident(name)), true) = (toks.get(i + 1), is(toks, i, &["let"]) && is(toks, i + 2, &["=", "|"])) else { continue };
        let Some(bar) = (i + 4..toks.len()).find(|k| toks.get(*k) == Some(&Tok::Punct('|'))) else { continue };
        let Some((open, at)) = ctor(bar + 1) else { continue };
        // Each parameter's name: the first word of each part between commas.
        let params: Vec<&str> = toks
            .get(i + 4..bar)
            .unwrap_or_default()
            .split(|t| *t == Tok::Punct(','))
            .map(|p| if let Some(Tok::Ident(n)) = p.first() { n.as_str() } else { "" })
            .collect();
        let labels = args(toks, open)
            .iter()
            .enumerate()
            .filter(|(k, _)| at.contains(k))
            .filter_map(|(_, a)| if let [Tok::Ident(n)] = a { params.iter().position(|p| p == n) } else { None })
            .collect();
        closures.insert(name.clone(), labels);
    }
    for i in 0..toks.len() {
        let after_fn = matches!(i.checked_sub(1).and_then(|k| toks.get(k)), Some(Tok::Ident(w)) if w == "fn");
        let (open, at): (usize, &[usize]) = if let Some(found) = ctor(i) {
            found
        } else if is(toks, i, &["g", "("]) && !after_fn {
            (i + 1, &[0])
        } else if let (Some(Tok::Ident(n)), Some(Tok::Punct('('))) = (toks.get(i), toks.get(i + 1))
            && let Some(at) = closures.get(n)
        {
            (i + 1, at)
        } else {
            continue;
        };
        for (_, a) in args(toks, open).iter().enumerate().filter(|(k, _)| at.contains(k)) {
            let chosen = matches!(a.first(), Some(Tok::Ident(w)) if w == "if");
            for t in if chosen { a } else { a.get(..1).filter(|_| a.len() == 1).unwrap_or_default() } {
                if let Tok::Str(s) = t {
                    add(texts, s, None, file, "ribbon");
                }
            }
        }
    }
}

/// `tl!("…")`, `tlc!("context", "…")`, `tln!(n, "…", "…")`, and a literal where a function
/// translates that parameter.
fn literals(toks: &[Tok], file: &str, through: &BTreeMap<String, BTreeSet<usize>>, consts: &BTreeMap<String, String>, texts: &mut Texts) {
    for i in 0..toks.len() {
        if is(toks, i, &["tl", "!", "("]) {
            if let Some(s) = args(toks, i + 2).first().and_then(|a| lit(a)) {
                add(texts, s, None, file, "interface");
            } else if let (Some(Tok::Ident(name)), Some(Tok::Punct(')'))) = (toks.get(i + 3), toks.get(i + 4))
                && let Some(s) = consts.get(name)
            {
                add(texts, s, None, file, "interface");
            }
        } else if is(toks, i, &["tlc", "!", "("]) {
            if let [context, s] = args(toks, i + 2).as_slice()
                && let (Some(context), Some(s)) = (lit(context), lit(s))
            {
                add_in(texts, Some(context), s, None, file, "interface");
            }
        } else if is(toks, i, &["tln", "!", "("]) {
            let a = args(toks, i + 2);
            if let [_, one, other] = a.as_slice()
                && let (Some(one), Some(other)) = (lit(one), lit(other))
            {
                add(texts, one, Some(other), file, "interface");
            }
        } else if let (Some(Tok::Ident(name)), Some(Tok::Punct('('))) = (toks.get(i), toks.get(i + 1))
            && let Some(at) = through.get(name)
            && !matches!(i.checked_sub(1).and_then(|k| toks.get(k)), Some(Tok::Ident(w)) if w == "fn")
        {
            let method = matches!(i.checked_sub(1).and_then(|k| toks.get(k)), Some(Tok::Punct('.')));
            for (k, a) in args(toks, i + 1).iter().enumerate() {
                // A method's parameters count `self`; its call doesn't.
                let index = if method { k + 1 } else { k };
                if at.contains(&index)
                    && let Some(s) = lit(a)
                {
                    add(texts, s, None, file, "interface");
                }
            }
        }
    }
}

/// Which parameters of which functions go through `tl!`: `fn mi(ui, app, label: &str, …)` with
/// `tl!(label)` in its body.
fn translating_params(toks: &[Tok], out: &mut BTreeMap<String, BTreeSet<usize>>) {
    for i in 0..toks.len() {
        let (Some(Tok::Ident(f)), Some(Tok::Ident(name))) = (toks.get(i), toks.get(i + 1)) else { continue };
        if f != "fn" {
            continue;
        }
        // Generics before the parameters.
        let mut open = i + 2;
        if toks.get(open) == Some(&Tok::Punct('<')) {
            let mut depth = 0usize;
            while let Some(t) = toks.get(open) {
                match t {
                    Tok::Punct('<') => depth += 1,
                    Tok::Punct('>') => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            open += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                open += 1;
            }
        }
        if toks.get(open) != Some(&Tok::Punct('(')) {
            continue;
        }
        let params: Vec<Option<String>> = args(toks, open)
            .iter()
            .map(|p| match p {
                [Tok::Ident(n), Tok::Punct(':'), Tok::Punct('&'), Tok::Ident(t)] if t == "str" => Some(n.clone()),
                [Tok::Ident(n), Tok::Punct(':'), Tok::Punct('&'), Tok::Ident(l), Tok::Ident(t)] if t == "str" && l.starts_with('\'') => {
                    Some(n.clone())
                }
                _ => None,
            })
            .collect();
        if params.iter().all(Option::is_none) {
            continue;
        }
        let Some(body) = (close(toks, open)..toks.len()).find(|k| matches!(toks.get(*k), Some(Tok::Punct('{' | ';')))) else { continue };
        if toks.get(body) != Some(&Tok::Punct('{')) {
            continue;
        }
        let end = close(toks, body);
        let body = toks.get(body..end).unwrap_or_default();
        for (k, p) in params.iter().enumerate() {
            let Some(p) = p else { continue };
            // `tl!(p)` in the window; `Arg::words(p)` or `lang.t(p)` in the engine.
            let used = (0..body.len()).any(|j| {
                let at = |k: usize| body.get(j + k) == Some(&Tok::Ident(p.clone())) && body.get(j + k + 1) == Some(&Tok::Punct(')'));
                (is(body, j, &["tl", "!", "("]) && at(3)) || (is(body, j, &["words", "("]) && at(2)) || (is(body, j, &[".", "t", "("]) && at(3))
            });
            if used {
                out.entry(name.clone()).or_default().insert(k);
            }
        }
    }
}

/// A command's label, and the parts of its location (`Home › Respond`).
fn command(texts: &mut Texts, file: &str, label: &str, location: &str) {
    add(texts, label, None, file, "command label");
    for part in location.split(" › ") {
        add(texts, part, None, file, "ribbon location");
    }
}

/// `UI_COMMANDS`: `("ui.theme", "Interface Theme", "File › Options", …)`.
fn window_commands(toks: &[Tok], file: &str, texts: &mut Texts) {
    for i in 0..toks.len() {
        if let (Some(Tok::Punct('(')), Some(Tok::Str(id)), Some(Tok::Punct(',')), Some(Tok::Str(label)), Some(Tok::Punct(',')), Some(Tok::Str(loc))) =
            (toks.get(i), toks.get(i + 1), toks.get(i + 2), toks.get(i + 3), toks.get(i + 4), toks.get(i + 5))
            && id.starts_with("ui.")
        {
            command(texts, file, label, loc);
        }
    }
}

/// `CommandSpec::new("mail.reply", "Reply", "Home › Respond", …)`.
fn commands(toks: &[Tok], file: &str, texts: &mut Texts) {
    for i in 0..toks.len() {
        if is(toks, i, &["CommandSpec", ":", ":", "new", "("]) {
            let a = args(toks, i + 4);
            if let (Some(label), Some(loc)) = (a.get(1).and_then(|a| lit(a)), a.get(2).and_then(|a| lit(a))) {
                command(texts, file, label, loc);
            }
        }
    }
}

/// `Some("…")` from `range`.
fn somes(toks: &[Tok], range: std::ops::Range<usize>, file: &str, kind: &'static str, texts: &mut Texts) {
    for j in range {
        if is(toks, j, &["Some", "("])
            && let (Some(Tok::Str(s)), Some(Tok::Punct(')'))) = (toks.get(j + 2), toks.get(j + 3))
        {
            add(texts, s, None, file, kind);
        }
    }
}

/// Why a command can't run: `.when(|s| … Some("…"))`, `.selection(…)`, and the functions they
/// name (`fn needs_message(s: &Session) -> Option<&'static str>`).
fn reasons(toks: &[Tok], file: &str, texts: &mut Texts) {
    const KIND: &str = "why a command can't run";
    for i in 0..toks.len() {
        if is(toks, i, &[".", "when", "("]) || is(toks, i, &[".", "selection", "("]) {
            somes(toks, i + 2..close(toks, i + 2), file, KIND, texts);
        }
        if is(toks, i, &["fn"]) && toks.get(i + 2) == Some(&Tok::Punct('(')) {
            let end = close(toks, i + 2);
            let session = toks.get(i + 2..end).unwrap_or_default().contains(&Tok::Ident("Session".into()));
            if session && is(toks, end + 1, &["-", ">", "Option", "<", "&", "'static", "str", ">", "{"]) {
                let body = end + 9;
                somes(toks, body..close(toks, body), file, KIND, texts);
            }
        }
    }
}

/// The literals of `fn label(self) -> &'static str { … }` and the like (in the time crate, also
/// `name` and `short`: the days).
fn names(toks: &[Tok], file: &str, texts: &mut Texts) {
    let time = file.starts_with("crates/time/");
    for i in 0..toks.len() {
        let Some(Tok::Ident(name)) = toks.get(i + 1) else { continue };
        let wanted = NAME_FNS.contains(&name.as_str()) || (time && (name == "name" || name == "short"));
        if !wanted || !is(toks, i, &["fn"]) || toks.get(i + 2) != Some(&Tok::Punct('(')) {
            continue;
        }
        let end = close(toks, i + 2);
        let params = toks.get(i + 3..end).unwrap_or_default();
        let by_self = matches!(params, [Tok::Ident(s)] | [Tok::Punct('&'), Tok::Ident(s)] if s == "self");
        if by_self && is(toks, end + 1, &["-", ">", "&", "'static", "str", "{"]) {
            let body = end + 7;
            for t in toks.get(body..close(toks, body)).unwrap_or_default() {
                if let Tok::Str(s) = t {
                    add(texts, s, None, file, "name");
                    // A standard folder's default name is also looked up as the folder's
                    // (`Lang::folder`): "Archive" the folder isn't "Archive" the command.
                    if name == "default_name" {
                        add_in(texts, Some(mailcraft_i18n::FOLDER), s, None, file, "folder name");
                    }
                }
            }
        }
    }
}

/// Every `const NAME: &str = "…";`, by name.
fn constants(toks: &[Tok], out: &mut BTreeMap<String, String>) {
    for i in 0..toks.len() {
        if !is(toks, i, &["const"]) {
            continue;
        }
        let Some(Tok::Ident(name)) = toks.get(i + 1) else { continue };
        let ty = if is(toks, i + 2, &[":", "&", "str", "="]) {
            4
        } else if is(toks, i + 2, &[":", "&", "'static", "str", "="]) {
            5
        } else {
            continue;
        };
        if let (Some(Tok::Str(s)), Some(Tok::Punct(';'))) = (toks.get(i + 2 + ty), toks.get(i + 3 + ty)) {
            out.insert(name.clone(), s.clone());
        }
    }
}

/// What the engine and the model say to the owner: the first argument of `failed!(…)`,
/// `said!(…)`, `fail(…)`, `CmdError::Failed(…)`, `ModelError::Invalid(…)`, `invalid(…)` and
/// `Arg::words(…)`, a literal or a constant (`fail(BUSY)`); `lang.t("…")` and
/// `lang.tn(n, "…", "…")`; a literal passed where a function says that parameter
/// (`added_by_newer(a, "…")`); and the constants a `fn refusal…() -> &'static str` gives. A text
/// with a backtick in it names a parameter: it is for an agent, and stays English.
fn engine_says(toks: &[Tok], file: &str, through: &BTreeMap<String, BTreeSet<usize>>, consts: &BTreeMap<String, String>, texts: &mut Texts) {
    const KIND: &str = "what a command says";
    let mut say = |s: &str, plural: Option<&str>| {
        if !s.contains('`') {
            add(texts, s, plural, file, KIND);
        }
    };
    // A literal or a constant, as it is or `.into()`, `.to_string()`.
    let text = |a: &[Tok]| -> Option<String> {
        let rest_ok = matches!(a.get(1), None | Some(Tok::Punct('.')));
        match a.first() {
            Some(Tok::Str(s)) if rest_ok => Some(s.clone()),
            Some(Tok::Ident(n)) if rest_ok => consts.get(n).cloned(),
            _ => None,
        }
    };
    for i in 0..toks.len() {
        let after_fn = matches!(i.checked_sub(1).and_then(|k| toks.get(k)), Some(Tok::Ident(w)) if w == "fn");
        let open = if is(toks, i, &["failed", "!", "("]) || is(toks, i, &["said", "!", "("]) {
            Some(i + 2)
        } else if !after_fn && ["fail", "Failed", "Invalid", "invalid", "words"].iter().any(|f| is(toks, i, &[f, "("])) {
            Some(i + 1)
        } else {
            None
        };
        if let Some(open) = open {
            if let Some(s) = args(toks, open).first().and_then(|a| text(a)) {
                say(&s, None);
            }
        } else if is(toks, i, &[".", "t", "("]) {
            if let Some(s) = args(toks, i + 2).first().and_then(|a| text(a)) {
                say(&s, None);
            }
        } else if is(toks, i, &[".", "tn", "("]) {
            if let [_, one, other] = args(toks, i + 2).as_slice()
                && let (Some(one), Some(other)) = (lit(one), lit(other))
            {
                say(one, Some(other));
            }
        } else if let Some(Tok::Ident(name)) = toks.get(i)
            && toks.get(i + 1) == Some(&Tok::Punct('('))
            && !after_fn
            && let Some(at) = through.get(name)
        {
            let method = matches!(i.checked_sub(1).and_then(|k| toks.get(k)), Some(Tok::Punct('.')));
            for (k, a) in args(toks, i + 1).iter().enumerate() {
                if at.contains(&if method { k + 1 } else { k })
                    && let Some(s) = lit(a)
                {
                    say(s, None);
                }
            }
        }
        if is(toks, i, &["fn"])
            && let Some(Tok::Ident(name)) = toks.get(i + 1)
            && name.starts_with("refusal")
            && toks.get(i + 2) == Some(&Tok::Punct('('))
        {
            let end = close(toks, i + 2);
            if is(toks, end + 1, &["-", ">", "&", "'static", "str", "{"]) {
                let body = end + 6;
                for t in toks.get(body..close(toks, body)).unwrap_or_default() {
                    match t {
                        Tok::Str(s) => say(s, None),
                        Tok::Ident(n) => {
                            if let Some(s) = consts.get(n) {
                                say(s, None);
                            }
                        }
                        Tok::Punct(_) => {}
                    }
                }
            }
        }
    }
}

/// The literals of every `const` of the dates' file: names, patterns, and the plurals as
/// `("{n} day", "{n} days")`.
fn dates(toks: &[Tok], file: &str, texts: &mut Texts) {
    for i in 0..toks.len() {
        if !is(toks, i, &["const"]) {
            continue;
        }
        let Some(eq) = (i..toks.len()).find(|k| toks.get(*k) == Some(&Tok::Punct('='))) else { continue };
        let end = (eq..toks.len()).find(|k| toks.get(*k) == Some(&Tok::Punct(';'))).unwrap_or(toks.len());
        let value = toks.get(eq + 1..end).unwrap_or_default();
        match value {
            [Tok::Punct('('), Tok::Str(one), Tok::Punct(','), Tok::Str(many), Tok::Punct(')')] => {
                add(texts, one, Some(many), file, "date and time");
            }
            _ => {
                for t in value {
                    if let Tok::Str(s) = t {
                        add(texts, s, None, file, "date and time");
                    }
                }
            }
        }
    }
}

// ---- The template and the catalogues. ----

const HEADER: &str = "Project-Id-Version: MailCraft\nMIME-Version: 1.0\nContent-Type: text/plain; charset=UTF-8\nContent-Transfer-Encoding: 8bit\nPlural-Forms: nplurals=INTEGER; plural=EXPRESSION;\n";

/// The entry of a text found under `key` ([`Texts`]).
fn entry_of(key: &str, f: &Found, forms: usize) -> Entry {
    let (context, id) = match key.split_once('\u{4}') {
        Some((c, id)) => (Some(c.to_string()), id),
        None => (None, key),
    };
    Entry {
        extracted: vec![f.kinds.iter().copied().collect::<Vec<_>>().join(", ")],
        references: vec![f.refs.iter().cloned().collect::<Vec<_>>().join(" ")],
        context,
        id: id.to_string(),
        id_plural: f.plural.clone(),
        strs: vec![String::new(); if f.plural.is_some() { forms } else { 1 }],
        ..Entry::default()
    }
}

fn template(found: &Texts) -> String {
    let mut out = String::from(
        "# The English text of MailCraft's interface, gathered by `cargo xtask i18n` (never edit it by\n# hand). A language's catalogue is `<code>.po` beside it, kept up to date by the same command.\n",
    );
    let mut entries = vec![Entry { strs: vec![HEADER.into()], ..Entry::default() }];
    entries.extend(found.iter().map(|(id, f)| entry_of(id, f, 2)));
    out.push_str(&po::write(&entries));
    out
}

/// A catalogue brought up to date: the header and every translation it has kept, the template's
/// texts in the template's order, a new one empty; one no longer used dropped, or kept as obsolete
/// when it had a translation.
fn merge(old: &[Entry], found: &Texts) -> Vec<Entry> {
    let header = old.iter().find(|e| e.is_header() && !e.obsolete).cloned();
    let forms = header.as_ref().and_then(|h| h.header_field("Plural-Forms")).map_or(Rule::default(), Rule::from_header).forms();
    let by_key: BTreeMap<String, &Entry> = old.iter().filter(|e| !e.is_header()).map(|e| (e.key(), e)).collect();
    let mut out: Vec<Entry> = header.into_iter().collect();
    for (key, f) in found {
        let mut e = entry_of(key, f, forms);
        if let Some(was) = by_key.get(key) {
            e.comments = was.comments.clone();
            e.previous = was.previous.clone();
            e.flags = was.flags.clone();
            if was.id_plural.is_some() == f.plural.is_some() && was.strs.iter().any(|s| !s.is_empty()) {
                e.strs = was.strs.clone();
            }
            e.line = was.line;
        }
        out.push(e);
    }
    for was in old.iter().filter(|e| !e.is_header() && !found.contains_key(&e.key())) {
        if was.strs.iter().any(|s| !s.is_empty()) {
            out.push(Entry { obsolete: true, references: Vec::new(), extracted: Vec::new(), ..(*was).clone() });
        }
    }
    out
}

fn coverage(entries: &[Entry]) -> (usize, usize) {
    let live = entries.iter().filter(|e| !e.obsolete && !e.is_header() && e.translated());
    live.fold((0, 0), |(t, f), e| (t + 1, f + usize::from(e.fuzzy())))
}

/// A translation whose `{placeholders}` aren't its English's.
fn mismatch(e: &Entry) -> Option<String> {
    let want = po::placeholders(&e.id);
    let want_plural = e.id_plural.as_deref().map(po::placeholders);
    for (k, s) in e.strs.iter().enumerate().filter(|(_, s)| !s.is_empty()) {
        let have = po::placeholders(s);
        // A plural's first form may leave the count out ("un message"), as long as it names no other.
        let ok = match &want_plural {
            Some(p) => have == *p || (k == 0 && (have == want || have.iter().all(|h| p.contains(h)))),
            None => have == want,
        };
        if !ok {
            let mut msg = String::new();
            let _ = write!(msg, "{:?}: the translation has {{{}}}, the English {{{}}}", e.id, have.join("}, {"), want.join("}, {"));
            return Some(msg);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_what_the_window_says() {
        let src = r##"
            fn mi(ui: &mut Ui, label: &str, id: &str) { ui.button(tl!(label)); }
            fn draw() {
                let a = tl!("Reply"); // tl!("not this")
                let b = tln!(n, "{n} message", "{n} messages");
                let f = tlc!("folder", "Archive");
                mi(ui, "New Message", "mail.new");
                let c = tl!(r#"Say "hi""#);
                let g = tl!(NOTE);
                let d = '"'; let e = tl!("Line \
                    two");
            }
            const NOTE: &str = "A named note";
            fn label(self) -> &'static str { match self { A => "Upcoming", B => "Today" } }
            mod tests { fn t() { tl!("Only in a test"); } }
        "##;
        let toks = without_tests(lex(src));
        let mut through = BTreeMap::new();
        translating_params(&toks, &mut through);
        assert_eq!(through.get("mi"), Some(&BTreeSet::from([1])));
        let mut texts = Texts::new();
        let mut consts = BTreeMap::new();
        constants(&toks, &mut consts);
        literals(&toks, "a.rs", &through, &consts, &mut texts);
        names(&toks, "a.rs", &mut texts);
        let ids: Vec<&str> = texts.keys().map(String::as_str).collect();
        assert_eq!(ids, ["A named note", "Line two", "New Message", "Reply", "Say \"hi\"", "Today", "Upcoming", "folder\u{4}Archive", "{n} message"]);
        assert_eq!(texts.get("{n} message").and_then(|f| f.plural.as_deref()), Some("{n} messages"));
    }

    #[test]
    fn the_ribbons_labels() {
        let src = r#"
            fn g(title: &'static str, items: Vec<Item>) -> Group { Group { title, items } }
            fn home() -> Tab {
                Tab::of(vec![g("Respond", vec![
                    Item::big("newMail", "New\nEmail", "mail.new", json!({})),
                    Item::small("reply", "Reply", "", "mail.reply", json!({})),
                    Item::big_menu("move", "move", "Move", "Move to a folder", menu()),
                    Item::small_menu("rules", "rules", "Rules", "", menu()),
                    Item::gather("followUp", "flag", "Follow Up").at(3, 3),
                    Item::new("x", "icon", "Side-by-\nSide", Size::Big),
                    Item::big("icon", LABEL, "mail.new", json!({})),
                    Item::big(if on { "starOn" } else { "star" }, if on { "Remove from\nFavorites" } else { "Add to\nFavorites" }, "f", no()),
                ])]);
                let due = |icon, label, tip, key, date: Value| Item::small(icon, label, tip, "tasks.edit", json!({"due": date})).key(key);
                due("flag", "Today", "Due today", "tasks.due.today", json!(1));
            }
        "#;
        let mut texts = Texts::new();
        ribbon(&lex(src), "ribbon.rs", &mut texts);
        let ids: Vec<&str> = texts.keys().map(String::as_str).collect();
        assert_eq!(
            ids,
            [
                "Add to\nFavorites",
                "Due today",
                "Follow Up",
                "Move",
                "Move to a folder",
                "New\nEmail",
                "Remove from\nFavorites",
                "Reply",
                "Respond",
                "Rules",
                "Side-by-\nSide",
                "Today"
            ]
        );
    }

    #[test]
    fn engine_tables() {
        let src = r#"
            fn needs_message(s: &Session) -> Option<&'static str> { if x { None } else { Some("select a message") } }
            fn all() { vec![CommandSpec::new("mail.reply", "Reply", "Home › Respond", reply).when(|s| Some("nothing to undo"))] }
            const MONTHS: [&str; 2] = ["January", "May"];
            const MEDIUM: &str = "{month} {day}, {year}";
            const DAYS: (&str, &str) = ("{n} day", "{n} days");
        "#;
        let toks = lex(src);
        let mut texts = Texts::new();
        commands(&toks, "e.rs", &mut texts);
        reasons(&toks, "e.rs", &mut texts);
        dates(&toks, "crates/i18n/src/date.rs", &mut texts);
        let ids: Vec<&str> = texts.keys().map(String::as_str).collect();
        assert_eq!(ids, ["Home", "January", "May", "Reply", "Respond", "nothing to undo", "select a message", "{month} {day}, {year}", "{n} day"]);
        assert_eq!(texts.get("{n} day").and_then(|f| f.plural.as_deref()), Some("{n} days"));
    }

    #[test]
    fn what_the_engine_says() {
        let src = r#"
            const BUSY: &str = "the mailbox is busy";
            const ORIGINAL: &str = "-----Original Message-----";
            pub(crate) fn added_by_newer(a: &Account, then: &'static str) -> CmdError { failed!("{who} was added later, {then}", who, then = Arg::words(then)) }
            fn fail(msg: impl Into<String>) -> CmdError { CmdError::Failed(msg.into()) }
            fn refusal_into(&self) -> &'static str { if x { BUSY } else { "not yet" } }
            fn run() {
                fail("no message selected");
                fail(BUSY);
                fail("`id` is required");
                fail(format!("{x} isn't here"));
                Err(ModelError::Invalid("no such paragraph".into()));
                added_by_newer(a, "which keeps its settings");
                let s = lang.tn(n, "{n} message", "{n} messages");
                let u = lang.t("Up to date");
                let o = lang.t(ORIGINAL);
            }
        "#;
        let toks = lex(src);
        let (mut says, mut consts) = (BTreeMap::new(), BTreeMap::new());
        translating_params(&toks, &mut says);
        constants(&toks, &mut consts);
        let mut texts = Texts::new();
        engine_says(&toks, "e.rs", &says, &consts, &mut texts);
        let ids: Vec<&str> = texts.keys().map(String::as_str).collect();
        assert_eq!(
            ids,
            [
                "-----Original Message-----",
                "Up to date",
                "no message selected",
                "no such paragraph",
                "not yet",
                "the mailbox is busy",
                "which keeps its settings",
                "{n} message",
                "{who} was added later, {then}"
            ]
        );
    }

    #[test]
    fn merging_keeps_translations_and_checks_placeholders() {
        let mut found = Texts::new();
        add(&mut found, "Reply", None, "a.rs", "interface");
        add(&mut found, "{n} message", Some("{n} messages"), "a.rs", "interface");
        add_in(&mut found, Some("folder"), "Archive", None, "a.rs", "folder name");
        add(&mut found, "Archive", None, "a.rs", "interface");
        let old = po::parse(
            "msgid \"\"\nmsgstr \"Plural-Forms: nplurals=2; plural=(n > 1);\\n\"\n\n#, fuzzy\nmsgid \"Reply\"\nmsgstr \"Répondre\"\n\nmsgid \"Gone\"\nmsgstr \"Parti\"\n\nmsgid \"Gone untranslated\"\nmsgstr \"\"\n\nmsgctxt \"folder\"\nmsgid \"Archive\"\nmsgstr \"Archives\"\n\nmsgid \"Archive\"\nmsgstr \"Archiver\"\n\nmsgctxt \"gone\"\nmsgid \"Archive\"\nmsgstr \"Vieux\"\n",
        );
        let merged = merge(&old.entries, &found);
        let reply = merged.iter().find(|e| e.id == "Reply").unwrap();
        assert_eq!((reply.strs.clone(), reply.fuzzy()), (vec!["Répondre".to_string()], true));
        assert!(merged.iter().any(|e| e.id == "Gone" && e.obsolete));
        assert!(!merged.iter().any(|e| e.id == "Gone untranslated"));
        assert_eq!(merged.iter().find(|e| e.id == "{n} message").unwrap().strs.len(), 2);
        let archive = |c: Option<&str>| merged.iter().find(|e| e.id == "Archive" && e.context.as_deref() == c && !e.obsolete).map(|e| e.strs.clone());
        assert_eq!((archive(Some("folder")), archive(None)), (Some(vec!["Archives".to_string()]), Some(vec!["Archiver".to_string()])));
        assert!(merged.iter().any(|e| e.context.as_deref() == Some("gone") && e.obsolete));
        assert!(po::write(&merged).contains("msgctxt \"folder\"\nmsgid \"Archive\"\nmsgstr \"Archives\""));
        assert_eq!(coverage(&merged), (3, 1));
        let bad = Entry { id: "{n} of {total}".into(), strs: vec!["{n} sur {totale}".into()], ..Entry::default() };
        assert!(mismatch(&bad).is_some());
        let bad_in_context = Entry { context: Some("folder".into()), ..bad };
        assert!(mismatch(&bad_in_context).is_some());
        let one = Entry {
            id: "{n} message".into(),
            id_plural: Some("{n} messages".into()),
            strs: vec!["un message".into(), "{n} messages".into()],
            ..Entry::default()
        };
        assert!(mismatch(&one).is_none());
    }
}
