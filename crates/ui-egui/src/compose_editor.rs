//! The body of a message being written: a text editor that shows the formatting of an HTML
//! message as it will look (bold, italic, sizes, colours, highlights, links, lists, indents,
//! alignment, lines across and pictures), with misspelt words underlined, and the formatting bar
//! above it.
//!
//! The text itself is edited as plain text (the engine's `Compose::body`); what it looks like
//! comes from `Compose::styles` and is laid out paragraph by paragraph here. Every change goes
//! through a command: `compose.set` for the text, `compose.select` for the selection, and the
//! `format.*` commands for the look.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, Galley, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Compose;
use mailcraft_engine::model::Rgb;
use mailcraft_engine::model::rich::{self, Align, CharStyle, List, PICTURE, ParaStyle, Styles};
use serde_json::{Value, json};

use crate::reading::colors::Paper;
use crate::reading::images::{Picture, Pictures, frame};
use crate::theme::{Tokens, mono, regular, semibold};
use crate::{MailApp, icons};

/// The size of text that names none, in points.
pub const BASE: f32 = 13.0;
/// How far one step of indent moves a paragraph in.
const INDENT_STEP: f32 = 28.0;
/// The room a list marker takes.
const MARKER_W: f32 = 20.0;
/// Widest and tallest a picture is shown while writing.
const PICTURE_W: f32 = 480.0;
const PICTURE_H: f32 = 360.0;

/// Colours Font Color offers (our own choice), after Automatic.
pub const FONT_COLORS: [(u32, &str); 10] = [
    (0xB91C1C, tn!("Dark red")),
    (0xE5484D, tn!("Red")),
    (0xEA580C, tn!("Orange")),
    (0xCA8A04, tn!("Gold")),
    (0x16A34A, tn!("Green")),
    (0x0D9488, tn!("Teal")),
    (0x2563EB, tn!("Blue")),
    (0x1E3A8A, tn!("Navy")),
    (0x7C3AED, tn!("Purple")),
    (0x4B5563, tn!("Grey")),
];

/// Colours Text Highlight Color offers (our own choice), after None.
pub const HIGHLIGHTS: [(u32, &str); 6] = [
    (0xFFF176, tn!("Yellow")),
    (0xB9F6CA, tn!("Green")),
    (0xB3E5FC, tn!("Blue")),
    (0xF8BBD0, tn!("Pink")),
    (0xFFD59E, tn!("Orange")),
    (0xE0E0E0, tn!("Grey")),
];

fn rgb(hex: u32) -> Rgb {
    Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn html_rgb(c: Rgb) -> mailcraft_engine::html::Rgb {
    mailcraft_engine::html::Rgb(c.0, c.1, c.2)
}

/// What the layout of the body needs, apart from the text.
#[derive(Clone)]
struct Look {
    t: Tokens,
    styles: Styles,
    html: bool,
    /// Misspelt words, in characters.
    misspelt: Arc<Vec<Range<usize>>>,
    /// How large each picture is shown, by content id.
    pictures: HashMap<String, egui::Vec2>,
    pixels_per_point: f32,
}

/// The size a run of text is set in.
fn size_of(style: &CharStyle) -> f32 {
    style.size.map_or(BASE, f32::from)
}

/// How a run of text looks on the editor's paper.
fn format_of(look: &Look, style: &CharStyle, misspelt: bool) -> TextFormat {
    let t = &look.t;
    let size = size_of(style);
    let font = if style.fixed {
        mono(size - 0.5)
    } else if style.bold {
        semibold(size)
    } else {
        regular(size)
    };
    let window = Paper::window(t);
    let paper = style.highlight.map_or(window, |h| Paper::of(html_rgb(h), t));
    let ink = paper.ink(t, style.color.map(html_rgb), style.link.is_some(), false);
    let underline = if misspelt {
        Stroke::new(1.5, t.red)
    } else if style.underline || style.link.is_some() {
        Stroke::new(1.0, ink)
    } else {
        Stroke::NONE
    };
    TextFormat {
        font_id: font,
        color: ink,
        background: if style.highlight.is_some() { paper.fill } else { Color32::TRANSPARENT },
        italics: style.italic,
        underline,
        strikethrough: if style.strike { Stroke::new(1.0, ink) } else { Stroke::NONE },
        line_height: Some((size * 1.45).round()),
        ..TextFormat::default()
    }
}

/// How far a paragraph's text is moved in.
fn inset(para: &ParaStyle) -> f32 {
    f32::from(para.indent.min(rich::MAX_INDENT)) * INDENT_STEP + if para.list != List::None { MARKER_W } else { 0.0 }
}

/// Lay out the body: each paragraph wrapped in the room it has, moved in and aligned, then all
/// of them one under the other.
fn layout(ui: &Ui, text: &str, look: &Look, wrap: f32) -> Arc<Galley> {
    let wrap = if wrap.is_finite() { wrap.max(80.0) } else { 600.0 };
    let mut galleys: Vec<Arc<Galley>> = Vec::new();
    let mut at = 0usize;
    let mut runs = RunCursor::new(&look.styles);
    let plain = CharStyle::default();
    for (index, line) in text.split('\n').enumerate() {
        let para = if look.html { look.styles.paras.get(index).copied().unwrap_or_default() } else { ParaStyle::default() };
        let inset = inset(&para).min(wrap - 60.0).max(0.0);
        let room = (wrap - inset).max(60.0);
        let mut job = LayoutJob { wrap: TextWrapping { max_width: room, ..TextWrapping::default() }, ..LayoutJob::default() };
        if line.is_empty() || para.rule {
            // An empty paragraph is as high as the text typed into it would be.
            let style = if look.html { look.styles.typing_style(at) } else { plain.clone() };
            let mut format = format_of(look, &style, false);
            if para.rule {
                format.line_height = Some(BASE * 1.6);
            }
            job.append(line, 0.0, format);
        } else {
            let mut piece = String::new();
            let mut piece_format: Option<TextFormat> = None;
            for (i, c) in line.chars().enumerate() {
                let style = if look.html { runs.style_at(at + i) } else { &plain };
                let misspelt = look.misspelt.iter().any(|r| r.contains(&(at + i)));
                if c == PICTURE {
                    if let Some(f) = piece_format.take() {
                        job.append(&std::mem::take(&mut piece), 0.0, f);
                    }
                    let size = style.picture.as_ref().and_then(|cid| look.pictures.get(cid)).copied().unwrap_or(vec2(24.0, 18.0));
                    let mut format = format_of(look, &CharStyle::default(), false);
                    format.color = Color32::TRANSPARENT;
                    format.line_height = Some(size.y.max(BASE * 1.45));
                    format.valign = egui::Align::BOTTOM;
                    let mut buf = [0u8; 4];
                    job.append(c.encode_utf8(&mut buf), size.x.min(room - 10.0).max(0.0), format);
                    continue;
                }
                let format = format_of(look, style, misspelt);
                if piece_format.as_ref() != Some(&format) {
                    if let Some(f) = piece_format.take() {
                        job.append(&std::mem::take(&mut piece), 0.0, f);
                    }
                    piece_format = Some(format);
                }
                piece.push(c);
            }
            if let Some(f) = piece_format.take() {
                job.append(&piece, 0.0, f);
            }
        }
        at += rich::char_len(line) + 1;
        let laid = ui.ctx().fonts_mut(|f| f.layout_job(job));
        let mut g: Galley = (*laid).clone();
        // In, and along its line as the paragraph is aligned.
        let mut rect = Rect::NOTHING;
        let mut mesh = Rect::NOTHING;
        for row in &mut g.rows {
            let width = row.size.x;
            let along = match para.align {
                Align::Left => 0.0,
                Align::Center => ((room - width) / 2.0).max(0.0),
                Align::Right => (room - width).max(0.0),
            };
            row.pos.x += inset + along;
            rect |= row.rect();
            mesh |= row.visuals.mesh_bounds.translate(row.pos.to_vec2());
        }
        if rect.is_positive() || rect.height() > 0.0 {
            g.rect = rect;
        }
        g.mesh_bounds = mesh;
        galleys.push(Arc::new(g));
    }
    let mut whole = LayoutJob { wrap: TextWrapping { max_width: wrap, ..TextWrapping::default() }, ..LayoutJob::default() };
    whole.append(text, 0.0, format_of(look, &plain, false));
    Arc::new(Galley::concat(Arc::new(whole), &galleys, look.pixels_per_point))
}

/// The styles of a text, one character after the other.
struct RunCursor<'a> {
    runs: &'a [rich::Run],
    /// The run the last character asked about was in, and where it starts.
    index: usize,
    start: usize,
}

impl<'a> RunCursor<'a> {
    fn new(styles: &'a Styles) -> Self {
        RunCursor { runs: &styles.runs, index: 0, start: 0 }
    }

    /// The style of character `at`; characters are asked for in order.
    fn style_at(&mut self, at: usize) -> &'a CharStyle {
        static PLAIN: CharStyle = CharStyle {
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            fixed: false,
            size: None,
            color: None,
            highlight: None,
            link: None,
            picture: None,
        };
        if at < self.start {
            self.index = 0;
            self.start = 0;
        }
        while let Some(run) = self.runs.get(self.index) {
            if at < self.start + run.len {
                return &run.style;
            }
            self.start += run.len;
            self.index += 1;
        }
        &PLAIN
    }
}

/// The misspelt words of the owner's own text (not the quoted original), worked out again only
/// when the text changes.
fn misspellings(ui: &Ui, c: &Compose) -> Arc<Vec<Range<usize>>> {
    let own = mailcraft_engine::cmd::format::own_text(c);
    let key = own.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3));
    let id = egui::Id::new("compose_misspelt");
    if let Some((k, found)) = ui.data(|d| d.get_temp::<(u64, Arc<Vec<Range<usize>>>)>(id))
        && k == key
    {
        return found;
    }
    let found: Arc<Vec<Range<usize>>> = Arc::new(mailcraft_engine::spell::check(own).into_iter().map(|m| m.start..m.end).collect());
    ui.data_mut(|d| d.insert_temp(id, (key, found.clone())));
    found
}

/// The size each picture of the message is shown at while writing, by content id.
fn picture_sizes(ui: &Ui, c: &Compose, room: f32) -> HashMap<String, egui::Vec2> {
    let mut out = HashMap::new();
    if !c.html {
        return out;
    }
    let pictures = Pictures::of(ui.ctx());
    let Ok(mut pictures) = pictures.lock() else { return out };
    let now = frame(ui.ctx());
    for a in c.attachments.iter().filter(|a| a.inline && !a.content_id.is_empty()) {
        let size = pictures.size(&a.data, now).map_or(vec2(120.0, 90.0), |[w, h]| vec2(w as f32, h as f32));
        let widest = room.clamp(40.0, PICTURE_W);
        let k = (widest / size.x.max(1.0)).min(PICTURE_H / size.y.max(1.0)).min(1.0);
        out.insert(a.content_id.clone(), vec2((size.x * k).round().max(8.0), (size.y * k).round().max(8.0)));
    }
    out
}

/// The editor of the body. `focus`: give it the keyboard, with the caret at the top.
pub fn body(app: &mut MailApp, ui: &mut Ui, c: &Compose, focus: bool) {
    let t = Tokens::get(ui.ctx());
    let id = ui.id().with("compose_body_edit");
    let mut styles = c.styles.clone();
    styles.normalize(&c.body);
    let room = ui.available_width();
    let look = Look {
        t,
        styles,
        html: c.html,
        misspelt: misspellings(ui, c),
        pictures: picture_sizes(ui, c, room),
        pixels_per_point: ui.ctx().pixels_per_point(),
    };
    let before = c.body.clone();
    let typing = c.typing.clone();
    let mut text = c.body.clone();
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| -> Arc<Galley> {
        let now = buf.as_str();
        if now == before {
            return layout(ui, now, &look, wrap);
        }
        // Typed this frame: the look follows the text as the engine will make it.
        let mut moved = look.clone();
        moved.styles.edit(&before, now, typing.as_ref());
        moved.misspelt = Arc::new(Vec::new());
        layout(ui, now, &moved, wrap)
    };
    let rows = ((ui.available_height() / 19.0) as usize).max(8);
    let output = egui::TextEdit::multiline(&mut text)
        .id(id)
        .frame(egui::Frame::NONE)
        .font(regular(BASE))
        .desired_width(f32::INFINITY)
        .desired_rows(rows)
        .hint_text(tl!("Write your message"))
        .layouter(&mut layouter)
        .show(ui);
    let resp = output.response.response.clone();
    if focus {
        resp.request_focus();
        // The caret goes to the top, above the quoted original.
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            state.store(ui.ctx(), id);
        }
    }
    decorate(ui, c, &look, &output.galley, output.galley_pos);
    if resp.changed() && text != c.body {
        let _ = app.execute("compose.set", json!({"body": text}));
    }
    // Formatting acts on what is selected: the engine is told when that changes.
    if let Some(range) = output.cursor_range
        && resp.has_focus()
    {
        let (a, b) = (range.secondary.index.0, range.primary.index.0);
        let now = (a.min(b), a.max(b));
        let known = app.session.compose.as_ref().and_then(|c| c.selection);
        if known != Some(now) {
            let _ = app.execute("compose.select", json!({"start": now.0, "end": now.1}));
        }
    }
    spelling_menu(app, ui, &resp, &output.galley, output.galley_pos, &look);
    // After a click on the formatting bar the keyboard comes back here.
    if ui.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("compose_refocus")).unwrap_or(false)) {
        ui.memory_mut(|m| m.request_focus(id));
    }
}

/// What the text editor doesn't draw: list markers, lines across and pictures.
fn decorate(ui: &Ui, c: &Compose, look: &Look, galley: &Galley, origin: egui::Pos2) {
    let t = &look.t;
    let painter = ui.painter();
    let paras = &look.styles.paras;
    // The numbers of numbered lists, by depth.
    let mut numbers: Vec<u32> = Vec::new();
    let mut para = 0usize;
    let mut first_row = true;
    let mut char_at = 0usize;
    let pictures: HashMap<&str, &Arc<Vec<u8>>> =
        c.attachments.iter().filter(|a| a.inline && !a.content_id.is_empty()).map(|a| (a.content_id.as_str(), &a.data)).collect();
    let mut picture_cache = Pictures::of(ui.ctx());
    for row in &galley.rows {
        let style = if look.html { paras.get(para).copied().unwrap_or_default() } else { ParaStyle::default() };
        let rect = row.rect().translate(origin.to_vec2());
        if first_row && look.html {
            let depth = usize::from(style.indent.min(rich::MAX_INDENT));
            let x = origin.x + f32::from(style.indent.min(rich::MAX_INDENT)) * INDENT_STEP;
            let label = match style.list {
                List::None => {
                    numbers.clear();
                    None
                }
                List::Bullet => {
                    numbers.truncate(depth);
                    Some(["•", "◦", "▪"][depth % 3].to_string())
                }
                List::Number => {
                    numbers.truncate(depth + 1);
                    numbers.resize(depth + 1, 0);
                    let n = numbers.last_mut().map_or(1, |n| {
                        *n = n.saturating_add(1);
                        *n
                    });
                    Some(format!("{n}."))
                }
            };
            if let Some(label) = label {
                let size = look.styles.style_of(char_at).map_or(BASE, size_of);
                let ink = Paper::window(t).text(t, false);
                painter.text(pos2(x + 2.0, rect.center().y), Align2::LEFT_CENTER, label, regular(size), ink);
            }
            if style.rule {
                let y = rect.center().y.round() + 0.5;
                painter.line_segment(
                    [pos2(x, y), pos2(origin.x + galley.rect.width().max(ui.available_width()) - 4.0, y)],
                    Stroke::new(1.0, t.border_strong),
                );
            }
        }
        // Pictures, where their mark is.
        for (k, glyph) in row.glyphs.iter().enumerate() {
            let index = char_at + k;
            if glyph.chr == PICTURE
                && let Some(cid) = look.styles.style_of(index).and_then(|s| s.picture.clone())
                && let Some(size) = look.pictures.get(&cid)
            {
                let right = rect.min.x + glyph.pos.x;
                let shown = Rect::from_min_size(pos2(right - size.x, rect.max.y - size.y), *size);
                let texture = pictures.get(cid.as_str()).and_then(|data| picture_cache.lock().ok().map(|mut p| p.texture(ui.ctx(), data)));
                match texture {
                    Some(Picture::Ready(texture)) => {
                        painter.image(texture.id(), shown, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    Some(Picture::Pending) => {
                        painter.rect_filled(shown, 2.0, t.panel);
                        ui.ctx().request_repaint();
                    }
                    _ => {
                        painter.rect_stroke(shown, 2.0, Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
                        icons::paint(painter, Rect::from_center_size(shown.center(), vec2(20.0, 20.0)), "picture", t.text_dim, t.accent);
                    }
                }
            }
        }
        char_at += row.char_count_excluding_newline().0;
        first_row = row.ends_with_newline;
        if row.ends_with_newline {
            char_at += 1;
            para += 1;
        }
    }
    let _ = &mut picture_cache;
}

/// A right click on a misspelt word offers the words it may have been meant as.
fn spelling_menu(app: &mut MailApp, ui: &Ui, resp: &egui::Response, galley: &Galley, origin: egui::Pos2, look: &Look) {
    let store = egui::Id::new("compose_spelling_word");
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let at = galley.cursor_from_pos(pos - origin).index.0;
        let hit = look.misspelt.iter().find(|r| r.start <= at && at <= r.end).cloned();
        ui.data_mut(|d| {
            if let Some(r) = hit {
                d.insert_temp(store, r);
            } else {
                d.remove_temp::<Range<usize>>(store);
            }
        });
    }
    let Some(word) = ui.data(|d| d.get_temp::<Range<usize>>(store)) else { return };
    let Some(c) = app.session.compose.clone() else { return };
    let text: String = c.body.chars().skip(word.start).take(word.end - word.start).collect();
    resp.context_menu(|ui| {
        let near = mailcraft_engine::spell::suggest(&text, 6);
        if near.is_empty() {
            ui.label(egui::RichText::new(tl!("No suggestions")).color(Tokens::get(ui.ctx()).text_dim));
        }
        for w in near {
            if ui.button(&w).clicked() {
                let _ = app.execute("compose.select", json!({"start": word.start, "end": word.end}));
                let _ = app.execute("edit.replace", json!({"text": text, "with": w, "matchCase": true}));
                ui.close();
            }
        }
    });
}

/// One button of the formatting bar; `on`: it shows what is selected has it.
fn bar_button(ui: &mut Ui, icon: &str, tip: &str, on: bool, enabled: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(28.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
    if on {
        ui.painter().rect_filled(r, 4.0, t.checked);
    } else if resp.hovered() && enabled {
        ui.painter().rect_filled(r, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    let c = if enabled { t.icon } else { t.text_disabled };
    icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), icon, c, if enabled { t.accent } else { c });
    resp.on_hover_text(tip)
}

fn separator(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(9.0, 20.0), Sense::hover());
    ui.painter().line_segment([pos2(r.center().x, r.min.y), pos2(r.center().x, r.max.y)], Stroke::new(1.0, t.border));
}

/// Run a formatting command, and give the keyboard back to the text.
fn act(app: &mut MailApp, ui: &Ui, id: &str, params: Value) {
    if let Err(e) = app.run(id, params) {
        app.status(e);
    }
    ui.data_mut(|d| d.insert_temp(egui::Id::new("compose_refocus"), true));
}

/// A grid of colour swatches; the one picked.
fn swatches(ui: &mut Ui, colors: &[(u32, &str)], none: &str) -> Option<Option<Rgb>> {
    let t = Tokens::get(ui.ctx());
    let mut picked = None;
    if ui.button(none).clicked() {
        picked = Some(None);
    }
    ui.horizontal_wrapped(|ui| {
        ui.set_max_width(5.0 * 26.0);
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (hex, name) in colors {
            let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
            ui.painter().rect_filled(r, 3.0, Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, *hex as u8));
            ui.painter().rect_stroke(r, 3.0, Stroke::new(if resp.hovered() { 2.0 } else { 1.0 }, t.border_strong), egui::StrokeKind::Inside);
            if resp.on_hover_text(tl!(*name)).clicked() {
                picked = Some(Some(rgb(*hex)));
            }
        }
    });
    picked
}

/// The formatting bar above the body: what the Basic Text group of the ribbon has, at hand.
pub fn format_bar(app: &mut MailApp, ui: &mut Ui, c: &Compose) {
    let t = Tokens::get(ui.ctx());
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let mut row =
        ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(12.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut row;
    ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
    if !c.html {
        ui.label(egui::RichText::new(tl!("Plain text")).font(regular(12.0)).color(t.text_dim));
        ui.add_space(8.0);
        if crate::widgets::icon_text_button(ui, "formatHtml", tl!("Switch to HTML"), true)
            .on_hover_text(tl!("Write with formatting and pictures"))
            .clicked()
        {
            act(app, ui, "compose.formatHtml", json!({}));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if bar_button(ui, "spelling", tl!("Spelling (F7)"), false, true).clicked() {
                act(app, ui, "ui.spelling", json!({}));
            }
        });
        return;
    }
    let st = mailcraft_engine::cmd::format::state(c);
    let tip = |label: &str, sc: String| if sc.is_empty() { label.to_string() } else { format!("{label} ({sc})") };
    for (icon, label, id, on) in [
        ("bold", tl!("Bold"), "format.bold", st.bold),
        ("italic", tl!("Italic"), "format.italic", st.italic),
        ("underline", tl!("Underline"), "format.underline", st.underline),
        ("strike", tl!("Strikethrough"), "format.strike", st.strike),
    ] {
        let tip = tip(label, crate::widgets::shortcut_text(app, id));
        if bar_button(ui, icon, &tip, on, true).clicked() {
            act(app, ui, id, json!({}));
        }
    }
    separator(ui);
    // The size, as a menu.
    let size_label = st.size.map_or_else(|| tl!("Normal").to_string(), |s| s.to_string());
    let size_btn = ui
        .add(egui::Button::new(egui::RichText::new(format!("{size_label} ▾")).font(regular(12.0))).min_size(vec2(64.0, 24.0)))
        .on_hover_text(tl!("Font Size"));
    egui::Popup::menu(&size_btn).show(|ui| {
        if ui.button(tl!("Larger")).clicked() {
            act(app, ui, "format.size", json!({"size": "larger"}));
        }
        if ui.button(tl!("Smaller")).clicked() {
            act(app, ui, "format.size", json!({"size": "smaller"}));
        }
        ui.separator();
        let mut sizes: Vec<Option<u8>> = rich::SIZES.iter().copied().map(Some).collect();
        sizes.insert(2, None);
        for size in sizes {
            let label = size.map_or_else(|| tl!("Normal").to_string(), |s| s.to_string());
            if ui.selectable_label(st.size == size, label).clicked() {
                act(app, ui, "format.size", json!({"size": size}));
            }
        }
    });
    let fixed = bar_button(ui, "fixedWidth", tl!("Fixed-width font"), st.fixed, true);
    if fixed.clicked() {
        act(app, ui, "format.font", json!({"face": if st.fixed { "default" } else { "fixed" }}));
    }
    let color = bar_button(ui, "fontColor", tl!("Font Color"), false, true);
    egui::Popup::menu(&color).show(|ui| {
        if let Some(picked) = swatches(ui, &FONT_COLORS, tl!("Automatic")) {
            act(app, ui, "format.color", json!({"color": picked.map(Rgb::hex)}));
            ui.close();
        }
    });
    let mark = bar_button(ui, "highlight", tl!("Text Highlight Color"), false, true);
    egui::Popup::menu(&mark).show(|ui| {
        if let Some(picked) = swatches(ui, &HIGHLIGHTS, tl!("No color")) {
            act(app, ui, "format.highlight", json!({"color": picked.map(Rgb::hex)}));
            ui.close();
        }
    });
    separator(ui);
    for (icon, label, id, on) in
        [("bullets", tl!("Bullets"), "format.bullets", st.bullets), ("numbering", tl!("Numbering"), "format.numbering", st.numbering)]
    {
        if bar_button(ui, icon, label, on, true).clicked() {
            act(app, ui, id, json!({}));
        }
    }
    if bar_button(ui, "outdent", tl!("Decrease Indent"), false, true).clicked() {
        act(app, ui, "format.outdent", json!({}));
    }
    if bar_button(ui, "indent", tl!("Increase Indent"), false, true).clicked() {
        act(app, ui, "format.indent", json!({}));
    }
    separator(ui);
    for (icon, label, value) in [
        ("alignLeft", tl!("Align Left"), Align::Left),
        ("alignCenter", tl!("Center"), Align::Center),
        ("alignRight", tl!("Align Right"), Align::Right),
    ] {
        if bar_button(ui, icon, label, st.align == value, true).clicked() {
            act(app, ui, "format.align", json!({"align": value}));
        }
    }
    separator(ui);
    let link = bar_button(ui, "hyperlink", tl!("Link"), st.link.is_some(), true);
    link_popup(app, &link, st.link.as_deref());
    if bar_button(ui, "picture", tl!("Pictures"), false, true).clicked() {
        act(app, ui, "ui.insertPicture", json!({}));
    }
    if bar_button(ui, "rule", tl!("Horizontal Line"), false, true).clicked() {
        act(app, ui, "insert.rule", json!({}));
    }
    if bar_button(ui, "clearFormatting", tl!("Clear All Formatting"), false, true).clicked() {
        act(app, ui, "format.clear", json!({}));
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if bar_button(ui, "formatPlain", tl!("Plain Text (the formatting goes)"), false, true).clicked() {
            act(app, ui, "compose.formatPlain", json!({}));
        }
        if bar_button(ui, "spelling", tl!("Spelling (F7)"), false, true).clicked() {
            act(app, ui, "ui.spelling", json!({}));
        }
    });
}

/// The address a link is given, asked for under the Link button.
fn link_popup(app: &mut MailApp, button: &egui::Response, current: Option<&str>) {
    let store = egui::Id::new("compose_link_address");
    if button.clicked() {
        let start = current.unwrap_or("https://").to_string();
        button.ctx.data_mut(|d| d.insert_temp(store, start));
    }
    egui::Popup::menu(button).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let mut address: String = ui.data(|d| d.get_temp(store)).unwrap_or_default();
        ui.label(egui::RichText::new(tl!("Address")).font(regular(12.0)));
        let edit = ui.add(egui::TextEdit::singleline(&mut address).desired_width(260.0).hint_text("https://example.org"));
        ui.data_mut(|d| d.insert_temp(store, address.clone()));
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            if ui.button(tl!("Insert")).clicked() || enter {
                act(app, ui, "compose.link", json!({"url": address.trim()}));
                ui.close();
            }
            if current.is_some() && ui.button(tl!("Remove Link")).clicked() {
                act(app, ui, "compose.link", json!({"url": null}));
                ui.close();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_are_found_in_order_and_out_of_it() {
        let text = "ab\ncd";
        let mut styles = Styles::plain(text);
        styles.apply(text, 3..5, |c| c.bold = true);
        let mut cursor = RunCursor::new(&styles);
        assert!(!cursor.style_at(0).bold);
        assert!(cursor.style_at(4).bold);
        assert!(!cursor.style_at(1).bold, "asking for an earlier one starts again");
        assert!(!cursor.style_at(99).bold);
    }

    #[test]
    fn paragraphs_move_in_for_lists_and_indents() {
        assert_eq!(inset(&ParaStyle::default()), 0.0);
        assert_eq!(inset(&ParaStyle { list: List::Bullet, ..ParaStyle::default() }), MARKER_W);
        assert_eq!(inset(&ParaStyle { indent: 2, ..ParaStyle::default() }), 2.0 * INDENT_STEP);
        assert_eq!(inset(&ParaStyle { indent: 200, ..ParaStyle::default() }), f32::from(rich::MAX_INDENT) * INDENT_STEP);
    }
}
