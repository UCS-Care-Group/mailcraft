// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The affix file: the directives this dictionary uses, read from the format described
//! by the hunspell(5) manual page. Unknown directives and malformed lines are skipped.
//! A `SET` other than UTF-8 is an error.

use crate::error::Error;

const MAX_LINE: usize = 4096;
const MAX_RULES: usize = 10_000;
const MAX_CONDITION: usize = 128;
const MAX_REPS: usize = 10_000;
const MAX_ICONV: usize = 1_000;
const MAX_PATTERNS: usize = 1_000;
const MAX_ATOMS: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Prefix,
    Suffix,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Quant {
    One,
    /// Zero or more (`*` in a compound pattern).
    Star,
    /// Zero or one (`?` in a compound pattern).
    Opt,
}

#[derive(Clone, Copy)]
pub(crate) struct Pat {
    pub flag: char,
    pub quant: Quant,
}

#[derive(Clone)]
enum Atom {
    Any,
    Char(char),
    Class { negated: bool, chars: Vec<char> },
}

impl Atom {
    fn matches(&self, c: char) -> bool {
        match self {
            Atom::Any => true,
            Atom::Char(expected) => *expected == c,
            Atom::Class { negated, chars } => {
                let has = chars.contains(&c);
                if *negated { !has } else { has }
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Condition {
    atoms: Vec<Atom>,
}

impl Condition {
    pub(crate) fn matches_end(&self, stem: &str) -> bool {
        let n = self.atoms.len();
        if n == 0 || n > MAX_ATOMS {
            return false;
        }
        let mut buf = ['\0'; MAX_ATOMS];
        if !last_chars(stem, n, &mut buf) {
            return false;
        }
        self.matches_buf(&buf[..n])
    }

    pub(crate) fn matches_start(&self, stem: &str) -> bool {
        let n = self.atoms.len();
        if n == 0 || n > MAX_ATOMS {
            return false;
        }
        let mut buf = ['\0'; MAX_ATOMS];
        let mut count = 0usize;
        for c in stem.chars() {
            if count >= n {
                break;
            }
            if let Some(slot) = buf.get_mut(count) {
                *slot = c;
            }
            count = count.saturating_add(1);
        }
        count == n && self.matches_buf(&buf[..n])
    }

    fn matches_buf(&self, chars: &[char]) -> bool {
        if self.atoms.len() != chars.len() {
            return false;
        }
        self.atoms.iter().zip(chars.iter()).all(|(atom, c)| atom.matches(*c))
    }
}

fn last_chars(stem: &str, n: usize, buf: &mut [char]) -> bool {
    let mut count = 0usize;
    for c in stem.chars().rev() {
        if count >= n {
            break;
        }
        let index = n.saturating_sub(count).saturating_sub(1);
        if let Some(slot) = buf.get_mut(index) {
            *slot = c;
        }
        count = count.saturating_add(1);
    }
    count == n
}

#[derive(Clone)]
pub(crate) struct Rule {
    pub flag: char,
    pub cross: bool,
    pub strip: String,
    pub add: String,
    pub condition: Condition,
}

pub(crate) struct Affix {
    pub try_chars: Vec<char>,
    pub iconv: Vec<(String, String)>,
    pub nosuggest: Option<char>,
    pub compound_min: usize,
    pub only_in_compound: Option<char>,
    pub patterns: Vec<Vec<Pat>>,
    pub word_chars: Vec<char>,
    pub prefixes: Vec<Rule>,
    pub suffixes: Vec<Rule>,
    pub reps: Vec<(String, String)>,
}

struct Open {
    kind: Kind,
    flag: char,
    cross: bool,
}

pub(crate) fn parse(text: &str) -> Result<Affix, Error> {
    let mut aff = Affix {
        try_chars: Vec::new(),
        iconv: Vec::new(),
        nosuggest: None,
        compound_min: 3,
        only_in_compound: None,
        patterns: Vec::new(),
        word_chars: Vec::new(),
        prefixes: Vec::new(),
        suffixes: Vec::new(),
        reps: Vec::new(),
    };
    let mut open: Option<Open> = None;
    for raw in text.strip_prefix('\u{feff}').unwrap_or(text).lines() {
        if raw.len() > MAX_LINE {
            continue;
        }
        let line = raw.trim_matches(['\r', '\n']).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields: [&str; 6] = ["", "", "", "", "", ""];
        let mut n = 0usize;
        for field in line.split_whitespace() {
            if n >= fields.len() {
                break;
            }
            if let Some(slot) = fields.get_mut(n) {
                *slot = field;
            }
            n = n.saturating_add(1);
        }
        let key = fields.first().copied().unwrap_or("").to_ascii_uppercase();
        if key == "SET" {
            let value = fields.get(1).copied().unwrap_or("");
            if !value.eq_ignore_ascii_case("UTF-8") && !value.eq_ignore_ascii_case("UTF8") {
                return Err(Error::UnsupportedEncoding);
            }
            continue;
        }
        apply_line(&mut aff, &mut open, &key, &fields, n);
    }
    Ok(aff)
}

fn apply_line(aff: &mut Affix, open: &mut Option<Open>, key: &str, fields: &[&str; 6], n: usize) {
    match key {
        "TRY" => {
            if aff.try_chars.is_empty() {
                let raw = fields.get(1).copied().unwrap_or("");
                aff.try_chars = raw.chars().take(256).collect();
            }
        }
        "ICONV" => push_pair(&mut aff.iconv, fields, n, MAX_ICONV, false, false),
        "NOSUGGEST" => {
            if aff.nosuggest.is_none() {
                aff.nosuggest = one_char(fields.get(1).copied().unwrap_or(""));
            }
        }
        "COMPOUNDMIN" => {
            if let Ok(value) = fields.get(1).copied().unwrap_or("").parse::<usize>() {
                aff.compound_min = value.clamp(1, 64);
            }
        }
        "ONLYINCOMPOUND" => {
            if aff.only_in_compound.is_none() {
                aff.only_in_compound = one_char(fields.get(1).copied().unwrap_or(""));
            }
        }
        "COMPOUNDRULE" => push_pattern(aff, fields, n),
        "WORDCHARS" => {
            if aff.word_chars.is_empty() {
                let raw = fields.get(1).copied().unwrap_or("");
                aff.word_chars = raw.chars().take(256).collect();
            }
        }
        "PFX" => push_affix(aff, open, Kind::Prefix, fields, n),
        "SFX" => push_affix(aff, open, Kind::Suffix, fields, n),
        "REP" => push_pair(&mut aff.reps, fields, n, MAX_REPS, true, true),
        _ => {}
    }
}

fn one_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let c = chars.next()?;
    if chars.next().is_some() { None } else { Some(c) }
}

fn push_pair(out: &mut Vec<(String, String)>, fields: &[&str; 6], n: usize, cap: usize, underscore: bool, lower_from: bool) {
    if n < 3 || out.len() >= cap {
        return;
    }
    let from = fields.get(1).copied().unwrap_or("");
    let to = fields.get(2).copied().unwrap_or("");
    if from.is_empty() || from.chars().count() > 64 || to.chars().count() > 64 {
        return;
    }
    let from = if lower_from { crate::fold::to_lower(from) } else { from.to_string() };
    let to = if underscore { to.replace('_', " ") } else { to.to_string() };
    if from.is_empty() {
        return;
    }
    out.push((from, to));
}

fn push_pattern(aff: &mut Affix, fields: &[&str; 6], n: usize) {
    if n < 2 || aff.patterns.len() >= MAX_PATTERNS {
        return;
    }
    let raw = fields.get(1).copied().unwrap_or("");
    if raw.chars().all(|c| c.is_ascii_digit()) {
        return;
    }
    if let Some(pattern) = parse_pattern(raw) {
        aff.patterns.push(pattern);
    }
}

fn parse_pattern(raw: &str) -> Option<Vec<Pat>> {
    let chars: Vec<char> = raw.chars().take(65).collect();
    if chars.is_empty() || chars.len() > 64 {
        return None;
    }
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < chars.len() && out.len() < 32 {
        let flag = *chars.get(i)?;
        if flag == '*' || flag == '?' {
            return None;
        }
        i = i.saturating_add(1);
        let quant = match chars.get(i).copied() {
            Some('*') => {
                i = i.saturating_add(1);
                Quant::Star
            }
            Some('?') => {
                i = i.saturating_add(1);
                Quant::Opt
            }
            _ => Quant::One,
        };
        out.push(Pat { flag, quant });
    }
    if out.is_empty() { None } else { Some(out) }
}

fn push_affix(aff: &mut Affix, open: &mut Option<Open>, kind: Kind, fields: &[&str; 6], n: usize) {
    let rules = match kind {
        Kind::Prefix => &mut aff.prefixes,
        Kind::Suffix => &mut aff.suffixes,
    };
    if rules.len() >= MAX_RULES {
        return;
    }
    let flag = match one_char(fields.get(1).copied().unwrap_or("")) {
        Some(flag) => flag,
        None => return,
    };
    if n == 4 {
        let cross = fields.get(2).copied().unwrap_or("");
        let count = fields.get(3).copied().unwrap_or("");
        if (cross.eq_ignore_ascii_case("Y") || cross.eq_ignore_ascii_case("N")) && count.chars().all(|c| c.is_ascii_digit()) {
            *open = Some(Open { kind, flag, cross: cross.eq_ignore_ascii_case("Y") });
        }
        return;
    }
    if n < 5 {
        return;
    }
    let Some(group) = open.as_ref() else { return };
    if group.kind != kind || group.flag != flag {
        return;
    }
    let strip_raw = fields.get(2).copied().unwrap_or("");
    let add = fields.get(3).copied().unwrap_or("");
    let cond = fields.get(4).copied().unwrap_or("");
    if add.is_empty() || add == "0" || add.chars().count() > 64 || strip_raw.chars().count() > 64 {
        return;
    }
    let Some(condition) = parse_condition(cond) else { return };
    let strip = if strip_raw == "0" { String::new() } else { strip_raw.to_string() };
    rules.push(Rule { flag, cross: group.cross, strip, add: add.to_string(), condition });
}

fn parse_condition(raw: &str) -> Option<Condition> {
    let chars: Vec<char> = raw.chars().take(MAX_CONDITION.saturating_add(1)).collect();
    if chars.is_empty() || chars.len() > MAX_CONDITION {
        return None;
    }
    let mut i = 0usize;
    let mut atoms = Vec::new();
    while i < chars.len() {
        if atoms.len() >= MAX_ATOMS {
            return None;
        }
        let c = *chars.get(i)?;
        i = i.saturating_add(1);
        let atom = match c {
            '.' => Atom::Any,
            '[' => {
                let negated = chars.get(i).copied() == Some('^');
                if negated {
                    i = i.saturating_add(1);
                }
                let start = i;
                while i < chars.len() && chars.get(i).copied() != Some(']') {
                    i = i.saturating_add(1);
                }
                if chars.get(i).copied() != Some(']') || i <= start {
                    return None;
                }
                let class = chars.get(start..i)?.to_vec();
                i = i.saturating_add(1);
                Atom::Class { negated, chars: class }
            }
            ']' => return None,
            other => Atom::Char(other),
        };
        atoms.push(atom);
    }
    if atoms.is_empty() { None } else { Some(Condition { atoms }) }
}

pub(crate) fn surface_suffix(stem: &str, rule: &Rule) -> Option<String> {
    let lower = crate::fold::to_lower(stem);
    if !rule.condition.matches_end(&lower) {
        return None;
    }
    join_affix(stem, &rule.strip, &rule.add, false)
}

pub(crate) fn surface_prefix(stem: &str, rule: &Rule) -> Option<String> {
    let lower = crate::fold::to_lower(stem);
    if !rule.condition.matches_start(&lower) {
        return None;
    }
    join_affix(stem, &rule.strip, &rule.add, true)
}

pub(crate) fn surface_both(stem: &str, prefix: &Rule, suffix: &Rule) -> Option<String> {
    let lower = crate::fold::to_lower(stem);
    if !prefix.condition.matches_start(&lower) || !suffix.condition.matches_end(&lower) {
        return None;
    }
    let chars: Vec<char> = stem.chars().take(257).collect();
    let ps = prefix.strip.chars().count();
    let ss = suffix.strip.chars().count();
    if chars.len() < ps.saturating_add(ss) || chars.len() > 256 {
        return None;
    }
    let mut out = String::from(&prefix.add);
    let end = chars.len().saturating_sub(ss);
    for c in chars.iter().skip(ps).take(end.saturating_sub(ps)) {
        out.push(*c);
    }
    out.push_str(&suffix.add);
    Some(out)
}

fn join_affix(stem: &str, strip: &str, add: &str, prefix: bool) -> Option<String> {
    let chars: Vec<char> = stem.chars().take(257).collect();
    let n = strip.chars().count();
    if chars.len() < n || chars.len() > 256 {
        return None;
    }
    let mut out = String::new();
    if prefix {
        out.push_str(add);
        for c in chars.iter().skip(n) {
            out.push(*c);
        }
    } else {
        let keep = chars.len().saturating_sub(n);
        for c in chars.iter().take(keep) {
            out.push(*c);
        }
        out.push_str(add);
    }
    Some(out)
}
