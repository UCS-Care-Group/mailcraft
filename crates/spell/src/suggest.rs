// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Spelling suggestions: replacement table, one edit, then an n-gram shortlist
//! re-ranked by a weighted Damerau-Levenshtein distance.

use std::collections::HashMap;

use crate::dict::{Dictionary, grams_of, overlap};
use crate::fold::{self, CaseKind};

const MAX_OUT: usize = 6;
const MAX_CHARS: usize = 64;
const NGRAM_KEEP: usize = 80;
const NGRAM_WINDOW: usize = 3;

struct Sink<'a> {
    kind: CaseKind,
    input: &'a str,
    seen: Vec<String>,
    out: Vec<String>,
}

impl<'a> Sink<'a> {
    fn new(input: &'a str, kind: CaseKind) -> Self {
        Self { kind, input, seen: Vec::new(), out: Vec::new() }
    }

    fn full(&self) -> bool {
        self.out.len() >= MAX_OUT
    }

    fn push(&mut self, candidate: &str) {
        if self.full() || candidate.is_empty() || candidate.chars().count() > 80 {
            return;
        }
        let shown = fold::apply_case(self.kind, candidate);
        if shown.is_empty() || shown == self.input {
            return;
        }
        let key = fold::to_lower(&fold::fold_accents(&shown));
        if self.seen.iter().any(|have| have == &key) {
            return;
        }
        self.seen.push(key);
        self.out.push(shown);
    }
}

impl Dictionary {
    /// Up to six spellings for `word`. `extra` words (the owner's own list, the first 10_000)
    /// are candidates too.
    ///
    /// Words longer than 64 characters yield nothing: the search is bounded.
    /// A stem marked `NOSUGGEST` is never offered. The suggestion is the dictionary spelling,
    /// with the capitals of `word` applied. An accent on `word` is not copied onto it.
    /// A candidate that already has an internal capital keeps that spelling.
    pub fn suggest(&self, word: &str, extra: &[&str]) -> Vec<String> {
        let mut count = 0usize;
        for _ in word.chars() {
            count = count.saturating_add(1);
            if count > MAX_CHARS {
                return Vec::new();
            }
        }
        if count == 0 || !word.chars().any(|c| c.is_alphabetic()) {
            return Vec::new();
        }
        let kind = fold::case_kind(word);
        let lower_input = !word.chars().any(|c| c.is_uppercase());
        let (_original, _folded, lower) = self.owned_forms(word);
        let extras = Extras::new(extra);
        let mut sink = Sink::new(word, kind);
        self.suggest_rep(lower.as_str(), &mut sink);
        if !sink.full() {
            self.suggest_edits(lower.as_str(), &extras, lower_input, &mut sink);
        }
        if !sink.full() {
            self.suggest_ngram(lower.as_str(), &extras, lower_input, &mut sink);
        }
        sink.out
    }

    fn owned_forms(&self, word: &str) -> (String, String, String) {
        let (original, folded, lower) = self.prepare(word);
        (original.into_owned(), folded.into_owned(), lower.into_owned())
    }

    fn suggest_rep(&self, lower: &str, sink: &mut Sink<'_>) {
        let mut checked = 0u32;
        for (from, to) in self.reps() {
            if from.is_empty() {
                continue;
            }
            let mut start = 0usize;
            while start < lower.len() && checked < 512 {
                let Some(rest) = lower.get(start..) else { break };
                let Some(rel) = rest.find(from.as_str()) else { break };
                let at = start.saturating_add(rel);
                let end = at.saturating_add(from.len());
                if !lower.is_char_boundary(at) || !lower.is_char_boundary(end) {
                    break;
                }
                let mut replaced = String::with_capacity(lower.len().saturating_add(to.len()));
                replaced.push_str(lower.get(..at).unwrap_or(""));
                replaced.push_str(to);
                replaced.push_str(lower.get(end..).unwrap_or(""));
                checked = checked.saturating_add(1);
                if phrase_ok(self, &replaced) {
                    sink.push(&replaced);
                }
                let step = from.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                let next = at.saturating_add(step);
                if next <= start {
                    break;
                }
                start = next;
                if sink.full() {
                    return;
                }
            }
        }
    }

    fn suggest_edits(&self, lower: &str, extras: &Extras<'_>, lower_input: bool, sink: &mut Sink<'_>) {
        let chars: Vec<char> = lower.chars().take(MAX_CHARS).collect();
        if chars.is_empty() {
            return;
        }
        let mut tries = Vec::new();
        for ch in self.try_chars() {
            for low in ch.to_lowercase() {
                if !tries.contains(&low) {
                    tries.push(low);
                }
            }
        }
        let mut cands = Vec::new();
        self.keyboard_subs(&chars, extras, &mut cands);
        self.swaps(&chars, extras, &mut cands);
        self.deletes(&chars, extras, &mut cands);
        self.inserts(&chars, &tries, extras, &mut cands);
        self.substitutes(&chars, &tries, extras, &mut cands);
        self.splits(&chars, extras, &mut cands);
        cands.sort_by(|a, b| {
            rank_key(a, lower_input).cmp(&rank_key(b, lower_input)).then(b.overlap.cmp(&a.overlap)).then(a.word.len().cmp(&b.word.len()))
        });
        for cand in cands {
            sink.push(&cand.word);
            if sink.full() {
                return;
            }
        }
    }

    fn keyboard_subs(&self, chars: &[char], extras: &Extras<'_>, cands: &mut Vec<Ranked>) {
        for i in 0..chars.len() {
            let Some(&ch) = chars.get(i) else { continue };
            let mut neigh = ['\0'; 8];
            let n = neighbours(ch, &mut neigh);
            for slot in neigh.iter().take(n) {
                let word = replace_at(chars, i, *slot);
                self.consider(chars, word, COST_KEY, i == 0, extras, cands);
            }
        }
    }

    fn swaps(&self, chars: &[char], extras: &Extras<'_>, cands: &mut Vec<Ranked>) {
        if chars.len() < 2 {
            return;
        }
        for i in 0..chars.len().saturating_sub(1) {
            let (Some(&a), Some(&b)) = (chars.get(i), chars.get(i.saturating_add(1))) else { continue };
            if a == b {
                continue;
            }
            let mut word = String::with_capacity(chars.len());
            for (k, ch) in chars.iter().enumerate() {
                if k == i {
                    word.push(b);
                } else if k == i.saturating_add(1) {
                    word.push(a);
                } else {
                    word.push(*ch);
                }
            }
            self.consider(chars, word, COST_SWAP, i == 0, extras, cands);
        }
    }

    fn deletes(&self, chars: &[char], extras: &Extras<'_>, cands: &mut Vec<Ranked>) {
        if chars.len() < 2 {
            return;
        }
        for i in 0..chars.len() {
            let deleted = chars.get(i).copied().unwrap_or('\0');
            let prev = if i > 0 { chars.get(i.saturating_sub(1)).copied() } else { None };
            let next = chars.get(i.saturating_add(1)).copied();
            let cost = if i == 0 {
                COST_DELETE_FIRST
            } else if prev == Some(deleted) || next == Some(deleted) {
                COST_DOUBLE_DEL
            } else {
                COST_DELETE
            };
            let mut word = String::with_capacity(chars.len());
            for (k, ch) in chars.iter().enumerate() {
                if k != i {
                    word.push(*ch);
                }
            }
            self.consider(chars, word, cost, i == 0, extras, cands);
        }
    }

    fn inserts(&self, chars: &[char], tries: &[char], extras: &Extras<'_>, cands: &mut Vec<Ranked>) {
        for i in 0..=chars.len() {
            for ch in tries {
                let prev = if i > 0 { chars.get(i.saturating_sub(1)).copied() } else { None };
                let next = chars.get(i).copied();
                let cost = if prev == Some(*ch) || next == Some(*ch) { COST_DOUBLE_INS } else { COST_INSERT };
                let word = insert_at(chars, i, *ch);
                self.consider(chars, word, cost, i == 0, extras, cands);
            }
        }
    }

    fn substitutes(&self, chars: &[char], tries: &[char], extras: &Extras<'_>, cands: &mut Vec<Ranked>) {
        for i in 0..chars.len() {
            let Some(&current) = chars.get(i) else { continue };
            for ch in tries {
                if ch.eq_ignore_ascii_case(&current) || neighbour_of(current, *ch) {
                    continue;
                }
                let word = replace_at(chars, i, *ch);
                self.consider(chars, word, COST_SUB, i == 0, extras, cands);
            }
        }
    }

    fn splits(&self, chars: &[char], extras: &Extras<'_>, cands: &mut Vec<Ranked>) {
        if chars.len() < 4 {
            return;
        }
        for i in 1..chars.len() {
            let left: String = chars.iter().take(i).collect();
            let right: String = chars.iter().skip(i).collect();
            if !split_piece(&left) || !split_piece(&right) {
                continue;
            }
            if part_ok(self, &left, extras) && part_ok(self, &right, extras) {
                let mut phrase = left;
                phrase.push(' ');
                phrase.push_str(&right);
                self.consider(chars, phrase, COST_SPLIT, false, extras, cands);
            }
        }
    }

    fn consider(&self, typo: &[char], word: String, cost: u32, first_changed: bool, extras: &Extras<'_>, cands: &mut Vec<Ranked>) {
        let phrase = word.contains(' ');
        if word.is_empty() {
            return;
        }
        // An edit that exists only in the owner's list is stored under its own spelling.
        // The generated candidate is lowercase (`mailcraft`); offering that and then the
        // stored `MailCraft` drops the second as a duplicate.
        let word = if phrase || self.suggestable(&word) {
            word
        } else {
            match extras.display(&word) {
                Some(shown) => shown.to_string(),
                None => return,
            }
        };
        let letters: Vec<char> = word.chars().filter(|c| *c != ' ').collect();
        let overlap = shared_grams(typo, &letters);
        let capital = u8::from(word.chars().any(|c| c.is_uppercase()));
        if let Some(existing) = cands.iter_mut().find(|cand| cand.word == word) {
            if (cost, u8::from(first_changed)) < (existing.cost, existing.first_changed) {
                existing.cost = cost;
                existing.first_changed = u8::from(first_changed);
                existing.overlap = overlap;
                existing.capital = capital;
            }
            return;
        }
        cands.push(Ranked { cost, first_changed: u8::from(first_changed), capital, overlap, word });
    }

    fn suggest_ngram(&self, lower: &str, extras: &Extras<'_>, lower_input: bool, sink: &mut Sink<'_>) {
        let chars: Vec<char> = lower.chars().take(MAX_CHARS).collect();
        let len = chars.len();
        if len == 0 {
            return;
        }
        let (query_bi, query_tri) = grams_of(lower);
        let mut scored: Vec<(i32, u32)> = Vec::new();
        let lo = len.saturating_sub(NGRAM_WINDOW);
        let hi = len.saturating_add(NGRAM_WINDOW).min(64);
        for bucket in lo..=hi {
            for &id in self.ids_of_len(bucket) {
                let bi = overlap(&query_bi, self.bigrams(id));
                let tri = overlap(&query_tri, self.trigrams(id));
                let diff = (bucket as i32).saturating_sub(len as i32).unsigned_abs() as i32;
                let score = (bi as i32).saturating_mul(2).saturating_add((tri as i32).saturating_mul(3)).saturating_sub(diff.saturating_mul(2));
                if score > 0 {
                    scored.push((score, id));
                }
            }
        }
        if scored.len() > NGRAM_KEEP {
            scored.select_nth_unstable_by(NGRAM_KEEP, |a, b| b.0.cmp(&a.0));
            scored.truncate(NGRAM_KEEP);
        }
        scored.sort_unstable_by_key(|item| std::cmp::Reverse(item.0));
        let limit = distance_limit(len);
        let mut ranked: Vec<Near> = Vec::new();
        let mut forms = Vec::new();
        for &(score, id) in &scored {
            forms.clear();
            self.expansions(id, &mut forms);
            for form in &forms {
                let distance = weighted_distance(&chars, form);
                if distance <= limit {
                    ranked.push(Near::new(distance, score, form.clone()));
                }
            }
        }
        let mut extra_scored: Vec<(i32, String, &str)> = Vec::new();
        for (key, display) in &extras.by_key {
            let elen = key.chars().count();
            if elen == 0 || elen > MAX_CHARS {
                continue;
            }
            let diff = (elen as i32).saturating_sub(len as i32).unsigned_abs();
            if usize::try_from(diff).unwrap_or(usize::MAX) > NGRAM_WINDOW {
                continue;
            }
            let (bi, tri) = grams_of(key);
            let score = (overlap(&query_bi, &bi) as i32)
                .saturating_mul(2)
                .saturating_add((overlap(&query_tri, &tri) as i32).saturating_mul(3))
                .saturating_sub((diff as i32).saturating_mul(2));
            if score > 0 {
                extra_scored.push((score, key.clone(), *display));
            }
        }
        extra_scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(b.2)));
        if extra_scored.len() > NGRAM_KEEP {
            extra_scored.truncate(NGRAM_KEEP);
        }
        for (score, key, display) in extra_scored {
            let distance = weighted_distance(&chars, &key);
            if distance <= limit {
                ranked.push(Near::new(distance, score, display.to_string()));
            }
        }
        ranked.sort_by(|a, b| near_key(a, lower_input).cmp(&near_key(b, lower_input)).then(b.score.cmp(&a.score)));
        for near in ranked {
            let word = near.word;
            sink.push(&word);
            if sink.full() {
                break;
            }
        }
    }
}

/// Cheaper costs are the typos a person is more likely to make.
const COST_DOUBLE_INS: u32 = 10;
const COST_DOUBLE_DEL: u32 = 15;
const COST_SWAP: u32 = 20;
const COST_INSERT: u32 = 30;
const COST_KEY: u32 = 40;
const COST_DELETE: u32 = 55;
const COST_DELETE_FIRST: u32 = 70;
const COST_SUB: u32 = 80;
const COST_SPLIT: u32 = 200;

struct Ranked {
    cost: u32,
    first_changed: u8,
    capital: u8,
    overlap: u32,
    word: String,
}

fn rank_key(cand: &Ranked, lower_input: bool) -> (u32, u8, u8) {
    let capital = if lower_input { cand.capital } else { 0 };
    (cand.cost.saturating_add(u32::from(capital)), cand.first_changed, capital)
}

struct Near {
    distance: u32,
    capital: u8,
    score: i32,
    word: String,
}

impl Near {
    fn new(distance: u32, score: i32, word: String) -> Self {
        let capital = u8::from(word.chars().any(|c| c.is_uppercase()));
        Self { distance, capital, score, word }
    }
}

/// A capitalised stem sorts just after lowercase words at the same distance and the next one.
fn near_key(item: &Near, lower_input: bool) -> (u32, u8) {
    let capital = if lower_input { item.capital } else { 0 };
    (item.distance.saturating_add(u32::from(capital)), capital)
}

/// `a` and `I` may stand alone. Anything shorter than three letters is an affix fragment
/// (`ed`, `ca`, `l`) and is not a split.
fn split_piece(part: &str) -> bool {
    let mut n = 0usize;
    for _ in part.chars() {
        n = n.saturating_add(1);
        if n >= 3 {
            return true;
        }
    }
    n == 1 && (part == "a" || part == "i")
}

fn shared_grams(a: &[char], b: &[char]) -> u32 {
    let mut score = 0u32;
    if a.len() >= 2 && b.len() >= 2 {
        for i in 0..a.len().saturating_sub(1) {
            let (Some(a0), Some(a1)) = (a.get(i).copied(), a.get(i.saturating_add(1)).copied()) else { continue };
            for j in 0..b.len().saturating_sub(1) {
                if b.get(j).copied() == Some(a0) && b.get(j.saturating_add(1)).copied() == Some(a1) {
                    score = score.saturating_add(1);
                }
            }
        }
    }
    if a.len() >= 3 && b.len() >= 3 {
        for i in 0..a.len().saturating_sub(2) {
            let (Some(a0), Some(a1), Some(a2)) = (a.get(i).copied(), a.get(i.saturating_add(1)).copied(), a.get(i.saturating_add(2)).copied()) else {
                continue;
            };
            for j in 0..b.len().saturating_sub(2) {
                if b.get(j).copied() == Some(a0) && b.get(j.saturating_add(1)).copied() == Some(a1) && b.get(j.saturating_add(2)).copied() == Some(a2)
                {
                    score = score.saturating_add(3);
                }
            }
        }
    }
    score
}

fn phrase_ok(dict: &Dictionary, phrase: &str) -> bool {
    let mut any = false;
    for part in phrase.split(' ') {
        if part.is_empty() {
            continue;
        }
        any = true;
        if !dict.suggestable(part) {
            return false;
        }
    }
    any
}

fn part_ok(dict: &Dictionary, word: &str, extras: &Extras<'_>) -> bool {
    dict.suggestable(word) || extras.has(word)
}

/// The owner's own words, indexed once per suggestion so a list of thousands stays a lookup.
struct Extras<'a> {
    by_key: HashMap<String, &'a str>,
}

impl<'a> Extras<'a> {
    const MAX_EXTRA: usize = 10_000;

    fn new(extra: &'a [&str]) -> Self {
        let mut by_key = HashMap::new();
        for word in extra.iter().take(Self::MAX_EXTRA) {
            if word.is_empty() {
                continue;
            }
            let mut count = 0usize;
            for _ in word.chars() {
                count = count.saturating_add(1);
                if count > MAX_CHARS {
                    break;
                }
            }
            if count == 0 || count > MAX_CHARS {
                continue;
            }
            let key = fold::to_lower(&fold::fold_accents(word));
            by_key.entry(key).or_insert(*word);
        }
        Self { by_key }
    }

    fn has(&self, word: &str) -> bool {
        self.display(word).is_some()
    }

    /// The spelling the owner saved, when `word` is the same word in any case.
    fn display(&self, word: &str) -> Option<&'a str> {
        if self.by_key.is_empty() {
            return None;
        }
        let key = fold::to_lower(&fold::fold_accents(word));
        self.by_key.get(&key).copied()
    }
}

fn replace_at(chars: &[char], index: usize, new: char) -> String {
    let mut out = String::with_capacity(chars.len());
    for (k, ch) in chars.iter().enumerate() {
        out.push(if k == index { new } else { *ch });
    }
    out
}

fn insert_at(chars: &[char], index: usize, new: char) -> String {
    let mut out = String::with_capacity(chars.len().saturating_add(1));
    for (k, ch) in chars.iter().enumerate() {
        if k == index {
            out.push(new);
        }
        out.push(*ch);
    }
    if index >= chars.len() {
        out.push(new);
    }
    out
}

fn distance_limit(len: usize) -> u32 {
    if len <= 3 {
        2
    } else if len <= 5 {
        3
    } else {
        4
    }
}

fn weighted_distance(query: &[char], candidate: &str) -> u32 {
    let mut other = Vec::with_capacity(query.len().saturating_add(4));
    for c in candidate.chars().take(80) {
        if c.is_ascii_uppercase() {
            other.push(c.to_ascii_lowercase());
        } else {
            for low in c.to_lowercase() {
                other.push(low);
                if other.len() >= 80 {
                    break;
                }
            }
        }
        if other.len() >= 80 {
            break;
        }
    }
    damerau(query, &other, distance_limit(query.len()))
}

fn damerau(a: &[char], b: &[char], limit: u32) -> u32 {
    let n = a.len();
    let m = b.len();
    if n > 80 || m > 80 {
        return limit.saturating_add(1);
    }
    let mut prev2 = vec![0u32; m.saturating_add(1)];
    let mut prev = vec![0u32; m.saturating_add(1)];
    let mut cur = vec![0u32; m.saturating_add(1)];
    for j in 0..=m {
        if let Some(slot) = prev.get_mut(j) {
            *slot = (j as u32).saturating_mul(1);
        }
    }
    for i in 1..=n {
        if let Some(slot) = cur.get_mut(0) {
            *slot = (i as u32).saturating_mul(1);
        }
        let mut row_min = u32::MAX;
        for j in 1..=m {
            let ac = a.get(i.saturating_sub(1)).copied().unwrap_or('\0');
            let bc = b.get(j.saturating_sub(1)).copied().unwrap_or('\0');
            let sub = if ac == bc {
                0
            } else if neighbour_of(ac, bc) {
                1
            } else {
                2
            };
            let del = prev.get(j).copied().unwrap_or(limit).saturating_add(1);
            let ins = cur.get(j.saturating_sub(1)).copied().unwrap_or(limit).saturating_add(1);
            let rep = prev.get(j.saturating_sub(1)).copied().unwrap_or(limit).saturating_add(sub);
            let mut best = del.min(ins).min(rep);
            if i > 1 && j > 1 {
                let a_prev = a.get(i.saturating_sub(2)).copied().unwrap_or('\0');
                let b_prev = b.get(j.saturating_sub(2)).copied().unwrap_or('\0');
                if ac == b_prev && a_prev == bc {
                    let swap = prev2.get(j.saturating_sub(2)).copied().unwrap_or(limit).saturating_add(1);
                    best = best.min(swap);
                }
            }
            if let Some(slot) = cur.get_mut(j) {
                *slot = best;
            }
            row_min = row_min.min(best);
        }
        if row_min > limit {
            return limit.saturating_add(1);
        }
        prev2.clone_from(&prev);
        prev.clone_from(&cur);
    }
    prev.get(m).copied().unwrap_or(limit.saturating_add(1))
}

const ROWS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];

fn neighbours(ch: char, out: &mut [char; 8]) -> usize {
    let ch = ch.to_ascii_lowercase();
    let mut n = 0usize;
    let Some((row, col)) = key_pos(ch) else { return 0 };
    let mut push = |c: Option<char>| {
        if let Some(c) = c
            && n < out.len()
            && !out[..n].contains(&c)
            && let Some(slot) = out.get_mut(n)
        {
            *slot = c;
            n = n.saturating_add(1);
        }
    };
    push(key_at(row, col.wrapping_sub(1)));
    push(key_at(row, col.saturating_add(1)));
    if row > 0 {
        push(key_at(row - 1, col.wrapping_sub(1)));
        push(key_at(row - 1, col));
    }
    push(key_at(row.saturating_add(1), col));
    push(key_at(row.saturating_add(1), col.saturating_add(1)));
    n
}

fn neighbour_of(a: char, b: char) -> bool {
    let mut buf = ['\0'; 8];
    let n = neighbours(a, &mut buf);
    buf.iter().take(n).any(|c| *c == b.to_ascii_lowercase())
}

fn key_pos(ch: char) -> Option<(usize, usize)> {
    for (row, keys) in ROWS.iter().enumerate() {
        if let Some(col) = keys.find(ch) {
            return Some((row, col));
        }
    }
    None
}

fn key_at(row: usize, col: usize) -> Option<char> {
    ROWS.get(row)?.chars().nth(col)
}
