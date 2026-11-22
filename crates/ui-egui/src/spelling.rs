// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offline spelling in the editable part of a message. A host provides dictionaries: the operating
//! system's, where it has them, and the US English list built into MailCraft. Ranges returned by
//! the host use bytes; the composer edits by character offset.

use std::ops::Range;

use egui::{Context, Ui};
use mailcraft_engine::spelling::{self, Filter};
use mailcraft_model::rich::{OBJECT, Paragraph};
use serde_json::{Value, json};

use crate::MailApp;
use crate::compose::editor;

pub(crate) const REVIEW_PARAGRAPHS_PER_FRAME: usize = 8;
/// How long the word under the caret stays unmarked after a character is typed, in the body and
/// the Subject.
pub(crate) const TYPING_PAUSE: f64 = 0.6;
pub use mailcraft_spell::{BuiltIn as BundledChecker, EN_US_TAG, HostChecker, MAX_ISSUES};
pub(crate) const MAX_SUGGESTIONS: usize = 6;
const OFF: &str = "off";

pub type Host = Box<dyn HostChecker>;

pub(crate) struct Checked {
    pub locale: String,
    /// [`MailApp::spelling_rev`] when this was checked. A newer revision is checked again.
    pub rev: u64,
    pub issues: Vec<Issue>,
    pub(crate) available: bool,
    pub(crate) partial: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Issue {
    pub span: Range<usize>,
    pub(crate) word: String,
}

#[derive(Clone)]
pub(crate) struct Suggestions {
    issue: Issue,
    words: Vec<String>,
}

/// What the spelling menu asked for: a replacement, or one of the dictionary actions.
#[derive(Clone)]
pub(crate) enum Choice {
    Replace(String),
    IgnoreAll,
    Add,
}

fn preference(app: &MailApp, ctx: &Context) -> String {
    ctx.data(|d| d.get_temp(editor::id(app).with("spelling-language"))).unwrap_or_default()
}

pub(crate) fn locale(app: &MailApp, ctx: &Context) -> Option<String> {
    let host = app.services.spelling.as_deref()?;
    if ctx.data(|d| d.get_temp::<bool>(editor::id(app).with("spelling-off")).unwrap_or_default()) {
        return None;
    }
    let selected = preference(app, ctx);
    if !selected.is_empty() {
        return Some(selected);
    }
    let languages = host.locales();
    // The saved default, when this message has not chosen and the host still offers that tag.
    // Off is a choice for this message only, never a saved default.
    let saved = app.ui.spelling_language.as_str();
    if !saved.is_empty()
        && let Some(tag) = languages.iter().find(|tag| tag.as_str() == saved)
    {
        return Some(tag.clone());
    }
    let interface = app.language();
    let system = app.services.system_locale.as_deref();
    // Prefer the system's regional variant if it matches the interface language.
    let same = |tag: &str| mailcraft_i18n::Lang::from_code(tag) == Some(interface);
    let normalized = |tag: &str| tag.replace('_', "-").to_ascii_lowercase();
    if let Some(system) = system.filter(|tag| same(tag)) {
        let system = normalized(system.split(['.', '@']).next().unwrap_or(system));
        if let Some(tag) = languages.iter().find(|tag| normalized(tag) == system) {
            return Some(tag.clone());
        }
    }
    Some(languages.into_iter().find(|tag| same(tag)).unwrap_or_default())
}

pub(crate) fn locale_name(tag: &str) -> String {
    if tag == EN_US_TAG {
        return tl!("English (United States, built-in)").to_string();
    }
    mailcraft_i18n::Lang::from_code(tag).map_or_else(|| tag.to_string(), |lang| format!("{} ({tag})", lang.native_name()))
}

/// The filter the window checks with: the saved dictionary, this run's Ignore All, and the two switches.
pub(crate) fn filter_of(app: &MailApp) -> Filter<'_> {
    Filter {
        custom: app.ui.custom_dictionary.as_slice(),
        ignored: &app.spelling_ignored,
        ignore_uppercase: app.ui.spelling_ignore_uppercase,
        ignore_numbers: app.ui.spelling_ignore_numbers,
    }
}

/// Check one paragraph with the window's filter, tagged with the revision it was checked at.
pub(crate) fn check_in(app: &MailApp, locale: &str, paragraph: &Paragraph) -> Checked {
    let rev = app.spelling_rev;
    let Some(host) = app.services.spelling.as_deref() else {
        return Checked { locale: locale.to_string(), rev, issues: Vec::new(), available: false, partial: false };
    };
    let filter = filter_of(app);
    check(host, locale, paragraph, &filter, rev)
}

pub(crate) fn check(host: &dyn HostChecker, locale: &str, paragraph: &Paragraph, filter: &Filter<'_>, rev: u64) -> Checked {
    let report = spelling::check_paragraph(host, locale, paragraph, filter);
    Checked {
        locale: locale.to_string(),
        rev,
        issues: report.issues.into_iter().map(|issue| Issue { span: issue.span, word: issue.word }).collect(),
        available: report.available,
        partial: report.partial,
    }
}

fn kept_suggestion(word: &str) -> bool {
    !word.is_empty() && word.len() <= 256 && !word.contains(['\n', '\r', '\0', OBJECT])
}

/// Suggestions for `word`. The built-in list is asked with the custom dictionary as extra candidates.
/// Another host is asked on its own, and a custom word one or two edits away is offered first.
fn suggest_words(host: &dyn HostChecker, locale: &str, word: &str, custom: &[String]) -> Vec<String> {
    let mut words = if locale == EN_US_TAG {
        let extra: Vec<&str> = custom.iter().map(String::as_str).collect();
        mailcraft_spell::en_us().map(|dict| dict.suggest(word, &extra)).unwrap_or_default()
    } else {
        let mut words = host.suggest(locale, word);
        prepend_custom(&mut words, word, custom);
        words
    };
    words.retain(|word| kept_suggestion(word));
    words.truncate(MAX_SUGGESTIONS);
    words
}

/// Custom words within edit distance 1 or 2, in front of what the host offered. Bounded so a long
/// dictionary stays cheap on a click.
fn prepend_custom(words: &mut Vec<String>, typed: &str, custom: &[String]) {
    if custom.is_empty() || typed.chars().count() > 32 {
        return;
    }
    let mut extra = Vec::new();
    for entry in custom.iter().take(2_000) {
        if extra.len() >= MAX_SUGGESTIONS {
            break;
        }
        if !kept_suggestion(entry) || words.iter().any(|word| word == entry) || extra.iter().any(|word| word == entry) {
            continue;
        }
        if within_edit(typed, entry, 2) {
            extra.push(entry.clone());
        }
    }
    if extra.is_empty() {
        return;
    }
    extra.append(words);
    *words = extra;
}

/// Whether `a` and `b` differ by 1 or 2 insertions, deletions or substitutions. Longer words, and
/// words whose lengths already differ by more than `max`, are left out.
fn within_edit(a: &str, b: &str, max: usize) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n > 32 || m > 32 || n.abs_diff(m) > max || n == 0 || m == 0 {
        return false;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m.saturating_add(1)];
    for i in 0..n {
        let left = cur.get_mut(0);
        if let Some(cell) = left {
            *cell = i.saturating_add(1);
        }
        let mut row_min = i.saturating_add(1);
        for j in 0..m {
            let cost = usize::from(a.get(i) != b.get(j));
            let del = prev.get(j.saturating_add(1)).copied().unwrap_or(max.saturating_add(1)).saturating_add(1);
            let ins = cur.get(j).copied().unwrap_or(max.saturating_add(1)).saturating_add(1);
            let sub = prev.get(j).copied().unwrap_or(max.saturating_add(1)).saturating_add(cost);
            let best = del.min(ins).min(sub);
            if let Some(cell) = cur.get_mut(j.saturating_add(1)) {
                *cell = best;
            }
            row_min = row_min.min(best);
        }
        if row_min > max {
            return false;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    match prev.get(m).copied() {
        Some(d) => (1..=max).contains(&d),
        None => false,
    }
}

impl Issue {
    pub(crate) fn at(span: Range<usize>, word: String) -> Self {
        Self { span, word }
    }
}

impl Suggestions {
    pub(crate) fn new(host: &dyn HostChecker, locale: &str, mut issue: Issue, start: usize, custom: &[String]) -> Self {
        let words = suggest_words(host, locale, &issue.word, custom);
        issue.span = start + issue.span.start..start + issue.span.end;
        Self { issue, words }
    }

    pub(crate) fn word(&self) -> &str {
        &self.issue.word
    }

    /// Character offsets of the marked word in the text it was built for (the body, or the Subject
    /// when `start` passed to [`Suggestions::new`] was 0).
    pub(crate) fn place(&self) -> (usize, usize) {
        (self.issue.span.start, self.issue.span.end)
    }

    pub(crate) fn words(&self) -> &[String] {
        &self.words
    }

    pub(crate) fn show(&self, ui: &mut Ui, chosen: &mut Option<Choice>) -> bool {
        let mut keyboard = false;
        if self.words.is_empty() {
            ui.label(tl!("No spelling suggestions"));
        }
        for word in &self.words {
            let response = ui.button(word);
            keyboard |= response.has_focus();
            if response.clicked() {
                *chosen = Some(Choice::Replace(word.clone()));
                ui.close();
            }
        }
        ui.separator();
        let ignore = ui.button(tl!("Ignore All"));
        keyboard |= ignore.has_focus();
        if ignore.clicked() {
            *chosen = Some(Choice::IgnoreAll);
            ui.close();
        }
        let add = ui.button(tl!("Add to Dictionary"));
        keyboard |= add.has_focus();
        if add.clicked() {
            *chosen = Some(Choice::Add);
            ui.close();
        }
        ui.separator();
        keyboard
    }

    pub(crate) fn apply(self, app: &mut MailApp, word: String) {
        let span = self.issue.span;
        if app.run("compose.replaceText", json!({"from": span.start, "to": span.end, "expected": self.issue.word, "text": word})).is_ok() {
            app.want_body_focus();
        }
    }
}

// The menu is also the way to find a spelling error with the keyboard. The next mark is selected
// in the editor. The menu's replacements are for the currently selected word, if it is marked.
pub(crate) fn review(ui: &mut Ui, app: &mut MailApp) {
    // The key and this item open the walkthrough. A script's `review.spelling` does not: it runs
    // the engine command (`MailApp::execute` never asks `input_dialog`).
    crate::widgets::mi(ui, app, "Check Spelling…", "review.spelling", json!({}));
    if ui.add(egui::Button::new(tl!("Custom Dictionary…")).min_size(egui::vec2(190.0, 0.0))).clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("backstage-show-spelling"), true));
        let _ = app.run("ui.backstage", json!({"value": true, "page": "options"}));
        ui.close();
    }
    ui.separator();
    let Some(host) = app.services.spelling.as_deref() else {
        ui.label(tl!("Spelling is unavailable on this platform"));
        return;
    };
    let selected = preference(app, ui.ctx());
    let off_id = editor::id(app).with("spelling-off");
    let is_off = ui.ctx().data(|d| d.get_temp::<bool>(off_id).unwrap_or_default());
    let mut languages = host.locales();
    languages.sort_unstable();
    languages.dedup();
    ui.menu_button(tl!("Spelling language"), |ui| {
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            let choices = [(String::new(), tl!("Automatic").to_string()), (OFF.to_string(), tl!("Off").to_string())];
            for (tag, name) in choices.into_iter().chain(languages.into_iter().map(|tag| (tag.clone(), locale_name(&tag)))) {
                if ui.selectable_label(if is_off { tag == OFF } else { tag == selected }, name).clicked() {
                    let id = editor::id(app).with("spelling-language");
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(off_id, tag == OFF);
                        if tag != OFF {
                            d.insert_temp(id, tag);
                        }
                    });
                    ui.close();
                }
            }
        });
    });
    if let Some(locale) = locale(app, ui.ctx()) {
        if ui.button(tl!("Find next spelling error")).clicked() {
            review_next(app, ui.ctx(), &locale);
            ui.close();
        }
        if let Some((issue, start)) = editor::selected_spelling(app, ui.ctx(), &locale) {
            ui.menu_button(tl!("Spelling suggestions"), |ui| {
                let Some(host) = app.services.spelling.as_deref() else { return };
                let key = editor::id(app).with("review-suggestions");
                let now = ui.input(|input| input.time);
                let rev = app.spelling_rev;
                let cached: Option<(String, usize, Issue, Suggestions, f64, u64)> = ui.ctx().data(|d| d.get_temp(key));
                let suggestions = if let Some((tag, offset, checked, suggestions, retry_at, cached_rev)) = cached
                    && tag == locale
                    && offset == start
                    && checked == issue
                    && cached_rev == rev
                    && (!suggestions.words.is_empty() || now < retry_at)
                {
                    suggestions
                } else {
                    let suggestions = Suggestions::new(host, &locale, issue.clone(), start, app.ui.custom_dictionary.as_slice());
                    ui.ctx().data_mut(|d| d.insert_temp(key, (locale.clone(), start, issue, suggestions.clone(), now + 2.0, rev)));
                    suggestions
                };
                let mut chosen = None;
                suggestions.show(ui, &mut chosen);
                if let Some(choice) = chosen {
                    match choice {
                        Choice::Replace(word) => suggestions.apply(app, word),
                        Choice::IgnoreAll => {
                            let _ = app.run("ui.dictionary.ignore", json!({"word": suggestions.word()}));
                        }
                        Choice::Add => {
                            let _ = app.run("ui.dictionary.add", json!({"word": suggestions.word()}));
                        }
                    }
                }
            });
        }
    }
}

#[derive(Clone)]
struct ReviewScan {
    body: std::sync::Arc<mailcraft_model::rich::RichText>,
    locale: String,
    selection: mailcraft_model::rich::Selection,
    paragraphs: std::sync::Arc<Vec<usize>>,
    first: usize,
    visited: usize,
    partial: bool,
    /// [`MailApp::spelling_rev`] when this scan started. A newer revision starts again.
    rev: u64,
}

fn review_next(app: &mut MailApp, ctx: &Context, locale: &str) {
    let Some(compose) = app.session.compose.as_ref() else { return };
    let body = compose.body.text();
    let selection = compose.body.selection();
    let mut start = 0;
    let paragraphs = body
        .paragraphs()
        .iter()
        .map(|p| {
            let at = start;
            start += p.chars() + 1;
            at
        })
        .collect::<Vec<_>>();
    let first = paragraphs.partition_point(|p| *p <= selection.end()).saturating_sub(1);
    let scan = ReviewScan {
        body: std::sync::Arc::new(body.clone()),
        locale: locale.to_owned(),
        selection,
        paragraphs: std::sync::Arc::new(paragraphs),
        first,
        visited: 0,
        partial: false,
        rev: app.spelling_rev,
    };
    ctx.data_mut(|d| d.insert_temp(editor::id(app).with("spelling-scan"), scan));
    ctx.request_repaint();
}

pub(crate) fn advance_review(app: &mut MailApp, ctx: &Context, locale: Option<&str>) {
    let key = editor::id(app).with("spelling-scan");
    let Some(mut scan) = ctx.data_mut(|d| {
        let scan = d.get_temp::<ReviewScan>(key);
        d.remove::<ReviewScan>(key);
        scan
    }) else {
        return;
    };
    if app.services.spelling.is_none() {
        return;
    }
    let Some(compose) = app.session.compose.as_ref() else { return };
    if locale != Some(&scan.locale) || compose.body.text() != scan.body.as_ref() || compose.body.selection() != scan.selection {
        return;
    }
    if app.spelling_rev != scan.rev {
        let locale = scan.locale.clone();
        review_next(app, ctx, &locale);
        return;
    }
    let count = scan.paragraphs.len();
    let total = count + 1; // The first paragraph is also visited on wrap, up to the selection.
    for _ in 0..REVIEW_PARAGRAPHS_PER_FRAME {
        if count == 0 || scan.visited >= total {
            app.status(if scan.partial { tl!("No spelling errors in the part checked") } else { tl!("No spelling errors found") });
            return;
        }
        let index = (scan.first + scan.visited) % count;
        let (Some(&start), Some(paragraph)) = (scan.paragraphs.get(index), scan.body.paragraphs().get(index)) else { return };
        let report = check_in(app, &scan.locale, paragraph);
        if !report.available {
            app.status(tl!("The spelling dictionary is unavailable"));
            return;
        }
        scan.partial |= report.partial;
        let position = scan.visited;
        scan.visited += 1;
        if let Some(issue) = report.issues.into_iter().find(|issue| {
            let from = start + issue.span.start;
            (position != 0 || from >= scan.selection.end()) && (position != count || from < scan.selection.end())
        }) {
            if app.execute("compose.select", json!({"from": start + issue.span.start, "to": start + issue.span.end})).is_ok() {
                app.want_body_focus();
                editor::reveal_spelling(app, ctx);
            }
            return;
        }
    }
    ctx.data_mut(|d| d.insert_temp(key, scan));
    ctx.request_repaint();
}

/// How long a saved language tag may be. Longer saved text is cut; a command that sends more is refused.
pub(crate) const MAX_LANGUAGE_CHARS: usize = 64;

/// Bump the revision so cached paragraph checks and review suggestions are done again.
pub(crate) fn bump(app: &mut MailApp) {
    app.spelling_rev = app.spelling_rev.wrapping_add(1);
}

/// A saved language tag: control characters removed, capped, and `off` dropped (that choice is per message).
pub(crate) fn clean_language(tag: &str) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for c in tag.chars() {
        if c.is_control() || c.is_whitespace() {
            continue;
        }
        count = count.saturating_add(1);
        if count > MAX_LANGUAGE_CHARS {
            break;
        }
        out.push(c);
    }
    if out.eq_ignore_ascii_case(OFF) { String::new() } else { out }
}

/// A switch read from `ui.json` that is anything but false stays on (the spelling defaults).
pub(crate) fn read_on<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let value = <Value as serde::Deserialize>::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(true))
}

/// A language tag read from `ui.json`. Anything but a string is Automatic.
pub(crate) fn read_language<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = <Value as serde::Deserialize>::deserialize(deserializer)?;
    Ok(clean_language(value.as_str().unwrap_or("")))
}

/// The custom dictionary read from `ui.json`: cleaned, deduplicated and capped. A value that is
/// not a list is an empty dictionary, so one bad field does not discard the rest of the settings.
pub(crate) fn read_dictionary<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    struct Visit;
    impl<'de> serde::de::Visitor<'de> for Visit {
        type Value = Vec<String>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a list of words")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            let mut seen = Kept::default();
            let mut looked = 0usize;
            // A hostile file can list far more than the dictionary keeps. Stop copying once it is
            // full, and stop reading after a bounded look so the rest is not stored.
            while looked < spelling::MAX_CUSTOM_WORDS.saturating_mul(2) {
                looked = looked.saturating_add(1);
                match seq.next_element::<Word>() {
                    Ok(Some(Word(Some(word)))) => {
                        if out.len() < spelling::MAX_CUSTOM_WORDS && seen.keep(&word) {
                            out.push(word);
                        }
                    }
                    Ok(Some(Word(None))) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            while seq.next_element::<serde::de::IgnoredAny>().ok().flatten().is_some() {}
            Ok(out)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_bool<E: serde::de::Error>(self, _value: bool) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_i64<E: serde::de::Error>(self, _value: i64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_u64<E: serde::de::Error>(self, _value: u64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_f64<E: serde::de::Error>(self, _value: f64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_str<E: serde::de::Error>(self, _value: &str) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_string<E: serde::de::Error>(self, _value: String) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            while map.next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?.is_some() {}
            Ok(Vec::new())
        }
    }
    deserializer.deserialize_any(Visit)
}

struct Word(Option<String>);

impl<'de> serde::Deserialize<'de> for Word {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visit;
        impl<'de> serde::de::Visitor<'de> for Visit {
            type Value = Option<String>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a word")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(accept_saved_word(value))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                self.visit_str(&value)
            }
            fn visit_borrowed_str<E: serde::de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
                self.visit_str(value)
            }
            fn visit_bool<E: serde::de::Error>(self, _value: bool) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_i64<E: serde::de::Error>(self, _value: i64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_u64<E: serde::de::Error>(self, _value: u64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_f64<E: serde::de::Error>(self, _value: f64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(None)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?.is_some() {}
                Ok(None)
            }
        }
        Ok(Word(deserializer.deserialize_any(Visit)?))
    }
}

fn accept_saved_word(text: &str) -> Option<String> {
    // A multi-megabyte string is already in the file's parse. Don't copy it in to throw it out.
    if text.len() > spelling::MAX_WORD_CHARS.saturating_mul(4).saturating_add(32) {
        return None;
    }
    spelling::clean_custom_word(text)
}

/// Words already kept, so a long list is deduplicated without scanning it again for each word.
/// A lowercase entry covers every casing. A capitalised entry covers that spelling and the same
/// word in all capitals, and does not cover the lowercase word.
#[derive(Default)]
struct Kept {
    folded: std::collections::HashSet<String>,
    lower: std::collections::HashSet<String>,
    exact: std::collections::HashSet<String>,
}

impl Kept {
    /// `true` when `word` is not already covered and is now remembered.
    fn keep(&mut self, word: &str) -> bool {
        if self.has(word) {
            return false;
        }
        let (exact, folded, has_upper) = word_keys(word);
        self.folded.insert(folded.clone());
        if has_upper {
            self.exact.insert(exact);
        } else {
            self.lower.insert(folded);
        }
        true
    }

    fn has(&self, word: &str) -> bool {
        let (exact, folded, has_upper) = word_keys(word);
        if !has_upper {
            // A capitalised entry is recorded in `folded` but does not cover the lowercase word,
            // matching `word_matches`: only a lowercase entry does.
            return self.lower.contains(&folded);
        }
        self.lower.contains(&folded) || self.exact.contains(&exact) || (all_capitals(&exact) && self.folded.contains(&folded))
    }
}

/// Apostrophe-folded spelling, its lower case, and whether it has a capital.
fn word_keys(word: &str) -> (String, String, bool) {
    let exact: String = word.chars().map(|c| if c == '\u{2019}' { '\'' } else { c }).collect();
    let has_upper = exact.chars().any(char::is_uppercase);
    let folded: String = exact.chars().flat_map(char::to_lowercase).collect();
    (exact, folded, has_upper)
}

fn all_capitals(word: &str) -> bool {
    let mut letters = 0usize;
    for c in word.chars() {
        if !c.is_alphabetic() {
            continue;
        }
        if !c.is_uppercase() {
            return false;
        }
        letters = letters.saturating_add(1);
    }
    letters >= 1
}

fn ignored_has(words: &std::collections::BTreeSet<String>, word: &str) -> bool {
    words.iter().any(|entry| spelling::word_matches(entry, word))
}

/// Clean what [`MailApp::apply_prefs`] just took from a file, including a `UiState` built in code.
pub(crate) fn sanitize_saved(ui: &mut crate::UiState) {
    ui.spelling_language = clean_language(&ui.spelling_language);
    let taken = std::mem::take(&mut ui.custom_dictionary);
    let mut out = Vec::new();
    let mut seen = Kept::default();
    for word in taken {
        if out.len() >= spelling::MAX_CUSTOM_WORDS {
            break;
        }
        let Some(word) = accept_saved_word(&word) else { continue };
        if seen.keep(&word) {
            out.push(word);
        }
    }
    ui.custom_dictionary = out;
}

fn spelling_state(app: &MailApp) -> Value {
    json!({
        "asYouType": app.ui.check_spelling_as_you_type,
        "language": app.ui.spelling_language,
        "ignoreUppercase": app.ui.spelling_ignore_uppercase,
        "ignoreNumbers": app.ui.spelling_ignore_numbers,
        "customWords": app.ui.custom_dictionary.len(),
    })
}

fn opt_bool(p: &Value, key: &str) -> Result<Option<bool>, String> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("`{key}` is true or false")),
    }
}

fn one_word(p: &Value) -> Result<String, String> {
    let Some(word) = p.get("word").and_then(Value::as_str) else {
        return Err("missing `word`".into());
    };
    spelling::clean_custom_word(word)
        .ok_or_else(|| tl!("A dictionary word is letters, digits, apostrophes and hyphens, up to 64 characters.").to_string())
}

fn spelling_command(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let as_you_type = opt_bool(p, "asYouType")?;
    let ignore_uppercase = opt_bool(p, "ignoreUppercase")?;
    let ignore_numbers = opt_bool(p, "ignoreNumbers")?;
    // Checked before anything is written, so an unknown tag leaves the saved choices as they were.
    let language = match p.get("language") {
        None | Some(Value::Null) => None,
        Some(Value::String(tag)) => Some(accepted_language(app, tag)?),
        Some(_) => return Err("`language` is \"\" for Automatic, or a language offered here".into()),
    };
    let mut changed = false;
    if let Some(on) = as_you_type
        && on != app.ui.check_spelling_as_you_type
    {
        app.ui.check_spelling_as_you_type = on;
        changed = true;
    }
    if let Some(on) = ignore_uppercase
        && on != app.ui.spelling_ignore_uppercase
    {
        app.ui.spelling_ignore_uppercase = on;
        changed = true;
    }
    if let Some(on) = ignore_numbers
        && on != app.ui.spelling_ignore_numbers
    {
        app.ui.spelling_ignore_numbers = on;
        changed = true;
    }
    if let Some(tag) = language
        && tag != app.ui.spelling_language
    {
        app.ui.spelling_language = tag;
        changed = true;
    }
    if changed {
        bump(app);
    }
    Ok(spelling_state(app))
}

/// `""` is Automatic. Anything else must be a tag the host offers, unchanged.
fn accepted_language(app: &MailApp, tag: &str) -> Result<String, String> {
    if tag.chars().any(char::is_control) || tag.chars().count() > MAX_LANGUAGE_CHARS {
        return Err("`language` is \"\" for Automatic, or a language offered here".into());
    }
    let tag = tag.trim();
    if tag.is_empty() {
        return Ok(String::new());
    }
    if tag.eq_ignore_ascii_case(OFF) {
        return Err("`language` is \"\" for Automatic, or a language offered here".into());
    }
    let offered = app.services.spelling.as_ref().is_some_and(|host| host.locales().iter().any(|have| have == tag));
    if !offered {
        return Err("`language` is \"\" for Automatic, or a language offered here".into());
    }
    Ok(tag.to_string())
}

fn add_word(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let word = one_word(p)?;
    // Only an entry the checker would accept for this spelling suppresses the add. A capitalised
    // entry does not cover the lowercase word, so that spelling replaces it.
    if app.ui.custom_dictionary.iter().any(|entry| spelling::word_matches(entry, &word)) {
        return Ok(json!({"word": word, "added": false}));
    }
    app.ui.custom_dictionary.retain(|entry| spelling::word_matches(entry, &word) || !spelling::word_matches(&word, entry));
    if app.ui.custom_dictionary.len() >= spelling::MAX_CUSTOM_WORDS {
        return Err(tl!("The custom dictionary is full.").to_string());
    }
    app.ui.custom_dictionary.push(word.clone());
    bump(app);
    Ok(json!({"word": word, "added": true}))
}

fn remove_word(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let word = one_word(p)?;
    let before = app.ui.custom_dictionary.len();
    app.ui.custom_dictionary.retain(|entry| !(spelling::word_matches(entry, &word) || spelling::word_matches(&word, entry)));
    let removed = before.saturating_sub(app.ui.custom_dictionary.len());
    if removed == 0 {
        return Err(tl!("That word isn't in the custom dictionary.").to_string());
    }
    bump(app);
    Ok(json!({"word": word, "removed": removed}))
}

fn ignore_word(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let word = one_word(p)?;
    if ignored_has(&app.spelling_ignored, &word) {
        return Ok(json!({"word": word, "ignored": true}));
    }
    if app.spelling_ignored.len() >= spelling::MAX_CUSTOM_WORDS {
        return Err(tl!("Ignore All is full.").to_string());
    }
    app.spelling_ignored.insert(word.clone());
    bump(app);
    Ok(json!({"word": word, "ignored": true}))
}

/// `ui.spelling` and `ui.dictionary.*`. `None` when `id` is some other command.
pub(crate) fn command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.spelling" => spelling_command(app, p),
        "ui.dictionary.add" => add_word(app, p),
        "ui.dictionary.remove" => remove_word(app, p),
        "ui.dictionary.ignore" => ignore_word(app, p),
        "ui.dictionary.list" => Ok(json!({"words": app.ui.custom_dictionary})),
        _ => return None,
    })
}

/// The wavy underline under a misspelt word, in the body's line and the Subject's. `y` is the
/// baseline the wave sits on; `left` and `right` are the word's edges.
pub(crate) fn paint_squiggle(painter: &egui::Painter, left: f32, right: f32, y: f32, ink: egui::Color32) {
    if left >= right {
        return;
    }
    let path = std::iter::successors(Some(left), |x| (*x + 2.0 < right).then_some(*x + 2.0))
        .chain(std::iter::once(right))
        .enumerate()
        .map(|(i, x)| egui::pos2(x, y + (i % 2) as f32))
        .collect();
    painter.add(egui::Shape::line(path, egui::Stroke::new(1.0, ink)));
}

/// Words typed into File › Options, separated by commas, spaces or new lines.
/// Words from a custom-dictionary draft that were not added, including the one that failed and
/// every word after it, so the draft can be tried again.
pub(crate) fn words_not_added(app: &mut MailApp, words: Vec<String>) -> Vec<String> {
    let mut rest = Vec::new();
    let mut failed = false;
    for word in words {
        if failed || app.run("ui.dictionary.add", json!({"word": word})).is_err() {
            failed = true;
            rest.push(word);
        }
    }
    rest
}

pub(crate) fn split_words(text: &str) -> Vec<&str> {
    text.split(|c: char| c == ',' || c.is_whitespace()).filter(|word| !word.is_empty()).collect()
}

#[cfg(test)]
mod tests;
