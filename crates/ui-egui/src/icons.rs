// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's icon set, drawn in code (original artwork; no external icon assets).
//!
//! Icons are designed on a 20×20 grid in one stroke: ink (`c`), an accent (`a`) on the one part
//! that is the action or the changed state, and a few theme colours for state marks.

use egui::epaint::{Mesh, TextShape};
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

/// Draws on the 20-unit grid of one icon, fitted to the screen's pixels.
///
/// Everything is worked out in device pixels: the grid's origin is a whole pixel, a stroke is a
/// whole number of pixels wide (one unit, and never less than one pixel), a straight stroke runs
/// down the middle of a row or column of pixels, and the edges of a fill lie between pixels. So
/// a 20 pt icon at 100 % is drawn in one-pixel lines that are sharp, not two grey half-lines.
struct Pen<'a> {
    p: &'a Painter,
    /// Device pixels per point.
    ppp: f32,
    /// The grid's (0, 0), in device pixels.
    o: Pos2,
    /// Device pixels per grid unit.
    k: f32,
    /// The stroke's width in device pixels.
    w: f32,
    c: Color32,
    a: Color32,
}

/// A line being drawn, in device pixels, from straight pieces and curves.
struct Path {
    pts: Vec<Pos2>,
    /// The last piece was an arc, whose end the next straight piece keeps to.
    after_arc: bool,
}

impl<'a> Pen<'a> {
    fn new(p: &'a Painter, r: Rect, c: Color32, a: Color32) -> Option<Self> {
        let ppp = p.pixels_per_point();
        let size = r.width().min(r.height());
        if !(size > 0.0 && size.is_finite() && ppp > 0.0 && ppp.is_finite()) {
            return None;
        }
        let k = size * ppp / 20.0;
        let corner = r.center() - vec2(size, size) / 2.0;
        let o = pos2((corner.x * ppp).round(), (corner.y * ppp).round());
        Some(Pen { p, ppp, o, k, w: k.round().max(1.0), c, a })
    }
    /// One grid unit in points.
    fn s(&self) -> f32 {
        self.k / self.ppp
    }
    /// A grid point in device pixels, as it falls.
    fn d(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.o.x + x * self.k, self.o.y + y * self.k)
    }
    /// A device-pixel coordinate moved to where a stroke of this width is sharp: the middle of a
    /// pixel for an odd width, the line between two for an even one.
    fn sv(&self, v: f32) -> f32 {
        if self.w as i32 % 2 == 1 { (v - 0.5).round() + 0.5 } else { v.round() }
    }
    /// A grid point for a stroke, in device pixels, snapped.
    fn sp(&self, x: f32, y: f32) -> Pos2 {
        let q = self.d(x, y);
        pos2(self.sv(q.x), self.sv(q.y))
    }
    /// A grid point for the edge of a fill, in device pixels: on the line between pixels.
    fn fp(&self, x: f32, y: f32) -> Pos2 {
        let q = self.d(x, y);
        pos2(q.x.round(), q.y.round())
    }
    /// Device pixels to points.
    fn out(&self, q: Pos2) -> Pos2 {
        pos2(q.x / self.ppp, q.y / self.ppp)
    }
    /// A grid point in points (not snapped).
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        self.out(self.d(x, y))
    }
    fn stroke(&self, c: Color32) -> Stroke {
        Stroke::new(self.w / self.ppp, c)
    }
    /// Stroke a line given in device pixels. Its ends and its corners are round: a stroke of
    /// one pixel is too thin to show it, a wider one has a dot of its width at each.
    fn stroke_px(&self, pts: &[Pos2], closed: bool, c: Color32) {
        if pts.len() < 2 {
            return;
        }
        let v: Vec<Pos2> = pts.iter().map(|q| self.out(*q)).collect();
        if self.w >= 2.0 && c.a() == 255 {
            let r = self.w / 2.0 / self.ppp;
            let n = v.len();
            for (i, q) in v.iter().enumerate() {
                let end = !closed && (i == 0 || i + 1 == n);
                let corner = || {
                    let (Some(a), Some(b)) = (v.get((i + n - 1) % n), v.get((i + 1) % n)) else { return false };
                    let (d1, d2) = (*q - *a, *b - *q);
                    let (l1, l2) = (d1.length(), d2.length());
                    l1 > 0.0 && l2 > 0.0 && d1.dot(d2) / (l1 * l2) < 0.94
                };
                if end || ((closed || (i > 0 && i + 1 < n)) && corner()) {
                    self.p.circle_filled(*q, r, c);
                }
            }
        }
        self.p.add(if closed { Shape::closed_line(v, self.stroke(c)) } else { Shape::line(v, self.stroke(c)) });
    }
    /// A polyline of straight strokes, its corners sharp.
    fn line(&self, pts: &[(f32, f32)]) {
        self.line_c(pts, self.c);
    }
    fn line_c(&self, pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.sp(x, y)).collect();
        self.stroke_px(&v, false, c);
    }
    /// A closed outline, its corners sharp.
    fn closed(&self, pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.sp(x, y)).collect();
        self.stroke_px(&v, true, c);
    }
    /// A polyline whose corners are rounded with radius `r` units (a box's one unit).
    fn rounded(&self, pts: &[(f32, f32)], closed: bool, r: f32, c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.sp(x, y)).collect();
        self.stroke_px(&round_corners(&v, closed, r * self.k), closed, c);
    }
    /// A box, its corners rounded by one unit.
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
        self.rounded(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], true, 1.0, c);
    }
    /// A filled box, its edges between pixels; its corners rounded by `r` units.
    fn frect_r(&self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, c: Color32) {
        let (a, b) = (self.out(self.fp(x0, y0)), self.out(self.fp(x1, y1)));
        let rect = Rect::from_min_max(a, b);
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        let radius = (r * self.s()).min(rect.width().min(rect.height()) / 2.0);
        self.p.rect_filled(rect, radius, c);
    }
    fn frect(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
        self.frect_r(x0, y0, x1, y1, 0.0, c);
    }
    /// A filled convex polygon, its corners on whole pixels.
    fn fill(&self, pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.out(self.fp(x, y))).collect();
        self.p.add(Shape::convex_polygon(v, c, Stroke::NONE));
    }
    /// A filled polygon that every corner of can be seen from `centre` (a flag, a star, a
    /// plane): one mesh, so its inside has no seams, with its outline stroked over the mesh's
    /// hard edge in the same colour.
    fn fill_shape(&self, centre: (f32, f32), pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.sp(x, y)).collect();
        let mut mesh = Mesh::default();
        mesh.colored_vertex(self.out(self.d(centre.0, centre.1)), c);
        for q in &v {
            mesh.colored_vertex(self.out(*q), c);
        }
        let n = v.len() as u32;
        for i in 0..n {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
        }
        self.p.add(Shape::mesh(mesh));
        self.stroke_px(&v, true, c);
    }
    /// Where a circle centred at (`x`, `y`) of radius `r` units is drawn: its centre snapped for
    /// a stroke, its radius a whole number of pixels.
    fn arc_geom(&self, x: f32, y: f32, r: f32) -> (Pos2, f32) {
        let q = self.d(x, y);
        (pos2(self.sv(q.x), self.sv(q.y)), (r * self.k).round().max(1.0))
    }
    fn circle(&self, x: f32, y: f32, r: f32, c: Color32) {
        let (q, rr) = self.arc_geom(x, y, r);
        self.p.circle_stroke(self.out(q), rr / self.ppp, self.stroke(c));
    }
    /// A dot. A tiny one sits on a pixel; a bigger one has its edge between pixels.
    fn fcircle(&self, x: f32, y: f32, r: f32, c: Color32) {
        let q = self.d(x, y);
        let rr = r * self.k;
        let (q, rr) =
            if rr >= 1.25 { (pos2(q.x.round(), q.y.round()), rr.round()) } else { (pos2(q.x.floor() + 0.5, q.y.floor() + 0.5), rr.max(0.6)) };
        self.p.circle_filled(self.out(q), rr / self.ppp, c);
    }
    /// An arc of a circle from `from` to `to` degrees (0 = east, clockwise; either way round).
    fn arc(&self, x: f32, y: f32, r: f32, from: f32, to: f32, c: Color32) {
        self.path().a(self, x, y, r, from, to).draw(self, c);
    }
    /// An ellipse, or an arc of one, from `from` to `to` degrees (all curve, so not snapped but
    /// for its centre).
    #[allow(clippy::too_many_arguments)]
    fn ellipse_arc(&self, x: f32, y: f32, rx: f32, ry: f32, from: f32, to: f32, c: Color32) {
        let steps = (((to - from).abs() / 360.0) * (rx.max(ry) * self.k * 3.0)).clamp(8.0, 64.0) as usize;
        let q = self.d(x, y);
        let centre = pos2(self.sv(q.x), self.sv(q.y));
        let (rx, ry) = ((rx * self.k).round().max(1.0), (ry * self.k).round().max(1.0));
        let v: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let t = (from + (to - from) * i as f32 / steps as f32).to_radians();
                centre + vec2(rx * t.cos(), ry * t.sin())
            })
            .collect();
        self.stroke_px(&v, false, c);
    }
    fn path(&self) -> Path {
        Path { pts: Vec::new(), after_arc: false }
    }
    /// Text in the interface's semibold face, centred on (`x`, `y`), `size` units high, turned
    /// clockwise by `angle` radians.
    #[allow(clippy::too_many_arguments)]
    fn text(&self, x: f32, y: f32, size: f32, t: &str, c: Color32, angle: f32) {
        let font: FontId = crate::theme::semibold(size * self.s());
        let galley = self.p.layout_no_wrap(t.to_string(), font, c);
        let at = self.pt(x, y) - galley.size() / 2.0;
        self.p.add(Shape::Text(TextShape::new(at, galley, c).with_angle_and_anchor(angle, Align2::CENTER_CENTER)));
    }
    /// An open arrowhead (a chevron) with its tip at `tip` (device pixels), pointing at `dir`
    /// degrees, its arms `len` units long.
    fn head_px(&self, tip: Pos2, dir: f32, len: f32, c: Color32) {
        let arm = |turn: f32| {
            let t = (dir + 180.0 + turn).to_radians();
            let q = tip + vec2(t.cos(), t.sin()) * len * self.k;
            pos2(self.sv(q.x), self.sv(q.y))
        };
        self.stroke_px(&[arm(-45.0), tip, arm(45.0)], false, c);
    }
    fn head(&self, x: f32, y: f32, dir: f32, len: f32, c: Color32) {
        self.head_px(self.sp(x, y), dir, len, c);
    }
}

impl Path {
    /// Start at a grid point.
    fn m(mut self, pen: &Pen<'_>, x: f32, y: f32) -> Self {
        self.pts.push(pen.sp(x, y));
        self.after_arc = false;
        self
    }
    /// A straight piece to a grid point. After an arc it keeps to the arc's end along the axis it
    /// nearly shares, so the join has no kink.
    fn l(mut self, pen: &Pen<'_>, x: f32, y: f32) -> Self {
        let mut q = pen.sp(x, y);
        if self.after_arc
            && let Some(prev) = self.pts.last()
        {
            if (q.x - prev.x).abs() < 1.5 {
                q.x = prev.x;
            }
            if (q.y - prev.y).abs() < 1.5 {
                q.y = prev.y;
            }
        }
        self.pts.push(q);
        self.after_arc = false;
        self
    }
    /// An arc from `from` to `to` degrees (0 = east, clockwise), joined to what came before. The
    /// point before it moves onto the arc's start along the axis it nearly shares.
    #[allow(clippy::too_many_arguments)]
    fn a(mut self, pen: &Pen<'_>, x: f32, y: f32, r: f32, from: f32, to: f32) -> Self {
        let (centre, rr) = pen.arc_geom(x, y, r);
        let at = |deg: f32| {
            let t = deg.to_radians();
            centre + vec2(rr * t.cos(), rr * t.sin())
        };
        let start = at(from);
        if let Some(last) = self.pts.last_mut() {
            if (last.x - start.x).abs() < 1.5 {
                last.x = start.x;
            }
            if (last.y - start.y).abs() < 1.5 {
                last.y = start.y;
            }
        }
        let steps = (((to - from).abs() / 360.0) * rr * 3.0).clamp(4.0, 64.0) as usize;
        for i in 0..=steps {
            self.pts.push(at(from + (to - from) * i as f32 / steps as f32));
        }
        self.after_arc = true;
        self
    }
    /// A cubic curve from where the line is to the grid point (`x`, `y`).
    fn c(mut self, pen: &Pen<'_>, c1: (f32, f32), c2: (f32, f32), x: f32, y: f32) -> Self {
        let Some(&p0) = self.pts.last() else { return self.m(pen, x, y) };
        let (p1, p2, p3) = (pen.d(c1.0, c1.1), pen.d(c2.0, c2.1), pen.sp(x, y));
        let steps = ((p3 - p0).length() / 1.5).clamp(6.0, 32.0) as usize;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            let q = p0.to_vec2() * (u * u * u) + p1.to_vec2() * (3.0 * u * u * t) + p2.to_vec2() * (3.0 * u * t * t) + p3.to_vec2() * (t * t * t);
            self.pts.push(q.to_pos2());
        }
        self.after_arc = false;
        self
    }
    fn last(&self) -> Option<Pos2> {
        self.pts.last().copied()
    }
    fn draw(&self, pen: &Pen<'_>, c: Color32) {
        pen.stroke_px(&self.pts, false, c);
    }
    fn draw_closed(&self, pen: &Pen<'_>, c: Color32) {
        pen.stroke_px(&self.pts, true, c);
    }
}

/// Round a polyline's corners (device pixels) with radius `r` pixels.
fn round_corners(pts: &[Pos2], closed: bool, r: f32) -> Vec<Pos2> {
    let n = pts.len();
    if r < 0.5 || n < 3 {
        return pts.to_vec();
    }
    let mut out = Vec::with_capacity(n * 5);
    for (i, &v) in pts.iter().enumerate() {
        if !closed && (i == 0 || i + 1 == n) {
            out.push(v);
            continue;
        }
        let (Some(&prev), Some(&next)) = (pts.get((i + n - 1) % n), pts.get((i + 1) % n)) else { continue };
        let (d1, d2): (Vec2, Vec2) = (prev - v, next - v);
        let (l1, l2) = (d1.length(), d2.length());
        if l1 < 0.01 || l2 < 0.01 {
            out.push(v);
            continue;
        }
        let t = r.min(l1 / 2.0).min(l2 / 2.0);
        let (p1, p2) = (v + d1 / l1 * t, v + d2 / l2 * t);
        for s in 0..=4 {
            let u = s as f32 / 4.0;
            let q = p1.to_vec2() * ((1.0 - u) * (1.0 - u)) + v.to_vec2() * (2.0 * u * (1.0 - u)) + p2.to_vec2() * (u * u);
            out.push(q.to_pos2());
        }
    }
    out
}

/// The shapes several icons share.
impl Pen<'_> {
    /// A box with its bottom-right corner left open for a modifier (the "new" plus, a badge):
    /// nothing of it comes within a unit of the modifier's zone (12–18 both ways).
    fn cut_box(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
        self.rounded(&[(x1, 10.0), (x1, y0), (x0, y0), (x0, y1), (10.0, y1)], false, 1.0, c);
    }
    /// An envelope's flap, from its top corners to the middle.
    fn flap(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
        self.line_c(&[(x0 + 1.0, y0 + 1.0), ((x0 + x1) / 2.0, y0 + (y1 - y0) * 0.5), (x1 - 1.0, y0 + 1.0)], c);
    }
    /// An envelope: a box with its flap.
    fn envelope(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
        self.rect(x0, y0, x1, y1, c);
        self.flap(x0, y0, x1, y1, c);
    }
    /// The one "new" modifier: a plus in the accent, bottom right.
    fn new_plus(&self) {
        self.line_c(&[(12.5, 15.0), (17.5, 15.0)], self.a);
        self.line_c(&[(15.0, 12.5), (15.0, 17.5)], self.a);
    }
    /// A cross in the modifier's zone.
    fn badge_x(&self, c: Color32) {
        self.line_c(&[(12.5, 12.5), (17.5, 17.5)], c);
        self.line_c(&[(17.5, 12.5), (12.5, 17.5)], c);
    }
    /// A tick in the modifier's zone.
    fn badge_check(&self, c: Color32) {
        self.line_c(&[(12.0, 15.0), (14.25, 17.25), (18.0, 12.75)], c);
    }
    /// The one tick.
    fn check(&self, x: f32, y: f32, k: f32, c: Color32) {
        self.line_c(&[(x - 3.5 * k, y), (x - 1.0 * k, y + 2.5 * k), (x + 3.75 * k, y - 3.0 * k)], c);
    }
    fn plus(&self, x: f32, y: f32, k: f32, c: Color32) {
        self.line_c(&[(x - k, y), (x + k, y)], c);
        self.line_c(&[(x, y - k), (x, y + k)], c);
    }
    /// A folder, whole or with its corner open for a modifier.
    fn folder(&self, cut: bool, c: Color32) {
        if cut {
            self.rounded(&[(17.5, 10.0), (17.5, 6.5), (9.5, 6.5), (7.5, 4.5), (2.5, 4.5), (2.5, 15.5), (10.0, 15.5)], false, 1.0, c);
        } else {
            self.rounded(&[(2.5, 4.5), (7.5, 4.5), (9.5, 6.5), (17.5, 6.5), (17.5, 15.5), (2.5, 15.5)], true, 1.0, c);
        }
    }
    /// A tray with its well, open at the top (the Inbox, the Outbox).
    fn tray(&self, c: Color32) {
        self.rounded(&[(2.5, 10.5), (2.5, 15.5), (17.5, 15.5), (17.5, 10.5)], false, 1.0, c);
        self.line_c(&[(2.5, 10.5), (6.5, 10.5), (7.5, 12.5), (12.5, 12.5), (13.5, 10.5), (17.5, 10.5)], c);
    }
    /// A bracket open at the top, that something goes into or comes out of.
    fn bracket(&self, c: Color32) {
        self.rounded(&[(3.5, 11.5), (3.5, 16.5), (16.5, 16.5), (16.5, 11.5)], false, 1.0, c);
    }
    /// A bin's lid and handle; `tilted` when it is lifted.
    fn lid(&self, tilted: bool, c: Color32) {
        if tilted {
            self.line_c(&[(3.5, 6.0), (15.5, 2.5)], c);
        } else {
            self.line_c(&[(3.0, 5.5), (17.0, 5.5)], c);
            self.rounded(&[(7.5, 5.5), (7.5, 3.0), (12.5, 3.0), (12.5, 5.5)], false, 1.0, c);
        }
    }
    /// A calendar page: its binding and a line under its header. `cut` leaves its corner open.
    fn calendar(&self, cut: bool, c: Color32) {
        if cut {
            self.cut_box(2.5, 4.5, 17.5, 16.5, c);
        } else {
            self.rect(2.5, 4.5, 17.5, 16.5, c);
        }
        self.line_c(&[(2.5, 8.0), (17.5, 8.0)], c);
        self.line_c(&[(6.5, 2.5), (6.5, 5.5)], c);
        self.line_c(&[(13.5, 2.5), (13.5, 5.5)], c);
    }
    /// A head and shoulders: the head at (`x`, `hy`) of radius `hr`; the shoulders an arc round
    /// (`x`, `sy`) of radius `sr`, from `from` to `to` degrees.
    #[allow(clippy::too_many_arguments)]
    fn person(&self, x: f32, hy: f32, hr: f32, sy: f32, sr: f32, from: f32, to: f32, c: Color32) {
        self.circle(x, hy, hr, c);
        self.arc(x, sy, sr, from, to, c);
    }
    /// A question mark of strokes, `s` times the size of one 10 units high, centred on (x, y).
    fn question(&self, x: f32, y: f32, s: f32, c: Color32) {
        self.path().a(self, x, y - 2.25 * s, 2.75 * s, 180.0, 405.0).l(self, x, y + 1.0 * s).l(self, x, y + 2.0 * s).draw(self, c);
        self.fcircle(x, y + 4.25 * s, 0.9 * s, c);
    }
    /// A line through grid points that are all curve (not snapped), turned by `turn` degrees
    /// round the middle of the grid.
    fn curve(&self, pts: &[(f32, f32)], turn: f32, c: Color32) {
        let (sin, cos) = turn.to_radians().sin_cos();
        let v: Vec<Pos2> = pts
            .iter()
            .map(|&(x, y)| {
                let (dx, dy) = (x - 10.0, y - 10.0);
                self.d(10.0 + dx * cos - dy * sin, 10.0 + dx * sin + dy * cos)
            })
            .collect();
        self.stroke_px(&v, false, c);
    }
    /// Little boxes in a grid of `cols` by `rows` inside (`x0`, `y0`)–(`x1`, `y1`), each a whole
    /// number of pixels with a gap of a stroke's width between them: the days of a calendar
    /// view. The one at `lit` is in `hi`, the others in `c`.
    #[allow(clippy::too_many_arguments)]
    fn cells(&self, (x0, y0, x1, y1): (f32, f32, f32, f32), cols: u32, rows: u32, lit: (u32, u32), c: Color32, hi: Color32) {
        let (a, b) = (self.fp(x0, y0), self.fp(x1, y1));
        let g = self.w;
        let fit = |len: f32, n: u32| {
            let n = n.max(1) as f32;
            let cell = ((len - (n - 1.0) * g) / n).floor().max(1.0);
            let used = cell * n + (n - 1.0) * g;
            (cell, ((len - used) / 2.0).floor().max(0.0))
        };
        let ((cw, ox), (ch, oy)) = (fit(b.x - a.x, cols), fit(b.y - a.y, rows));
        for row in 0..rows {
            for col in 0..cols {
                let min = pos2(a.x + ox + col as f32 * (cw + g), a.y + oy + row as f32 * (ch + g));
                let rect = Rect::from_min_size(self.out(min), vec2(cw, ch) / self.ppp);
                self.p.rect_filled(rect, 0.0, if (col, row) == lit { hi } else { c });
            }
        }
    }
}

/// Paint icon `name` into `r`, drawn on a 20-unit grid in one stroke.
///
/// `c` is the ink. `accent` marks the one part that is the action or the state that changed (a
/// "new" plus, an arrow that moves something, the dot of what is unread); a few state marks have
/// their own colours from the theme (a flag, a star, a tick, the categories). When `accent` is
/// `c` (a disabled button, the module bar), every part is drawn in `c`. Below 16 pt a few icons
/// leave out the detail that would not hold at that size.
pub fn paint(p: &Painter, r: Rect, name: &str, c: Color32, accent: Color32) {
    let Some(pen) = Pen::new(p, r, c, accent) else { return };
    let t = crate::theme::Tokens::get(p.ctx());
    let mono = c == accent;
    let hue = |h: Color32| if mono { c } else { h };
    let a = accent;
    let (red, yellow, green, purple, on_accent) = (hue(t.icon_red), hue(t.icon_yellow), hue(t.icon_green), hue(t.icon_purple), t.on_accent);
    let size = r.width().min(r.height());
    let small = size <= 16.5;
    let tiny = size <= 12.5;
    let quiet = c.linear_multiply(0.7);
    match name {
        // Mail
        "mail" | "email" => pen.envelope(2.5, 4.5, 17.5, 15.5, c),
        "unread" | "markUnread" => {
            pen.cut_box(2.5, 4.5, 17.5, 15.5, c);
            pen.flap(2.5, 4.5, 17.5, 15.5, c);
            pen.fcircle(15.0, 15.0, 2.5, a);
        }
        "new" | "newMail" => {
            pen.cut_box(2.5, 4.5, 17.5, 15.5, c);
            pen.flap(2.5, 4.5, 17.5, 15.5, c);
            pen.new_plus();
        }
        "read" | "markRead" | "toggleRead" => {
            pen.rounded(&[(2.5, 8.5), (10.0, 3.5), (17.5, 8.5), (17.5, 16.5), (2.5, 16.5)], true, 1.0, c);
            pen.line(&[(2.5, 8.5), (10.0, 13.0), (17.5, 8.5)]);
        }
        "markAllRead" => {
            pen.rounded(&[(2.5, 8.5), (10.0, 3.5), (17.5, 8.5), (17.5, 16.5), (2.5, 16.5)], true, 1.0, c);
            pen.check(10.0, 12.0, 0.9, green);
        }
        "reply" => {
            let tail = pen.path().m(&pen, 3.5, 8.5).l(&pen, 10.0, 8.5).c(&pen, (13.5, 8.5), (16.5, 11.0), 16.5, 15.5);
            tail.draw(&pen, c);
            pen.head(3.5, 8.5, 180.0, 4.0, c);
        }
        "replyAll" => {
            let tail = pen.path().m(&pen, 7.0, 8.5).l(&pen, 11.5, 8.5).c(&pen, (14.5, 8.5), (17.0, 11.0), 17.0, 15.5);
            tail.draw(&pen, c);
            pen.head(7.0, 8.5, 180.0, 4.0, c);
            pen.head(2.5, 8.5, 180.0, 4.0, c);
        }
        "forward" => {
            let tail = pen.path().m(&pen, 16.5, 8.5).l(&pen, 10.0, 8.5).c(&pen, (6.5, 8.5), (3.5, 11.0), 3.5, 15.5);
            tail.draw(&pen, c);
            pen.head(16.5, 8.5, 0.0, 4.0, c);
        }
        // Forward, higher, and the message it carries whole: an envelope in the modifier's corner.
        "forwardAsAttachment" => {
            let tail = pen.path().m(&pen, 16.5, 5.5).l(&pen, 10.0, 5.5).c(&pen, (6.5, 5.5), (3.5, 8.0), 3.5, 12.5);
            tail.draw(&pen, c);
            pen.head(16.5, 5.5, 0.0, 4.0, c);
            pen.envelope(11.5, 12.0, 18.0, 17.0, c);
        }
        "delete" | "deleted" | "discard" | "remove" => {
            pen.lid(false, c);
            pen.rounded(&[(5.0, 5.5), (5.0, 16.5), (15.0, 16.5), (15.0, 5.5)], false, 1.0, c);
            if !tiny {
                pen.line(&[(8.5, 8.5), (8.5, 13.5)]);
                pen.line(&[(11.5, 8.5), (11.5, 13.5)]);
            }
        }
        // For good: the bin, and a cross that says so (not its colour alone).
        "deletePermanently" => {
            pen.lid(false, c);
            pen.rounded(&[(5.0, 5.5), (5.0, 16.5), (10.0, 16.5)], false, 1.0, c);
            pen.line(&[(15.0, 5.5), (15.0, 10.0)]);
            pen.badge_x(red);
        }
        // Emptied: the bin with its lid lifted.
        "empty" | "emptyFolder" => {
            pen.lid(true, c);
            pen.rounded(&[(5.0, 7.5), (5.0, 16.5), (15.0, 16.5), (15.0, 7.5)], false, 1.0, c);
        }
        "archive" => {
            pen.rect(2.5, 3.5, 17.5, 7.5, c);
            pen.rounded(&[(3.5, 7.5), (3.5, 16.5), (16.5, 16.5), (16.5, 7.5)], false, 1.0, c);
            pen.line(&[(8.0, 10.5), (12.0, 10.5)]);
        }
        "junk" => {
            pen.circle(10.0, 10.0, 7.0, c);
            pen.line(&[(5.0, 5.0), (15.0, 15.0)]);
        }
        "notJunk" => {
            pen.arc(8.5, 8.5, 6.0, 66.0, 384.0, c);
            pen.line(&[(4.5, 4.5), (10.0, 10.0)]);
            pen.badge_check(green);
        }
        "move" => {
            pen.folder(false, c);
            pen.line_c(&[(6.0, 11.0), (13.5, 11.0)], a);
            pen.head(13.5, 11.0, 0.0, 2.75, a);
        }
        "copy" => {
            pen.rounded(&[(5.5, 13.5), (2.5, 13.5), (2.5, 2.5), (12.5, 2.5), (12.5, 4.5)], false, 1.0, c);
            pen.rect(7.5, 6.5, 17.5, 17.5, c);
        }
        // A swallowtail on its pole: filled when flagged, an outline when not.
        "flag" | "toggleFlag" | "followUp" => {
            pen.fill_shape((8.0, 7.0), &[(4.5, 3.5), (16.0, 3.5), (12.5, 7.5), (16.0, 11.5), (4.5, 11.5)], red);
            pen.line(&[(4.5, 2.5), (4.5, 17.5)]);
        }
        "clearFlag" => {
            pen.closed(&[(4.5, 3.5), (16.0, 3.5), (12.5, 7.5), (16.0, 11.5), (4.5, 11.5)], c);
            pen.line(&[(4.5, 2.5), (4.5, 17.5)]);
        }
        "complete" | "check" | "accept" | "tick" => pen.check(10.0, 10.5, 1.5, green),
        "categorize" | "category" | "categories" => {
            pen.fcircle(6.5, 6.5, 2.75, red);
            pen.fcircle(13.5, 6.5, 2.75, yellow);
            pen.fcircle(6.5, 13.5, 2.75, green);
            pen.fcircle(13.5, 13.5, 2.75, purple);
        }
        "pin" => {
            pen.rounded(&[(7.5, 2.5), (12.5, 2.5), (12.0, 8.0), (14.5, 10.5), (5.5, 10.5), (8.0, 8.0)], true, 0.5, c);
            pen.line(&[(10.0, 10.5), (10.0, 17.5)]);
        }
        // A message, and an arrow that takes it somewhere.
        "rules" | "createRule" => {
            pen.envelope(2.5, 2.5, 12.5, 9.5, c);
            pen.rounded(&[(7.5, 11.5), (7.5, 15.0), (16.5, 15.0)], false, 2.0, a);
            pen.head(16.5, 15.0, 0.0, 2.75, a);
        }
        // A paper clip on the slant, its ends round.
        "attach" | "attachment" => {
            let mut pts = vec![(13.0, 6.0)];
            let arc = |pts: &mut Vec<(f32, f32)>, cx: f32, cy: f32, r: f32, from: f32, to: f32| {
                for i in 0..=12 {
                    let t = (from + (to - from) * i as f32 / 12.0).to_radians();
                    pts.push((cx + r * t.cos(), cy + r * t.sin()));
                }
            };
            arc(&mut pts, 10.0, 13.5, 3.0, 0.0, 180.0);
            arc(&mut pts, 8.5, 5.0, 1.5, 180.0, 360.0);
            pts.push((10.0, 11.5));
            pen.curve(&pts, 40.0, c);
        }
        // One paper plane: filled when it is the action, an outline for the folder.
        "send" | "sent" => {
            let plane = [(2.5, 3.5), (17.5, 10.0), (2.5, 16.5), (5.5, 10.0)];
            if name == "send" && !mono {
                pen.fill_shape((8.0, 10.0), &plane, a);
                pen.line_c(&[(5.5, 10.0), (11.0, 10.0)], on_accent);
            } else {
                pen.closed(&plane, c);
                pen.line(&[(5.5, 10.0), (11.0, 10.0)]);
            }
        }
        "drafts" | "editDraft" | "edit" | "rename" => {
            pen.closed(&[(3.5, 16.5), (4.25, 12.75), (13.0, 4.0), (16.0, 7.0), (7.25, 15.75)], c);
            pen.line(&[(11.0, 6.0), (14.0, 9.0)]);
        }
        "inbox" => {
            pen.tray(c);
            pen.line(&[(10.0, 2.5), (10.0, 8.5)]);
            pen.head(10.0, 8.5, 90.0, 2.75, c);
        }
        "outbox" => {
            pen.tray(c);
            pen.line(&[(10.0, 8.5), (10.0, 2.5)]);
            pen.head(10.0, 2.5, 270.0, 2.75, c);
        }
        "import" => {
            pen.bracket(c);
            pen.line_c(&[(10.0, 2.5), (10.0, 12.5)], a);
            pen.head(10.0, 12.5, 90.0, 3.0, a);
        }
        "export" => {
            pen.bracket(c);
            pen.line_c(&[(10.0, 12.5), (10.0, 2.5)], a);
            pen.head(10.0, 2.5, 270.0, 3.0, a);
        }
        // An open folder.
        "open" => {
            pen.rounded(&[(15.5, 9.0), (15.5, 6.5), (9.5, 6.5), (7.5, 4.5), (2.5, 4.5), (2.5, 15.5)], false, 1.0, c);
            pen.rounded(&[(2.5, 15.5), (5.0, 9.0), (17.5, 9.0), (15.0, 15.5)], true, 0.5, c);
        }
        "folder" | "custom" | "select" => pen.folder(false, c),
        "folderNew" | "newFolder" => {
            pen.folder(true, c);
            pen.new_plus();
        }
        "favorite" | "star" | "starOn" => {
            let pts =
                [(10.0, 2.5), (12.1, 7.4), (17.5, 7.9), (13.4, 11.5), (14.6, 16.8), (10.0, 14.0), (5.4, 16.8), (6.6, 11.5), (2.5, 7.9), (7.9, 7.4)];
            if name == "starOn" {
                pen.fill_shape((10.0, 10.2), &pts, yellow);
            } else {
                pen.closed(&pts, c);
            }
        }
        // Compose
        "save" | "saveDraft" => {
            pen.rounded(&[(3.5, 3.5), (13.5, 3.5), (16.5, 6.5), (16.5, 16.5), (3.5, 16.5)], true, 1.0, c);
            pen.rounded(&[(6.5, 3.5), (6.5, 7.5), (12.5, 7.5), (12.5, 3.5)], false, 1.0, c);
            if !tiny {
                pen.rounded(&[(6.0, 16.5), (6.0, 11.5), (14.0, 11.5), (14.0, 16.5)], false, 1.0, c);
            }
        }
        "highImportance" | "importanceHigh" => {
            pen.frect_r(9.0, 2.5, 11.0, 12.5, 1.0, red);
            pen.fcircle(10.0, 15.75, 1.25, red);
        }
        "lowImportance" | "importanceLow" => {
            pen.line_c(&[(10.0, 3.0), (10.0, 16.5)], a);
            pen.head(10.0, 16.5, 90.0, 4.5, a);
        }
        "signature" => {
            pen.path()
                .m(&pen, 3.0, 13.0)
                .c(&pen, (4.0, 7.0), (7.0, 5.5), 7.5, 9.5)
                .c(&pen, (8.0, 13.5), (9.5, 14.0), 11.0, 10.5)
                .c(&pen, (12.0, 8.5), (13.0, 10.0), 14.0, 12.0)
                .c(&pen, (14.5, 13.0), (16.0, 12.5), 17.0, 11.0)
                .draw(&pen, c);
            pen.line(&[(2.5, 16.5), (17.5, 16.5)]);
        }
        "checkNames" => {
            pen.person(7.0, 7.0, 2.75, 17.5, 4.5, 180.0, 360.0, c);
            pen.badge_check(green);
        }
        "close" | "cancel" | "decline" => {
            let k = if name == "decline" { red } else { c };
            pen.line_c(&[(5.0, 5.0), (15.0, 15.0)], k);
            pen.line_c(&[(15.0, 5.0), (5.0, 15.0)], k);
        }
        "tentative" => pen.question(10.0, 10.0, 1.2, c),
        // Modules and navigation
        "calendar" | "appointment" => {
            pen.calendar(false, c);
            if !small {
                for (x, y) in [(6.5, 11.0), (10.0, 11.0), (13.5, 11.0), (6.5, 14.0), (10.0, 14.0)] {
                    pen.fcircle(x, y, 0.75, c);
                }
            }
        }
        "newAppointment" | "newEvent" => {
            pen.calendar(true, c);
            pen.new_plus();
        }
        "today" => {
            pen.calendar(false, c);
            pen.frect_r(8.0, 10.0, 12.0, 14.0, 1.0, a);
        }
        // A view of days: one, the five of a working week, seven, and a month's weeks.
        "day" | "workWeek" | "week" | "month" => {
            pen.rect(2.5, 3.5, 17.5, 16.5, c);
            pen.line(&[(2.5, 6.5), (17.5, 6.5)]);
            let inside = (4.0, 8.0, 16.0, 15.0);
            match name {
                "day" => pen.cells(inside, 1, 3, (0, 1), quiet, a),
                "workWeek" => pen.cells(inside, 5, 1, (2, 0), quiet, a),
                "week" => pen.cells((3.0, 8.0, 17.0, 15.0), 7, 1, (3, 0), quiet, a),
                _ => pen.cells(inside, 3, 3, (1, 1), quiet, a),
            }
        }
        "people" | "contact" | "account" => pen.person(10.0, 6.5, 3.0, 17.5, 6.5, 180.0, 360.0, c),
        "newContact" => {
            pen.person(7.0, 7.5, 2.75, 17.5, 4.0, 180.0, 360.0, c);
            pen.new_plus();
        }
        // A person, and a magnifier over the modifier's zone: their mail, looked for.
        "findFromSender" => {
            pen.person(7.0, 7.5, 2.75, 17.5, 4.0, 180.0, 360.0, c);
            pen.circle(14.0, 13.5, 2.75, a);
            pen.line_c(&[(16.0, 15.5), (18.0, 17.5)], a);
        }
        "meeting" | "fromMessage" => {
            pen.person(7.5, 7.5, 2.75, 17.5, 5.0, 180.0, 360.0, c);
            pen.person(13.5, 5.5, 2.25, 14.5, 4.0, 250.0, 360.0, c);
        }
        // Two people side by side, and the modifier's lines that bring them together into one
        // (Merge Duplicates, #506).
        "mergeContacts" => {
            pen.person(5.5, 5.5, 2.25, 13.5, 3.5, 180.0, 360.0, c);
            pen.person(14.5, 5.5, 2.25, 13.5, 3.5, 180.0, 360.0, c);
            pen.line_c(&[(5.5, 14.5), (10.0, 18.0), (14.5, 14.5)], a);
        }
        "newMeeting" => {
            pen.person(7.0, 8.0, 2.75, 17.5, 4.0, 180.0, 360.0, c);
            pen.person(12.5, 5.0, 2.0, 13.0, 3.5, 245.0, 320.0, c);
            pen.new_plus();
        }
        // A calendar page that came by mail: an invitation (the flap of its envelope), its
        // cancellation, or somebody's answer to one.
        "meetingRequest" | "meetingCancelled" | "meetingAccepted" | "meetingTentative" | "meetingDeclined" => {
            pen.calendar(false, c);
            match name {
                "meetingRequest" => pen.line(&[(5.5, 10.5), (10.0, 13.5), (14.5, 10.5)]),
                "meetingAccepted" => pen.check(10.0, 12.5, 0.85, green),
                "meetingTentative" => pen.question(10.0, 12.0, 0.55, c),
                // Called off: struck through.
                "meetingCancelled" => pen.line_c(&[(4.0, 16.0), (16.0, 6.5)], red),
                _ => {
                    pen.line_c(&[(7.5, 10.0), (12.5, 15.0)], red);
                    pen.line_c(&[(12.5, 10.0), (7.5, 15.0)], red);
                }
            }
        }
        "tasks" | "task" => {
            pen.rect(3.5, 3.5, 16.5, 16.5, c);
            pen.check(10.0, 10.5, 1.0, green);
        }
        "newTask" => {
            pen.cut_box(3.5, 3.5, 16.5, 16.5, c);
            pen.check(9.0, 9.5, 0.9, green);
            pen.new_plus();
        }
        "next" | "forwardPeriod" => pen.line(&[(7.5, 4.5), (13.0, 10.0), (7.5, 15.5)]),
        "previous" | "back" => pen.line(&[(12.5, 4.5), (7.0, 10.0), (12.5, 15.5)]),
        // A bell: its dome an arc, its rim, and the clapper below.
        "reminder" | "bell" => {
            pen.path()
                .m(&pen, 3.5, 14.5)
                .l(&pen, 5.0, 13.0)
                .l(&pen, 5.0, 9.0)
                .a(&pen, 10.0, 9.0, 5.0, 180.0, 360.0)
                .l(&pen, 15.0, 13.0)
                .l(&pen, 16.5, 14.5)
                .draw_closed(&pen, c);
            pen.line(&[(10.0, 2.5), (10.0, 4.0)]);
            pen.arc(10.0, 14.5, 2.0, 0.0, 180.0, c);
        }
        "private" | "lock" => {
            pen.rect(4.5, 9.5, 15.5, 16.5, c);
            pen.path().m(&pen, 7.0, 9.5).l(&pen, 7.0, 6.5).a(&pen, 10.0, 6.5, 3.0, 180.0, 360.0).l(&pen, 13.0, 9.5).draw(&pen, c);
            if !tiny {
                pen.fcircle(10.0, 13.0, 1.0, c);
            }
        }
        // Again and again: a loop of two halves, each with its head.
        "recurrence" | "repeat" => {
            pen.rounded(&[(3.0, 11.5), (3.0, 5.5), (16.0, 5.5)], false, 2.5, c);
            pen.head(16.0, 5.5, 0.0, 2.5, c);
            pen.rounded(&[(17.0, 8.5), (17.0, 14.5), (4.0, 14.5)], false, 2.5, c);
            pen.head(4.0, 14.5, 180.0, 2.5, c);
        }
        // People's time in rows, and the meeting's own time down across them.
        "scheduling" => {
            pen.line_c(&[(8.5, 2.5), (8.5, 17.5)], a);
            pen.line_c(&[(12.5, 2.5), (12.5, 17.5)], a);
            pen.frect(2.5, 5.0, 7.0, 7.0, c);
            pen.frect(14.0, 9.0, 17.5, 11.0, c);
            pen.frect(3.5, 13.0, 7.0, 15.0, c);
            pen.frect(14.0, 13.0, 16.5, 15.0, c);
        }
        // Two arrows chasing each other round.
        "sync" | "request" | "sendReceive" => {
            let top = pen.path().a(&pen, 10.0, 10.0, 6.5, 200.0, 325.0);
            top.draw(&pen, c);
            let bottom = pen.path().a(&pen, 10.0, 10.0, 6.5, 20.0, 145.0);
            bottom.draw(&pen, c);
            if let (Some(t1), Some(t2)) = (top.last(), bottom.last()) {
                pen.head_px(t1, 55.0, 3.0, c);
                pen.head_px(t2, 235.0, 3.0, c);
            }
        }
        // Send/Receive Progress: accounts one under another, how far each has got.
        "syncProgress" => {
            for (n, done) in [(0.0, 13.0), (1.0, 8.0), (2.0, 4.5)] {
                let y = 3.5 + n * 5.0;
                pen.frect_r(2.5, y, done, y + 3.0, 1.0, a);
                pen.rect(2.5, y, 17.5, y + 3.0, c);
            }
        }
        "showAs" => {
            pen.frect_r(3.0, 3.0, 7.5, 17.0, 1.0, a);
            pen.rect(3.5, 3.5, 16.5, 16.5, c);
        }
        // View
        "search" | "find" | "findRelated" => {
            pen.circle(8.5, 8.5, 5.5, c);
            pen.line(&[(12.5, 12.5), (17.0, 17.0)]);
        }
        "filter" => pen.rounded(&[(2.5, 4.0), (17.5, 4.0), (11.5, 10.5), (11.5, 16.5), (8.5, 14.5), (8.5, 10.5)], true, 0.5, c),
        "readingPane" => {
            pen.rect(2.5, 3.5, 17.5, 16.5, c);
            pen.line_c(&[(10.5, 3.5), (10.5, 16.5)], a);
        }
        "folderPane" => {
            pen.rect(2.5, 3.5, 17.5, 16.5, c);
            pen.line_c(&[(7.0, 3.5), (7.0, 16.5)], a);
        }
        // Messages one behind another.
        "conversations" => {
            pen.rounded(&[(5.5, 13.0), (2.5, 13.0), (2.5, 3.5), (13.5, 3.5), (13.5, 6.0)], false, 1.0, c);
            pen.envelope(6.5, 8.0, 17.5, 16.5, c);
        }
        // The ribbon: a band under its tabs (the one open in the accent), with a row of commands.
        "ribbon" => {
            pen.rect(2.5, 7.5, 17.5, 15.5, c);
            pen.line_c(&[(3.5, 4.5), (7.5, 4.5)], a);
            pen.line_c(&[(10.0, 4.5), (13.0, 4.5)], quiet);
            for x in [6.5, 10.0, 13.5] {
                pen.frect(x - 1.0, 10.5, x + 1.0, 12.5, c);
            }
        }
        "sort" | "reverseSort" => {
            pen.line(&[(6.0, 3.5), (6.0, 16.5)]);
            pen.head(6.0, 3.5, 270.0, 2.75, c);
            pen.line(&[(14.0, 3.5), (14.0, 16.5)]);
            pen.head(14.0, 16.5, 90.0, 2.75, c);
        }
        "previewLines" | "list" => {
            for y in [5.0, 10.0, 15.0] {
                pen.fcircle(4.0, y, 1.0, c);
                pen.line(&[(7.0, y), (17.5, y)]);
            }
        }
        "reset" => {
            // Back the other way round, the head at the top.
            let ring = pen.path().a(&pen, 10.0, 10.5, 6.5, 240.0, -60.0);
            ring.draw(&pen, c);
            if let Some(tip) = ring.last() {
                pen.head_px(tip, 210.0, 3.0, c);
            }
        }
        // Quick access and general: a U-turn with an open head.
        "undo" => {
            let turn = pen.path().m(&pen, 7.0, 15.5).a(&pen, 11.5, 11.5, 4.0, 90.0, -90.0).l(&pen, 3.0, 7.5);
            turn.draw(&pen, c);
            if let Some(tip) = turn.last() {
                pen.head_px(tip, 180.0, 3.5, c);
            }
        }
        "redo" => {
            let turn = pen.path().m(&pen, 13.0, 15.5).a(&pen, 8.5, 11.5, 4.0, 90.0, 270.0).l(&pen, 17.0, 7.5);
            turn.draw(&pen, c);
            if let Some(tip) = turn.last() {
                pen.head_px(tip, 0.0, 3.5, c);
            }
        }
        "more" => {
            for x in [5.0, 10.0, 15.0] {
                pen.fcircle(x, 10.0, 1.25, c);
            }
        }
        "dropdown" => pen.fill(&[(4.5, 7.5), (15.5, 7.5), (10.0, 13.5)], c),
        "expanded" => pen.line(&[(5.0, 7.5), (10.0, 12.5), (15.0, 7.5)]),
        "collapsed" => pen.line(&[(7.5, 5.0), (12.5, 10.0), (7.5, 15.0)]),
        "launcher" => {
            pen.rounded(&[(16.0, 4.0), (16.0, 16.0), (4.0, 16.0)], false, 1.0, c);
            pen.line(&[(5.5, 5.5), (12.5, 12.5)]);
            pen.line(&[(12.5, 8.0), (12.5, 12.5), (8.0, 12.5)]);
        }
        "plus" | "add" => pen.plus(10.0, 10.0, 6.0, c),
        "minus" => pen.line(&[(4.0, 10.0), (16.0, 10.0)]),
        // Settings: three sliders, each set somewhere else.
        "options" | "settings" => {
            for (y, x) in [(4.5, 12.5), (10.0, 6.5), (15.5, 10.5)] {
                if small {
                    pen.line(&[(2.5, y), (17.5, y)]);
                    pen.fcircle(x, y, 2.0, c);
                } else {
                    pen.line(&[(2.5, y), (x - 2.0, y)]);
                    pen.line(&[(x + 2.0, y), (17.5, y)]);
                    pen.circle(x, y, 2.0, c);
                }
            }
        }
        "info" | "about" => {
            pen.circle(10.0, 10.0, 7.5, c);
            pen.fcircle(10.0, 6.25, 1.0, a);
            pen.line_c(&[(10.0, 9.0), (10.0, 14.5)], a);
        }
        "help" => {
            pen.circle(10.0, 10.0, 7.5, c);
            pen.question(10.0, 9.75, 0.6, a);
        }
        // Report a Problem: a speech bubble with an exclamation mark in it.
        "report" | "reportProblem" => {
            pen.rounded(&[(2.5, 3.5), (17.5, 3.5), (17.5, 13.5), (9.0, 13.5), (5.5, 17.0), (5.5, 13.5), (2.5, 13.5)], true, 1.0, c);
            pen.line_c(&[(10.0, 6.0), (10.0, 9.5)], a);
            pen.fcircle(10.0, 11.5, 0.9, a);
        }
        "print" => {
            pen.rounded(&[(6.0, 6.5), (6.0, 2.5), (14.0, 2.5), (14.0, 6.5)], false, 1.0, c);
            pen.rounded(&[(5.5, 13.5), (2.5, 13.5), (2.5, 6.5), (17.5, 6.5), (17.5, 13.5), (14.5, 13.5)], false, 1.0, c);
            pen.rect(5.5, 10.5, 14.5, 17.5, c);
        }
        // Light and dark: a disc half filled.
        "theme" => {
            pen.circle(10.0, 10.0, 7.0, c);
            let (centre, rr) = pen.arc_geom(10.0, 10.0, 7.0);
            let half: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = (270.0 + i as f32 * 180.0 / 16.0_f32).to_radians();
                    pen.out(centre + vec2(rr * t.cos(), rr * t.sin()))
                })
                .collect();
            p.add(Shape::convex_polygon(half, c, Stroke::NONE));
        }
        "phone" => {
            pen.rect(5.5, 2.5, 14.5, 17.5, c);
            pen.line(&[(8.5, 14.5), (11.5, 14.5)]);
        }
        // The one globe: its outline, the equator and a meridian.
        "web" | "globe" => {
            pen.circle(10.0, 10.0, 7.5, c);
            pen.line(&[(2.5, 10.0), (17.5, 10.0)]);
            pen.ellipse_arc(10.0, 10.0, 3.0, 7.5, 0.0, 360.0, c);
        }
        // The one link: two links of a chain and the bar that joins them.
        "link" | "insertLink" => {
            pen.path().m(&pen, 8.5, 7.0).l(&pen, 6.0, 7.0).a(&pen, 6.0, 10.0, 3.0, 270.0, 90.0).l(&pen, 8.5, 13.0).draw(&pen, c);
            pen.path().m(&pen, 11.5, 7.0).l(&pen, 14.0, 7.0).a(&pen, 14.0, 10.0, 3.0, -90.0, 90.0).l(&pen, 11.5, 13.0).draw(&pen, c);
            pen.line(&[(7.0, 10.0), (13.0, 10.0)]);
        }
        // A round pin with its point.
        "address" | "location" => {
            pen.path().m(&pen, 10.0, 17.5).a(&pen, 10.0, 8.0, 5.5, 144.6, 395.4).draw_closed(&pen, c);
            if small {
                pen.fcircle(10.0, 8.0, 1.75, c);
            } else {
                pen.circle(10.0, 8.0, 2.0, c);
            }
        }
        "clock" | "time" => {
            pen.circle(10.0, 10.0, 7.5, c);
            pen.line(&[(10.0, 5.5), (10.0, 10.0), (13.0, 12.0)]);
        }
        // Insert › Date & Time: a calendar page with a small clock over its corner.
        "dateTime" => {
            pen.calendar(true, c);
            pen.circle(14.5, 14.5, 3.75, a);
            pen.line_c(&[(14.5, 12.5), (14.5, 14.5), (16.0, 15.5)], a);
        }
        // A control still to be built: an hourglass.
        "pending" => {
            pen.line(&[(5.0, 3.0), (15.0, 3.0)]);
            pen.line(&[(5.0, 17.0), (15.0, 17.0)]);
            pen.rounded(&[(6.0, 3.0), (6.0, 5.5), (10.0, 10.0), (6.0, 14.5), (6.0, 17.0)], false, 1.0, c);
            pen.rounded(&[(14.0, 3.0), (14.0, 5.5), (10.0, 10.0), (14.0, 14.5), (14.0, 17.0)], false, 1.0, c);
            pen.line_c(&[(8.0, 15.5), (12.0, 15.5)], a);
        }
        // Review › Word Count: lines of text and a tally of four strokes and one across them.
        "wordCount" => {
            pen.line(&[(2.5, 4.5), (17.5, 4.5)]);
            pen.line(&[(2.5, 8.5), (12.5, 8.5)]);
            pen.line(&[(2.5, 12.5), (8.5, 12.5)]);
            for x in [11.0, 13.0, 15.0, 17.0] {
                pen.line_c(&[(x, 11.5), (x, 17.5)], a);
            }
            pen.line_c(&[(10.0, 16.5), (18.0, 12.5)], a);
        }
        // A meeting held online: a camera, its lens to the right.
        "onlineMeeting" | "joinMeeting" => {
            pen.rect(2.5, 5.5, 12.5, 14.5, c);
            pen.rounded(&[(14.0, 8.75), (17.5, 6.5), (17.5, 13.5), (14.0, 11.25)], true, 0.5, c);
        }
        // Another time for a meeting: a clock, and an arrow on to a later hour.
        "proposeTime" => {
            pen.circle(7.5, 7.5, 5.0, c);
            pen.line(&[(7.5, 4.5), (7.5, 7.5), (9.5, 8.75)]);
            pen.line_c(&[(10.5, 15.0), (17.5, 15.0)], a);
            pen.head(17.5, 15.0, 0.0, 2.75, a);
        }
        // What the people invited are asked for: one line ticked, one with a question.
        "responseOptions" => {
            pen.check(5.0, 6.5, 0.7, green);
            pen.line(&[(10.0, 6.0), (17.5, 6.0)]);
            pen.question(5.0, 13.5, 0.5, c);
            pen.line(&[(10.0, 14.0), (17.5, 14.0)]);
        }
        // The globe, and the hour a place on it has.
        "timeZones" => {
            pen.arc(8.5, 8.5, 6.0, 66.0, 384.0, c);
            pen.line(&[(2.5, 8.5), (14.5, 8.5)]);
            pen.ellipse_arc(8.5, 8.5, 2.5, 6.0, 0.0, 360.0, c);
            pen.circle(15.0, 15.0, 2.75, c);
            if !small {
                pen.line(&[(15.0, 13.75), (15.0, 15.0), (16.25, 15.75)]);
            }
        }
        "notes" | "description" => {
            pen.rect(4.0, 2.5, 16.0, 17.5, c);
            for y in [7.0, 10.0, 13.0] {
                pen.line(&[(6.5, y), (13.5, y)]);
            }
        }
        // Account setup
        "work" => {
            pen.rect(2.5, 6.5, 17.5, 16.5, c);
            pen.rounded(&[(7.5, 6.5), (7.5, 3.5), (12.5, 3.5), (12.5, 6.5)], false, 1.0, c);
            if small {
                pen.line(&[(2.5, 11.0), (17.5, 11.0)]);
            } else {
                pen.line(&[(2.5, 11.0), (8.5, 11.0)]);
                pen.line(&[(11.5, 11.0), (17.5, 11.0)]);
                pen.rect(8.5, 10.0, 11.5, 12.5, c);
            }
        }
        "server" => {
            for y in [3.5, 11.5] {
                pen.rect(3.5, y, 16.5, y + 5.0, c);
                pen.fcircle(6.5, y + 2.5, 1.0, c);
                pen.line(&[(10.0, y + 2.5), (14.0, y + 2.5)]);
            }
        }
        "device" => {
            pen.rect(2.5, 3.5, 17.5, 13.5, c);
            pen.line(&[(10.0, 13.5), (10.0, 16.5)]);
            pen.line(&[(6.5, 16.5), (13.5, 16.5)]);
        }
        // Formatting the text of a message being written: the only letters, all in one face.
        "bold" => pen.text(10.0, 10.0, 15.0, "B", c, 0.0),
        "italic" => pen.text(10.0, 10.0, 15.0, "I", c, 0.22),
        "underline" => {
            pen.text(10.0, 8.5, 13.0, "U", c, 0.0);
            pen.line(&[(4.5, 16.5), (15.5, 16.5)]);
        }
        "fontSize" => {
            pen.text(7.5, 10.0, 15.0, "A", c, 0.0);
            pen.text(15.5, 12.0, 10.0, "A", c, 0.0);
        }
        "font" => {
            pen.text(7.0, 10.0, 15.0, "A", c, 0.0);
            pen.text(14.75, 12.0, 11.0, "a", c, 0.0);
        }
        "fontColor" => {
            pen.text(10.0, 8.0, 13.0, "A", c, 0.0);
            pen.frect_r(3.5, 14.5, 16.5, 17.5, 1.0, a);
        }
        "clearFormatting" => {
            pen.text(7.0, 7.5, 11.0, "A", c, 0.0);
            pen.badge_x(c);
        }
        "bullets" => {
            for y in [5.0, 10.0, 15.0] {
                pen.fcircle(4.5, y, 1.25, c);
                pen.line(&[(8.0, y), (17.5, y)]);
            }
        }
        // Numbers of strokes, not of a font.
        "numbering" => {
            let x = 4.5;
            pen.line(&[(x - 1.25, 4.0), (x + 0.25, 3.0), (x + 0.25, 9.0)]);
            pen.path().a(&pen, x, 12.5, 1.5, 180.0, 380.0).l(&pen, x - 1.5, 17.0).l(&pen, x + 1.75, 17.0).draw(&pen, c);
            pen.line(&[(9.0, 6.0), (17.5, 6.0)]);
            pen.line(&[(9.0, 14.0), (17.5, 14.0)]);
        }
        // Code: its brackets and the slash between.
        "formatHtml" => {
            pen.head(2.5, 10.0, 180.0, 5.0, c);
            pen.head(17.5, 10.0, 0.0, 5.0, c);
            pen.line(&[(11.5, 4.5), (8.5, 15.5)]);
        }
        "increaseIndent" | "decreaseIndent" => {
            pen.line(&[(2.5, 3.5), (17.5, 3.5)]);
            pen.line(&[(9.5, 7.5), (17.5, 7.5)]);
            pen.line(&[(9.5, 12.5), (17.5, 12.5)]);
            pen.line(&[(2.5, 16.5), (17.5, 16.5)]);
            if name == "increaseIndent" {
                pen.line_c(&[(2.5, 10.0), (6.5, 10.0)], a);
                pen.head(6.5, 10.0, 0.0, 2.5, a);
            } else {
                pen.line_c(&[(6.5, 10.0), (2.5, 10.0)], a);
                pen.head(2.5, 10.0, 180.0, 2.5, a);
            }
        }
        "formatPlain" => {
            pen.line(&[(3.5, 5.5), (16.5, 5.5)]);
            pen.line(&[(3.5, 10.0), (16.5, 10.0)]);
            pen.line(&[(3.5, 14.5), (11.5, 14.5)]);
        }
        // A password's eye (#438): open, a click shows the password; struck through, it hides
        // it again.
        "showPassword" | "hidePassword" => {
            let lids = pen.path().m(&pen, 2.5, 10.0).c(&pen, (5.5, 4.5), (14.5, 4.5), 17.5, 10.0).c(&pen, (14.5, 15.5), (5.5, 15.5), 2.5, 10.0);
            lids.draw_closed(&pen, c);
            pen.circle(10.0, 10.0, 2.5, c);
            if name == "hidePassword" {
                pen.line(&[(3.5, 16.5), (16.5, 3.5)]);
            }
        }
        // Anything else: a lettered tile, so a missing drawing is visible rather than empty.
        _ => {
            pen.rect(3.5, 3.5, 16.5, 16.5, c);
            let letter: String = name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
            pen.text(10.0, 10.0, 9.5, &letter, c, 0.0);
        }
    }
}

/// The names [`paint`] draws (not a lettered tile); tests check ribbon icons against it.
pub const NAMES: &[&str] = &[
    "mail",
    "email",
    "unread",
    "markUnread",
    "new",
    "newMail",
    "read",
    "markRead",
    "toggleRead",
    "markAllRead",
    "reply",
    "replyAll",
    "forward",
    "forwardAsAttachment",
    "delete",
    "deleted",
    "discard",
    "deletePermanently",
    "remove",
    "empty",
    "emptyFolder",
    "archive",
    "junk",
    "notJunk",
    "move",
    "copy",
    "flag",
    "toggleFlag",
    "followUp",
    "clearFlag",
    "complete",
    "check",
    "accept",
    "categorize",
    "category",
    "categories",
    "pin",
    "rules",
    "createRule",
    "attach",
    "attachment",
    "send",
    "sent",
    "drafts",
    "editDraft",
    "edit",
    "rename",
    "inbox",
    "outbox",
    "export",
    "import",
    "open",
    "folder",
    "custom",
    "select",
    "folderNew",
    "newFolder",
    "favorite",
    "star",
    "starOn",
    "save",
    "saveDraft",
    "highImportance",
    "importanceHigh",
    "lowImportance",
    "importanceLow",
    "signature",
    "checkNames",
    "close",
    "decline",
    "cancel",
    "tentative",
    "calendar",
    "appointment",
    "newAppointment",
    "newEvent",
    "today",
    "day",
    "workWeek",
    "week",
    "month",
    "people",
    "contact",
    "account",
    "newContact",
    "meeting",
    "newMeeting",
    "mergeContacts",
    "fromMessage",
    "meetingRequest",
    "meetingCancelled",
    "meetingAccepted",
    "meetingTentative",
    "meetingDeclined",
    "tasks",
    "task",
    "newTask",
    "next",
    "forwardPeriod",
    "previous",
    "back",
    "reminder",
    "bell",
    "private",
    "lock",
    "recurrence",
    "repeat",
    "scheduling",
    "sync",
    "syncProgress",
    "request",
    "sendReceive",
    "showAs",
    "search",
    "find",
    "findRelated",
    "findFromSender",
    "filter",
    "readingPane",
    "folderPane",
    "conversations",
    "ribbon",
    "sort",
    "reverseSort",
    "previewLines",
    "list",
    "reset",
    "undo",
    "redo",
    "more",
    "dropdown",
    "expanded",
    "collapsed",
    "launcher",
    "plus",
    "add",
    "minus",
    "options",
    "settings",
    "info",
    "about",
    "help",
    "report",
    "reportProblem",
    "print",
    "theme",
    "phone",
    "web",
    "link",
    "address",
    "location",
    "clock",
    "time",
    "dateTime",
    "wordCount",
    "pending",
    "onlineMeeting",
    "joinMeeting",
    "proposeTime",
    "responseOptions",
    "timeZones",
    "globe",
    "notes",
    "description",
    "work",
    "server",
    "device",
    "tick",
    "bold",
    "italic",
    "underline",
    "fontSize",
    "font",
    "fontColor",
    "bullets",
    "numbering",
    "insertLink",
    "clearFormatting",
    "formatHtml",
    "formatPlain",
    "increaseIndent",
    "decreaseIndent",
    "showPassword",
    "hidePassword",
];
