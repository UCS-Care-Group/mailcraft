// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Review › Spelling & Grammar (F7). The dialog walks the message with the active spelling host
//! (an operating-system dictionary when the platform has one, otherwise the built-in list) and the
//! message's language. It starts at the top: the Subject first, then each body paragraph. A
//! signature open in File › Signatures is the document instead, and that message's Subject is
//! left out.
//!
//! Each frame checks at most [`REVIEW_PARAGRAPHS_PER_FRAME`] paragraphs, then asks to be drawn
//! again, so a long message cannot freeze one frame. After every Change, Change All, Ignore or
//! Add it scans again from the top, so offsets are taken from the text as it is. Ignore Once
//! remembers the occurrence by where it sat in the list of that word (the first `teh`, the
//! second), not by a character offset, so a later edit does not point it at a different word.
//!
//! The dialog only collects the replacement. Change runs `compose.replaceText` in the body or
//! `compose.replaceSubject` in the Subject, each one step of `compose.undo`. Change All runs
//! `edit.replace` (one undo step for the body, whole words only) and one `compose.set` for the
//! Subject, which `compose.undo` does not take back.

use std::ops::Range;

use egui::{Align2, Ui, vec2};
use mailcraft_model::rich::Paragraph;
use serde_json::json;

use crate::MailApp;
use crate::dialogs::button_row;
use crate::spelling::{self, Issue, REVIEW_PARAGRAPHS_PER_FRAME, Suggestions};
use crate::theme::{Tokens, medium, regular, semibold};
use crate::widgets::plain_button;

pub(crate) const WIDTH: f32 = 640.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Subject,
    Body,
}

#[derive(Clone, PartialEq, Eq)]
struct Once {
    place: Place,
    word: String,
    nth: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct Mistake {
    place: Place,
    from: usize,
    to: usize,
    word: String,
    nth: usize,
    words: Vec<String>,
    context: String,
    emphasis: Range<usize>,
}

enum Act {
    Change,
    ChangeAll,
    IgnoreOnce,
    IgnoreAll,
    Add,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Step {
    #[default]
    Subject,
    Body(usize),
}

/// The walkthrough. One is open at a time, over the message it was started on.
#[derive(Default)]
pub struct SpellingDialog {
    replacement: String,
    note: String,
    current: Option<Mistake>,
    step: Step,
    /// How many times each word has been seen in this scan, so Ignore Once can name an occurrence.
    counts: Vec<(Place, String, usize)>,
    ignored: Vec<Once>,
    partial: bool,
    complete: bool,
    revealed: bool,
    filled: bool,
    /// Started while a message was being written: closing that message closes the dialog.
    tied_to_compose: bool,
}

impl SpellingDialog {
    /// Opened for the message being written, or the signature open in File › Signatures.
    /// `None` when there is nothing to check, or spelling is off or has no dictionary: the status
    /// bar says which.
    pub fn open(app: &mut MailApp) -> Option<Self> {
        app.session.doc()?;
        if app.services.spelling.is_none() {
            app.status(tl!("Spelling is unavailable on this platform"));
            return None;
        }
        if let Some(ctx) = app.ctx.clone()
            && spelling::locale(app, &ctx).is_none()
        {
            app.status(tl!("Spelling is off for this message"));
            return None;
        }
        Some(Self { tied_to_compose: app.session.compose.is_some(), ..Self::default() })
    }

    pub(crate) fn tied_to_compose(&self) -> bool {
        self.tied_to_compose
    }

    /// The dialog's contents. `true` when it should close.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        if app.session.doc().is_none() || (self.tied_to_compose && app.session.compose.is_none()) {
            return true;
        }
        let Some(locale) = spelling::locale(app, ui.ctx()) else {
            app.status(tl!("Spelling is off for this message"));
            return true;
        };
        if self.current.is_none() && !self.complete && self.scan_some(app, &locale) {
            app.status(tl!("The spelling dictionary is unavailable"));
            return true;
        }
        if let Some(mistake) = self.current.clone() {
            return self.draw_mistake(app, ui, &mistake);
        }
        if self.complete {
            return self.draw_done(ui);
        }
        self.draw_checking(ui)
    }

    /// `true` when the host has no dictionary for this language.
    fn scan_some(&mut self, app: &mut MailApp, locale: &str) -> bool {
        let mut budget = REVIEW_PARAGRAPHS_PER_FRAME;
        if self.step == Step::Subject {
            budget = budget.saturating_sub(1);
            self.step = Step::Body(0);
            if app.session.signature_edit.is_none() {
                let subject = app.session.compose.as_ref().map(|c| c.subject.clone()).unwrap_or_default();
                let report = plain_report(app, locale, &subject);
                if !report.available {
                    return true;
                }
                self.partial |= report.partial;
                let issues: Vec<(Range<usize>, String)> = report.issues.into_iter().map(|issue| (issue.span, issue.word)).collect();
                if self.present(app, locale, Place::Subject, 0, &issues) {
                    return false;
                }
            }
        }
        if budget == 0 || self.current.is_some() || self.complete {
            return false;
        }
        let (batch, end) = self.take_paragraphs(app, budget);
        let mut finished = end && batch.is_empty();
        for (index, origin, paragraph) in batch {
            let report = spelling::check_in(app, locale, &paragraph);
            self.step = Step::Body(index.saturating_add(1));
            if !report.available {
                return true;
            }
            self.partial |= report.partial;
            let issues: Vec<(Range<usize>, String)> = report.issues.into_iter().map(|issue| (issue.span, issue.word.clone())).collect();
            if self.present(app, locale, Place::Body, origin, &issues) {
                return false;
            }
            finished = end && self.step == Step::Body(index.saturating_add(1));
        }
        if finished {
            self.complete = true;
        }
        false
    }

    /// The next `budget` paragraphs, and whether they run to the end of the document.
    fn take_paragraphs(&mut self, app: &MailApp, budget: usize) -> (Vec<(usize, usize, Paragraph)>, bool) {
        let Step::Body(start) = self.step else { return (Vec::new(), false) };
        let Some((body, _)) = app.session.doc() else {
            self.complete = true;
            return (Vec::new(), true);
        };
        let paragraphs = body.text().paragraphs();
        if start >= paragraphs.len() {
            self.complete = true;
            return (Vec::new(), true);
        }
        let mut origin = 0usize;
        let mut out = Vec::new();
        for (index, paragraph) in paragraphs.iter().enumerate() {
            if index >= start && out.len() < budget {
                out.push((index, origin, paragraph.clone()));
            }
            origin = origin.saturating_add(paragraph.chars()).saturating_add(1);
            if index >= start && out.len() >= budget {
                break;
            }
        }
        let end = out.last().is_none_or(|(index, _, _)| index.saturating_add(1) >= paragraphs.len());
        (out, end)
    }

    fn present(&mut self, app: &MailApp, locale: &str, place: Place, origin: usize, issues: &[(Range<usize>, String)]) -> bool {
        for (span, word) in issues {
            let nth = self.bump(place, word);
            if self.ignored.iter().any(|once| once.place == place && once.word == *word && once.nth == nth) {
                continue;
            }
            let from = origin.saturating_add(span.start);
            let to = origin.saturating_add(span.end);
            let source = match place {
                Place::Subject => app.session.compose.as_ref().map(|c| c.subject.clone()).unwrap_or_default(),
                Place::Body => app.session.doc().map(|(body, _)| body.plain()).unwrap_or_default(),
            };
            let (context, emphasis) = context_of(&source, from, to);
            self.current = Some(Mistake {
                place,
                from,
                to,
                word: word.clone(),
                nth,
                words: suggest(app, locale, span.clone(), word, origin),
                context,
                emphasis,
            });
            self.filled = false;
            self.revealed = false;
            return true;
        }
        false
    }

    fn bump(&mut self, place: Place, word: &str) -> usize {
        if let Some(slot) = self.counts.iter_mut().find(|(at, seen, _)| *at == place && seen == word) {
            let nth = slot.2;
            slot.2 = slot.2.saturating_add(1);
            return nth;
        }
        self.counts.push((place, word.to_string(), 1));
        0
    }

    fn restart(&mut self) {
        self.current = None;
        self.step = Step::Subject;
        self.counts.clear();
        self.partial = false;
        self.complete = false;
        self.revealed = false;
        self.filled = false;
        self.note.clear();
    }

    fn draw_checking(&self, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        ui.label(egui::RichText::new(tl!("Checking spelling…")).color(t.text));
        ui.ctx().request_repaint();
        let mut done = false;
        ui.add_space(4.0);
        button_row(ui, |ui| done = plain_button(ui, tl!("Close")).clicked());
        done
    }

    fn draw_done(&self, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let said = if self.partial {
            tl!("The spelling check is complete. A part of the message was too long to check.")
        } else {
            tl!("The spelling check is complete.")
        };
        ui.label(egui::RichText::new(said).color(t.text));
        ui.add_space(4.0);
        let mut done = false;
        button_row(ui, |ui| done = plain_button(ui, tl!("Close")).clicked());
        done
    }

    fn draw_mistake(&mut self, app: &mut MailApp, ui: &mut Ui, mistake: &Mistake) -> bool {
        let t = Tokens::get(ui.ctx());
        if !self.revealed {
            self.revealed = true;
            if mistake.place == Place::Body && app.execute("compose.select", json!({"from": mistake.from, "to": mistake.to})).is_ok() {
                crate::compose::editor::reveal_spelling(app, ui.ctx());
            }
        }
        let mut focus = false;
        if !self.filled {
            self.replacement = mistake.words.first().cloned().unwrap_or_else(|| mistake.word.clone());
            self.filled = true;
            focus = true;
        }
        let button_w = button_width(ui);
        let mut act = None;
        ui.horizontal_top(|ui| {
            let left = (ui.available_width() - button_w - 12.0).max(240.0);
            ui.vertical(|ui| {
                ui.set_width(left);
                ui.label(egui::RichText::new(tl!("Not in dictionary")).color(t.text_dim).font(regular(12.0)));
                ui.label(egui::RichText::new(mistake.word.as_str()).font(semibold(18.0)).color(t.text));
                ui.add(egui::Label::new(emphasis(&t, &mistake.context, mistake.emphasis.clone())).wrap());
                ui.label(tl!("Change to"));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.replacement)
                        .id(egui::Id::unique("spelling-replacement"))
                        .char_limit(255)
                        .desired_width(f32::INFINITY)
                        .font(regular(14.0)),
                );
                let resp = crate::text_menu::field(resp);
                app.hit.add("spellingReplacement", 0, resp.rect);
                if focus {
                    resp.request_focus();
                }
                ui.label(tl!("Suggestions"));
                egui::Frame::NONE.stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(4.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("spelling-suggestions").max_height(132.0).auto_shrink([false, true]).show(ui, |ui| {
                        if mistake.words.is_empty() {
                            ui.label(egui::RichText::new(tl!("No suggestions")).color(t.text_dim));
                        }
                        for (index, word) in mistake.words.iter().enumerate() {
                            let resp = ui.selectable_label(self.replacement == *word, word);
                            if resp.clicked() {
                                self.replacement.clone_from(word);
                            }
                            if resp.double_clicked() {
                                act = Some(Act::Change);
                            }
                            app.hit.add("spellingSuggestion", index as u64, resp.rect);
                        }
                    });
                });
                if !self.note.is_empty() {
                    ui.label(egui::RichText::new(self.note.as_str()).color(t.red));
                }
            });
            ui.vertical(|ui| {
                ui.set_width(button_w);
                ui.spacing_mut().item_spacing.y = 6.0;
                if press(ui, app, "spellingChange", tl!("Change"), true, button_w) {
                    act = Some(Act::Change);
                }
                if press(ui, app, "spellingChangeAll", tl!("Change All"), false, button_w) {
                    act = Some(Act::ChangeAll);
                }
                if press(ui, app, "spellingIgnoreOnce", tl!("Ignore Once"), false, button_w) {
                    act = Some(Act::IgnoreOnce);
                }
                if press(ui, app, "spellingIgnoreAll", tl!("Ignore All"), false, button_w) {
                    act = Some(Act::IgnoreAll);
                }
                if press(ui, app, "spellingAdd", tl!("Add to Dictionary"), false, button_w) {
                    act = Some(Act::Add);
                }
                if press(ui, app, "spellingClose", tl!("Close"), false, button_w) {
                    act = Some(Act::Close);
                }
            });
        });
        if act.as_ref().is_some_and(|act| matches!(act, Act::Close)) {
            return true;
        }
        if let Some(act) = act {
            self.perform(app, mistake, act);
            let locale = spelling::locale(app, ui.ctx()).unwrap_or_default();
            if !locale.is_empty() && self.current.is_none() && !self.complete && self.scan_some(app, &locale) {
                app.status(tl!("The spelling dictionary is unavailable"));
                return true;
            }
        }
        false
    }

    fn perform(&mut self, app: &mut MailApp, mistake: &Mistake, act: Act) {
        match act {
            Act::Close => {}
            Act::IgnoreOnce => self.skip(mistake),
            Act::IgnoreAll => match app.execute("ui.dictionary.ignore", json!({"word": mistake.word})) {
                Ok(_) => self.restart(),
                Err(e) => self.note = e,
            },
            Act::Add => match app.execute("ui.dictionary.add", json!({"word": mistake.word})) {
                Ok(_) => self.restart(),
                Err(e) => self.note = e,
            },
            Act::Change => self.change(app, mistake, false),
            Act::ChangeAll => self.change(app, mistake, true),
        }
    }

    fn skip(&mut self, mistake: &Mistake) {
        self.ignored.push(Once { place: mistake.place, word: mistake.word.clone(), nth: mistake.nth });
        self.restart();
    }

    fn change(&mut self, app: &mut MailApp, mistake: &Mistake, all: bool) {
        let replacement = clean_replacement(&self.replacement);
        if replacement == mistake.word {
            if all {
                self.note = tl!("That's the same word.").to_string();
                return;
            }
            self.skip(mistake);
            return;
        }
        if all {
            self.change_all(app, mistake, &replacement);
            return;
        }
        match mistake.place {
            Place::Subject => match app
                .execute("compose.replaceSubject", json!({"from": mistake.from, "to": mistake.to, "expected": mistake.word, "text": replacement}))
            {
                Ok(_) => self.restart(),
                Err(e) => self.note = e,
            },
            Place::Body => match app
                .execute("compose.replaceText", json!({"from": mistake.from, "to": mistake.to, "expected": mistake.word, "text": replacement}))
            {
                Ok(_) => self.restart(),
                Err(e) => self.note = e,
            },
        }
    }

    /// Every whole-word occurrence. The body is one `edit.replace` (one undo step). The Subject
    /// is one `compose.set`, and only while the document is the message: a signature open for
    /// editing is not that message. `edit.replace` uses the document's whole reach, which is the
    /// signature itself when that is what is open, and the whole message (including its signature
    /// block) otherwise.
    fn change_all(&mut self, app: &mut MailApp, mistake: &Mistake, replacement: &str) {
        if app.session.signature_edit.is_none()
            && let Some(subject) = app.session.compose.as_ref().map(|c| c.subject.clone())
        {
            let next = replace_whole_words(&subject, &mistake.word, replacement);
            if next != subject
                && let Err(e) = app.execute("compose.set", json!({"subject": next}))
            {
                self.note = e;
                return;
            }
        }
        match app.execute("edit.replace", json!({"text": mistake.word, "with": replacement, "matchCase": true, "wholeWord": true, "all": true})) {
            Ok(_) => self.restart(),
            Err(e) => self.note = e,
        }
    }
}

fn plain_report(app: &MailApp, locale: &str, text: &str) -> mailcraft_engine::spelling::Report {
    let Some(host) = app.services.spelling.as_deref() else {
        return mailcraft_engine::spelling::Report { issues: Vec::new(), available: false, partial: false };
    };
    let filter = spelling::filter_of(app);
    mailcraft_engine::spelling::check_plain(host, locale, text, &filter)
}

fn suggest(app: &MailApp, locale: &str, span: Range<usize>, word: &str, origin: usize) -> Vec<String> {
    let Some(host) = app.services.spelling.as_deref() else { return Vec::new() };
    let issue = Issue { span, word: word.to_string() };
    Suggestions::new(host, locale, issue, origin, app.ui.custom_dictionary.as_slice()).words().to_vec()
}

fn clean_replacement(text: &str) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for c in text.chars() {
        if matches!(c, '\n' | '\r' | '\0') {
            continue;
        }
        count = count.saturating_add(1);
        if count > 255 {
            break;
        }
        out.push(c);
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '\'' | '\u{2019}')
}

/// `word` wherever it stands alone, in this letter case. Neighbours that are letters, digits,
/// `_` or an apostrophe keep it, so `teh` inside `Tehran` stays.
fn replace_whole_words(text: &str, word: &str, with: &str) -> String {
    if word.is_empty() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let needle: Vec<char> = word.chars().collect();
    let n = needle.len();
    let mut out = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        let end = i.saturating_add(n);
        let matched = chars.get(i..end).is_some_and(|slice| slice == needle.as_slice());
        let before_ok = i == 0 || chars.get(i.saturating_sub(1)).copied().is_some_and(|c| !is_word_char(c));
        let after_ok = chars.get(end).copied().is_none_or(|c| !is_word_char(c));
        if matched && before_ok && after_ok {
            out.push_str(with);
            i = end;
        } else if let Some(c) = chars.get(i) {
            out.push(*c);
            i = i.saturating_add(1);
        } else {
            break;
        }
    }
    out
}

/// The sentence (or line) around `[from, to)`, and where the word sits in that extract.
fn context_of(text: &str, from: usize, to: usize) -> (String, Range<usize>) {
    let total = text.chars().count();
    let from = from.min(total);
    let to = to.min(total).max(from);
    let mut sentence = 0usize;
    for (i, c) in text.chars().enumerate() {
        if i >= from {
            break;
        }
        if matches!(c, '\n' | '\r' | '.' | '!' | '?') {
            sentence = i.saturating_add(1);
        }
        if from.saturating_sub(sentence) > 100 {
            sentence = from.saturating_sub(100);
        }
    }
    let mut start = sentence;
    for (i, c) in text.chars().enumerate().skip(sentence) {
        if i >= from || !c.is_whitespace() {
            start = i.min(from);
            break;
        }
    }
    let mut end = to;
    for (i, c) in text.chars().enumerate().skip(to) {
        end = i.saturating_add(1);
        if matches!(c, '\n' | '\r' | '.' | '!' | '?') || end.saturating_sub(start) > 160 {
            break;
        }
    }
    let snippet: String = text.chars().skip(start).take(end.saturating_sub(start)).collect();
    let emphasis = from.saturating_sub(start)..to.saturating_sub(start).min(snippet.chars().count());
    (snippet, emphasis)
}

fn emphasis(t: &Tokens, text: &str, range: Range<usize>) -> egui::text::LayoutJob {
    let (before, word, after) = split_at(text, range);
    let plain = egui::TextFormat { font_id: regular(13.5), color: t.text, ..Default::default() };
    let marked = egui::TextFormat { font_id: semibold(13.5), color: t.red, ..Default::default() };
    let mut job = egui::text::LayoutJob::default();
    job.append(&before, 0.0, plain.clone());
    job.append(&word, 0.0, marked);
    job.append(&after, 0.0, plain);
    job
}

fn split_at(text: &str, range: Range<usize>) -> (String, String, String) {
    let mut before = String::new();
    let mut word = String::new();
    let mut after = String::new();
    for (i, c) in text.chars().enumerate() {
        if i < range.start {
            before.push(c);
        } else if i < range.end {
            word.push(c);
        } else {
            after.push(c);
        }
    }
    (before, word, after)
}

fn button_width(ui: &Ui) -> f32 {
    let labels = [tl!("Change"), tl!("Change All"), tl!("Ignore Once"), tl!("Ignore All"), tl!("Add to Dictionary"), tl!("Close")];
    let widest = labels.into_iter().map(|label| label_width(ui, label)).fold(0.0_f32, f32::max);
    widest + 28.0
}

fn label_width(ui: &Ui, label: &str) -> f32 {
    ui.painter().layout_no_wrap(label.to_string(), medium(12.5), egui::Color32::PLACEHOLDER).size().x
}

fn press(ui: &mut Ui, app: &mut MailApp, hit: &'static str, label: &str, primary: bool, width: f32) -> bool {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), egui::Sense::click());
    let fill = if primary {
        if resp.hovered() { t.accent_text } else { t.accent }
    } else if resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.input
    };
    let stroke = if primary { egui::Stroke::NONE } else { egui::Stroke::new(1.0, t.input_border) };
    ui.painter().rect(rect, 6.0, fill, stroke, egui::StrokeKind::Inside);
    let ink = if primary { t.on_accent } else { t.text };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, medium(12.5), ink);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    app.hit.add(hit, 0, rect);
    resp.clicked()
}

#[cfg(test)]
#[path = "spelling_dialog_tests.rs"]
mod spelling_dialog_tests;
