// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures behind a box (#601): the `background` attribute of a table, a cell or the body, and
//! `background-image`, `background-repeat`, `background-position`, `background-size` and the
//! picture in `background`, from a `style` or the message's style sheets.
//!
//! A picture behind a box is a picture like any other: one attached to the message (`cid:`) or
//! written into it (`data:`, within the caps a message's pictures share, `data.rs`) is shown;
//! one on the web is shown only when the owner asked for the message's pictures, through the
//! same request and the same limits as an `<img>`. Nothing here fetches anything: the address
//! is kept, and a UI asks the host for it or not.
//!
//! What is read, and what isn't:
//!
//! | Read | As |
//! |---|---|
//! | the picture | the first `url()` in `background-image` or `background`; `none` and a gradient are no picture |
//! | repeating | `repeat`, `no-repeat`, `repeat-x`, `repeat-y` (`space` and `round` as `repeat`), or two of them |
//! | where it starts | `left`, `center`, `right`, `top`, `bottom`, a percentage (0 to 100) or a length (0 to [`MAX_BG_OFFSET_PX`]; a negative one is 0); a four-value position is read for its keywords |
//! | its size | `auto`, `cover`, `contain`, or a width and a height (a length, a percentage or `auto`) |
//!
//! The text of a `style` attribute and of a style sheet's rule is capped (`style.rs`, `sheet.rs`),
//! so a picture written into a style is read only when it is small; one in the `background`
//! attribute has the caps of any other picture written into the message. A row's (`<tr>`)
//! `background` is not read: each of its cells would start the picture again.

use serde::{Deserialize, Serialize};

use crate::Length;
use crate::data::Picture;

/// The farthest a picture behind a box can be moved from its top left corner, in CSS pixels.
pub const MAX_BG_OFFSET_PX: u16 = 2048;

/// The largest size a picture behind a box can be asked to be drawn at, in CSS pixels.
pub const MAX_BG_SIZE_PX: u16 = 8192;

/// How a picture behind a box is repeated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BgRepeat {
    /// Across and down: the box is tiled with it.
    #[default]
    Both,
    /// Across only.
    X,
    /// Down only.
    Y,
    /// Once.
    None,
}

impl BgRepeat {
    /// Whether it repeats across and whether down.
    pub fn axes(self) -> [bool; 2] {
        match self {
            BgRepeat::Both => [true, true],
            BgRepeat::X => [true, false],
            BgRepeat::Y => [false, true],
            BgRepeat::None => [false, false],
        }
    }
}

/// Where a picture behind a box starts, on one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BgOffset {
    /// As CSS has it: that much of the room the picture leaves, 0 to 100 (`center` is 50).
    Percent(u8),
    /// CSS pixels from the box's edge, 0 to [`MAX_BG_OFFSET_PX`].
    Px(u16),
}

impl Default for BgOffset {
    fn default() -> Self {
        BgOffset::Percent(0)
    }
}

/// The size a picture behind a box is drawn at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BgSize {
    /// Its own size.
    #[default]
    Auto,
    /// As small as covers the whole box, its shape kept.
    Cover,
    /// As large as fits in the box, its shape kept.
    Contain,
    /// A width and a height asked for (a percentage of the box's); `None` keeps the shape.
    Given { width: Option<Length>, height: Option<Length> },
}

/// A picture behind a box: its address and how it is laid behind what the box holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BgPicture {
    /// A safe `http`, `https` or `cid:` address, or `data:` and its type for one written into
    /// the message (never its content).
    pub src: String,
    /// The bytes of a picture written into the message, when it can be shown.
    #[serde(skip)]
    pub data: Option<Picture>,
    #[serde(default)]
    pub repeat: BgRepeat,
    /// Across, then down.
    #[serde(default)]
    pub position: [BgOffset; 2],
    #[serde(default)]
    pub size: BgSize,
}

/// What a style says of a picture behind a box, before the builder makes it one.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct BgCss {
    /// The address as written; `Some(None)` where the style says there is none (`none`, a
    /// gradient, or a `background` without a picture).
    pub image: Option<Option<String>>,
    pub repeat: Option<BgRepeat>,
    pub position: Option<[BgOffset; 2]>,
    pub size: Option<BgSize>,
}

impl BgCss {
    /// Read one declaration, when it is about a picture behind a box. `raw` is the value as
    /// written (an address keeps its letter case), without its `!important`.
    pub fn read(&mut self, property: &str, raw: &str) {
        match property {
            "background" => {
                // The shorthand sets every part, to what it says or to the part's default.
                let layer = first_layer(raw);
                let (url, rest) = url_in(layer);
                let rest = rest.to_ascii_lowercase();
                // (A colour function is one word, and its own `/` is not the one before the size.)
                let all = words(&rest);
                let (place, size) = match all.iter().position(|w| *w == "/") {
                    Some(at) => (all.get(..at).unwrap_or_default(), all.get(at + 1..).unwrap_or_default()),
                    None => (&all[..], &[][..]),
                };
                self.image = Some(url);
                // (The repeating may come anywhere, after the size too.)
                self.repeat = Some(repeat(&all).unwrap_or_default());
                self.position = Some(position(place).unwrap_or_default());
                // (The size is what follows the `/`, up to the next keyword that isn't one of its own.)
                let size_words: Vec<&str> = size.iter().copied().take_while(|w| is_size_word(w)).take(2).collect();
                self.size = Some(sizes(&size_words).unwrap_or_default());
            }
            "background-image" => self.image = Some(url_in(first_layer(raw)).0),
            "background-repeat" => {
                let lower = first_layer(raw).to_ascii_lowercase();
                let words: Vec<&str> = lower.split_ascii_whitespace().collect();
                self.repeat = repeat(&words).or(self.repeat);
            }
            "background-position" => {
                let lower = first_layer(raw).to_ascii_lowercase();
                let words: Vec<&str> = lower.split_ascii_whitespace().collect();
                self.position = position(&words).or(self.position);
            }
            "background-size" => {
                let lower = first_layer(raw).to_ascii_lowercase();
                let words: Vec<&str> = lower.split_ascii_whitespace().take(2).collect();
                self.size = sizes(&words).or(self.size);
            }
            _ => {}
        }
    }
}

/// Whether a property is one of those read here.
pub(crate) fn is_bg_property(property: &str) -> bool {
    matches!(property, "background" | "background-image" | "background-repeat" | "background-position" | "background-size")
}

/// The words of a shorthand: separated by white space and around a `/`, outside parentheses
/// and quotes, a word with parentheses in it (a colour function, a gradient) left out whole.
fn words<'a>(value: &'a str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let (mut depth, mut quote, mut start, mut paren) = (0u32, 0u8, 0usize, false);
    let end_word = |out: &mut Vec<&'a str>, from: usize, to: usize, paren: bool| {
        if let Some(word) = value.get(from..to).filter(|w| !w.is_empty() && !paren) {
            out.push(word);
        }
    };
    for (i, b) in value.bytes().enumerate() {
        match b {
            b'"' | b'\'' if quote == 0 => quote = b,
            b if b == quote => quote = 0,
            b'(' if quote == 0 => {
                depth = depth.saturating_add(1);
                paren = true;
            }
            b')' if quote == 0 => depth = depth.saturating_sub(1),
            b'/' if quote == 0 && depth == 0 => {
                end_word(&mut out, start, i, paren);
                out.push("/");
                (start, paren) = (i + 1, false);
            }
            b if b.is_ascii_whitespace() && quote == 0 && depth == 0 => {
                end_word(&mut out, start, i, paren);
                (start, paren) = (i + 1, false);
            }
            _ => {}
        }
    }
    end_word(&mut out, start, value.len(), paren);
    out
}

/// The first layer of a value of several (`url(a), url(b)`): up to the first comma outside
/// parentheses and quotes.
fn first_layer(value: &str) -> &str {
    let (mut depth, mut quote) = (0u32, 0u8);
    for (i, b) in value.bytes().enumerate() {
        match b {
            b'"' | b'\'' if quote == 0 => quote = b,
            b if b == quote => quote = 0,
            b'(' if quote == 0 => depth = depth.saturating_add(1),
            b')' if quote == 0 => depth = depth.saturating_sub(1),
            b',' if quote == 0 && depth == 0 => return value.get(..i).unwrap_or(value),
            _ => {}
        }
    }
    value
}

/// The address of the first `url()` in a value (letter case kept, quotes and white space round
/// it taken off), and the value without it. No address when there is no `url()`, when it is
/// empty, or when it is never closed.
pub(crate) fn url_in(value: &str) -> (Option<String>, String) {
    let bytes = value.as_bytes();
    let start = bytes.windows(4).position(|w| w.eq_ignore_ascii_case(b"url("));
    let Some(start) = start else { return (None, value.to_string()) };
    let open = start + 4;
    let mut quote = 0u8;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'"' | b'\'' if quote == 0 => quote = b,
            b if b == quote => quote = 0,
            b')' if quote == 0 => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(end) = end else { return (None, value.get(..start).unwrap_or("").to_string()) };
    let inside = value.get(open..end).unwrap_or("").trim();
    let unquoted = match inside.as_bytes().first() {
        Some(q @ (b'"' | b'\'')) if inside.len() >= 2 && inside.as_bytes().last() == Some(q) => inside.get(1..inside.len() - 1).unwrap_or(""),
        _ => inside,
    };
    let rest = format!("{} {}", value.get(..start).unwrap_or(""), value.get(end + 1..).unwrap_or(""));
    let url = unquoted.trim();
    ((!url.is_empty()).then(|| url.to_string()), rest)
}

/// The repeating that words say, when they say any: one keyword, or two (across, then down).
fn repeat(words: &[&str]) -> Option<BgRepeat> {
    let one = |w: &str| match w {
        "repeat" | "space" | "round" => Some(true),
        "no-repeat" => Some(false),
        _ => None,
    };
    if let Some(r) = words.iter().find_map(|w| match *w {
        "repeat-x" => Some(BgRepeat::X),
        "repeat-y" => Some(BgRepeat::Y),
        _ => None,
    }) {
        return Some(r);
    }
    let mut found = words.iter().filter_map(|w| one(w));
    let across = found.next()?;
    let down = found.next().unwrap_or(across);
    Some(match (across, down) {
        (true, true) => BgRepeat::Both,
        (true, false) => BgRepeat::X,
        (false, true) => BgRepeat::Y,
        (false, false) => BgRepeat::None,
    })
}

/// One part of a position that is not a keyword of one axis: a length or a percentage.
fn offset(word: &str) -> Option<BgOffset> {
    let (n, unit) = crate::style::dimension(word)?;
    if unit == "%" {
        return Some(BgOffset::Percent(n.round().clamp(0.0, 100.0) as u8));
    }
    let px = crate::style::length_px(word)?;
    Some(BgOffset::Px(px.round().clamp(0.0, f32::from(MAX_BG_OFFSET_PX)) as u16))
}

/// Where words put the picture, when they say anything of it: keywords of either axis in any
/// order, and lengths or percentages across and then down. What one of them leaves is centred.
fn position(words: &[&str]) -> Option<[BgOffset; 2]> {
    let (mut x, mut y) = (None, None);
    // Centres and lengths, in order, for whichever axis is left.
    let mut pending: Vec<BgOffset> = Vec::new();
    let mut any = false;
    for word in words {
        match *word {
            "left" => x = Some(BgOffset::Percent(0)),
            "right" => x = Some(BgOffset::Percent(100)),
            "top" => y = Some(BgOffset::Percent(0)),
            "bottom" => y = Some(BgOffset::Percent(100)),
            "center" => pending.push(BgOffset::Percent(50)),
            other => match offset(other) {
                // (In a four-value position the lengths are from the keyword before them: only
                // the keywords are read.)
                Some(o) if pending.len() < 2 && (x.is_none() || y.is_none()) => pending.push(o),
                Some(_) => {}
                None => continue,
            },
        }
        any = true;
    }
    if !any {
        return None;
    }
    for o in pending {
        if x.is_none() {
            x = Some(o);
        } else if y.is_none() {
            y = Some(o);
        }
    }
    Some([x.unwrap_or(BgOffset::Percent(50)), y.unwrap_or(BgOffset::Percent(50))])
}

fn is_size_word(word: &str) -> bool {
    matches!(word, "auto" | "cover" | "contain") || crate::style::dimension(word).is_some()
}

/// One side of a size: a length, a percentage, or `auto` (`Some(None)`).
fn side(word: &str) -> Option<Option<Length>> {
    if word == "auto" {
        return Some(None);
    }
    let (n, unit) = crate::style::dimension(word)?;
    if unit == "%" {
        return (n > 0.0).then(|| Some(Length::Percent(n.round().clamp(1.0, 100.0) as u8)));
    }
    let px = crate::style::length_px(word)?;
    (px >= 1.0).then(|| Some(Length::Px(px.round().min(f32::from(MAX_BG_SIZE_PX)) as u16)))
}

/// The size words ask for, when they make sense as one.
fn sizes(words: &[&str]) -> Option<BgSize> {
    match words {
        ["cover", ..] => Some(BgSize::Cover),
        ["contain", ..] => Some(BgSize::Contain),
        [one] => side(one).map(|width| if width.is_none() { BgSize::Auto } else { BgSize::Given { width, height: None } }),
        [w, h, ..] => {
            let (width, height) = (side(w)?, side(h)?);
            Some(if width.is_none() && height.is_none() { BgSize::Auto } else { BgSize::Given { width, height } })
        }
        [] => None,
    }
}
