// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The faces the composer's six families are drawn in (#582): which file is looked for, in what
//! order, and how a found face becomes a family. What a family outside the six is drawn in is
//! [`crate::fonts`]'s business, and tested there.

use egui::FontFamily;
use mailcraft_model::rich::Built;

use super::{SystemFont, document_family, face_files, first_face, font_definitions_with};

/// A font that is certainly one: the bundled Inter, standing in for an installed face.
const A_FONT: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

/// A folder of `files`, and its path.
fn folder(tag: &str, files: &[&str]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-doc-fonts-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in files {
        std::fs::write(dir.join(name), A_FONT).unwrap();
    }
    dir
}

#[test]
fn the_vendors_face_is_looked_for_before_the_open_ones() {
    let all = |_: &str| true;
    assert_eq!(first_face(Built::Arial, false, all), Some("arial.ttf"));
    // Only the open faces: Liberation Sans before Arimo.
    let open = |n: &str| n.starts_with("liberation") || n.starts_with("arimo");
    assert_eq!(first_face(Built::Arial, false, open), Some("liberationsans-regular.ttf"));
    assert_eq!(first_face(Built::Arial, true, |n: &str| n == "arimo-bold.ttf"), Some("arimo-bold.ttf"));
    assert_eq!(first_face(Built::Calibri, false, |n: &str| n == "carlito-regular.ttf"), Some("carlito-regular.ttf"));
    assert_eq!(first_face(Built::Georgia, false, |n: &str| n.starts_with("gelasio")), Some("gelasio-regular.ttf"));
    assert_eq!(first_face(Built::Verdana, true, |n: &str| n == "dejavusans-bold.ttf"), Some("dejavusans-bold.ttf"));
    assert_eq!(first_face(Built::Georgia, false, |_: &str| false), None);
}

#[test]
fn the_system_font_has_no_face_of_its_own() {
    // It is the window's own font; there is nothing to read.
    assert!(face_files(Built::System, false).is_empty());
    assert!(face_files(Built::System, true).is_empty());
}

#[test]
fn courier_new_is_looked_for_as_the_vendors_face_then_the_open_ones() {
    assert_eq!(first_face(Built::CourierNew, false, |_: &str| true), Some("cour.ttf"));
    assert_eq!(first_face(Built::CourierNew, true, |_: &str| true), Some("courbd.ttf"));
    // macOS names the file in full.
    assert_eq!(first_face(Built::CourierNew, true, |n: &str| n.starts_with("courier new")), Some("courier new bold.ttf"));
    // Only the open metric-compatible faces: Liberation Mono before Cousine.
    let open = |n: &str| n.starts_with("liberation") || n.starts_with("cousine");
    assert_eq!(first_face(Built::CourierNew, false, open), Some("liberationmono-regular.ttf"));
    assert_eq!(first_face(Built::CourierNew, true, |n: &str| n == "cousine-bold.ttf"), Some("cousine-bold.ttf"));
}

#[test]
fn an_installed_face_is_registered_with_the_windows_font_behind_it() {
    let dir = folder("georgia", &["Georgia.ttf"]);
    let system = SystemFont::none().with_document_faces(std::slice::from_ref(&dir));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(system.document_faces(), vec![(Built::Georgia, false)]);
    let fonts = font_definitions_with(&system);
    let chain = |bold| fonts.families.get(&document_family(Built::Georgia, bold)).cloned().unwrap();
    assert_eq!(chain(false).first().map(String::as_str), Some("DocGeorgia"));
    assert!(chain(false).contains(&"Inter".to_string()));
    // No bold file: no bold family, so bold text keeps the window's own bold.
    assert!(!fonts.families.contains_key(&document_family(Built::Georgia, true)));
    // Fonts nobody has installed get no family.
    assert!(!fonts.families.contains_key(&document_family(Built::Arial, false)));
    assert!(!fonts.families.contains_key(&FontFamily::Name("font-system".into())));
}

#[test]
fn an_installed_fixed_width_face_has_the_bundled_fixed_width_font_behind_it() {
    let dir = folder("mono", &["cour.ttf"]);
    let system = SystemFont::none().with_document_faces(std::slice::from_ref(&dir));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(system.document_faces(), vec![(Built::CourierNew, false)]);
    let fonts = font_definitions_with(&system);
    let chain = fonts.families.get(&document_family(Built::CourierNew, false)).cloned().unwrap();
    // A glyph the face lacks is drawn as wide as the others, in JetBrains Mono.
    assert_eq!(chain.get(..2), Some(&["DocCourier New".to_string(), "Mono".to_string()][..]), "{chain:?}");
}
