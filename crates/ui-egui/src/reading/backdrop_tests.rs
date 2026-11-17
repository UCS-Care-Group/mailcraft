// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #601: where the copies of a picture behind a box go, how large they are, how many there may be.

use std::sync::Arc;

use egui::{Color32, Rect, pos2, vec2};
use mailcraft_html::{BgOffset, BgSize, Length};

use super::backdrop::{DARK_TINT, MAX_TILES, offset, tile_size, tiles, tint};
use super::layout::BackdropItem;
use crate::theme::{Accent, Tokens};

fn item(tile: egui::Vec2, offset: egui::Vec2, repeat: [bool; 2]) -> BackdropItem {
    BackdropItem { data: Arc::new(Vec::new()), tile, offset, repeat, tint: Color32::WHITE, veil: None }
}

#[test]
fn a_picture_is_sized_as_the_box_asks() {
    let (natural, room) = (vec2(300.0, 100.0), vec2(600.0, 400.0));
    assert_eq!(tile_size(BgSize::Auto, natural, room), natural);
    // Cover: the box is filled, the picture's shape kept; contain: the picture is all there.
    assert_eq!(tile_size(BgSize::Cover, natural, room), vec2(1200.0, 400.0));
    assert_eq!(tile_size(BgSize::Contain, natural, room), vec2(600.0, 200.0));
    let given = |width, height| tile_size(BgSize::Given { width, height }, natural, room);
    assert_eq!(given(Some(Length::Percent(50)), None), vec2(300.0, 100.0));
    assert_eq!(given(None, Some(Length::Px(50))), vec2(150.0, 50.0));
    assert_eq!(given(Some(Length::Px(20)), Some(Length::Percent(100))), vec2(20.0, 400.0));
    // Nothing is ever of no size, whatever the box or the picture.
    for size in [BgSize::Auto, BgSize::Cover, BgSize::Contain] {
        for (n, r) in [(vec2(0.0, 0.0), room), (natural, vec2(0.0, 0.0)), (vec2(f32::NAN, 1.0), room)] {
            let s = tile_size(size, n, r);
            assert!(s.x >= 1.0 && s.y >= 1.0 && s.x.is_finite() && s.y.is_finite(), "{size:?} {n:?} {r:?}: {s:?}");
        }
    }
}

#[test]
fn a_picture_starts_where_the_box_puts_it() {
    let (tile, room) = (vec2(100.0, 50.0), vec2(600.0, 250.0));
    assert_eq!(offset([BgOffset::Percent(0); 2], tile, room), vec2(0.0, 0.0));
    assert_eq!(offset([BgOffset::Percent(50); 2], tile, room), vec2(250.0, 100.0));
    assert_eq!(offset([BgOffset::Percent(100), BgOffset::Px(10)], tile, room), vec2(500.0, 10.0));
}

#[test]
fn copies_fill_the_box_on_the_sides_it_repeats_and_only_what_is_in_view() {
    let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(100.0, 60.0));
    // Once.
    assert_eq!(
        tiles(&item(vec2(40.0, 30.0), vec2(30.0, 15.0), [false, false]), rect, rect, MAX_TILES),
        [Rect::from_min_size(pos2(40.0, 35.0), vec2(40.0, 30.0))]
    );
    // Across and down: from before the box's edge where it starts inside it.
    let all = tiles(&item(vec2(40.0, 30.0), vec2(30.0, 15.0), [true, true]), rect, rect, MAX_TILES);
    assert_eq!(all.len(), 3 * 3);
    assert!(all.iter().all(|t| t.intersects(rect)));
    let covered = all.iter().fold(Rect::NOTHING, |a, t| a.union(*t));
    assert!(covered.contains_rect(rect), "{covered:?}");
    // Across only: one row.
    let row = tiles(&item(vec2(40.0, 30.0), vec2(0.0, 0.0), [true, false]), rect, rect, MAX_TILES);
    assert!(row.len() == 3 && row.iter().all(|t| t.min.y == 20.0));
    // Only what is in view.
    let view = Rect::from_min_size(pos2(10.0, 20.0), vec2(20.0, 20.0));
    assert_eq!(tiles(&item(vec2(10.0, 10.0), vec2(0.0, 0.0), [true, true]), rect, view, MAX_TILES).len(), 4);
    // Out of view, or out of the box: nothing.
    assert!(
        tiles(&item(vec2(10.0, 10.0), vec2(0.0, 0.0), [true, true]), rect, Rect::from_min_size(pos2(500.0, 0.0), vec2(9.0, 9.0)), MAX_TILES)
            .is_empty()
    );
    assert!(tiles(&item(vec2(10.0, 10.0), vec2(200.0, 0.0), [false, false]), rect, rect, MAX_TILES).is_empty());
}

#[test]
fn a_tiny_picture_over_a_large_box_is_drawn_in_few_copies() {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 20_000.0));
    // (And fewer still where the frame has little left: the boxes in view share what it may paint.)
    for max in [MAX_TILES, 64, 1] {
        for tile in [vec2(1.0, 1.0), vec2(1.0, 600.0), vec2(4.0, 4.0)] {
            let copies = tiles(&item(tile, vec2(0.0, 0.0), [true, true]), rect, rect, max);
            assert!(!copies.is_empty() && copies.len() <= max, "{tile:?}: {}", copies.len());
            let covered = copies.iter().fold(Rect::NOTHING, |a, t| a.union(*t));
            assert!(covered.contains_rect(rect), "{tile:?}: {covered:?}");
        }
    }
}

#[test]
fn the_dark_theme_dims_the_picture_and_high_contrast_shows_none() {
    assert_eq!(tint(&Tokens::light()), Some(Color32::WHITE));
    assert_eq!(tint(&Tokens::dark()), Some(DARK_TINT));
    // A message with its own design for dark windows has its pictures as it made them.
    assert_eq!(tint(&Tokens { sender_dark: true, ..Tokens::dark() }), Some(Color32::WHITE));
    for dark in [false, true] {
        assert_eq!(tint(&Tokens::build(dark, Accent::Blue, true)), None);
    }
}
