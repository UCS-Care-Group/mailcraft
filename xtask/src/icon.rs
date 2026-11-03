// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask icon`: write MailCraft's app icon in every size the platforms want.
//!
//! The source is one vector drawing, `assets/app-icon/source/mailcraft.svg`: an envelope that is
//! also a calendar page, with its two ring tabs, in front of two tilted sheets, on a rounded tile
//! in blue. It was drawn by hand for MailCraft (design: Martin Ball); nothing is traced or copied
//! from any other product's icon. It is rendered with `resvg`, a pure-Rust SVG renderer, here in
//! xtask only: the program itself never reads the SVG, only the files written from it.
//!
//! The tile is fitted to the canvas, whatever margin the drawing has round it (the bounding box of
//! what it draws, made square about its centre). The macOS renders sit on Apple's 824/1024 grid,
//! with the margin macOS icons keep. The 16–48 px renders show the drawing a little closer inside
//! the same tile, so the page and its envelope still read.
//!
//! Output (all listed in ATTRIBUTION.md): `assets/app-icon/mailcraft-1024.png`,
//! `mailcraft-macos-512.png` (with the margin macOS icons keep), `mailcraft.ico`, `mailcraft.icns`
//! and the `hicolor` PNG tree for Linux, with the SVG itself as the scalable one.

use std::path::Path;

/// The drawing, under `assets/app-icon`.
const SOURCE: &str = "source/mailcraft.svg";
/// The reverse-DNS name the Linux icon files carry.
const APP_ID: &str = "io.github.billsecond.mailcraft";
/// macOS icons sit on Apple's grid: an 824 px body on a 1024 px canvas.
const MAC_SCALE: f32 = 824.0 / 1024.0;

/// How much closer a size shows the drawing inside its tile: the small ones, so it still reads.
fn zoom(size: u32) -> f32 {
    match size {
        0..=16 => 1.14,
        17..=32 => 1.08,
        33..=48 => 1.04,
        _ => 1.0,
    }
}

/// The parsed drawing, and the square on it that becomes the canvas.
struct Drawing {
    tree: resvg::usvg::Tree,
    /// Centre and side of that square, in the drawing's units.
    cx: f32,
    cy: f32,
    side: f32,
}

impl Drawing {
    fn parse(svg: &[u8]) -> Result<Drawing, String> {
        let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).map_err(|e| format!("reading the icon's SVG: {e}"))?;
        // What it draws, without its shadows' and filters' reach: the tile.
        let bbox = tree.root().abs_bounding_box();
        let side = bbox.width().max(bbox.height());
        if !side.is_finite() || side <= 0.0 {
            return Err("the icon's SVG draws nothing".to_string());
        }
        Ok(Drawing { tree, cx: bbox.x() + bbox.width() / 2.0, cy: bbox.y() + bbox.height() / 2.0, side })
    }

    /// The drawing as a `size`×`size` pixmap: the tile inside `scale` of the canvas (1.0 fills
    /// it), the drawing `zoom` times closer about the tile's centre.
    fn pixmap(&self, size: u32, scale: f32, zoom: f32) -> Result<resvg::tiny_skia::Pixmap, String> {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).ok_or("icon size out of range")?;
        let k = size as f32 * scale * zoom / self.side;
        let half = size as f32 / 2.0;
        let transform = resvg::tiny_skia::Transform::from_row(k, 0.0, 0.0, k, half - self.cx * k, half - self.cy * k);
        resvg::render(&self.tree, transform, &mut pixmap.as_mut());
        Ok(pixmap)
    }

    /// Straight-alpha RGBA8 pixels of the icon at `size`, inside `scale` of the canvas. A zoomed
    /// render is cut back to the tile's own outline, so only what is inside the tile comes closer.
    fn render(&self, size: u32, scale: f32, zoom: f32) -> Result<Vec<u8>, String> {
        let plain = self.pixmap(size, scale, 1.0)?;
        let near = if zoom > 1.0 { Some(self.pixmap(size, scale, zoom)?) } else { None };
        let mut out = Vec::with_capacity((size * size * 4) as usize);
        for (i, px) in plain.pixels().iter().enumerate() {
            let a = px.alpha();
            let c = match near.as_ref().and_then(|n| n.pixels().get(i)) {
                // Premultiplied, so scaling every channel by the outline's alpha masks it.
                Some(z) => {
                    let m = |v: u8| ((u16::from(v) * u16::from(a) + 127) / 255) as u8;
                    resvg::tiny_skia::PremultipliedColorU8::from_rgba(m(z.red()), m(z.green()), m(z.blue()), m(z.alpha())).unwrap_or(*px)
                }
                None => *px,
            };
            let c = c.demultiply();
            out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        Ok(out)
    }

    fn png(&self, size: u32, scale: f32) -> Result<Vec<u8>, String> {
        let zoom = if scale >= 1.0 { zoom(size) } else { 1.0 };
        encode(size, self.render(size, scale, zoom)?)
    }

    /// The SVG's text with its `viewBox` on the tile's square, so the scalable icon fills its
    /// canvas as the PNGs do.
    fn fitted(&self, svg: &str) -> Result<String, String> {
        const ATTR: &str = "viewBox=\"";
        let start = svg.find(ATTR).ok_or("the icon's SVG has no viewBox")? + ATTR.len();
        let len = svg.get(start..).and_then(|rest| rest.find('"')).ok_or("the icon's SVG has an unclosed viewBox")?;
        let (head, tail) = (svg.get(..start).unwrap_or_default(), svg.get(start + len..).unwrap_or_default());
        // The square is in the canvas's pixels; the viewBox is in the drawing's own units.
        let old: Vec<f32> =
            svg.get(start..start + len).unwrap_or_default().split([' ', ',']).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
        let [vx, vy, vw, vh] = old[..] else { return Err("the icon's SVG has a viewBox that isn't four numbers".to_string()) };
        let size = self.tree.size();
        let (sx, sy) = (size.width() / vw, size.height() / vh);
        if !(sx.is_finite() && sx > 0.0 && (sx - sy).abs() <= sx * 1e-4) {
            return Err("the icon's SVG is not drawn at its viewBox's proportions".to_string());
        }
        let r = |v: f32| (v * 10.0).round() / 10.0;
        let (x, y, side) = (vx + (self.cx - self.side / 2.0) / sx, vy + (self.cy - self.side / 2.0) / sx, self.side / sx);
        Ok(format!("{head}{} {} {} {}{tail}", r(x), r(y), r(side), r(side)))
    }
}

fn encode(size: u32, pixels: Vec<u8>) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(size, size, pixels).ok_or("icon buffer has the wrong size")?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut bytes, image::ImageFormat::Png).map_err(|e| format!("encoding the icon: {e}"))?;
    Ok(bytes.into_inner())
}

/// An Apple `.icns` holding PNG entries.
fn icns(entries: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let total: usize = 8 + entries.iter().map(|(_, d)| 8 + d.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(total as u32).to_be_bytes());
    for (kind, data) in entries {
        out.extend_from_slice(*kind);
        out.extend_from_slice(&((8 + data.len()) as u32).to_be_bytes());
        out.extend_from_slice(data);
    }
    out
}

pub fn run(root: &Path) -> Result<(), String> {
    let dir = root.join("assets/app-icon");
    let write = |rel: &str, bytes: &[u8]| -> Result<(), String> {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("wrote {}", path.display());
        Ok(())
    };
    let path = dir.join(SOURCE);
    let svg = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let drawing = Drawing::parse(&svg)?;

    let mut by_size = std::collections::BTreeMap::new();
    for size in [16u32, 24, 32, 48, 64, 128, 256, 512, 1024] {
        by_size.insert(size, drawing.png(size, 1.0)?);
    }
    let get = |size: u32| by_size.get(&size).cloned().ok_or_else(|| format!("no {size} px icon"));
    for size in [16u32, 24, 32, 48, 64, 128, 256, 512] {
        write(&format!("hicolor/{size}x{size}/apps/{APP_ID}.png"), &get(size)?)?;
    }
    let text = std::str::from_utf8(&svg).map_err(|e| format!("{}: {e}", path.display()))?;
    write(&format!("hicolor/scalable/apps/{APP_ID}.svg"), drawing.fitted(text)?.as_bytes())?;
    write("mailcraft-1024.png", &get(1024)?)?;
    write("mailcraft-macos-512.png", &drawing.png(512, MAC_SCALE)?)?;
    write("mailcraft.ico", &crate::ico::pack(&[get(16)?, get(24)?, get(32)?, get(48)?, get(64)?, get(128)?, get(256)?])?)?;
    let mac = |size: u32| drawing.png(size, MAC_SCALE);
    let entries: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"icp4", mac(16)?),
        (b"icp5", mac(32)?),
        (b"icp6", mac(64)?),
        (b"ic07", mac(128)?),
        (b"ic08", mac(256)?),
        (b"ic09", mac(512)?),
        (b"ic10", mac(1024)?),
        (b"ic11", mac(32)?),
        (b"ic12", mac(64)?),
        (b"ic13", mac(256)?),
        (b"ic14", mac(512)?),
    ];
    write("mailcraft.icns", &icns(&entries))?;
    Ok(())
}

#[cfg(test)]
#[path = "icon_tests.rs"]
mod icon_tests;
