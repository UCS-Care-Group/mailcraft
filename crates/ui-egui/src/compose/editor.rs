// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's text in a message being written, drawn as it will be sent: bold, italic,
//! underline, a font size, links, bulleted and numbered lists, and a signature's pictures
//! (`mailcraft_model::rich`). The same editor edits a signature in File › Signatures
//! ([`Which`]); while one is open there, the message's body is only drawn.
//!
//! The widget only draws and listens. Every change, the caret and the selection included, is a
//! command (`compose.select`, `compose.type`, `compose.delete`, `format.*`, `compose.link`,
//! `edit.paste`, `compose.undo`/`redo`), so the keyboard, the ribbon, a script and an agent do the
//! same thing to the same text.
//!
//! Each paragraph is laid out once and kept with what it was laid out from ([`Layout`]); a
//! change lays out again only the paragraphs it touched, and a frame paints only the paragraphs
//! in view, so a long message costs what is on screen.
//!
//! The formatting keys (Ctrl+B, I, U, K, Z, Y; Cmd on a Mac) are the body's only while it has the
//! keyboard: elsewhere Ctrl+U marks a message unread and Ctrl+Z undoes what was done to the
//! mailbox (`keys::global_shortcuts` leaves them to the body, [`owns_chord`]).

use std::sync::{Arc, Mutex};

use egui::epaint::text::CharIndex;
use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Align2, Event, Galley, Key, Modifiers, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use mailcraft_model::rich::{ListKind, OBJECT, Paragraph, Selection, Style, numbers};
use mailcraft_model::signature::Picture;
use mailcraft_model::{RichBody, RichText};
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::{Tokens, regular, semibold};

/// The message's own font size on screen, in points (the reading pane's).
pub const BASE_SIZE: f32 = 13.0;

/// How far a list item's text stands in for each level, its bullet or number in the last step.
const INDENT: f32 = 28.0;

/// How far the text of a paragraph stands in: list items by their level.
fn indent_of(p: &Paragraph) -> f32 {
    if p.list == ListKind::None { 0.0 } else { INDENT * (f32::from(p.level) + 1.0) }
}

/// Longest text an input method is shown composing.
const MAX_PREEDIT: usize = 256;

/// Which text the editor shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// The message being written.
    Message,
    /// The signature open in File › Signatures.
    Signature,
}

impl Which {
    fn id(self) -> egui::Id {
        match self {
            Which::Message => id(),
            Which::Signature => egui::Id::unique("mailcraft-signature-body"),
        }
    }
}

/// The body's id: what has the keyboard when it does.
pub fn id() -> egui::Id {
    egui::Id::unique("mailcraft-compose-body")
}

/// Whether the body (or a signature being edited) has the keyboard.
pub fn has_focus(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.has_focus(id()) || m.has_focus(Which::Signature.id()))
}

/// The key a command has in the body (shown in tooltips and menus). They are not the registry's
/// shortcuts: they mean something else outside the body.
pub fn key_of(command: &str) -> Option<&'static str> {
    Some(match command {
        "format.bold" => "Mod+B",
        "format.italic" => "Mod+I",
        "format.underline" => "Mod+U",
        "compose.link" => "Mod+K",
        "compose.undo" => "Mod+Z",
        "compose.redo" => "Mod+Y",
        _ => return None,
    })
}

/// The keys that, with Mod, are the body's while it has the keyboard.
pub fn owns_chord(key: Key) -> bool {
    matches!(key, Key::B | Key::I | Key::U | Key::K | Key::Z | Key::Y | Key::A)
}

/// A size in points as the screen draws it (as the reading pane draws `font-size: Npt`).
fn on_screen(size: Option<u8>) -> f32 {
    size.map_or(BASE_SIZE, |pt| f32::from(pt) * 4.0 / 3.0)
}

/// How a run looks on screen; in a plain-text message everything is plain.
fn text_format(style: &Style, plain: bool, t: &Tokens) -> TextFormat {
    if plain {
        return TextFormat::simple(regular(BASE_SIZE), t.text);
    }
    let size = on_screen(style.size);
    let font = if style.bold { semibold(size) } else { regular(size) };
    // ponytail: a font is drawn in the window's own; the preview shows it (R4.5.4).
    let own = style.color.as_deref().and_then(|c| egui::Color32::from_hex(c).ok()).map(|c| crate::reading::colors::readable(c, t.surface));
    let color = if style.link.is_some() { t.accent_text } else { own.unwrap_or(t.text) };
    let underline = if style.underline || style.link.is_some() { Stroke::new(1.0, color) } else { Stroke::NONE };
    TextFormat { font_id: font, color, italics: style.italic, underline, ..Default::default() }
}

/// What a layout was made for: when any of it changes, every paragraph is laid out again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayoutKey {
    width: u32,
    dark: bool,
    scale: u32,
    plain: bool,
    /// The pictures' ids and widths.
    pictures: u64,
}

/// A picture in a paragraph, laid out: where it stands in the paragraph's text and where it is
/// drawn, in the paragraph.
#[derive(Clone)]
struct PictureBox {
    local: usize,
    id: u32,
    rect: Rect,
}

/// One paragraph, laid out.
struct Para {
    /// What it was laid out from.
    source: Paragraph,
    /// Its number in a numbered list (0 otherwise).
    number: u32,
    galley: Arc<Galley>,
    chars: usize,
    /// Its first character's position in the text, and its top in the body.
    start: usize,
    top: f32,
    pictures: Vec<PictureBox>,
}

impl Para {
    fn indent(&self) -> f32 {
        indent_of(&self.source)
    }

    fn height(&self) -> f32 {
        self.galley.size().y.max(1.0)
    }

    fn end(&self) -> usize {
        self.start + self.chars
    }

    /// The picture at a place in the paragraph.
    fn picture_at(&self, local: usize) -> Option<&PictureBox> {
        self.pictures.iter().find(|b| b.local == local)
    }

    /// The style of the character at `local` (the one after that offset).
    fn style_at(&self, local: usize) -> Option<&Style> {
        let mut seen = 0;
        for run in &self.source.runs {
            let n = run.text.chars().count();
            if local < seen + n {
                return Some(&run.style);
            }
            seen += n;
        }
        None
    }
}

/// The body's paragraphs as they were last laid out.
#[derive(Default)]
pub(crate) struct Layout {
    key: Option<LayoutKey>,
    paras: Vec<Para>,
    height: f32,
    /// How full the font atlas was when the paragraphs were last checked. It only fills up,
    /// except when it is made anew (the theme's text settings changed, or it was full): then the
    /// glyphs the galleys point at are gone, and everything is laid out again.
    atlas: f32,
    /// How many paragraphs were laid out so far, for tests.
    laid: usize,
}

impl Layout {
    /// Bring the layout up to `doc`: the paragraphs that changed (and only those) are laid out
    /// again. Whether anything did.
    fn sync(&mut self, ctx: &egui::Context, doc: &RichText, pictures: &[Picture], key: LayoutKey, t: &Tokens, width: f32) -> bool {
        let atlas = ctx.fonts(|f| f.font_atlas_fill_ratio());
        if self.key != Some(key) || atlas < self.atlas {
            self.paras.clear();
            self.key = Some(key);
        }
        self.atlas = atlas;
        let paragraphs = doc.paragraphs();
        let numbers = numbers(paragraphs);
        let same = |new: usize, old: usize| matches!((paragraphs.get(new), numbers.get(new), self.paras.get(old)), (Some(p), Some(n), Some(o)) if o.number == *n && o.source == *p);
        let (n_new, n_old) = (paragraphs.len(), self.paras.len());
        let shorter = n_new.min(n_old);
        let prefix = (0..shorter).take_while(|&i| same(i, i)).count();
        if prefix == n_new && n_new == n_old {
            return false;
        }
        let suffix = (0..shorter - prefix).take_while(|&k| same(n_new - 1 - k, n_old - 1 - k)).count();
        let fresh: Vec<Para> = (prefix..n_new - suffix)
            .filter_map(|i| Some(self.lay(ctx, paragraphs.get(i)?, numbers.get(i).copied().unwrap_or(0), key.plain, pictures, t, width)))
            .collect();
        self.paras.splice(prefix..n_old - suffix, fresh);
        let (mut start, mut top) = (0, 0.0);
        for p in &mut self.paras {
            p.start = start;
            p.top = top;
            start += p.chars + 1;
            top += p.height();
        }
        self.height = top;
        true
    }

    fn lay(&mut self, ctx: &egui::Context, p: &Paragraph, number: u32, plain: bool, pictures: &[Picture], t: &Tokens, width: f32) -> Para {
        let indent = indent_of(p);
        let room = (width - indent).max(24.0);
        let mut job = LayoutJob::default();
        job.wrap.max_width = room;
        // A picture is a gap in the line as wide as the picture before an unseen character, in a
        // line as tall as the picture; it is painted into the gap.
        let mut sized = Vec::new();
        let mut local = 0;
        for run in &p.runs {
            match run.picture.and_then(|id| Some((id, size_of(ctx, pictures.iter().find(|x| x.id == id)?, room)))) {
                Some((id, size)) if !plain => {
                    let format =
                        TextFormat { font_id: regular(1.0), color: egui::Color32::TRANSPARENT, line_height: Some(size.y), ..Default::default() };
                    job.append(&OBJECT.to_string(), size.x, format);
                    sized.push((local, id, size));
                }
                _ => job.append(&run.text, 0.0, text_format(&run.style, plain, t)),
            }
            local += run.text.chars().count();
        }
        if job.sections.is_empty() {
            job.append("", 0.0, text_format(&Style::default(), plain, t));
        }
        self.laid += 1;
        let galley = ctx.fonts_mut(|f| f.layout_job(job));
        let pictures = sized
            .into_iter()
            .filter_map(|(local, id, size)| {
                let glyph = galley.pos_from_cursor(CCursor::new(local));
                let rect = Rect::from_min_max(pos2(glyph.min.x - size.x, glyph.max.y - size.y), pos2(glyph.min.x, glyph.max.y));
                rect.is_finite().then_some(PictureBox { local, id, rect })
            })
            .collect();
        Para { chars: p.chars(), source: p.clone(), number, galley, start: 0, top: 0.0, pictures }
    }

    /// Characters in the text.
    fn len(&self) -> usize {
        self.paras.last().map_or(0, Para::end)
    }

    /// The paragraph a height in the body is in (the nearest one above or below it).
    fn para_at_y(&self, y: f32) -> Option<&Para> {
        let i = self.paras.partition_point(|p| p.top + p.height() <= y);
        self.paras.get(i.min(self.paras.len().saturating_sub(1)))
    }

    /// The paragraph a position is in.
    fn para_at(&self, pos: usize) -> Option<&Para> {
        let i = self.paras.partition_point(|p| p.end() < pos);
        self.paras.get(i.min(self.paras.len().saturating_sub(1)))
    }

    /// The position nearest to a point of the body.
    fn hit(&self, at: Vec2) -> usize {
        let Some(p) = self.para_at_y(at.y) else { return 0 };
        let local = vec2(at.x - p.indent(), at.y - p.top);
        if let Some(b) = p.pictures.iter().find(|b| b.rect.contains(local.to_pos2())) {
            return p.start + b.local + usize::from(local.x > b.rect.center().x);
        }
        p.start + p.galley.cursor_from_pos(local).index.0.min(p.chars)
    }

    /// The picture under a point of the body: its place in the text, its id and where it is.
    fn picture_hit(&self, at: Vec2) -> Option<(usize, u32, Rect)> {
        let p = self.para_at_y(at.y)?;
        let local = vec2(at.x - p.indent(), at.y - p.top);
        let b = p.pictures.iter().find(|b| b.rect.contains(local.to_pos2()))?;
        Some((p.start + b.local, b.id, b.rect.translate(vec2(p.indent(), p.top))))
    }

    /// The picture that is the selection, alone: its id and where it is in the body.
    fn selected_picture(&self, sel: Selection) -> Option<(u32, Rect)> {
        if sel.end() != sel.start() + 1 {
            return None;
        }
        let p = self.para_at(sel.start())?;
        let b = p.picture_at(sel.start().checked_sub(p.start)?)?;
        Some((b.id, b.rect.translate(vec2(p.indent(), p.top))))
    }

    /// Where the link under a point of the body leads.
    fn link_at(&self, at: Vec2) -> Option<String> {
        let p = self.para_at_y(at.y)?;
        if at.y < p.top || at.y > p.top + p.height() {
            return None;
        }
        let local = vec2(at.x - p.indent(), at.y - p.top);
        let c = p.galley.cursor_from_pos(local).index.0;
        // Over the text itself, not the room after the end of its line.
        let over = p.galley.pos_from_cursor(CCursor::new(c)).min.x;
        let next = p.galley.pos_from_cursor(CCursor::new(c + 1)).min.x;
        let column = if local.x < over { c.checked_sub(1)? } else { c };
        if local.x > next.max(over) + 1.0 && c >= p.chars {
            return None;
        }
        p.style_at(column).and_then(|s| s.link.clone())
    }

    /// The caret at a position, in the body.
    fn caret(&self, pos: usize) -> Rect {
        let Some(p) = self.para_at(pos) else { return Rect::from_min_size(pos2(0.0, 0.0), vec2(0.0, BASE_SIZE * 1.3)) };
        let local = pos.saturating_sub(p.start).min(p.chars);
        if let Some(b) = p.picture_at(local) {
            // Before a picture is at its left, not after its gap.
            return Rect::from_min_max(pos2(b.rect.min.x, b.rect.min.y), pos2(b.rect.min.x, b.rect.max.y)).translate(vec2(p.indent(), p.top));
        }
        let r = p.galley.pos_from_cursor(CCursor::new(local)).translate(vec2(p.indent(), p.top));
        if r.height() < 1.0 { Rect::from_min_size(pos2(p.indent(), p.top), vec2(0.0, p.height())) } else { r }
    }

    /// The start and the end of the line a position is on.
    fn line_ends(&self, pos: usize) -> (usize, usize) {
        let Some(p) = self.para_at(pos) else { return (0, 0) };
        let c = CCursor::new(pos.saturating_sub(p.start).min(p.chars));
        let begin = p.galley.cursor_begin_of_row(&c).index.0.min(p.chars);
        let end = p.galley.cursor_end_of_row(&c).index.0.min(p.chars);
        (p.start + begin, p.start + end)
    }

    /// The paragraph a position is in, as `[from, to)`.
    fn paragraph_ends(&self, pos: usize) -> (usize, usize) {
        self.para_at(pos).map_or((0, 0), |p| (p.start, p.end()))
    }

    /// The word a position is in or at the start of, as `[from, to)`.
    fn word_ends(&self, pos: usize) -> (usize, usize) {
        let Some(p) = self.para_at(pos) else { return (pos, pos) };
        let chars: Vec<char> = p.source.text().chars().collect();
        let at = pos.saturating_sub(p.start).min(chars.len());
        let kind = |i: usize| chars.get(i).map(|c| class(*c));
        let here = kind(at).or_else(|| at.checked_sub(1).and_then(kind));
        let Some(here) = here else { return (pos, pos) };
        let mut a = at.min(chars.len());
        while a > 0 && kind(a - 1) == Some(here) {
            a -= 1;
        }
        let mut b = at;
        while kind(b) == Some(here) {
            b += 1;
        }
        (p.start + a, p.start + b.max(a))
    }

    /// Where Ctrl+Left goes from a position: the start of this word or the one before.
    fn word_left(&self, pos: usize) -> usize {
        let Some(p) = self.para_at(pos) else { return 0 };
        let chars: Vec<char> = p.source.text().chars().collect();
        let mut i = pos.saturating_sub(p.start).min(chars.len());
        if i == 0 {
            return pos.saturating_sub(1);
        }
        while i > 0 && chars.get(i - 1).is_some_and(|c| class(*c) != 2) {
            i -= 1;
        }
        while i > 0 && chars.get(i - 1).is_some_and(|c| class(*c) == 2) {
            i -= 1;
        }
        p.start + i
    }

    /// Where Ctrl+Right goes from a position: the end of this word or the next.
    fn word_right(&self, pos: usize) -> usize {
        let Some(p) = self.para_at(pos) else { return 0 };
        let chars: Vec<char> = p.source.text().chars().collect();
        let mut i = pos.saturating_sub(p.start).min(chars.len());
        if i >= chars.len() {
            return (pos + 1).min(self.len());
        }
        while chars.get(i).is_some_and(|c| class(*c) != 2) {
            i += 1;
        }
        while chars.get(i).is_some_and(|c| class(*c) == 2) {
            i += 1;
        }
        p.start + i
    }
}

/// What a character is for moving by words: 0 blank, 1 punctuation, 2 part of a word.
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' || c == '\'' {
        2
    } else {
        1
    }
}

/// The size a picture is shown at, in points (a CSS pixel each), within `room`: the width the
/// owner gave it or its own, its height in proportion.
fn size_of(ctx: &egui::Context, p: &Picture, room: f32) -> Vec2 {
    let frame = crate::reading::images::frame(ctx);
    let own = crate::reading::images::Pictures::of(ctx).lock().ok().and_then(|mut c| c.size(&p.data, frame));
    let [w, h] = own.map_or([120.0, 40.0], |[w, h]| [w.max(1) as f32, h.max(1) as f32]);
    let width = p.width.map_or(w, f32::from).min(room).max(8.0);
    vec2(width, (width * h / w).clamp(4.0, 2000.0))
}

/// A short fingerprint of the pictures' ids and widths, so that the layout follows them.
fn pictures_key(pictures: &[Picture]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in pictures {
        (p.id, p.width, std::sync::Arc::as_ptr(&p.data)).hash(&mut h);
    }
    h.finish()
}

/// The layout of the body, kept from frame to frame.
fn store(ctx: &egui::Context, which: Which) -> Arc<Mutex<Layout>> {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<Layout>>>(which.id().with("layout")).clone())
}

/// How many paragraphs of the body were laid out since the window opened, for tests.
#[cfg(test)]
pub(crate) fn laid(ctx: &egui::Context) -> usize {
    store(ctx, Which::Message).lock().map_or(0, |l| l.laid)
}

/// What the body remembers between frames besides its layout.
#[derive(Clone, Default)]
struct State {
    /// Where the caret goes up and down from (the column it was in before it went up or down).
    column: Option<f32>,
    /// Where the pointer went down, for dragging a selection from there.
    anchor: Option<usize>,
    /// What an input method is composing, not yet typed.
    preedit: String,
    /// A picture's corner being dragged: its id and the width it has so far.
    resizing: Option<(u32, f32)>,
}

/// What the keyboard and the pointer do to the text, as the command that does it.
struct Ask {
    command: &'static str,
    params: Value,
}

fn ask(command: &'static str, params: Value) -> Option<Ask> {
    Some(Ask { command, params })
}

fn select(anchor: usize, head: usize) -> Option<Ask> {
    ask("compose.select", json!({"from": anchor, "to": head}))
}

/// The text `which` names, and whether it is plain.
fn body(app: &MailApp, which: Which) -> Option<(&RichBody, bool)> {
    match which {
        Which::Message => app.session.compose.as_ref().map(|c| (&c.body, c.format == mailcraft_engine::BodyFormat::Plain)),
        Which::Signature => app.session.signature_edit.as_ref().map(|e| (&e.body, false)),
    }
}

/// The pictures the text `which` names may show.
fn pictures(app: &MailApp, which: Which) -> Vec<Picture> {
    match which {
        // The signature's, and those the owner put in the text (#547).
        Which::Message => app
            .session
            .compose
            .as_ref()
            .map(|c| c.signature.iter().flat_map(|s| s.pictures.iter()).chain(c.pictures.iter()).cloned().collect())
            .unwrap_or_default(),
        Which::Signature => app.session.signature_edit.as_ref().map(|e| e.signature.pictures().to_vec()).unwrap_or_default(),
    }
}

/// The selection, the caret and whether the text is plain.
fn now(app: &MailApp, which: Which) -> Option<(Selection, bool)> {
    body(app, which).map(|(b, plain)| (b.selection(), plain))
}

/// The body in the room `ui` has, at least `min_height` tall. `take_focus`: it gets the keyboard
/// now (a reply that just opened).
pub(crate) fn show(app: &mut MailApp, ui: &mut Ui, min_height: f32, take_focus: bool) {
    show_doc(app, ui, Which::Message, min_height, take_focus);
}

/// The text `which` names, edited. The message's body is only drawn while a signature is open:
/// the editing commands are the signature's then.
pub(crate) fn show_doc(app: &mut MailApp, ui: &mut Ui, which: Which, min_height: f32, take_focus: bool) {
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let id = which.id();
    let width = ui.available_width().max(40.0);
    let Some((_, plain)) = now(app, which) else { return };
    let editable = which == Which::Signature || app.session.signature_edit.is_none();
    let shown = pictures(app, which);
    let key = LayoutKey { width: width as u32, dark: t.dark, scale: ctx.pixels_per_point().to_bits(), plain, pictures: pictures_key(&shown) };
    let store = store(&ctx, which);
    let mut layout = store.lock().unwrap_or_else(|e| e.into_inner());
    let resync = |layout: &mut Layout, app: &MailApp| {
        if let Some((b, _)) = body(app, which) {
            layout.sync(&ctx, b.text(), &shown, key, &t, width);
        }
    };
    resync(&mut layout, app);

    let (rect, _) = ui.allocate_exact_size(vec2(width, layout.height.max(min_height)), Sense::hover());
    let resp = ui.interact(rect, id, if editable { Sense::click_and_drag() } else { Sense::hover() });
    app.hit.add(if which == Which::Message { "composeBody" } else { "signatureBody" }, 0, rect);
    let origin = rect.min;
    let state_id = id.with("state");
    let mut st: State = ctx.data(|d| d.get_temp(state_id)).unwrap_or_default();
    if editable && (take_focus || (which == Which::Message && std::mem::take(&mut app.body_focus_wanted))) {
        resp.request_focus();
    }
    let mut reveal = false;

    // A selected picture of a signature is made wider or narrower by its corner.
    if which == Which::Signature
        && let Some((sel, _)) = now(app, which)
        && let Some((pid, r)) = layout.selected_picture(sel)
    {
        let r = r.translate(origin.to_vec2());
        let corner = Rect::from_center_size(r.right_bottom(), vec2(10.0, 10.0));
        let handle = ui.interact(corner, id.with("corner"), Sense::drag());
        if handle.hovered() || handle.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
        if handle.dragged() {
            let w = st.resizing.filter(|(i, _)| *i == pid).map_or(r.width(), |(_, w)| w) + handle.drag_delta().x;
            st.resizing = Some((pid, w.clamp(8.0, f32::from(mailcraft_model::signature::MAX_WIDTH))));
        }
        if handle.drag_stopped()
            && let Some((i, w)) = st.resizing.take()
        {
            let _ = app.run("signature.picture.set", json!({"id": i, "width": w.round() as u16}));
        }
    }

    // The pointer: a click puts the caret, or selects a picture; a drag selects, a double click a
    // word, a triple click the paragraph; Shift and a click selects from where the selection
    // started.
    if editable
        && st.resizing.is_none()
        && let Some(pointer) = resp.interact_pointer_pos()
        && let Some((sel, _)) = now(app, which)
    {
        let at = layout.hit(pointer - origin);
        let (pressed, shift) = ui.input(|i| (i.pointer.primary_pressed(), i.modifiers.shift));
        let picture = layout.picture_hit(pointer - origin).filter(|_| pressed && !shift);
        let asked = if let Some((place, _, _)) = picture {
            st.anchor = None;
            resp.request_focus();
            select(place, place + 1)
        } else if resp.triple_clicked() {
            st.anchor = None;
            let (a, b) = layout.paragraph_ends(at);
            select(a, b)
        } else if resp.double_clicked() {
            st.anchor = None;
            let (a, b) = layout.word_ends(at);
            select(a, b)
        } else if pressed && resp.is_pointer_button_down_on() {
            let anchor = if shift { sel.anchor } else { at };
            st.anchor = Some(anchor);
            resp.request_focus();
            select(anchor, at)
        } else if resp.dragged() {
            reveal = true;
            st.anchor.filter(|a| (*a, at) != (sel.anchor, sel.head)).and_then(|a| select(a, at))
        } else {
            None
        };
        if let Some(Ask { command, params }) = asked {
            let _ = app.execute(command, params);
            st.column = None;
        }
    }
    if resp.hovered() && editable {
        ctx.set_cursor_icon(egui::CursorIcon::Text);
        if let Some(url) = ui.input(|i| i.pointer.hover_pos()).and_then(|p| layout.link_at(p - origin)) {
            resp.clone().on_hover_text(url);
        }
    }

    // The keyboard.
    let focused = editable && (resp.has_focus() || ctx.memory(|m| m.has_focus(id)));
    // Coming back to the window: the system's input method is armed again (some desktops leave
    // it off after the window was left, and then nothing typed arrives), and what it was
    // composing when the window was left is forgotten.
    let (left, came_back) = ui.input(|i| {
        let seen = |to: bool| i.events.iter().any(|e| matches!(e, Event::WindowFocused(f) if *f == to));
        (seen(false), seen(true))
    });
    if left || came_back {
        st.preedit.clear();
    }
    if focused {
        ctx.memory_mut(|m| {
            m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false })
        });
        let page = ui.clip_rect().height().max(40.0) - 20.0;
        for event in ui.input(|i| i.events.clone()) {
            resync(&mut layout, app);
            let Some((sel, plain)) = now(app, which) else { break };
            let asked = match event {
                Event::Text(text) if !text.is_empty() => ask("compose.type", json!({"text": text})),
                Event::Ime(egui::ImeEvent::Commit(text)) => {
                    st.preedit.clear();
                    (!text.is_empty() && text != "\n" && text != "\r").then(|| Ask { command: "compose.type", params: json!({"text": text}) })
                }
                Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                    st.preedit = text.chars().take(MAX_PREEDIT).collect();
                    None
                }
                Event::Copy | Event::Cut if !sel.is_empty() => {
                    // HTML of the set and its text, through the host's clipboard where it has
                    // one that takes HTML (the desktop); egui's takes the text.
                    let cut = matches!(event, Event::Cut);
                    if let Ok(got) = app.execute(if cut { "edit.cut" } else { "edit.copy" }, json!({})) {
                        let text = got.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                        let html = got.get("html").and_then(Value::as_str);
                        let put = match (&app.services.clipboard, html) {
                            (Some(board), Some(html)) => board.put(html, &text),
                            _ => Err(String::new()),
                        };
                        if put.is_err() {
                            ctx.copy_text(text);
                        }
                        reveal = true;
                    }
                    None
                }
                // What is pasted comes as HTML when the clipboard has some (reduced to the set
                // by `edit.paste`), else as the text egui read from it; into a signature, a
                // picture on the clipboard comes as a picture.
                Event::Paste(text) => {
                    let html = app.services.clipboard.as_ref().and_then(|board| board.html()).filter(|h| !h.trim().is_empty());
                    // Into the message, a picture only when nothing else is on the clipboard (#547).
                    let image = (text.is_empty() && (which == Which::Signature || html.is_none()))
                        .then(|| app.services.clipboard.as_ref().and_then(|b| b.image()))
                        .flatten();
                    match (image, html) {
                        (Some(png), _) => {
                            let command = if which == Which::Signature { "signature.picture.add" } else { "insert.picture" };
                            ask(command, json!({"data": crate::files::base64_lite::encode(&png), "file": "picture.png"}))
                        }
                        (None, Some(html)) => ask("edit.paste", json!({"html": html})),
                        (None, None) => ask("edit.paste", json!({"text": text})),
                    }
                }
                Event::Key { key, pressed: true, modifiers, .. } => {
                    let (asked, vertical) = key_press(&layout, &mut st, sel, plain, key, modifiers, page);
                    if !vertical {
                        st.column = None;
                    }
                    asked
                }
                _ => None,
            };
            if let Some(Ask { command, params }) = asked {
                reveal = true;
                // `run`: the link's dialog opens from Ctrl+K, and what goes wrong is said.
                let enabled = app.enabled(command);
                if enabled
                    || !matches!(command, "compose.undo" | "compose.redo" | "format.bold" | "format.italic" | "format.underline" | "compose.link")
                {
                    let _ = app.run(command, params);
                }
            }
        }
        resync(&mut layout, app);
    } else {
        st.preedit.clear();
    }

    // Paint what is in view.
    let Some((sel, _)) = now(app, which) else { return };
    let painter = ui.painter();
    let clip = ui.clip_rect();
    let fill = ui.visuals().selection.bg_fill;
    let fill = if focused { fill } else { fill.gamma_multiply(0.55) };
    let first = layout.paras.partition_point(|p| origin.y + p.top + p.height() < clip.min.y);
    for p in layout.paras.iter().skip(first) {
        if origin.y + p.top > clip.max.y {
            break;
        }
        let at = origin + vec2(p.indent(), p.top);
        if !sel.is_empty() && sel.start() <= p.end() && sel.end() >= p.start {
            paint_selection(painter, p, at, sel, fill);
        }
        painter.galley(at, p.galley.clone(), t.text);
        for b in &p.pictures {
            let r = b.rect.translate(at.to_vec2());
            let picture = shown.iter().find(|x| x.id == b.id);
            let cache = crate::reading::images::Pictures::of(&ctx);
            let texture = picture.and_then(|x| cache.lock().ok().map(|mut c| c.texture(&ctx, &x.data, r.size())));
            match texture {
                Some(crate::reading::images::Picture::Ready(tex)) => {
                    painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                Some(crate::reading::images::Picture::Pending) => {
                    painter.rect_filled(r, 2.0, t.panel);
                    ctx.request_repaint();
                }
                _ => {
                    painter.rect_stroke(r, 2.0, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                    let alt = picture.map(|x| x.alt.clone()).unwrap_or_default();
                    painter.with_clip_rect(r).text(r.left_top() + vec2(4.0, 4.0), Align2::LEFT_TOP, alt, regular(11.0), t.text_dim);
                }
            }
        }
        let row = p.galley.rows.first().map_or(Rect::from_min_size(at, vec2(0.0, p.height())), |r| r.rect().translate(at.to_vec2()));
        let ink = if plain { t.text } else { p.source.runs.first().map_or(t.text, |r| if r.style.link.is_some() { t.accent_text } else { t.text }) };
        let size = if plain { BASE_SIZE } else { on_screen(p.source.runs.first().and_then(|r| r.style.size)) };
        // The marker stands in the last step of the indent: a bullet of the level's kind (filled
        // circle, open circle, square, and round again), or the item's number.
        let step = origin.x + p.indent() - INDENT;
        match p.source.list {
            ListKind::None => {}
            ListKind::Bullet => {
                let c = pos2(step + INDENT * 0.45, row.center().y);
                let r = (size * 0.19).clamp(2.0, 5.0);
                match p.source.level % 3 {
                    0 => {
                        painter.circle_filled(c, r, ink);
                    }
                    1 => {
                        painter.circle_stroke(c, r * 0.9, Stroke::new(1.0, ink));
                    }
                    _ => {
                        painter.rect_filled(Rect::from_center_size(c, vec2(r * 1.6, r * 1.6)), 0.0, ink);
                    }
                }
            }
            ListKind::Numbered => {
                painter.text(pos2(step + INDENT - 7.0, row.center().y), Align2::RIGHT_CENTER, format!("{}.", p.number), regular(size), ink);
            }
        }
    }
    if layout.len() == 0 && !focused {
        let hint = if which == Which::Message { tl!("Write your message") } else { tl!("Write your signature") };
        painter.text(origin, Align2::LEFT_TOP, hint, regular(BASE_SIZE), t.text_disabled);
    }
    // The selected picture: a frame, and the corner that sizes it.
    if let Some((pid, r)) = layout.selected_picture(sel) {
        let mut r = r.translate(origin.to_vec2());
        if let Some((_, w)) = st.resizing.filter(|(i, _)| *i == pid) {
            r = Rect::from_min_size(r.min, vec2(w, w * r.height() / r.width().max(1.0)));
        }
        painter.rect_stroke(r, 0.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Outside);
        if which == Which::Signature {
            painter.rect_filled(Rect::from_center_size(r.right_bottom(), vec2(8.0, 8.0)), 1.0, t.accent);
        }
    }
    let caret = layout.caret(sel.head).translate(origin.to_vec2());
    if focused {
        if st.preedit.is_empty() {
            painter.line_segment([caret.center_top(), caret.center_bottom()], Stroke::new(1.5, t.text));
        } else {
            // What the input method is composing, over the text at the caret until it is typed.
            let galley = ctx.fonts_mut(|f| f.layout_no_wrap(st.preedit.clone(), regular(BASE_SIZE), t.text));
            let r = Rect::from_min_size(caret.left_top(), galley.size());
            painter.rect_filled(r.expand(1.0), 2.0, t.input);
            painter.galley(r.min, galley, t.text);
            painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, t.text));
        }
        let to_global = ctx.layer_transform_to_global(ui.layer_id()).unwrap_or_default();
        ctx.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Normal,
                rect: to_global * rect,
                cursor_rect: to_global * caret,
                should_interrupt_composition: came_back,
            });
        });
    }
    if reveal {
        ui.scroll_to_rect(caret.expand2(vec2(2.0, 6.0)), None);
    }
    drop(layout);
    ctx.data_mut(|d| d.insert_temp(state_id, st));
}

/// The selection's part of a paragraph, behind its text.
fn paint_selection(painter: &egui::Painter, p: &Para, at: egui::Pos2, sel: Selection, fill: egui::Color32) {
    let a = sel.start().saturating_sub(p.start).min(p.chars);
    let b = sel.end().saturating_sub(p.start).min(p.chars);
    // The paragraph's break is selected too: a sliver after its end says so.
    let breaks = sel.end() > p.end();
    let mut seen = 0usize;
    let rows = p.galley.rows.len();
    for (i, row) in p.galley.rows.iter().enumerate() {
        let n = row.char_count_including_newline().0;
        let (rs, re) = (seen, seen + n);
        seen = re;
        let last = i + 1 == rows;
        let (lo, hi) = (a.max(rs), b.min(re));
        if lo >= hi && !(breaks && last && hi >= lo) {
            continue;
        }
        let x0 = row.pos.x + row.x_offset(CharIndex(lo.saturating_sub(rs)));
        let x1 = row.pos.x + row.x_offset(CharIndex(hi.saturating_sub(rs))) + if breaks && last { 5.0 } else { 0.0 };
        let r = Rect::from_min_max(pos2(x0, row.min_y()), pos2(x1, row.max_y())).translate(at.to_vec2());
        painter.rect_filled(r, 0.0, fill);
    }
}

/// What a key does: the command, and whether it moved the caret up or down (which keeps the
/// column it goes up and down in).
fn key_press(layout: &Layout, st: &mut State, sel: Selection, plain: bool, key: Key, m: Modifiers, page: f32) -> (Option<Ask>, bool) {
    let len = layout.len();
    let head = sel.head;
    // Moving by words: Ctrl (Alt on a Mac); to the ends of a line: Cmd on a Mac.
    let word = m.alt || (m.ctrl && !m.mac_cmd);
    let to = |target: usize| {
        let anchor = if m.shift { sel.anchor } else { target };
        select(anchor, target.min(len))
    };
    let chord = m.command && !m.alt;
    let vertical = |st: &mut State, dy: f32| {
        let caret = layout.caret(head);
        let column = *st.column.get_or_insert(caret.center().x);
        let y = if dy < 0.0 { caret.min.y + dy } else { caret.max.y + dy };
        layout.hit(vec2(column, y))
    };
    let asked = match key {
        Key::B | Key::I | Key::U if chord && !m.shift => {
            if plain {
                None
            } else {
                ask(
                    match key {
                        Key::B => "format.bold",
                        Key::I => "format.italic",
                        _ => "format.underline",
                    },
                    json!({}),
                )
            }
        }
        Key::K if chord && !plain => ask("compose.link", json!({})),
        Key::Z if chord => ask(if m.shift { "compose.redo" } else { "compose.undo" }, json!({})),
        Key::Y if chord => ask("compose.redo", json!({})),
        Key::A if chord => select(0, len),
        Key::ArrowLeft => {
            let target = if !sel.is_empty() && !m.shift {
                sel.start()
            } else if m.mac_cmd {
                layout.line_ends(head).0
            } else if word {
                layout.word_left(head)
            } else {
                head.saturating_sub(1)
            };
            to(target)
        }
        Key::ArrowRight => {
            let target = if !sel.is_empty() && !m.shift {
                sel.end()
            } else if m.mac_cmd {
                layout.line_ends(head).1
            } else if word {
                layout.word_right(head)
            } else {
                (head + 1).min(len)
            };
            to(target)
        }
        Key::ArrowUp if m.mac_cmd => to(0),
        Key::ArrowDown if m.mac_cmd => to(len),
        Key::ArrowUp => return (to(vertical(st, -2.0)), true),
        Key::ArrowDown => return (to(vertical(st, 2.0)), true),
        Key::PageUp => return (to(vertical(st, -page)), true),
        Key::PageDown => return (to(vertical(st, page)), true),
        Key::Home => to(if m.ctrl || m.command { 0 } else { layout.line_ends(head).0 }),
        Key::End => to(if m.ctrl || m.command { len } else { layout.line_ends(head).1 }),
        Key::Enter if !m.command && !m.alt => ask("compose.type", json!({"text": "\n"})),
        Key::Backspace if sel.is_empty() && word && head > 0 => ask("compose.delete", json!({"from": layout.word_left(head), "to": head})),
        Key::Backspace => ask("compose.delete", json!({})),
        Key::Delete if sel.is_empty() && word && head < len => ask("compose.delete", json!({"from": head, "to": layout.word_right(head)})),
        Key::Delete => ask("compose.delete", json!({"forward": true})),
        // In a list, Tab at an item's start (or over a selection) takes it a level deeper and
        // Shift+Tab a level out (out of the list from the outermost); elsewhere Tab is a tab.
        Key::Tab if !m.command && !m.alt && !m.shift => {
            let in_list = layout.para_at(head).is_some_and(|p| p.source.list != ListKind::None);
            let at_start = layout.paragraph_ends(head).0 == head;
            if in_list && !plain && (at_start || !sel.is_empty()) {
                ask("format.indent", json!({}))
            } else {
                ask("compose.type", json!({"text": "\t"}))
            }
        }
        Key::Tab if m.shift && !plain && layout.para_at(head).is_some_and(|p| p.source.list != ListKind::None) => ask("format.outdent", json!({})),
        _ => None,
    };
    (asked, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_letters_and_digits() {
        assert_eq!([class(' '), class(','), class('é'), class('7')], [0, 1, 2, 2]);
    }
}
