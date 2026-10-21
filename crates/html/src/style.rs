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
//! | margins | `margin` and its sides: a size in `px`, `pt`, `em` or `%` (of 600 px), 0 to 64 pixels a side, negative is 0; `auto` is read for centring only |
//! | padding | `padding` and its sides: the same units, 0 to 64 pixels a side |
//! | lines round a box | `border` and its sides, `border-width`, `border-style`, `border-color`: a width of 1 to 4 pixels (`thin`, `medium` and `thick` too), solid, dashed or dotted (every other style is drawn solid), and a colour a UI adjusts to its paper |
//! | rounded corners | `border-radius`: its first length, 0 to 24 pixels |
//! | line height | `line-height`: a number, a percentage, `em` or a length, 1 to 2.5 times the font size; also the part after the `/` of `font` |
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

use crate::{
    Align, Insets, Length, Line, LineHeight, LineStyle, MAX_BOX_PX, MAX_FONT_PX, MAX_LINE_HEIGHT, MAX_LINE_PX, MAX_RADIUS_PX, MIN_FONT_PX,
    MIN_LINE_HEIGHT, Rgb, truncate,
};

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
    /// Borders: top, right, bottom, left. Which sides have a line (a line of [`Css::lines`]
    /// that is set).
    pub borders: [bool; 4],
    /// The lines themselves: width, style and colour of each side.
    pub lines: [Line; 4],
    /// Corners: the first length of `border-radius`, in pixels.
    pub radius: u8,
    /// Padding in pixels per side (top, right, bottom, left); `None` where the style says nothing.
    pub padding: [Option<u8>; 4],
    /// Margin in pixels per side; `None` where the style says nothing, or `auto`.
    pub margin: [Option<u8>; 4],
    /// `Some(None)` is `normal`, which asks for nothing of its own.
    pub line_height: Option<Option<LineHeight>>,
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
        let mut raw = [RawLine::default(); 4];
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
                            if let Some((_, height)) = token.split_once('/') {
                                css.line_height = line_height(height).or(css.line_height);
                            }
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
                    if let Some(sides) = sides4(value, margin_px) {
                        for (slot, side) in css.margin.iter_mut().zip(sides) {
                            *slot = side;
                        }
                    }
                }
                "margin-left" => {
                    css.margin_left_auto = Some(value == "auto");
                    set_side(&mut css.margin, 3, margin_px(value));
                }
                "margin-right" => {
                    css.margin_right_auto = Some(value == "auto");
                    set_side(&mut css.margin, 1, margin_px(value));
                }
                "margin-top" | "margin-block-start" => {
                    css.margin_top_zero = is_slight(value).or(css.margin_top_zero);
                    set_side(&mut css.margin, 0, margin_px(value));
                }
                "margin-bottom" | "margin-block-end" => {
                    css.margin_bottom_zero = is_slight(value).or(css.margin_bottom_zero);
                    set_side(&mut css.margin, 2, margin_px(value));
                }
                "padding" => {
                    if let Some(sides) = sides4(value, padding_px) {
                        for (slot, side) in css.padding.iter_mut().zip(sides) {
                            *slot = Some(side);
                        }
                    }
                }
                "padding-top" | "padding-block-start" => set_side(&mut css.padding, 0, padding_px(value).map(Some)),
                "padding-right" => set_side(&mut css.padding, 1, padding_px(value).map(Some)),
                "padding-bottom" | "padding-block-end" => set_side(&mut css.padding, 2, padding_px(value).map(Some)),
                "padding-left" => set_side(&mut css.padding, 3, padding_px(value).map(Some)),
                "border" => {
                    if let Some(line) = line_shorthand(value) {
                        raw = [line; 4];
                    }
                }
                "border-top" | "border-right" | "border-bottom" | "border-left" => {
                    let side = match property.as_str() {
                        "border-top" => 0,
                        "border-right" => 1,
                        "border-bottom" => 2,
                        _ => 3,
                    };
                    if let (Some(line), Some(slot)) = (line_shorthand(value), raw.get_mut(side)) {
                        *slot = line;
                    }
                }
                "border-width" => {
                    if let Some(sides) = sides4(value, border_width) {
                        for (slot, width) in raw.iter_mut().zip(sides) {
                            slot.width = Some(width);
                        }
                    }
                }
                "border-style" => {
                    if let Some(sides) = sides4(value, line_style) {
                        for (slot, style) in raw.iter_mut().zip(sides) {
                            slot.style = Some(style);
                        }
                    }
                }
                "border-color" => {
                    if let Some(sides) = sides4(value, hue) {
                        for (slot, hue) in raw.iter_mut().zip(sides) {
                            slot.hue = Some(hue);
                        }
                    }
                }
                "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" => {
                    if let (Some(width), Some(slot)) = (border_width(value), raw.get_mut(side_of(&property))) {
                        slot.width = Some(width);
                    }
                }
                "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {
                    if let (Some(style), Some(slot)) = (line_style(value), raw.get_mut(side_of(&property))) {
                        slot.style = Some(style);
                    }
                }
                "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => {
                    if let (Some(hue), Some(slot)) = (hue(value), raw.get_mut(side_of(&property))) {
                        slot.hue = Some(hue);
                    }
                }
                "border-radius" => css.radius = radius_px(value).unwrap_or(css.radius),
                "line-height" => css.line_height = line_height(value).or(css.line_height),
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
        for ((line, raw), border) in css.lines.iter_mut().zip(raw).zip(&mut css.borders) {
            *line = raw.finish();
            *border = line.is_set();
        }
        css
    }

    /// The padding, where any side of it is given (the others are 0).
    pub fn padding_insets(&self) -> Option<Insets> {
        let [top, right, bottom, left] = self.padding;
        if self.padding.iter().all(Option::is_none) {
            return None;
        }
        Some(Insets { top: top.unwrap_or(0), right: right.unwrap_or(0), bottom: bottom.unwrap_or(0), left: left.unwrap_or(0) })
    }

    /// The margins given in pixels (the others, and `auto`, are 0).
    pub fn margin_insets(&self) -> Insets {
        let [top, right, bottom, left] = self.margin;
        Insets { top: top.unwrap_or(0), right: right.unwrap_or(0), bottom: bottom.unwrap_or(0), left: left.unwrap_or(0) }
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

/// What `border` and its parts said about one side, before they are put together.
#[derive(Clone, Copy, Debug, Default)]
struct RawLine {
    width: Option<u8>,
    /// `Some(None)`: `none` or `hidden`.
    style: Option<Option<LineStyle>>,
    hue: Option<Hue>,
}

/// A colour named for a line.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hue {
    Rgb(Rgb),
    /// `transparent`: the line takes room and shows nothing.
    Clear,
    /// A keyword that names no colour of its own (`currentcolor`, `windowtext`): the reader's line colour.
    Plain,
}

/// The line width when none is given: CSS's `medium`.
const MEDIUM_LINE_PX: u8 = 3;

impl RawLine {
    fn finish(self) -> Line {
        if self.style == Some(None) || self.width == Some(0) || self.hue == Some(Hue::Clear) {
            return Line::default();
        }
        // (A width or a style alone is a line, as it has been; a colour alone is not.)
        if self.width.is_none() && self.style.is_none() {
            return Line::default();
        }
        Line {
            width: self.width.unwrap_or(MEDIUM_LINE_PX),
            style: self.style.flatten().unwrap_or(LineStyle::Solid),
            color: match self.hue {
                Some(Hue::Rgb(rgb)) => Some(rgb),
                _ => None,
            },
        }
    }
}

/// Which side (top, right, bottom, left) a property like `border-left-width` is about.
fn side_of(property: &str) -> usize {
    if property.starts_with("border-top") {
        0
    } else if property.starts_with("border-right") {
        1
    } else if property.starts_with("border-bottom") {
        2
    } else {
        3
    }
}

fn set_side<T>(sides: &mut [T; 4], index: usize, value: Option<T>) {
    if let (Some(value), Some(slot)) = (value, sides.get_mut(index)) {
        *slot = value;
    }
}

/// A value of one to four parts, one for each side as CSS has it (all four; vertical and
/// horizontal; top, horizontal and bottom; top, right, bottom and left). `None` when any part is
/// unreadable or there are more than four: the whole declaration is then ignored.
fn sides4<T: Copy>(value: &str, parse: impl Fn(&str) -> Option<T>) -> Option<[T; 4]> {
    let mut parts = Vec::with_capacity(4);
    for token in tokens(value).take(5) {
        if parts.len() == 4 {
            return None;
        }
        parts.push(parse(token)?);
    }
    let at = |i: usize| parts.get(i).copied();
    match parts.len() {
        1 => Some([at(0)?; 4]),
        2 => Some([at(0)?, at(1)?, at(0)?, at(1)?]),
        3 => Some([at(0)?, at(1)?, at(2)?, at(1)?]),
        4 => Some([at(0)?, at(1)?, at(2)?, at(3)?]),
        _ => None,
    }
}

/// A length for room round a box, in whole pixels from 0 to [`MAX_BOX_PX`]: `px`, `pt` and the
/// other absolute units, `em` and `rem` (of the base font size), `%` (of a 600 pixel page, as a
/// share of an unknown width has to be guessed). Anything else, a function above all (`calc()`,
/// `var()`, `env()`), gives nothing; so does a bare number that is not zero. A negative length
/// is 0 where `negative` allows it and nothing where it does not.
fn box_px(token: &str, negative: bool) -> Option<u8> {
    if token.contains('(') {
        return None;
    }
    let (n, unit) = dimension(token)?;
    if n < 0.0 {
        return negative.then_some(0);
    }
    let px = match unit {
        "em" | "rem" => n * BASE_FONT_PX,
        "%" => n * 6.0,
        _ => length_px(token)?,
    };
    px.is_finite().then(|| px.round().clamp(0.0, f32::from(MAX_BOX_PX)) as u8)
}

fn padding_px(token: &str) -> Option<u8> {
    box_px(token, false)
}

/// A margin: `Some(None)` for `auto`, which centres and takes no room of its own.
fn margin_px(token: &str) -> Option<Option<u8>> {
    if token == "auto" { Some(None) } else { box_px(token, true).map(Some) }
}

/// The width of a line: 0 for none, else 1 to [`MAX_LINE_PX`] pixels (anything under a pixel is
/// a pixel). `thin`, `medium` and `thick` are 1, 3 and 4.
fn border_width(token: &str) -> Option<u8> {
    match token {
        "thin" => Some(1),
        "medium" => Some(MEDIUM_LINE_PX),
        "thick" => Some(MAX_LINE_PX),
        _ => {
            if token.contains('(') {
                return None;
            }
            let (n, _) = dimension(token)?;
            if n < 0.0 {
                return None;
            }
            let px = length_px(token)?;
            Some(if px <= 0.0 { 0 } else { px.round().clamp(1.0, f32::from(MAX_LINE_PX)) as u8 })
        }
    }
}

/// A line style: `Some(None)` for `none` and `hidden`.
fn line_style(token: &str) -> Option<Option<LineStyle>> {
    match token {
        "none" | "hidden" => Some(None),
        "solid" | "double" | "groove" | "ridge" | "inset" | "outset" => Some(Some(LineStyle::Solid)),
        "dashed" => Some(Some(LineStyle::Dashed)),
        "dotted" => Some(Some(LineStyle::Dotted)),
        _ => None,
    }
}

/// A colour for a line.
fn hue(token: &str) -> Option<Hue> {
    if token == "transparent" {
        return Some(Hue::Clear);
    }
    if token.starts_with("rgb") || token.starts_with('#') {
        return color(token).map(Hue::Rgb);
    }
    if token.contains('(') {
        return None;
    }
    // A name: its colour if it is one, else a keyword that says to use the reader's own.
    match color(token) {
        Some(rgb) => Some(Hue::Rgb(rgb)),
        None if matches!(
            token,
            "currentcolor" | "windowtext" | "window" | "buttontext" | "buttonface" | "buttonborder" | "graytext" | "canvastext"
        ) =>
        {
            Some(Hue::Plain)
        }
        None => None,
    }
}

/// The value of `border` or of one side of it: any of a width, a style and a colour, in any
/// order. A function, a keyword that is none of the three that is not a plain word, or the
/// words that inherit give nothing.
fn line_shorthand(value: &str) -> Option<RawLine> {
    let mut out = RawLine::default();
    for (n, token) in tokens(value).take(5).enumerate() {
        // (A width, a style and a colour; more is not a border.)
        if n >= 4 {
            return None;
        }
        if matches!(token, "inherit" | "initial" | "unset" | "revert") {
            return None;
        }
        if let Some(style) = line_style(token) {
            out.style = Some(style);
        } else if let Some(width) = border_width(token) {
            out.width = Some(width);
        } else {
            out.hue = Some(hue(token)?);
        }
    }
    if out.width.is_none() && out.style.is_none() && out.hue.is_none() {
        return None;
    }
    // (What the shorthand leaves out is what a bare `border` has: a solid line of medium width.)
    out.width = Some(out.width.unwrap_or(MEDIUM_LINE_PX));
    out.style = Some(out.style.unwrap_or(Some(LineStyle::Solid)));
    Some(out)
}

/// The first length of a `border-radius`, 0 to [`MAX_RADIUS_PX`] pixels. A share is of a box
/// 48 pixels across, so that `50%` is as round as a corner gets.
fn radius_px(value: &str) -> Option<u8> {
    let first = tokens(value.split('/').next().unwrap_or("")).next()?;
    if first.contains('(') {
        return None;
    }
    let (n, unit) = dimension(first)?;
    if n < 0.0 {
        return None;
    }
    let px = match unit {
        "%" => n * 0.48,
        "em" | "rem" => n * BASE_FONT_PX,
        _ => length_px(first)?,
    };
    px.is_finite().then(|| px.round().clamp(0.0, f32::from(MAX_RADIUS_PX)) as u8)
}

/// A `line-height`: `Some(None)` for `normal`. A number, a percentage or `em` is a multiple of
/// the font size, 1 to 2.5 times; a length is pixels. Zero, negative, functions and nonsense give nothing.
fn line_height(value: &str) -> Option<Option<LineHeight>> {
    let value = value.trim();
    if value == "normal" {
        return Some(None);
    }
    if value.contains('(') {
        return None;
    }
    let (n, unit) = dimension(value)?;
    if n <= 0.0 {
        return None;
    }
    let scale =
        |hundredths: f32| Some(Some(LineHeight::Scale(hundredths.round().clamp(f32::from(MIN_LINE_HEIGHT), f32::from(MAX_LINE_HEIGHT)) as u16)));
    match unit {
        "" => scale(n * 100.0),
        "%" => scale(n),
        "em" | "rem" => scale(n * 100.0),
        _ => length_px(value).map(|px| Some(LineHeight::Px(px.round().clamp(1.0, 200.0) as u8))),
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
            "position:absolute; left:-9999px; float:left; line-height:0; \
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

    /// #71: every value of the box properties is held to its range.
    fn in_range(css: &Css) -> bool {
        css.padding.iter().chain(&css.margin).flatten().all(|px| *px <= MAX_BOX_PX)
            && css.radius <= MAX_RADIUS_PX
            && css.lines.iter().all(|l| l.width <= MAX_LINE_PX)
            && css.borders.iter().zip(&css.lines).all(|(b, l)| *b == l.is_set())
            && match css.line_height {
                Some(Some(LineHeight::Scale(n))) => (MIN_LINE_HEIGHT..=MAX_LINE_HEIGHT).contains(&n),
                Some(Some(LineHeight::Px(n))) => (1..=200).contains(&n),
                _ => true,
            }
    }

    #[test]
    fn padding_is_read_per_side_and_clamped() {
        let padding = |style: &str| Css::parse(style).padding;
        assert_eq!(padding("padding:10px"), [Some(10); 4]);
        assert_eq!(padding("padding:4px 8px"), [Some(4), Some(8), Some(4), Some(8)]);
        assert_eq!(padding("padding:1px 2px 3px"), [Some(1), Some(2), Some(3), Some(2)]);
        assert_eq!(padding("padding:1px 2px 3px 4px"), [Some(1), Some(2), Some(3), Some(4)]);
        assert_eq!(padding("padding:0"), [Some(0); 4]);
        // Units: points, em (of the base size) and a share of a 600 pixel page.
        assert_eq!(padding("padding:6pt 1em 0.5em 5%"), [Some(8), Some(13), Some(7), Some(30)]);
        // A side on its own, after the shorthand.
        assert_eq!(padding("padding:10px;padding-left:20px;padding-top:0"), [Some(0), Some(10), Some(10), Some(20)]);
        assert_eq!(padding("padding-bottom:12px"), [None, None, Some(12), None]);
        // Clamped.
        assert_eq!(padding("padding:9999px"), [Some(MAX_BOX_PX); 4]);
        assert_eq!(padding("padding:100em 99999% 99999px 3in"), [Some(MAX_BOX_PX); 4]);
        let css = Css::parse("padding:5px 0 0 7px");
        assert_eq!(css.padding_insets(), Some(Insets { top: 5, right: 0, bottom: 0, left: 7 }));
        assert_eq!(Css::parse("color:red").padding_insets(), None);
    }

    #[test]
    fn hostile_padding_is_ignored() {
        for bad in [
            "padding:calc(10px + 5px)",
            "padding:var(--gap)",
            "padding:env(safe-area-inset-top)",
            "padding:expression(alert(1))",
            "padding:url(x.png)",
            "padding:-5px",
            "padding:10",
            "padding:auto",
            "padding:1px 2px 3px 4px 5px",
            "padding:1px 2px bogus",
            "padding:inherit",
            "padding:",
            "padding:1e3px",
            "padding:NaN",
            "padding:infinitypx",
        ] {
            assert_eq!(Css::parse(bad).padding, [None; 4], "{bad}");
        }
        // A bad part of the shorthand spoils the whole declaration, not the earlier one.
        assert_eq!(Css::parse("padding:3px;padding:calc(1px)").padding, [Some(3); 4]);
    }

    #[test]
    fn margins_have_real_sizes_and_auto_is_for_centring() {
        let margin = |style: &str| Css::parse(style).margin;
        assert_eq!(margin("margin:0 0 16px"), [Some(0), Some(0), Some(16), Some(0)]);
        assert_eq!(margin("margin:10px auto"), [Some(10), None, Some(10), None]);
        assert_eq!(margin("margin-top:8px;margin-left:12px"), [Some(8), None, None, Some(12)]);
        assert_eq!(margin("margin:-30px"), [Some(0); 4]);
        assert_eq!(margin("margin:500px"), [Some(MAX_BOX_PX); 4]);
        assert_eq!(margin("margin:2em"), [Some(26); 4]);
        assert_eq!(margin("margin:calc(1px) 3px"), [None; 4]);
        assert_eq!(margin("margin:1px 2px 3px 4px 5px"), [None; 4]);
        let css = Css::parse("margin:4px 8px 12px 16px");
        assert_eq!(css.margin_insets(), Insets { top: 4, right: 8, bottom: 12, left: 16 });
        // What was read before still is.
        assert!(Css::parse("margin:0 auto").centred());
        assert_eq!(Css::parse("margin:0").margin_top_zero, Some(true));
        assert_eq!(Css::parse("margin:20px").margin_top_zero, Some(false));
    }

    #[test]
    fn lines_have_a_width_a_style_and_a_colour_per_side() {
        let line = |width, style, color| Line { width, style, color };
        let grey = Some(Rgb(0xcc, 0xcc, 0xcc));
        let lines = Css::parse("border:1px solid #ccc").lines;
        assert_eq!(lines, [line(1, LineStyle::Solid, grey); 4]);
        let css = Css::parse("border:2px dashed #ccc;border-bottom:3px dotted red;border-left:none");
        assert_eq!(css.lines[0], line(2, LineStyle::Dashed, grey));
        assert_eq!(css.lines[2], line(3, LineStyle::Dotted, Some(Rgb(255, 0, 0))));
        assert_eq!(css.lines[3], Line::default());
        assert_eq!(css.borders, [true, true, true, false]);
        // The parts, and 1-4 values for the sides.
        let css = Css::parse("border-width:1px 2px 0 4px;border-style:solid;border-color:#111 #222");
        assert_eq!(
            css.lines,
            [
                line(1, LineStyle::Solid, Some(Rgb(0x11, 0x11, 0x11))),
                line(2, LineStyle::Solid, Some(Rgb(0x22, 0x22, 0x22))),
                Line::default(),
                line(4, LineStyle::Solid, Some(Rgb(0x22, 0x22, 0x22))),
            ]
        );
        assert_eq!(
            Css::parse("border-top-width:2px;border-top-style:dashed;border-top-color:blue").lines[0],
            line(2, LineStyle::Dashed, Some(Rgb(0, 0, 255)))
        );
        // A width is held to 1-4 pixels (a hair is one), the odd styles are solid, a colour that is
        // not a colour is the reader's own, and a clear one shows nothing.
        assert_eq!(Css::parse("border:0.2px solid red").lines[0].width, 1);
        assert_eq!(Css::parse("border:40px solid red").lines[0].width, MAX_LINE_PX);
        assert_eq!(Css::parse("border:thin double red").lines[0], line(1, LineStyle::Solid, Some(Rgb(255, 0, 0))));
        assert_eq!(Css::parse("border:medium groove").lines[0], line(3, LineStyle::Solid, None));
        assert_eq!(Css::parse("border:thick solid").lines[0].width, 4);
        assert_eq!(Css::parse("border:solid windowtext 1.0pt").lines[0], line(1, LineStyle::Solid, None));
        assert!(!Css::parse("border:1px solid transparent").lines[0].is_set());
        assert!(!Css::parse("border:0;border-top:hidden").lines.iter().any(Line::is_set));
        // A colour alone is not a line; a width or a style alone is, as it has been.
        assert!(!Css::parse("border-color:red").lines[0].is_set());
        assert!(Css::parse("border-width:2px").lines[0].is_set());
        assert!(Css::parse("border-style:dashed").lines[0].is_set());
        assert!(!Css::parse("border:1px solid;border-bottom:none").framed());
    }

    #[test]
    fn hostile_lines_are_ignored() {
        for bad in [
            "border:calc(1px + 1px) solid red",
            "border:1px solid var(--c)",
            "border:url(x.png)",
            "border:expression(1)",
            "border:1px solid red blue green orange yellow pink purple",
            "border:inherit",
            "border:-4px solid red",
            "border-width:1px 2px 3px 4px 5px",
            "border-color:rgb(((",
            "border-style:sunken",
            "border-bottom:wavy",
        ] {
            let css = Css::parse(bad);
            assert!(!css.lines.iter().any(Line::is_set), "{bad}");
            assert!(in_range(&css));
        }
    }

    #[test]
    fn corners_are_round_up_to_a_limit() {
        let radius = |style: &str| Css::parse(style).radius;
        assert_eq!(radius("border-radius:6px"), 6);
        assert_eq!(radius("border-radius:4px 8px 12px 16px"), 4);
        assert_eq!(radius("border-radius:8px / 20px"), 8);
        assert_eq!(radius("border-radius:999px"), MAX_RADIUS_PX);
        assert_eq!(radius("border-radius:50%"), MAX_RADIUS_PX);
        assert_eq!(radius("border-radius:10%"), 5);
        assert_eq!(radius("border-radius:0.5em"), 7);
        assert_eq!(radius("border-radius:6pt"), 8);
        assert_eq!(radius("border-radius:-3px"), 0);
        assert_eq!(radius("border-radius:calc(4px)"), 0);
        assert_eq!(radius("border-radius:var(--r)"), 0);
        assert_eq!(radius("border-radius:big"), 0);
        // A later bad one leaves the earlier.
        assert_eq!(radius("border-radius:6px;border-radius:calc(1px)"), 6);
    }

    #[test]
    fn line_height_is_a_multiple_of_the_font_or_a_length() {
        let height = |style: &str| Css::parse(style).line_height;
        let scale = |n| Some(Some(LineHeight::Scale(n)));
        assert_eq!(height("line-height:1.5"), scale(150));
        assert_eq!(height("line-height:150%"), scale(150));
        assert_eq!(height("line-height:1.25em"), scale(125));
        assert_eq!(height("line-height:24px"), Some(Some(LineHeight::Px(24))));
        assert_eq!(height("line-height:18pt"), Some(Some(LineHeight::Px(24))));
        assert_eq!(height("line-height:normal"), Some(None));
        // Held to 1-2.5 times.
        assert_eq!(height("line-height:0.5"), scale(MIN_LINE_HEIGHT));
        assert_eq!(height("line-height:9"), scale(MAX_LINE_HEIGHT));
        assert_eq!(height("line-height:5000%"), scale(MAX_LINE_HEIGHT));
        assert_eq!(height("line-height:900px"), Some(Some(LineHeight::Px(200))));
        // Nothing for the rest.
        for bad in ["0", "-1", "calc(1.5)", "var(--lh)", "tall", "1.5 2", "NaN", ""] {
            assert_eq!(height(&format!("line-height:{bad}")), None, "{bad}");
        }
        // From the shorthand.
        assert_eq!(height("font:12px/1.6 Arial, sans-serif"), scale(160));
        assert_eq!(height("font:bold 14px/20px Arial"), Some(Some(LineHeight::Px(20))));
        assert_eq!(height("font:12px Arial"), None);
        // What the number means for a line of text: held to the range for the size.
        assert_eq!(LineHeight::Scale(150).of(10.0), 15.0);
        assert_eq!(LineHeight::Px(10).of(20.0), 20.0);
        assert_eq!(LineHeight::Px(200).of(10.0), 25.0);
        assert_eq!(LineHeight::Scale(100).of(f32::NAN), 1.0);
    }

    proptest::proptest! {
        #[test]
        fn any_style_parses(s in proptest::prelude::any::<String>()) {
            let css = Css::parse(&s);
            if let Some(size) = css.size.and_then(|s| s.resolve(None)) {
                proptest::prop_assert!((MIN_FONT_PX..=MAX_FONT_PX).contains(&size));
            }
            proptest::prop_assert!(in_range(&css));
        }

        #[test]
        fn box_declarations_stay_in_range(s in "((padding|padding-top|padding-left|margin|margin-right|margin-bottom|border|border-top|border-left|border-width|border-style|border-color|border-radius|line-height|font) ?: ?[#a-z0-9.%()+/,;:'\" !-]{0,24};?){0,16}") {
            let css = Css::parse(&s);
            proptest::prop_assert!(in_range(&css));
            let _ = (css.padding_insets(), css.margin_insets(), css.framed());
        }

        #[test]
        fn numbers_and_units_stay_in_range(
            n in "[-+]?[0-9]{0,18}(\\.[0-9]{0,18})?",
            unit in "(px|pt|em|rem|%|in|cm|mm|pc|vh|ch|fr|)",
            property in "(padding|margin|border-width|border-radius|line-height|border)",
        ) {
            let css = Css::parse(&format!("{property}:{n}{unit}"));
            proptest::prop_assert!(in_range(&css));
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
