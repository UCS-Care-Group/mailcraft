// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing in rich text: the selection in the message being written, typing, formatting and
//! undo inside the message (`mailcraft_model::rich`).
//!
//! These are pure commands: they change the message being written, never the mailbox, so they
//! are not on the mailbox's undo history. The message has its own (`compose.undo`). While a
//! signature is open in File › Signatures they work on it instead ([`Session::doc`]).
//! Positions count characters of the body's text, a paragraph break being one.

use mailcraft_mime::rich::{Img, ImgRef};
use mailcraft_model::rich::{Align, Font, Format, ListKind, MAX_SIZE, MIN_SIZE, safe_color};
use mailcraft_model::signature::Picture;
use mailcraft_model::{ModelError, RichBody, RichText};
use serde_json::{Value, json};

use super::{bad, fail, needs_compose};
use crate::Doc;
use crate::session::{enum_code, parse_enum};
use crate::{BodyFormat, CmdError, CmdResult, CommandSpec, Compose, Session, p};

const RANGE: &str = r#"{"from"?: number, "to"?: number} (default: the selection)"#;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("compose.select", "Select Text", "Message", select).pure().params(r#"{"from": number, "to"?: number} (to: the caret, default from)"#).when_writing(needs_doc),
        CommandSpec::new("compose.type", "Type", "Message", type_text)
            .pure()
            .params(r#"{"text": string} — over the selection, in the formatting at the caret; a line break starts a paragraph (of the same kind; a line break alone in an empty list item ends the list there)"#)
            .when_writing(needs_doc),
        CommandSpec::new("compose.delete", "Delete Text", "Message", delete)
            .pure()
            .params(r#"{"from"?: number, "to"?: number, "forward"?: bool} (default: the selection, else the character before the caret, or after it with forward)"#)
            .when_writing(needs_doc),
        CommandSpec::new("compose.undo", "Undo Typing", "Message", undo).pure().when_writing(|s| match s.doc() {
            None => Some("no message is being written"),
            Some((b, _)) if !b.can_undo() => Some("nothing to undo in the message"),
            Some(_) => None,
        }),
        CommandSpec::new("compose.redo", "Redo Typing", "Message", redo).pure().when_writing(|s| match s.doc() {
            None => Some("no message is being written"),
            Some((b, _)) if !b.can_redo() => Some("nothing to redo in the message"),
            Some(_) => None,
        }),
        CommandSpec::new("format.bold", "Bold", "Message › Basic Text", bold).pure().params(RANGE).when_writing(needs_html),
        CommandSpec::new("format.italic", "Italic", "Message › Basic Text", italic).pure().params(RANGE).when_writing(needs_html),
        CommandSpec::new("format.underline", "Underline", "Message › Basic Text", underline).pure().params(RANGE).when_writing(needs_html),
        CommandSpec::new("format.size", "Font Size", "Message › Basic Text", size)
            .pure()
            .params(r#"{"size": number (points, 8–36) | null (the message's own size), "from"?: number, "to"?: number}"#)
            .when_writing(needs_html),
        CommandSpec::new("format.color", "Font Color", "Message › Basic Text", color)
            .pure()
            .params(r##"{"value": "#rrggbb" | null (the message's own colour), "from"?: number, "to"?: number}"##)
            .when_writing(needs_html),
        CommandSpec::new("format.highlight", "Text Highlight Color", "Message › Basic Text", highlight)
            .pure()
            .params(r##"{"value": "#rrggbb" | null (no highlight), "from"?: number, "to"?: number}"##)
            .when_writing(needs_html),
        CommandSpec::new("insert.symbol", "Symbol", "Insert › Symbols", symbol)
            .pure()
            .params(r#"{"char": string (one character)} | {"code": number | "U+20AC"} — put in at the caret, over the selection; a control or format character, a private-use or non-character code point, or white space other than the no-break space is refused"#)
            .when_writing(needs_doc),
        CommandSpec::new("format.font", "Font", "Message › Basic Text", font)
            .pure()
            .params(r#"{"value": "Arial"|"Calibri"|"Georgia"|"Verdana"|"System" | null (the message's own font), "from"?: number, "to"?: number}"#)
            .when_writing(needs_html),
        CommandSpec::new("format.bullets", "Bullets", "Message › Basic Text", bullets).pure().params(RANGE).when_writing(needs_html),
        CommandSpec::new("format.numbering", "Numbering", "Message › Basic Text", numbering).pure().params(RANGE).when_writing(needs_html),
        CommandSpec::new("format.align", "Align Text", "Message › Basic Text", align)
            .pure()
            .params(r#"{"value": "left"|"center"|"right", "from"?: number, "to"?: number} — every ordinary paragraph the range (default: the selection) touches; a list item stays left. Kept when sent (text-align) and on a paste, but not for a blank line; a plain-text message has none. Answers {"value"}: the alignment where the range starts. The Commands list doesn't offer it, having no way to say which"#)
            .when_writing(needs_html),
        CommandSpec::new("format.clear", "Clear All Formatting", "Message › Basic Text", clear).pure().params(RANGE).when_writing(needs_html),
        CommandSpec::new("compose.link", "Link", "Message › Include", link)
            .pure()
            .params(r#"{"url"?: string (any URI scheme but javascript, vbscript, data, file and cid, e.g. https, mailto, tel, ms-settings; none: remove the link), "from"?: number, "to"?: number}"#)
            .when_writing(needs_html),
        CommandSpec::new("format.indent", "Increase Indent", "Message › Basic Text", indent)
            .pure()
            .params(r#"{"from"?: number, "to"?: number} — the list items there a level deeper (eight levels)"#)
            .when_writing(needs_html),
        CommandSpec::new("format.outdent", "Decrease Indent", "Message › Basic Text", outdent)
            .pure()
            .params(r#"{"from"?: number, "to"?: number} — the list items there a level out; from the outermost level, out of the list"#)
            .when_writing(needs_html),
        CommandSpec::new("edit.copy", "Copy", "Message › Clipboard", copy)
            .pure()
            .params(r#"{"from"?: number, "to"?: number} — answers the selection as {"html", "text"} for the clipboard (HTML only in an HTML message); a picture of the message's own, or of the open signature, is an <img> naming it by a cid: (never its bytes), and its description in brackets in the text; changes nothing"#)
            .when_writing(needs_doc),
        CommandSpec::new("edit.cut", "Cut", "Message › Clipboard", cut)
            .pure()
            .params(r#"{"from"?: number, "to"?: number} — answers the selection as edit.copy does, and takes it out of the text (a picture taken out comes back with edit.paste, in the same message or signature)"#)
            .when_writing(needs_doc),
        CommandSpec::new("edit.paste", "Paste", "Message › Clipboard", paste)
            .pure()
            .params(r#"{"html"?: string, "text"?: string} — over the selection; HTML is reduced to bold, italic, underline, a size, a colour, a highlight, the composer's fonts, links, lists and a paragraph's alignment (in a plain-text message, to the text a copy of it gives: lists with their markers, links with their address, each picture named by cid: as its description); HTML that comes to no text gives way to `text`, and nothing to paste changes nothing. A cid: picture comes only when it is one edit.copy named of the message's own pictures (in a signature, the open signature's) that it holds now, the same bytes, as wide and described as it has it; a paste that would show one in the message's signature (which goes out with its own pictures only), or have the text show pictures more than 30 times (a signature's, 10), each time counted, is refused; any other picture is left out, which a signature's answer says in `note` when the reading pane shows it"#)
            .when_writing(needs_doc),
        CommandSpec::new("compose.formatHtml", "HTML", "Format Text › Format", format_html).pure().when_writing(needs_compose),
        CommandSpec::new("compose.formatPlain", "Plain Text", "Format Text › Format", format_plain).pure().when_writing(needs_compose),
    ]
}

/// Enabled while a message is being written or a signature is open.
pub(super) fn needs_doc(s: &Session) -> Option<&'static str> {
    if s.doc().is_none() { Some("no message is being written") } else { None }
}

/// Enabled only while a message in HTML is being written (or a signature is open).
pub(super) fn needs_html(s: &Session) -> Option<&'static str> {
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

/// Where the text the owner edits ends in the message being written (or the signature open): its
/// end. Find, Replace and the formats keep within it.
pub(super) fn reach(s: &Session) -> usize {
    s.doc().map_or(0, |(b, _)| b.text().len())
}

/// Select what `from` and `to` name, when the params name something, within `reach`.
fn select_range(body: &mut RichBody, v: &Value, reach: usize) -> Result<(), CmdError> {
    let (from, to) = (position(v, "from")?, position(v, "to")?);
    if from.is_none() && to.is_none() {
        return Ok(());
    }
    let sel = body.selection();
    let from = from.unwrap_or(sel.start());
    let to = to.unwrap_or(from);
    let len = body.text().len();
    // Past the end is an error as ever; past the owner's text, only as far as it goes.
    if from > len || to > len {
        return Err(model(ModelError::Invalid(format!("the text has {len} characters"))));
    }
    body.select(from.min(reach), to.min(reach)).map_err(model)
}

/// Run an edit of the text; the message (or signature) is changed when its text is.
pub(super) fn edit<T>(s: &mut Session, v: &Value, f: impl FnOnce(&mut RichBody) -> Result<T, ModelError>) -> Result<T, CmdError> {
    let reach = reach(s);
    let d = doc_mut(s)?;
    select_range(d.body, v, reach)?;
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

fn highlight(s: &mut Session, v: &Value) -> CmdResult {
    let value = match v.get("value") {
        Some(Value::Null) => None,
        Some(_) => Some(p::str(v, "value").and_then(safe_color).ok_or_else(|| bad("`value` is a colour written #rrggbb, or null"))?),
        None => return Err(bad("`value` (#rrggbb, or null) is required")),
    };
    edit(s, v, |b| b.format(Format::Highlight(value.clone())))?;
    Ok(json!({"value": value}))
}

/// Symbols Insert › Symbol offers: common marks the fonts the window draws in all have.
pub const SYMBOLS: [char; 36] = [
    '©', '®', '™', '€', '£', '¥', '¢', '§', '¶', '°', '±', '×', '÷', '≠', '≤', '≥', '≈', '∞', 'µ', '½', '¼', '¾', '…', '–', '—', '•', '·', '←', '→',
    '↑', '↓', '«', '»', '¿', '¡', '\u{a0}',
];

/// Whether `c` may be put in as a symbol: something that shows (or the no-break space), not a
/// control or format character, a private-use code point, a non-character or the character a
/// picture stands as.
pub fn symbol_ok(c: char) -> bool {
    let n = u32::from(c);
    let private = (0xe000..=0xf8ff).contains(&n) || n >= 0xf_0000;
    let nonchar = (0xfdd0..=0xfdef).contains(&n) || n & 0xfffe == 0xfffe;
    !(c.is_control()
        || mailcraft_names::is_format(c)
        || private
        || nonchar
        || c == mailcraft_model::rich::OBJECT
        || (c.is_whitespace() && c != '\u{a0}'))
}

/// The symbol the params name.
fn symbol_of(v: &Value) -> Result<char, CmdError> {
    let bad_code = || bad("`code` is a Unicode code point, a number or written U+20AC");
    let c = match (v.get("char"), v.get("code")) {
        (Some(Value::String(t)), None) => {
            let mut chars = t.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c,
                _ => return Err(bad("`char` is one character")),
            }
        }
        (None, Some(Value::Number(_))) => p::u64(v, "code").and_then(|n| u32::try_from(n).ok()).and_then(char::from_u32).ok_or_else(bad_code)?,
        (None, Some(Value::String(t))) => {
            let hex = t.trim();
            let hex = hex.strip_prefix("U+").or_else(|| hex.strip_prefix("u+")).unwrap_or(hex);
            if hex.is_empty() || hex.len() > 6 {
                return Err(bad_code());
            }
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32).ok_or_else(bad_code)?
        }
        _ => return Err(bad("`char` (one character) or `code` (a code point) is required")),
    };
    if !symbol_ok(c) {
        return Err(bad("that character can't be put in: it is a control or format character, white space, or shows nothing"));
    }
    Ok(c)
}

fn symbol(s: &mut Session, v: &Value) -> CmdResult {
    let c = symbol_of(v)?;
    edit(s, &json!({}), |b| b.type_text(&c.to_string()))?;
    let mut out = state(s);
    if let Some(o) = out.as_object_mut() {
        o.insert("char".into(), json!(c.to_string()));
        o.insert("code".into(), json!(format!("U+{:04X}", u32::from(c))));
    }
    Ok(out)
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

fn align(s: &mut Session, v: &Value) -> CmdResult {
    let align = p::str(v, "value").and_then(parse_enum::<Align>).ok_or_else(|| bad("`value` is left, center or right"))?;
    edit(s, v, |b| b.set_align(align))?;
    Ok(json!({"value": s.doc().map(|(b, _)| enum_code(&b.text().align_at(b.selection().start())))}))
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
/// of the set, written as the message's own HTML part is, and its plain text. A picture of the
/// text's holder ([`held`]) is an `<img>` naming it ([`picture_cid`]), never its bytes, and its
/// description in brackets in the text, so that a picture alone is never an empty text.
fn clipboard(s: &mut Session, v: &Value) -> Result<Value, CmdError> {
    let reach = reach(s);
    select_range(doc_mut(s)?.body, v, reach)?;
    let (body, plain) = s.doc().ok_or_else(|| fail("no message is being written"))?;
    let sel = body.selection();
    if sel.is_empty() {
        return Err(fail("nothing is selected in the message"));
    }
    let part = own(s, &body.text().slice(sel.start(), sel.end()).map_err(model)?);
    let (prefix, pictures, _) = held(s);
    let img = |id: u32| pictures.iter().find(|p| p.id == id).map(|p| Img::of(p, picture_cid(prefix, p)));
    let html = (!plain).then(|| mailcraft_mime::rich::to_html_with(&part, &img));
    Ok(json!({"text": plain_text(s, &part), "html": html}))
}

/// `part` of the text being edited as plain text, as a copy of it has it: a picture of the
/// text's holder ([`held`]) its description in brackets. Also what goes to the primary selection.
pub fn plain_text(s: &Session, part: &RichText) -> String {
    let (_, pictures, _) = held(s);
    own(s, part).to_plain_with(|id| pictures.iter().find(|p| p.id == id).map(|p| super::compose::said(&p.alt)))
}

/// `part` of the text being edited with only its holder's pictures ([`held`]) named by their
/// ids: in a message, a picture of the signature put in it (whose id may be one of the message's
/// own) names none, so that it is copied, described and counted as nothing, as ever.
fn own(s: &Session, part: &RichText) -> RichText {
    if s.signature_edit.is_some() || !part.paragraphs().iter().any(|p| p.signature) {
        return part.clone();
    }
    // (No picture of a message has an id below the first.)
    let none = mailcraft_model::pictures::FIRST_ID.saturating_sub(1);
    let paragraphs = part.paragraphs().iter().cloned().map(|mut p| {
        if p.signature {
            p.runs.iter_mut().filter(|r| r.picture.is_some()).for_each(|r| r.picture = Some(none));
        }
        p
    });
    RichText::from_paragraphs(paragraphs.collect())
}

/// Whose pictures the text being edited shows, as a copy names them: the open signature's
/// (`sig-`), else the message's own (`pic-`; never its signature's, which go out only in their
/// signature); and the most times its text may show them.
fn held(s: &Session) -> (&'static str, &[Picture], usize) {
    match (&s.signature_edit, &s.compose) {
        (Some(e), _) => ("sig-", e.signature.pictures(), mailcraft_model::signature::MAX_PICTURES),
        (None, Some(c)) => ("pic-", &c.pictures, mailcraft_model::pictures::MAX_PICTURES),
        (None, None) => ("pic-", &[], 0),
    }
}

/// How many more times a paste or a new picture may show the pictures of the text's holder
/// ([`held`]): as many as it may hold, each time it shows one counted, less those it shows outside
/// the selection it takes the place of; `None` in the signature put in a message, which goes out
/// with its own pictures only ([`not_in_signature`]).
pub(super) fn room(s: &Session) -> Option<usize> {
    let (prefix, pictures, most) = held(s);
    let (body, _) = s.doc()?;
    let (text, sel) = (body.text(), body.selection());
    let in_signature = text.signature_start().zip(text.signature()).is_some_and(|(at, sig)| (at..=at + sig.len()).contains(&sel.start()));
    if prefix == "pic-" && in_signature {
        return None;
    }
    let shown = |t: &RichText| own(s, t).pictures().iter().filter(|id| pictures.iter().any(|p| p.id == **id)).count();
    let selected = text.slice(sel.start(), sel.end()).map_or(0, |t| shown(&t));
    Some(most.saturating_sub(shown(text).saturating_sub(selected)))
}

/// Why a picture of the message does not go in the signature put in it.
pub(super) fn not_in_signature() -> CmdError {
    fail("the signature in the message goes out with its own pictures only: put the picture outside it")
}

/// Why no more pictures go in the text of its holder ([`held`]).
pub(super) fn too_many(s: &Session) -> CmdError {
    match s.signature_edit {
        Some(_) => failed!("a signature holds at most {n} pictures", n = mailcraft_model::signature::MAX_PICTURES),
        None => failed!("a message holds at most {n} pictures", n = mailcraft_model::pictures::MAX_PICTURES),
    }
}

/// The Content-ID a copy names a picture by: its holder, its id and its bytes' [`fingerprint`].
fn picture_cid(prefix: &str, p: &Picture) -> String {
    format!("{prefix}{}.{}@mailcraft.invalid", p.id, fingerprint(&p.data))
}

/// What a copy names a picture by beside its id: 16 hex digits of the FNV-1a hash of its bytes, so
/// that a picture of the same id in another message or signature is never taken for it.
pub(super) fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash = (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
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
    // not taken, alone), the text beside it, weighed only then; and nothing at all changes nothing,
    // the selection included. Lines left blank on purpose are something. (What is looked at is
    // what would be pasted, once cleaned of what a text never keeps.) A picture the HTML names is
    // shown only when it is one the text's holder holds now, by the name a copy gives it (bytes and
    // all), and while there is room ([`room`]): a paste that would show more than that, or show one
    // in the message's signature, is refused, so that nothing it names is lost unsaid. It is then
    // as the holder has it, whatever width or description the HTML gives it. A plain-text message
    // takes the text a copy of it gives: lists with their markers, links with their address, and
    // each attached picture the reading pane shows as its description, the holder's as it has it.
    let mut left_out = 0usize;
    let from_html = match html {
        Some(html) if html.len() > MAX_PASTE => return Err(too_much()),
        Some(html) => {
            let (prefix, pictures, _) = held(s);
            // Named once, and only when the HTML names a picture: a hash goes over every byte.
            let mut names: Option<Vec<(String, &Picture)>> = None;
            // In a plain-text message, what each picture says, by the number it is given.
            let mut described: Vec<String> = Vec::new();
            let mut claim = |img: &ImgRef| {
                let names = names.get_or_insert_with(|| pictures.iter().map(|p| (picture_cid(prefix, p), p)).collect());
                let picture = names.iter().find(|(cid, _)| *cid == img.cid).map(|(_, p)| *p);
                if plain {
                    let n = u32::try_from(described.len()).ok();
                    described.push(super::compose::said(picture.map_or(&img.alt, |p| &p.alt)));
                    return n;
                }
                left_out += usize::from(picture.is_none());
                picture.map(|p| p.id)
            };
            let (rich, others) = mailcraft_mime::rich::from_html_with_pictures_and_others(html, &mut claim);
            left_out += others;
            let rich = if plain {
                RichText::from_plain(&rich.to_plain_with(|n| described.get(usize::try_from(n).ok()?).cloned()))
            } else {
                // Counted as they are pasted: one past the most paragraphs a text holds is not.
                let shown = rich.pictures().len();
                match room(s) {
                    None if shown > 0 => return Err(not_in_signature()),
                    Some(room) if shown > room => return Err(too_many(s)),
                    _ => rich,
                }
            };
            Some(rich).filter(|rich| !rich.is_empty())
        }
        None => None,
    };
    let fragment = match (from_html, text) {
        (Some(rich), _) => Some(rich),
        (None, Some(text)) if text.len() > MAX_PASTE => return Err(too_much()),
        (None, Some(text)) => Some(RichText::from_plain(text)),
        (None, None) => None,
    };
    if let Some(fragment) = fragment.filter(|f| !f.is_empty()) {
        edit(s, &json!({}), |b| b.paste(&fragment))?;
    }
    let mut out = state(s);
    // A signature's pictures come from files only (nothing is fetched), or from its own copied:
    // say so when the paste left out others the reading pane shows, even with nothing else pasted.
    if left_out > 0
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

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod highlight_tests;
