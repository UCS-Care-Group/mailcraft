// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offline spelling in the editable part of a message. A desktop host provides its installed
//! dictionaries. Ranges returned by the host use bytes; the composer edits by character offset.

use std::ops::Range;

use egui::{Context, Ui};
use mailcraft_model::rich::{OBJECT, Paragraph};
use serde_json::json;

use crate::MailApp;
use crate::compose::editor;

const MAX_PARAGRAPH_BYTES: usize = 8192;
const REVIEW_PARAGRAPHS_PER_FRAME: usize = 8;
pub use mailcraft_spell::{HostChecker, MAX_ISSUES};
const MAX_SUGGESTIONS: usize = 6;
const OFF: &str = "off";

pub type Host = Box<dyn HostChecker>;

pub(crate) struct Checked {
    pub locale: String,
    pub issues: Vec<Issue>,
    pub(crate) available: bool,
    partial: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Issue {
    pub span: Range<usize>,
    word: String,
}

#[derive(Clone)]
pub(crate) struct Suggestions {
    issue: Issue,
    words: Vec<String>,
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

fn locale_name(tag: &str) -> String {
    mailcraft_i18n::Lang::from_code(tag).map_or_else(|| tag.to_string(), |lang| format!("{} ({tag})", lang.native_name()))
}

pub(crate) fn check(host: &dyn HostChecker, locale: &str, paragraph: &Paragraph) -> Checked {
    let mut report = Checked { locale: locale.to_string(), issues: Vec::new(), available: true, partial: false };
    let bytes: usize = paragraph.runs.iter().map(|run| run.text.len()).sum();
    if paragraph.signature || bytes == 0 {
        return report;
    }
    if bytes > MAX_PARAGRAPH_BYTES {
        report.partial = true;
        return report;
    }
    let text = paragraph.text();
    let mut input = String::with_capacity(bytes);
    for run in &paragraph.runs {
        if run.style.link.is_some() || run.picture.is_some() {
            input.extend(std::iter::repeat_n(' ', run.text.len()));
        } else {
            input.push_str(&run.text);
        }
    }
    let Some(mut ranges) = host.check(locale, &input) else {
        report.available = false;
        return report;
    };
    report.partial = ranges.len() > MAX_ISSUES;
    ranges.truncate(MAX_ISSUES);
    ranges.sort_unstable_by_key(|range| range.start);
    let mut last = 0;
    let mut offset = 0;
    for range in ranges {
        let Some(word) = text.get(range.clone()).filter(|word| word.chars().any(char::is_alphabetic)) else { continue };
        if range.start < last || input.get(range.clone()) != Some(word) || word.contains(OBJECT) {
            continue;
        }
        if within_address(&text, &range) {
            continue;
        }
        offset += text.get(last..range.start).unwrap_or_default().chars().count();
        let end = offset + word.chars().count();
        report.issues.push(Issue { span: offset..end, word: word.to_string() });
        (last, offset) = (range.end, end);
    }
    report
}

/// Links are masked from the provider. Use the original text to find the address itself,
/// so a change of formatting in its middle does not unprotect the visible half.
fn within_address(text: &str, word: &Range<usize>) -> bool {
    let boundary = |ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | OBJECT);
    let left = text.get(..word.start).unwrap_or_default().rsplit(boundary).next().unwrap_or_default();
    let right = text.get(word.end..).unwrap_or_default().split(boundary).next().unwrap_or_default();
    let start = word.start - left.len();
    let end = word.end + right.len();
    let Some(token) = text.get(start..end) else { return false };
    let offset = word.start - start..word.end - start;
    let schemes = token.match_indices("://").filter_map(|(at, _)| {
        let prefix = token.get(..at)?;
        let scheme = prefix.rsplit(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))).next().unwrap_or_default();
        scheme.as_bytes().first().is_some_and(u8::is_ascii_alphabetic).then_some(at - scheme.len())
    });
    let punctuation = ['—', '–', '(', ')', '[', ']', ',', ';'];
    let web_starts = std::iter::once(0)
        .chain(token.match_indices(punctuation).map(|(at, mark)| at + mark.len()))
        .filter(|&at| token.get(at..at + 4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("www.")));
    if schemes.chain(web_starts).any(|at| at < offset.end && offset.start < url_end(token, at)) {
        return true;
    }

    let email_left = left.rsplit(punctuation).next().unwrap_or_default();
    let email_right = right.split(punctuation).next().unwrap_or_default();
    text.get(word.start - email_left.len()..word.end + email_right.len()).is_some_and(|address| address.contains('@'))
}

/// A closing parenthesis outside the URL ends a surrounding prose parenthesis.
/// Parentheses that open inside a URL can close within its path.
fn url_end(token: &str, start: usize) -> usize {
    let Some(url) = token.get(start..) else { return start };
    let mut depth = 0usize;
    for (at, ch) in url.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => return start + at,
            ')' => depth -= 1,
            _ => {}
        }
    }
    token.len()
}

impl Suggestions {
    pub(crate) fn new(host: &dyn HostChecker, locale: &str, mut issue: Issue, start: usize) -> Self {
        let words = host
            .suggest(locale, &issue.word)
            .into_iter()
            .filter(|word| !word.is_empty() && word.len() <= 256 && !word.contains(['\n', '\r', '\0', OBJECT]))
            .take(MAX_SUGGESTIONS)
            .collect();
        issue.span = start + issue.span.start..start + issue.span.end;
        Self { issue, words }
    }

    pub(crate) fn show(&self, ui: &mut Ui, chosen: &mut Option<String>) -> bool {
        let mut keyboard = false;
        if self.words.is_empty() {
            ui.label(tl!("No spelling suggestions"));
        }
        for word in &self.words {
            let response = ui.button(word);
            keyboard |= response.has_focus();
            if response.clicked() {
                *chosen = Some(word.clone());
                ui.close();
            }
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
                let cached: Option<(String, usize, Issue, Suggestions, f64)> = ui.ctx().data(|d| d.get_temp(key));
                let suggestions = if let Some((tag, offset, checked, suggestions, retry_at)) = cached
                    && tag == locale
                    && offset == start
                    && checked == issue
                    && (!suggestions.words.is_empty() || now < retry_at)
                {
                    suggestions
                } else {
                    let suggestions = Suggestions::new(host, &locale, issue.clone(), start);
                    ui.ctx().data_mut(|d| d.insert_temp(key, (locale, start, issue, suggestions.clone(), now + 2.0)));
                    suggestions
                };
                let mut chosen = None;
                suggestions.show(ui, &mut chosen);
                if let Some(word) = chosen {
                    suggestions.apply(app, word);
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
    let (Some(host), Some(compose)) = (app.services.spelling.as_deref(), app.session.compose.as_ref()) else { return };
    if locale != Some(&scan.locale) || compose.body.text() != scan.body.as_ref() || compose.body.selection() != scan.selection {
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
        let report = check(host, &scan.locale, paragraph);
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

#[cfg(test)]
mod tests;
