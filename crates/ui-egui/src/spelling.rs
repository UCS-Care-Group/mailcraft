// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offline spelling in the editable part of a message. A host provides dictionaries: the operating
//! system's, where it has them, and the US English list built into MailCraft. Ranges returned by
//! the host use bytes; the composer edits by character offset.

use std::ops::Range;

use egui::{Context, Ui};
use mailcraft_engine::spelling::{self, Filter};
use mailcraft_model::rich::{OBJECT, Paragraph};
use serde_json::json;

use crate::MailApp;
use crate::compose::editor;

const REVIEW_PARAGRAPHS_PER_FRAME: usize = 8;
pub use mailcraft_spell::{BuiltIn as BundledChecker, EN_US_TAG, HostChecker, MAX_ISSUES};
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
    if tag == EN_US_TAG {
        return tl!("English (United States, built-in)").to_string();
    }
    mailcraft_i18n::Lang::from_code(tag).map_or_else(|| tag.to_string(), |lang| format!("{} ({tag})", lang.native_name()))
}

pub(crate) fn check(host: &dyn HostChecker, locale: &str, paragraph: &Paragraph) -> Checked {
    let report = spelling::check_paragraph(host, locale, paragraph, &Filter::default());
    Checked {
        locale: locale.to_string(),
        issues: report.issues.into_iter().map(|issue| Issue { span: issue.span, word: issue.word }).collect(),
        available: report.available,
        partial: report.partial,
    }
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
