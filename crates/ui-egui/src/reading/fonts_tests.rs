// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #600: a message is drawn in the first of its fonts that is installed here, once egui has it,
//! and in the window's font until then and when none is.

use egui::FontFamily;

use super::{Chooser, MAX_FACES, Pick, Store, family_of, generation};
use crate::font_index::font_index_tests::FontsDir;
use crate::reading::images::Pictures;
use crate::reading::layout::{ItemKind, Laid};

const REGULAR: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
const SEMIBOLD: &[u8] = include_bytes!("../../../../assets/fonts/Inter-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../../../../assets/fonts/JetBrainsMono-Regular.ttf");

/// A context with the interface's fonts, and the fonts of `dir` as what is installed.
fn context(dir: &FontsDir) -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::font_definitions_with(&crate::theme::SystemFont::none()));
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    Store::use_index(&ctx, dir.index());
    ctx
}

/// Run `f` in a pass of `ctx`.
fn pass<R>(ctx: &egui::Context, f: impl FnOnce(&egui::Context) -> R) -> R {
    let mut out = None;
    let mut f = Some(f);
    ctx.run_ui(egui::RawInput::default(), |ui| {
        if let Some(f) = f.take() {
            out = Some(f(ui.ctx()));
        }
    })
    .drop_without_applying_deltas();
    out.unwrap()
}

/// The families the text of `html` is laid out in, in a pass of `ctx`, in order.
fn families(ctx: &egui::Context, html: &str) -> Vec<FontFamily> {
    pass(ctx, |ctx| {
        let blocks = mailcraft_html::parse(html);
        let pictures = Pictures::of(ctx);
        let mut pictures = pictures.lock().unwrap();
        let mut laid = Laid::new(600.0);
        laid.advance(ctx, &blocks, None, &mut pictures, usize::MAX);
        assert!(laid.done);
        let mut out = Vec::new();
        for item in &laid.items {
            if let ItemKind::Text(text) = &item.kind {
                for section in &text.job.sections {
                    if !out.contains(&section.format.font_id.family) {
                        out.push(section.format.font_id.family.clone());
                    }
                }
            }
        }
        out
    })
}

fn installed(name: &str) -> FontsDir {
    let dir = FontsDir::new(name);
    dir.put("one.ttf", REGULAR);
    dir.put("two.ttf", SEMIBOLD);
    dir.put("three.ttf", MONO);
    dir
}

#[test]
fn the_first_installed_font_of_the_list_is_used_once_egui_has_it() {
    let dir = installed("first");
    let ctx = context(&dir);
    let html = "<p style=\"font-family:'No Such Font', Inter, serif\">Hello there</p>";
    let before = generation(&ctx);
    // The first layout asks for the face and is drawn in the window's font meanwhile.
    assert_eq!(families(&ctx, html), vec![FontFamily::Proportional]);
    // The face is given to egui, and a pass asked for, which takes it in: then it draws.
    assert!(ctx.has_requested_repaint());
    pass(&ctx, |_| {});
    assert_ne!(generation(&ctx), before);
    assert_eq!(families(&ctx, html), vec![family_of("inter", false)]);
    // The family has the window's fonts behind it, for the letters the face lacks.
    let chain = ctx.fonts(|f| f.definitions().families.get(&family_of("inter", false)).cloned()).unwrap();
    let window = ctx.fonts(|f| f.definitions().families.get(&FontFamily::Proportional).cloned()).unwrap();
    assert_eq!(chain.len(), window.len() + 1);
    assert!(chain.ends_with(&window));
    assert_eq!(Store::counts(&ctx).0, 1);
}

#[test]
fn bold_text_has_the_bold_face_and_a_family_without_one_the_windows_bold() {
    let dir = installed("bold");
    let ctx = context(&dir);
    let html = "<p style=\"font-family:Inter\"><b>Bold</b> and not</p><p style=\"font-family:'JetBrains Mono'\"><b>Bold</b> too</p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    let got = families(&ctx, html);
    assert!(got.contains(&family_of("inter", true)), "{got:?}");
    assert!(got.contains(&family_of("inter", false)), "{got:?}");
    assert!(got.contains(&family_of("jetbrains mono", false)), "{got:?}");
    // JetBrains Mono has no bold face: its bold text is the window's bold.
    assert!(!got.contains(&family_of("jetbrains mono", true)), "{got:?}");
    assert!(got.contains(&FontFamily::Name("semibold".into())), "{got:?}");
}

#[test]
fn generic_families_and_fonts_that_are_not_installed() {
    let dir = installed("generic");
    let ctx = context(&dir);
    let pick = |list: &str| pass(&ctx, |ctx| Chooser::new(ctx).pick(list, false));
    assert_eq!(pick("no such font"), Pick::Own);
    assert_eq!(pick("no such font, monospace"), Pick::Mono);
    assert_eq!(pick("courier-ish,ui-monospace"), Pick::Mono);
    // A generic family ends the list: the window's own font, whatever comes after.
    assert_eq!(pick("sans-serif, inter"), Pick::Own);
    assert_eq!(pick("cursive, inter"), Pick::Own);
    // `serif` is this system's serif font when one is installed, else the window's.
    assert_eq!(pick("serif, inter"), Pick::Own);
    // In the browser there is nothing installed, and `monospace` is all that counts.
    let mut none = Chooser::none();
    assert_eq!(none.pick("inter", false), Pick::Own);
    assert_eq!(none.pick("inter, monospace", false), Pick::Mono);
}

#[test]
fn monospace_is_laid_out_in_the_windows_monospaced_font() {
    let dir = installed("mono");
    let ctx = context(&dir);
    assert_eq!(families(&ctx, "<p style=\"font-family:'Courier New', monospace\">x = 1</p>"), vec![FontFamily::Monospace]);
}

#[test]
fn past_the_caps_a_font_counts_as_not_installed() {
    let dir = installed("caps");
    let ctx = context(&dir);
    Store::pretend_read(&ctx, MAX_FACES, 0);
    let html = "<p style=\"font-family:Inter, 'JetBrains Mono', monospace\">Hello</p>";
    let before = generation(&ctx);
    assert_eq!(families(&ctx, html), vec![FontFamily::Proportional]);
    // Neither face could be read: the list goes on to its next font each time.
    assert_ne!(generation(&ctx), before);
    families(&ctx, html);
    assert_eq!(families(&ctx, html), vec![FontFamily::Monospace]);
    assert_eq!(Store::counts(&ctx), (MAX_FACES, 0));
}

#[test]
fn a_hostile_list_reads_no_more_than_its_share() {
    let dir = installed("hostile");
    let ctx = context(&dir);
    // Every name a list can hold, none installed but one, which is read once.
    let names: Vec<String> = (0..100).map(|i| format!("nope{i}")).chain(["inter".to_string()]).collect();
    let html: String = (0..50).map(|_| format!("<p style=\"font-family:{}\">x</p><p style=\"font-family:inter\">y</p>", names.join(","))).collect();
    families(&ctx, &html);
    pass(&ctx, |_| {});
    let got = families(&ctx, &html);
    assert!(got.contains(&FontFamily::Proportional) && got.contains(&family_of("inter", false)), "{got:?}");
    assert_eq!(Store::counts(&ctx).0, 1);
}

#[test]
fn a_face_asked_for_by_two_names_is_read_once() {
    let dir = installed("twice");
    let ctx = context(&dir);
    // Inter's semibold file is "Inter SemiBold", and Inter's bold face.
    let html = "<p style=\"font-family:'Inter SemiBold'\">One</p><p style=\"font-family:Inter\"><b>Two</b></p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    let got = families(&ctx, html);
    assert!(got.contains(&family_of("inter semibold", false)) && got.contains(&family_of("inter", true)), "{got:?}");
    assert_eq!(Store::counts(&ctx), (1, SEMIBOLD.len() as u64));
}

#[test]
fn the_faces_are_given_again_when_egui_s_fonts_are_set_anew() {
    let dir = installed("anew");
    let ctx = context(&dir);
    let html = "<p style=\"font-family:Inter\">Hello</p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    assert_eq!(families(&ctx, html), vec![family_of("inter", false)]);
    // Something sets the window's fonts again, without the message's.
    ctx.set_fonts(crate::theme::font_definitions_with(&crate::theme::SystemFont::none()));
    pass(&ctx, |_| {});
    assert_eq!(families(&ctx, html), vec![FontFamily::Proportional]);
    pass(&ctx, super::tick);
    pass(&ctx, |_| {});
    assert_eq!(families(&ctx, html), vec![family_of("inter", false)]);
    assert_eq!(Store::counts(&ctx).0, 1);
}

#[test]
fn the_reading_pane_lays_a_message_out_again_once_its_font_has_come() {
    use crate::reading::Place;
    use crate::reading::tests::{app_with, select, web_message, window};
    let dir = installed("pane");
    let mut message = web_message(1, "news@example.org", "In a font of its own", &[]);
    message.body.html = Some("<p style=\"font-family:'No Such Font', Inter, sans-serif\">Set in Inter</p>".into());
    let (app, ids) = app_with(vec![message]);
    let mut harness = window(app, (900.0, 700.0));
    Store::use_index(&harness.ctx, dir.index());
    select(&mut harness, ids[0]);
    harness.run_steps(4);
    let families = crate::reading::view::with_laid(&harness.ctx, Place::Pane, ids[0], |laid| {
        laid.items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Text(text) => Some(text.job.sections.iter().map(|s| s.format.font_id.family.clone()).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>()
    })
    .unwrap();
    assert_eq!(families, vec![family_of("inter", false)]);
}
