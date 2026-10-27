// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The faces a message's fonts are drawn in (#582): which file is looked for, in what order, and
//! how a found face becomes a family.

use egui::FontFamily;
use mailcraft_model::rich::Font;

use super::{SystemFont, document_family, document_font, face_files, first_face, font_definitions_with};

/// A font that is certainly one: the bundled Inter, standing in for an installed face.
const A_FONT: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

#[test]
fn the_vendors_face_is_looked_for_before_the_open_ones() {
    let all = |_: &str| true;
    assert_eq!(first_face(Font::Arial, false, all), Some("arial.ttf"));
    // Only the open faces: Liberation Sans before Arimo.
    let open = |n: &str| n.starts_with("liberation") || n.starts_with("arimo");
    assert_eq!(first_face(Font::Arial, false, open), Some("liberationsans-regular.ttf"));
    assert_eq!(first_face(Font::Arial, true, |n: &str| n == "arimo-bold.ttf"), Some("arimo-bold.ttf"));
    assert_eq!(first_face(Font::Calibri, false, |n: &str| n == "carlito-regular.ttf"), Some("carlito-regular.ttf"));
    assert_eq!(first_face(Font::Georgia, false, |n: &str| n.starts_with("gelasio")), Some("gelasio-regular.ttf"));
    assert_eq!(first_face(Font::Verdana, true, |n: &str| n == "dejavusans-bold.ttf"), Some("dejavusans-bold.ttf"));
    assert_eq!(first_face(Font::Georgia, false, |_: &str| false), None);
}

#[test]
fn the_system_font_has_no_face_of_its_own() {
    assert!(face_files(Font::System, false).is_empty());
    assert!(face_files(Font::System, true).is_empty());
    assert_eq!(document_font(Some(Font::System), false, 14.0, |_| true), None);
}

#[test]
fn a_font_is_drawn_in_its_family_only_when_one_was_registered() {
    let georgia = document_family(Font::Georgia, false);
    assert_eq!(document_font(Some(Font::Georgia), false, 14.0, |f| *f == georgia).map(|f| f.family), Some(georgia));
    assert_eq!(document_font(Some(Font::Georgia), false, 14.0, |_| false), None);
    assert_eq!(document_font(None, false, 14.0, |_| true), None);
}

#[test]
fn an_installed_face_is_registered_with_the_windows_font_behind_it() {
    let dir = std::env::temp_dir().join(format!("mailcraft-doc-fonts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Georgia.ttf"), A_FONT).unwrap();
    let system = SystemFont::none().with_document_faces(std::slice::from_ref(&dir));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(system.document_faces(), vec![(Font::Georgia, false)]);
    let fonts = font_definitions_with(&system);
    let chain = |bold| fonts.families.get(&document_family(Font::Georgia, bold)).cloned().unwrap();
    assert_eq!(chain(false).first().map(String::as_str), Some("DocGeorgia"));
    assert!(chain(false).contains(&"Inter".to_string()));
    // No bold file: no bold family, so bold text keeps the window's own bold.
    assert!(!fonts.families.contains_key(&document_family(Font::Georgia, true)));
    assert!(document_font(Some(Font::Georgia), true, 14.0, |f: &FontFamily| fonts.families.contains_key(f)).is_none());
    // Fonts nobody has installed get no family.
    assert!(!fonts.families.contains_key(&document_family(Font::Arial, false)));
    assert!(!fonts.families.contains_key(&FontFamily::Name("font-system".into())));
}
