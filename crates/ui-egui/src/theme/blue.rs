// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The dark theme's two styles (#1384): neutral greys (the default, #1242) or the blue-tinted
//! greys the dark theme had before them. Only the greys differ: the fixed colours (a flag, the
//! categories, the calendars) are the same in both, and each accent has a row of its own for
//! each style, checked by `theme_contrast_tests.rs` like the others.

use egui::{Color32, Stroke};

use super::Tokens;
use super::accent::{Accent, AccentInk};

fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Which greys the dark theme is drawn in. The light theme is the same for both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DarkStyle {
    /// Neutral greys, as in the other Craft programs.
    #[default]
    Neutral,
    /// Greys with a cool, blue tint.
    Blue,
}

impl DarkStyle {
    /// In picker order.
    pub const ALL: [DarkStyle; 2] = [DarkStyle::Neutral, DarkStyle::Blue];

    /// The value used in `ui.json` and by the `ui.darkStyle` command.
    pub fn code(self) -> &'static str {
        match self {
            DarkStyle::Neutral => "neutral",
            DarkStyle::Blue => "blue",
        }
    }

    pub fn from_code(code: &str) -> Option<DarkStyle> {
        DarkStyle::ALL.into_iter().find(|s| s.code().eq_ignore_ascii_case(code.trim()))
    }

    /// The English name (translated with `tl!` where shown).
    pub fn label(self) -> &'static str {
        match self {
            DarkStyle::Neutral => "Neutral grey",
            DarkStyle::Blue => "Blue grey",
        }
    }
}

/// An unknown saved value reads as the default rather than discarding the whole `ui.json`.
impl<'de> serde::Deserialize<'de> for DarkStyle {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
        Ok(v.as_str().and_then(DarkStyle::from_code).unwrap_or_default())
    }
}

impl serde::Serialize for DarkStyle {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl Accent {
    /// The accent's colours on the blue-tinted dark greys, high contrast or not: its washes are
    /// tinted to sit on them.
    pub fn ink_on_blue(self, high_contrast: bool) -> AccentInk {
        let row = |a, at, oa, soft, st, sel, today| AccentInk {
            accent: hex(a),
            text: hex(at),
            on_accent: hex(oa),
            soft: hex(soft),
            soft_text: hex(st),
            selection: hex(sel),
            today: hex(today),
        };
        match (self, high_contrast) {
            (Accent::Blue, false) => row(0x5AA2FF, 0x8FBFFF, 0x08132B, 0x1F3358, 0xCFE0FF, 0x1F2B40, 0x1B2B45),
            (Accent::Teal, false) => row(0x3FD1C0, 0x7DE3D6, 0x04201D, 0x123A38, 0xC4F5EF, 0x15292C, 0x143036),
            (Accent::Violet, false) => row(0xA593FF, 0xC2B6FF, 0x150C3D, 0x2B2358, 0xDDD5FF, 0x221E40, 0x262050),
            (Accent::Graphite, false) => row(0x9AA8BB, 0xC0CAD8, 0x0F1722, 0x2A3342, 0xDDE3EC, 0x202733, 0x232B38),
            (Accent::Blue, true) => row(0x8CC0FF, 0xB5D5FF, 0x051022, 0x1A3A6E, 0xE4EEFF, 0x1A2E4E, 0x183059),
            (Accent::Teal, true) => row(0x6FE8DA, 0x9FF2E8, 0x03201C, 0x0F4440, 0xE0FAF7, 0x123B3A, 0x10403F),
            (Accent::Violet, true) => row(0xC2B6FF, 0xDAD2FF, 0x120A35, 0x352A72, 0xEFEBFF, 0x2A2552, 0x2E2865),
            (Accent::Graphite, true) => row(0xC0CCDB, 0xDCE3EC, 0x0C131D, 0x344055, 0xF0F3F7, 0x2A3342, 0x2E3849),
        }
    }
}

impl Tokens {
    /// The tokens for a dark window in `style`, in `accent`, with high contrast on or off.
    pub fn build_dark(style: DarkStyle, accent: Accent, high_contrast: bool) -> Tokens {
        match style {
            DarkStyle::Neutral => Tokens::build(true, accent, high_contrast),
            DarkStyle::Blue => {
                let base = Tokens::dark_blue();
                let base = if high_contrast { base.high_contrast().high_contrast_blue() } else { base };
                base.with_accent(accent.ink_on_blue(high_contrast))
            }
        }
    }

    /// The dark palette in blue-tinted greys: one cool ramp, window to folder pane to list to
    /// ribbon. Everything that is not a grey is the neutral palette's ([`Tokens::dark`]).
    pub fn dark_blue() -> Tokens {
        let t = Tokens::dark();
        let window = hex(0x12161D);
        let panel = hex(0x161B23);
        let surface = hex(0x1B212B);
        let text = hex(0xF1F4F9);
        let hover = hex(0x2A3240);
        let soft = hex(0x1F3358);
        let input_border = hex(0x7D889C);
        let ribbon = hex(0x202733);
        let dim = hex(0xB4BCCB);
        Tokens {
            title_bar: window,
            ribbon,
            tab_strip: window,
            nav: window,
            canvas: window,
            panel,
            surface,
            border: hex(0x2C3442),
            border_strong: hex(0x4A5568),
            text,
            text_dim: dim,
            text_disabled: hex(0x6E788C),
            icon: hex(0xDFE4EC),
            hover,
            pressed: hex(0x364052),
            checked: soft,
            input: surface,
            input_border,
            group_label: dim,
            status_bar: window,
            selection: hex(0x1F2B40),
            flagged_row: hex(0x34262C),
            off_hours: hex(0x171C24),
            today: hex(0x1B2B45),
            menu: ribbon,
            selection_unfocused: hex(0x1A2230),
            selection_edge_unfocused: hex(0x4A5872),
            selection_pill: soft,
            accent_soft: soft,
            pill_quiet: hex(0x262E3B),
            hover_subtle: hover.linear_multiply(0.6),
            hover_nav: panel.lerp_to_gamma(hover, 0.7),
            surface_sunken: surface.linear_multiply(0.55),
            rule: hex(0x252C38),
            border_faint: hex(0x222934),
            track: hex(0x364052),
            lane: panel,
            ribbon_edge: Stroke::new(1.0, hex(0x2A3240)),
            field_edge: input_border,
            backstage_column: panel,
            backstage_ink: text,
            ink_on_dark: text,
            ink_on_light: surface,
            ..t
        }
    }

    /// The blue-tinted dark palette made stronger: the greys [`Tokens::high_contrast`] gives the
    /// neutral one, in the same cool ramp. Called on tokens that are already high contrast.
    pub(super) fn high_contrast_blue(mut self) -> Tokens {
        let window = hex(0x080A0E);
        let panel = hex(0x0C0F14);
        let surface = hex(0x10141A);
        let ribbon = hex(0x161B23);
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
        self.text_dim = hex(0xE0E6F0);
        self.group_label = hex(0xE0E6F0);
        self.text_disabled = hex(0x9AA5B8);
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
        self.off_hours = hex(0x0E1218);
        self.backstage_column = panel;
        self.ink_on_light = surface;
        self.hover_subtle = hover.linear_multiply(0.6);
        self.hover_nav = panel.lerp_to_gamma(hover, 0.7);
        self.surface_sunken = surface.linear_multiply(0.55);
        self
    }
}
