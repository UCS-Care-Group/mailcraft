// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The little of CSS that is read.
//!
//! There is no CSS engine: no selectors, no cascade, no `<style>` blocks, no classes, nothing
//! fetched. [`Css::parse`] reads one `style` attribute for a whitelist of presentational facts and
//! ignores every other declaration:
//!
//! | Read | From |
//! |---|---|
//! | text colour | `color` |
//! | background colour | `background-color`, the colour in `background` (a `url()` there is never used) |
//! | bold, italic | `font-weight`, `font-style`, the same words in `font` |
//! | underline, strike | `text-decoration`, `text-decoration-line` |
//! | alignment | `text-align` |
//! | font size | `font-size`, the size in `font` |
//! | font family | `font-family`, `<font face>`: the first family's name only, for pasting into the composer (the reading pane draws in its own font) |
//! | hidden | `display:none`, `visibility:hidden`, `opacity:0`, `max-height:0`, `max-width:0`, a zero `height` or `width` with `overflow:hidden` |
//! | text too small to read | a font size of 2 pixels or less: the text at that size is not shown; what is inside at a size of its own is, and so are pictures, and so is the text of an element with a `class` or an `id` (or inside one that asks for the size and has one): its size is given in a style sheet, and style sheets are not read |
//! | a margin or none | `margin`, `margin-top`, `margin-bottom`: only whether there is one to speak of (over 6 pixels) |
//! | a border or none | `border` and its sides, `border-style`, `border-width`: only whether there is one |
//! | Outlook's lists | `mso-list` (the level, and the marker text to replace) |
//! | a table's or cell's width | `width` (pixels or a percentage, clamped like the attribute, which it beats), `max-width` (clamped; a zero one hides) |
//! | centring | `margin: auto`, `margin: 0 auto`, or `margin-left` and `margin-right` both `auto`: like `align=center` on a table, or on a block with a `max-width` |
//! | a picture's size | `width` and `height` in pixels, where its attributes give none |
//!
//! The old-style attributes (`color`, `bgcolor`, `align`, `width`, `height`, `<font size>`) are
//! read by the helpers at the end.
//!
//! Two things a newsletter writes for one mail program are read as every other program reads
//! them. A message is shown here the way it is written for programs that are not Outlook: what
//! it keeps for Outlook alone (`<!--[if mso]>…<![endif]-->`, drawn in VML) is a comment and is
//! dropped, so what it hides from Outlook alone (`mso-hide:all`: the button or the picture that
//! everybody else gets in its place) is shown, or the message would have neither. And a font
//! size of nothing around pictures and columns (`font-size:0`, to close the gaps between them)
//! hides the text written at that size, not the pictures and not the text inside that names a
//! size of its own. Nor the text inside that gets its size from a style sheet: a builder sets
//! nothing on every cell and the sizes that count by class, in a `<style>` block that is not
//! read here, so an element with a `class` or an `id` is not held to the nothing around it
//! (without that, such a message showed no text at all).

use crate::{Align, Length, MAX_FONT_PX, MIN_FONT_PX, Rgb, truncate};

/// Longest `style` attribute read, in bytes; the rest is ignored.
const MAX_STYLE: usize = 4096;

/// Most declarations read from one `style` attribute.
const MAX_DECLARATIONS: usize = 64;

/// Longest value looked at, in bytes; a longer one is ignored.
const MAX_VALUE: usize = 1024;

/// The size text has when a message doesn't say, in CSS pixels: what a relative size (`120%`,
/// `<small>`) is measured against.
pub(crate) const BASE_FONT_PX: f32 = 13.0;

/// A margin up to this many pixels is no paragraph gap: lines that close are still one block of lines.
const SLIGHT_MARGIN_PX: f32 = 6.0;

/// Text this small or smaller is a hiding trick, not something to read.
const HIDDEN_FONT_PX: f32 = 2.0;

/// A font size before it is resolved against the size around it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Size {
    /// CSS pixels.
    Px(f32),
    /// Times the size around it.
    Scale(f32),
}

impl Size {
    /// The size in whole pixels, clamped to what is readable, given the size around it; `None`
    /// when it is so small that the text is meant not to be seen.
    pub fn resolve(self, parent: Option<u8>) -> Option<u8> {
        let px = match self {
            Size::Px(px) => px,
            Size::Scale(scale) => scale * parent.map_or(BASE_FONT_PX, f32::from),
        };
        if !px.is_finite() || px <= HIDDEN_FONT_PX {
            return None;
        }
        Some(px.round().clamp(f32::from(MIN_FONT_PX), f32::from(MAX_FONT_PX)) as u8)
    }
}

/// What an Outlook list paragraph says about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MsoList {
    /// The written-out marker (a bullet character, `1.`), to be replaced by a real one.
    Ignore,
    /// The item's level, from 1.
    Level(u8),
}

/// The whitelisted facts of one `style` attribute.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Css {
    pub color: Option<Rgb>,
    pub background: Option<Rgb>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub align: Option<Align>,
    pub size: Option<Size>,
    /// The first family of `font-family` ([`family`]).
    pub font: Option<String>,
    pub hidden: bool,
    /// Whether the margin above (below) is given, and given as none to speak of: zero, or a
    /// few pixels (a line's own leading is more than that).
    pub margin_top_zero: Option<bool>,
    pub margin_bottom_zero: Option<bool>,
    /// Borders: top, right, bottom, left.
    pub borders: [bool; 4],
    pub mso_list: Option<MsoList>,
    /// The width and height when given in absolute units, in CSS pixels (for telling a tracking
    /// pixel; never used to size anything).
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// The `width` a table or cell asks for, like its `width` attribute ([`Length`], clamped).
    pub layout_width: Option<Length>,
    /// A `max-width` other than zero, clamped the same way.
    pub max_width: Option<Length>,
    /// Whether the left (right) margin is `auto`.
    pub margin_left_auto: Option<bool>,
    pub margin_right_auto: Option<bool>,
}

impl Css {
    /// Read a `style` attribute. Unknown properties, unreadable values and everything past the
    /// caps are ignored; a later declaration of a property replaces an earlier one.
    pub fn parse(style: &str) -> Css {
        let mut css = Css::default();
        let mut clipped = false;
        let mut zero_box = false;
        for (property, value) in declarations(truncate(style, MAX_STYLE)).take(MAX_DECLARATIONS) {
            if value.len() > MAX_VALUE || property.len() > 32 {
                continue;
            }
            let property = property.to_ascii_lowercase();
            let value = value.to_ascii_lowercase();
            let value = value.trim_end_matches("!important").trim();
            match property.as_str() {
                "color" => {
                    if let Some(c) = color(value) {
                        css.color = Some(c);
                    }
                }
                "background-color" => css.background = color(value),
                "background" => css.background = tokens(value).find_map(color),
                "font-weight" => css.bold = weight(value).or(css.bold),
                "font-style" => css.italic = slant(value).or(css.italic),
                "font" => {
                    for token in tokens(value) {
                        if let Some(bold) = weight(token).filter(|_| token != "normal") {
                            css.bold = Some(bold);
                        } else if let Some(italic) = slant(token).filter(|_| token != "normal") {
                            css.italic = Some(italic);
                        } else if let Some(size) = font_size(token.split('/').next().unwrap_or("")) {
                            // The size comes before the family; what follows is not looked at.
                            css.size = Some(size);
                            break;
                        }
                    }
                }
                "text-decoration" | "text-decoration-line" => {
                    if tokens(value).any(|t| t == "none") {
                        css.underline = Some(false);
                        css.strike = Some(false);
                    }
                    if tokens(value).any(|t| t == "underline") {
                        css.underline = Some(true);
                    }
                    if tokens(value).any(|t| t == "line-through") {
                        css.strike = Some(true);
                    }
                }
                "text-align" => css.align = align(value).or(css.align),
                "font-size" => css.size = font_size(value).or(css.size),
                "font-family" => css.font = family(value).or(css.font.take()),
                "display" => css.hidden |= value == "none",
                "visibility" => css.hidden |= matches!(value, "hidden" | "collapse"),
                "opacity" => css.hidden |= number(value.trim_end_matches('%')).is_some_and(|n| n <= 0.0),
                // (`mso-hide:all` hides nothing here: see the notes at the top.)
                "max-height" => css.hidden |= is_zero(value) == Some(true),
                "max-width" => {
                    css.hidden |= is_zero(value) == Some(true);
                    css.max_width = css_length(value).or(css.max_width);
                }
                "overflow" | "overflow-x" | "overflow-y" => clipped |= tokens(value).any(|t| matches!(t, "hidden" | "clip")),
                "height" => {
                    css.height = length_px(value).or(css.height);
                    zero_box |= is_zero(value) == Some(true);
                }
                "width" => {
                    css.width = length_px(value).or(css.width);
                    css.layout_width = css_length(value).or(css.layout_width);
                    zero_box |= is_zero(value) == Some(true);
                }
                "margin" => {
                    let parts: Vec<&str> = tokens(value).take(4).collect();
                    let top = parts.first().copied().and_then(is_slight);
                    let bottom = parts.get(2).copied().and_then(is_slight).or(if parts.len() < 3 { top } else { None });
                    let auto = |part: Option<&&str>| part.map(|p| *p == "auto");
                    // One value is all four sides, two are vertical then horizontal, three add the bottom.
                    let right = parts.get(1).or(parts.first());
                    let left = parts.get(3).or(right);
                    css.margin_left_auto = auto(left).or(css.margin_left_auto);
                    css.margin_right_auto = auto(right).or(css.margin_right_auto);
                    css.margin_top_zero = top.or(css.margin_top_zero);
                    css.margin_bottom_zero = bottom.or(css.margin_bottom_zero);
                }
                "margin-left" => css.margin_left_auto = Some(value == "auto"),
                "margin-right" => css.margin_right_auto = Some(value == "auto"),
                "margin-top" | "margin-block-start" => css.margin_top_zero = is_slight(value).or(css.margin_top_zero),
                "margin-bottom" | "margin-block-end" => css.margin_bottom_zero = is_slight(value).or(css.margin_bottom_zero),
                "border" | "border-style" | "border-width" => {
                    if let Some(on) = has_border(value) {
                        css.borders = [on; 4];
                    }
                }
                "border-top" | "border-right" | "border-bottom" | "border-left" => {
                    let side = match property.as_str() {
                        "border-top" => 0,
                        "border-right" => 1,
                        "border-bottom" => 2,
                        _ => 3,
                    };
                    if let (Some(on), Some(slot)) = (has_border(value), css.borders.get_mut(side)) {
                        *slot = on;
                    }
                }
                "mso-list" => {
                    css.mso_list = if value.starts_with("ignore") {
                        Some(MsoList::Ignore)
                    } else {
                        tokens(value)
                            .find_map(|t| t.strip_prefix("level"))
                            .and_then(|n| n.parse::<u8>().ok())
                            .map(|level| MsoList::Level(level.clamp(1, 9)))
                            .or(css.mso_list)
                    };
                }
                _ => {}
            }
        }
        // (A font size too small to read hides the text written at it, and nothing else: that is
        // seen to where the size is resolved, [`Size::resolve`].)
        css.hidden |= zero_box && clipped;
        css
    }

    /// Whether any side has a border.
    #[cfg(test)]
    pub fn any_border(&self) -> bool {
        self.borders.iter().any(|b| *b)
    }

    /// Centred by automatic margins on both sides (`margin: 0 auto`).
    pub fn centred(&self) -> bool {
        self.margin_left_auto == Some(true) && self.margin_right_auto == Some(true)
    }

    /// Whether every side has a border.
    pub fn framed(&self) -> bool {
        self.borders.iter().all(|b| *b)
    }
}

/// The `property: value` pairs of a `style` attribute. A `;` inside parentheses or quotes (a
/// `url(data:…;base64,…)`) doesn't end a declaration.
fn declarations(style: &str) -> impl Iterator<Item = (&str, &str)> {
    let bytes = style.as_bytes();
    let mut start = 0;
    std::iter::from_fn(move || {
        while start < bytes.len() {
            let mut depth = 0u32;
            let mut quote = 0u8;
            let mut end = start;
            while let Some(&b) = bytes.get(end) {
                match b {
                    b'"' | b'\'' if quote == 0 => quote = b,
                    b if b == quote => quote = 0,
                    b'(' if quote == 0 => depth = depth.saturating_add(1),
                    b')' if quote == 0 => depth = depth.saturating_sub(1),
                    b';' if quote == 0 && depth == 0 => break,
                    _ => {}
                }
                end += 1;
            }
            let declaration = style.get(start..end).unwrap_or("");
            start = end + 1;
            if let Some((property, value)) = declaration.split_once(':') {
                let (property, value) = (property.trim(), value.trim());
                if !property.is_empty() && !value.is_empty() {
                    return Some((property, value));
                }
            }
        }
        None
    })
}

/// The parts of a value separated by white space; white space inside parentheses separates nothing.
fn tokens(value: &str) -> impl Iterator<Item = &str> {
    let bytes = value.as_bytes();
    let mut start = 0;
    std::iter::from_fn(move || {
        while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
            start += 1;
        }
        if start >= bytes.len() {
            return None;
        }
        let mut depth = 0u32;
        let mut end = start;
        while let Some(&b) = bytes.get(end) {
            match b {
                b'(' => depth = depth.saturating_add(1),
                b')' => depth = depth.saturating_sub(1),
                b if b.is_ascii_whitespace() && depth == 0 => break,
                _ => {}
            }
            end += 1;
        }
        let token = value.get(start..end).unwrap_or("");
        start = end;
        Some(token)
    })
}

/// A finite number, as CSS writes them (`12`, `.5`, `-3.25`, `1e2` is not one).
fn number(text: &str) -> Option<f32> {
    let text = text.trim();
    if text.is_empty() || text.len() > 16 || !text.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+')) {
        return None;
    }
    text.parse::<f32>().ok().filter(|n| n.is_finite())
}

/// A number and its unit (lower case; empty when there is none).
fn dimension(value: &str) -> Option<(f32, &str)> {
    let value = value.trim();
    let split = value.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))).unwrap_or(value.len());
    let n = number(value.get(..split)?)?;
    Some((n, value.get(split..)?.trim()))
}

/// A length in absolute units, in CSS pixels. Relative lengths (`em`, `%`) give nothing.
fn length_px(value: &str) -> Option<f32> {
    let (n, unit) = dimension(value)?;
    let px = match unit {
        "px" => n,
        "pt" => n * 4.0 / 3.0,
        "pc" => n * 16.0,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "" if n == 0.0 => 0.0,
        _ => return None,
    };
    px.is_finite().then_some(px)
}

/// A `width` or `max-width` as a [`Length`], clamped like the attribute's: pixels (or any absolute
/// unit) from 1 to 65,535, or a percentage from 1 to 100. Zero, negative and nonsense give nothing.
fn css_length(value: &str) -> Option<Length> {
    let (n, unit) = dimension(value)?;
    if unit == "%" {
        return (n >= 1.0).then(|| Length::Percent(n.round().min(100.0) as u8));
    }
    let px = length_px(value)?;
    (px >= 1.0).then(|| Length::Px(px.round().min(f32::from(u16::MAX)) as u16))
}

/// Whether a length is zero, whatever its unit; `None` for something that isn't a length (`auto`).
fn is_zero(value: &str) -> Option<bool> {
    dimension(value).map(|(n, _)| n == 0.0)
}

/// Whether a margin is none to speak of: zero or negative, or no more than [`SLIGHT_MARGIN_PX`];
/// `None` for something that isn't a length (`auto`).
fn is_slight(value: &str) -> Option<bool> {
    let (n, unit) = dimension(value)?;
    Some(match unit {
        "em" | "rem" => n * BASE_FONT_PX <= SLIGHT_MARGIN_PX,
        "%" => n <= 0.0,
        _ => length_px(value).map_or(n <= 0.0, |px| px <= SLIGHT_MARGIN_PX),
    })
}

/// The first family of a `font-family` list (or `<font face>`), lower case and unquoted, when it
/// is a plain name (letters, digits, spaces and hyphens, at most 32 of them).
pub(crate) fn family(value: &str) -> Option<String> {
    let first = value.split(',').next()?.trim().trim_matches(['\'', '"']).trim().to_ascii_lowercase();
    let plain = !first.is_empty() && first.len() <= 32 && first.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-');
    plain.then_some(first)
}

fn weight(value: &str) -> Option<bool> {
    match value {
        "bold" | "bolder" => Some(true),
        "normal" | "lighter" => Some(false),
        _ => value.parse::<u16>().ok().filter(|n| (1..=1000).contains(n)).map(|n| n >= 600),
    }
}

fn slant(value: &str) -> Option<bool> {
    match value {
        "italic" | "oblique" => Some(true),
        "normal" => Some(false),
        _ => None,
    }
}

fn align(value: &str) -> Option<Align> {
    match value {
        "left" | "start" | "justify" => Some(Align::Left),
        "center" | "-webkit-center" | "-moz-center" => Some(Align::Center),
        "right" | "end" => Some(Align::Right),
        _ => None,
    }
}

/// A `font-size` value.
fn font_size(value: &str) -> Option<Size> {
    let px = |px: f32| Some(Size::Px(px));
    match value {
        "xx-small" => px(9.0),
        "x-small" => px(10.0),
        "small" => px(13.0),
        "medium" => px(16.0),
        "large" => px(18.0),
        "x-large" => px(24.0),
        "xx-large" => px(32.0),
        "xxx-large" => px(48.0),
        "smaller" => Some(Size::Scale(0.85)),
        "larger" => Some(Size::Scale(1.2)),
        _ => {
            let (n, unit) = dimension(value)?;
            if n < 0.0 {
                return None;
            }
            match unit {
                "em" | "rem" => Some(Size::Scale(n)),
                "%" => Some(Size::Scale(n / 100.0)),
                _ => length_px(value).map(Size::Px),
            }
        }
    }
}

/// Whether a `border` value asks for a border: `Some(false)` for `none`, `hidden` or a zero width,
/// `None` for a value that says nothing (`inherit`).
fn has_border(value: &str) -> Option<bool> {
    let mut seen = false;
    for token in tokens(value) {
        if matches!(token, "none" | "hidden") || is_zero(token) == Some(true) {
            return Some(false);
        }
        seen |= !matches!(token, "inherit" | "initial" | "unset" | "revert");
    }
    seen.then_some(true)
}

/// A CSS colour: `#rgb`, `#rrggbb` (with or without alpha), `rgb()`, `rgba()` or a name. A fully
/// transparent colour, a keyword that names no colour (`inherit`, `transparent`, `windowtext`) and
/// anything unreadable give `None`. The text is expected in lower case.
pub(crate) fn color(value: &str) -> Option<Rgb> {
    let value = value.trim();
    if value.len() > 64 {
        return None;
    }
    if let Some(hex) = value.strip_prefix('#') {
        return hex_color(hex);
    }
    if let Some(inner) = value.strip_prefix("rgba(").or_else(|| value.strip_prefix("rgb(")) {
        let inner = inner.strip_suffix(')')?;
        let mut parts = inner.split(|c: char| c == ',' || c == '/' || c.is_ascii_whitespace()).filter(|p| !p.is_empty());
        let mut channel = || -> Option<u8> {
            let part = parts.next()?;
            let n = match part.strip_suffix('%') {
                Some(percent) => number(percent)? * 2.55,
                None => number(part)?,
            };
            Some(n.round().clamp(0.0, 255.0) as u8)
        };
        let rgb = Rgb(channel()?, channel()?, channel()?);
        // What can't be seen is no colour.
        let alpha = match parts.next() {
            Some(a) => match a.strip_suffix('%') {
                Some(percent) => number(percent)? / 100.0,
                None => number(a)?,
            },
            None => 1.0,
        };
        return (alpha > 0.02).then_some(rgb);
    }
    let index = NAMED.binary_search_by(|(name, _)| name.cmp(&value)).ok()?;
    let (_, rgb) = NAMED.get(index)?;
    Some(Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, *rgb as u8))
}

fn hex_color(hex: &str) -> Option<Rgb> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| hex.get(i..=i).and_then(|d| u8::from_str_radix(d, 16).ok());
    let pair = |i: usize| hex.get(i..i + 2).and_then(|d| u8::from_str_radix(d, 16).ok());
    match hex.len() {
        3 | 4 => {
            if hex.len() == 4 && digit(3)? == 0 {
                return None;
            }
            Some(Rgb(digit(0)? * 17, digit(1)? * 17, digit(2)? * 17))
        }
        6 | 8 => {
            if hex.len() == 8 && pair(6)? == 0 {
                return None;
            }
            Some(Rgb(pair(0)?, pair(2)?, pair(4)?))
        }
        _ => None,
    }
}

/// The colour of a `color`, `bgcolor` or `text` attribute: a CSS colour, or six hex digits
/// without their `#` (old pages leave it off).
pub(crate) fn attr_color(value: &str) -> Option<Rgb> {
    let value = value.trim().to_ascii_lowercase();
    color(&value).or_else(|| if value.len() == 6 { hex_color(&value) } else { None })
}

/// The pixels of a `width`, `height` or `border` attribute (`600`, `600px`); a percentage gives nothing.
pub(crate) fn attr_px(value: &str) -> Option<f32> {
    let value = value.trim().to_ascii_lowercase();
    let (n, unit) = dimension(&value)?;
    (matches!(unit, "" | "px") && n >= 0.0).then_some(n)
}

/// A `width` attribute: pixels or a percentage. Zero and nonsense give nothing.
pub(crate) fn attr_length(value: &str) -> Option<Length> {
    let value = value.trim().to_ascii_lowercase();
    let (n, unit) = dimension(&value)?;
    if n < 1.0 {
        return None;
    }
    match unit {
        "" | "px" => Some(Length::Px(n.round().min(f32::from(u16::MAX)) as u16)),
        "%" => Some(Length::Percent(n.round().min(100.0) as u8)),
        _ => None,
    }
}

/// An `align` attribute.
pub(crate) fn attr_align(value: &str) -> Option<Align> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "middle" => Some(Align::Center),
        other => align(other),
    }
}

/// A `<font size>`: 1 to 7, or a step up or down from 3 (`+1`, `-2`).
pub(crate) fn font_tag_size(value: &str) -> Option<Size> {
    let value = value.trim();
    let n: i32 = value.trim_start_matches('+').parse().ok().filter(|n: &i32| (-9..=9).contains(n))?;
    let step = if value.starts_with(['+', '-']) { 3 + n } else { n };
    let px = match step.clamp(1, 7) {
        1 => 10.0,
        2 => 13.0,
        3 => 16.0,
        4 => 18.0,
        5 => 24.0,
        6 => 32.0,
        _ => 48.0,
    };
    Some(Size::Px(px))
}

/// The CSS colour names, in alphabetical order.
const NAMED: [(&str, u32); 148] = [
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_names_are_sorted() {
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn colours() {
        assert_eq!(color("#fff"), Some(Rgb(255, 255, 255)));
        assert_eq!(color("#1f497d"), Some(Rgb(0x1f, 0x49, 0x7d)));
        assert_eq!(color("#1f497d80"), Some(Rgb(0x1f, 0x49, 0x7d)));
        assert_eq!(color("#abc8"), Some(Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(color("rgb(31, 73, 125)"), Some(Rgb(31, 73, 125)));
        assert_eq!(color("rgb(31 73 125 / 50%)"), Some(Rgb(31, 73, 125)));
        assert_eq!(color("rgba(255,0,0,.5)"), Some(Rgb(255, 0, 0)));
        assert_eq!(color("rgb(100%, 0%, 50%)"), Some(Rgb(255, 0, 128)));
        assert_eq!(color("rgb(999, -5, 12.6)"), Some(Rgb(255, 0, 13)));
        assert_eq!(color("  navy "), Some(Rgb(0, 0, 0x80)));
        assert_eq!(color("rebeccapurple"), Some(Rgb(0x66, 0x33, 0x99)));
        // Nothing to see, or nothing to read.
        for none in [
            "",
            "transparent",
            "inherit",
            "windowtext",
            "currentcolor",
            "#00000000",
            "#0000",
            "rgba(0,0,0,0)",
            "rgb(1,2)",
            "rgb(a,b,c)",
            "#12",
            "#12345",
            "#ggg",
            "url(x)",
            "chucknorris",
            "rgb(1,2,3",
            "#ééé",
        ] {
            assert_eq!(color(none), None, "{none:?}");
        }
        assert_eq!(color(&"a".repeat(10_000)), None);
    }

    #[test]
    fn attribute_values() {
        assert_eq!(attr_color("#FFCC00"), Some(Rgb(255, 204, 0)));
        assert_eq!(attr_color("ffcc00"), Some(Rgb(255, 204, 0)));
        assert_eq!(attr_color(" White "), Some(Rgb(255, 255, 255)));
        assert_eq!(attr_color("zzzzzz"), None);
        assert_eq!(attr_length("600"), Some(Length::Px(600)));
        assert_eq!(attr_length(" 600px "), Some(Length::Px(600)));
        assert_eq!(attr_length("100%"), Some(Length::Percent(100)));
        assert_eq!(attr_length("250%"), Some(Length::Percent(100)));
        assert_eq!(attr_length("99999999"), Some(Length::Px(u16::MAX)));
        for none in ["", "0", "-5", "auto", "5em", "1e9", "NaN", "inf"] {
            assert_eq!(attr_length(none), None, "{none:?}");
        }
        assert_eq!(attr_px("1"), Some(1.0));
        assert_eq!(attr_px("0px"), Some(0.0));
        assert_eq!(attr_px("50%"), None);
        assert_eq!(attr_align("CENTER"), Some(Align::Center));
        assert_eq!(attr_align("middle"), Some(Align::Center));
        assert_eq!(attr_align("justify"), Some(Align::Left));
        assert_eq!(attr_align("absmiddle"), None);
        assert_eq!(font_tag_size("2"), Some(Size::Px(13.0)));
        assert_eq!(font_tag_size("+1"), Some(Size::Px(18.0)));
        assert_eq!(font_tag_size("-2"), Some(Size::Px(10.0)));
        assert_eq!(font_tag_size("7"), Some(Size::Px(48.0)));
        assert_eq!(font_tag_size("99"), None);
        assert_eq!(font_tag_size("big"), None);
    }

    #[test]
    fn the_whitelist() {
        let css = Css::parse(
            "COLOR: #1F497D; background-color: rgb(255, 255, 0); font-weight: 700; font-style: italic; \
             text-decoration: underline line-through; text-align: right; font-size: 11.0pt",
        );
        assert_eq!(css.color, Some(Rgb(0x1f, 0x49, 0x7d)));
        assert_eq!(css.background, Some(Rgb(255, 255, 0)));
        assert_eq!((css.bold, css.italic, css.underline, css.strike), (Some(true), Some(true), Some(true), Some(true)));
        assert_eq!(css.align, Some(Align::Right));
        assert_eq!(css.size.and_then(|s| s.resolve(None)), Some(15));
        assert!(!css.hidden);
        // Later declarations win; `!important` is only a word.
        let css = Css::parse("font-weight:bold;font-weight:normal !important;text-decoration:none;color:red;color:nonsense");
        assert_eq!((css.bold, css.underline, css.strike), (Some(false), Some(false), Some(false)));
        assert_eq!(css.color, Some(Rgb(255, 0, 0)));
    }

    #[test]
    fn widths_and_automatic_margins_are_read_clamped() {
        let w = |v: &str| Css::parse(&format!("width:{v}")).layout_width;
        assert_eq!(w("600px"), Some(Length::Px(600)));
        assert_eq!(w("450pt"), Some(Length::Px(600)));
        assert_eq!(w("80%"), Some(Length::Percent(80)));
        assert_eq!(w("250%"), Some(Length::Percent(100)));
        assert_eq!(w("99999999px"), Some(Length::Px(u16::MAX)));
        for none in ["0", "0px", "-5px", "-1%", "0.2px", "auto", "12em", "calc(1px + 2px)", "px", "1e9px", "abc"] {
            assert_eq!(w(none), None, "{none}");
        }
        let m = |v: &str| Css::parse(&format!("max-width:{v}"));
        assert_eq!(m("600px").max_width, Some(Length::Px(600)));
        assert!(!m("600px").hidden);
        assert!(m("0").hidden && m("0").max_width.is_none());
        assert_eq!(m("none").max_width, None);
        let centred = |v: &str| Css::parse(v).centred();
        for yes in [
            "margin:0 auto",
            "margin:auto",
            "margin: 0 auto 0 auto",
            "margin:1px auto 2px",
            "margin-left:auto;margin-right:auto",
            "margin:0;margin-left:auto;margin-right:AUTO",
        ] {
            assert!(centred(yes), "{yes}");
        }
        for no in
            ["margin:0", "margin:auto 0", "margin-left:auto", "margin:0 auto;margin-right:0", "margin:0 auto 0 0", "margin:auto;margin-left:4px"]
        {
            assert!(!centred(no), "{no}");
        }
    }

    #[test]
    fn everything_else_is_ignored() {
        let css = Css::parse(
            "position:absolute; left:-9999px; float:left; padding:40px; line-height:0; \
             background-image:url(https://tracker.example/p.gif); content:'x'; behavior:url(x.htc); min-width:50%; z-index:9; \
             transform:scale(9); animation:spin 1s; letter-spacing:4em; text-transform:uppercase",
        );
        assert_eq!(css, Css::default());
    }

    #[test]
    fn a_font_family_is_its_first_name_alone() {
        assert_eq!(Css::parse("font-family:'Comic Sans MS', cursive").font.as_deref(), Some("comic sans ms"));
        assert_eq!(Css::parse("font-family:\"Calibri\",sans-serif").font.as_deref(), Some("calibri"));
        assert_eq!(Css::parse("font-family:url(x);").font, None);
        assert_eq!(Css::parse(&format!("font-family:{}", "a".repeat(40))).font, None);
    }

    #[test]
    fn the_background_shorthand_gives_only_its_colour() {
        assert_eq!(Css::parse("background:#f4f4f4 url(https://example.org/bg.png) no-repeat top left").background, Some(Rgb(0xf4, 0xf4, 0xf4)));
        assert_eq!(Css::parse("background:url('data:image/png;base64,AAAA') rgb(0, 102, 204)").background, Some(Rgb(0, 102, 204)));
        assert_eq!(Css::parse("background: none").background, None);
        assert_eq!(Css::parse("background:#fff; background-color: transparent").background, None);
        assert_eq!(Css::parse("background: linear-gradient(#fff, #000)").background, None);
    }

    #[test]
    fn font_sizes() {
        let size = |style: &str, parent: Option<u8>| Css::parse(style).size.and_then(|s| s.resolve(parent));
        assert_eq!(size("font-size:14px", None), Some(14));
        assert_eq!(size("font-size:12pt", None), Some(16));
        assert_eq!(size("font-size:small", None), Some(13));
        assert_eq!(size("font-size:150%", Some(20)), Some(30));
        assert_eq!(size("font-size:1.5em", None), Some(20));
        assert_eq!(size("font-size:smaller", Some(20)), Some(17));
        // Clamped to what can be read.
        assert_eq!(size("font-size:500px", None), Some(MAX_FONT_PX));
        assert_eq!(size("font-size:6px", None), Some(MIN_FONT_PX));
        assert_eq!(size("font-size:99999999999999999999pt", None), None);
        assert_eq!(size("font-size:-4px", None), None);
        assert_eq!(size("font-size:big", None), None);
        // The shorthand: weight, slant and size; the family is not read.
        let css = Css::parse("font: italic bold 7.0pt/1.2 \"Times New Roman\"");
        assert_eq!((css.bold, css.italic, css.size.and_then(|s| s.resolve(None))), (Some(true), Some(true), Some(MIN_FONT_PX)));
        assert_eq!(Css::parse("font: 12px bold").bold, None);
    }

    #[test]
    fn hiding_tricks() {
        for hidden in [
            "display:none",
            "DISPLAY : NONE !important",
            "visibility:hidden",
            "opacity:0",
            "opacity: 0.0",
            "max-height:0px",
            "max-width:0",
            "height:0;overflow:hidden",
            "overflow:hidden; width:0px",
            "color:#fff;display:none;font-size:1px;line-height:1px;max-height:0;max-width:0;opacity:0;overflow:hidden;mso-hide:all",
        ] {
            assert!(Css::parse(hidden).hidden, "{hidden}");
        }
        for shown in [
            "display:block",
            "visibility:visible",
            "opacity:0.5",
            "opacity:1",
            "font-size:9px",
            "height:0",
            "overflow:hidden",
            "max-height:40px",
            "height:10px;overflow:hidden",
            "display:inline-block;width:98%",
            "color:#ffffff",
            // Hidden from Outlook alone, which is not what reads the message here.
            "mso-hide:all",
            "mso-hide:all;display:inline-block",
            "",
        ] {
            assert!(!Css::parse(shown).hidden, "{shown}");
        }
        // A font size too small to read hides text, not the element: it is a size that resolves
        // to nothing, whatever is inside is judged by its own.
        for tiny in ["font-size:0", "font-size:1px", "font-size:0.0pt", "font-size:2px;line-height:0"] {
            let css = Css::parse(tiny);
            assert!(!css.hidden, "{tiny}");
            assert_eq!(css.size.and_then(|s| s.resolve(Some(16))), None, "{tiny}");
        }
        // A relative size is judged against the size around it.
        assert_eq!(Css::parse("font-size:0%").size.and_then(|s| s.resolve(Some(16))), None);
        assert_eq!(Css::parse("font-size:10%").size.and_then(|s| s.resolve(Some(16))), None);
    }

    #[test]
    fn margins_and_borders_are_yes_or_no() {
        let css = Css::parse("margin:0in");
        assert_eq!((css.margin_top_zero, css.margin_bottom_zero), (Some(true), Some(true)));
        let css = Css::parse("margin:0 0 12pt 0");
        assert_eq!((css.margin_top_zero, css.margin_bottom_zero), (Some(true), Some(false)));
        let css = Css::parse("margin: 0 auto; margin-bottom: 1em");
        assert_eq!((css.margin_top_zero, css.margin_bottom_zero), (Some(true), Some(false)));
        let css = Css::parse("margin: auto");
        assert_eq!((css.margin_top_zero, css.margin_bottom_zero), (None, None));
        // A few pixels are no gap; a line's worth is.
        let css = Css::parse("margin:10px 0 2px 0");
        assert_eq!((css.margin_top_zero, css.margin_bottom_zero), (Some(false), Some(true)));
        let css = Css::parse("margin-top:-4px;margin-bottom:.4em");
        assert_eq!((css.margin_top_zero, css.margin_bottom_zero), (Some(true), Some(true)));
        let css = Css::parse("margin-top:6px;margin-bottom:6.5px");
        assert_eq!((css.margin_top_zero, css.margin_bottom_zero), (Some(true), Some(false)));
        let css = Css::parse("margin: 1em 5% nonsense");
        assert_eq!((css.margin_top_zero, css.margin_bottom_zero), (Some(false), None));
        assert!(Css::parse("border:solid windowtext 1.0pt").framed());
        assert!(Css::parse("border:1px solid #ccc").framed());
        assert!(!Css::parse("border:none").any_border());
        assert!(!Css::parse("border:0").any_border());
        assert!(!Css::parse("border: 0px solid red").any_border());
        assert_eq!(Css::parse("border:none;border-top:solid #E1E1E1 1.0pt;padding:3.0pt 0in 0in 0in").borders, [true, false, false, false]);
        assert_eq!(Css::parse("border-left: 1px solid rgb(204, 204, 204)").borders, [false, false, false, true]);
        assert_eq!(Css::parse("border:1px solid;border-bottom:none").borders, [true, true, false, true]);
        assert!(!Css::parse("border-collapse:collapse;border-color:red;border-radius:4px").any_border());
    }

    #[test]
    fn outlook_lists() {
        assert_eq!(Css::parse("text-indent:-.25in;mso-list:l0 level2 lfo1").mso_list, Some(MsoList::Level(2)));
        assert_eq!(Css::parse("mso-list:Ignore").mso_list, Some(MsoList::Ignore));
        assert_eq!(Css::parse("mso-list:l0 level99 lfo1").mso_list, Some(MsoList::Level(9)));
        assert_eq!(Css::parse("mso-list:none").mso_list, None);
    }

    #[test]
    fn hostile_styles_are_bounded() {
        let many = "color:red;".repeat(100_000);
        assert_eq!(Css::parse(&many).color, Some(Rgb(255, 0, 0)));
        // A declaration past the caps is not read.
        let late = format!("{}display:none", "x:y;".repeat(MAX_DECLARATIONS + 1));
        assert!(!Css::parse(&late).hidden);
        let long = format!("{}display:none", " ".repeat(MAX_STYLE));
        assert!(!Css::parse(&long).hidden);
        for junk in [";;;:::;;", ":", "color", "color:", ":red", "((((", "'\"'\"", "color:rgb(((((", "font:///", "margin:    ", "é:é;ü"] {
            let _ = Css::parse(junk);
        }
        assert_eq!(Css::parse("background:url(a;b);color:blue").color, Some(Rgb(0, 0, 255)));
        assert_eq!(Css::parse("font-family:'a;b';color:blue").color, Some(Rgb(0, 0, 255)));
    }

    proptest::proptest! {
        #[test]
        fn any_style_parses(s in proptest::prelude::any::<String>()) {
            let css = Css::parse(&s);
            if let Some(size) = css.size.and_then(|s| s.resolve(None)) {
                proptest::prop_assert!((MIN_FONT_PX..=MAX_FONT_PX).contains(&size));
            }
        }

        #[test]
        fn declaration_soup_parses(s in "((color|background|font-size|font|display|margin|border|width|opacity|mso-list|text-align) ?: ?[#a-z0-9.%() ,;:'\"!-]{0,16};?){0,12}") {
            let css = Css::parse(&s);
            let _ = (css.any_border(), css.framed());
            let _ = attr_color(&s);
            let _ = attr_length(&s);
            let _ = font_tag_size(&s);
        }
    }
}
