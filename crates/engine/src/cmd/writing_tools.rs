// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Two small tools for the message being written (#507): Insert › Date & Time, which types
//! today's date or the time at the caret in one of a few formats, and Review › Word Count, which
//! counts the words, characters and paragraphs of the body, or of the selection when there is one.
//!
//! The date is the session's clock (or the instant `at` names: the dialog's, so that OK puts in
//! what it showed) in the zone the calendar is shown in ([`Session::local`]), written in the
//! interface language ([`Session::interface_language`], `mailcraft_i18n::date`): a
//! person reads it in the message. Both work on the signature open in File › Signatures instead,
//! as the other editing commands do ([`Session::doc`]).

use mailcraft_i18n::date::{self, Day};
use mailcraft_model::RichText;
use mailcraft_time::Timestamp;
use serde_json::{Value, json};
use unicode_segmentation::UnicodeSegmentation;

use super::format::{edit, needs_doc, position, state};
use super::{bad, fail};
use crate::{CmdResult, CommandSpec, Session, p};

/// The formats Insert › Date & Time offers, in the order its dialog lists them.
pub const DATE_TIME_FORMATS: [&str; 8] = ["long", "longDate", "medium", "numeric", "iso", "numericTime", "time", "monthYear"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("insert.dateTime", "Date & Time", "Insert › Text", insert_date_time)
            .pure()
            .params(
                r#"{"format": "long" (Monday, October 26, 2026) | "longDate" (October 26, 2026) | "medium" (Oct 26, 2026) | "numeric" (10/26/2026) | "iso" (2026-10-26) | "numericTime" (10/26/2026 2:05 PM) | "time" (2:05 PM) | "monthYear" (October 2026), "at"?: number (seconds since 1970 UTC; default now)} — that time, in the interface language and on the owner's clock, typed over the selection; answers the text put in"#,
            )
            .when(needs_doc),
        CommandSpec::new("review.wordCount", "Word Count", "Review › Proofing", word_count)
            .pure()
            .key("Mod+Shift+G")
            .params(
                r#"{"from"?: number, "to"?: number} (with either: from `from`, default the start, to `to`, default the end; with neither: the selection, else the whole body) — answers {"words", "characters" (without spaces), "charactersWithSpaces", "paragraphs", "selection": bool (a part was counted, not the whole body)}; changes nothing. Words are Unicode's (UAX #29): an e-mail or web address, or `e-mail`, counts as several"#,
            )
            .when(needs_doc),
    ]
}

/// The instant `at`, as `format` writes it; `None` for a format there is no such thing as.
pub fn date_time_text(s: &Session, format: &str, at: Timestamp) -> Option<String> {
    let now = s.local(at);
    let d = now.date;
    let day = Day { year: d.year, month: d.month, day: d.day, weekday: d.weekday().index() };
    let lang = &s.interface_language;
    Some(match format {
        "long" => date::long(lang, day),
        "longDate" => date::long_without_weekday(lang, day),
        "medium" => date::medium(lang, day),
        "numeric" => date::numeric(lang, day),
        "iso" => d.iso(),
        "numericTime" => format!("{} {}", date::numeric(lang, day), s.time_said(now)),
        "time" => s.time_said(now),
        "monthYear" => date::month_year(lang, d.year, d.month),
        _ => return None,
    })
}

/// Every format with what it would put in for the instant `at`, for the dialog.
pub fn date_time_choices(s: &Session, at: Timestamp) -> Vec<(&'static str, String)> {
    DATE_TIME_FORMATS.iter().filter_map(|f| date_time_text(s, f, at).map(|text| (*f, text))).collect()
}

fn insert_date_time(s: &mut Session, v: &Value) -> CmdResult {
    let format = p::str(v, "format").ok_or_else(|| bad(format!("`format` is one of {}", DATE_TIME_FORMATS.join(", "))))?;
    let at = match v.get("at") {
        None | Some(Value::Null) => s.now(),
        Some(_) => p::i64(v, "at").ok_or_else(|| bad("`at` is a time in seconds since 1970 (UTC)"))?,
    };
    let text = date_time_text(s, format, at).ok_or_else(|| bad(format!("`format` is one of {}", DATE_TIME_FORMATS.join(", "))))?;
    edit(s, &json!({}), |b| b.type_text(&text))?;
    let mut answer = state(s);
    if let Some(map) = answer.as_object_mut() {
        map.insert("text".into(), json!(text));
    }
    Ok(answer)
}

/// What Word Count says of a stretch of text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub words: usize,
    /// Characters that are not white space (a picture is not a character).
    pub characters: usize,
    /// Characters, the spaces and tabs between words too (a paragraph break is not one).
    pub with_spaces: usize,
    /// Paragraphs with something in them besides white space.
    pub paragraphs: usize,
}

/// Count `text` in one pass over it.
///
/// A word is what Unicode's word boundaries (UAX #29) make one with a letter or a digit in it:
/// `don't` and `3.14` are one, `e-mail` is two (`e`, `mail`), each ideograph of Chinese or
/// Japanese is one (a run of katakana is one), and punctuation on its own is none. (Word counts
/// what spaces separate, so `e-mail` is one word there.)
pub fn count(text: &RichText) -> Counts {
    let mut c = Counts::default();
    for paragraph in text.paragraphs() {
        let mut filled = false;
        for run in paragraph.runs.iter().filter(|r| r.picture.is_none()) {
            for ch in run.text.chars() {
                c.with_spaces += 1;
                if !ch.is_whitespace() {
                    c.characters += 1;
                    filled = true;
                }
            }
        }
        if filled {
            c.paragraphs += 1;
            // (The paragraph whole: a word may run across a change of formatting.)
            c.words += paragraph.text().unicode_words().count();
        }
    }
    c
}

fn word_count(s: &mut Session, v: &Value) -> CmdResult {
    let (body, _) = s.doc().ok_or_else(|| fail("no message is being written"))?;
    let (from, to) = (position(v, "from")?, position(v, "to")?);
    let sel = body.selection();
    let len = body.text().len();
    // A part: the one named (from the start, or to the end, for the end not named), else the
    // selection when there is one.
    let part = match (from, to) {
        (None, None) => (!sel.is_empty()).then(|| (sel.start(), sel.end())),
        (from, to) => {
            let (from, to) = (from.unwrap_or(0), to.unwrap_or(len));
            Some((from.min(to), from.max(to)))
        }
    };
    let selection = part.is_some();
    let counts = match part {
        Some((from, to)) => count(&body.text().slice(from, to)?),
        None => count(body.text()),
    };
    Ok(json!({
        "words": counts.words,
        "characters": counts.characters,
        "charactersWithSpaces": counts.with_spaces,
        "paragraphs": counts.paragraphs,
        "selection": selection,
    }))
}
