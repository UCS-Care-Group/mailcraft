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
}

impl Accent {
    /// In picker order.
    pub const ALL: [Accent; 4] = [Accent::Blue, Accent::Teal, Accent::Violet, Accent::Graphite];

    /// The value used in `ui.json` and by the `ui.accent` command.
    pub fn code(self) -> &'static str {
        match self {
            Accent::Blue => "blue",
            Accent::Teal => "teal",
            Accent::Violet => "violet",
            Accent::Graphite => "graphite",
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
        }
    }

    /// The accent's colours for a window that is `dark` and `high_contrast` or not.
    pub fn ink(self, dark: bool, high_contrast: bool) -> AccentInk {
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
            (Accent::Blue, true, false) => row(0x5AA2FF, 0x8FBFFF, 0x08132B, 0x1F3358, 0xCFE0FF, 0x1F2B40, 0x1B2B45),
            (Accent::Teal, true, false) => row(0x3FD1C0, 0x7DE3D6, 0x04201D, 0x123A38, 0xC4F5EF, 0x15292C, 0x143036),
            (Accent::Violet, true, false) => row(0xA593FF, 0xC2B6FF, 0x150C3D, 0x2B2358, 0xDDD5FF, 0x221E40, 0x262050),
            (Accent::Graphite, true, false) => row(0x9AA8BB, 0xC0CAD8, 0x0F1722, 0x2A3342, 0xDDE3EC, 0x202733, 0x232B38),
            (Accent::Blue, false, true) => row(0x0847B0, 0x062B9E, 0xFFFFFF, 0xD6E4FF, 0x062B9E, 0xE6EFFF, 0xDDE9FF),
            (Accent::Teal, false, true) => row(0x065A50, 0x054740, 0xFFFFFF, 0xCDEDE8, 0x054740, 0xE0F4F1, 0xD8F0EC),
            (Accent::Violet, false, true) => row(0x4A2EBC, 0x3A22A0, 0xFFFFFF, 0xE2DCFF, 0x3A22A0, 0xEEEAFF, 0xE8E3FF),
            (Accent::Graphite, false, true) => row(0x33414F, 0x1F2A36, 0xFFFFFF, 0xDCE1E8, 0x1F2A36, 0xEBEEF2, 0xE4E8ED),
            (Accent::Blue, true, true) => row(0x8CC0FF, 0xB5D5FF, 0x051022, 0x1A3A6E, 0xE4EEFF, 0x1A2E4E, 0x183059),
            (Accent::Teal, true, true) => row(0x6FE8DA, 0x9FF2E8, 0x03201C, 0x0F4440, 0xE0FAF7, 0x123B3A, 0x10403F),
            (Accent::Violet, true, true) => row(0xC2B6FF, 0xDAD2FF, 0x120A35, 0x352A72, 0xEFEBFF, 0x2A2552, 0x2E2865),
            (Accent::Graphite, true, true) => row(0xC0CCDB, 0xDCE3EC, 0x0C131D, 0x344055, 0xF0F3F7, 0x2A3342, 0x2E3849),
        }
    }
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
            let window = hex(0x080A0E);
            let panel = hex(0x0C0F14);
            let surface = hex(0x10141A);
            let ribbon = hex(0x161B23);
            let text = Color32::WHITE;
            let hover = hex(0x262F3D);
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
            self.text_dim = hex(0xE0E6F0);
            self.group_label = hex(0xE0E6F0);
            self.text_disabled = hex(0x9AA5B8);
            self.icon = Color32::WHITE;
            self.border = hex(0x9AA6BA);
            self.border_strong = hex(0xC8D0DE);
            self.input_border = hex(0xC0C9D8);
            self.field_edge = self.input_border;
            self.rule = hex(0x667286);
            self.border_faint = hex(0x667286);
            self.ribbon_edge = Stroke::new(1.0, self.border);
            self.hover = hover;
            self.pressed = hex(0x3A4556);
            self.track = hex(0x5A6578);
            self.pill_quiet = hex(0x2A3342);
            self.selection_unfocused = hex(0x1A2230);
            self.selection_edge_unfocused = hex(0xA0AEC6);
            self.flagged_row = hex(0x3E2830);
            self.overdue = hex(0xFFB3B3);
            self.off_hours = hex(0x0E1218);
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
        let base = if dark { Tokens::dark() } else { Tokens::light() };
        let base = if high_contrast { base.high_contrast() } else { base };
        base.with_accent(accent.ink(dark, high_contrast))
    }
}
