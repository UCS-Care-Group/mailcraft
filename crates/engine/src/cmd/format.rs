//! Formatting the message being written (Message › Basic Text, Format Text, Insert, Review).
//!
//! The body of a message in HTML form is its text plus `Compose::styles`. Every command here
//! acts on a range of the body, in characters: `start` and `end` when the caller gives them,
//! else what the editor last said was selected (`compose.select`), else the whole body. With a
//! caret and nothing selected, a character format is kept for what is typed next.

use std::ops::Range;

use mailcraft_model::rich::{self, Align, CharStyle, List, PICTURE, ParaStyle, char_len};
use mailcraft_model::{Attachment, Rgb};
use serde_json::{Value, json};

use super::{bad, done, fail, needs_compose, source};
use crate::{CmdError, CmdResult, CommandSpec, Compose, Session, p};

/// Largest picture that can be put in a message.
pub const MAX_PICTURE: usize = 10 << 20;

/// Most pictures one message can show.
const MAX_PICTURES: usize = 100;

/// Longest text one command can put in.
const MAX_INSERT: usize = 64 * 1024;

/// The colour Text Highlight Color uses when none is named.
pub const DEFAULT_HIGHLIGHT: Rgb = Rgb(0xFF, 0xF1, 0x76);

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("compose.select", "Select Text", "Message", select)
            .pure()
            .params(r#"{"start": number, "end"?: number} — what the editor has selected in the body (a caret when both are the same); formatting acts on it"#)
            .when(needs_compose),
        CommandSpec::new("compose.formatHtml", "HTML", "Format Text › Format", format_html).pure().when(needs_compose),
        CommandSpec::new("compose.formatPlain", "Plain Text", "Format Text › Format", format_plain)
            .pure()
            .params("{} — the formatting goes (list markers and link addresses are written out as text; pictures stay as attachments)")
            .when(needs_compose),
        CommandSpec::new("format.bold", "Bold", "Message › Basic Text", bold).pure().key("Mod+B").params(TOGGLE).when(needs_rich),
        CommandSpec::new("format.italic", "Italic", "Message › Basic Text", italic).pure().key("Mod+I").params(TOGGLE).when(needs_rich),
        CommandSpec::new("format.underline", "Underline", "Message › Basic Text", underline).pure().key("Mod+U").params(TOGGLE).when(needs_rich),
        CommandSpec::new("format.strike", "Strikethrough", "Message › Basic Text", strike).pure().params(TOGGLE).when(needs_rich),
        CommandSpec::new("format.font", "Font", "Message › Basic Text", font)
            .pure()
            .params(r#"{"face": "default"|"fixed", "start"?, "end"?} — the message's own font, or a fixed-width one"#)
            .when(needs_rich),
        CommandSpec::new("format.size", "Font Size", "Message › Basic Text", size)
            .pure()
            .params(r#"{"size": 10-32 (CSS pixels) | null (the reader's own) | "larger" | "smaller", "start"?, "end"?}"#)
            .when(needs_rich),
        CommandSpec::new("format.color", "Font Color", "Message › Basic Text", color)
            .pure()
            .params(r##"{"color": "#RRGGBB" | null (automatic), "start"?, "end"?}"##)
            .when(needs_rich),
        CommandSpec::new("format.highlight", "Text Highlight Color", "Message › Basic Text", highlight)
            .pure()
            .params(r##"{"color"?: "#RRGGBB" | null (none; default: yellow), "start"?, "end"?}"##)
            .when(needs_rich),
        CommandSpec::new("format.clear", "Clear All Formatting", "Message › Basic Text", clear)
            .pure()
            .key("Mod+Space")
            .params(r#"{"start"?, "end"?} — plain text and plain paragraphs again (links and pictures stay)"#)
            .when(needs_rich),
        CommandSpec::new("format.bullets", "Bullets", "Message › Basic Text", bullets).pure().params(PARA_TOGGLE).when(needs_rich),
        CommandSpec::new("format.numbering", "Numbering", "Message › Basic Text", numbering).pure().params(PARA_TOGGLE).when(needs_rich),
        CommandSpec::new("format.indent", "Increase Indent", "Format Text › Paragraph", indent)
            .pure()
            .params(r#"{"by"?: 1 | -1 (default 1), "start"?, "end"?} — the paragraphs move in (a list item is nested deeper)"#)
            .when(needs_rich),
        CommandSpec::new("format.outdent", "Decrease Indent", "Format Text › Paragraph", outdent).pure().params(r#"{"start"?, "end"?}"#).when(needs_rich),
        CommandSpec::new("format.align", "Align Text", "Message › Basic Text", align)
            .pure()
            .params(r#"{"align": "left"|"center"|"right", "start"?, "end"?}"#)
            .when(needs_rich),
        CommandSpec::new("compose.link", "Link", "Insert › Links", link)
            .pure()
            .params(r#"{"url": "https://…"|"mailto:…"|"tel:…" | null (take the link off), "text"?: string (put in at the caret, linked), "start"?, "end"?} — only http, https, mailto and tel addresses"#)
            .when(needs_rich),
        CommandSpec::new("insert.picture", "Pictures", "Insert › Illustrations", picture)
            .pure()
            .params(r#"{"path": string} | {"name": string, "data": base64}, "at"?: number — a PNG, JPEG, GIF or WebP picture (by its own bytes; at most 10 MB) shown in the message, on a line of its own"#)
            .when(needs_compose),
        CommandSpec::new("insert.rule", "Horizontal Line", "Insert › Symbols", rule).pure().params(r#"{"at"?: number} — a line across, under the paragraph"#).when(needs_rich),
        CommandSpec::new("insert.dateTime", "Date & Time", "Insert › Text", date_time)
            .pure()
            .params(r#"{"format"?: "long"|"short"|"time"|"dateTime" (default long), "at"?: number}"#)
            .when(needs_compose),
        CommandSpec::new("insert.symbol", "Symbol", "Insert › Symbols", symbol)
            .pure()
            .params(r#"{"symbol": string (one character, or a few), "at"?: number}"#)
            .when(needs_compose),
        CommandSpec::new("edit.find", "Find", "Format Text › Editing", find)
            .pure()
            .params(r#"{"text": string, "from"?: number, "matchCase"?: bool} — the next place in the body the text is (from the caret; selects it)"#)
            .when(needs_compose),
        CommandSpec::new("edit.replace", "Replace", "Format Text › Editing", replace)
            .pure()
            .params(r#"{"text": string, "with": string, "all"?: bool, "matchCase"?: bool} — in the body"#)
            .when(needs_compose),
        CommandSpec::new("review.wordCount", "Word Count", "Review › Proofing", word_count).pure().when(needs_compose),
        CommandSpec::new("review.spelling", "Spelling & Grammar", "Review › Proofing", spelling)
            .pure()
            .key("F7")
            .params(r#"{"text"?: string (check this instead of the message), "suggestions"?: bool (default true)} — words that are not in the English (US) word list, with their places in the text and words they may have been meant as; the quoted original of a reply is left out"#),
    ]
}

const TOGGLE: &str = r#"{"value"?: bool (default: toggle), "start"?, "end"?}"#;
const PARA_TOGGLE: &str = r#"{"value"?: bool (default: toggle), "start"?, "end"?} — the paragraphs the range touches"#;

/// Enabled only while a message is being written in HTML.
pub(crate) fn needs_rich(s: &Session) -> Option<&'static str> {
    match &s.compose {
        None => Some("no message is being written"),
        Some(c) if !c.html => Some("the message is plain text: switch it to HTML (Format Text › HTML)"),
        Some(_) => None,
    }
}

fn compose_mut(s: &mut Session) -> Result<&mut Compose, CmdError> {
    s.compose.as_mut().ok_or_else(|| fail("no message is being written"))
}

fn rich_mut(s: &mut Session) -> Result<&mut Compose, CmdError> {
    let c = compose_mut(s)?;
    if !c.html {
        return Err(fail("the message is plain text: switch it to HTML (Format Text › HTML)"));
    }
    c.styles.normalize(&c.body);
    Ok(c)
}

/// The characters a command acts on.
fn range(c: &Compose, v: &Value) -> Range<usize> {
    let len = char_len(&c.body);
    let clamp = |n: u64| usize::try_from(n).unwrap_or(usize::MAX).min(len);
    match (p::u64(v, "start"), p::u64(v, "end")) {
        (Some(a), Some(b)) => {
            let (a, b) = (clamp(a), clamp(b));
            a.min(b)..a.max(b)
        }
        (Some(a), None) => clamp(a)..clamp(a),
        (None, Some(b)) => 0..clamp(b),
        (None, None) => match c.selection {
            Some((a, b)) => {
                let (a, b) = (a.min(len), b.min(len));
                a.min(b)..a.max(b)
            }
            None => 0..len,
        },
    }
}

/// Where something is put in: `at`, else the caret (the end of the selection), else the end.
fn caret(c: &Compose, v: &Value) -> usize {
    let len = char_len(&c.body);
    match p::u64(v, "at") {
        Some(at) => usize::try_from(at).unwrap_or(usize::MAX).min(len),
        None => c.selection.map_or(len, |(a, b)| a.max(b).min(len)),
    }
}

/// The look at a caret: what was chosen there, else what typing there would get.
fn typing_at(c: &Compose, at: usize) -> CharStyle {
    match (&c.typing, c.selection) {
        (Some(t), Some((a, b))) if a == b && a == at => t.clone(),
        _ => c.styles.typing_style(at),
    }
}

/// Change the look of `r`: of the text in it, or of what is typed next at a caret.
fn change_chars(c: &mut Compose, r: Range<usize>, change: impl Fn(&mut CharStyle)) {
    if r.is_empty() {
        let mut t = typing_at(c, r.start);
        change(&mut t);
        c.typing = Some(t);
        c.selection = Some((r.start, r.start));
        return;
    }
    let before = c.styles.clone();
    c.styles.apply(&c.body, r, change);
    if c.styles != before {
        c.dirty = true;
    }
}

/// Whether the text of `r` (or the caret's look) has what `test` asks for.
fn has(c: &Compose, r: &Range<usize>, test: impl Fn(&CharStyle) -> bool) -> bool {
    if r.is_empty() { test(&typing_at(c, r.start)) } else { c.styles.every(r.clone(), test) }
}

fn toggle_char(s: &mut Session, v: &Value, get: fn(&CharStyle) -> bool, set: fn(&mut CharStyle, bool)) -> CmdResult {
    let c = rich_mut(s)?;
    let r = range(c, v);
    let on = p::bool(v, "value").unwrap_or_else(|| !has(c, &r, get));
    change_chars(c, r, |style| set(style, on));
    Ok(json!({"value": on}))
}

fn bold(s: &mut Session, v: &Value) -> CmdResult {
    toggle_char(s, v, |c| c.bold, |c, on| c.bold = on)
}

fn italic(s: &mut Session, v: &Value) -> CmdResult {
    toggle_char(s, v, |c| c.italic, |c, on| c.italic = on)
}

fn underline(s: &mut Session, v: &Value) -> CmdResult {
    toggle_char(s, v, |c| c.underline, |c, on| c.underline = on)
}

fn strike(s: &mut Session, v: &Value) -> CmdResult {
    toggle_char(s, v, |c| c.strike, |c, on| c.strike = on)
}

fn font(s: &mut Session, v: &Value) -> CmdResult {
    let fixed = match p::req_str(v, "face")? {
        "fixed" | "mono" | "monospace" => true,
        "default" | "normal" => false,
        _ => return Err(bad("`face` is \"default\" or \"fixed\"")),
    };
    let c = rich_mut(s)?;
    let r = range(c, v);
    change_chars(c, r, |style| style.fixed = fixed);
    Ok(json!({"face": if fixed { "fixed" } else { "default" }}))
}

/// The next size up or down from `size` among [`rich::SIZES`] (the reader's own size sits
/// between 12 and 16).
pub fn step_size(size: Option<u8>, up: bool) -> Option<u8> {
    let here = size.map_or(14, u16::from);
    if up {
        let next = rich::SIZES.iter().copied().find(|s| u16::from(*s) > here);
        match size {
            Some(s) if s < 16 && next.is_some_and(|n| n >= 16) => None,
            _ => next.or(Some(rich::MAX_SIZE)),
        }
    } else {
        let next = rich::SIZES.iter().rev().copied().find(|s| u16::from(*s) < here);
        match size {
            Some(s) if s > 12 && next.is_some_and(|n| n <= 12) => None,
            _ => next.or(Some(rich::MIN_SIZE)),
        }
    }
}

fn size(s: &mut Session, v: &Value) -> CmdResult {
    enum Want {
        Set(Option<u8>),
        Step(bool),
    }
    let want = match v.get("size") {
        Some(Value::Null) => Want::Set(None),
        Some(Value::String(w)) if w == "larger" => Want::Step(true),
        Some(Value::String(w)) if w == "smaller" => Want::Step(false),
        Some(n) if n.as_u64().is_some() => Want::Set(n.as_u64().map(|n| n.clamp(u64::from(rich::MIN_SIZE), u64::from(rich::MAX_SIZE)) as u8)),
        _ => return Err(bad("`size` is 10-32, null, \"larger\" or \"smaller\"")),
    };
    let c = rich_mut(s)?;
    let r = range(c, v);
    let answer = match want {
        Want::Set(size) => {
            change_chars(c, r, |style| style.size = size);
            json!({"size": size})
        }
        Want::Step(up) => {
            change_chars(c, r, |style| style.size = step_size(style.size, up));
            json!({"stepped": if up { "larger" } else { "smaller" }})
        }
    };
    Ok(answer)
}

/// A colour param: `#RRGGBB`, `null` for none, or `fallback` when it is left out.
fn color_param(v: &Value, fallback: Option<Option<Rgb>>) -> Result<Option<Rgb>, CmdError> {
    match v.get("color") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Rgb::parse(text).map(Some).ok_or_else(|| bad("`color` is \"#RRGGBB\" or null")),
        None => fallback.ok_or_else(|| bad("`color` (\"#RRGGBB\" or null) is required")),
        Some(_) => Err(bad("`color` is \"#RRGGBB\" or null")),
    }
}

fn color(s: &mut Session, v: &Value) -> CmdResult {
    let color = color_param(v, None)?;
    let c = rich_mut(s)?;
    let r = range(c, v);
    change_chars(c, r, |style| style.color = color);
    Ok(json!({"color": color.map(Rgb::hex)}))
}

fn highlight(s: &mut Session, v: &Value) -> CmdResult {
    let color = color_param(v, Some(Some(DEFAULT_HIGHLIGHT)))?;
    let c = rich_mut(s)?;
    let r = range(c, v);
    change_chars(c, r, |style| style.highlight = color);
    Ok(json!({"color": color.map(Rgb::hex)}))
}

fn clear(s: &mut Session, v: &Value) -> CmdResult {
    let c = rich_mut(s)?;
    let r = range(c, v);
    change_chars(c, r.clone(), |style| *style = CharStyle { link: style.link.take(), picture: style.picture.take(), ..CharStyle::default() });
    let before = c.styles.clone();
    c.styles.apply_paras(&c.body, r, |para| *para = ParaStyle { rule: para.rule, ..ParaStyle::default() });
    if c.styles != before {
        c.dirty = true;
    }
    done()
}

fn change_paras(c: &mut Compose, r: Range<usize>, change: impl FnMut(&mut ParaStyle)) {
    let before = c.styles.clone();
    c.styles.apply_paras(&c.body, r, change);
    if c.styles != before {
        c.dirty = true;
    }
}

fn toggle_list(s: &mut Session, v: &Value, kind: List) -> CmdResult {
    let c = rich_mut(s)?;
    let r = range(c, v);
    let all = c.styles.paras_in(&c.body, r.clone()).iter().all(|p| p.list == kind);
    let on = p::bool(v, "value").unwrap_or(!all);
    change_paras(c, r, |para| {
        if on {
            para.list = kind;
        } else if para.list == kind {
            para.list = List::None;
        }
    });
    Ok(json!({"value": on}))
}

fn bullets(s: &mut Session, v: &Value) -> CmdResult {
    toggle_list(s, v, List::Bullet)
}

fn numbering(s: &mut Session, v: &Value) -> CmdResult {
    toggle_list(s, v, List::Number)
}

fn shift(s: &mut Session, v: &Value, by: i64) -> CmdResult {
    let c = rich_mut(s)?;
    let r = range(c, v);
    change_paras(c, r, |para| {
        para.indent = if by >= 0 { para.indent.saturating_add(1).min(rich::MAX_INDENT) } else { para.indent.saturating_sub(1) };
    });
    done()
}

fn indent(s: &mut Session, v: &Value) -> CmdResult {
    let by = match v.get("by") {
        None => 1,
        Some(n) => match n.as_i64() {
            Some(1) => 1,
            Some(-1) => -1,
            _ => return Err(bad("`by` is 1 or -1")),
        },
    };
    shift(s, v, by)
}

fn outdent(s: &mut Session, v: &Value) -> CmdResult {
    shift(s, v, -1)
}

fn align(s: &mut Session, v: &Value) -> CmdResult {
    let align = Align::parse(p::req_str(v, "align")?).ok_or_else(|| bad("`align` is left, center or right"))?;
    let c = rich_mut(s)?;
    let r = range(c, v);
    change_paras(c, r, |para| para.align = align);
    Ok(json!({"align": align}))
}

/// Put `text` in at `at`, looking like `style`; the caret goes after it.
fn insert(c: &mut Compose, at: usize, text: &str, style: &CharStyle) {
    let mut body = std::mem::take(&mut c.body);
    c.styles.splice(&mut body, at..at, text, style);
    c.body = body;
    let end = at + char_len(text);
    c.selection = Some((end, end));
    c.typing = None;
    c.dirty = true;
}

fn link(s: &mut Session, v: &Value) -> CmdResult {
    let url = match v.get("url") {
        Some(Value::Null) => None,
        Some(Value::String(u)) if u.trim().is_empty() => None,
        Some(Value::String(u)) => {
            let u = u.trim();
            // An address typed without its scheme.
            let u = if !u.contains(':') && u.contains('@') {
                format!("mailto:{u}")
            } else if !u.contains(':') && u.contains('.') {
                format!("https://{u}")
            } else {
                u.to_string()
            };
            if !rich::is_link(&u) {
                return Err(bad("a link is an http, https, mailto or tel address"));
            }
            Some(u)
        }
        _ => return Err(bad("`url` (an address, or null) is required")),
    };
    let text = p::str(v, "text").map(|t| t.replace(['\r', '\n'], " ")).filter(|t| !t.trim().is_empty());
    let c = rich_mut(s)?;
    let r = range(c, v);
    match (&url, r.is_empty()) {
        (Some(u), true) => {
            let shown = text.unwrap_or_else(|| u.trim_start_matches("mailto:").to_string());
            if shown.len() > MAX_INSERT {
                return Err(bad("`text` is too long"));
            }
            let style = CharStyle { link: Some(u.clone()), ..typing_at(c, r.start) };
            insert(c, r.start, &shown, &style);
        }
        (None, true) => {
            // The whole link the caret is in.
            let link = c.styles.style_of(r.start).and_then(|st| st.link.clone());
            let Some(link) = link else { return Err(fail("there is no link here")) };
            let (mut a, mut b) = (r.start, r.start);
            while a > 0 && c.styles.style_of(a - 1).is_some_and(|st| st.link.as_deref() == Some(link.as_str())) {
                a -= 1;
            }
            while c.styles.style_of(b).is_some_and(|st| st.link.as_deref() == Some(link.as_str())) {
                b += 1;
            }
            change_chars(c, a..b, |st| st.link = None);
        }
        (_, false) => {
            let url = url.clone();
            change_chars(c, r, |st| {
                if st.picture.is_none() {
                    st.link = url.clone();
                }
            });
        }
    }
    Ok(json!({"url": url}))
}

/// What kind of picture `bytes` are, by their own first bytes: the media type and a file
/// extension. Only PNG, JPEG, GIF and WebP.
pub fn picture_kind(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(("image/png", "png"))
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

fn picture(s: &mut Session, v: &Value) -> CmdResult {
    let (name, bytes) = source(v)?;
    if bytes.len() > MAX_PICTURE {
        return Err(fail(format!("the picture is larger than {} MB", MAX_PICTURE >> 20)));
    }
    let (mime, ext) = picture_kind(&bytes).ok_or_else(|| fail("that isn't a PNG, JPEG, GIF or WebP picture"))?;
    let name = name.rsplit(['/', '\\']).next().unwrap_or("").trim().chars().filter(|c| !c.is_control()).take(120).collect::<String>();
    let name = if name.is_empty() { format!("picture.{ext}") } else { name };
    let c = compose_mut(s)?;
    let shown = c.attachments.iter().filter(|a| a.inline && !a.content_id.is_empty()).count();
    if shown >= MAX_PICTURES || c.attachments.len() >= 200 {
        return Err(fail("this message already has as many pictures as it can carry"));
    }
    // A picture is shown in HTML only.
    let switched = !c.html;
    if switched {
        c.html = true;
        c.styles = rich::Styles::plain(&c.body);
    }
    c.styles.normalize(&c.body);
    // A content id made from the picture, so the same picture is the same part.
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3));
    let cid = format!("image{:03}.{hash:016x}@mailcraft", shown + 1);
    let mut attachment = Attachment::new(name.clone(), mime.to_string(), bytes);
    attachment.inline = true;
    attachment.content_id = cid.clone();
    c.attachments.push(attachment);
    // On a line of its own.
    let at = caret(c, v);
    let chars: Vec<char> = c.body.chars().collect();
    let line_start = at == 0 || chars.get(at - 1) == Some(&'\n');
    let line_end = at >= chars.len() || chars.get(at) == Some(&'\n');
    let mut text = String::new();
    if !line_start {
        text.push('\n');
    }
    text.push(PICTURE);
    if !line_end {
        text.push('\n');
    }
    let before = if line_start { 0 } else { 1 };
    let mut body = std::mem::take(&mut c.body);
    c.styles.splice(&mut body, at..at, &text, &CharStyle::default());
    c.body = body;
    let mark = at + before;
    c.styles.apply(&c.body, mark..mark + 1, |st| *st = CharStyle { picture: Some(cid.clone()), ..CharStyle::default() });
    let after = at + char_len(&text);
    c.selection = Some((after, after));
    c.typing = None;
    c.dirty = true;
    Ok(json!({"name": name, "contentId": cid, "html": true, "switched": switched}))
}

fn rule(s: &mut Session, v: &Value) -> CmdResult {
    let c = rich_mut(s)?;
    let at = caret(c, v);
    // At the end of the paragraph the caret is in: a new one that is the line.
    let chars: Vec<char> = c.body.chars().collect();
    let end = (at..chars.len()).find(|i| chars.get(*i) == Some(&'\n')).unwrap_or(chars.len());
    let mut body = std::mem::take(&mut c.body);
    c.styles.splice(&mut body, end..end, "\n", &CharStyle::default());
    c.body = body;
    let line = rich::line_of(&c.body, end + 1);
    if let Some(para) = c.styles.paras.get_mut(line) {
        *para = ParaStyle { rule: true, ..ParaStyle::default() };
    }
    c.selection = Some((end + 1, end + 1));
    c.typing = None;
    c.dirty = true;
    done()
}

fn put_text(s: &mut Session, v: &Value, text: &str) -> CmdResult {
    if text.len() > MAX_INSERT {
        return Err(bad("that is too much text to put in at once"));
    }
    let c = compose_mut(s)?;
    c.styles.normalize(&c.body);
    let at = caret(c, v);
    let style = if c.html { typing_at(c, at) } else { CharStyle::default() };
    insert(c, at, text, &style);
    Ok(json!({"inserted": text}))
}

fn date_time(s: &mut Session, v: &Value) -> CmdResult {
    let now = s.local(s.now());
    let text = match p::str(v, "format").unwrap_or("long") {
        "long" => now.date.long(),
        "short" => now.date.iso(),
        "time" => now.time_short(s.prefs.twelve_hour),
        "dateTime" => format!("{} {}", now.date.long(), now.time_short(s.prefs.twelve_hour)),
        _ => return Err(bad("`format` is long, short, time or dateTime")),
    };
    put_text(s, v, &text)
}

fn symbol(s: &mut Session, v: &Value) -> CmdResult {
    let symbol = p::req_str(v, "symbol")?;
    let symbol: String = symbol.chars().filter(|c| !c.is_control() && *c != PICTURE).take(8).collect();
    if symbol.is_empty() {
        return Err(bad("`symbol` is a character to put in"));
    }
    put_text(s, v, &symbol)
}

/// Where `needle` next is in `hay` from character `from` (wrapping round to the start), in
/// characters.
fn find_in(hay: &str, needle: &str, from: usize, match_case: bool) -> Option<Range<usize>> {
    let fold = |t: &str| if match_case { t.to_string() } else { t.to_lowercase() };
    let (hay_f, needle_f) = (fold(hay), fold(needle));
    // Case folding can change lengths; search char by char so positions stay those of `hay`.
    let hay_chars: Vec<char> = if hay_f.chars().count() == hay.chars().count() { hay_f.chars().collect() } else { hay.chars().collect() };
    let needle_chars: Vec<char> = if hay_f.chars().count() == hay.chars().count() { needle_f.chars().collect() } else { needle.chars().collect() };
    let n = needle_chars.len();
    if n == 0 || n > hay_chars.len() {
        return None;
    }
    let starts = (from.min(hay_chars.len())..=hay_chars.len() - n).chain(0..from.min(hay_chars.len() - n + 1));
    for start in starts {
        if hay_chars.get(start..start + n) == Some(needle_chars.as_slice()) {
            return Some(start..start + n);
        }
    }
    None
}

fn find(s: &mut Session, v: &Value) -> CmdResult {
    let needle = p::req_str(v, "text")?.to_string();
    let match_case = p::bool(v, "matchCase").unwrap_or(false);
    let c = compose_mut(s)?;
    let from = p::u64(v, "from").map_or_else(|| c.selection.map_or(0, |(a, b)| a.max(b)), |f| usize::try_from(f).unwrap_or(usize::MAX));
    match find_in(&c.body, &needle, from, match_case) {
        Some(r) => {
            c.selection = Some((r.start, r.end));
            c.typing = None;
            Ok(json!({"found": true, "start": r.start, "end": r.end}))
        }
        None => Ok(json!({"found": false})),
    }
}

fn replace(s: &mut Session, v: &Value) -> CmdResult {
    let needle = p::req_str(v, "text")?.to_string();
    let with = p::req_str(v, "with")?.to_string();
    if with.len() > MAX_INSERT {
        return Err(bad("`with` is too long"));
    }
    let all = p::bool(v, "all").unwrap_or(false);
    let match_case = p::bool(v, "matchCase").unwrap_or(false);
    let c = compose_mut(s)?;
    c.styles.normalize(&c.body);
    let mut count = 0usize;
    // All of them from the top; one from the selection on.
    let mut from = if all { 0 } else { c.selection.map_or(0, |(a, b)| a.min(b)) };
    let total_before = char_len(&c.body);
    while let Some(r) = find_in(&c.body, &needle, from, match_case) {
        // Once round: what comes before the start is reached by wrapping, not again.
        if all && count > 0 && r.start < from {
            break;
        }
        let style = c.styles.style_of(r.start).map(CharStyle::continued).unwrap_or_default();
        let mut body = std::mem::take(&mut c.body);
        c.styles.splice(&mut body, r.clone(), &with, &style);
        c.body = body;
        count += 1;
        from = r.start + char_len(&with);
        c.selection = Some((r.start, from));
        if !all || count > total_before {
            break;
        }
    }
    if count > 0 {
        c.typing = None;
        c.dirty = true;
    }
    Ok(json!({"replaced": count}))
}

/// Words, characters and paragraphs of a text.
pub fn counts(text: &str) -> Value {
    let words = text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count();
    let chars = text.chars().filter(|c| *c != '\n' && *c != PICTURE).count();
    let no_spaces = text.chars().filter(|c| !c.is_whitespace() && *c != PICTURE).count();
    let paragraphs = text.split('\n').filter(|l| !l.trim().is_empty()).count();
    json!({"words": words, "characters": chars, "charactersNoSpaces": no_spaces, "paragraphs": paragraphs, "lines": text.split('\n').count()})
}

fn word_count(s: &mut Session, _: &Value) -> CmdResult {
    let c = s.compose.as_ref().ok_or_else(|| fail("no message is being written"))?;
    Ok(counts(&c.body))
}

/// How much of the body is the owner's own: everything above the quoted original of a reply.
pub fn own_text(c: &Compose) -> &str {
    let end = super::compose::quote_start(c).map_or(c.body.len(), |at| rich::byte_at(&c.body, at));
    c.body.get(..end).unwrap_or(&c.body)
}

fn spelling(s: &mut Session, v: &Value) -> CmdResult {
    let text = match p::str(v, "text") {
        Some(t) => t.to_string(),
        None => s.compose.as_ref().map(|c| own_text(c).to_string()).ok_or_else(|| fail("no message is being written; pass `text`"))?,
    };
    if text.len() > crate::io::MAX_FILE as usize {
        return Err(bad("that text is too long to check"));
    }
    let suggestions = p::bool(v, "suggestions").unwrap_or(true);
    let found: Vec<Value> = crate::spell::check(&text)
        .into_iter()
        .enumerate()
        .map(|(i, m)| {
            // Suggestions for the first ones only: each takes a search of the word list.
            let near = if suggestions && i < 50 { crate::spell::suggest(&m.word, 5) } else { Vec::new() };
            json!({"word": m.word, "start": m.start, "end": m.end, "suggestions": near})
        })
        .collect();
    Ok(json!({"misspelled": found, "language": "en-US"}))
}

fn select(s: &mut Session, v: &Value) -> CmdResult {
    let start = p::u64(v, "start").ok_or_else(|| bad("`start` (number) is required"))?;
    let end = p::u64(v, "end").unwrap_or(start);
    let c = compose_mut(s)?;
    let len = char_len(&c.body);
    let clamp = |n: u64| usize::try_from(n).unwrap_or(usize::MAX).min(len);
    let now = (clamp(start), clamp(end));
    if c.selection != Some(now) {
        c.selection = Some(now);
        // A look chosen at a caret is for typing there.
        c.typing = None;
    }
    Ok(json!({"start": now.0, "end": now.1}))
}

fn format_html(s: &mut Session, _: &Value) -> CmdResult {
    let c = compose_mut(s)?;
    if !c.html {
        c.html = true;
        c.styles = rich::Styles::plain(&c.body);
        c.dirty = true;
    }
    Ok(json!({"html": true}))
}

fn format_plain(s: &mut Session, _: &Value) -> CmdResult {
    let c = compose_mut(s)?;
    if c.html {
        c.body = mailcraft_mime::rich::to_text(&c.body, &c.styles);
        c.styles = rich::Styles::plain(&c.body);
        c.html = false;
        c.typing = None;
        c.selection = None;
        c.dirty = true;
    }
    Ok(json!({"html": false}))
}

/// What the formatting buttons show for the selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatState {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub fixed: bool,
    pub bullets: bool,
    pub numbering: bool,
    pub align: Align,
    /// The size of the text, when it is all one size.
    pub size: Option<u8>,
    pub link: Option<String>,
}

/// What the formatting buttons show for the selection of the message being written.
pub fn state(c: &Compose) -> FormatState {
    if !c.html {
        return FormatState::default();
    }
    let mut styles = c.styles.clone();
    styles.normalize(&c.body);
    let len = char_len(&c.body);
    let r = match c.selection {
        Some((a, b)) => a.min(b).min(len)..a.max(b).min(len),
        None => 0..0,
    };
    let caret = typing_at(c, r.start);
    let is = |test: fn(&CharStyle) -> bool| if r.is_empty() { test(&caret) } else { styles.every(r.clone(), test) };
    let paras = styles.paras_in(&c.body, r.clone());
    let first = paras.first().copied().unwrap_or_default();
    let size = if r.is_empty() { caret.size } else { styles.style_of(r.start).and_then(|st| st.size) };
    FormatState {
        bold: is(|c| c.bold),
        italic: is(|c| c.italic),
        underline: is(|c| c.underline),
        strike: is(|c| c.strike),
        fixed: is(|c| c.fixed),
        bullets: !paras.is_empty() && paras.iter().all(|p| p.list == List::Bullet),
        numbering: !paras.is_empty() && paras.iter().all(|p| p.list == List::Number),
        align: first.align,
        size,
        link: styles
            .style_of(r.start)
            .and_then(|st| st.link.clone())
            .or_else(|| r.start.checked_sub(1).and_then(|i| styles.style_of(i)).and_then(|st| st.link.clone())),
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
