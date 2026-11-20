// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn automatic_reply_info_tile_mirrors_only_its_visual_reading_edge() {
    let tile = egui::Rect::from_min_size(egui::pos2(40.0, 20.0), vec2(560.0, 52.0));
    let (ltr_icon, ltr_caption) = auto_reply_tile_geometry(tile, false);
    let (rtl_icon, rtl_caption) = auto_reply_tile_geometry(tile, true);

    assert_eq!(ltr_icon.center().x, tile.min.x + 26.0);
    assert_eq!(rtl_icon.center().x, tile.max.x - 26.0);
    assert_eq!(ltr_caption.min.x, tile.min.x + 52.0);
    assert_eq!(rtl_caption.max.x, tile.max.x - 52.0);
    assert!((ltr_caption.width() - rtl_caption.width()).abs() < f32::EPSILON);
    assert!(ltr_icon.max.x < ltr_caption.min.x, "LTR reply icon must precede text");
    assert!(rtl_caption.max.x < rtl_icon.min.x, "Hebrew reply icon must follow text");
    assert!(ltr_icon.min.x >= tile.min.x && rtl_icon.max.x <= tile.max.x);
}
