// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures behind a box (#601): a table's, a cell's, a block's or the body's, from its
//! `background` attribute or its style (`mailcraft_html::BgPicture`).
//!
//! The picture is one of the message's own (attached, `cid:`, or written into it, `data:`), shown
//! always, or one on the web, shown only when the owner asked for the message's pictures and it
//! has come: the same bytes, the same limits and the same request as an `<img>` (`images.rs`,
//! `remote.rs`). It is painted over the box's colour and under what the box holds, clipped to
//! the box (to its rectangle: rounded corners are not cut off it), at the size and place its
//! style asks, once or repeated across and down. The copies are counted for the part of the
//! box in view: past [`MAX_TILES`] they are drawn larger, as few as cover it, so a picture of a
//! pixel behind a long message costs no more than a few thousand; and all the boxes in view
//! together take at most [`MAX_FRAME_TILES`] a frame (a box past it has fewer, larger copies).
//!
//! The text on it is read against the box's colour, the one its sender gives for the programs
//! that show no picture ([`super::colors`]). In the dark theme that colour is turned dark when it
//! is light, and the picture is dimmed with it ([`DARK_TINT`]), so the text made readable on the
//! colour stays readable on the picture; a dark colour, which stays, leaves its picture as it is
//! too, and so do a message with its own design for dark windows and a message read on a light
//! page. A box with a picture and no colour has its text read against the paper round it, and
//! the picture is veiled with that paper ([`VEIL`]; dimmed under it too in the dark theme) so the text stays readable on it in either
//! theme. With high contrast on, no picture is painted behind a box: the text is on its colour
//! alone.

use egui::{Color32, Rect, Vec2, pos2, vec2};
use mailcraft_html::{BgOffset, BgSize, Length};

use super::layout::BackdropItem;
use crate::theme::Tokens;

/// Most copies of a picture painted behind one box in a frame.
pub const MAX_TILES: usize = 4096;

/// Most copies of pictures behind boxes painted in a frame, all the boxes in view together.
pub const MAX_FRAME_TILES: usize = 16_384;

/// How much of the paper round it covers a picture behind a box with no colour of its own.
pub const VEIL: f32 = 0.7;

/// What a picture behind a box is multiplied by in the dark theme: dimmed to under two fifths.
pub const DARK_TINT: Color32 = Color32::from_gray(96);

/// The colour a picture behind a box is multiplied by in this theme, or `None` when no picture
/// is painted behind a box at all (high contrast).
pub fn tint(t: &Tokens) -> Option<Color32> {
    if t.high_contrast {
        None
    } else if t.dark && !t.sender_dark {
        Some(DARK_TINT)
    } else {
        Some(Color32::WHITE)
    }
}

/// A length of a size asked for, against the room of the box on that side.
fn resolve(length: Length, room: f32) -> f32 {
    match length {
        Length::Px(px) => f32::from(px),
        Length::Percent(p) => room * f32::from(p) / 100.0,
    }
}

/// The size one copy of a picture of `natural` points is drawn at in a box of `room`: as the
/// box asks ([`BgSize`]), never less than a point, never anything that isn't a number.
pub fn tile_size(size: BgSize, natural: Vec2, room: Vec2) -> Vec2 {
    let fine = |v: f32| if v.is_finite() && v >= 1.0 { v } else { 1.0 };
    let natural = vec2(fine(natural.x), fine(natural.y));
    let room = vec2(fine(room.x), fine(room.y));
    let shape = natural.x / natural.y;
    let wanted = match size {
        BgSize::Auto => natural,
        BgSize::Cover => natural * (room.x / natural.x).max(room.y / natural.y),
        BgSize::Contain => natural * (room.x / natural.x).min(room.y / natural.y),
        BgSize::Given { width, height } => match (width.map(|w| resolve(w, room.x)), height.map(|h| resolve(h, room.y))) {
            (Some(w), Some(h)) => vec2(w, h),
            (Some(w), None) => vec2(w, w / shape),
            (None, Some(h)) => vec2(h * shape, h),
            (None, None) => natural,
        },
    };
    vec2(fine(wanted.x), fine(wanted.y))
}

/// Where the first copy's top left corner is, from the box's: a percentage of the room the copy
/// leaves (so 100% puts it against the far edge, as CSS does), or a length.
pub fn offset(position: [BgOffset; 2], tile: Vec2, room: Vec2) -> Vec2 {
    let one = |at: BgOffset, tile: f32, room: f32| match at {
        BgOffset::Percent(p) => (room - tile) * f32::from(p) / 100.0,
        BgOffset::Px(px) => f32::from(px),
    };
    let [x, y] = position;
    vec2(one(x, tile.x, room.x), one(y, tile.y, room.y))
}

/// The copies to paint of a picture behind a box at `rect`, in the part `visible` of it that is
/// in view: those that touch it, at most `max` of them (larger ones, as few as cover it, when
/// the picture's own size would take more).
pub fn tiles(item: &BackdropItem, rect: Rect, visible: Rect, max: usize) -> Vec<Rect> {
    let max = i64::try_from(max.clamp(1, MAX_TILES)).unwrap_or(1);
    let view = rect.intersect(visible);
    if !(view.is_positive() && item.tile.x.is_finite() && item.tile.y.is_finite()) {
        return Vec::new();
    }
    let first = rect.min + item.offset;
    // The first and last copy on one side, from the first copy; a side that doesn't repeat has the one.
    let span = |repeat: bool, start: f32, tile: f32, low: f32, high: f32| -> (i64, i64) {
        if !repeat {
            return (0, 0);
        }
        (((low - start) / tile).floor() as i64, ((high - start) / tile).ceil() as i64 - 1)
    };
    let natural = vec2(item.tile.x.max(1.0), item.tile.y.max(1.0));
    let [across, down] = item.repeat;
    let counts = |tile: Vec2| {
        let (x0, x1) = span(across, first.x, tile.x, view.min.x, view.max.x);
        let (y0, y1) = span(down, first.y, tile.y, view.min.y, view.max.y);
        let across = x1.saturating_sub(x0).saturating_add(1).max(0);
        let down = y1.saturating_sub(y0).saturating_add(1).max(0);
        ((x0, x1), (y0, y1), across.saturating_mul(down))
    };
    let (mut tile, (mut xs, mut ys, mut count)) = (natural, counts(natural));
    if count > max {
        // Larger copies until they are few enough: the shape kept where it repeats both ways, and
        // longer along the one side where it repeats on one (a strip of a gradient, mostly).
        let over = count as f64 / max as f64;
        let mut factor = if across && down { over.sqrt() } else { over }.ceil().max(2.0) as f32;
        for _ in 0..16 {
            tile = vec2(if across { natural.x * factor } else { natural.x }, if down { natural.y * factor } else { natural.y });
            (xs, ys, count) = counts(tile);
            if count <= max {
                break;
            }
            factor *= 1.25;
        }
        if count > max {
            return Vec::new();
        }
    }
    let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for row in ys.0..=ys.1 {
        for column in xs.0..=xs.1 {
            let at = pos2(first.x + column as f32 * tile.x, first.y + row as f32 * tile.y);
            let copy = Rect::from_min_size(at, tile);
            if copy.intersects(view) {
                out.push(copy);
            }
        }
    }
    out
}

/// Paint the copies of a picture behind a box at `rect` that are in `clip`, clipped to the box,
/// in one mesh, at most `max` of them; with its veil over them. How many were painted.
pub fn paint(painter: &egui::Painter, rect: Rect, clip: Rect, item: &BackdropItem, texture: egui::TextureId, max: usize) -> usize {
    let copies = tiles(item, rect, clip, max);
    if copies.is_empty() {
        return 0;
    }
    let painted = copies.len();
    let mut mesh = egui::Mesh::with_texture(texture);
    let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    for copy in copies {
        mesh.add_rect_with_uv(copy, whole, item.tint);
    }
    let clipped = painter.with_clip_rect(rect.intersect(clip));
    clipped.add(egui::Shape::mesh(mesh));
    if let Some(veil) = item.veil {
        clipped.rect_filled(rect, 0.0, veil);
    }
    painted
}
