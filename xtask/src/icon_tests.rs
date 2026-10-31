// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

const SVG: &[u8] = include_bytes!("../../assets/app-icon/source/mailcraft.svg");

/// The alpha of the pixel at (x, y) of a straight-alpha RGBA8 render.
fn alpha(px: &[u8], size: u32, x: u32, y: u32) -> u8 {
    px[((y * size + x) * 4 + 3) as usize]
}

#[test]
fn the_tile_fills_the_canvas_and_its_corners_are_cut() {
    let d = Drawing::parse(SVG).unwrap();
    let size = 256;
    let px = d.render(size, 1.0, 1.0).unwrap();
    assert_eq!(px.len(), (size * size * 4) as usize);
    // The rounded corners are see-through; the middle of each edge and the centre are not.
    for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
        assert_eq!(alpha(&px, size, x, y), 0, "corner {x},{y}");
    }
    for (x, y) in [(size / 2, 1), (size / 2, size - 2), (1, size / 2), (size - 2, size / 2), (size / 2, size / 2)] {
        assert_eq!(alpha(&px, size, x, y), 255, "{x},{y}");
    }
}

#[test]
fn macos_keeps_its_margin() {
    let d = Drawing::parse(SVG).unwrap();
    let size = 256;
    let px = d.render(size, MAC_SCALE, 1.0).unwrap();
    // About 10% each side is left clear.
    assert_eq!(alpha(&px, size, size / 2, 4), 0);
    assert_eq!(alpha(&px, size, 4, size / 2), 0);
    assert_eq!(alpha(&px, size, size / 2, size / 2), 255);
}

#[test]
fn a_closer_drawing_stays_inside_the_tile() {
    let d = Drawing::parse(SVG).unwrap();
    let size = 64;
    let plain = d.render(size, 1.0, 1.0).unwrap();
    let near = d.render(size, 1.0, 1.3).unwrap();
    // The outline is the same; what is inside it differs.
    for y in 0..size {
        for x in 0..size {
            assert_eq!(alpha(&plain, size, x, y), alpha(&near, size, x, y), "{x},{y}");
        }
    }
    assert_ne!(plain, near);
    assert!(zoom(16) > zoom(48) && zoom(64) == 1.0);
}

#[test]
fn the_scalable_icon_is_fitted_to_the_tile() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect x="10" y="20" width="60" height="50" fill="red"/></svg>"#;
    let d = Drawing::parse(svg.as_bytes()).unwrap();
    let fitted = d.fitted(svg).unwrap();
    assert!(fitted.contains(r#"viewBox="10 15 60 60""#), "{fitted}");
    // And it still parses, the tile now filling the whole canvas.
    let again = Drawing::parse(fitted.as_bytes()).unwrap();
    assert_eq!((again.cx, again.cy, again.side), (30.0, 30.0, 60.0));
    assert_eq!((again.tree.size().width(), again.tree.size().height()), (60.0, 60.0));
    // Drawn at another size than its viewBox, the same square in its own units.
    let sized = svg.replace("viewBox", r#"width="200" height="200" viewBox"#);
    assert!(Drawing::parse(sized.as_bytes()).unwrap().fitted(&sized).unwrap().contains(r#"viewBox="10 15 60 60""#));
    assert!(d.fitted("<svg/>").is_err());
    assert!(d.fitted(r#"<svg viewBox="0 0"#).is_err());
}

#[test]
fn an_svg_that_draws_nothing_is_refused() {
    assert!(Drawing::parse(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"/>"#).is_err());
    assert!(Drawing::parse(b"not svg").is_err());
}

#[test]
fn renders_and_packs() {
    let d = Drawing::parse(SVG).unwrap();
    let data = d.png(32, 1.0).unwrap();
    assert_eq!(crate::ico::png_size(&data).unwrap(), (32, 32));
    let icns = icns(&[(b"icp5", data.clone())]);
    assert_eq!(&icns[..4], b"icns");
    assert_eq!(u32::from_be_bytes([icns[4], icns[5], icns[6], icns[7]]) as usize, icns.len());
    assert_eq!(icns.len(), 16 + data.len());
}
