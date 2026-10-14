//! Formatted text being written: a message body in HTML form.
//!
//! The text itself stays plain (`Compose::body`): paragraphs are its lines, and a picture is one
//! [`PICTURE`] character. [`Styles`] says how each stretch of it looks (runs of characters with a
//! [`CharStyle`]) and how each paragraph sits (one [`ParaStyle`] per line). Positions are counted
//! in characters, as a text editor's cursor is.
//!
//! Everything here takes any input and never panics: [`Styles::normalize`] makes styles that
//! came from anywhere (a saved window, a caller's params) fit their text again, and every edit
//! ends with it.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::Rgb;

/// The character that stands for a picture in the text; its [`CharStyle::picture`] says which.
pub const PICTURE: char = '\u{FFFC}';

/// Deepest a paragraph may be indented (or a list nested).
pub const MAX_INDENT: u8 = 8;

/// Smallest and largest font size text may be given, in CSS pixels (what a reader shows).
pub const MIN_SIZE: u8 = 10;
pub const MAX_SIZE: u8 = 32;

/// The font sizes offered, in CSS pixels; `None` (the reader's own size) comes between the
/// second and the third.
pub const SIZES: [u8; 6] = [10, 12, 16, 20, 24, 32];

/// Longest link address kept, in bytes.
pub const MAX_LINK: usize = 2048;

/// Longest content id a picture may have, in bytes.
pub const MAX_CONTENT_ID: usize = 200;

fn is_false(b: &bool) -> bool {
    !*b
}

/// How a stretch of text looks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CharStyle {
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub strike: bool,
    /// A fixed-width font.
    #[serde(skip_serializing_if = "is_false")]
    pub fixed: bool,
    /// Font size in CSS pixels ([`MIN_SIZE`]–[`MAX_SIZE`]); `None` for the reader's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    /// A colour behind the text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight: Option<Rgb>,
    /// Where the text leads (`http`, `https`, `mailto` or `tel`; see [`is_link`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// On a [`PICTURE`] character: the content id of the inline attachment it shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
}

impl CharStyle {
    /// Nothing set.
    pub fn is_plain(&self) -> bool {
        *self == CharStyle::default()
    }

    /// The same style with what can't be kept taken off: a size out of range is brought into
    /// it, a link that may not be followed is dropped, a content id loses what could end it.
    fn sanitized(mut self) -> CharStyle {
        self.size = self.size.map(|s| s.clamp(MIN_SIZE, MAX_SIZE));
        self.link = self.link.filter(|l| is_link(l)).map(|l| l.trim().to_string());
        self.picture = self.picture.map(|p| clean_content_id(&p)).filter(|p| !p.is_empty());
        self
    }

    /// The style typed text takes after this one: the same look, but not the picture, and not
    /// the link (text typed at the end of a link is not part of it).
    pub fn continued(&self) -> CharStyle {
        CharStyle { link: None, picture: None, ..self.clone() }
    }
}

/// A content id without anything that could end it early or reach a header line.
pub fn clean_content_id(raw: &str) -> String {
    let id: String = raw.chars().filter(|c| !c.is_control() && !c.is_whitespace() && !matches!(c, '<' | '>' | '"')).collect();
    let mut end = id.len().min(MAX_CONTENT_ID);
    while !id.is_char_boundary(end) {
        end -= 1;
    }
    id.get(..end).unwrap_or("").to_string()
}

/// Whether a link may be written into a message: `http`, `https`, `mailto` or `tel`, with
/// something after the scheme, no control characters or spaces, at most [`MAX_LINK`] bytes.
pub fn is_link(url: &str) -> bool {
    let url = url.trim_matches(' ');
    if url.is_empty() || url.len() > MAX_LINK || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else { return false };
    !rest.is_empty() && ["http", "https", "mailto", "tel"].iter().any(|s| scheme.eq_ignore_ascii_case(s))
}

/// What stands in front of a paragraph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum List {
    #[default]
    None,
    Bullet,
    Number,
}

/// How a paragraph sits in its line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

impl Align {
    pub fn parse(s: &str) -> Option<Align> {
        match s.trim().to_ascii_lowercase().as_str() {
            "left" => Some(Align::Left),
            "center" | "centre" => Some(Align::Center),
            "right" => Some(Align::Right),
            _ => None,
        }
    }
}

/// How one paragraph (one line of the text) sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ParaStyle {
    #[serde(skip_serializing_if = "is_default")]
    pub list: List,
    /// Steps in from the margin, up to [`MAX_INDENT`]; in a list, how deep it is nested.
    #[serde(skip_serializing_if = "is_default")]
    pub indent: u8,
    #[serde(skip_serializing_if = "is_default")]
    pub align: Align,
    /// The paragraph is a horizontal line (its text, if any, is not shown).
    #[serde(skip_serializing_if = "is_false")]
    pub rule: bool,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// A run of characters that look the same.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Run {
    /// How many characters.
    pub len: usize,
    #[serde(skip_serializing_if = "CharStyle::is_plain")]
    pub style: CharStyle,
}

/// How a text looks: runs of characters, and one style per paragraph.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Styles {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<Run>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paras: Vec<ParaStyle>,
}

/// One paragraph of a formatted text, ready to be written out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paragraph<'a> {
    pub style: ParaStyle,
    /// Its text in runs of one look each (no line break).
    pub runs: Vec<(&'a str, &'a CharStyle)>,
}

impl Paragraph<'_> {
    pub fn text(&self) -> String {
        self.runs.iter().map(|(t, _)| *t).collect()
    }
}

/// How many characters `text` has.
pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// The byte offset of character `index` in `text` (its length when past the end).
pub fn byte_at(text: &str, index: usize) -> usize {
    text.char_indices().nth(index).map_or(text.len(), |(at, _)| at)
}

/// Which paragraph (line) character `index` is in.
pub fn line_of(text: &str, index: usize) -> usize {
    text.chars().take(index).filter(|c| *c == '\n').count()
}

impl Styles {
    /// Styles for `text` with nothing set.
    pub fn plain(text: &str) -> Styles {
        let mut s = Styles::default();
        s.normalize(text);
        s
    }

    /// Whether nothing is set anywhere.
    pub fn is_plain(&self) -> bool {
        self.runs.iter().all(|r| r.style.is_plain()) && self.paras.iter().all(|p| *p == ParaStyle::default())
    }

    /// Make the styles fit `text`: the runs cover exactly its characters, adjacent runs that look
    /// the same are one, a picture is only ever on a [`PICTURE`] character, every style is in
    /// range, and there is one paragraph style per line.
    pub fn normalize(&mut self, text: &str) {
        let total = char_len(text);
        let mut runs: Vec<Run> = Vec::with_capacity(self.runs.len() + 1);
        let mut left = total;
        let mut chars = text.chars();
        for run in std::mem::take(&mut self.runs) {
            let len = run.len.min(left);
            if len == 0 {
                continue;
            }
            left -= len;
            let style = run.style.sanitized();
            if style.picture.is_some() {
                // Only the picture characters keep it.
                for c in chars.by_ref().take(len) {
                    let style = if c == PICTURE { style.clone() } else { CharStyle { picture: None, ..style.clone() } };
                    push_run(&mut runs, 1, style);
                }
            } else {
                // `nth(len - 1)` steps over the run's characters.
                let _ = chars.nth(len - 1);
                push_run(&mut runs, len, style);
            }
        }
        if left > 0 {
            push_run(&mut runs, left, CharStyle::default());
        }
        self.runs = runs;
        let lines = text.split('\n').count();
        self.paras.truncate(lines);
        self.paras.resize(lines, ParaStyle::default());
        for p in &mut self.paras {
            p.indent = p.indent.min(MAX_INDENT);
        }
    }

    /// The style of the character at `index`.
    pub fn style_of(&self, index: usize) -> Option<&CharStyle> {
        let mut at = 0usize;
        for run in &self.runs {
            if index < at + run.len {
                return Some(&run.style);
            }
            at += run.len;
        }
        None
    }

    /// The style text typed at `index` takes: that of the character before it, or of the one
    /// after at the start of the text; without the picture or the link.
    pub fn typing_style(&self, index: usize) -> CharStyle {
        let before = index.checked_sub(1).and_then(|i| self.style_of(i));
        before.or_else(|| self.style_of(index)).map(CharStyle::continued).unwrap_or_default()
    }

    /// Whether every character in `range` has what `test` asks for (an empty range: the style
    /// typing there would get).
    pub fn every(&self, range: Range<usize>, test: impl Fn(&CharStyle) -> bool) -> bool {
        if range.start >= range.end {
            return test(&self.typing_style(range.start));
        }
        let mut at = 0usize;
        let mut any = false;
        for run in &self.runs {
            let (start, end) = (at, at + run.len);
            at = end;
            if end <= range.start || start >= range.end {
                continue;
            }
            any = true;
            if !test(&run.style) {
                return false;
            }
        }
        any
    }

    /// Change the style of the characters in `range`.
    pub fn apply(&mut self, text: &str, range: Range<usize>, mut change: impl FnMut(&mut CharStyle)) {
        let total = char_len(text);
        let (start, end) = (range.start.min(total), range.end.min(total));
        if start >= end {
            return;
        }
        self.normalize(text);
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len() + 2);
        let mut at = 0usize;
        for run in std::mem::take(&mut self.runs) {
            let (a, b) = (at, at + run.len);
            at = b;
            let (lo, hi) = (a.max(start), b.min(end));
            if lo >= hi {
                out.push(run);
                continue;
            }
            if a < lo {
                out.push(Run { len: lo - a, style: run.style.clone() });
            }
            let mut changed = run.style.clone();
            change(&mut changed);
            out.push(Run { len: hi - lo, style: changed });
            if hi < b {
                out.push(Run { len: b - hi, style: run.style });
            }
        }
        self.runs = out;
        self.normalize(text);
    }

    /// Change the style of every paragraph `range` touches.
    pub fn apply_paras(&mut self, text: &str, range: Range<usize>, mut change: impl FnMut(&mut ParaStyle)) {
        self.normalize(text);
        let first = line_of(text, range.start);
        let last = line_of(text, range.end.max(range.start));
        for p in self.paras.iter_mut().skip(first).take(last.saturating_sub(first) + 1) {
            change(p);
        }
        self.normalize(text);
    }

    /// The paragraph styles of the paragraphs `range` touches.
    pub fn paras_in(&self, text: &str, range: Range<usize>) -> &[ParaStyle] {
        let first = line_of(text, range.start);
        let last = line_of(text, range.end.max(range.start));
        let end = (last + 1).min(self.paras.len());
        self.paras.get(first.min(end)..end).unwrap_or(&[])
    }

    /// Follow the text from `old` to `new`: what was taken out loses its style, what was put in
    /// gets `typing` (or the style of the text it was typed after), paragraphs that were joined
    /// or split keep the style of the first one.
    pub fn edit(&mut self, old: &str, new: &str, typing: Option<&CharStyle>) {
        self.normalize(old);
        let old_chars: Vec<char> = old.chars().collect();
        let new_chars: Vec<char> = new.chars().collect();
        let mut prefix = old_chars.iter().zip(&new_chars).take_while(|(a, b)| a == b).count();
        let room = old_chars.len().min(new_chars.len()) - prefix;
        let mut suffix = old_chars.iter().rev().zip(new_chars.iter().rev()).take(room).take_while(|(a, b)| a == b).count();
        // A line break put in where one already was is the new one: Enter at the end of a
        // paragraph makes the next one like it.
        while old_chars.len() == prefix + suffix
            && prefix > 0
            && old_chars.get(prefix - 1) == Some(&'\n')
            && new_chars.get(new_chars.len() - suffix - 1) == Some(&'\n')
            && new_chars.len() > prefix + suffix
        {
            prefix -= 1;
            suffix += 1;
        }
        let removed = old_chars.get(prefix..old_chars.len() - suffix).unwrap_or(&[]);
        let inserted = new_chars.get(prefix..new_chars.len() - suffix).unwrap_or(&[]);
        if removed.is_empty() && inserted.is_empty() {
            self.normalize(new);
            return;
        }
        let style = match typing {
            Some(t) => t.clone().continued(),
            None => {
                let mut s = self.typing_style(prefix);
                // Inside a link, typing stays in it.
                let before = prefix.checked_sub(1).and_then(|i| self.style_of(i)).and_then(|s| s.link.clone());
                let after = self.style_of(prefix + removed.len()).and_then(|s| s.link.clone());
                if before.is_some() && before == after {
                    s.link = before;
                }
                s
            }
        };
        let mut text = old.to_string();
        let inserted: String = inserted.iter().collect();
        self.splice(&mut text, prefix..prefix + removed.len(), &inserted, &style);
        self.normalize(new);
    }

    /// Put `inserted`, looking like `style`, in place of the characters `range` of `text`, and
    /// follow it: paragraphs that are joined keep the style of the first, and those that are
    /// made take the style of the one they were split from.
    pub fn splice(&mut self, text: &mut String, range: Range<usize>, inserted: &str, style: &CharStyle) {
        self.normalize(text);
        let total = char_len(text);
        let (cut_start, cut_end) = (range.start.min(total), range.end.clamp(range.start.min(total), total));
        let (from, to) = (byte_at(text, cut_start), byte_at(text, cut_end));
        let line = text.get(..from).map_or(0, |t| t.matches('\n').count());
        let joined = text.get(from..to).map_or(0, |t| t.matches('\n').count());
        let split = inserted.matches('\n').count();
        let count = char_len(inserted);
        // The runs: cut out what was removed, put in what was inserted.
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len() + 2);
        let mut at = 0usize;
        let mut placed = false;
        for run in std::mem::take(&mut self.runs) {
            let (a, b) = (at, at + run.len);
            at = b;
            if a < cut_start {
                out.push(Run { len: b.min(cut_start) - a, style: run.style.clone() });
            }
            if !placed && b >= cut_start {
                out.push(Run { len: count, style: style.clone() });
                placed = true;
            }
            if b > cut_end {
                let from = a.max(cut_end);
                out.push(Run { len: b - from, style: run.style });
            }
        }
        if !placed {
            out.push(Run { len: count, style: style.clone() });
        }
        self.runs = out;
        // The paragraphs.
        if line < self.paras.len() {
            let first = line + 1;
            let gone = joined.min(self.paras.len().saturating_sub(first));
            self.paras.drain(first..first + gone);
            let mut copy = self.paras.get(line).copied().unwrap_or_default();
            copy.rule = false;
            let at = first.min(self.paras.len());
            self.paras.splice(at..at, std::iter::repeat_n(copy, split));
        }
        text.replace_range(from..to, inserted);
        self.normalize(text);
    }

    /// Put `more`, with its styles, after `text`: the paragraph `text` ends in and the first of
    /// `more` become one, styled as the first.
    pub fn append(&mut self, text: &mut String, more: &str, styles: &Styles) {
        self.normalize(text);
        let mut other = styles.clone();
        other.normalize(more);
        self.runs.extend(other.runs);
        self.paras.extend(other.paras.into_iter().skip(1));
        text.push_str(more);
        self.normalize(text);
    }

    /// The content ids of the pictures in the text.
    pub fn pictures(&self) -> impl Iterator<Item = &str> {
        self.runs.iter().filter_map(|r| r.style.picture.as_deref())
    }

    /// The text in paragraphs, each with its runs.
    pub fn paragraphs<'a>(&'a self, text: &'a str) -> Vec<Paragraph<'a>> {
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
        let mut out: Vec<Paragraph<'a>> = Vec::new();
        let mut current: Vec<(&'a str, &'a CharStyle)> = Vec::new();
        let mut runs = self.runs.iter();
        let mut run: Option<(&Run, usize)> = None;
        let mut start = 0usize;
        let mut style: &CharStyle = &PLAIN;
        let flush = |current: &mut Vec<(&'a str, &'a CharStyle)>, from: usize, to: usize, style: &'a CharStyle| {
            if let Some(piece) = text.get(from..to).filter(|p| !p.is_empty()) {
                current.push((piece, style));
            }
        };
        for (at, c) in text.char_indices() {
            // The style of this character.
            let here: &CharStyle = loop {
                match run {
                    Some((r, left)) if left > 0 => {
                        run = Some((r, left - 1));
                        break &r.style;
                    }
                    _ => match runs.next() {
                        Some(r) => run = Some((r, r.len)),
                        None => break &PLAIN,
                    },
                }
            };
            if c == '\n' {
                flush(&mut current, start, at, style);
                let index = out.len();
                out.push(Paragraph { style: self.paras.get(index).copied().unwrap_or_default(), runs: std::mem::take(&mut current) });
                start = at + 1;
                style = here;
                continue;
            }
            if here != style {
                flush(&mut current, start, at, style);
                start = at;
            }
            style = here;
        }
        flush(&mut current, start, text.len(), style);
        let index = out.len();
        out.push(Paragraph { style: self.paras.get(index).copied().unwrap_or_default(), runs: current });
        out
    }
}

/// Add a run to the end of `runs`, joining it to the last one when they look the same.
fn push_run(runs: &mut Vec<Run>, len: usize, style: CharStyle) {
    if len == 0 {
        return;
    }
    match runs.last_mut() {
        Some(last) if last.style == style => last.len += len,
        _ => runs.push(Run { len, style }),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn bold() -> CharStyle {
        CharStyle { bold: true, ..CharStyle::default() }
    }

    #[test]
    fn normalize_fits_the_runs_and_paragraphs_to_the_text() {
        let mut s = Styles { runs: vec![Run { len: 99, style: bold() }, Run { len: 3, style: bold() }], paras: vec![] };
        s.normalize("ab\ncd");
        assert_eq!(s.runs, vec![Run { len: 5, style: bold() }]);
        assert_eq!(s.paras.len(), 2);
        let mut s =
            Styles { runs: vec![Run { len: 1, style: CharStyle { size: Some(200), link: Some("javascript:x".into()), ..bold() } }], paras: vec![] };
        s.normalize("abc");
        assert_eq!(s.runs[0].style, CharStyle { size: Some(MAX_SIZE), ..bold() });
        assert_eq!(s.runs[1], Run { len: 2, style: CharStyle::default() });
        // A picture is only on a picture character.
        let pic = CharStyle { picture: Some("p1@x<y>".into()), ..CharStyle::default() };
        let mut s = Styles { runs: vec![Run { len: 3, style: pic }], paras: vec![] };
        s.normalize("a\u{FFFC}b");
        assert_eq!(s.runs.len(), 3);
        assert_eq!(s.runs[1].style.picture.as_deref(), Some("p1@xy"));
        assert!(s.runs[0].style.is_plain() && s.runs[2].style.is_plain());
    }

    #[test]
    fn applying_and_testing_a_style() {
        let text = "Hello world";
        let mut s = Styles::plain(text);
        s.apply(text, 6..11, |c| c.bold = true);
        assert!(s.every(6..11, |c| c.bold));
        assert!(!s.every(0..11, |c| c.bold));
        assert!(s.every(11..11, |c| c.bold), "typing at the end continues the bold");
        let p = s.paragraphs(text);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].runs, vec![("Hello ", &CharStyle::default()), ("world", &bold())]);
    }

    #[test]
    fn edits_keep_styles_where_the_text_stayed() {
        let old = "one two three";
        let mut s = Styles::plain(old);
        s.apply(old, 4..7, |c| c.italic = true);
        // Typing inside the italic word is italic.
        let new = "one twXo three";
        s.edit(old, new, None);
        assert!(s.every(4..8, |c| c.italic) && !s.every(8..9, |c| c.italic));
        // Typing with a style chosen for it.
        let newer = "Ione twXo three";
        s.edit(new, newer, Some(&bold()));
        assert!(s.every(0..1, |c| c.bold && !c.italic));
        assert!(s.every(5..9, |c| c.italic));
    }

    #[test]
    fn splitting_and_joining_paragraphs() {
        let old = "a\nb";
        let mut s = Styles::plain(old);
        s.apply_paras(old, 0..0, |p| p.list = List::Bullet);
        assert_eq!(s.paras, vec![ParaStyle { list: List::Bullet, ..ParaStyle::default() }, ParaStyle::default()]);
        // Enter at the end of the bullet: a new bullet.
        s.edit(old, "a\n\nb", None);
        assert_eq!(s.paras.iter().map(|p| p.list).collect::<Vec<_>>(), vec![List::Bullet, List::Bullet, List::None]);
        // Joined again.
        s.edit("a\n\nb", "ab", None);
        assert_eq!(s.paras, vec![ParaStyle { list: List::Bullet, ..ParaStyle::default() }]);
        assert_eq!(s.paras_in("ab", 1..1).len(), 1);
    }

    #[test]
    fn paragraphs_split_runs_at_line_breaks() {
        let text = "ab\ncd\n";
        let mut s = Styles::plain(text);
        s.apply(text, 1..4, |c| c.underline = true);
        let p = s.paragraphs(text);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].text(), "ab");
        assert_eq!(p[1].runs.first().map(|r| r.0), Some("c"));
        assert!(p[1].runs[0].1.underline && !p[1].runs[1].1.underline);
        assert!(p[2].runs.is_empty());
    }

    #[test]
    fn links_are_only_the_safe_kinds() {
        for ok in ["https://example.org/a b".replace(' ', ""), "mailto:ada@example.org".into(), "tel:+1555".into(), "HTTP://x".into()] {
            assert!(is_link(&ok), "{ok}");
        }
        for bad in ["", "javascript:alert(1)", "cid:x", "data:x", "https://exa mple.org", "https://x\n", "https:", "example.org"] {
            assert!(!is_link(bad), "{bad:?}");
        }
    }

    fn arb_style() -> impl Strategy<Value = CharStyle> {
        (
            any::<[bool; 5]>(),
            proptest::option::of(0u8..255),
            proptest::option::of(any::<(u8, u8, u8)>()),
            proptest::option::of("[a-z:/.]{0,12}"),
            proptest::option::of("[a-z@<>.]{0,8}"),
        )
            .prop_map(|(b, size, color, link, picture)| CharStyle {
                bold: b[0],
                italic: b[1],
                underline: b[2],
                strike: b[3],
                fixed: b[4],
                size,
                color: color.map(|(r, g, b)| Rgb(r, g, b)),
                highlight: None,
                link,
                picture,
            })
    }

    fn arb_styles() -> impl Strategy<Value = Styles> {
        (proptest::collection::vec((0usize..20, arb_style()), 0..8), proptest::collection::vec((0u8..20, any::<bool>()), 0..8)).prop_map(
            |(runs, paras)| Styles {
                runs: runs.into_iter().map(|(len, style)| Run { len, style }).collect(),
                paras: paras.into_iter().map(|(indent, rule)| ParaStyle { indent, rule, ..ParaStyle::default() }).collect(),
            },
        )
    }

    proptest! {
        #[test]
        fn any_edit_leaves_styles_that_fit(old in "[a-c\n\u{FFFC}é]{0,30}", new in "[a-c\n\u{FFFC}é]{0,30}", mut s in arb_styles(), typing in proptest::option::of(arb_style())) {
            s.edit(&old, &new, typing.as_ref());
            prop_assert_eq!(s.runs.iter().map(|r| r.len).sum::<usize>(), char_len(&new));
            prop_assert_eq!(s.paras.len(), new.split('\n').count());
            prop_assert!(s.runs.iter().all(|r| r.len > 0));
            prop_assert!(s.runs.windows(2).all(|w| w[0].style != w[1].style));
            let paragraphs = s.paragraphs(&new);
            prop_assert_eq!(paragraphs.len(), s.paras.len());
            let joined: Vec<String> = paragraphs.iter().map(Paragraph::text).collect();
            prop_assert_eq!(joined.join("\n"), new.clone());
            let len = char_len(&new);
            s.apply(&new, len / 3..len, |c| c.bold = !c.bold);
            s.apply_paras(&new, 0..len, |p| p.indent = p.indent.saturating_add(3));
            prop_assert_eq!(s.runs.iter().map(|r| r.len).sum::<usize>(), len);
        }
    }
}
