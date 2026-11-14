// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the Android app asks the core to translate: the English literals in `t("…")` and
//! `tn(n, "one", "other")` calls in Kotlin, for `cargo xtask i18n`. Kotlin has no macro to find them
//! by, so the app's one rule is that every text the owner reads goes through a call named `t` or
//! `tn` whose texts are string literals (a call on any receiver, `core.t("…")`, counts).

/// One text found: the English, and its plural form for `tn`.
#[derive(Debug, PartialEq, Eq)]
pub struct Said {
    pub id: String,
    pub plural: Option<String>,
}

/// The texts of `t(…)` and `tn(…)` calls in the Kotlin source `src`, in order. A call whose text is
/// not a plain literal (a variable, a template with `$`) is skipped: it can't be gathered.
pub fn scan(src: &str) -> Vec<Said> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // Comments and strings are skipped whole so that a `t(` in them is not a call.
        match (chars.get(i), chars.get(i + 1)) {
            (Some('/'), Some('/')) => {
                while chars.get(i).is_some_and(|c| *c != '\n') {
                    i += 1;
                }
                continue;
            }
            (Some('/'), Some('*')) => {
                i += 2;
                while i < chars.len() && !(chars.get(i) == Some(&'*') && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
                continue;
            }
            (Some('"'), _) => {
                i = literal(&chars, i).1;
                continue;
            }
            _ => {}
        }
        if let Some((open, tn)) = call_at(&chars, i) {
            if let Some(said) = arguments(&chars, open + 1, tn) {
                out.push(said);
            }
            i = open + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// `t(` or `tn(` at `i` (white space may stand before the `(`), as a word of its own (not `get(`,
/// `format(`, `x.at(`): the index of the `(` and whether it is `tn`.
fn call_at(chars: &[char], i: usize) -> Option<(usize, bool)> {
    let word = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
    if i > 0 && word(chars.get(i - 1)) {
        return None;
    }
    let (len, tn) = match (chars.get(i), chars.get(i + 1)) {
        (Some('t'), Some('n')) => (2, true),
        (Some('t'), _) => (1, false),
        _ => return None,
    };
    if word(chars.get(i + len)) {
        return None;
    }
    let mut open = i + len;
    while chars.get(open).is_some_and(|c| c.is_whitespace()) {
        open += 1;
    }
    (chars.get(open) == Some(&'(')).then_some((open, tn))
}

/// The literals of the call whose `(` is just before `at`: one for `t`, the last two literal arguments
/// for `tn` (`tn(n, "one", "other")`).
fn arguments(chars: &[char], at: usize, tn: bool) -> Option<Said> {
    let mut i = at;
    let mut texts: Vec<Option<String>> = Vec::new();
    let mut depth = 0usize;
    let mut piece_start = true;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                let (s, end) = literal(chars, i);
                if depth == 0 && piece_start {
                    let alone = chars[end..].iter().find(|c| !c.is_whitespace()).is_some_and(|c| *c == ',' || *c == ')');
                    texts.push(if alone { s } else { None });
                    piece_start = false;
                }
                i = end;
                continue;
            }
            '(' | '[' | '{' => {
                // An argument that opens with a bracket still holds its slot.
                if depth == 0 && piece_start {
                    texts.push(None);
                    piece_start = false;
                }
                depth += 1;
            }
            ')' | ']' | '}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                piece_start = true;
                i += 1;
                continue;
            }
            c if c.is_whitespace() => {}
            _ => {
                if depth == 0 && piece_start {
                    texts.push(None);
                    piece_start = false;
                }
            }
        }
        i += 1;
    }
    if tn {
        let two = texts.get(1..3)?;
        Some(Said { id: two[0].clone()?, plural: Some(two[1].clone()?) })
    } else {
        Some(Said { id: texts.first()?.clone()?, plural: None })
    }
}

/// The string literal opening at `start` (a `"`): its text (`None` when it has a template `$name`
/// or `${…}`, which is not one text) and the index after its closing quote.
fn literal(chars: &[char], start: usize) -> (Option<String>, usize) {
    let mut out = String::new();
    let mut template = false;
    let triple = chars.get(start + 1) == Some(&'"') && chars.get(start + 2) == Some(&'"');
    let mut i = if triple { start + 3 } else { start + 1 };
    while i < chars.len() {
        if triple {
            // A raw string: no escapes, and it ends at the last of a run of three or more quotes.
            if chars[i] == '"' && chars.get(i + 1) == Some(&'"') && chars.get(i + 2) == Some(&'"') {
                let mut end = i + 3;
                while chars.get(end) == Some(&'"') {
                    out.push('"');
                    end += 1;
                }
                return (if template { None } else { Some(out) }, end);
            }
            if chars[i] == '$' && chars.get(i + 1).is_some_and(|c| c.is_alphabetic() || *c == '{' || *c == '_') {
                template = true;
            }
            out.push(chars[i]);
            i += 1;
            continue;
        }
        match chars[i] {
            '"' => return (if template { None } else { Some(out) }, i + 1),
            '\\' => {
                i += 1;
                match chars.get(i) {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('u') => {
                        let hex: String = chars.iter().skip(i + 1).take(4).collect();
                        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            Some(c) => out.push(c),
                            None => template = true,
                        }
                        i += 4;
                    }
                    Some(c) => out.push(*c),
                    None => break,
                }
            }
            '$' if chars.get(i + 1).is_some_and(|c| c.is_alphabetic() || *c == '{' || *c == '_') => {
                template = true;
                out.push('$');
            }
            c => out.push(c),
        }
        i += 1;
    }
    (None, chars.len())
}

#[cfg(test)]
#[path = "i18n_kotlin_tests.rs"]
mod i18n_kotlin_tests;
