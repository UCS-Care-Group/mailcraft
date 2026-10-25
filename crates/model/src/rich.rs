// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The text of a message being written, in rich text: paragraphs of runs.
//!
//! The set is deliberately small (the roadmap's minimal rich-text reply): bold, italic,
//! underline, one font size, a text colour, a font from a short list ([`Font`]), links to `http`,
//! `https`, `mailto` and `tel` addresses, and paragraphs that are plain, bulleted or numbered.
//! Nothing else can be expressed, so nothing else can be sent: pasted formatting is reduced to
//! this set before it gets here (`mailcraft_mime::rich`), and the HTML that goes out is written
//! from it.
//!
//! A picture is a run of its own holding one [`OBJECT`] character and the picture's id
//! ([`Run::picture`]); the picture itself (its bytes, width and description) is kept beside the
//! text by whatever holds it (a signature, R4.5). Typed or pasted text never holds the object
//! character, so only that holder can put a picture in.
//!
//! A position is a count of characters into [`RichText::text`]: the paragraphs' text joined by
//! one `\n` each. That is also the text a plain editor shows, so a position means the same in
//! both. Every edit takes any positions and any text: positions past the end are an error, text
//! loses its control characters, and sizes are capped ([`MAX_CHARS`], [`MAX_PARAGRAPHS`]).
//!
//! [`RichBody`] adds what writing needs on top: the selection, the formatting the next typed
//! character takes, and undo inside the message.

use serde::{Deserialize, Serialize};

use crate::ModelError;

mod body;

pub use body::{MAX_UNDO, RichBody, Selection};

/// Most characters one body holds (counting one for each paragraph break).
pub const MAX_CHARS: usize = 1 << 20;

/// Most paragraphs one body holds.
pub const MAX_PARAGRAPHS: usize = 50_000;

/// The smallest font size, in points.
pub const MIN_SIZE: u8 = 8;

/// The largest font size, in points.
pub const MAX_SIZE: u8 = 36;

/// Longest link address, in bytes.
pub const MAX_LINK: usize = 2048;

/// The deepest a list item may be indented: levels 0 (the outermost) to this, eight in all.
pub const MAX_LEVEL: u8 = 7;

/// The character a picture stands as in the text (U+FFFC OBJECT REPLACEMENT CHARACTER).
pub const OBJECT: char = '\u{fffc}';

/// A font the composer offers: widely installed families, written as their names with fallbacks.
/// No font is shipped with MailCraft.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Font {
    Arial,
    Calibri,
    Georgia,
    Verdana,
    /// The reader's own interface font (Segoe UI on Windows, the system's elsewhere).
    System,
}

impl Font {
    pub const ALL: [Font; 5] = [Font::Arial, Font::Calibri, Font::Georgia, Font::Verdana, Font::System];

    /// The name a menu shows.
    pub fn label(self) -> &'static str {
        match self {
            Font::Arial => "Arial",
            Font::Calibri => "Calibri",
            Font::Georgia => "Georgia",
            Font::Verdana => "Verdana",
            Font::System => "System",
        }
    }

    /// The CSS `font-family` it is written as.
    pub fn css(self) -> &'static str {
        match self {
            Font::Arial => "Arial, Helvetica, sans-serif",
            Font::Calibri => "Calibri, Carlito, sans-serif",
            Font::Georgia => "Georgia, serif",
            Font::Verdana => "Verdana, Geneva, sans-serif",
            Font::System => "'Segoe UI', system-ui, sans-serif",
        }
    }

    /// The font a CSS `font-family` list asks for first, when it is one of these.
    pub fn from_css(families: &str) -> Option<Font> {
        let first = families.split(',').next()?.trim().trim_matches(['\'', '"']).to_ascii_lowercase();
        Some(match first.as_str() {
            "arial" | "helvetica" => Font::Arial,
            "calibri" | "carlito" => Font::Calibri,
            "georgia" => Font::Georgia,
            "verdana" | "geneva" => Font::Verdana,
            "segoe ui" | "system-ui" | "-apple-system" => Font::System,
            _ => return None,
        })
    }

    /// From a menu's name or a command's (`"arial"`, `"Calibri"`).
    pub fn parse(name: &str) -> Option<Font> {
        Font::ALL.into_iter().find(|f| f.label().eq_ignore_ascii_case(name.trim()))
    }
}

/// A text colour as `#rrggbb` (lower case), from `#rgb`, `#rrggbb` or `rrggbb`; `None` otherwise.
pub fn safe_color(value: &str) -> Option<String> {
    let hex = value.trim().trim_start_matches('#');
    let full: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        _ => return None,
    };
    full.chars().all(|c| c.is_ascii_hexdigit()).then(|| format!("#{}", full.to_ascii_lowercase()))
}

/// What a paragraph is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ListKind {
    /// An ordinary paragraph.
    #[default]
    None,
    /// An item of a bulleted list.
    Bullet,
    /// An item of a numbered list; the number is its place among the numbered items of its level
    /// right above it (see [`numbers`]).
    Numbered,
}

/// How a run of text looks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Style {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    /// Font size in points, [`MIN_SIZE`] to [`MAX_SIZE`]; `None` for the message's own size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u8>,
    /// Where the text leads: always an address [`safe_link`] accepts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Text colour, `#rrggbb` ([`safe_color`]); `None` for the message's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Font; `None` for the message's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<Font>,
}

impl Style {
    /// The same style with its size clamped and an unsafe link dropped.
    pub fn clean(mut self) -> Style {
        self.size = self.size.map(clamp_size);
        self.link = self.link.as_deref().and_then(safe_link);
        self.color = self.color.as_deref().and_then(safe_color);
        self
    }

    /// No formatting at all.
    pub fn is_plain(&self) -> bool {
        *self == Style::default()
    }
}

/// A size in points, within [`MIN_SIZE`]..=[`MAX_SIZE`].
pub fn clamp_size(points: u8) -> u8 {
    points.clamp(MIN_SIZE, MAX_SIZE)
}

/// A link address as it may be kept and sent: `http`, `https`, `mailto` and `tel` only (the
/// scheme in any letter case), at most [`MAX_LINK`] bytes, nothing invisible or blank inside.
/// `http` and `https` need a host. Spaces around it are dropped; anything else is `None`.
pub fn safe_link(url: &str) -> Option<String> {
    let url = url.trim_matches(' ');
    if url.is_empty() || url.len() > MAX_LINK || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (scheme, rest) = url.split_once(':')?;
    let web = ["http", "https"].iter().any(|s| scheme.eq_ignore_ascii_case(s));
    let other = ["mailto", "tel"].iter().any(|s| scheme.eq_ignore_ascii_case(s));
    let ok = if web {
        rest.strip_prefix("//").is_some_and(|host| !host.is_empty() && !host.starts_with('/'))
            && url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty()))
    } else {
        other && !rest.is_empty()
    };
    ok.then(|| url.to_string())
}

/// Text as a body may hold it: no control characters but the tab, no line breaks (they split
/// paragraphs, see [`lines`]), and no [`OBJECT`] (only a picture run holds one).
fn clean_line(s: &str) -> String {
    s.chars().filter(|c| (*c == '\t' || !c.is_control()) && *c != OBJECT).collect()
}

/// The lines of `text`, whatever ends them (`\r\n`, `\r`, `\n`), each cleaned.
fn lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n").split(['\n', '\r']).map(clean_line).collect()
}

/// Text that looks one way, or one picture.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Run {
    pub text: String,
    #[serde(flatten)]
    pub style: Style,
    /// A picture's id, kept by the text's holder: the run's text is then one [`OBJECT`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<u32>,
}

impl Run {
    pub fn new(text: impl Into<String>, style: Style) -> Run {
        Run { text: text.into(), style, picture: None }
    }

    /// The picture `id`, as it stands in the text.
    pub fn picture(id: u32, style: Style) -> Run {
        Run { text: OBJECT.to_string(), style, picture: Some(id) }
    }

    pub fn plain(text: impl Into<String>) -> Run {
        Run::new(text, Style::default())
    }

    fn chars(&self) -> usize {
        self.text.chars().count()
    }
}

/// One paragraph: its runs, whether it is a list item, and how deep in its list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Paragraph {
    pub runs: Vec<Run>,
    #[serde(skip_serializing_if = "is_none_list")]
    pub list: ListKind,
    /// How deep a list item is indented, 0 to [`MAX_LEVEL`]; always 0 for an ordinary paragraph.
    /// (Stored before levels existed, it is missing, and 0.)
    #[serde(skip_serializing_if = "is_zero")]
    pub level: u8,
    /// Part of the signature put in a message (R4.5): what choosing another replaces, and what is
    /// swapped when the From account changes.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub signature: bool,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

fn is_none_list(l: &ListKind) -> bool {
    *l == ListKind::None
}

impl Paragraph {
    pub fn new(runs: Vec<Run>, list: ListKind) -> Paragraph {
        let mut p = Paragraph { runs, list, level: 0, signature: false };
        p.normalize();
        p
    }

    /// The same paragraph at a level of its list (kept within 0 to [`MAX_LEVEL`]; an ordinary
    /// paragraph has none).
    pub fn at_level(mut self, level: u8) -> Paragraph {
        self.level = if self.list == ListKind::None { 0 } else { level.min(MAX_LEVEL) };
        self
    }

    /// The paragraph's text without formatting.
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    pub fn chars(&self) -> usize {
        self.runs.iter().map(Run::chars).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|r| r.text.is_empty())
    }

    /// Drop empty runs and join neighbours that look the same.
    fn normalize(&mut self) {
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for run in std::mem::take(&mut self.runs) {
            if run.text.is_empty() {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.style == run.style && last.picture.is_none() && run.picture.is_none() => last.text.push_str(&run.text),
                _ => out.push(run),
            }
        }
        self.runs = out;
    }

    /// Make a run start at character `offset` (clamped to the paragraph); the index of that run
    /// (`runs.len()` at the end).
    fn split_at(&mut self, offset: usize) -> usize {
        let mut seen = 0;
        for i in 0..self.runs.len() {
            let Some(run) = self.runs.get_mut(i) else { break };
            let n = run.chars();
            if offset <= seen {
                return i;
            }
            if offset < seen + n {
                let byte = run.text.char_indices().nth(offset - seen).map_or(run.text.len(), |(b, _)| b);
                let tail = run.text.split_off(byte);
                let style = run.style.clone();
                self.runs.insert(i + 1, Run::new(tail, style));
                return i + 1;
            }
            seen += n;
        }
        self.runs.len()
    }

    /// Cut the paragraph at `offset`: what follows becomes a paragraph of the same kind.
    fn split_off(&mut self, offset: usize) -> Paragraph {
        let i = self.split_at(offset);
        let tail = if i < self.runs.len() { self.runs.split_off(i) } else { Vec::new() };
        Paragraph { runs: tail, list: self.list, level: self.level, signature: self.signature }
    }

    /// The style of the character before `offset`, or of the first character at the start.
    fn style_at(&self, offset: usize) -> Style {
        if offset == 0 {
            return self.runs.first().map(|r| r.style.clone()).unwrap_or_default();
        }
        let mut seen = 0;
        for run in &self.runs {
            let n = run.chars();
            if offset > seen && offset <= seen + n {
                return run.style.clone();
            }
            seen += n;
        }
        self.runs.last().map(|r| r.style.clone()).unwrap_or_default()
    }

    /// Apply `f` to the runs between two offsets of this paragraph.
    fn restyle(&mut self, from: usize, to: usize, f: &impl Fn(&mut Style)) {
        if from >= to {
            return;
        }
        let a = self.split_at(from);
        let b = self.split_at(to);
        for run in self.runs.iter_mut().take(b).skip(a) {
            f(&mut run.style);
        }
        self.normalize();
    }

    /// Whether every run between two offsets passes `f` (an empty span: the style at `from`).
    fn all(&self, from: usize, to: usize, f: &impl Fn(&Style) -> bool) -> bool {
        if from >= to {
            return true;
        }
        let mut seen = 0;
        for run in &self.runs {
            let n = run.chars();
            let (start, end) = (seen, seen + n);
            if end > from && start < to && !f(&run.style) {
                return false;
            }
            seen = end;
        }
        true
    }
}

/// A change of formatting over a span of text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Bold(bool),
    Italic(bool),
    Underline(bool),
    /// Points, clamped to [`MIN_SIZE`]..=[`MAX_SIZE`]; `None` for the message's own size.
    Size(Option<u8>),
    /// An address [`safe_link`] accepts, or `None` to remove the link.
    Link(Option<String>),
    /// A `#rrggbb` colour ([`safe_color`]), or `None` for the message's own.
    Color(Option<String>),
    /// A font, or `None` for the message's own.
    Font(Option<Font>),
    /// Back to plain text, links and all.
    Clear,
}

impl Format {
    fn apply(&self, style: &mut Style) {
        match self {
            Format::Bold(on) => style.bold = *on,
            Format::Italic(on) => style.italic = *on,
            Format::Underline(on) => style.underline = *on,
            Format::Size(size) => style.size = size.map(clamp_size),
            Format::Link(url) => style.link = url.as_deref().and_then(safe_link),
            Format::Color(color) => style.color = color.as_deref().and_then(safe_color),
            Format::Font(font) => style.font = *font,
            Format::Clear => *style = Style::default(),
        }
    }
}

/// A rich-text document: at least one paragraph, runs never empty, neighbouring runs never
/// alike, no control characters but the tab. Deserializing cleans whatever comes in.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "RawText")]
pub struct RichText {
    paragraphs: Vec<Paragraph>,
}

/// A document as it was stored or sent, before it is cleaned.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawText {
    paragraphs: Vec<Paragraph>,
}

impl From<RawText> for RichText {
    fn from(raw: RawText) -> RichText {
        RichText::from_paragraphs(raw.paragraphs)
    }
}

impl Default for RichText {
    fn default() -> Self {
        RichText::new()
    }
}

impl RichText {
    /// One empty paragraph.
    pub fn new() -> RichText {
        RichText { paragraphs: vec![Paragraph::default()] }
    }

    /// Plain text, one paragraph per line.
    pub fn from_plain(text: &str) -> RichText {
        let paragraphs = lines(text).into_iter().map(|l| Paragraph::new(vec![Run::plain(l)], ListKind::None)).collect();
        RichText::from_paragraphs(paragraphs)
    }

    /// Paragraphs from anywhere, cleaned: line breaks inside a run start a new paragraph of the
    /// same kind, styles are cleaned, and what is over the caps is dropped.
    pub fn from_paragraphs(paragraphs: Vec<Paragraph>) -> RichText {
        let mut out: Vec<Paragraph> = Vec::new();
        let mut budget = MAX_CHARS;
        'all: for p in paragraphs {
            let level = if p.list == ListKind::None { 0 } else { p.level.min(MAX_LEVEL) };
            let mut cur = Paragraph { runs: Vec::new(), list: p.list, level, signature: p.signature };
            for run in p.runs {
                let style = run.style.clean();
                if let Some(id) = run.picture {
                    if budget == 0 {
                        break 'all;
                    }
                    budget -= 1;
                    cur.runs.push(Run::picture(id, style));
                    continue;
                }
                for (i, line) in lines(&run.text).into_iter().enumerate() {
                    if i > 0 {
                        cur.normalize();
                        out.push(std::mem::replace(&mut cur, Paragraph { runs: Vec::new(), list: p.list, level, signature: p.signature }));
                        budget = budget.saturating_sub(1);
                        if out.len() >= MAX_PARAGRAPHS || budget == 0 {
                            break 'all;
                        }
                    }
                    let n = line.chars().count();
                    let line = if n > budget { line.chars().take(budget).collect() } else { line };
                    budget = budget.saturating_sub(n.min(budget));
                    cur.runs.push(Run::new(line, style.clone()));
                }
            }
            cur.normalize();
            out.push(cur);
            budget = budget.saturating_sub(1);
            if out.len() >= MAX_PARAGRAPHS || budget == 0 {
                break;
            }
        }
        if out.is_empty() {
            out.push(Paragraph::default());
        }
        RichText { paragraphs: out }
    }

    pub fn paragraphs(&self) -> &[Paragraph] {
        &self.paragraphs
    }

    /// The text without formatting: the paragraphs joined by `\n`. Positions count its characters.
    pub fn text(&self) -> String {
        self.paragraphs.iter().map(Paragraph::text).collect::<Vec<_>>().join("\n")
    }

    /// Characters in [`RichText::text`].
    pub fn len(&self) -> usize {
        self.paragraphs.iter().map(Paragraph::chars).sum::<usize>() + self.paragraphs.len().saturating_sub(1)
    }

    /// No text at all (one empty paragraph).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether anything in it is more than plain text.
    pub fn has_formatting(&self) -> bool {
        self.paragraphs.iter().any(|p| p.list != ListKind::None || p.runs.iter().any(|r| !r.style.is_plain() || r.picture.is_some()))
    }

    /// The pictures' ids, in the order they stand.
    pub fn pictures(&self) -> Vec<u32> {
        self.paragraphs.iter().flat_map(|p| &p.runs).filter_map(|r| r.picture).collect()
    }

    /// The text as it goes in a plain-text part: bullets as `- `, numbers as `1. `, and a link's
    /// address after its text when the text doesn't say it. Pictures are left out.
    pub fn to_plain(&self) -> String {
        self.to_plain_with(|_| None)
    }

    /// [`RichText::to_plain`], with a picture as its description in brackets when `describe` has
    /// one for it (`[Lumen Studio logo]`), else left out.
    pub fn to_plain_with(&self, describe: impl Fn(u32) -> Option<String>) -> String {
        let mut out = String::new();
        let numbers = numbers(&self.paragraphs);
        for (i, p) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if p.list != ListKind::None {
                for _ in 0..p.level {
                    out.push_str("   ");
                }
            }
            match p.list {
                ListKind::None => {}
                ListKind::Bullet => out.push_str("- "),
                ListKind::Numbered => out.push_str(&format!("{}. ", numbers.get(i).copied().unwrap_or(1))),
            }
            for run in &p.runs {
                if let Some(id) = run.picture {
                    if let Some(text) = describe(id).map(|t| clean_line(&t)).filter(|t| !t.trim().is_empty()) {
                        out.push_str(&format!("[{}]", text.trim()));
                    }
                    continue;
                }
                out.push_str(&run.text);
                if let Some(url) = &run.style.link {
                    let shown = run.text.trim();
                    let bare = url.split_once(':').map_or(url.as_str(), |(_, r)| r.trim_start_matches('/'));
                    // A telephone number says its link however it is spaced (`+44 (0)20…`).
                    let digits = |t: &str| t.replace("(0)", "").chars().filter(char::is_ascii_digit).collect::<String>();
                    let number = url.to_ascii_lowercase().starts_with("tel:") && digits(shown) == digits(bare);
                    if !number && !shown.eq_ignore_ascii_case(url) && !shown.eq_ignore_ascii_case(bare) {
                        out.push_str(&format!(" <{url}>"));
                    }
                }
            }
        }
        out
    }

    /// The first and last paragraph of the signature put in it.
    fn signature_paragraphs(&self) -> Option<(usize, usize)> {
        let first = self.paragraphs.iter().position(|p| p.signature)?;
        let last = self.paragraphs.iter().rposition(|p| p.signature)?;
        Some((first, last))
    }

    /// The position the signature put in it starts at.
    pub fn signature_start(&self) -> Option<usize> {
        let (a, _) = self.signature_paragraphs()?;
        Some(self.paragraphs.iter().take(a).map(|p| p.chars() + 1).sum())
    }

    /// The signature put in it, as a document of its own; `None` when there is none.
    pub fn signature(&self) -> Option<RichText> {
        let (a, b) = self.signature_paragraphs()?;
        let paragraphs = self.paragraphs.get(a..=b)?.iter().map(|p| Paragraph { signature: false, ..p.clone() }).collect();
        Some(RichText::from_paragraphs(paragraphs))
    }

    /// The text before the signature, the signature, and the text after it, when it has one.
    pub fn around_signature(&self) -> Option<(Option<RichText>, RichText, Option<RichText>)> {
        let (a, b) = self.signature_paragraphs()?;
        let part = |range: &[Paragraph]| (!range.is_empty()).then(|| RichText::from_paragraphs(range.to_vec()));
        Some((self.paragraphs.get(..a).and_then(part), self.signature()?, self.paragraphs.get(b + 1..).and_then(part)))
    }

    /// Put `sig` in as the signature: in place of the one there, else as paragraphs of their own
    /// at `at` (the paragraph there is cut; an empty rest of the last paragraph is left out).
    pub fn put_signature(&mut self, sig: &RichText, at: usize) -> Result<(), ModelError> {
        let marked: Vec<Paragraph> = sig.paragraphs.iter().map(|p| Paragraph { signature: true, ..p.clone() }).collect();
        if self.len().saturating_add(sig.len()) > MAX_CHARS || self.paragraphs.len().saturating_add(marked.len()) > MAX_PARAGRAPHS {
            return Err(ModelError::Invalid("the message can't hold that much text".into()));
        }
        if let Some((a, b)) = self.signature_paragraphs() {
            self.paragraphs.splice(a..=b, marked);
            return Ok(());
        }
        let (pi, off) = self.locate(at)?;
        let last = pi + 1 == self.paragraphs.len();
        let Some(target) = self.paragraphs.get_mut(pi) else { return Err(ModelError::Invalid("no such paragraph".into())) };
        let tail = target.split_off(off);
        let mut new = marked;
        if !(tail.is_empty() && last) {
            new.push(Paragraph { signature: false, ..tail });
        }
        self.paragraphs.splice(pi + 1..pi + 1, new);
        Ok(())
    }

    /// Take the signature out, if there is one.
    pub fn remove_signature(&mut self) {
        if let Some((a, b)) = self.signature_paragraphs() {
            self.paragraphs.drain(a..=b);
            if self.paragraphs.is_empty() {
                self.paragraphs.push(Paragraph::default());
            }
        }
    }

    /// The level of the list item `pos` is in (0 outside a list).
    pub fn level_at(&self, pos: usize) -> u8 {
        self.locate(pos).ok().and_then(|(p, _)| self.paragraphs.get(p)).map_or(0, |p| p.level)
    }

    /// Paragraph and offset of a position.
    fn locate(&self, pos: usize) -> Result<(usize, usize), ModelError> {
        let mut rest = pos;
        for (i, p) in self.paragraphs.iter().enumerate() {
            let n = p.chars();
            if rest <= n {
                return Ok((i, rest));
            }
            rest -= n + 1;
        }
        Err(ModelError::Invalid(format!("position {pos} is past the end of the text ({} characters)", self.len())))
    }

    /// `from` and `to` in order, both checked.
    fn span(&self, from: usize, to: usize) -> Result<(usize, usize), ModelError> {
        let (a, b) = (from.min(to), from.max(to));
        self.locate(b)?;
        Ok((a, b))
    }

    /// The style of the character before `pos` (at a paragraph's start, of its first one).
    pub fn style_at(&self, pos: usize) -> Style {
        match self.locate(pos) {
            Ok((p, o)) => self.paragraphs.get(p).map(|p| p.style_at(o)).unwrap_or_default(),
            Err(_) => Style::default(),
        }
    }

    /// The kind of the paragraph `pos` is in.
    pub fn list_at(&self, pos: usize) -> ListKind {
        self.locate(pos).ok().and_then(|(p, _)| self.paragraphs.get(p)).map_or(ListKind::None, |p| p.list)
    }

    /// Whether the paragraph `pos` is in has no text (false past the end).
    pub fn paragraph_is_empty(&self, pos: usize) -> bool {
        self.locate(pos).ok().and_then(|(p, _)| self.paragraphs.get(p)).is_some_and(Paragraph::is_empty)
    }

    /// Where the paragraph `pos` is in starts and ends, as `[from, to)` (the break after it not
    /// included); `None` past the end.
    pub fn paragraph_span(&self, pos: usize) -> Option<(usize, usize)> {
        let (p, off) = self.locate(pos).ok()?;
        let n = self.paragraphs.get(p)?.chars();
        Some((pos - off, pos - off + n))
    }

    /// The whole link the position is in or at an end of, as `[from, to)`.
    pub fn link_span(&self, pos: usize) -> Option<(usize, usize)> {
        let (pi, off) = self.locate(pos).ok()?;
        let p = self.paragraphs.get(pi)?;
        let base = pos - off;
        let mut bounds = Vec::with_capacity(p.runs.len());
        let mut seen = 0;
        for run in &p.runs {
            let n = run.chars();
            bounds.push((seen, seen + n, run.style.link.as_deref()));
            seen += n;
        }
        let i = bounds
            .iter()
            .position(|(a, b, l)| l.is_some() && *a < off && off <= *b)
            .or_else(|| bounds.iter().position(|(a, b, l)| l.is_some() && *a <= off && off < *b))?;
        let link = bounds.get(i)?.2;
        let mut first = i;
        while first > 0 && bounds.get(first - 1).is_some_and(|b| b.2 == link) {
            first -= 1;
        }
        let mut last = i;
        while bounds.get(last + 1).is_some_and(|b| b.2 == link) {
            last += 1;
        }
        Some((base + bounds.get(first)?.0, base + bounds.get(last)?.1))
    }

    /// Each paragraph `[from, to)` touches (at least the one `from` is in), with the offsets
    /// inside it.
    fn pieces(&self, from: usize, to: usize) -> Result<Vec<(usize, usize, usize)>, ModelError> {
        let (from, to) = self.span(from, to)?;
        let (pa, oa) = self.locate(from)?;
        let (pb, ob) = self.locate(to)?;
        let mut out = Vec::new();
        for i in pa..=pb {
            let n = self.paragraphs.get(i).map_or(0, Paragraph::chars);
            let start = if i == pa { oa } else { 0 };
            let end = if i == pb { ob } else { n };
            out.push((i, start, end));
        }
        Ok(out)
    }

    /// Whether all the text in `[from, to)` passes `f`; for an empty span, the style at `from`.
    pub fn all(&self, from: usize, to: usize, f: impl Fn(&Style) -> bool) -> Result<bool, ModelError> {
        let (from, to) = self.span(from, to)?;
        if from == to {
            return Ok(f(&self.style_at(from)));
        }
        let pieces = self.pieces(from, to)?;
        let any_text = pieces.iter().any(|(_, a, b)| a < b);
        if !any_text {
            return Ok(f(&self.style_at(from)));
        }
        Ok(pieces.iter().all(|(i, a, b)| self.paragraphs.get(*i).is_none_or(|p| p.all(*a, *b, &f))))
    }

    /// Whether every paragraph `[from, to)` touches is of `kind`.
    pub fn all_lists(&self, from: usize, to: usize, kind: ListKind) -> Result<bool, ModelError> {
        Ok(self.pieces(from, to)?.iter().all(|(i, _, _)| self.paragraphs.get(*i).is_some_and(|p| p.list == kind)))
    }

    /// A copy of `[from, to)`, formatting and all.
    pub fn slice(&self, from: usize, to: usize) -> Result<RichText, ModelError> {
        let pieces = self.pieces(from, to)?;
        let mut out = Vec::new();
        for (i, a, b) in pieces {
            let Some(p) = self.paragraphs.get(i) else { continue };
            let mut p = p.clone();
            let mut mid = p.split_off(a);
            mid.split_off(b - a);
            out.push(mid);
        }
        Ok(RichText::from_paragraphs(out))
    }

    // ---- edits ----

    /// Put `text` at `at` in `style`; line breaks in it start paragraphs of the kind of the one
    /// it goes into. The position after it.
    pub fn insert(&mut self, at: usize, text: &str, style: &Style) -> Result<usize, ModelError> {
        let (p, _) = self.locate(at)?;
        let (list, level) = self.paragraphs.get(p).map_or((ListKind::None, 0), |p| (p.list, p.level));
        let style = style.clone().clean();
        let fragment = RichText::from_paragraphs(
            lines(text).into_iter().map(|l| Paragraph::new(vec![Run::new(l, style.clone())], list).at_level(level)).collect(),
        );
        self.insert_rich(at, &fragment)
    }

    /// Put another document's paragraphs at `at` (pasting). Its first paragraph joins the one at
    /// `at` (and gives it its kind when it goes in at that paragraph's start and is a list item);
    /// its last is followed by the rest of that one. The position after it.
    pub fn insert_rich(&mut self, at: usize, fragment: &RichText) -> Result<usize, ModelError> {
        let (pi, off) = self.locate(at)?;
        let added = fragment.len();
        if self.len().saturating_add(added) > MAX_CHARS
            || self.paragraphs.len().saturating_add(fragment.paragraphs.len().saturating_sub(1)) > MAX_PARAGRAPHS
        {
            return Err(ModelError::Invalid("the message can't hold that much text".into()));
        }
        let Some(target) = self.paragraphs.get_mut(pi) else { return Err(ModelError::Invalid("no such paragraph".into())) };
        let mut tail = target.split_off(off);
        let mut parts = fragment.paragraphs.iter().cloned();
        let Some(first) = parts.next() else { return Ok(at) };
        if off == 0 && first.list != ListKind::None {
            target.list = first.list;
            target.level = first.level;
        }
        target.runs.extend(first.runs);
        let rest: Vec<Paragraph> = parts.collect();
        match rest.split_last() {
            None => {
                target.runs.append(&mut tail.runs);
                target.normalize();
            }
            Some((last, middle)) => {
                target.normalize();
                let mut last = last.clone();
                last.runs.append(&mut tail.runs);
                last.normalize();
                let at_index = pi + 1;
                let mut new: Vec<Paragraph> = middle.to_vec();
                new.push(last);
                self.paragraphs.splice(at_index..at_index, new);
            }
        }
        Ok(at + added)
    }

    /// Remove `[from, to)`; a paragraph break inside it joins the paragraphs on either side
    /// (the first one's kind stays).
    pub fn delete(&mut self, from: usize, to: usize) -> Result<(), ModelError> {
        let (from, to) = self.span(from, to)?;
        if from == to {
            return Ok(());
        }
        let (pa, oa) = self.locate(from)?;
        let (pb, ob) = self.locate(to)?;
        if pa == pb {
            if let Some(p) = self.paragraphs.get_mut(pa) {
                let a = p.split_at(oa);
                let b = p.split_at(ob);
                if a < b {
                    p.runs.drain(a..b);
                }
                p.normalize();
            }
            return Ok(());
        }
        let tail = match self.paragraphs.get_mut(pb) {
            Some(p) => p.split_off(ob),
            None => return Ok(()),
        };
        if let Some(p) = self.paragraphs.get_mut(pa) {
            p.split_off(oa);
            p.runs.extend(tail.runs);
            p.normalize();
        }
        self.paragraphs.drain(pa + 1..=pb.min(self.paragraphs.len().saturating_sub(1)));
        Ok(())
    }

    /// Change how `[from, to)` looks. A link must be to an address [`safe_link`] accepts.
    pub fn format(&mut self, from: usize, to: usize, change: &Format) -> Result<(), ModelError> {
        if let Format::Link(Some(url)) = change
            && safe_link(url).is_none()
        {
            return Err(ModelError::Invalid("a link goes to an http, https, mailto or tel address".into()));
        }
        if let Format::Color(Some(color)) = change
            && safe_color(color).is_none()
        {
            return Err(ModelError::Invalid("a colour is written #rrggbb".into()));
        }
        for (i, a, b) in self.pieces(from, to)? {
            if let Some(p) = self.paragraphs.get_mut(i) {
                p.restyle(a, b, &|s: &mut Style| change.apply(s));
            }
        }
        Ok(())
    }

    /// Make every paragraph `[from, to)` touches a paragraph of `kind`.
    pub fn set_list(&mut self, from: usize, to: usize, kind: ListKind) -> Result<(), ModelError> {
        for (i, _, _) in self.pieces(from, to)? {
            if let Some(p) = self.paragraphs.get_mut(i) {
                p.list = kind;
                if kind == ListKind::None {
                    p.level = 0;
                }
            }
        }
        Ok(())
    }

    /// Move the list items `[from, to)` touches `by` levels deeper (or shallower when negative),
    /// within 0 to [`MAX_LEVEL`]. Ordinary paragraphs stay as they are. Whether anything moved.
    pub fn indent(&mut self, from: usize, to: usize, by: i8) -> Result<bool, ModelError> {
        let mut moved = false;
        for (i, _, _) in self.pieces(from, to)? {
            if let Some(p) = self.paragraphs.get_mut(i).filter(|p| p.list != ListKind::None) {
                let level = p.level.saturating_add_signed(by).min(MAX_LEVEL);
                moved |= level != p.level;
                p.level = level;
            }
        }
        Ok(moved)
    }

    /// Make the text `text` with as small an edit as there is: the characters both share at the
    /// start and at the end keep their formatting, and what is typed in between takes the style
    /// of the character before it. What a plain editor's change becomes.
    pub fn set_text(&mut self, text: &str) -> Result<(), ModelError> {
        let new: Vec<char> = lines(text).join("\n").chars().collect();
        let old: Vec<char> = self.text().chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let max_suffix = old.len().min(new.len()) - prefix;
        let suffix = old.iter().rev().zip(new.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
        let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
        let typed: String = new.get(prefix..new_end).map(|s| s.iter().collect()).unwrap_or_default();
        let mut style = self.style_at(prefix);
        style.link = None;
        let mut next = self.clone();
        next.delete(prefix, old_end)?;
        next.insert(prefix, &typed, &style)?;
        *self = next;
        Ok(())
    }
}

/// The number of each paragraph that is an item of a numbered list (0 for the others), as the
/// window, the plain-text part and the HTML part (`<ol>` in `<ol>`) all number them: an item
/// counts on from the item before it at its level, and a list starts again at 1 after anything
/// that ends it there: an ordinary paragraph, an item of the other kind at that level, or an item
/// at a shallower level.
pub fn numbers(paragraphs: &[Paragraph]) -> Vec<u32> {
    // Per level: the kind of the list open there, and how far it has counted.
    let mut open: Vec<(ListKind, u32)> = Vec::new();
    paragraphs
        .iter()
        .map(|p| {
            if p.list == ListKind::None {
                open.clear();
                return 0;
            }
            let level = usize::from(p.level.min(MAX_LEVEL));
            open.truncate(level + 1);
            while open.len() <= level {
                open.push((ListKind::None, 0));
            }
            let Some(here) = open.get_mut(level) else { return 0 };
            if here.0 != p.list {
                *here = (p.list, 0);
            }
            here.1 = here.1.saturating_add(1);
            if p.list == ListKind::Numbered { here.1 } else { 0 }
        })
        .collect()
}

#[cfg(test)]
mod tests;
