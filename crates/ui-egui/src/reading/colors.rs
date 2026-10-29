// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's colours on the window's colours.
//!
//! A message is written for a white page; the window may be dark, and the message may be out to
//! hide text. Two rules keep it readable, in both themes:
//!
//! 1. **Backgrounds.** In the light theme a background the message asks for is painted as asked,
//!    and so it is in the dark theme for a message whose style sheet brings its own design for
//!    dark windows (`@media (prefers-color-scheme: dark)`, [`Tokens::sender_dark`]): the sender's
//!    palette is used, and only text whose contrast falls short (rule 2) is adjusted.
//!    In the dark theme a light one (lightness over a half) is turned dark by inverting its
//!    lightness, keeping its hue and muting it a little, so a white card becomes the window's own
//!    surface and a pale band a slightly lighter one; a dark or strongly coloured one stays.
//! 2. **Text.** Every text colour is checked against the background that is really behind it
//!    (the window's surface, or the block's own background after rule 1). A colour with a contrast
//!    under [`MIN_CONTRAST`] is first turned over the same way, its lightness inverted and its hue
//!    kept: black text in a dark window becomes white, Outlook's dark blue a light blue, and a
//!    message's darker and lighter greys keep their order. If that is still not enough, it is moved
//!    towards black or white, whichever the background is further from, until it reaches
//!    [`TARGET_CONTRAST`]. Text that names no colour takes the window's text colour on the
//!    window's surface, and near black or near white on a background of the message's own. So
//!    nothing is invisible: not white on white, not black in a dark window.

use egui::Color32;
use mailcraft_html::Rgb;

use crate::theme::Tokens;

/// The lowest contrast (WCAG 2 ratio, 1 to 21) at which a message's own text colour is used as it is.
pub const MIN_CONTRAST: f32 = 3.0;

/// The contrast a colour that falls short is brought to: what WCAG asks of body text.
pub const TARGET_CONTRAST: f32 = 4.5;

/// The least contrast a line round a box keeps with the paper it is drawn on: a line is not text,
/// so far less than [`MIN_CONTRAST`], but never invisible.
pub const LINE_CONTRAST: f32 = 1.5;

pub fn c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// Relative luminance, 0 (black) to 1 (white).
pub fn luminance(c: Color32) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// The contrast ratio of two opaque colours, 1 (the same) to 21 (black on white).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

/// `ink` as it can be read on `paper`: itself when the contrast is enough; else with its
/// lightness turned over (the colour a dark-on-light design becomes light-on-dark, and the other
/// way round); else moved towards black or white until it can be read.
pub fn readable(ink: Color32, paper: Color32) -> Color32 {
    if contrast(ink, paper) >= MIN_CONTRAST {
        return ink;
    }
    let (h, s, l) = hsl(ink);
    let turned = from_hsl(h, s, 1.0 - l);
    if contrast(turned, paper) >= TARGET_CONTRAST {
        return turned;
    }
    at_least(if contrast(turned, paper) > contrast(ink, paper) { turned } else { ink }, paper, TARGET_CONTRAST)
}

/// `ink` itself when its contrast on `paper` is at least `min`, else moved towards black or white
/// until it reaches [`TARGET_CONTRAST`].
fn at_least(ink: Color32, paper: Color32, min: f32) -> Color32 {
    if contrast(ink, paper) >= min {
        return ink;
    }
    let end = if contrast(Color32::BLACK, paper) >= contrast(Color32::WHITE, paper) { Color32::BLACK } else { Color32::WHITE };
    for step in 1..=12 {
        let moved = mix(ink, end, step as f32 / 12.0);
        if contrast(moved, paper) >= TARGET_CONTRAST {
            return moved;
        }
    }
    end
}

/// Hue (0 to 6), saturation and lightness of a colour.
fn hsl(c: Color32) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(c.r()) / 255.0, f32::from(c.g()) / 255.0, f32::from(c.b()) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6);
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h, s.clamp(0.0, 1.0), l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> Color32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h.rem_euclid(6.0) as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(byte(r), byte(g), byte(b))
}

/// What is behind a piece of the message: the window's own surface, or a background the message
/// asked for (already turned dark in the dark theme).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    pub fill: Color32,
    /// The message asked for it.
    pub own: bool,
}

impl Paper {
    /// The window's surface.
    pub fn window(t: &Tokens) -> Paper {
        Paper { fill: t.surface, own: false }
    }

    /// A background the message asks for, as it is painted in this theme.
    pub fn of(asked: Rgb, t: &Tokens) -> Paper {
        let asked = c32(asked);
        // (A message with its own design for dark windows has its backgrounds as it asks.)
        if !t.dark || t.sender_dark {
            return Paper { fill: asked, own: true };
        }
        let (h, s, l) = hsl(asked);
        if l <= 0.5 {
            return Paper { fill: asked, own: true };
        }
        // White lands on the window's surface; a half stays a half, so nothing jumps there.
        let floor = hsl(t.surface).2.clamp(0.0, 0.4);
        let dark = floor + (1.0 - l) * (1.0 - 2.0 * floor);
        Paper { fill: from_hsl(h, s * 0.6, dark), own: true }
    }

    /// The colour of text that names none.
    pub fn text(&self, t: &Tokens, quoted: bool) -> Color32 {
        if !self.own {
            return if quoted { t.text_dim } else { t.text };
        }
        let (dark, light) = (Tokens::light().text, Tokens::dark().text);
        let best = if contrast(dark, self.fill) >= contrast(light, self.fill) { dark } else { light };
        // On a mid-tone neither is enough: then it is black or white.
        at_least(best, self.fill, TARGET_CONTRAST)
    }

    /// The colour of a link that names none.
    pub fn link(&self, t: &Tokens) -> Color32 {
        if !self.own {
            return t.accent_text;
        }
        let (dark, light) = (Tokens::light().accent_text, Tokens::dark().accent_text);
        readable(if contrast(dark, self.fill) >= contrast(light, self.fill) { dark } else { light }, self.fill)
    }

    /// The colour of a rule, a quote bar or a cell border.
    pub fn line(&self, t: &Tokens) -> Color32 {
        if self.own { mix(self.fill, self.text(t, false), 0.28) } else { t.border_strong }
    }

    /// The colour of a line round a box that asks for `asked`: as a background colour is turned
    /// for this theme, and then kept [`LINE_CONTRAST`] apart from the paper it is drawn on, so it
    /// is seen in light and dark and on any colour. The reader's own line colour when it asks for none.
    pub fn border(&self, t: &Tokens, asked: Option<Rgb>) -> Color32 {
        let Some(asked) = asked else { return self.line(t) };
        let turned = Paper::of(asked, t).fill;
        if contrast(turned, self.fill) >= LINE_CONTRAST {
            return turned;
        }
        let end = if contrast(Color32::BLACK, self.fill) >= contrast(Color32::WHITE, self.fill) { Color32::BLACK } else { Color32::WHITE };
        (1..=12).map(|step| mix(turned, end, step as f32 / 12.0)).find(|moved| contrast(*moved, self.fill) >= LINE_CONTRAST).unwrap_or(end)
    }

    /// The colour a run of text is painted in: the one it asks for when that can be read here,
    /// else that one adjusted; a link's or the text's own when it asks for none.
    pub fn ink(&self, t: &Tokens, asked: Option<Rgb>, link: bool, quoted: bool) -> Color32 {
        match asked {
            Some(asked) => readable(c32(asked), self.fill),
            None if link => self.link(t),
            None => self.text(t, quoted),
        }
    }

    /// The subtle shade of a header cell or a placeholder on this paper.
    pub fn shade(&self, t: &Tokens) -> Color32 {
        if self.own { mix(self.fill, self.text(t, false), 0.08) } else { t.panel }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(hex: u32) -> Color32 {
        Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    #[test]
    fn contrast_is_the_wcag_ratio() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
        // Mid grey on white, a well-known value.
        assert!((contrast(rgb(0x767676), Color32::WHITE) - 4.54).abs() < 0.02);
        assert_eq!(contrast(rgb(0x123456), rgb(0xabcdef)), contrast(rgb(0xabcdef), rgb(0x123456)));
    }

    #[test]
    fn colours_that_can_be_read_are_left_alone() {
        for (ink, paper) in
            [(0x1f497d, 0xffffff), (0xc00000, 0xffffff), (0x000000, 0xffffff), (0xffffff, 0x0066cc), (0x7cc8e8, 0x1c1c1c), (0xeeeeee, 0x1c1c1c)]
        {
            assert_eq!(readable(rgb(ink), rgb(paper)), rgb(ink), "{ink:06x} on {paper:06x}");
        }
    }

    #[test]
    fn colours_that_cannot_be_read_are_adjusted_until_they_can() {
        for paper in [0xffffff, 0x1c1c1c, 0x000000, 0x808080, 0xffff00, 0x0066cc, 0xff0000, 0x003366, 0xf4f4f4, 0x777777] {
            for ink in
                [0xffffff, 0x000000, 0x1c1c1c, 0xfefefe, 0xffff00, 0x00ffff, 0x1f497d, 0x808080, 0x777777, 0xff0000, 0x00ff00, 0x003366, 0xc00000]
            {
                let shown = readable(rgb(ink), rgb(paper));
                let ratio = contrast(shown, rgb(paper));
                assert!(ratio >= MIN_CONTRAST, "{ink:06x} on {paper:06x} is shown as {shown:?}: {ratio}");
                if contrast(rgb(ink), rgb(paper)) < MIN_CONTRAST {
                    assert!(ratio >= TARGET_CONTRAST || shown == Color32::BLACK || shown == Color32::WHITE, "{ink:06x} on {paper:06x}: {ratio}");
                }
            }
        }
        // The hue is kept where it can be: Outlook's dark blue becomes a lighter blue in a dark window.
        let blue = readable(rgb(0x1f497d), rgb(0x1c1c1c));
        assert!(blue.b() > blue.r() && blue.b() > 0x7d, "{blue:?}");
        let red = readable(rgb(0xc00000), rgb(0x1c1c1c));
        assert!(red.r() > red.g() && red.r() > red.b(), "{red:?}");
        // Black in a dark window is white, white on a white page black, and greys keep their order.
        assert_eq!(readable(Color32::BLACK, rgb(0x1c1c1c)), Color32::WHITE);
        assert_eq!(readable(Color32::WHITE, Color32::WHITE), Color32::BLACK);
        let (strong, soft) = (readable(rgb(0x1c1c1c), rgb(0x1c1c1c)), readable(rgb(0x3d463b), rgb(0x1c1c1c)));
        assert!(luminance(strong) > luminance(soft) && contrast(soft, rgb(0x1c1c1c)) > 7.0, "{strong:?} {soft:?}");
    }

    #[test]
    fn backgrounds_are_kept_in_the_light_theme_and_turned_in_the_dark_one() {
        let (light, dark) = (Tokens::light(), Tokens::dark());
        for asked in
            [Rgb(255, 255, 255), Rgb(0xf4, 0xf4, 0xf4), Rgb(0xff, 0xf8, 0xe1), Rgb(0x00, 0x66, 0xcc), Rgb(0x11, 0x11, 0x11), Rgb(255, 255, 0)]
        {
            assert_eq!(Paper::of(asked, &light), Paper { fill: c32(asked), own: true });
        }
        // White becomes the window's surface, a pale grey a little lighter than that.
        let white = Paper::of(Rgb(255, 255, 255), &dark).fill;
        let pale = Paper::of(Rgb(0xf4, 0xf4, 0xf4), &dark).fill;
        assert!(contrast(white, dark.surface) < 1.05, "{white:?}");
        assert!(luminance(pale) > luminance(white) && luminance(pale) < 0.05, "{pale:?}");
        // A tint keeps its hue, muted: pale yellow is a dark yellow-brown, not grey and not bright.
        let note = Paper::of(Rgb(0xff, 0xf8, 0xe1), &dark).fill;
        assert!(note.r() > note.b() && note.g() > note.b() && luminance(note) < 0.08, "{note:?}");
        // What is dark or strongly coloured already stays as it is.
        for asked in [Rgb(0x00, 0x66, 0xcc), Rgb(0x11, 0x11, 0x11), Rgb(0x2f, 0x4f, 0x3a), Rgb(0xc8, 0x55, 0x2b), Rgb(255, 0, 0)] {
            assert_eq!(Paper::of(asked, &dark).fill, c32(asked));
        }
        // Lightness turns over smoothly: no colour lands far from its neighbour.
        let mut last = Paper::of(Rgb(0, 0, 0), &dark).fill;
        for v in 1..=255u8 {
            let now = Paper::of(Rgb(v, v, v), &dark).fill;
            assert!((i32::from(now.r()) - i32::from(last.r())).abs() <= 3, "{v}: {last:?} then {now:?}");
            last = now;
        }
    }

    #[test]
    fn text_and_links_can_be_read_on_every_paper() {
        for t in [Tokens::light(), Tokens::dark()] {
            let mut papers = vec![Paper::window(&t)];
            for r in [0u8, 0x33, 0x80, 0xcc, 0xff] {
                for g in [0u8, 0x80, 0xff] {
                    for b in [0u8, 0x66, 0xff] {
                        papers.push(Paper::of(Rgb(r, g, b), &t));
                    }
                }
            }
            for paper in papers {
                for quoted in [false, true] {
                    assert!(contrast(paper.text(&t, quoted), paper.fill) >= TARGET_CONTRAST, "text on {paper:?}");
                }
                assert!(contrast(paper.link(&t), paper.fill) >= MIN_CONTRAST, "link on {paper:?}: {:?}", paper.link(&t));
                for asked in [Rgb(255, 255, 255), Rgb(0, 0, 0), Rgb(0x1c, 0x1c, 0x1c), Rgb(255, 255, 0), Rgb(0x1f, 0x49, 0x7d), Rgb(0x80, 0x80, 0x80)]
                {
                    for link in [false, true] {
                        let ink = paper.ink(&t, Some(asked), link, false);
                        assert!(contrast(ink, paper.fill) >= MIN_CONTRAST, "{asked:?} on {paper:?} is {ink:?}");
                    }
                }
                assert!(contrast(paper.line(&t), paper.fill) > 1.1, "a line on {paper:?}");
            }
        }
    }

    #[test]
    fn hsl_round_trips() {
        for hex in [0x000000, 0xffffff, 0x808080, 0xff0000, 0x00ff00, 0x0000ff, 0x1f497d, 0xfff8e1, 0xc8552b, 0x2f4f3a, 0xeef1ec] {
            let (h, s, l) = hsl(rgb(hex));
            let back = from_hsl(h, s, l);
            let near = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs() <= 1;
            assert!(near(back.r(), rgb(hex).r()) && near(back.g(), rgb(hex).g()) && near(back.b(), rgb(hex).b()), "{hex:06x} came back as {back:?}");
        }
    }
}
