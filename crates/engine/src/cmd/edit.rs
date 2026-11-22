// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Undo and redo. For a Microsoft account, undoing a change that reached its server is another
//! change, which the next run sends; what can't be undone there is said ([`crate::undo`]).
//!
//! Find and Replace in the message being written (or the signature open in File › Signatures):
//! plain text on one line, letter case counting only when asked, and only a whole word when asked.
//! Replace All is one step of the message's own undo (`compose.undo`). Neither reaches past the
//! owner's text ([`super::format::reach`]).

use mailcraft_model::rich::{MAX_FIND, Place};
use serde_json::{Value, json};
use unicode_general_category::{GeneralCategory, get_general_category};

use super::format::{edit, needs_doc, needs_html, reach};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Why Undo and Redo wait while a .pst file comes in a batch at a time (#815).
const IMPORTING: &str = "Undo waits until the .pst file is imported";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("edit.undo", "Undo", "Quick Access", undo).key("Mod+Z").when(|s| {
            if s.pst_running.is_some() {
                Some(IMPORTING)
            } else if s.can_undo() {
                None
            } else {
                Some("nothing to undo")
            }
        }),
        CommandSpec::new("edit.redo", "Redo", "Quick Access", redo).key("Mod+Y / Mod+Shift+Z").when(|s| {
            if s.pst_running.is_some() {
                Some(IMPORTING)
            } else if s.can_redo() {
                None
            } else {
                Some("nothing to redo")
            }
        }),
        CommandSpec::new("edit.formatPainter", "Format Painter", "Message › Clipboard", format_painter)
            .pure()
            .params(r#"{"apply"?: bool, "from"?: number, "to"?: number} — copies the look of the selection (or puts away a look copied before); apply: puts the copied look on the selection, or on from–to, as one step, and puts it away. Answers {"copied", "look"}"#)
            .when_writing(needs_html),
        CommandSpec::new("edit.find", "Find", "Format Text › Editing", find)
            .pure()
            .params(r#"{"text": string (1–255 characters, one line), "matchCase"?: bool, "wholeWord"?: bool, "backward"?: bool} — selects the next place the text is after the selection (before it, backward), going round from the other end; wholeWord: only where the text is a whole word; answers {"found", "from", "to", "count"} (count: the places in all)"#)
            .when_writing(needs_doc),
        CommandSpec::new("edit.replace", "Replace", "Format Text › Editing", replace)
            .pure()
            .params(r#"{"text": string (1–255 characters, one line), "with": string (0–255 characters, one line), "matchCase"?: bool, "wholeWord"?: bool, "all"?: bool, "spans"?: [{"from": number, "to": number}]} — puts `with` in place of the selection when it is the text, then selects the next place; wholeWord: only where the text is a whole word; all: in place of every one, as one step of the message's undo. spans: those places only, when each one is exactly `text`, as one undo step (at most 100000, in any order). spans takes precedence over wholeWord, matchCase and all, so a caller can replace the exact places it found. Answers {"replaced", "found", "from", "to"}"#)
            .when_writing(needs_doc),
    ]
}

/// An agent without Delete doesn't undo or redo a step that takes a message away (#473), that
/// has a category deleted in Outlook (#661), or that takes away contacts, events, tasks, calendars
/// or categories made since (notes and journal entries count too): refused before
/// anything moves, so the stacks stay as they are. The gate asked that
/// before the command; it is asked again here, of the mailbox the step runs on (#1674:
/// `Session::run` has settled the zone, and a move between the gate and the run can make an
/// undo take events away that it kept before).
fn refuse_taking(s: &Session, id: &str, redo: bool) -> Result<(), CmdError> {
    use crate::agent::{Class, needs_owner};
    if s.agent.as_ref().is_none_or(|allow| allow.contains(&Class::Delete)) {
        return Ok(());
    }
    let refuse = |what: &str| Err(CmdError::Failed(needs_owner(id, Class::Delete, false, what)));
    let (taken, items) = s.undo_takes(redo);
    if !taken.is_empty() {
        refuse("take away messages; nothing was changed")
    } else if !s.undo_deletes_categories(redo).is_empty() {
        refuse("delete categories in Outlook; nothing was changed")
    } else if !items.is_empty() {
        refuse("take away contacts, events, tasks, calendars or categories made since, for good; nothing was changed")
    } else {
        Ok(())
    }
}

/// The step is not taken while it would put a message being written back in an Outbox or take it
/// away under its composer (#950): written there, it could be sent with its edits lost, twice,
/// or deleted with its queued copy.
fn refuse_under_composer(s: &Session, redo: bool) -> Result<(), CmdError> {
    // An agent the owner allowed to delete has been asked already (#473), so taking the message
    // away is let be for it (the composer keeps what it holds and saves it again); putting it back
    // in an Outbox never is, whoever asks.
    let held = s.all_composing().filter_map(|c| c.draft).filter(|d| s.undo_changes_message(redo, *d)).any(|d| {
        let (takes, queues) = s.undo_takes_or_queues(redo, d);
        queues || (takes && s.agent.is_none())
    });
    if held { Err(fail("that step changes a message that is open for writing: close it first")) } else { Ok(()) }
}

fn undo(s: &mut Session, _: &Value) -> CmdResult {
    refuse_under_composer(s, false)?;
    refuse_taking(s, "edit.undo", false)?;
    let label = s.undo_label();
    s.undo();
    let said = said(s);
    Ok(json!({"undone": label, "notUndone": said}))
}

fn redo(s: &mut Session, _: &Value) -> CmdResult {
    refuse_under_composer(s, true)?;
    refuse_taking(s, "edit.redo", true)?;
    let label = s.redo_label();
    s.redo();
    let said = said(s);
    Ok(json!({"redone": label, "notRedone": said}))
}

/// What the undo or redo couldn't do, said in the status bar too.
fn said(s: &mut Session) -> Vec<String> {
    let said = s.undo_said().to_vec();
    if !said.is_empty() {
        s.sync_status = said.join(" ");
    }
    said
}

/// A text the params give for Find or Replace: at most [`MAX_FIND`] characters, on one line.
fn find_text(v: &Value, key: &str, empty: bool) -> Result<String, CmdError> {
    let text = match v.get(key) {
        Some(Value::String(t)) => t,
        _ => return Err(bad(format!("`{key}` (string) is required"))),
    };
    if text.contains(['\n', '\r']) {
        return Err(bad(format!("`{key}` is one line of text")));
    }
    if text.is_empty() && !empty {
        return Err(bad(format!("`{key}` is empty")));
    }
    if text.chars().count() > MAX_FIND {
        return Err(bad(format!("`{key}` is at most {MAX_FIND} characters")));
    }
    Ok(text.clone())
}

/// A letter, a digit, `_`, or an apostrophe (`'` or `’`): part of a word for whole-word Find and Replace.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '\'' | '’')
}

/// U+200C (ZWNJ) or U+200D (ZWJ). Between two word characters it is inside the word.
fn is_word_joiner(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}')
}

/// A combining mark (general category Mn, Mc or Me): part of the word it follows.
fn is_combining_mark(c: char) -> bool {
    matches!(get_general_category(c), GeneralCategory::NonspacingMark | GeneralCategory::SpacingMark | GeneralCategory::EnclosingMark)
}

/// The places `text` is in the owner's text. Without `whole_word` that is one pass of
/// [`mailcraft_model::RichText::find`], which reports places that do not overlap. With it, the
/// plain text is walked: a place that is a whole word is kept and the walk continues at its end,
/// and any other place is passed by one character, so a later whole word that overlapped it is
/// still found. Places that are kept still do not overlap.
fn places(s: &Session, text: &str, match_case: bool, whole_word: bool) -> Vec<Place> {
    let Some((body, _)) = s.doc() else { return Vec::new() };
    let end = reach(s);
    if whole_word { whole_word_places(&body.plain(), text, match_case, end) } else { body.text().find(text, match_case, 0, end) }
}

/// `c` as [`mailcraft_model::RichText::find`] compares it when letter case does not count.
fn fold_char(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(folded), None) => folded,
        _ => c,
    }
}

/// The owner's text, indexed by character, and the same characters folded for one search.
struct Scanned<'a> {
    text: &'a str,
    /// Byte offset of character `i` in `text`, and one past the last character indexed.
    byte: Vec<usize>,
    /// Those characters as Find compares them, so the next occurrence is one search, not a scan of every place.
    folded: String,
    /// Byte offset of character `i` in `folded`, and one past the last.
    folded_byte: Vec<usize>,
    needle: String,
    needle_len: usize,
}

impl<'a> Scanned<'a> {
    /// The first `end` characters of `text` (or the whole of it, when it is shorter).
    fn new(text: &'a str, end: usize, needle: &str, match_case: bool) -> Self {
        let mut byte = Vec::new();
        let mut folded = String::new();
        let mut folded_byte = Vec::new();
        let mut n = 0usize;
        let mut last = 0usize;
        for (at, c) in text.char_indices() {
            if n >= end {
                break;
            }
            byte.push(at);
            folded_byte.push(folded.len());
            folded.push(if match_case { c } else { fold_char(c) });
            n = n.saturating_add(1);
            last = at.saturating_add(c.len_utf8());
        }
        byte.push(last);
        folded_byte.push(folded.len());
        let needle = if match_case { needle.to_string() } else { needle.chars().map(fold_char).collect() };
        let needle_len = needle.chars().count();
        Scanned { text, byte, folded, folded_byte, needle, needle_len }
    }

    fn char_at(&self, i: usize) -> Option<char> {
        let from = *self.byte.get(i)?;
        let to = *self.byte.get(i.saturating_add(1))?;
        self.text.get(from..to)?.chars().next()
    }

    /// The next occurrence of `needle` at character `from` or after it, within the indexed text.
    fn find_from(&self, from: usize) -> Option<usize> {
        if self.needle.is_empty() {
            return None;
        }
        let origin = *self.folded_byte.get(from)?;
        let rel = self.folded.get(origin..)?.find(&self.needle)?;
        let abs = origin.saturating_add(rel);
        let at = self.folded_byte.partition_point(|&b| b < abs);
        if self.folded_byte.get(at).copied() != Some(abs) || at < from {
            return None;
        }
        let stop = at.saturating_add(self.needle_len);
        let n = self.byte.len().saturating_sub(1);
        (stop <= n && stop > at).then_some(at)
    }

    /// The character before `start` and the character after `end` are not part of the same word.
    /// A combining mark after the place keeps it inside the word, as does a word character with
    /// only marks between it and the place, and a place that itself starts on a mark.
    /// A ZWNJ or ZWJ joins the word only when a word character sits on its other side.
    fn is_whole_word(&self, start: usize, end: usize) -> bool {
        if start >= end {
            return false;
        }
        let Some(first) = self.char_at(start) else { return false };
        if is_combining_mark(first) {
            return false;
        }
        !self.joins_word_before(start) && !self.joins_word_after(end)
    }

    fn joins_word_before(&self, start: usize) -> bool {
        let mut i = start;
        while i > 0 {
            let Some(c) = self.char_at(i.saturating_sub(1)) else { break };
            if is_combining_mark(c) || is_word_joiner(c) {
                i = i.saturating_sub(1);
                continue;
            }
            return is_word_char(c);
        }
        false
    }

    fn joins_word_after(&self, end: usize) -> bool {
        let mut i = end;
        let mut crossed_joiner = false;
        loop {
            let Some(c) = self.char_at(i) else { return false };
            if is_word_joiner(c) {
                crossed_joiner = true;
                i = i.saturating_add(1);
                continue;
            }
            if is_combining_mark(c) {
                if !crossed_joiner {
                    return true;
                }
                i = i.saturating_add(1);
                continue;
            }
            return is_word_char(c);
        }
    }
}

/// [`places`] with `whole_word`. The walk moves forward on every step and stops at `end`.
fn whole_word_places(text: &str, needle: &str, match_case: bool, end: usize) -> Vec<Place> {
    let needle_len = needle.chars().count();
    if needle_len == 0 || needle_len > MAX_FIND {
        return Vec::new();
    }
    let scanned = Scanned::new(text, end, needle, match_case);
    let mut kept = Vec::new();
    let mut p = 0usize;
    while p < end {
        let Some(start) = scanned.find_from(p) else { break };
        let stop = start.saturating_add(scanned.needle_len);
        if stop > end || stop <= start {
            break;
        }
        let next = if scanned.is_whole_word(start, stop) {
            kept.push((start, stop));
            stop
        } else {
            start.saturating_add(1)
        };
        // `end` is the owner's text. A step that does not move on cannot be trusted to finish.
        if next <= p {
            break;
        }
        p = next;
    }
    kept
}

/// Select the place after the selection (before it, `backward`), going round; the answer.
fn select_next(s: &mut Session, places: &[Place], backward: bool) -> CmdResult {
    let sel = s.doc().map(|(b, _)| b.selection()).unwrap_or_default();
    let next = if backward {
        places.iter().rev().find(|(_, b)| *b <= sel.start()).or(places.last())
    } else {
        places.iter().find(|(a, _)| *a >= sel.end()).or(places.first())
    };
    let Some(&(from, to)) = next else {
        return Ok(json!({"found": false, "count": 0}));
    };
    s.doc_mut().ok_or_else(|| fail("no message is being written"))?.body.select(from, to).map_err(|e| fail(e.to_string()))?;
    Ok(json!({"found": true, "from": from, "to": to, "count": places.len()}))
}

fn format_painter(s: &mut Session, v: &Value) -> CmdResult {
    let armed = s.doc().is_some_and(|(b, _)| b.painter().is_some());
    if p::bool(v, "apply").unwrap_or(false) {
        if !armed {
            return Err(fail("no look was copied: press Format Painter on the text to copy it from first"));
        }
        edit(s, v, |b| b.paste_look())?;
        return Ok(json!({"copied": false}));
    }
    if armed {
        // A second press puts it away.
        edit(s, &json!({}), |b| {
            b.drop_look();
            Ok(())
        })?;
        return Ok(json!({"copied": false}));
    }
    let look = edit(s, v, |b| {
        b.copy_look();
        Ok(b.painter().cloned())
    })?;
    Ok(json!({"copied": true, "look": look}))
}

fn find(s: &mut Session, v: &Value) -> CmdResult {
    let text = find_text(v, "text", false)?;
    let match_case = p::bool(v, "matchCase").unwrap_or(false);
    let whole_word = p::bool(v, "wholeWord").unwrap_or(false);
    let places = places(s, &text, match_case, whole_word);
    select_next(s, &places, p::bool(v, "backward").unwrap_or(false))
}

/// Most places one `spans` list may name. A longer list is refused and changes nothing.
const MAX_SPANS: usize = 100_000;

fn replace(s: &mut Session, v: &Value) -> CmdResult {
    let text = find_text(v, "text", false)?;
    let with = find_text(v, "with", true)?;
    if let Some(spans) = v.get("spans") {
        return replace_spans(s, &text, &with, spans);
    }
    let match_case = p::bool(v, "matchCase").unwrap_or(false);
    let whole_word = p::bool(v, "wholeWord").unwrap_or(false);
    let found = places(s, &text, match_case, whole_word);
    if p::bool(v, "all").unwrap_or(false) {
        if !found.is_empty() {
            edit(s, &json!({}), |b| b.replace_places(&found, &with))?;
        }
        return Ok(json!({"replaced": found.len(), "found": false}));
    }
    // The selection, when it is one of the places; then the next.
    let sel = s.doc().map(|(b, _)| b.selection()).unwrap_or_default();
    let here = found.iter().find(|p| **p == (sel.start(), sel.end())).copied();
    if let Some(place) = here {
        edit(s, &json!({}), |b| b.replace_places(&[place], &with))?;
    }
    let after = places(s, &text, match_case, whole_word);
    let mut out = select_next(s, &after, false)?;
    if let Some(o) = out.as_object_mut() {
        o.insert("replaced".into(), json!(usize::from(here.is_some())));
    }
    Ok(out)
}

/// Put `with` at each span, when that span is exactly `text`, as one undo step. A span that is
/// not that text, or not a place in the text, or past the owner's text, changes nothing.
fn replace_spans(s: &mut Session, text: &str, with: &str, spans: &Value) -> CmdResult {
    let Some(list) = spans.as_array() else {
        return Err(bad("`spans` is a list of {from, to}"));
    };
    if list.len() > MAX_SPANS {
        return Err(bad(format!("`spans` has at most {MAX_SPANS} places")));
    }
    let Some((body, _)) = s.doc() else {
        return Err(fail("no message is being written"));
    };
    let plain = body.plain();
    let end = reach(s);
    let mut places = Vec::with_capacity(list.len());
    for item in list {
        let from = item.get("from").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
        let to = item.get("to").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
        let (Some(from), Some(to)) = (from, to) else {
            return Err(bad("`spans` is a list of {from, to}"));
        };
        if from >= to {
            return Err(bad("`spans` is a list of {from, to}"));
        }
        places.push((from, to));
    }
    if places.iter().any(|&(_, to)| to > end) {
        return Err(fail("the text has changed at the place to replace"));
    }
    places.sort_by_key(|place| place.0);
    if !spans_match(&plain, text, &places) {
        return Err(fail("the text has changed at the place to replace"));
    }
    if !places.is_empty() {
        edit(s, &json!({}), |b| b.replace_places(&places, with))?;
    }
    Ok(json!({"replaced": places.len(), "found": false}))
}

/// Sorted `places` do not overlap, and each one is exactly `text`. One walk of `plain`,
/// comparing borrowed slices, so a long list does not re-read the draft for every place.
/// `false` leaves the text unchanged: the caller has not edited yet.
fn spans_match(plain: &str, text: &str, places: &[(usize, usize)]) -> bool {
    let mut chars = plain.char_indices().peekable();
    let mut index = 0usize;
    let mut last = 0usize;
    for &(from, to) in places {
        if from < last || to <= from {
            return false;
        }
        while index < from {
            if chars.next().is_none() {
                return false;
            }
            index = index.saturating_add(1);
        }
        let start_byte = chars.peek().map(|(byte, _)| *byte).unwrap_or(plain.len());
        while index < to {
            if chars.next().is_none() {
                return false;
            }
            index = index.saturating_add(1);
        }
        let end_byte = chars.peek().map(|(byte, _)| *byte).unwrap_or(plain.len());
        if plain.get(start_byte..end_byte) != Some(text) {
            return false;
        }
        last = to;
    }
    true
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod find_tests;
