// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Emoji in every font of the window (#832).

use egui::{FontFamily, FontId};

use super::{SystemFont, font_definitions_with};

/// Emoji as messages send them: a wink, a smile with sweat, a thumbs up, a heart.
const EMOJI: &str = "😉😅👍❤";

/// Every family of the window draws emoji: since egui 0.37
/// its emoji faces come only with their own feature, and without them each was an empty box.
#[test]
fn every_family_draws_emoji() {
    let fonts = font_definitions_with(&SystemFont::none());
    let families: Vec<FontFamily> = fonts.families.keys().cloned().collect();
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    for family in families {
        let id = FontId::new(12.0, family.clone());
        assert!(ctx.fonts_mut(|f| f.has_glyphs(&id, EMOJI)), "{family:?} lacks emoji");
    }
}

/// The system's colour emoji face, found in a fonts folder however its name is cased, goes in
/// front of egui's black-and-white emoji in every family, and nothing else moves.
#[test]
fn the_colour_emoji_face_goes_in_front_of_the_black_and_white() {
    let dir = std::env::temp_dir().join(format!("mailcraft-emoji-fonts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("noto")).unwrap();
    // The bundled Inter stands in for a colour emoji face.
    std::fs::write(dir.join("noto/NOTOCOLOREMOJI.TTF"), include_bytes!("../../../assets/fonts/Inter-Regular.ttf")).unwrap();
    let system = SystemFont::none().with_emoji_from(std::slice::from_ref(&dir), &["missing.ttf", "NotoColorEmoji.ttf"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(system.has_emoji());
    let fonts = font_definitions_with(&system);
    let without = font_definitions_with(&SystemFont::none());
    for (family, faces) in &fonts.families {
        let at = faces.iter().position(|name| name == "SystemEmoji").unwrap_or_else(|| panic!("{family:?} has no colour emoji"));
        assert_eq!(faces[at + 1], "NotoEmoji-Regular", "{family:?}");
        let mut rest = faces.clone();
        rest.remove(at);
        assert_eq!(rest, without.families[family], "{family:?}");
    }
}

/// No colour emoji face in the folders: none is used, and the families are as without one.
#[test]
fn no_colour_emoji_face_leaves_the_families_alone() {
    let dir = std::env::temp_dir().join(format!("mailcraft-no-emoji-fonts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let system = SystemFont::none().with_emoji_from(std::slice::from_ref(&dir), &["NotoColorEmoji.ttf"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!system.has_emoji());
    assert_eq!(font_definitions_with(&system).families, font_definitions_with(&SystemFont::none()).families);
}
