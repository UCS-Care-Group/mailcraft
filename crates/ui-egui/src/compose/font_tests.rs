// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A run of the composer is drawn in the font it was given (#582): one of the six, or any family
//! installed on this computer (#1493) — and in the window's own font while a face is being read,
//! or when none is installed.

use egui::FontFamily;
use mailcraft_model::rich::{Built, Font, Style};

use super::editor::text_format;
use crate::fonts::Chooser;
use crate::theme::{Tokens, document_family, regular};

/// A window with the interface's fonts and the faces `system` found.
fn context(system: &crate::theme::SystemFont) -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::font_definitions_with(system));
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    ctx
}

/// A folder holding `files`, each a real font (the bundled Inter standing in).
fn folder(tag: &str, files: &[&str]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-compose-fonts-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in files {
        std::fs::write(dir.join(name), A_FONT).unwrap();
    }
    dir
}

const A_FONT: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");

/// What a run of `style` looks in a pass of `ctx`.
fn look(ctx: &egui::Context, style: &Style, plain: bool) -> FontFamily {
    let mut fonts = Chooser::new(ctx);
    let format = text_format(style, plain, &Tokens::light(), &mut fonts);
    fonts.finish(ctx);
    format.font_id.family
}

fn georgia() -> Style {
    Style { font: Built::Georgia.font().into(), ..Style::default() }
}

fn courier_new() -> Style {
    Style { font: Font::parse("Courier New"), ..Style::default() }
}

#[test]
fn a_run_is_drawn_in_the_family_registered_for_its_font() {
    let dir = folder("georgia", &["georgia.ttf", "georgiab.ttf"]);
    let ctx = context(&crate::theme::SystemFont::none().with_document_faces(std::slice::from_ref(&dir)));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(look(&ctx, &georgia(), false), document_family(Built::Georgia, false));
    // Bold in the bold family, since its face was installed too.
    let bold = Style { bold: true, ..georgia() };
    assert_eq!(look(&ctx, &bold, false), document_family(Built::Georgia, true));
}

#[test]
fn a_run_is_drawn_in_the_windows_font_when_none_was_installed() {
    let ctx = context(&crate::theme::SystemFont::none());
    assert_eq!(look(&ctx, &georgia(), false), regular(14.0).family);
    // A plain-text message draws everything in the window's font, whatever it was given.
    assert_eq!(look(&ctx, &georgia(), true), regular(14.0).family);
}

#[test]
fn the_system_font_is_the_windows_font() {
    // "System" asks for the reader's own interface font, which is what the window draws in
    // anyway: nothing is read for it (#1493).
    let ctx = context(&crate::theme::SystemFont::none());
    let style = Style { font: Built::System.font().into(), ..Style::default() };
    assert_eq!(look(&ctx, &style, false), regular(14.0).family);
    let bold = Style { bold: true, ..style };
    assert_eq!(look(&ctx, &bold, false), crate::theme::semibold(14.0).family);
}

#[test]
fn a_fixed_width_run_stays_fixed_width_when_its_face_isnt_installed() {
    // #969: text art keeps its columns while it is written, Courier New installed or not.
    let ctx = context(&crate::theme::SystemFont::none());
    assert_eq!(look(&ctx, &courier_new(), false), FontFamily::Monospace);
    let bold = Style { bold: true, ..courier_new() };
    assert_eq!(look(&ctx, &bold, false), FontFamily::Monospace);
}

#[test]
fn a_bold_fixed_width_run_without_a_bold_face_keeps_the_regular_face() {
    let dir = folder("mono", &["cour.ttf"]);
    let ctx = context(&crate::theme::SystemFont::none().with_document_faces(std::slice::from_ref(&dir)));
    let _ = std::fs::remove_dir_all(&dir);
    let regular_face = document_family(Built::CourierNew, false);
    let bold = Style { bold: true, ..courier_new() };
    assert_eq!(look(&ctx, &bold, false), regular_face);
}

#[test]
fn a_family_installed_here_is_asked_for_by_its_own_name() {
    // #1493: what the menu lists is what is installed; a family outside the six is drawn in its
    // own face once it has been read, and in the window's font until then.
    let dir = crate::font_index::font_index_tests::FontsDir::new("composer");
    dir.put("inter.ttf", A_FONT);
    let ctx = context(&crate::theme::SystemFont::none());
    crate::fonts::Store::use_index(&ctx, dir.index());
    let style = Style { font: Font::parse("Inter, monospace"), ..Style::default() };
    // The face isn't read yet: the window's font, and the read is asked for.
    assert_eq!(look(&ctx, &style, false), regular(14.0).family);
    // Once egui has the family, the run is drawn in it.
    let family = crate::fonts::family_of("inter", false);
    let mut fonts = ctx.fonts(|f| f.definitions().clone());
    fonts.font_data.insert("inter-face".into(), egui::FontData::from_static(A_FONT).into());
    fonts.families.insert(family.clone(), vec!["inter-face".into()]);
    ctx.set_fonts(fonts);
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    assert_eq!(look(&ctx, &style, false), family);
}
