// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's mark, small, for the title bar.

use egui::{Color32, Painter, Rect, Stroke, pos2, vec2};

/// The app icon's blue tile with a calendar page and an envelope on it, drawn from its shapes
/// (original artwork; the tile's blue is the icon's own). `r` is a square.
pub fn app_mark(p: &Painter, r: Rect) {
    let s = r.width().min(r.height());
    let at = |x: f32, y: f32| pos2(r.min.x + x * s, r.min.y + y * s);
    let size = |w: f32, h: f32| vec2(w * s, h * s);
    let tile = Color32::from_rgb(0x0F, 0x68, 0xF0);
    let sheet = Color32::from_rgb(0x9A, 0xCF, 0xFC);
    p.rect_filled(r, s * 0.22, tile);
    // The sheet behind the page.
    p.rect_filled(Rect::from_min_size(at(0.2, 0.17), size(0.62, 0.62)), s * 0.07, sheet.gamma_multiply(0.55));
    // The calendar page, with its header band, rings and the envelope's flap.
    let page = Rect::from_min_size(at(0.24, 0.27), size(0.56, 0.5));
    p.rect_filled(page, s * 0.06, Color32::WHITE);
    p.rect_filled(Rect::from_min_size(page.min, size(0.56, 0.1)), s * 0.06, Color32::from_rgb(0x58, 0xD3, 0xFB));
    for x in [0.34, 0.66] {
        p.rect_filled(Rect::from_center_size(at(x, 0.27), size(0.05, 0.12)), s * 0.025, Color32::from_rgb(0xC9, 0xF0, 0xFF));
    }
    let stroke = Stroke::new((s * 0.045).max(1.0), tile);
    p.line_segment([at(0.28, 0.45), at(0.52, 0.6)], stroke);
    p.line_segment([at(0.76, 0.45), at(0.52, 0.6)], stroke);
}
