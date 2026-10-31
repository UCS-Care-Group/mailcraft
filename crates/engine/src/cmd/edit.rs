// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Undo and redo. For a Microsoft account, undoing a change that reached its server is another
//! change, which the next run sends; what can't be undone there is said ([`crate::undo`]).
//!
//! Find and Replace in the message being written (or the signature open in File › Signatures):
//! plain text on one line, letter case counting only when asked. Replace All is one step of the
//! message's own undo (`compose.undo`). Neither reaches past the owner's text
//! ([`super::format::reach`]).

use mailcraft_model::rich::{MAX_FIND, Place};
use serde_json::{Value, json};

use super::format::{edit, needs_doc, needs_html, reach};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("edit.undo", "Undo", "Quick Access", undo).key("Mod+Z").when(|s| if s.can_undo() { None } else { Some("nothing to undo") }),
        CommandSpec::new("edit.redo", "Redo", "Quick Access", redo)
            .key("Mod+Y / Mod+Shift+Z")
            .when(|s| if s.can_redo() { None } else { Some("nothing to redo") }),
        CommandSpec::new("edit.formatPainter", "Format Painter", "Message › Clipboard", format_painter)
            .pure()
            .params(r#"{"apply"?: bool, "from"?: number, "to"?: number} — copies the look of the selection (or puts away a look copied before); apply: puts the copied look on the selection, or on from–to, as one step, and puts it away. Answers {"copied", "look"}"#)
            .when(needs_html),
        CommandSpec::new("edit.find", "Find", "Format Text › Editing", find)
            .pure()
            .params(r#"{"text": string (1–255 characters, one line), "matchCase"?: bool, "backward"?: bool} — selects the next place the text is after the selection (before it, backward), going round from the other end; answers {"found", "from", "to", "count"} (count: the places in all)"#)
            .when(needs_doc),
        CommandSpec::new("edit.replace", "Replace", "Format Text › Editing", replace)
            .pure()
            .params(r#"{"text": string (1–255 characters, one line), "with": string (0–255 characters, one line), "matchCase"?: bool, "all"?: bool} — puts `with` in place of the selection when it is the text, then selects the next place; all: in place of every one, as one step of the message's undo. Answers {"replaced", "found", "from", "to"}"#)
            .when(needs_doc),
    ]
}

fn undo(s: &mut Session, _: &Value) -> CmdResult {
    let label = s.undo_label();
    s.undo();
    let said = said(s);
    Ok(json!({"undone": label, "notUndone": said}))
}

fn redo(s: &mut Session, _: &Value) -> CmdResult {
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

/// The places `text` is in the owner's text.
fn places(s: &Session, text: &str, match_case: bool) -> Vec<Place> {
    s.doc().map(|(b, _)| b.text().find(text, match_case, 0, reach(s))).unwrap_or_default()
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
    let places = places(s, &text, match_case);
    select_next(s, &places, p::bool(v, "backward").unwrap_or(false))
}

fn replace(s: &mut Session, v: &Value) -> CmdResult {
    let text = find_text(v, "text", false)?;
    let with = find_text(v, "with", true)?;
    let match_case = p::bool(v, "matchCase").unwrap_or(false);
    let found = places(s, &text, match_case);
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
    let after = places(s, &text, match_case);
    let mut out = select_next(s, &after, false)?;
    if let Some(o) = out.as_object_mut() {
        o.insert("replaced".into(), json!(usize::from(here.is_some())));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod find_tests;
