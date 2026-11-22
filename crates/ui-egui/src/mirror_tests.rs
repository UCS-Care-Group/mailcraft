// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use egui::epaint::{CircleShape, RectShape};
use egui::{Color32, Stroke};

fn mirror() -> Mirror {
    Mirror { axis: 100.0 }
}

#[test]
fn rect_moves_to_the_other_side() {
    let m = mirror();
    let mut s = Shape::Rect(RectShape::filled(
        Rect::from_min_max(Pos2::new(10.0, 5.0), Pos2::new(30.0, 15.0)),
        CornerRadius { nw: 4, ne: 0, sw: 8, se: 0 },
        Color32::RED,
    ));
    m.shape(&mut s);
    let Shape::Rect(r) = s else { panic!("not a rect") };
    assert_eq!(r.rect, Rect::from_min_max(Pos2::new(70.0, 5.0), Pos2::new(90.0, 15.0)));
    assert_eq!((r.corner_radius.ne, r.corner_radius.se, r.corner_radius.nw), (4, 8, 0));
}

#[test]
fn circle_centre_is_mirrored() {
    let m = mirror();
    let mut s = Shape::Circle(CircleShape::stroke(Pos2::new(20.0, 7.0), 3.0, Stroke::NONE));
    m.shape(&mut s);
    let Shape::Circle(c) = s else { panic!("not a circle") };
    assert_eq!(c.center, Pos2::new(80.0, 7.0));
}

#[test]
fn mirroring_twice_is_the_identity() {
    let m = mirror();
    let r = Rect::from_min_max(Pos2::new(3.0, 1.0), Pos2::new(41.0, 9.0));
    assert_eq!(m.rect(m.rect(r)), r);
}

#[test]
fn mesh_keeps_its_shape_but_moves() {
    let m = mirror();
    let mut mesh = Mesh::default();
    mesh.add_colored_rect(Rect::from_min_max(Pos2::new(10.0, 0.0), Pos2::new(20.0, 4.0)), Color32::WHITE);
    let mut s = Shape::mesh(mesh);
    m.shape(&mut s);
    let Shape::Mesh(out) = s else { panic!("not a mesh") };
    let b = out.calc_bounds();
    assert_eq!((b.min.x, b.max.x), (80.0, 90.0));
}
