// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The accent presets and high contrast: hand-written colour tables for every accent in light,
//! dark, high-contrast light and high-contrast dark, and the stronger neutrals of high contrast.
//! Nothing here is computed from a hue: each value was chosen and then checked by
//! `theme_contrast_tests.rs`.

use egui::{Color32, Stroke};

use super::Tokens;

fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn hex(v: u32) -> Color32 {
    c((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// The accent of a theme: the one colour that marks what is chosen, unread or in view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accent {
    #[default]
    Blue,
    Teal,
    Violet,
    Graphite,
    /// A colour of the owner's own: `custom` in `ui.json`, with the colour itself kept beside it
    /// (`customAccent`). Its ink is worked out from the colour, not looked up (see [`custom_ink`]).
    Custom,
}

/// The colour a custom accent starts with, when the owner has picked none yet.
pub const DEFAULT_CUSTOM_COLOR: Color32 = Color32::from_rgb(0xC2, 0x18, 0x5B);

impl Accent {
    /// In picker order.
    pub const ALL: [Accent; 5] = [Accent::Blue, Accent::Teal, Accent::Violet, Accent::Graphite, Accent::Custom];

    /// The value used in `ui.json` and by the `ui.accent` command.
    pub fn code(self) -> &'static str {
        match self {
            Accent::Blue => "blue",
            Accent::Teal => "teal",
            Accent::Violet => "violet",
            Accent::Graphite => "graphite",
            Accent::Custom => "custom",
        }
    }

    pub fn from_code(code: &str) -> Option<Accent> {
        Accent::ALL.into_iter().find(|a| a.code().eq_ignore_ascii_case(code.trim()))
    }

    /// The English name (translated with `tl!` where shown).
    pub fn label(self) -> &'static str {
        match self {
            Accent::Blue => "Blue",
            Accent::Teal => "Teal",
            Accent::Violet => "Violet",
            Accent::Graphite => "Graphite",
            Accent::Custom => "Custom color",
        }
    }

    /// The accent's colours for a window that is `dark` and `high_contrast` or not. A custom
    /// accent is worked out from `custom` against the neutral palette of that mode; the palette
    /// it is used on fits it to its own sheets in [`Tokens::build_choice`].
    pub fn ink(self, dark: bool, high_contrast: bool, custom: Option<Color32>) -> AccentInk {
        let row = |a, at, oa, soft, st, sel, today| AccentInk {
            accent: hex(a),
            text: hex(at),
            on_accent: hex(oa),
            soft: hex(soft),
            soft_text: hex(st),
            selection: hex(sel),
            today: hex(today),
        };
        match (self, dark, high_contrast) {
            (Accent::Blue, false, false) => row(0x0F68F0, 0x0634C4, 0xFFFFFF, 0xE3EDFF, 0x0634C4, 0xEEF4FF, 0xEAF1FF),
            (Accent::Teal, false, false) => row(0x0B7F74, 0x085F57, 0xFFFFFF, 0xDCF2EF, 0x085F57, 0xECF8F6, 0xE6F6F3),
            (Accent::Violet, false, false) => row(0x6A4CF0, 0x4A2FC4, 0xFFFFFF, 0xECE8FF, 0x4A2FC4, 0xF3F0FF, 0xF0ECFF),
            (Accent::Graphite, false, false) => row(0x45556A, 0x2F3D50, 0xFFFFFF, 0xE6EAF0, 0x2F3D50, 0xF0F2F5, 0xEDF0F4),
            (Accent::Blue, true, false) => row(0x5AA2FF, 0x8FBFFF, 0x08132B, 0x263451, 0xCFE0FF, 0x262C39, 0x222C3E),
            (Accent::Teal, true, false) => row(0x3FD1C0, 0x7DE3D6, 0x04201D, 0x193B31, 0xC4F5EF, 0x1C2A25, 0x1B312F),
            (Accent::Violet, true, false) => row(0xA593FF, 0xC2B6FF, 0x150C3D, 0x322451, 0xDDD5FF, 0x291F39, 0x2D2149),
            (Accent::Graphite, true, false) => row(0x9AA8BB, 0xC0CAD8, 0x0F1722, 0x31343B, 0xDDE3EC, 0x27282C, 0x2A2C31),
            (Accent::Blue, false, true) => row(0x0847B0, 0x062B9E, 0xFFFFFF, 0xD6E4FF, 0x062B9E, 0xE6EFFF, 0xDDE9FF),
            (Accent::Teal, false, true) => row(0x065A50, 0x054740, 0xFFFFFF, 0xCDEDE8, 0x054740, 0xE0F4F1, 0xD8F0EC),
            (Accent::Violet, false, true) => row(0x4A2EBC, 0x3A22A0, 0xFFFFFF, 0xE2DCFF, 0x3A22A0, 0xEEEAFF, 0xE8E3FF),
            (Accent::Graphite, false, true) => row(0x33414F, 0x1F2A36, 0xFFFFFF, 0xDCE1E8, 0x1F2A36, 0xEBEEF2, 0xE4E8ED),
            (Accent::Blue, true, true) => row(0x8CC0FF, 0xB5D5FF, 0x051022, 0x1C3867, 0xE4EEFF, 0x1C2C47, 0x1A2E52),
            (Accent::Teal, true, true) => row(0x6FE8DA, 0x9FF2E8, 0x03201C, 0x114239, 0xE0FAF7, 0x143933, 0x123E38),
            (Accent::Violet, true, true) => row(0xC2B6FF, 0xDAD2FF, 0x120A35, 0x37286B, 0xEFEBFF, 0x2C234B, 0x30265E),
            (Accent::Graphite, true, true) => row(0xC0CCDB, 0xDCE3EC, 0x0C131D, 0x363E4E, 0xF0F3F7, 0x2C313B, 0x303642),
            (Accent::Custom, dark, high_contrast) => {
                let base = if dark { Tokens::dark() } else { Tokens::light() };
                let base = if high_contrast { base.high_contrast() } else { base };
                custom_ink(custom.unwrap_or(DEFAULT_CUSTOM_COLOR), &base)
            }
        }
    }
}

/// A custom colour as "#RRGGBB", as it is kept in `ui.json` and answered by `ui.accent`.
pub fn hex_string(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

/// "#rgb", "#rrggbb" or "rrggbb", any case; anything else is None. Bounded: no allocation, no panic.
pub fn parse_hex_color(s: &str) -> Option<Color32> {
    let digit = |c: u8| -> Option<u16> {
        match c {
            b'0'..=b'9' => Some(u16::from(c - b'0')),
            b'a'..=b'f' => Some(u16::from(c - b'a') + 10),
            b'A'..=b'F' => Some(u16::from(c - b'A') + 10),
            _ => None,
        }
    };
    let t = s.trim().strip_prefix('#').unwrap_or_else(|| s.trim());
    let b = t.as_bytes();
    match b.len() {
        3 => {
            let mut rgb = [0u8; 3];
            for (i, &c) in b.iter().enumerate() {
                rgb[i] = u8::try_from(digit(c)? * 17).ok()?;
            }
            Some(Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
        }
        6 => {
            let mut rgb = [0u8; 3];
            for i in 0..3 {
                rgb[i] = u8::try_from(digit(b[2 * i])? * 16 + digit(b[2 * i + 1])?).ok()?;
            }
            Some(Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
        }
        _ => None,
    }
}

/// A colour from a command's params: "#rrggbb" as a string, or `[r, g, b]` with each 0–255.
pub fn parse_color_value(v: &serde_json::Value) -> Option<Color32> {
    match v {
        serde_json::Value::String(s) => parse_hex_color(s),
        serde_json::Value::Array(a) if a.len() == 3 => {
            let mut rgb = [0u8; 3];
            for (i, x) in a.iter().enumerate() {
                rgb[i] = u8::try_from(x.as_u64()?).ok()?;
            }
            Some(Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
        }
        _ => None,
    }
}

/// `Option<Color32>` as "#rrggbb" in `ui.json`: anything else reads as none, like an unknown
/// accent code reads as the default.
pub mod color_option {
    use super::{Color32, hex_string, parse_hex_color};

    pub fn serialize<S: serde::Serializer>(c: &Option<Color32>, s: S) -> Result<S::Ok, S::Error> {
        match c {
            Some(c) => s.serialize_str(&hex_string(*c)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Color32>, D::Error> {
        let v = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
        Ok(v.as_str().and_then(parse_hex_color))
    }
}

// The WCAG 2.x relative luminance maths, as `theme_contrast_tests.rs` checks with: a custom
// accent is fitted so the same pairs pass for it as for the hand-written presets.
fn channel_lin(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn luminance(c: Color32) -> f32 {
    0.2126 * channel_lin(c.r()) + 0.7152 * channel_lin(c.g()) + 0.0722 * channel_lin(c.b())
}

/// Black or white, whichever stands out more on `bg`: the ink on a colour shown as the owner chose
/// it, which no theme has fitted an ink to (the tick on the custom accent's swatch). The better of
/// the two is always at least 4.5:1.
pub fn black_or_white_on(bg: Color32) -> Color32 {
    let l = luminance(bg);
    // Against black the ratio is (l + 0.05) / 0.05, against white 1.05 / (l + 0.05).
    if (l + 0.05) / 0.05 >= 1.05 / (l + 0.05) { Color32::BLACK } else { Color32::WHITE }
}

fn to_lin(c: Color32) -> [f32; 3] {
    [channel_lin(c.r()), channel_lin(c.g()), channel_lin(c.b())]
}

fn from_lin(l: [f32; 3]) -> Color32 {
    let ch = |v: f32| -> u8 {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        u8::try_from((s * 255.0).round() as i64).unwrap_or(255)
    };
    Color32::from_rgb(ch(l[0]), ch(l[1]), ch(l[2]))
}

/// `a` eased toward `b` in linear light.
fn lerp_lin(a: Color32, b: Color32, t: f32) -> Color32 {
    let (la, lb) = (to_lin(a), to_lin(b));
    let t = t.clamp(0.0, 1.0);
    from_lin([la[0] + (lb[0] - la[0]) * t, la[1] + (lb[1] - la[1]) * t, la[2] + (lb[2] - la[2]) * t])
}

/// `c` moved to luminance `target`, keeping its hue: scaled down in linear light when darkening
/// (luminance is linear in linear light), or eased toward white when lightening.
fn with_luminance(c: Color32, target: f32) -> Color32 {
    let cur = luminance(c);
    let target = target.clamp(0.0, 1.0);
    if cur <= f32::EPSILON {
        return from_lin([target, target, target]);
    }
    if cur >= target {
        let k = target / cur;
        let l = to_lin(c);
        from_lin([l[0] * k, l[1] * k, l[2] * k])
    } else {
        // L(lerp(c, white, t)) = (1 - t) * cur + t, so the mix that reaches the target is exact.
        let t = ((target - cur) / (1.0 - cur)).clamp(0.0, 1.0);
        lerp_lin(c, Color32::WHITE, t)
    }
}

/// `fg` adjusted — toward black on light `bgs`, toward white on dark ones — until
/// `contrast(fg, bg) >= min` for every one of them, with a little margin. All the backgrounds of
/// one kind (the sheets of a light or of a dark window) are on the same side, so the strictest
/// bound wins: the smallest cap on a light sheet, the highest floor on a dark one. A colour that
/// already meets the bound is kept as it is: only an unsuitable one is moved, clamped to the
/// bound, so e.g. a custom black stays black on a light window.
fn fg_for(fg: Color32, bgs: &[Color32], min: f32) -> Color32 {
    if luminance(bgs[0]) >= 0.18 {
        let mut cap = f32::INFINITY;
        for &bg in bgs {
            cap = cap.min((luminance(bg) + 0.05) / min - 0.05);
        }
        let bound = cap.max(0.0) * 0.97;
        if luminance(fg) <= bound { fg } else { with_luminance(fg, bound) }
    } else {
        let mut floor = 0.0_f32;
        for &bg in bgs {
            floor = floor.max(min * (luminance(bg) + 0.05) - 0.05);
        }
        let bound = floor.min(0.98) * 1.03;
        if luminance(fg) >= bound { fg } else { with_luminance(fg, bound) }
    }
}

/// `bg` eased toward `toward` (the sheet behind it, whose own contrast with every `fg` already
/// passes) until `contrast(fg, bg) >= min` for every `fg`, with a little margin. Used for the
/// washes, which are mixes of the sheet and the accent and can always be eased back toward the
/// sheet.
fn bg_for(bg: Color32, fgs: &[Color32], min: f32, toward: Color32) -> Color32 {
    let light = luminance(toward) >= 0.18;
    let mut bound = if light { 0.0_f32 } else { 1.0_f32 };
    for &fg in fgs {
        let lf = luminance(fg);
        let need = if light { min * (lf + 0.05) - 0.05 } else { (lf + 0.05) / min - 0.05 };
        bound = if light { bound.max(need) } else { bound.min(need) };
    }
    // The margin keeps a little away from the exact ratio; the sheet's own luminance is the most
    // the wash can reach.
    let target = if light { (bound * 1.03).min(luminance(toward)) } else { (bound * 0.97).max(luminance(toward)) };
    let cur = luminance(bg);
    if light && cur >= target || !light && cur <= target {
        return bg;
    }
    let lt = luminance(toward);
    let t = if (lt - cur).abs() <= f32::EPSILON { 1.0 } else { ((target - cur) / (lt - cur)).clamp(0.0, 1.0) };
    lerp_lin(bg, toward, t)
}

/// The ink of a custom accent, worked out from its colour against the palette it will sit on
/// (`base`, already dark or light and high contrast or not): the accent is fitted to the sheets,
/// the ink on it is the opposite extreme, and the washes are mixes of the sheet and the accent,
/// each fitted until the pairs `theme_contrast_tests.rs` checks pass for it.
pub(crate) fn custom_ink(color: Color32, base: &Tokens) -> AccentInk {
    let dark = base.dark;
    // Text-level pairs need 4.5:1, and 7:1 in high contrast (where the graphic pairs of the
    // accent and the edges rise to 7:1 too); fitting everything to the higher covers both.
    let req = if base.high_contrast { 7.0 } else { 4.5 };
    let surface = base.surface;
    // Fit against the window background too: in the light theme the canvas is a little darker
    // than the sheets, so it is the strictest bound for the accent as text. And against the
    // message list's flagged and shaded rows, where an unread sender is in it (#1657).
    let accent = fg_for(color, &[surface, base.panel, base.canvas, base.flagged_row, base.row_shade()], req);
    let text = accent;
    // The accent is dark on a light window and light on a dark one, so the ink on it is the
    // opposite extreme; the fit above leaves enough margin for 4.5:1, and for 7:1 in high
    // contrast.
    let on_accent = if dark { Color32::BLACK } else { Color32::WHITE };
    let mix = |t: f32| lerp_lin(surface, accent, t);
    // The ink on the soft wash: a darker shade of the accent on a light window, a lighter one on a
    // dark window.
    let soft_text = if dark { with_luminance(accent, luminance(accent).max(0.85)) } else { with_luminance(accent, luminance(accent) * 0.55) };
    let soft = bg_for(mix(if dark { 0.32 } else { 0.14 }), &[soft_text, base.text, base.text_dim], req, surface);
    let selection = bg_for(mix(if dark { 0.38 } else { 0.07 }), &[text, base.text, base.text_dim], req, surface);
    let today = bg_for(mix(if dark { 0.46 } else { 0.10 }), &[text, base.text], req, surface);
    AccentInk { accent, text, on_accent, soft, soft_text, selection, today }
}

/// An unknown saved value reads as the default rather than discarding the whole `ui.json`.
impl From<String> for Accent {
    fn from(s: String) -> Self {
        Accent::from_code(&s).unwrap_or_default()
    }
}

impl From<Accent> for String {
    fn from(a: Accent) -> Self {
        a.code().to_string()
    }
}

impl<'de> serde::Deserialize<'de> for Accent {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
        Ok(v.as_str().and_then(Accent::from_code).unwrap_or_default())
    }
}

impl serde::Serialize for Accent {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

/// A switch read from `ui.json` that is anything but a boolean reads as off.
pub fn lenient_bool<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
    Ok(v.as_bool().unwrap_or(false))
}

/// What an accent colours.
#[derive(Clone, Copy, Debug)]
pub struct AccentInk {
    pub accent: Color32,
    /// The accent as text on the window's sheets.
    pub text: Color32,
    pub on_accent: Color32,
    /// The wash behind a selected folder, a count pill and a checked button, and the ink on it.
    pub soft: Color32,
    pub soft_text: Color32,
    /// The wash of a selected row, and of today's column.
    pub selection: Color32,
    pub today: Color32,
}

impl Tokens {
    /// `fg` as text on `bg`: kept as it is when it reads at 4.5:1 there (7:1 in High Contrast),
    /// otherwise darkened on a light background or lightened on a dark one until it does. For a
    /// colour over a background it was not chosen for: a message-list row's unread sender,
    /// overdue text and red "[Draft]" on whatever is under them ([`Tokens::list_row_layers`], #1657).
    pub fn text_ink_on(&self, fg: Color32, bg: Color32) -> Color32 {
        fg_for(fg, &[bg], if self.high_contrast { 7.0 } else { 4.5 })
    }

    /// This palette with `ink` as its accent.
    pub fn with_accent(mut self, ink: AccentInk) -> Tokens {
        let (select, focus, band_a, band_b) = if self.dark { (0.45, 0.5, 0.35, 0.28) } else { (0.30, 0.5, 0.16, 0.16) };
        self.accent = ink.accent;
        self.accent_text = ink.text;
        self.on_accent = ink.on_accent;
        self.accent_soft = ink.soft;
        self.accent_soft_text = ink.soft_text;
        self.checked = ink.soft;
        self.selection_pill = ink.soft;
        self.selection = ink.selection;
        self.today = ink.today;
        self.selection_edge = ink.accent;
        self.backstage_mark = ink.accent;
        self.text_selection = ink.accent.linear_multiply(select);
        self.focus_ring = ink.accent.linear_multiply(focus);
        self.band_chosen = ink.accent.linear_multiply(band_a);
        self.band_meeting = ink.accent.linear_multiply(band_b);
        self
    }

    /// This palette made stronger: text and outlines at 7:1 or more on their sheets, firmer
    /// borders, no hairline that disappears. Fixed colours (a flag, the categories, calendars,
    /// avatars) are left alone but for the few of the interface's own that were not strong
    /// enough. The accent is set by [`Tokens::with_accent`].
    pub fn high_contrast(mut self) -> Tokens {
        self.high_contrast = true;
        if self.dark {
            let window = hex(0x0A0A0B);
            let panel = hex(0x0E0E0F);
            let surface = hex(0x121213);
            let ribbon = hex(0x19191B);
            let text = Color32::WHITE;
            let hover = hex(0x2C2C30);
            self.title_bar = window;
            self.tab_strip = window;
            self.nav = window;
            self.canvas = window;
            self.status_bar = window;
            self.panel = panel;
            self.lane = panel;
            self.surface = surface;
            self.input = surface;
            self.ribbon = ribbon;
            self.menu = ribbon;
            self.text = text;
            self.text_dim = hex(0xE6E6EA);
            self.group_label = hex(0xE6E6EA);
            self.text_disabled = hex(0xA0A0A8);
            self.icon = Color32::WHITE;
            self.border = hex(0xA8A8B0);
            self.border_strong = hex(0xCFCFD6);
            self.input_border = hex(0xC6C6CE);
            self.field_edge = self.input_border;
            self.rule = hex(0x6E6E76);
            self.border_faint = hex(0x6E6E76);
            self.ribbon_edge = Stroke::new(1.0, self.border);
            self.hover = hover;
            self.pressed = hex(0x404046);
            self.track = hex(0x616168);
            self.pill_quiet = hex(0x2E2E33);
            self.selection_unfocused = hex(0x222226);
            self.selection_edge_unfocused = hex(0xA8A8B0);
            self.flagged_row = hex(0x3E282D);
            self.overdue = hex(0xFFB3B3);
            self.off_hours = hex(0x101011);
            self.find_highlight = Color32::from_rgba_unmultiplied(0xE0, 0xBC, 0x00, 0xA0);
            self.red = hex(0xFF8A8A);
            self.green = hex(0x7FE0A8);
            self.orange = hex(0xFFB870);
            self.blue = hex(0x9CC0FF);
            self.icon_red = hex(0xFF9A9A);
            self.icon_yellow = hex(0xF5C860);
            self.icon_green = hex(0x7FE0A8);
            self.icon_purple = hex(0xCDB8FF);
            self.scrim = Color32::from_black_alpha(150);
            self.backstage_column = panel;
            self.backstage_ink = text;
            self.ink_on_dark = text;
            self.ink_on_light = surface;
            self.notice_wash_alpha = 70;
            self.notice_edge_alpha = 200;
            self.hover_subtle = hover.linear_multiply(0.6);
            self.hover_nav = panel.lerp_to_gamma(hover, 0.7);
            self.surface_sunken = surface.linear_multiply(0.55);
        } else {
            let window = hex(0xF2F4F7);
            let panel = hex(0xF8F9FB);
            let surface = Color32::WHITE;
            let text = hex(0x0A0D14);
            let hover = hex(0xDCE2EB);
            self.title_bar = window;
            self.tab_strip = window;
            self.nav = window;
            self.canvas = window;
            self.status_bar = window;
            self.lane = window;
            self.panel = panel;
            self.surface = surface;
            self.input = surface;
            self.ribbon = surface;
            self.menu = surface;
            self.text = text;
            self.text_dim = hex(0x2B3342);
            self.group_label = hex(0x2B3342);
            self.text_disabled = hex(0x5C6678);
            self.icon = hex(0x141A26);
            self.border = hex(0x434F61);
            self.border_strong = hex(0x2E3645);
            self.input_border = hex(0x3F4A5C);
            self.field_edge = self.input_border;
            self.rule = hex(0x8A94A6);
            self.border_faint = hex(0x8A94A6);
            self.ribbon_edge = Stroke::new(1.0, self.border);
            self.hover = hover;
            self.pressed = hex(0xC2CAD6);
            self.track = hex(0x8A94A6);
            self.pill_quiet = hex(0xE2E6ED);
            self.selection_unfocused = hex(0xEBEEF3);
            self.selection_edge_unfocused = hex(0x4A5568);
            self.flagged_row = hex(0xFCE8EB);
            self.overdue = hex(0x8A140C);
            self.off_hours = hex(0xF0F2F6);
            self.find_highlight = Color32::from_rgba_unmultiplied(0xFF, 0xD0, 0x00, 0xB0);
            self.red = hex(0xA31B1B);
            self.green = hex(0x105A33);
            self.orange = hex(0x7A3A00);
            self.blue = hex(0x1D4FA8);
            self.icon_red = hex(0x9E1230);
            self.icon_yellow = hex(0x6E4600);
            self.icon_green = hex(0x14603A);
            self.icon_purple = hex(0x5A3694);
            self.scrim = Color32::from_black_alpha(90);
            self.backstage_column = panel;
            self.backstage_ink = text;
            self.ink_on_dark = surface;
            self.ink_on_light = text;
            self.notice_wash_alpha = 50;
            self.notice_edge_alpha = 200;
            self.hover_subtle = hover.linear_multiply(0.6);
            self.hover_nav = panel.lerp_to_gamma(hover, 0.7);
            self.surface_sunken = surface.linear_multiply(0.55);
        }
        self
    }

    /// The tokens for a window that is `dark`, in `accent`, with high contrast on or off.
    pub fn build(dark: bool, accent: Accent, high_contrast: bool) -> Tokens {
        Self::build_choice(dark, accent, None, high_contrast)
    }

    /// The tokens for a window that is `dark`, in `accent` — with `custom` as the colour when the
    /// accent is a custom one, fitted to this palette's own sheets — and high contrast on or off.
    pub fn build_choice(dark: bool, accent: Accent, custom: Option<Color32>, high_contrast: bool) -> Tokens {
        let base = if dark { Tokens::dark() } else { Tokens::light() };
        let base = if high_contrast { base.high_contrast() } else { base };
        match accent {
            Accent::Custom => base.with_accent(custom_ink(custom.unwrap_or(DEFAULT_CUSTOM_COLOR), &base)),
            a => base.with_accent(a.ink(dark, high_contrast, custom)),
        }
    }
}
