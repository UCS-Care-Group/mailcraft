// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Classic Light (#1203): a light look for people used to the desktop mail programs of the early
//! 2010s. One flat white page from the title bar to the panes, parted by hairline rules, with
//! square corners; the ribbon's tabs in capitals and File a button in the accent; the status bar
//! filled with the accent; a selected message marked by a bar in the accent down its left edge;
//! and the ribbon and message list closer together ([`Tokens::dense`]). Only the greys, the
//! corners and how those parts are drawn are its own: the accent, high contrast and the fixed
//! colours (a flag, the categories, the calendars) are the light theme's. Each value was chosen
//! for this palette and checked by `theme_contrast_tests.rs` like the others.

use egui::{Color32, Stroke};

use super::Tokens;
use super::accent::{Accent, DEFAULT_CUSTOM_COLOR, custom_ink};

fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl Tokens {
    /// The Classic Light palette in the blue accent. Everything that is not a grey, a corner, the
    /// density or how the bands, tabs and rows are drawn is the light palette's ([`Tokens::light`]).
    pub fn classic_light() -> Tokens {
        let t = Tokens::light();
        // The window is nearly the page's white: only a breath of grey round the panes.
        let window = hex(0xF7F7F8);
        let panel = Color32::WHITE;
        let surface = Color32::WHITE;
        let text = hex(0x1F2023);
        let dim = hex(0x55575C);
        let hover = hex(0xE6E7EA);
        let border = hex(0xD3D4D8);
        let input_border = hex(0x7A7D83);
        Tokens {
            title_bar: window,
            tab_strip: window,
            nav: window,
            canvas: window,
            status_bar: window,
            lane: hex(0xF0F1F2),
            // The ribbon is part of the page, not a card on the frame.
            ribbon: surface,
            menu: surface,
            panel,
            surface,
            input: surface,
            border,
            border_strong: hex(0xA5A7AC),
            border_faint: hex(0xEAEBED),
            rule: hex(0xE2E3E6),
            text,
            text_dim: dim,
            group_label: dim,
            text_disabled: hex(0x7E8085),
            icon: hex(0x3A3C41),
            hover,
            pressed: hex(0xD0D2D6),
            input_border,
            field_edge: input_border,
            selection_unfocused: hex(0xEAEBED),
            selection_edge_unfocused: hex(0x80838A),
            off_hours: hex(0xF3F4F5),
            pill_quiet: hex(0xE7E8EA),
            track: hex(0xD0D2D6),
            ribbon_edge: Stroke::new(1.0, border),
            hover_subtle: hover.linear_multiply(0.6),
            hover_nav: panel.lerp_to_gamma(hover, 0.7),
            surface_sunken: surface.linear_multiply(0.55),
            backstage_column: panel,
            backstage_ink: text,
            ink_on_light: text,
            dense: true,
            sheet_radius: 0,
            control_radius: 0,
            status_band: true,
            tab_caps: true,
            file_tab_fill: true,
            row_bar: true,
            ..t
        }
    }

    /// The tokens of Classic Light in `accent` (with `custom` as the colour when the accent is a
    /// custom one, fitted to Classic Light's own sheets), with high contrast on or off. High
    /// contrast gives it the light theme's stronger greys, as it does the light theme; the corners
    /// and the density stay Classic Light's.
    pub fn build_classic(accent: Accent, custom: Option<Color32>, high_contrast: bool) -> Tokens {
        let base = Tokens::classic_light();
        let base = if high_contrast { base.high_contrast() } else { base };
        match accent {
            Accent::Custom => base.with_accent(custom_ink(custom.unwrap_or(DEFAULT_CUSTOM_COLOR), &base)),
            a => base.with_accent(a.ink(false, high_contrast, custom)),
        }
    }
}
