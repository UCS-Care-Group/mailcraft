// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Spelling in the Subject, the same check as a plain line of the body: the owner's filter, the
//! wavy underline, and the right-click list above Cut, Copy and Paste.

use egui::widgets::text_edit::TextEditOutput;
use egui::{Context, Id, Pos2, vec2};

use crate::MailApp;
use crate::spelling::{self, Choice, Issue, Suggestions};
use crate::theme::Tokens;

/// Issues remembered for one Subject, so a frame that shows the same line does not ask again.
#[derive(Clone)]
struct Cache {
    text: String,
    locale: String,
    rev: u64,
    issues: Vec<Issue>,
    /// `false` when the host had no dictionary. That result is used only until `until`.
    available: bool,
    until: f64,
}

const CHECKS: &str = "mailcraft-subject-spelling-checks";
const MARKS: &str = "mailcraft-subject-spelling-marks";

/// How many times the Subject was sent to the spelling host in this context, for tests.
#[cfg(test)]
pub(crate) fn check_count(ctx: &Context) -> u64 {
    ctx.data(|data| data.get_temp(Id::unique(CHECKS))).unwrap_or(0)
}

/// Squiggles painted on the Subject in the last frame, for tests.
#[cfg(test)]
pub(crate) fn mark_count(ctx: &Context) -> usize {
    ctx.data(|data| data.get_temp(Id::unique(MARKS))).unwrap_or(0)
}

/// Misspellings in `text`, cached by the line, the locale and [`MailApp::spelling_rev`].
/// A partial report is kept as the host returned it.
pub(crate) fn cached_issues(ctx: &Context, app: &MailApp, slot: Id, locale: &str, text: &str, now: f64) -> Vec<Issue> {
    let key = slot.with("subject-spelling");
    let rev = app.spelling_rev;
    if let Some(cache) = ctx.data(|data| data.get_temp::<Cache>(key))
        && cache.text == text
        && cache.locale == locale
        && cache.rev == rev
        && (cache.available || now < cache.until)
    {
        return cache.issues;
    }
    let Some(host) = app.services.spelling.as_deref() else {
        return Vec::new();
    };
    let report = mailcraft_engine::spelling::check_plain(host, locale, text, &spelling::filter_of(app));
    let issues = report.issues.into_iter().map(|issue| Issue::at(issue.span, issue.word)).collect::<Vec<_>>();
    let until = if report.available { 0.0 } else { now + 5.0 };
    if !report.available {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((until - now).max(0.0)));
    }
    ctx.data_mut(|data| {
        let count = data.get_temp::<u64>(Id::unique(CHECKS)).unwrap_or(0);
        data.insert_temp(Id::unique(CHECKS), count.saturating_add(1));
        data.insert_temp(
            key,
            Cache { text: text.to_string(), locale: locale.to_string(), rev, issues: issues.clone(), available: report.available, until },
        );
    });
    issues
}

/// Where the galley was painted. A long Subject scrolls sideways; `galley_pos` already includes
/// that, and the galley's own left edge is not part of the screen position.
fn galley_origin(out: &TextEditOutput) -> Pos2 {
    out.galley_pos - vec2(out.galley.rect.left(), 0.0)
}

/// Underlines, and the menu for a right click on a marked word. Returns whether a suggestion
/// replaced the Subject, so the field does not write the text it showed before that.
pub(crate) fn decorate(app: &mut MailApp, ui: &mut egui::Ui, out: &TextEditOutput, shown: &str) -> bool {
    let ctx = ui.ctx().clone();
    let now = ui.input(|input| input.time);
    let typed_key = out.response.id.with("subject-typed-until");
    if out.response.changed() {
        ctx.data_mut(|data| data.insert_temp(typed_key, now + spelling::TYPING_PAUSE));
    }
    let typed_until = ctx.data(|data| data.get_temp::<f64>(typed_key)).unwrap_or(0.0);
    let typing = out.response.has_focus() && now < typed_until;
    if typing {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((typed_until - now).max(0.0)));
    }
    let locale = spelling::locale(app, &ctx);
    let caret = out.cursor_range.filter(|range| range.is_empty()).map(|range| usize::from(range.primary.index));
    let menu_key = out.response.id.with("subject-menu");
    if out.response.secondary_clicked() {
        let suggestions = suggestion_at(app, &ctx, out, shown, locale.as_deref());
        ctx.data_mut(|data| match &suggestions {
            Some(suggestions) => {
                data.insert_temp(menu_key, suggestions.clone());
            }
            None => {
                data.remove::<Suggestions>(menu_key);
            }
        });
    }
    let mut marks = 0usize;
    if app.ui.check_spelling_as_you_type
        && app.services.spelling.is_some()
        && let Some(locale) = locale.as_deref()
    {
        let issues = cached_issues(&ctx, app, out.response.id, locale, shown, now);
        let origin = galley_origin(out);
        let painter = ui.painter().with_clip_rect(out.text_clip_rect);
        let ink = Tokens::get(&ctx).icon_red;
        for issue in &issues {
            let hide = typing && caret.is_some_and(|index| index > issue.span.start && index <= issue.span.end);
            if hide {
                continue;
            }
            let start = out.galley.pos_from_cursor(egui::text::CCursor::new(issue.span.start));
            let end = out.galley.pos_from_cursor(egui::text::CCursor::new(issue.span.end));
            spelling::paint_squiggle(&painter, origin.x + start.min.x, origin.x + end.min.x, origin.y + start.max.y - 1.0, ink);
            marks = marks.saturating_add(1);
        }
    }
    ctx.data_mut(|data| data.insert_temp(Id::unique(MARKS), marks));
    let mut chosen = None;
    crate::text_menu::field_with(out.response.response.clone(), |ui| {
        let suggestions = ui.ctx().data(|data| data.get_temp::<Suggestions>(menu_key));
        suggestions.as_ref().is_some_and(|suggestions| suggestions.show(ui, &mut chosen))
    });
    let Some(choice) = chosen else { return false };
    let Some(suggestions) = ctx.data_mut(|data| {
        let found = data.get_temp::<Suggestions>(menu_key);
        data.remove::<Suggestions>(menu_key);
        found
    }) else {
        return false;
    };
    out.response.request_focus();
    match choice {
        Choice::Replace(word) => subject_replaced(app, out, shown, &suggestions, &word),
        Choice::IgnoreAll => {
            let _ = app.run("ui.dictionary.ignore", serde_json::json!({"word": suggestions.word()}));
            false
        }
        Choice::Add => {
            let _ = app.run("ui.dictionary.add", serde_json::json!({"word": suggestions.word()}));
            false
        }
    }
}

/// Whether the suggestion replaced the Subject. A same-frame edit is part of the stored line: the
/// replacement is applied to the text on screen, and a replacement the line no longer matches
/// leaves that edit to be stored on its own.
fn subject_replaced(app: &mut MailApp, out: &TextEditOutput, shown: &str, suggestions: &Suggestions, word: &str) -> bool {
    let (from, to) = suggestions.place();
    let expected = suggestions.word();
    if out.response.changed() {
        let Some(next) = splice_word(shown, from, to, expected, word) else {
            return false;
        };
        return app.run("compose.set", serde_json::json!({"subject": next})).is_ok();
    }
    app.run("compose.replaceSubject", serde_json::json!({"from": from, "to": to, "expected": expected, "text": word})).is_ok()
}

/// `shown` with `expected` at `[from, to)` written as `with`.
fn splice_word(shown: &str, from: usize, to: usize, expected: &str, with: &str) -> Option<String> {
    let chars: Vec<char> = shown.chars().collect();
    let slice: String = chars.get(from..to)?.iter().collect();
    if slice != expected {
        return None;
    }
    let mut next = String::new();
    next.extend(chars.get(..from)?.iter().copied());
    next.push_str(with);
    next.extend(chars.get(to..)?.iter().copied());
    Some(next)
}

/// Suggestions for the marked word under the pointer, checked on demand when as-you-type is off.
fn suggestion_at(app: &MailApp, ctx: &Context, out: &TextEditOutput, shown: &str, locale: Option<&str>) -> Option<Suggestions> {
    let locale = locale?;
    let host = app.services.spelling.as_deref()?;
    let pointer = ctx.input(|input| input.pointer.interact_pos())?;
    let index = out.galley.cursor_from_pos(pointer - galley_origin(out)).index;
    let index = usize::from(index);
    let now = ctx.input(|input| input.time);
    let issue = cached_issues(ctx, app, out.response.id, locale, shown, now).into_iter().find(|issue| issue.span.contains(&index))?;
    Some(Suggestions::new(host, locale, issue, 0, app.ui.custom_dictionary.as_slice()))
}
