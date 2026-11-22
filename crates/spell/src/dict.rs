// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dictionary words, affix lookup and compound patterns.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::affix::{self, Affix, Pat, Quant, Rule};
use crate::error::Error;
use crate::fold::{case_ok, fold_accents, to_lower};

const MAX_ENTRIES: usize = 1_000_000;
const MAX_WORD: usize = 256;
const MAX_FLAGS: usize = 64;
const MAX_GRAMS: usize = 48;
const MAX_COMPOUND_CHARS: usize = 128;
const COMPOUND_STEPS: u32 = 20_000;

struct Stem {
    text: String,
    flag_off: u32,
    flag_len: u8,
    len: u16,
}

struct Group {
    add: String,
    rules: Vec<u16>,
}

struct Node {
    edge_ch: Vec<char>,
    edge_to: Vec<u32>,
    ends: Vec<u32>,
}

pub struct Dictionary {
    stems: Vec<Stem>,
    flag_arena: String,
    by_lower: HashMap<String, Vec<u32>>,
    suffix_groups: Vec<Group>,
    prefixes: Vec<Rule>,
    suffixes: Vec<Rule>,
    reps: Vec<(String, String)>,
    try_chars: Vec<char>,
    iconv: Vec<(String, String)>,
    word_chars: Vec<char>,
    compound_min: usize,
    only_in_compound: Option<char>,
    nosuggest: Option<char>,
    patterns: Vec<Vec<Pat>>,
    nodes: Vec<Node>,
    by_len: Vec<Vec<u32>>,
    bi_off: Vec<u32>,
    bi_data: Vec<u16>,
    tri_off: Vec<u32>,
    tri_data: Vec<u16>,
}

impl Dictionary {
    pub fn parse(aff: &str, dic: &str) -> Result<Self, Error> {
        let aff = affix::parse(aff)?;
        let mut dict = Self::from_aff(aff);
        dict.read_dic(dic);
        dict.finish();
        Ok(dict)
    }

    fn from_aff(aff: Affix) -> Self {
        Self {
            stems: Vec::new(),
            flag_arena: String::new(),
            by_lower: HashMap::new(),
            suffix_groups: Vec::new(),
            prefixes: aff.prefixes,
            suffixes: aff.suffixes,
            reps: aff.reps,
            try_chars: aff.try_chars,
            iconv: aff.iconv,
            word_chars: aff.word_chars,
            compound_min: aff.compound_min,
            only_in_compound: aff.only_in_compound,
            nosuggest: aff.nosuggest,
            patterns: aff.patterns,
            nodes: Vec::new(),
            by_len: Vec::new(),
            bi_off: Vec::new(),
            bi_data: Vec::new(),
            tri_off: Vec::new(),
            tri_data: Vec::new(),
        }
    }

    fn read_dic(&mut self, dic: &str) {
        let mut started = false;
        for raw in dic.strip_prefix('\u{feff}').unwrap_or(dic).lines() {
            if self.stems.len() >= MAX_ENTRIES {
                break;
            }
            if raw.len() > 4096 {
                continue;
            }
            let line = raw.trim_matches(['\r', '\n']).trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let head = line.split_whitespace().next().unwrap_or("");
            if head.is_empty() {
                continue;
            }
            if !started {
                started = true;
                if head.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
            }
            let (word, flags) = head.split_once('/').unwrap_or((head, ""));
            if word.is_empty() || word.chars().count() > MAX_WORD {
                continue;
            }
            self.insert_stem(word, flags);
        }
    }

    fn insert_stem(&mut self, word: &str, flags: &str) {
        let lower = to_lower(word);
        if lower.chars().count() > MAX_WORD {
            return;
        }
        // Queries fold accents before case (`İstanbul` → `istanbul`). Lowercasing
        // first expands U+0130 to `i` plus a dot, which folding does not remove,
        // so the entry would miss the key a query builds. ASCII `I` is one
        // character either way. `stem.text` stays the spelling written in the file.
        let key = to_lower(&fold_accents(word));
        if key.chars().count() > MAX_WORD {
            return;
        }
        let mut found = None;
        if let Some(ids) = self.by_lower.get(&key) {
            for &id in ids {
                if self.stems.get(id as usize).is_some_and(|stem| stem.text == word) {
                    found = Some(id);
                    break;
                }
            }
        }
        let cleaned = fit_flags(clean_flags(flags));
        if let Some(id) = found {
            self.merge_flags(id, &cleaned);
            return;
        }
        if self.by_lower.get(&key).is_some_and(|ids| ids.len() >= 16) {
            return;
        }
        let flag_off = u32::try_from(self.flag_arena.len()).unwrap_or(u32::MAX);
        if flag_off == u32::MAX {
            return;
        }
        self.flag_arena.push_str(&cleaned);
        let flag_len = u8::try_from(cleaned.len()).unwrap_or(255);
        let len = u16::try_from(lower.chars().count()).unwrap_or(u16::MAX);
        let id = u32::try_from(self.stems.len()).unwrap_or(u32::MAX);
        if id == u32::MAX {
            return;
        }
        self.stems.push(Stem { text: word.to_string(), flag_off, flag_len, len });
        self.by_lower.entry(key).or_default().push(id);
    }

    fn merge_flags(&mut self, id: u32, extra: &str) {
        let Some(stem) = self.stems.get(id as usize) else { return };
        let mut combined = stem_flags(&self.flag_arena, stem).to_string();
        for c in extra.chars() {
            if combined.chars().count() >= MAX_FLAGS {
                break;
            }
            if !combined.chars().any(|have| have == c) {
                combined.push(c);
            }
        }
        let combined = fit_flags(combined);
        let Some(stem) = self.stems.get_mut(id as usize) else { return };
        let off = u32::try_from(self.flag_arena.len()).unwrap_or(u32::MAX);
        if off == u32::MAX {
            return;
        }
        self.flag_arena.push_str(&combined);
        stem.flag_off = off;
        stem.flag_len = u8::try_from(combined.len()).unwrap_or(255);
    }

    fn finish(&mut self) {
        self.index_suffixes();
        self.index_ngrams();
        self.index_compounds();
    }

    fn index_suffixes(&mut self) {
        for (index, rule) in self.suffixes.iter().enumerate() {
            let Some(slot) = u16::try_from(index).ok() else { continue };
            if let Some(group) = self.suffix_groups.iter_mut().find(|group| group.add == rule.add) {
                group.rules.push(slot);
            } else {
                self.suffix_groups.push(Group { add: rule.add.clone(), rules: vec![slot] });
            }
        }
    }

    fn index_ngrams(&mut self) {
        self.by_len = vec![Vec::new(); 65];
        self.bi_off = Vec::with_capacity(self.stems.len().saturating_add(1));
        self.tri_off = Vec::with_capacity(self.stems.len().saturating_add(1));
        self.bi_off.push(0);
        self.tri_off.push(0);
        for (id, stem) in self.stems.iter().enumerate() {
            let len = usize::from(stem.len);
            if len < self.by_len.len()
                && let (Some(bucket), Some(slot)) = (self.by_len.get_mut(len), u32::try_from(id).ok())
            {
                bucket.push(slot);
            }
            let lower = to_lower(&stem.text);
            let (bi, tri) = grams_of(&lower);
            self.bi_data.extend(bi);
            self.tri_data.extend(tri);
            self.bi_off.push(u32::try_from(self.bi_data.len()).unwrap_or(u32::MAX));
            self.tri_off.push(u32::try_from(self.tri_data.len()).unwrap_or(u32::MAX));
        }
    }

    fn index_compounds(&mut self) {
        self.nodes.clear();
        self.nodes.push(Node { edge_ch: Vec::new(), edge_to: Vec::new(), ends: Vec::new() });
        if self.patterns.is_empty() {
            return;
        }
        let mut flags = Vec::new();
        for pattern in &self.patterns {
            for elem in pattern {
                if !flags.contains(&elem.flag) {
                    flags.push(elem.flag);
                }
            }
        }
        let mut pending = Vec::new();
        for (id, stem) in self.stems.iter().enumerate() {
            let text = stem_flags(&self.flag_arena, stem);
            if !flags.iter().any(|flag| text.chars().any(|c| c == *flag)) {
                continue;
            }
            let Some(slot) = u32::try_from(id).ok() else { continue };
            pending.push((to_lower(&stem.text), slot));
        }
        for (lower, slot) in pending {
            self.trie_insert(&lower, slot);
        }
    }

    fn trie_insert(&mut self, lower: &str, id: u32) {
        let mut node = 0u32;
        for ch in lower.chars().take(MAX_WORD) {
            node = self.trie_edge(node, ch);
        }
        let Some(slot) = self.nodes.get_mut(node as usize) else { return };
        if slot.ends.len() < 8 {
            slot.ends.push(id);
        }
    }

    fn trie_edge(&mut self, node: u32, ch: char) -> u32 {
        if let Some(existing) = self.nodes.get(node as usize).and_then(|n| edge_to(n, ch)) {
            return existing;
        }
        let next = u32::try_from(self.nodes.len()).unwrap_or(u32::MAX);
        if next == u32::MAX {
            return node;
        }
        self.nodes.push(Node { edge_ch: Vec::new(), edge_to: Vec::new(), ends: Vec::new() });
        let Some(slot) = self.nodes.get_mut(node as usize) else { return node };
        let pos = slot.edge_ch.iter().position(|have| *have > ch).unwrap_or(slot.edge_ch.len());
        slot.edge_ch.insert(pos, ch);
        slot.edge_to.insert(pos, next);
        next
    }

    pub fn is_correct(&self, word: &str) -> bool {
        self.judge(word).0
    }

    pub(crate) fn suggestable(&self, word: &str) -> bool {
        self.judge(word).1
    }

    fn judge(&self, word: &str) -> (bool, bool) {
        if word.len() > 4096 {
            return (false, false);
        }
        let (original, folded, lower) = self.prepare(word);
        if lower.chars().count() > MAX_WORD {
            return self.note_exact(original.as_ref(), folded.as_ref(), lower.as_ref());
        }
        if !original.contains('-') {
            return self.judge_token(original.as_ref(), folded.as_ref(), lower.as_ref());
        }
        let whole = self.judge_token(original.as_ref(), folded.as_ref(), lower.as_ref());
        // A recognised hyphenated entry keeps its own flags. Splitting `cat-dog/!`
        // would see `cat` and `dog` and offer a word the list marked NOSUGGEST.
        if whole.0 {
            return whole;
        }
        let mut any = false;
        let mut correct = true;
        let mut suggestable = true;
        for part in original.split('-') {
            if part.is_empty() {
                continue;
            }
            any = true;
            let (part_word, part_folded, part_lower) = self.prepare(part);
            let hit = self.judge_token(part_word.as_ref(), part_folded.as_ref(), part_lower.as_ref());
            if !hit.0 {
                correct = false;
                suggestable = false;
                break;
            }
            if !hit.1 {
                suggestable = false;
            }
        }
        if any && correct { (true, whole.1 || suggestable) } else { whole }
    }

    pub(crate) fn prepare<'a>(&'a self, word: &'a str) -> (Cow<'a, str>, Cow<'a, str>, Cow<'a, str>) {
        if self.needs_iconv(word) {
            let changed = apply_iconv(&self.iconv, word);
            let folded = fold_accents(&changed);
            let lower = to_lower(&folded);
            return (Cow::Owned(changed), Cow::Owned(folded), Cow::Owned(lower));
        }
        if word.is_ascii() {
            if word.bytes().all(|b| !b.is_ascii_uppercase()) {
                return (Cow::Borrowed(word), Cow::Borrowed(word), Cow::Borrowed(word));
            }
            return (Cow::Borrowed(word), Cow::Borrowed(word), Cow::Owned(word.to_ascii_lowercase()));
        }
        let folded = fold_accents(word);
        let lower = to_lower(&folded);
        (Cow::Owned(word.to_string()), Cow::Owned(folded), Cow::Owned(lower))
    }

    fn needs_iconv(&self, word: &str) -> bool {
        if self.iconv.is_empty() || (word.is_ascii() && self.iconv.iter().all(|(from, _)| !from.is_ascii())) {
            return false;
        }
        self.iconv.iter().any(|(from, _)| !from.is_empty() && word.contains(from))
    }

    fn judge_token(&self, original: &str, folded: &str, lower: &str) -> (bool, bool) {
        let mut correct = false;
        let mut suggestable = false;
        self.note_exact_into(original, folded, lower, &mut correct, &mut suggestable);
        if suggestable {
            return (true, true);
        }
        self.note_suffixes(original, folded, lower, &mut correct, &mut suggestable);
        if suggestable {
            return (true, true);
        }
        self.note_prefixes(original, folded, lower, &mut correct, &mut suggestable);
        if suggestable {
            return (true, true);
        }
        self.note_cross(original, folded, lower, &mut correct, &mut suggestable);
        if suggestable {
            return (true, true);
        }
        if let Some(clean) = self.compound_hit(original, folded, lower) {
            correct = true;
            suggestable = suggestable || clean;
        }
        (correct, suggestable)
    }

    fn note_exact(&self, original: &str, folded: &str, lower: &str) -> (bool, bool) {
        let mut correct = false;
        let mut suggestable = false;
        self.note_exact_into(original, folded, lower, &mut correct, &mut suggestable);
        (correct, suggestable)
    }

    fn note_exact_into(&self, original: &str, folded: &str, lower: &str, correct: &mut bool, suggestable: &mut bool) {
        let Some(ids) = self.by_lower.get(lower) else { return };
        for &id in ids.iter().take(16) {
            let Some(stem) = self.stems.get(id as usize) else { continue };
            if flagged(&self.flag_arena, stem, self.only_in_compound) {
                continue;
            }
            if !case_ok(&stem.text, original, folded) {
                continue;
            }
            *correct = true;
            if !flagged(&self.flag_arena, stem, self.nosuggest) {
                *suggestable = true;
            }
        }
    }

    fn note_suffixes(&self, original: &str, folded: &str, lower: &str, correct: &mut bool, suggestable: &mut bool) {
        let groups = self.suffix_groups.len();
        for gi in 0..groups {
            let hit = self.suffix_groups.get(gi).is_some_and(|group| !group.add.is_empty() && lower.ends_with(group.add.as_str()));
            if !hit {
                continue;
            }
            let Some(add) = self.suffix_groups.get(gi).map(|group| group.add.clone()) else { continue };
            let Some(base) = lower.strip_suffix(&add) else { continue };
            let rules = self.suffix_groups.get(gi).map(|group| group.rules.clone()).unwrap_or_default();
            for ri in rules {
                let Some(rule) = self.suffixes.get(usize::from(ri)).cloned() else { continue };
                let stem_l = joined(base, &rule.strip);
                if !rule.condition.matches_end(&stem_l) {
                    continue;
                }
                let flag = rule.flag;
                self.hit_stem(&stem_l, &[flag], original, folded, correct, suggestable, |text| affix::surface_suffix(text, &rule));
                if *suggestable {
                    return;
                }
            }
        }
    }

    fn note_prefixes(&self, original: &str, folded: &str, lower: &str, correct: &mut bool, suggestable: &mut bool) {
        let count = self.prefixes.len();
        for i in 0..count {
            let hit = self.prefixes.get(i).is_some_and(|rule| !rule.add.is_empty() && lower.starts_with(rule.add.as_str()));
            if !hit {
                continue;
            }
            let Some(rule) = self.prefixes.get(i).cloned() else { continue };
            let Some(base) = lower.strip_prefix(&rule.add) else { continue };
            let stem_l = prefixed(&rule.strip, base);
            if !rule.condition.matches_start(&stem_l) {
                continue;
            }
            let flag = rule.flag;
            self.hit_stem(&stem_l, &[flag], original, folded, correct, suggestable, |text| affix::surface_prefix(text, &rule));
            if *suggestable {
                return;
            }
        }
    }

    fn note_cross(&self, original: &str, folded: &str, lower: &str, correct: &mut bool, suggestable: &mut bool) {
        let prefixes = self.prefixes.len();
        let groups = self.suffix_groups.len();
        for pi in 0..prefixes {
            let hit = self.prefixes.get(pi).is_some_and(|prefix| prefix.cross && !prefix.add.is_empty() && lower.starts_with(prefix.add.as_str()));
            if !hit {
                continue;
            }
            let Some(prefix) = self.prefixes.get(pi).cloned() else { continue };
            let Some(after) = lower.strip_prefix(&prefix.add) else { continue };
            for gi in 0..groups {
                let suffix_hit = self.suffix_groups.get(gi).is_some_and(|group| !group.add.is_empty() && after.ends_with(group.add.as_str()));
                if !suffix_hit {
                    continue;
                }
                let Some(add) = self.suffix_groups.get(gi).map(|group| group.add.clone()) else { continue };
                let Some(mid) = after.strip_suffix(&add) else { continue };
                let rules = self.suffix_groups.get(gi).map(|group| group.rules.clone()).unwrap_or_default();
                for ri in rules {
                    let Some(suffix) = self.suffixes.get(usize::from(ri)).cloned() else { continue };
                    if !suffix.cross {
                        continue;
                    }
                    let stem_l = cross_stem(&prefix.strip, mid, &suffix.strip);
                    if !prefix.condition.matches_start(&stem_l) || !suffix.condition.matches_end(&stem_l) {
                        continue;
                    }
                    let flags = [prefix.flag, suffix.flag];
                    self.hit_stem(&stem_l, &flags, original, folded, correct, suggestable, |text| affix::surface_both(text, &prefix, &suffix));
                    if *suggestable {
                        return;
                    }
                }
            }
        }
    }

    fn hit_stem(
        &self,
        stem_lower: &str,
        flags: &[char],
        original: &str,
        folded: &str,
        correct: &mut bool,
        suggestable: &mut bool,
        surface_of: impl Fn(&str) -> Option<String>,
    ) {
        let Some(ids) = self.by_lower.get(stem_lower) else { return };
        for &id in ids.iter().take(16) {
            let Some(stem) = self.stems.get(id as usize) else { continue };
            if flagged(&self.flag_arena, stem, self.only_in_compound) {
                continue;
            }
            if !flags.iter().all(|flag| flag_has(&self.flag_arena, stem, *flag)) {
                continue;
            }
            let Some(surface) = surface_of(&stem.text) else { continue };
            if !case_ok(&surface, original, folded) {
                continue;
            }
            *correct = true;
            if !flagged(&self.flag_arena, stem, self.nosuggest) {
                *suggestable = true;
            }
        }
    }

    fn compound_hit(&self, original: &str, folded: &str, lower: &str) -> Option<bool> {
        if self.patterns.is_empty() || self.nodes.len() <= 1 {
            return None;
        }
        let chars: Vec<char> = lower.chars().take(MAX_COMPOUND_CHARS.saturating_add(1)).collect();
        if chars.len() < 2 || chars.len() > MAX_COMPOUND_CHARS {
            return None;
        }
        let jumps = self.compound_jumps(&chars);
        let mut steps = 0u32;
        let mut found: Option<bool> = None;
        for pattern in &self.patterns {
            if let Some(clean) = self.match_pattern(&jumps, pattern, 0, 0, 0, true, &mut steps)
                && case_ok(lower, original, folded)
            {
                found = or_opt(found, Some(clean));
                if found == Some(true) {
                    return found;
                }
            }
            if steps > COMPOUND_STEPS {
                break;
            }
        }
        found
    }

    fn compound_jumps(&self, chars: &[char]) -> Vec<Vec<(usize, u32)>> {
        let mut jumps = vec![Vec::new(); chars.len()];
        for start in 0..chars.len() {
            let mut node = 0u32;
            for (offset, ch) in chars.iter().enumerate().skip(start) {
                let Some(next) = self.nodes.get(node as usize).and_then(|n| edge_to(n, *ch)) else { break };
                node = next;
                let end = offset.saturating_add(1);
                if end.saturating_sub(start) < self.compound_min {
                    continue;
                }
                let Some(slot) = self.nodes.get(node as usize) else { break };
                let Some(list) = jumps.get_mut(start) else { break };
                for &id in slot.ends.iter().take(8) {
                    if list.len() >= 16 {
                        break;
                    }
                    list.push((end, id));
                }
            }
            if let Some(list) = jumps.get_mut(start) {
                list.sort_by_key(|item| std::cmp::Reverse(item.0));
            }
        }
        jumps
    }

    fn match_pattern(
        &self,
        jumps: &[Vec<(usize, u32)>],
        pattern: &[Pat],
        pos: usize,
        pi: usize,
        parts: u8,
        clean: bool,
        steps: &mut u32,
    ) -> Option<bool> {
        *steps = steps.saturating_add(1);
        if *steps > COMPOUND_STEPS {
            return None;
        }
        if pi >= pattern.len() {
            if pos >= jumps.len() && parts >= 2 {
                return Some(clean);
            }
            return None;
        }
        let elem = pattern.get(pi)?;
        match elem.quant {
            Quant::One => self.eat(jumps, pattern, pos, pi, parts, clean, steps, false),
            Quant::Opt => or_opt(
                self.match_pattern(jumps, pattern, pos, pi.saturating_add(1), parts, clean, steps),
                self.eat(jumps, pattern, pos, pi, parts, clean, steps, false),
            ),
            Quant::Star => {
                let skipped = self.match_pattern(jumps, pattern, pos, pi.saturating_add(1), parts, clean, steps);
                if skipped == Some(true) {
                    return skipped;
                }
                or_opt(skipped, self.eat(jumps, pattern, pos, pi, parts, clean, steps, true))
            }
        }
    }

    fn eat(
        &self,
        jumps: &[Vec<(usize, u32)>],
        pattern: &[Pat],
        pos: usize,
        pi: usize,
        parts: u8,
        clean: bool,
        steps: &mut u32,
        stay: bool,
    ) -> Option<bool> {
        let flag = pattern.get(pi)?.flag;
        let next_pi = if stay { pi } else { pi.saturating_add(1) };
        let mut best = None;
        let list = jumps.get(pos)?;
        for &(end, id) in list.iter().take(16) {
            if end <= pos {
                continue;
            }
            let Some(stem) = self.stems.get(id as usize) else { continue };
            if !flag_has(&self.flag_arena, stem, flag) {
                continue;
            }
            let next_clean = clean && !flagged(&self.flag_arena, stem, self.nosuggest);
            let next_parts = parts.saturating_add(1);
            let hit = self.match_pattern(jumps, pattern, end, next_pi, next_parts, next_clean, steps);
            best = or_opt(best, hit);
            if best == Some(true) || *steps > COMPOUND_STEPS {
                break;
            }
        }
        best
    }

    pub(crate) fn word_chars(&self) -> &[char] {
        &self.word_chars
    }

    pub(crate) fn reps(&self) -> &[(String, String)] {
        &self.reps
    }

    pub(crate) fn try_chars(&self) -> &[char] {
        &self.try_chars
    }

    #[cfg(test)]
    pub(crate) fn stem_count(&self) -> usize {
        self.stems.len()
    }

    pub(crate) fn ids_of_len(&self, len: usize) -> &[u32] {
        self.by_len.get(len).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn bigrams(&self, id: u32) -> &[u16] {
        gram_slice(&self.bi_off, &self.bi_data, id)
    }

    pub(crate) fn trigrams(&self, id: u32) -> &[u16] {
        gram_slice(&self.tri_off, &self.tri_data, id)
    }

    pub(crate) fn expansions(&self, id: u32, out: &mut Vec<String>) {
        let Some(stem) = self.stems.get(id as usize) else { return };
        let text = stem.text.clone();
        let flags = stem_flags(&self.flag_arena, stem).to_string();
        if flag_in(&flags, self.nosuggest) || flag_in(&flags, self.only_in_compound) {
            return;
        }
        out.push(text.clone());
        for rule in &self.suffixes {
            if !flags.chars().any(|c| c == rule.flag) {
                continue;
            }
            if let Some(word) = affix::surface_suffix(&text, rule) {
                out.push(word);
            }
            if out.len() >= 40 {
                return;
            }
        }
        for prefix in &self.prefixes {
            if !flags.chars().any(|c| c == prefix.flag) {
                continue;
            }
            let Some(word) = affix::surface_prefix(&text, prefix) else { continue };
            out.push(word);
            if out.len() >= 40 {
                return;
            }
            if !prefix.cross {
                continue;
            }
            for suffix in &self.suffixes {
                if !suffix.cross || !flags.chars().any(|c| c == suffix.flag) {
                    continue;
                }
                if let Some(word) = affix::surface_both(&text, prefix, suffix) {
                    out.push(word);
                }
                if out.len() >= 40 {
                    return;
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn memory_parts(&self) -> [usize; 4] {
        let mut stems = 0usize;
        for stem in &self.stems {
            stems = stems.saturating_add(std::mem::size_of::<Stem>().saturating_add(stem.text.capacity()));
        }
        let mut map = 0usize;
        for (key, ids) in &self.by_lower {
            map = map.saturating_add(key.capacity().saturating_add(ids.capacity().saturating_mul(4)).saturating_add(48));
        }
        let grams = self
            .bi_data
            .capacity()
            .saturating_add(self.tri_data.capacity())
            .saturating_mul(2)
            .saturating_add(self.bi_off.capacity().saturating_add(self.tri_off.capacity()).saturating_mul(4));
        let mut other = self.flag_arena.capacity();
        for node in &self.nodes {
            other = other.saturating_add(std::mem::size_of::<Node>());
            other = other.saturating_add(node.edge_ch.capacity().saturating_mul(4));
            other = other.saturating_add(node.edge_to.capacity().saturating_mul(4));
            other = other.saturating_add(node.ends.capacity().saturating_mul(4));
        }
        [stems, map, grams, other]
    }

    #[cfg(test)]
    pub(crate) fn estimate_bytes(&self) -> usize {
        let parts = self.memory_parts();
        let mut n = std::mem::size_of::<Self>();
        for part in parts {
            n = n.saturating_add(part);
        }
        n.saturating_mul(2)
    }
}

fn fit_flags(raw: String) -> String {
    if raw.len() <= 255 {
        return raw;
    }
    let mut end = 255usize;
    while end > 0 && !raw.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    raw.get(..end).unwrap_or("").to_string()
}

fn clean_flags(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_whitespace() {
            break;
        }
        if out.chars().count() >= MAX_FLAGS {
            break;
        }
        if !out.chars().any(|have| have == c) {
            out.push(c);
        }
    }
    out
}

fn stem_flags<'a>(arena: &'a str, stem: &Stem) -> &'a str {
    let start = stem.flag_off as usize;
    let end = start.saturating_add(usize::from(stem.flag_len));
    arena.get(start..end).unwrap_or("")
}

fn flag_has(arena: &str, stem: &Stem, flag: char) -> bool {
    stem_flags(arena, stem).chars().any(|c| c == flag)
}

fn flagged(arena: &str, stem: &Stem, flag: Option<char>) -> bool {
    flag.is_some_and(|flag| flag_has(arena, stem, flag))
}

fn flag_in(flags: &str, flag: Option<char>) -> bool {
    flag.is_some_and(|flag| flags.chars().any(|c| c == flag))
}

fn joined(base: &str, strip: &str) -> String {
    if strip.is_empty() {
        return base.to_string();
    }
    let mut out = String::with_capacity(base.len().saturating_add(strip.len()));
    out.push_str(base);
    out.push_str(strip);
    out
}

fn prefixed(strip: &str, base: &str) -> String {
    let mut out = String::with_capacity(strip.len().saturating_add(base.len()));
    out.push_str(strip);
    out.push_str(base);
    out
}

fn cross_stem(prefix_strip: &str, mid: &str, suffix_strip: &str) -> String {
    let mut out = String::with_capacity(prefix_strip.len().saturating_add(mid.len()).saturating_add(suffix_strip.len()));
    out.push_str(prefix_strip);
    out.push_str(mid);
    out.push_str(suffix_strip);
    out
}

fn edge_to(node: &Node, ch: char) -> Option<u32> {
    let pos = node.edge_ch.iter().position(|have| *have == ch)?;
    node.edge_to.get(pos).copied()
}

fn or_opt(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), _) | (_, Some(false)) => Some(false),
        _ => None,
    }
}

fn apply_iconv(rules: &[(String, String)], word: &str) -> String {
    let mut current = word.to_string();
    for (from, to) in rules {
        if from.is_empty() || !current.contains(from) {
            continue;
        }
        let mut out = String::with_capacity(current.len());
        let mut rest = current.as_str();
        let mut guard = 0u32;
        while guard < 64 {
            guard = guard.saturating_add(1);
            let Some(at) = rest.find(from) else { break };
            if !rest.is_char_boundary(at) {
                break;
            }
            out.push_str(rest.get(..at).unwrap_or(""));
            out.push_str(to);
            let next = at.saturating_add(from.len());
            if !rest.is_char_boundary(next) {
                break;
            }
            rest = rest.get(next..).unwrap_or("");
        }
        out.push_str(rest);
        if out.len() > 4096 {
            break;
        }
        current = out;
    }
    current
}

fn gram_slice<'a, T>(off: &'a [u32], data: &'a [T], id: u32) -> &'a [T] {
    let slot = id as usize;
    let start = off.get(slot).copied().unwrap_or(0) as usize;
    let end = off.get(slot.saturating_add(1)).copied().unwrap_or(start as u32) as usize;
    data.get(start..end).unwrap_or(&[])
}

pub(crate) fn grams_of(lower: &str) -> (Vec<u16>, Vec<u16>) {
    let chars: Vec<char> = lower.chars().take(MAX_WORD).collect();
    let mut bi = Vec::new();
    let mut tri = Vec::new();
    if chars.len() >= 2 {
        for pair in chars.windows(2) {
            if bi.len() >= MAX_GRAMS {
                break;
            }
            let (Some(&a), Some(&b)) = (pair.first(), pair.get(1)) else { continue };
            bi.push(hash2(a, b));
        }
    }
    if chars.len() >= 3 {
        for triple in chars.windows(3) {
            if tri.len() >= MAX_GRAMS {
                break;
            }
            let (Some(&a), Some(&b), Some(&c)) = (triple.first(), triple.get(1), triple.get(2)) else { continue };
            tri.push(hash3(a, b, c));
        }
    }
    bi.sort_unstable();
    bi.dedup();
    tri.sort_unstable();
    tri.dedup();
    (bi, tri)
}

fn mix2(a: char, b: char) -> u32 {
    let mut h = 0x811c_9dc5u32;
    h = h.wrapping_mul(0x0100_0193) ^ (a as u32);
    h.wrapping_mul(0x0100_0193) ^ (b as u32)
}

fn fold_hash(h: u32) -> u16 {
    (h ^ (h >> 16)) as u16
}

fn hash2(a: char, b: char) -> u16 {
    fold_hash(mix2(a, b))
}

fn hash3(a: char, b: char, c: char) -> u16 {
    fold_hash(mix2(a, b).wrapping_mul(0x0100_0193) ^ (c as u32))
}

pub(crate) fn overlap(a: &[u16], b: &[u16]) -> u32 {
    let mut i = 0usize;
    let mut j = 0usize;
    let mut n = 0u32;
    while i < a.len() && j < b.len() {
        let (Some(&av), Some(&bv)) = (a.get(i), b.get(j)) else { break };
        match av.cmp(&bv) {
            std::cmp::Ordering::Equal => {
                n = n.saturating_add(1);
                i = i.saturating_add(1);
                j = j.saturating_add(1);
            }
            std::cmp::Ordering::Less => i = i.saturating_add(1),
            std::cmp::Ordering::Greater => j = j.saturating_add(1),
        }
    }
    n
}
