// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A face with embedded black-and-white bitmaps beside its outlines (Calibri, Cambria, SimSun)
//! draws its text in the text's colour (#889): egui drew every glyph with a bitmap as a colour
//! glyph, white, so on a light page most of the letters were missing.

use std::sync::Arc;

use egui::{Color32, FontFamily, FontId};

use super::{hide_bitmap_strikes, is_face, read_face};
use crate::font_index::font_index_tests::{FontsDir, collection};

const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const TEXT: &str = "Hi there, this is a test of the reading pane.";

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// `font` (one face) with `extra` tables added, its directory sorted by tag as a font's is.
fn with_tables(font: &[u8], extra: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let tables = u16_at(font, 4) as usize;
    let grown = tables + extra.len();
    let shift = (extra.len() * 16) as u32;
    let mut records: Vec<[u8; 16]> = (0..tables)
        .map(|i| {
            let mut record: [u8; 16] = font[12 + i * 16..28 + i * 16].try_into().unwrap();
            let moved = u32_at(&record, 8) + shift;
            record[8..12].copy_from_slice(&moved.to_be_bytes());
            record
        })
        .collect();
    let mut body = font[12 + tables * 16..].to_vec();
    while !body.len().is_multiple_of(4) {
        body.push(0);
    }
    for (tag, data) in extra {
        let at = (12 + grown * 16 + body.len()) as u32;
        let mut record = [0u8; 16];
        record[..4].copy_from_slice(tag.as_slice());
        record[8..12].copy_from_slice(&at.to_be_bytes());
        record[12..].copy_from_slice(&(data.len() as u32).to_be_bytes());
        records.push(record);
        body.extend_from_slice(data);
        while !body.len().is_multiple_of(4) {
            body.push(0);
        }
    }
    records.sort_by_key(|r| [r[0], r[1], r[2], r[3]]);
    let mut out = font[..4].to_vec();
    out.extend_from_slice(&(grown as u16).to_be_bytes());
    out.extend_from_slice(&font[6..12]);
    for record in records {
        out.extend_from_slice(&record);
    }
    out.extend_from_slice(&body);
    out
}

/// Inter with a strike of 16-pixel black-and-white bitmaps for every glyph, as Calibri has for
/// its small sizes: `EBLC` with one strike of one index subtable (format 2, every glyph the same
/// size; image format 5, bit-aligned) and `EBDT` with an 8 by 8 bitmap for each glyph.
fn with_bitmaps(font: &[u8]) -> Vec<u8> {
    let maxp = (0..u16_at(font, 4) as usize).map(|i| 12 + i * 16).find(|r| &font[*r..*r + 4] == b"maxp").unwrap();
    let last = u16_at(font, u32_at(font, maxp + 8) as usize + 4) - 1;
    let mut eblc = Vec::new();
    eblc.extend_from_slice(&[0, 2, 0, 0, 0, 0, 0, 1]); // version 2.0, one strike
    eblc.extend_from_slice(&56u32.to_be_bytes()); // where its subtable array is
    eblc.extend_from_slice(&28u32.to_be_bytes()); // the array and its subtable
    eblc.extend_from_slice(&1u32.to_be_bytes()); // one subtable
    eblc.extend_from_slice(&0u32.to_be_bytes()); // colorRef
    let metrics = [12u8, 0xFC, 8, 1, 0, 0, 0, 0, 0, 0, 0, 0]; // ascender 12, descender -4, ...
    eblc.extend_from_slice(&metrics);
    eblc.extend_from_slice(&metrics);
    eblc.extend_from_slice(&0u16.to_be_bytes());
    eblc.extend_from_slice(&last.to_be_bytes());
    eblc.extend_from_slice(&[16, 16, 1, 1]); // 16 pixels an em, one bit a pixel, horizontal
    eblc.extend_from_slice(&0u16.to_be_bytes()); // the subtable array: glyphs 0 to the last
    eblc.extend_from_slice(&last.to_be_bytes());
    eblc.extend_from_slice(&8u32.to_be_bytes());
    eblc.extend_from_slice(&[0, 2, 0, 5]); // index format 2, image format 5
    eblc.extend_from_slice(&4u32.to_be_bytes()); // the images begin after EBDT's version
    eblc.extend_from_slice(&8u32.to_be_bytes()); // eight bytes each
    eblc.extend_from_slice(&[8, 8, 0, 8, 8, 0, 0, 8]); // 8 by 8, on the baseline, advance 8
    let mut ebdt = vec![0, 2, 0, 0];
    ebdt.extend(std::iter::repeat_n(0xFF, (usize::from(last) + 1) * 8));
    with_tables(font, &[(b"EBLC", eblc), (b"EBDT", ebdt)])
}

/// The characters of `text` that egui draws as colour glyphs in the face `index` of `bytes`, and
/// the number of characters drawn with no pixels at all (apart from spaces).
fn colour_and_blank(bytes: Vec<u8>, index: u32, text: &str) -> (Vec<char>, usize) {
    let mut fonts = egui::FontDefinitions::default();
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    fonts.font_data.insert("face".into(), Arc::new(data));
    fonts.families.insert(FontFamily::Name("face".into()), vec!["face".into()]);
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    let id = FontId::new(16.0, FontFamily::Name("face".into()));
    let galley = ctx.fonts_mut(|f| f.layout_no_wrap(text.into(), id, Color32::BLACK));
    let glyphs: Vec<&egui::epaint::text::Glyph> = galley.rows.iter().flat_map(|r| r.glyphs.iter()).collect();
    let colour = glyphs.iter().filter(|g| g.is_color).map(|g| g.chr).collect();
    let blank = glyphs.iter().filter(|g| g.chr != ' ' && g.uv_rect.is_nothing()).count();
    (colour, blank)
}

/// The synthetic face shows what went wrong: egui takes its letters for colour glyphs. A canary:
/// when it fails, egui draws such glyphs in the text's colour itself, and [`hide_bitmap_strikes`]
/// can go.
#[test]
fn a_face_with_bitmaps_draws_white_letters_as_it_comes() {
    let (colour, _) = colour_and_blank(with_bitmaps(INTER), 0, TEXT);
    assert!(colour.contains(&'H') && colour.contains(&'a'), "{colour:?}");
}

/// Read from a fonts folder, the same face draws every letter from its outlines, in the text's
/// colour: alone and as the second face of a collection.
#[test]
fn a_face_with_bitmaps_draws_its_outlines_in_the_texts_colour() {
    let dir = FontsDir::new("bitmaps");
    let single = dir.put("bitmaps.ttf", &with_bitmaps(INTER));
    let pair = dir.put("bitmaps.ttc", &collection(&with_bitmaps(INTER), 2));
    for (path, index) in [(single, 0), (pair, 1)] {
        let bytes = read_face(&path, u64::MAX, index).unwrap();
        assert!(skrifa::FontRef::from_index(&bytes, index).is_ok(), "{path:?}");
        assert_eq!(colour_and_blank(bytes, index, TEXT), (Vec::new(), 0), "{path:?}");
    }
}

/// Only the bitmaps' tables are renamed; every other table is where it was, and the directory
/// stays sorted.
#[test]
fn hiding_the_bitmaps_leaves_the_rest_of_the_face_alone() {
    let before = with_bitmaps(INTER);
    let mut after = before.clone();
    hide_bitmap_strikes(&mut after, 0);
    assert_eq!(before.len(), after.len());
    let tables = u16_at(&after, 4) as usize;
    let records = |bytes: &[u8]| -> Vec<[u8; 16]> { (0..tables).map(|i| bytes[12 + i * 16..28 + i * 16].try_into().unwrap()).collect() };
    let tags: Vec<[u8; 4]> = records(&after).iter().map(|r| r[..4].try_into().unwrap()).collect();
    assert!(tags.windows(2).all(|w| w[0] < w[1]), "sorted");
    assert!(tags.contains(b"eblc") && tags.contains(b"ebdt") && !tags.contains(b"EBLC") && !tags.contains(b"EBDT"));
    let mut renamed = records(&before);
    for r in &mut renamed {
        if matches!(&r[..4], b"EBLC" | b"EBDT") {
            r[..4].make_ascii_lowercase();
        }
    }
    renamed.sort_by_key(|r| [r[0], r[1], r[2], r[3]]);
    assert_eq!(renamed, records(&after));
    assert_eq!(before[12 + tables * 16..], after[12 + tables * 16..]);
    // A face without them is not touched.
    let mut plain = INTER.to_vec();
    hide_bitmap_strikes(&mut plain, 0);
    assert_eq!(plain, INTER);
}

/// A face of black-and-white bitmaps alone, with no outlines to draw from, is not used: its text
/// is drawn in another font rather than left blank.
#[test]
fn a_face_without_outlines_is_not_used() {
    let bitmaps_alone = glyf_renamed(&with_bitmaps(INTER));
    assert!(!is_face(&bitmaps_alone, 0));
    assert!(is_face(&with_bitmaps(INTER), 0));
    // Colour bitmaps alone, as the emoji faces of Linux and macOS have them, are drawn.
    let colour_alone = with_tables(&glyf_renamed(INTER), &[(b"CBLC", vec![0, 3, 0, 0, 0, 0, 0, 0]), (b"CBDT", vec![0, 3, 0, 0])]);
    assert!(is_face(&colour_alone, 0));
}

/// `font` with its `glyf` table renamed, so that it has no outlines.
fn glyf_renamed(font: &[u8]) -> Vec<u8> {
    let mut font = font.to_vec();
    for i in 0..u16_at(&font, 4) as usize {
        let tag = 12 + i * 16;
        if &font[tag..tag + 4] == b"glyf" {
            font[tag..tag + 4].copy_from_slice(b"glyx");
        }
    }
    font
}

/// Windows' own Calibri, where it is installed: every letter of a line is drawn, none in white.
#[cfg(windows)]
#[test]
fn calibri_draws_every_letter() {
    let Some(fonts) = ["WINDIR", "SystemRoot"].into_iter().filter_map(std::env::var_os).map(|r| std::path::PathBuf::from(r).join("Fonts")).next()
    else {
        return;
    };
    for file in ["calibri.ttf", "calibrib.ttf", "cambria.ttc"] {
        let Some(bytes) = read_face(&fonts.join(file), 64 * 1024 * 1024, 0) else { continue };
        assert_eq!(colour_and_blank(bytes, 0, TEXT), (Vec::new(), 0), "{file}");
    }
}
