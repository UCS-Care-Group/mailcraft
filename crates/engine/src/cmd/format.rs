// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing in rich text: the selection in the message being written, typing, formatting and
//! undo inside the message (`mailcraft_model::rich`).
//!
//! These are pure commands: they change the message being written, never the mailbox, so they
//! are not on the mailbox's undo history. The message has its own (`compose.undo`). While a
//! signature is open in File › Signatures they work on it instead ([`Session::doc`]).
//! Positions count characters of the body's text, a paragraph break being one.

use mailcraft_model::rich::{Font, Format, ListKind, MAX_SIZE, MIN_SIZE, safe_color};
use mailcraft_model::{ModelError, RichBody, RichText};
use serde_json::{Value, json};

use super::{bad, fail, needs_compose};
use crate::Doc;
use crate::{BodyFormat, CmdError, CmdResult, CommandSpec, Compose, Session, p};

const RANGE: &str = r#"{"from"?: number, "to"?: number} (default: the selection)"#;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("compose.select", "Select Text", "Message", select).pure().params(r#"{"from": number, "to"?: number} (to: the caret, default from)"#).when(needs_doc),
        CommandSpec::new("compose.type", "Type", "Message", type_text)
            .pure()
            .params(r#"{"text": string} — over the selection, in the formatting at the caret; a line break starts a paragraph (of the same kind; a line break alone in an empty list item ends the list there)"#)
            .when(needs_doc),
        CommandSpec::new("compose.delete", "Delete Text", "Message", delete)
            .pure()
            .params(r#"{"from"?: number, "to"?: number, "forward"?: bool} (default: the selection, else the character before the caret, or after it with forward)"#)
            .when(needs_doc),
        CommandSpec::new("compose.undo", "Undo Typing", "Message", undo).pure().when(|s| match s.doc() {
            None => Some("no message is being written"),
            Some((b, _)) if !b.can_undo() => Some("nothing to undo in the message"),
            Some(_) => None,
        }),
        CommandSpec::new("compose.redo", "Redo Typing", "Message", redo).pure().when(|s| match s.doc() {
            None => Some("no message is being written"),
            Some((b, _)) if !b.can_redo() => Some("nothing to redo in the message"),
            Some(_) => None,
        }),
        CommandSpec::new("format.bold", "Bold", "Message › Basic Text", bold).pure().params(RANGE).when(needs_html),
        CommandSpec::new("format.italic", "Italic", "Message › Basic Text", italic).pure().params(RANGE).when(needs_html),
        CommandSpec::new("format.underline", "Underline", "Message › Basic Text", underline).pure().params(RANGE).when(needs_html),
        CommandSpec::new("format.size", "Font Size", "Message › Basic Text", size)
            .pure()
            .params(r#"{"size": number (points, 8–36) | null (the message's own size), "from"?: number, "to"?: number}"#)
            .when(needs_html),
        CommandSpec::new("format.color", "Font Color", "Message › Basic Text", color)
            .pure()
            .params(r##"{"value": "#rrggbb" | null (the message's own colour), "from"?: number, "to"?: number}"##)
            .when(needs_html),
        CommandSpec::new("format.font", "Font", "Message › Basic Text", font)
            .pure()
            .params(r#"{"value": "Arial"|"Calibri"|"Georgia"|"Verdana"|"System" | null (the message's own font), "from"?: number, "to"?: number}"#)
            .when(needs_html),
        CommandSpec::new("format.bullets", "Bullets", "Message › Basic Text", bullets).pure().params(RANGE).when(needs_html),
        CommandSpec::new("format.numbering", "Numbering", "Message › Basic Text", numbering).pure().params(RANGE).when(needs_html),
        CommandSpec::new("format.clear", "Clear All Formatting", "Message › Basic Text", clear).pure().params(RANGE).when(needs_html),
        CommandSpec::new("compose.link", "Link", "Message › Include", link)
            .pure()
            .params(r#"{"url"?: string (any URI scheme but javascript, vbscript, data, file and cid, e.g. https, mailto, tel, ms-settings; none: remove the link), "from"?: number, "to"?: number}"#)
            .when(needs_html),
        CommandSpec::new("format.indent", "Increase Indent", "Message › Basic Text", indent)
            .pure()
            .params(r#"{"from"?: number, "to"?: number} — the list items there a level deeper (eight levels)"#)
            .when(needs_html),
        CommandSpec::new("format.outdent", "Decrease Indent", "Message › Basic Text", outdent)
            .pure()
            .params(r#"{"from"?: number, "to"?: number} — the list items there a level out; from the outermost level, out of the list"#)
            .when(needs_html),
        CommandSpec::new("edit.copy", "Copy", "Message › Clipboard", copy)
            .pure()
            .params(r#"{"from"?: number, "to"?: number} — answers the selection as {"html", "text"} for the clipboard (HTML only in an HTML message); changes nothing"#)
            .when(needs_doc),
        CommandSpec::new("edit.cut", "Cut", "Message › Clipboard", cut)
            .pure()
            .params(r#"{"from"?: number, "to"?: number} — answers the selection as edit.copy does, and takes it out of the text"#)
            .when(needs_doc),
        CommandSpec::new("edit.paste", "Paste", "Message › Clipboard", paste)
            .pure()
            .params(r#"{"html"?: string, "text"?: string} — over the selection; HTML is reduced to bold, italic, underline, a size, links and lists (to plain text in a plain-text message); HTML that comes to no text gives way to `text`, and nothing to paste changes nothing"#)
            .when(needs_doc),
        CommandSpec::new("compose.formatHtml", "HTML", "Format Text › Format", format_html).pure().when(needs_compose),
        CommandSpec::new("compose.formatPlain", "Plain Text", "Format Text › Format", format_plain).pure().when(needs_compose),
    ]
}

/// Enabled while a message is being written or a signature is open.
pub(super) fn needs_doc(s: &Session) -> Option<&'static str> {
    if s.doc().is_none() { Some("no message is being written") } else { None }
}

/// Enabled only while a message in HTML is being written (or a signature is open).
fn needs_html(s: &Session) -> Option<&'static str> {
    match s.doc() {
        None => Some("no message is being written"),
        Some((_, true)) => Some("the message is plain text; switch it to HTML first"),
        Some(_) => None,
    }
}

fn compose_mut(s: &mut Session) -> Result<&mut Compose, CmdError> {
    s.compose.as_mut().ok_or_else(|| fail("no message is being written"))
}

fn doc_mut(s: &mut Session) -> Result<Doc<'_>, CmdError> {
    s.doc_mut().ok_or_else(|| fail("no message is being written"))
}

fn model(e: ModelError) -> CmdError {
    fail(e.to_string())
}

/// A position from the params.
pub(super) fn position(v: &Value, key: &str) -> Result<Option<usize>, CmdError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => p::u64(v, key).and_then(|n| usize::try_from(n).ok()).map(Some).ok_or_else(|| bad(format!("`{key}` is a position (a number)"))),
    }
}

/// Select what `from` and `to` name, when the params name something.
fn select_range(body: &mut RichBody, v: &Value) -> Result<(), CmdError> {
    let (from, to) = (position(v, "from")?, position(v, "to")?);
    if from.is_none() && to.is_none() {
        return Ok(());
    }
    let sel = body.selection();
    let from = from.unwrap_or(sel.start());
    let to = to.unwrap_or(from);
    body.select(from, to).map_err(model)
}

/// Run an edit of the text; the message (or signature) is changed when its text is.
pub(super) fn edit<T>(s: &mut Session, v: &Value, f: impl FnOnce(&mut RichBody) -> Result<T, ModelError>) -> Result<T, CmdError> {
    let d = doc_mut(s)?;
    select_range(d.body, v)?;
    let before = d.body.text().clone();
    let out = f(d.body).map_err(model)?;
    if *d.body.text() != before {
        *d.dirty = true;
    }
    Ok(out)
}

/// What the answer of an edit says: the selection now.
pub(super) fn state(s: &Session) -> Value {
    match s.doc() {
        Some((body, _)) => {
            let sel = body.selection();
            json!({"from": sel.anchor, "to": sel.head, "length": body.text().len()})
        }
        None => json!({}),
    }
}

fn select(s: &mut Session, v: &Value) -> CmdResult {
    let from = position(v, "from")?.ok_or_else(|| bad("`from` (number) is required"))?;
    let to = position(v, "to")?.unwrap_or(from);
    doc_mut(s)?.body.select(from, to).map_err(model)?;
    Ok(state(s))
}

fn type_text(s: &mut Session, v: &Value) -> CmdResult {
    let text = p::req_str(v, "text")?.to_string();
    edit(s, &json!({}), |b| b.type_text(&text))?;
    Ok(state(s))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let forward = p::bool(v, "forward").unwrap_or(false);
    let named = position(v, "from")?.is_some() || position(v, "to")?.is_some();
    edit(s, v, |b| {
        let sel = b.selection();
        if named || !sel.is_empty() {
            b.delete(sel.start(), sel.end())
        } else if forward {
            b.delete_forward()
        } else {
            b.backspace()
        }
    })?;
    Ok(state(s))
}

fn undo(s: &mut Session, _: &Value) -> CmdResult {
    let d = doc_mut(s)?;
    if !d.body.undo() {
        return Err(fail("nothing to undo in the message"));
    }
    *d.dirty = true;
    Ok(state(s))
}

fn redo(s: &mut Session, _: &Value) -> CmdResult {
    let d = doc_mut(s)?;
    if !d.body.redo() {
        return Err(fail("nothing to redo in the message"));
    }
    *d.dirty = true;
    Ok(state(s))
}

fn bold(s: &mut Session, v: &Value) -> CmdResult {
    let on = edit(s, v, |b| b.toggle_bold())?;
    Ok(json!({"bold": on}))
}

fn italic(s: &mut Session, v: &Value) -> CmdResult {
    let on = edit(s, v, |b| b.toggle_italic())?;
    Ok(json!({"italic": on}))
}

fn underline(s: &mut Session, v: &Value) -> CmdResult {
    let on = edit(s, v, |b| b.toggle_underline())?;
    Ok(json!({"underline": on}))
}

fn color(s: &mut Session, v: &Value) -> CmdResult {
    let value = match v.get("value") {
        Some(Value::Null) => None,
        Some(_) => Some(p::str(v, "value").and_then(safe_color).ok_or_else(|| bad("`value` is a colour written #rrggbb, or null"))?),
        None => return Err(bad("`value` (#rrggbb, or null) is required")),
    };
    edit(s, v, |b| b.format(Format::Color(value.clone())))?;
    Ok(json!({"value": value}))
}

fn font(s: &mut Session, v: &Value) -> CmdResult {
    let value = match v.get("value") {
        Some(Value::Null) => None,
        Some(_) => {
            Some(p::str(v, "value").and_then(Font::parse).ok_or_else(|| bad("`value` is Arial, Calibri, Georgia, Verdana or System, or null"))?)
        }
        None => return Err(bad("`value` (a font, or null) is required")),
    };
    edit(s, v, |b| b.format(Format::Font(value)))?;
    Ok(json!({"value": value.map(Font::label)}))
}

fn size(s: &mut Session, v: &Value) -> CmdResult {
    let size = match v.get("size") {
        Some(Value::Null) => None,
        Some(_) => {
            let n = p::u64(v, "size").filter(|n| (u64::from(MIN_SIZE)..=u64::from(MAX_SIZE)).contains(n));
            Some(
                n.and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| bad(format!("`size` is a number of points, {MIN_SIZE} to {MAX_SIZE}, or null")))?,
            )
        }
        None => return Err(bad("`size` (points, or null) is required")),
    };
    edit(s, v, |b| b.format(Format::Size(size)))?;
    Ok(json!({"size": size}))
}

fn list(s: &mut Session, v: &Value, kind: ListKind) -> CmdResult {
    let now = edit(s, v, |b| b.toggle_list(kind))?;
    Ok(json!({"list": now}))
}

fn bullets(s: &mut Session, v: &Value) -> CmdResult {
    list(s, v, ListKind::Bullet)
}

fn numbering(s: &mut Session, v: &Value) -> CmdResult {
    list(s, v, ListKind::Numbered)
}

fn clear(s: &mut Session, v: &Value) -> CmdResult {
    edit(s, v, |b| b.clear_formatting())?;
    Ok(state(s))
}

fn link(s: &mut Session, v: &Value) -> CmdResult {
    let url = match v.get("url") {
        None | Some(Value::Null) => None,
        Some(Value::String(u)) => Some(u.clone()),
        Some(_) => return Err(bad("`url` is a string")),
    };
    edit(s, v, |b| b.set_link(url.as_deref()))?;
    Ok(state(s))
}

fn indent(s: &mut Session, v: &Value) -> CmdResult {
    edit(s, v, |b| b.indent(1))?;
    Ok(json!({"level": s.doc().map_or(0, |(b, _)| b.text().level_at(b.selection().start()))}))
}

fn outdent(s: &mut Session, v: &Value) -> CmdResult {
    edit(s, v, |b| b.indent(-1))?;
    let d = s.doc();
    Ok(json!({
        "level": d.map_or(0, |(b, _)| b.text().level_at(b.selection().start())),
        "list": d.map(|(b, _)| b.text().list_at(b.selection().start())),
    }))
}

/// The selection (after the params' range, when they name one) as the clipboard takes it: HTML
/// of the set, written as the message's own HTML part is, and its plain text.
fn clipboard(s: &mut Session, v: &Value) -> Result<Value, CmdError> {
    let d = doc_mut(s)?;
    select_range(d.body, v)?;
    let sel = d.body.selection();
    if sel.is_empty() {
        return Err(fail("nothing is selected in the message"));
    }
    let part = d.body.text().slice(sel.start(), sel.end()).map_err(model)?;
    let html = (!d.plain).then(|| mailcraft_mime::rich::to_html(&part));
    Ok(json!({"text": part.to_plain(), "html": html}))
}

fn copy(s: &mut Session, v: &Value) -> CmdResult {
    clipboard(s, v)
}

fn cut(s: &mut Session, v: &Value) -> CmdResult {
    let out = clipboard(s, v)?;
    edit(s, &json!({}), |b| {
        let sel = b.selection();
        b.delete(sel.start(), sel.end())
    })?;
    Ok(out)
}

/// Most HTML or text one paste takes, in bytes.
pub(crate) const MAX_PASTE: usize = 4 << 20;

fn paste(s: &mut Session, v: &Value) -> CmdResult {
    let plain = s.doc().is_some_and(|(_, plain)| plain);
    let (html, text) = (p::str(v, "html"), p::str(v, "text"));
    if html.is_none() && text.is_none() {
        return Err(bad("`html` or `text` (string) is required"));
    }
    let too_much = || fail("that is too much to paste into one message");
    // The HTML when there is some, reduced to the set; when it comes to nothing at all (a picture
    // alone, which a text can't hold), the text beside it, weighed only then; and nothing at all
    // changes nothing, the selection included. Lines left blank on purpose are something. (What
    // is looked at is what would be pasted, once cleaned of what a text never keeps.)
    let from_html = match html {
        Some(html) if html.len() > MAX_PASTE => return Err(too_much()),
        Some(html) => {
            let rich = mailcraft_mime::rich::from_html(html);
            Some(if plain { RichText::from_plain(&rich.text()) } else { rich }).filter(|rich| !rich.is_empty())
        }
        None => None,
    };
    let fragment = match (from_html, text) {
        (Some(rich), _) => rich,
        (None, Some(text)) if text.len() > MAX_PASTE => return Err(too_much()),
        (None, Some(text)) => RichText::from_plain(text),
        (None, None) => return Ok(state(s)),
    };
    if fragment.is_empty() {
        return Ok(state(s));
    }
    edit(s, &json!({}), |b| b.paste(&fragment))?;
    let mut out = state(s);
    // A signature's pictures come from files only (nothing is fetched); say so when the paste
    // had some.
    let pictures = p::str(v, "html").is_some_and(|h| h.to_ascii_lowercase().contains("<img"));
    if pictures
        && s.signature_edit.is_some()
        && let Some(o) = out.as_object_mut()
    {
        o.insert("note".into(), json!("pictures in what was pasted were left out: save each one and add it with Insert Picture"));
    }
    Ok(out)
}

fn set_format(s: &mut Session, format: BodyFormat) -> CmdResult {
    let c = compose_mut(s)?;
    if c.format != format {
        c.format = format;
        c.dirty = true;
    }
    Ok(json!({"format": c.format}))
}

fn format_html(s: &mut Session, _: &Value) -> CmdResult {
    set_format(s, BodyFormat::Html)
}

fn format_plain(s: &mut Session, _: &Value) -> CmdResult {
    set_format(s, BodyFormat::Plain)
}
