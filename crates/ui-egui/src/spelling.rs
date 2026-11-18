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
const MAX_REVIEW_BYTES: usize = 65536;
const MAX_ISSUES: usize = 256;
const MAX_SUGGESTIONS: usize = 6;
const OFF: &str = "off";

pub trait HostChecker {
    fn locales(&self) -> Vec<String>;
    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>>;
    fn suggest(&self, locale: &str, word: &str) -> Vec<String>;
}

pub type Host = Box<dyn HostChecker>;

pub(crate) struct Checked {
    pub locale: String,
    pub issues: Vec<Issue>,
    available: bool,
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
    let selected = preference(app, ctx);
    if selected == OFF {
        return None;
    }
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
    let mut report = Checked { locale: locale.to_string(), issues: Vec::new(), available: true };
    let bytes: usize = paragraph.runs.iter().map(|run| run.text.len()).sum();
    if paragraph.signature || bytes == 0 || bytes > MAX_PARAGRAPH_BYTES {
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
    ranges.truncate(MAX_ISSUES);
    ranges.sort_unstable_by_key(|range| range.start);
    let mut last = 0;
    let mut offset = 0;
    for range in ranges {
        let Some(word) = text.get(range.clone()).filter(|word| word.chars().any(char::is_alphabetic)) else { continue };
        if range.start < last || input.get(range.clone()) != Some(word) || word.contains(OBJECT) {
            continue;
        }
        let left = text.get(..range.start).unwrap_or_default().rsplit(char::is_whitespace).next().unwrap_or_default();
        let right = text.get(range.end..).unwrap_or_default().split(char::is_whitespace).next().unwrap_or_default();
        let token = format!("{left}{word}{right}");
        if token.contains('@') || token.contains("://") || token.get(..4).is_some_and(|start| start.eq_ignore_ascii_case("www.")) {
            continue;
        }
        offset += text.get(last..range.start).unwrap_or_default().chars().count();
        let end = offset + word.chars().count();
        report.issues.push(Issue { span: offset..end, word: word.to_string() });
        (last, offset) = (range.end, end);
    }
    report
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
    let mut languages = host.locales();
    languages.sort_unstable();
    languages.dedup();
    ui.menu_button(tl!("Spelling language"), |ui| {
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            let choices = [(String::new(), tl!("Automatic").to_string()), (OFF.to_string(), tl!("Off").to_string())];
            for (tag, name) in choices.into_iter().chain(languages.into_iter().map(|tag| (tag.clone(), locale_name(&tag)))) {
                if ui.selectable_label(tag == selected, name).clicked() {
                    let id = editor::id(app).with("spelling-language");
                    ui.ctx().data_mut(|d| d.insert_temp(id, tag));
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
                let cached: Option<(String, usize, Issue, Suggestions)> = ui.ctx().data(|d| d.get_temp(key));
                let suggestions = if let Some((tag, offset, checked, suggestions)) = cached
                    && tag == locale
                    && offset == start
                    && checked == issue
                {
                    suggestions
                } else {
                    let suggestions = Suggestions::new(host, &locale, issue.clone(), start);
                    ui.ctx().data_mut(|d| d.insert_temp(key, (locale, start, issue, suggestions.clone())));
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

fn review_next(app: &mut MailApp, ctx: &Context, locale: &str) {
    let (Some(host), Some(compose)) = (app.services.spelling.as_deref(), app.session.compose.as_ref()) else { return };
    let selection = compose.body.selection();
    let mut start = 0;
    let mut bytes = 0;
    let mut first = None;
    let mut next = None;
    let mut limited = false;
    for paragraph in compose.body.text().paragraphs() {
        bytes += 1 + paragraph.runs.iter().map(|run| run.text.len()).sum::<usize>();
        if bytes > MAX_REVIEW_BYTES {
            limited = true;
            break;
        }
        let report = check(host, locale, paragraph);
        if !report.available {
            app.status(tl!("The spelling dictionary is unavailable"));
            return;
        }
        for issue in report.issues {
            let span = start + issue.span.start..start + issue.span.end;
            if span.start >= selection.end() {
                next = Some(span);
                break;
            }
            first.get_or_insert(span);
        }
        if next.is_some() {
            break;
        }
        start += paragraph.chars() + 1;
    }
    if let Some(span) = next.or(first) {
        if app.execute("compose.select", json!({"from": span.start, "to": span.end})).is_ok() {
            app.want_body_focus();
            editor::reveal_spelling(app, ctx);
        }
    } else if limited {
        app.status(tl!("No spelling errors in the part checked"));
    } else {
        app.status(tl!("No spelling errors found"));
    }
}

#[cfg(test)]
mod tests;
