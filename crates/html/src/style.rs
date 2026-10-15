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
//! | hidden | `display:none`, `visibility:hidden`, `opacity:0`, `max-height:0`, `max-width:0`, a zero `height` or `width` with `overflow:hidden` |
//! | text too small to read | a font size of 2 pixels or less: the text at that size is not shown; what is inside at a size of its own is, and so are pictures, and so is the text of an element with a `class` or an `id` (or inside one that asks for the size and has one): its size is given in a style sheet, and style sheets are not read |
//! | a margin or none | `margin`, `margin-top`, `margin-bottom`: whether there is one to speak of (over 6 pixels) |
//! | room between blocks | `margin`, `margin-top`, `margin-bottom` in pixels, points or `em`, up to 64 pixels |
//! | centring | `margin-left` and `margin-right` (or `margin`) both `auto` |
//! | padding | `padding` and its sides, in pixels, points or `em`, up to 64 pixels a side |
//! | lines round a box | `border`, its sides, `border-width`, `border-style`, `border-color` and their sides: whether there is one, its width (1 to 4 pixels), solid or dashed, and its colour |
//! | rounded corners | `border-radius` (its first length), up to 24 pixels |
//! | line height | `line-height` as a number, a percentage, `em` or pixels, 1 to 2.5 times the font size |
//! | Outlook's lists | `mso-list` (the level, and the marker text to replace) |
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

/// A margin: a length in CSS pixels, or `auto` (what centres a box between two of them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Margin {
    Px(f32),
    Auto,
}

/// How a line round a box is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineStyle {
    /// `none` or `hidden`: no line.
    None,
    Solid,
    /// Dashed or dotted.
    Dashed,
}

/// What a message says of the line along one side of a box, before it is settled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LineSpec {
    /// In CSS pixels, as asked (not capped: a thick line of the box's own colour is room).
    pub width: Option<f32>,
    pub style: Option<LineStyle>,
    pub color: Option<Rgb>,
}

/// A line height before it is resolved against the font size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LineHeight {
    /// Times the font size.
    Scale(f32),
    /// CSS pixels.
    Px(f32),
}

impl LineHeight {
    /// In tenths of a font size of `size` pixels, within what is readable.
    pub fn tenths(self, size: Option<u8>) -> u8 {
        let scale = match self {
            LineHeight::Scale(n) => n,
            LineHeight::Px(px) => px / size.map_or(BASE_FONT_PX, f32::from).max(1.0),
        };
        let tenths = if scale.is_finite() { (scale * 10.0).round() } else { 0.0 };
        tenths.clamp(f32::from(crate::MIN_LINE_HEIGHT), f32::from(crate::MAX_LINE_HEIGHT)) as u8
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
    pub hidden: bool,
    /// Whether the margin above (below) is given, and given as none to speak of: zero, or a
    /// few pixels (a line's own leading is more than that).
    pub margin_top_zero: Option<bool>,
    pub margin_bottom_zero: Option<bool>,
    /// Borders: top, right, bottom, left.
    pub borders: [bool; 4],
    /// The same lines, as described: width, style, colour (top, right, bottom, left).
    pub lines: [LineSpec; 4],
    /// Padding, in CSS pixels (top, right, bottom, left), where given.
    pub padding: [Option<f32>; 4],
    /// Margins, where given in a length or `auto` (top, right, bottom, left).
    pub margins: [Option<Margin>; 4],
    /// Rounded corners, in CSS pixels.
    pub radius: Option<f32>,
    pub line_height: Option<LineHeight>,
    pub mso_list: Option<MsoList>,
    /// The width and height when given in absolute units, in CSS pixels (for telling a tracking
    /// pixel; never used to size anything).
    pub width: Option<f32>,
    pub height: Option<f32>,
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
                "display" => css.hidden |= value == "none",
                "visibility" => css.hidden |= matches!(value, "hidden" | "collapse"),
                "opacity" => css.hidden |= number(value.trim_end_matches('%')).is_some_and(|n| n <= 0.0),
                // (`mso-hide:all` hides nothing here: see the notes at the top.)
                "max-height" | "max-width" => css.hidden |= is_zero(value) == Some(true),
                "overflow" | "overflow-x" | "overflow-y" => clipped |= tokens(value).any(|t| matches!(t, "hidden" | "clip")),
                "height" => {
                    css.height = length_px(value).or(css.height);
                    zero_box |= is_zero(value) == Some(true);
                }
                "width" => {
                    css.width = length_px(value).or(css.width);
                    zero_box |= is_zero(value) == Some(true);
                }
                "margin" => {
                    let parts: Vec<&str> = tokens(value).take(4).collect();
                    let top = parts.first().copied().and_then(is_slight);
                    let bottom = parts.get(2).copied().and_then(is_slight).or(if parts.len() < 3 { top } else { None });
                    css.margin_top_zero = top.or(css.margin_top_zero);
                    css.margin_bottom_zero = bottom.or(css.margin_bottom_zero);
                    if let Some(sides) = four(&parts.iter().map(|p| margin(p)).collect::<Vec<_>>()) {
                        css.margins = sides;
                    }
                }
                "margin-top" | "margin-block-start" => {
                    css.margin_top_zero = is_slight(value).or(css.margin_top_zero);
                    css.margins[0] = margin(value).or(css.margins[0]);
                }
                "margin-bottom" | "margin-block-end" => {
                    css.margin_bottom_zero = is_slight(value).or(css.margin_bottom_zero);
                    css.margins[2] = margin(value).or(css.margins[2]);
                }
                "margin-left" | "margin-inline-start" => css.margins[3] = margin(value).or(css.margins[3]),
                "margin-right" | "margin-inline-end" => css.margins[1] = margin(value).or(css.margins[1]),
                "padding" => {
                    let parts: Vec<Option<f32>> = tokens(value).take(4).map(room_px).collect();
                    if let Some(sides) = four(&parts) {
                        css.padding = sides;
                    }
                }
                "padding-top" => css.padding[0] = room_px(value).or(css.padding[0]),
                "padding-right" => css.padding[1] = room_px(value).or(css.padding[1]),
                "padding-bottom" => css.padding[2] = room_px(value).or(css.padding[2]),
                "padding-left" => css.padding[3] = room_px(value).or(css.padding[3]),
                "border-color" => {
                    let parts: Vec<Option<Rgb>> = tokens(value).take(4).map(color).collect();
                    if let Some(sides) = four(&parts) {
                        for (spec, c) in css.lines.iter_mut().zip(sides) {
                            spec.color = c.or(spec.color);
                        }
                    }
                }
                "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => {
                    if let (Some(c), Some(spec)) = (color(value), css.lines.get_mut(side_of(&property))) {
                        spec.color = Some(c);
                    }
                }
                "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" => {
                    if let (Some(w), Some(spec)) = (line_width(value), css.lines.get_mut(side_of(&property))) {
                        spec.width = Some(w);
                    }
                }
                "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {
                    if let (Some(st), Some(spec)) = (line_style(value), css.lines.get_mut(side_of(&property))) {
                        spec.style = Some(st);
                    }
                }
                "border-radius" => css.radius = tokens(value).next().and_then(room_px).or(css.radius),
                "line-height" => css.line_height = line_height(value).or(css.line_height),
                "border" | "border-style" | "border-width" => {
                    if let Some(on) = has_border(value) {
                        css.borders = [on; 4];
                    }
                    match property.as_str() {
                        "border" => css.lines = [line_spec(value); 4],
                        "border-style" => {
                            let parts: Vec<Option<LineStyle>> = tokens(value).take(4).map(line_style).collect();
                            if let Some(sides) = four(&parts) {
                                for (spec, st) in css.lines.iter_mut().zip(sides) {
                                    spec.style = st.or(spec.style);
                                }
                            }
                        }
                        _ => {
                            let parts: Vec<Option<f32>> = tokens(value).take(4).map(line_width).collect();
                            if let Some(sides) = four(&parts) {
                                for (spec, w) in css.lines.iter_mut().zip(sides) {
                                    spec.width = w.or(spec.width);
                                }
                            }
                        }
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
                    if let Some(spec) = css.lines.get_mut(side) {
                        *spec = line_spec(value);
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

    /// Whether every side has a border.
    pub fn framed(&self) -> bool {
        self.borders.iter().all(|b| *b)
    }

    /// The line along side `side` (0 top, 1 right, 2 bottom, 3 left), as it is drawn: whether
    /// there is one (as [`Css::borders`] says), its width in pixels as asked (1 when it says
    /// none), whether it is dashed, and its colour.
    pub fn line(&self, side: usize) -> Option<(f32, bool, Option<Rgb>)> {
        if !self.borders.get(side).copied().unwrap_or(false) {
            return None;
        }
        let spec = self.lines.get(side).copied().unwrap_or_default();
        if spec.style == Some(LineStyle::None) {
            return None;
        }
        let width = spec.width.unwrap_or(1.0);
        if !width.is_finite() || width <= 0.0 {
            return None;
        }
        Some((width, spec.style == Some(LineStyle::Dashed), spec.color))
    }

    /// The padding, in whole pixels within [`crate::MAX_ROOM_PX`]; `None` when none is given.
    pub fn padding_px(&self) -> Option<crate::Edges> {
        if self.padding.iter().all(Option::is_none) {
            return None;
        }
        let px = |v: Option<f32>| clamp_room(v.unwrap_or(0.0));
        let [top, right, bottom, left] = self.padding;
        Some(crate::Edges { top: px(top), right: px(right), bottom: px(bottom), left: px(left) })
    }

    /// The margin above (0) or below (2), in whole pixels within [`crate::MAX_ROOM_PX`]; `None`
    /// when it is not given as a length.
    pub fn margin_px(&self, side: usize) -> Option<u8> {
        match self.margins.get(side).copied().flatten()? {
            Margin::Px(px) => Some(clamp_room(px)),
            Margin::Auto => None,
        }
    }

    /// Whether the box is centred between left and right margins that are both `auto`.
    pub fn centred(&self) -> bool {
        matches!(self.margins, [_, Some(Margin::Auto), _, Some(Margin::Auto)])
    }

    /// Rounded corners in whole pixels, within [`crate::MAX_RADIUS_PX`].
    pub fn radius_px(&self) -> u8 {
        self.radius.map_or(0, |r| if r.is_finite() { r.round().clamp(0.0, f32::from(crate::MAX_RADIUS_PX)) as u8 } else { 0 })
    }
}

/// A number of pixels as room within [`crate::MAX_ROOM_PX`]: nothing for less than nothing.
pub(crate) fn clamp_room(px: f32) -> u8 {
    if px.is_finite() { px.round().clamp(0.0, f32::from(crate::MAX_ROOM_PX)) as u8 } else { 0 }
}

/// One to four values for the four sides, as CSS writes them (top, right, bottom, left).
fn four<T: Copy>(values: &[T]) -> Option<[T; 4]> {
    match *values {
        [a] => Some([a, a, a, a]),
        [a, b] => Some([a, b, a, b]),
        [a, b, c] => Some([a, b, c, b]),
        [a, b, c, d] => Some([a, b, c, d]),
        _ => None,
    }
}

/// The side a `border-<side>-…` property is about (0 top, 1 right, 2 bottom, 3 left).
fn side_of(property: &str) -> usize {
    if property.contains("-top") {
        0
    } else if property.contains("-right") {
        1
    } else if property.contains("-bottom") {
        2
    } else {
        3
    }
}

/// A length as room: pixels (or points and the like), or `em` against the usual font size; less
/// than nothing is nothing. `None` for anything else (`auto`, a percentage).
fn room_px(value: &str) -> Option<f32> {
    let (n, unit) = dimension(value)?;
    let px = match unit {
        "em" | "rem" => n * BASE_FONT_PX,
        _ => length_px(value)?,
    };
    px.is_finite().then_some(px.max(0.0))
}

/// A margin: a length, or `auto`.
fn margin(value: &str) -> Option<Margin> {
    if value.trim() == "auto" { Some(Margin::Auto) } else { room_px(value).map(Margin::Px) }
}

/// A line's width: a length, or `thin`, `medium`, `thick`.
fn line_width(value: &str) -> Option<f32> {
    match value.trim() {
        "thin" => Some(1.0),
        "medium" => Some(3.0),
        "thick" => Some(5.0),
        other => length_px(other).map(|px| px.max(0.0)),
    }
}

/// A line's style: none, solid or dashed (the other drawn styles are solid here, `dotted` is dashed).
fn line_style(value: &str) -> Option<LineStyle> {
    match value.trim() {
        "none" | "hidden" => Some(LineStyle::None),
        "dashed" | "dotted" => Some(LineStyle::Dashed),
        "solid" | "double" | "groove" | "ridge" | "inset" | "outset" => Some(LineStyle::Solid),
        _ => None,
    }
}

/// A `border` shorthand: its width, style and colour, in any order.
fn line_spec(value: &str) -> LineSpec {
    let mut spec = LineSpec::default();
    for token in tokens(value).take(8) {
        if let Some(st) = line_style(token) {
            spec.style = Some(st);
        } else if let Some(w) = line_width(token) {
            spec.width = Some(w);
        } else if let Some(c) = color(token) {
            spec.color = Some(c);
        }
    }
    spec
}

/// A `line-height` value; `normal` and anything unreadable give `None`.
fn line_height(value: &str) -> Option<LineHeight> {
    let (n, unit) = dimension(value)?;
    if n <= 0.0 {
        return None;
    }
    match unit {
        "" => Some(LineHeight::Scale(n)),
        "em" | "rem" => Some(LineHeight::Scale(n)),
        "%" => Some(LineHeight::Scale(n / 100.0)),
        _ => length_px(value).map(LineHeight::Px),
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
    fn everything_else_is_ignored() {
        let css = Css::parse(
            "position:absolute; left:-9999px; float:left; font-family:'Comic Sans MS'; line-height:0; \
             background-image:url(https://tracker.example/p.gif); content:'x'; behavior:url(x.htc); width:50%; z-index:9; \
             transform:scale(9); animation:spin 1s; letter-spacing:4em; text-transform:uppercase",
        );
        assert_eq!(css, Css::default());
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

    #[test]
    fn padding_reads_one_to_four_values_capped() {
        let pad = |style: &str| Css::parse(style).padding_px().map(|e| (e.top, e.right, e.bottom, e.left));
        assert_eq!(pad("padding:32px"), Some((32, 32, 32, 32)));
        assert_eq!(pad("padding:25px 0"), Some((25, 0, 25, 0)));
        assert_eq!(pad("padding:1px 2px 3px"), Some((1, 2, 3, 2)));
        assert_eq!(pad("padding:1px 2px 3px 4px"), Some((1, 2, 3, 4)));
        assert_eq!(pad("padding:1em"), Some((13, 13, 13, 13)));
        assert_eq!(pad("padding:12pt 0"), Some((16, 0, 16, 0)));
        // Capped, never less than nothing, and what isn't a length says nothing.
        assert_eq!(pad("padding:9999px"), Some((64, 64, 64, 64)));
        assert_eq!(pad("padding:-20px"), Some((0, 0, 0, 0)));
        assert_eq!(pad("padding:10%"), None);
        assert_eq!(pad("padding:1px 2px 3px 4px 5px"), Some((1, 2, 3, 4)));
        assert_eq!(pad("padding:auto; padding-top:8px"), Some((8, 0, 0, 0)));
        assert_eq!(pad("color:red"), None);
    }

    #[test]
    fn margins_give_room_and_auto_centres() {
        let css = Css::parse("margin:30px auto");
        assert_eq!((css.margin_px(0), css.margin_px(2), css.centred()), (Some(30), Some(30), true));
        assert!(Css::parse("margin-left:auto;margin-right:auto").centred());
        assert!(!Css::parse("margin-left:auto").centred());
        assert!(!Css::parse("margin:0 auto 0 0").centred());
        assert_eq!(Css::parse("margin-top:25px").margin_px(0), Some(25));
        assert_eq!(Css::parse("margin-top:500px").margin_px(0), Some(64));
        assert_eq!(Css::parse("margin-top:-8px").margin_px(0), Some(0));
        assert_eq!(Css::parse("margin-top:auto").margin_px(0), None);
        // (Whether there is a margin at all is read as before.)
        assert_eq!(Css::parse("margin:0").margin_top_zero, Some(true));
    }

    #[test]
    fn lines_have_a_width_a_style_and_a_colour() {
        let css = Css::parse("border:1px solid #e8e5ef");
        assert_eq!(css.line(0), Some((1.0, false, Some(Rgb(0xe8, 0xe5, 0xef)))));
        assert_eq!(css.line(3), css.line(0));
        let css = Css::parse("border-top:2px dashed red; border-bottom:none");
        assert_eq!(css.line(0), Some((2.0, true, Some(Rgb(255, 0, 0)))));
        assert_eq!((css.line(1), css.line(2)), (None, None));
        // Width, style and colour by themselves, side by side.
        let css = Css::parse("border-width:1px; border-color:#000 #fff; border-style:solid");
        assert_eq!(css.line(1), Some((1.0, false, Some(Rgb(255, 255, 255)))));
        assert_eq!(css.line(2).map(|l| l.2), Some(Some(Rgb(0, 0, 0))));
        // A thick line is as thick as asked here (the builder caps it, or makes it room).
        assert_eq!(Css::parse("border-left:18px solid #2d3748").line(3).map(|l| l.0), Some(18.0));
        assert_eq!(Css::parse("border:thin solid").line(0).map(|l| l.0), Some(1.0));
        assert_eq!(Css::parse("border:0").line(0), None);
        assert_eq!(Css::parse("border-style:hidden; border-width:3px").line(0), None);
        assert_eq!(Css::parse("border-top-color:blue").line(0), None, "a colour alone is no line");
    }

    #[test]
    fn corners_and_line_heights_are_capped() {
        assert_eq!(Css::parse("border-radius:4px").radius_px(), 4);
        assert_eq!(Css::parse("border-radius:999px").radius_px(), crate::MAX_RADIUS_PX);
        assert_eq!(Css::parse("border-radius:-3px").radius_px(), 0);
        assert_eq!(Css::parse("border-radius:50%").radius_px(), 0);
        let tenths = |style: &str, size: Option<u8>| Css::parse(style).line_height.map(|l| l.tenths(size));
        assert_eq!(tenths("line-height:1.5", None), Some(15));
        assert_eq!(tenths("line-height:1.5em", None), Some(15));
        assert_eq!(tenths("line-height:150%", None), Some(15));
        assert_eq!(tenths("line-height:24px", Some(16)), Some(15));
        assert_eq!(tenths("line-height:9", None), Some(crate::MAX_LINE_HEIGHT));
        assert_eq!(tenths("line-height:0.2", None), Some(crate::MIN_LINE_HEIGHT));
        assert_eq!(tenths("line-height:normal", None), None);
        assert_eq!(tenths("line-height:0", None), None);
        assert_eq!(tenths("line-height:-2", None), None);
        assert_eq!(tenths("line-height:9999999999999999999px", Some(10)), None);
    }

    proptest::proptest! {
        #[test]
        fn new_whitelist_values_stay_within_their_caps(s in "((padding|padding-top|margin|margin-left|margin-right|border|border-top|border-width|border-color|border-style|border-radius|line-height) ?: ?[#a-z0-9.%() ,;:-]{0,24};?){0,10}") {
            let css = Css::parse(&s);
            if let Some(p) = css.padding_px() {
                for side in [p.top, p.right, p.bottom, p.left] {
                    proptest::prop_assert!(side <= crate::MAX_ROOM_PX);
                }
            }
            for side in 0..4 {
                if let Some((width, _, _)) = css.line(side) {
                    proptest::prop_assert!(width.is_finite() && width > 0.0);
                }
            }
            proptest::prop_assert!(css.radius_px() <= crate::MAX_RADIUS_PX);
            proptest::prop_assert!(css.margin_px(0).unwrap_or(0) <= crate::MAX_ROOM_PX);
            if let Some(l) = css.line_height {
                let t = l.tenths(Some(16));
                proptest::prop_assert!((crate::MIN_LINE_HEIGHT..=crate::MAX_LINE_HEIGHT).contains(&t));
            }
        }

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
