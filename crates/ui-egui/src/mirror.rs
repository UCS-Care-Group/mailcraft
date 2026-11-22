// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Right-to-left layout by mirroring the whole window.
//!
//! egui lays everything out left to right. For a right-to-left language the finished frame is
//! mirrored about the vertical centre line, and pointer input is mirrored back, so panels, rows,
//! ribbons, scroll bars and text boxes all flip without every call site knowing. Glyphs, images
//! and icons keep their own orientation: they move to the mirrored position but are not flipped.

use egui::epaint::{ClippedShape, CornerRadius, Mesh, Shape};
use egui::{Context, Event, FullOutput, Plugin, Pos2, RawInput, Rect, Vec2};

/// Mirrors the frame about a vertical axis while the UI language is right to left.
#[derive(Default)]
pub struct Mirror {
    /// Sum of the screen's left and right edges: `x` maps to `axis - x`.
    axis: f32,
}

impl Mirror {
    pub fn install(ctx: &Context) {
        ctx.add_plugin(Self::default());
    }

    fn x(&self, x: f32) -> f32 {
        self.axis - x
    }

    fn pos(&self, p: Pos2) -> Pos2 {
        Pos2::new(self.x(p.x), p.y)
    }

    fn rect(&self, r: Rect) -> Rect {
        Rect::from_min_max(Pos2::new(self.x(r.max.x), r.min.y), Pos2::new(self.x(r.min.x), r.max.y))
    }

    /// Moves `r` to its mirrored place and returns the horizontal shift to apply to content in it.
    fn shift_for(&self, r: Rect) -> f32 {
        self.rect(r).min.x - r.min.x
    }

    fn shape(&self, shape: &mut Shape) {
        match shape {
            Shape::Noop => {}
            Shape::Vec(v) => v.iter_mut().for_each(|s| self.shape(s)),
            Shape::Circle(c) => c.center = self.pos(c.center),
            Shape::Ellipse(e) => {
                e.center = self.pos(e.center);
                e.angle = -e.angle;
            }
            Shape::LineSegment { points, .. } => points.iter_mut().for_each(|p| *p = self.pos(*p)),
            Shape::Path(p) => p.points.iter_mut().for_each(|p| *p = self.pos(*p)),
            Shape::Band(b) => b.points.iter_mut().for_each(|p| p.x = self.x(p.x)),
            Shape::Rect(r) => {
                let moved = self.rect(r.rect);
                if r.brush.is_some() {
                    // An image: keep its pixels facing the same way.
                    r.rect = Rect::from_min_size(moved.min, r.rect.size());
                } else {
                    r.rect = moved;
                    r.corner_radius = flip_corners(r.corner_radius);
                }
            }
            Shape::Text(t) => {
                let g = t.galley.rect;
                let left = self.x(t.pos.x + g.max.x);
                t.pos.x = left - g.min.x;
            }
            Shape::Mesh(m) => {
                let bounds = mesh_bounds(m);
                let dx = self.shift_for(bounds);
                if dx != 0.0 {
                    let mesh: &mut Mesh = std::sync::Arc::make_mut(m);
                    mesh.vertices.iter_mut().for_each(|v| v.pos.x += dx);
                }
            }
            Shape::QuadraticBezier(b) => b.points.iter_mut().for_each(|p| *p = self.pos(*p)),
            Shape::CubicBezier(b) => b.points.iter_mut().for_each(|p| *p = self.pos(*p)),
            Shape::Callback(c) => c.rect = self.rect(c.rect),
        }
    }
}

fn flip_corners(c: CornerRadius) -> CornerRadius {
    CornerRadius { nw: c.ne, ne: c.nw, sw: c.se, se: c.sw }
}

fn mesh_bounds(m: &Mesh) -> Rect {
    m.calc_bounds()
}

impl Plugin for Mirror {
    fn debug_name(&self) -> &'static str {
        "Mirror"
    }

    fn input_hook(&mut self, ctx: &Context, input: &mut RawInput) {
        if !crate::i18n::rtl() {
            return;
        }
        let screen = input.screen_rect.unwrap_or_else(|| ctx.content_rect());
        self.axis = screen.min.x + screen.max.x;
        for ev in &mut input.events {
            match ev {
                Event::PointerMoved(p) => *p = self.pos(*p),
                Event::PointerButton { pos, .. } | Event::Touch { pos, .. } => *pos = self.pos(*pos),
                Event::MouseWheel { delta, .. } => *delta = Vec2::new(-delta.x, delta.y),
                _ => {}
            }
        }
    }

    fn output_hook(&mut self, ctx: &Context, output: &mut FullOutput) {
        if !crate::i18n::rtl() {
            return;
        }
        let screen = ctx.content_rect();
        self.axis = screen.min.x + screen.max.x;
        for ClippedShape { clip_rect, shape, .. } in &mut output.shapes {
            *clip_rect = self.rect(*clip_rect);
            self.shape(shape);
        }
    }
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod mirror_tests;
