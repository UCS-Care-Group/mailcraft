// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #600: a message is drawn in the first of its fonts that is installed here, once egui has it,
//! and in the window's font until then and when none is.

use egui::FontFamily;

use super::{
    Chooser, FaceState, IndexState, MAX_BYTES, MAX_FACES, Pick, Store, ask, coming, family_of, generation, generic_family, inspect, names, rescan,
    tick,
};
use crate::font_index::font_index_tests::{FontsDir, as_class, renamed};
use crate::reading::images::Pictures;
use crate::reading::layout::{ItemKind, Laid};

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
/// A proportional font with no bold face of its own.
const SANS: &[u8] = include_bytes!("../../../assets/fonts/SourceSans3-Regular.ttf");

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
    // JetBrains Mono has no bold face — and its characters are as wide as each other, so its bold
    // text keeps its own regular face and its columns stay lined up (#969).
    assert!(!got.contains(&family_of("jetbrains mono", true)), "{got:?}");
    assert!(!got.contains(&FontFamily::Name("semibold".into())), "{got:?}");
}

#[test]
fn bold_text_in_a_family_without_a_bold_face_is_the_windows_bold() {
    // #582, for any family and not the composer's six alone: egui can't make a face bold.
    let dir = FontsDir::new("sans");
    dir.put("sans.ttf", SANS);
    let ctx = context(&dir);
    let html = "<p style=\"font-family:'Source Sans 3'\"><b>Bold</b> and not</p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    let got = families(&ctx, html);
    assert!(got.contains(&family_of("source sans 3", false)), "{got:?}");
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
fn a_family_is_one_entry_however_its_faces_are_named() {
    // Inter's semibold file calls itself "Inter SemiBold" and its family "Inter": it is Inter's
    // bold face, not a font of its own (#1493) — one entry in the menu, and what asks for it by
    // that name is drawn in Inter.
    let dir = installed("twice");
    let ctx = context(&dir);
    let index = dir.index();
    let names = index.names();
    assert_eq!(names.iter().filter(|n| n.eq_ignore_ascii_case("inter")).count(), 1, "{names:?}");
    assert!(!names.contains(&"Inter SemiBold"), "{names:?}");
    assert!(index.has("inter semibold"), "still found by the name its file gives it");
    let html = "<p style=\"font-family:'Inter SemiBold'\">One</p><p style=\"font-family:Inter\"><b>Two</b></p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    // What asks for the semibold by name asks for the *family*, and is drawn in its regular face;
    // bold text in the family is drawn in the semibold face.
    let got = families(&ctx, html);
    assert!(got.contains(&family_of("inter semibold", false)) && got.contains(&family_of("inter", true)), "{got:?}");
    // The regular face for the one, the semibold face for the bold of the other: each read once.
    assert_eq!(Store::counts(&ctx), (2, (REGULAR.len() + SEMIBOLD.len()) as u64));
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
fn bold_text_in_the_fixed_width_font_without_its_bold_face_keeps_the_regular_face() {
    // #969: a bold line of text art keeps its columns, as it does while it is written.
    let dir = installed("courier");
    // Courier New's regular face alone, as the composer's fonts are found at start.
    let faces = FontsDir::new("courier-faces");
    faces.put("cour.ttf", MONO);
    let system = crate::theme::SystemFont::none().with_document_faces(std::slice::from_ref(&faces.0));
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::font_definitions_with(&system));
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    Store::use_index(&ctx, dir.index());
    let courier = mailcraft_model::rich::Built::CourierNew;
    let html = "<p style=\"font-family:'Courier New', Courier, monospace\"><b>+--+</b> |  |</p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    assert_eq!(families(&ctx, html), vec![crate::theme::document_family(courier, false)]);
}

#[test]
fn bold_text_in_a_fixed_family_whose_regular_face_failed_goes_on_to_the_next_font() {
    // #1493: a fixed family without a bold face is drawn in its own regular face, so its columns
    // hold. When that face can't be read, asking for it again every frame would never reach the
    // font written after it.
    let dir = installed("boldfailed");
    let ctx = context(&dir);
    Store::pretend_read(&ctx, MAX_FACES, MAX_BYTES);
    let first = pass(&ctx, |ctx| {
        let mut chooser = Chooser::new(ctx);
        let pick = chooser.pick("jetbrains mono, monospace", true);
        chooser.finish(ctx);
        pick
    });
    assert_eq!(first, Pick::Own, "its regular face is being asked for");
    let second = pass(&ctx, |ctx| {
        let mut chooser = Chooser::new(ctx);
        let pick = chooser.pick("jetbrains mono, monospace", true);
        chooser.finish(ctx);
        pick
    });
    assert_eq!(second, Pick::Mono, "the regular face failed: the list goes on to `monospace` (#1493)");
}

#[test]
fn looking_again_forgets_the_faces_that_couldnt_be_read() {
    // #1493: "Check for new fonts" lets a face that failed be read again — its file may have been
    // replaced — and a layout is made again whether or not the new list says anything different.
    let dir = installed("again");
    let ctx = context(&dir);
    Store::pretend_read(&ctx, MAX_FACES, MAX_BYTES);
    pass(&ctx, |ctx| {
        let mut chooser = Chooser::new(ctx);
        chooser.pick("inter", false);
        chooser.finish(ctx);
    });
    let store = Store::of(&ctx);
    let failed = |store: &std::sync::Arc<std::sync::Mutex<Store>>| {
        store.lock().unwrap_or_else(|e| e.into_inner()).faces.values().filter(|s| matches!(s, FaceState::Missing)).count()
    };
    assert!(failed(&store) > 0, "past the caps a face counts as not installed");
    let before = generation(&ctx);
    rescan(&ctx);
    assert_eq!(failed(&store), 0, "the look lets the faces that failed be asked for again");
    assert_ne!(generation(&ctx), before, "and the layout is made again");
}

#[test]
fn a_family_that_isnt_here_is_drawn_in_the_one_its_like() {
    // #1493: a message asking for Carlito, on a computer that has Calibri instead, keeps its look
    // rather than falling back on the window's own font. The name that goes out is untouched.
    assert_eq!(mailcraft_model::rich::Font::parse("Carlito").unwrap().css(), "carlito", "the name sent is the name asked for");
    let dir = FontsDir::new("cousin");
    dir.put("calibri.ttf", &renamed(REGULAR, "Calibri"));
    let ctx = context(&dir);
    let html = "<p style=\"font-family:Carlito\">Hello there</p>";
    // The first pass asks for the cousin's face and is drawn in the window's font meanwhile.
    assert_eq!(families(&ctx, html), vec![FontFamily::Proportional]);
    pass(&ctx, |_| {});
    assert_eq!(families(&ctx, html), vec![family_of("calibri", false)], "Carlito is drawn in Calibri (#1493)");
}

#[test]
fn a_fixed_family_that_isnt_here_keeps_its_columns() {
    // The part of the same thing that matters most: text lined up with spaces stays lined up when
    // the fixed family the message asks for isn't installed (#1493, #969).
    let dir = FontsDir::new("cousin-mono");
    dir.put("lm.ttf", &renamed(MONO, "Liberation Mono"));
    let ctx = context(&dir);
    let html = "<p style=\"font-family:Consolas\">columns</p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    assert_eq!(families(&ctx, html), vec![family_of("liberation mono", false)], "Consolas keeps a fixed family (#1493)");
}

#[test]
fn a_plain_courier_name_keeps_its_columns() {
    // The name a message asks for can be the plain one, without the "New": what changed is the
    // name, not the widths, so the columns have to hold anyway (#969, #1493).
    let dir = FontsDir::new("courier-cousin");
    dir.put("lm.ttf", &renamed(MONO, "Liberation Mono"));
    let ctx = context(&dir);
    let html = "<p style=\"font-family:Courier\">+--+  |  |</p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    assert_eq!(families(&ctx, html), vec![family_of("liberation mono", false)], "Courier keeps a fixed family (#1493)");
}

#[test]
fn geneva_is_drawn_in_verdana_when_geneva_isnt_here() {
    let dir = FontsDir::new("geneva");
    dir.put("v.ttf", &renamed(REGULAR, "Verdana"));
    let ctx = context(&dir);
    let html = "<p style=\"font-family:Geneva\">Hello there</p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    assert_eq!(families(&ctx, html), vec![family_of("verdana", false)], "Geneva is drawn in Verdana (#1493)");
}

#[test]
fn a_family_with_no_cousin_here_is_the_windows_own_still() {
    let dir = FontsDir::new("no-cousin");
    dir.put("one.ttf", &renamed(REGULAR, "Calibri"));
    let ctx = context(&dir);
    let html = "<p style=\"font-family:Nonesuch\">Hello there</p>";
    families(&ctx, html);
    pass(&ctx, |_| {});
    assert_eq!(families(&ctx, html), vec![FontFamily::Proportional], "nothing is invented for a name that has no cousin");
}

#[test]
fn the_generic_written_after_a_family_follows_its_own_class() {
    // #1493: what a recipient without the family chosen gets. The IBM classes the `OS/2` field is
    // filled with: slab serifs (5) are serif, class 8 is a sans-serif, and only 10 is written
    // rather than set.
    for (class, want) in [(5u8, "serif"), (8, "sans-serif"), (10, "cursive"), (0, "sans-serif")] {
        let dir = FontsDir::new(&format!("generic-{class}"));
        dir.put("s.ttf", &as_class(REGULAR, class));
        let ctx = context(&dir);
        assert_eq!(generic_family(&ctx, "Inter"), want, "class {class}");
    }
}

#[test]
fn a_face_waits_for_the_look_to_end_before_it_is_given_to_egui() {
    // #1493: while the fonts installed here are being looked at again, nothing is known about
    // which families are fixed width — a face given to egui then would keep the window's
    // proportional fonts behind it for good, because a family egui has is given again.
    let dir = installed("waiting");
    let ctx = context(&dir);
    let store = Store::of(&ctx);
    let key = ("jetbrains mono".to_string(), false);
    let given = |ctx: &egui::Context| ctx.fonts(|f| f.definitions().families.contains_key(&family_of("jetbrains mono", false)));
    let (send, receive) = std::sync::mpsc::channel::<super::FontIndex>();
    {
        let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
        store.index = IndexState::Building(receive);
        // As though a batch that began before the look had just brought it in.
        store.set(key, FaceState::Ready(std::sync::Arc::new(egui::FontData::from_owned(MONO.to_vec()))));
        store.dirty = true;
    }
    tick(&ctx);
    assert!(!given(&ctx), "the face waits for the look to end (#1493)");
    drop(send);
    tick(&ctx);
    assert!(given(&ctx), "and is given once the look is over");
}

#[test]
fn a_failure_from_before_the_last_look_doesnt_close_a_repaired_font() {
    // #1493: a read that failed while the owner was fixing the file says nothing about what is on
    // disk now — the face is asked for again rather than kept as not to be had.
    let dir = installed("repaired");
    let ctx = context(&dir);
    let store = Store::of(&ctx);
    let key = ("inter".to_string(), false);
    let failed = |store: &std::sync::Arc<std::sync::Mutex<Store>>| {
        store.lock().unwrap_or_else(|e| e.into_inner()).faces.get(&key).is_some_and(|s| matches!(s, FaceState::Missing))
    };
    let out = |store: &std::sync::Arc<std::sync::Mutex<Store>>| {
        store.lock().unwrap_or_else(|e| e.into_inner()).take(vec![(key.clone(), None)]);
    };
    // The same failure with no look in between is the face not being there, as it was.
    {
        let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
        store.faces.insert(key.clone(), FaceState::Queued);
        store.reading = vec![key.clone()];
        store.reading_rev = store.revision;
    }
    out(&store);
    assert!(failed(&store), "a face that can't be read counts as not installed");
    store.lock().unwrap_or_else(|e| e.into_inner()).faces.remove(&key);
    // Now the same failure from a batch that began before a look of its own.
    {
        let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
        store.faces.insert(key.clone(), FaceState::Queued);
        store.reading = vec![key.clone()];
        store.reading_rev = store.revision;
    }
    rescan(&ctx);
    let before = generation(&ctx);
    out(&store);
    assert!(!failed(&store), "the repaired font is asked for again (#1493)");
    // Letting it go is a change in what a layout is made with: the layout that settled on the
    // fallback is made again, and that is what asks for the face.
    assert_ne!(generation(&ctx), before, "and the layout is made again (#1493)");
}

#[test]
fn a_font_menu_lists_the_families_installed_here() {
    // #1493: the composer's and the signature editor's font menu. The six are listed by the menu
    // itself; what follows them is every other family this computer has.
    let dir = installed("menu");
    let ctx = context(&dir);
    let listed = names(&ctx).unwrap();
    assert!(listed.contains(&"Inter".to_string()) && listed.contains(&"JetBrains Mono".to_string()), "{listed:?}");
    // What the menu sends names the family and a font of its kind after it, so that a recipient
    // without it gets one of the same kind.
    assert_eq!(generic_family(&ctx, "JetBrains Mono"), "monospace");
    assert_eq!(generic_family(&ctx, "Inter"), "sans-serif");
    // The same answer for a name as the index keeps it, and for one it has never heard of.
    assert_eq!(generic_family(&ctx, "inter semibold"), "sans-serif");
    assert_eq!(generic_family(&ctx, "No Such Font"), "sans-serif");
    // What the control port answers: the list, and whether it is still being made. Nothing of the
    // owner's travels with it, and nothing is fetched for it.
    let seen = inspect(&ctx);
    assert_eq!(seen["coming"], serde_json::json!(false));
    assert!(seen["families"].as_array().is_some_and(|a| a.contains(&serde_json::json!("Inter"))), "{seen}");
}

#[test]
fn the_list_arrives_when_it_is_made_and_can_be_made_again() {
    // A window that never asked has nothing to list; asking starts the thread of its own, and a
    // window that asks twice is not asked twice.
    let ctx = egui::Context::default();
    assert!(names(&ctx).is_none());
    assert_eq!(inspect(&ctx)["families"], serde_json::json!([]));
    ask(&ctx);
    ask(&ctx);
    // Once the list is made it is what a menu lists, and `inspect` says the same.
    let dir = installed("arrived");
    Store::use_index(&ctx, dir.index());
    assert!(names(&ctx).is_some());
    assert!(inspect(&ctx)["families"].as_array().is_some_and(|a| !a.is_empty()), "{}", inspect(&ctx));
    // Looking again starts over, and the new list is taken in when it comes.
    rescan(&ctx);
    Store::use_index(&ctx, dir.index());
    assert!(names(&ctx).is_some());
    // A test's window has the fonts it is given, so nothing is ever "still coming" here; on a
    // real window it is, until the list arrives.
    assert!(!coming(&ctx));
}
