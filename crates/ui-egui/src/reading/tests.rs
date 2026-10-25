// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reading pane on the corpus of `mailcraft-html`: every message laid out in both themes and
//! at every width, what is shown of pictures and what is not, tables, contrast, and time.
//!
//! The pictures the corpus refers to are drawn here, in code.

use std::sync::{Arc, Mutex, mpsc};

use egui::Color32;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_html::Span;
use mailcraft_html::corpus::{self, PictureKind};
use mailcraft_model::{Address, Attachment, Body, Mailbox, Message};
use serde_json::json;

use super::Place;
use super::colors::{MIN_CONTRAST, contrast};
use super::images::Pictures;
use super::layout::{Item, ItemKind, Laid, PIECE_CHARS, pieces};
use super::view::{Doc, FRAME_BUDGET};
use crate::theme::{Appearance, Tokens};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

// ---- pictures, drawn in code ----

/// Something that looks like a screenshot: a window with a title bar, fields and a button.
fn drawing(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w.max(1) as f32, y as f32 / h.max(1) as f32);
        let edge = x < 2 || y < 2 || x + 2 >= w || y + 2 >= h;
        let rgb = if edge {
            [0x8a, 0x94, 0xa3]
        } else if fy < 0.12 {
            [0x1f, 0x5f, 0x8b]
        } else if (0.25..0.33).contains(&fy) && (0.08..0.6).contains(&fx) || (0.42..0.5).contains(&fy) && (0.08..0.8).contains(&fx) {
            [0xff, 0xff, 0xff]
        } else if (0.72..0.86).contains(&fy) && (0.68..0.92).contains(&fx) {
            [0x2e, 0x8b, 0x57]
        } else {
            [0xee, 0xf1, 0xf5]
        };
        image::Rgba([rgb[0], rgb[1], rgb[2], 0xff])
    })
}

/// A round mark on nothing: a picture with transparency.
fn mark(w: u32, h: u32, rgb: [u8; 3]) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0);
        let r = w.min(h) as f32 / 2.0 - 1.0;
        let inside = dx * dx + dy * dy <= r * r;
        let bar = dy.abs() < r * 0.14 && dx.abs() < r * 0.6;
        match (inside, bar) {
            (true, true) => image::Rgba([0xff, 0xff, 0xff, 0xff]),
            (true, false) => image::Rgba([rgb[0], rgb[1], rgb[2], 0xff]),
            _ => image::Rgba([0, 0, 0, 0]),
        }
    })
}

fn encode(image: image::DynamicImage, format: image::ImageFormat) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, format).unwrap();
    out.into_inner()
}

pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
    encode(image::DynamicImage::ImageRgba8(drawing(w, h)), image::ImageFormat::Png)
}

pub(crate) fn jpeg(w: u32, h: u32) -> Vec<u8> {
    encode(image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(drawing(w, h)).into_rgb8()), image::ImageFormat::Jpeg)
}

pub(crate) fn gif(w: u32, h: u32) -> Vec<u8> {
    encode(image::DynamicImage::ImageRgba8(mark(w, h, [0x2e, 0x8b, 0x57])), image::ImageFormat::Gif)
}

pub(crate) fn webp(w: u32, h: u32) -> Vec<u8> {
    encode(image::DynamicImage::ImageRgba8(drawing(w, h)), image::ImageFormat::WebP)
}

/// The bytes of a picture the corpus refers to.
fn picture(id: &str, kind: PictureKind, w: u32, h: u32) -> (Vec<u8>, &'static str, &'static str) {
    match kind {
        PictureKind::Png if id.contains("mark") || id.contains("image001") => {
            let rgb = if id.contains("image001") { [0x1f, 0x38, 0x64] } else { [0x00, 0x78, 0x8c] };
            (encode(image::DynamicImage::ImageRgba8(mark(w, h, rgb)), image::ImageFormat::Png), "image/png", "png")
        }
        PictureKind::Png => (png(w, h), "image/png", "png"),
        PictureKind::Jpeg => (jpeg(w, h), "image/jpeg", "jpg"),
        PictureKind::Gif if id.contains("cross") => {
            (encode(image::DynamicImage::ImageRgba8(mark(w, h, [0xc4, 0x2b, 0x1c])), image::ImageFormat::Gif), "image/gif", "gif")
        }
        PictureKind::Gif => (gif(w, h), "image/gif", "gif"),
        PictureKind::Damaged => {
            (b"\x89PNG\r\n\x1a\nthis was a picture once, before the capture tool had its way with it".to_vec(), "image/png", "png")
        }
    }
}

/// Every picture of the corpus as an inline attachment.
fn pictures() -> Vec<Attachment> {
    corpus::PICTURES
        .iter()
        .map(|(id, kind, w, h)| {
            let (bytes, mime, ext) = picture(id, *kind, *w, *h);
            Attachment {
                content_id: id.to_string(),
                inline: true,
                ..Attachment::new(format!("{}.{ext}", id.split(['@', '.']).next().unwrap_or("picture")), mime, bytes)
            }
        })
        .collect()
}

// ---- the corpus as messages ----

fn message(n: usize, from: (&str, &str), subject: &str, html: Option<String>, text: String) -> Message {
    let date = NOW - 3_600 * (n as i64 + 1);
    Message {
        from: Address::new(from.0, from.1),
        to: vec![Address::new("Avery Quinn", "avery@lumen-works.example")],
        subject: subject.to_string(),
        date,
        received: date,
        read: true,
        body: Body { text, html },
        // On the server (mail from a file is never vouched for).
        uid: 1,
        // The server vouched for every sender (a test of the lists would be of nothing otherwise).
        headers: vec![(
            "Authentication-Results".into(),
            format!("mx.example.org; dmarc=pass header.from={}", from.1.rsplit('@').next().unwrap_or("")),
        )],
        ..Message::default()
    }
}

fn html_message(n: usize, from: (&str, &str), subject: &str, html: &str) -> Message {
    let mut m = message(n, from, subject, Some(html.to_string()), mailcraft_html::to_text(html));
    // Each message carries the pictures it refers to, and no others.
    m.attachments = pictures().into_iter().filter(|a| html.contains(&format!("cid:{}", a.content_id))).collect();
    m
}

/// The corpus as mail: the ordinary messages, the plain-text thread, then the hostile ones.
pub(crate) fn corpus_messages() -> Vec<(&'static str, Message)> {
    let mut out = vec![
        (
            "outlook reply",
            html_message(
                0,
                ("Marta Lindqvist", "marta.lindqvist@tidewater-freight.example"),
                "RE: Q4 lane pricing: revised schedule",
                corpus::OUTLOOK_REPLY,
            ),
        ),
        ("gmail reply", html_message(1, ("Kofi Mensah", "kofi@juniperlabs.example"), "Re: Agenda for Friday", corpus::GMAIL_REPLY)),
        (
            "newsletter",
            html_message(
                2,
                ("Fernhollow Outfitters", "hello@news.fernhollow.example"),
                "Autumn on the ridge: new arrivals and a members' weekend",
                corpus::NEWSLETTER,
            ),
        ),
        (
            "receipt",
            html_message(3, ("Harrow & Birch Stationers", "orders@harrowbirch.example"), "Your order HB-20418 is on its way", corpus::RECEIPT),
        ),
        (
            "screenshots",
            html_message(4, ("Elias Brandt", "elias.brandt@quillon.example"), "Export dialog still wrong after the update", corpus::SCREENSHOTS),
        ),
        (
            "calendar notice",
            html_message(5, ("Samira Okonkwo", "calendar@lumen-works.example"), "Invitation: Design review: onboarding flow v3", corpus::CALENDAR),
        ),
        ("plain thread", message(6, ("Jorge Salas", "jorge@orchard-tools.example"), "Re: [dev] 4.8.1 rc2", None, corpus::PLAIN_THREAD.to_string())),
        (
            "built newsletter",
            html_message(
                30,
                ("Tallow & Wick Candle Works", "lamplight@letters.tallowwick.example"),
                "Lamplight: the evenings are drawing in",
                corpus::BUILT_NEWSLETTER,
            ),
        ),
        (
            "access digest",
            html_message(31, ("Bellweather Directory", "directory@bellweather.example"), "Your weekly access digest", corpus::ACCESS_DIGEST),
        ),
        (
            "style width table",
            html_message(32, ("Lamplight Trading", "payments@lamplight.example"), "Your weekly payments summary", corpus::STYLE_WIDTH_TABLE),
        ),
        ("style card", html_message(33, ("Lamplight Shop", "news@lamplight.example"), "New in the shop", corpus::STYLE_CARD)),
        (
            "style boxes",
            html_message(34, ("Hollowpine Rambling Club", "bulletin@hollowpine.example"), "The Hollowpine bulletin", corpus::STYLE_BOXES),
        ),
    ];
    // A file that is really attached, next to the pictures shown in place.
    if let Some((_, m)) = out.get_mut(4) {
        m.attachments.push(Attachment::new("export.log", "text/plain", b"09:14:02 export: delimiter is empty\n".to_vec()));
    }
    for (n, (name, html)) in corpus::hostile().into_iter().enumerate() {
        out.push((name, html_message(7 + n, ("Nobody You Know", "sender@evil.example"), &format!("Hostile: {name}"), &html)));
    }
    out
}

fn large_newsletter() -> Message {
    html_message(
        40,
        ("The Long Read", "editors@longread.example"),
        "The Long Read: everything, at length",
        &corpus::large_newsletter(mailcraft_html::MAX_INPUT),
    )
}

// ---- laying out without a window ----

/// Run `f` inside a frame of a context that has the interface's fonts and theme.
fn in_frame<R>(dark: bool, f: impl FnOnce(&egui::Context) -> R) -> R {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::font_definitions());
    crate::theme::apply(&ctx, if dark { Appearance::Dark } else { Appearance::Light }.preference(None));
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
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

/// A message laid out to the end at `width`.
fn lay_out(ctx: &egui::Context, m: &Message, width: f32) -> Laid {
    let doc = Doc::of(m);
    let pictures = Pictures::of(ctx);
    let mut pictures = pictures.lock().unwrap();
    let mut laid = Laid::new(width).cut(doc.cuts.clone());
    laid.advance(ctx, &doc.blocks, Some(m), &mut pictures, usize::MAX);
    assert!(laid.done);
    laid
}

fn corpus_message(name: &str) -> Message {
    corpus_messages().into_iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no {name} in the corpus")).1
}

/// The text of a laid-out piece.
fn text_of(item: &Item) -> &str {
    match &item.kind {
        ItemKind::Text(text) => &text.job.text,
        _ => "",
    }
}

fn find<'a>(laid: &'a Laid, start: &str) -> &'a Item {
    laid.items.iter().find(|i| text_of(i).starts_with(start)).unwrap_or_else(|| panic!("no text starts with {start:?}"))
}

/// What every layout must be: finished, finite, inside its own size, with every text readable.
fn check(laid: &Laid, t: &Tokens, what: &str) {
    assert!(laid.done, "{what}");
    assert!(laid.size.x.is_finite() && laid.size.y.is_finite() && laid.size.x >= laid.width() && laid.size.y >= 0.0, "{what}: {:?}", laid.size);
    for item in &laid.items {
        let r = item.rect;
        assert!(r.min.x.is_finite() && r.min.y.is_finite() && r.max.x.is_finite() && r.max.y.is_finite(), "{what}: {item:?}");
        assert!(r.width() >= 0.0 && r.height() >= 0.0, "{what}: an item inside out: {item:?}");
        assert!(
            r.min.y >= -0.5 && r.max.y <= laid.size.y + 0.5 && r.max.x <= laid.size.x + 0.5,
            "{what}: an item outside the message ({:?}): {item:?}",
            laid.size
        );
        if let ItemKind::Text(text) = &item.kind {
            assert!(!text.job.text.is_empty(), "{what}: empty text");
            for section in &text.job.sections {
                let paper = if section.format.background == Color32::TRANSPARENT { text.paper } else { section.format.background };
                let ratio = contrast(section.format.color, paper);
                let shown = text.job.text.get(section.byte_range.start.0..section.byte_range.end.0).unwrap_or("");
                assert!(
                    ratio >= MIN_CONTRAST - 0.01,
                    "{what}: {:?} on {paper:?} ({ratio:.2}) in the {} theme: {shown:?}",
                    section.format.color,
                    if t.dark { "dark" } else { "light" }
                );
                assert!((9.0..=33.0).contains(&section.format.font_id.size), "{what}: text of size {}", section.format.font_id.size);
            }
        }
    }
}

#[test]
fn the_whole_corpus_lays_out_in_both_themes_at_every_width() {
    let messages = corpus_messages();
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let t = Tokens::get(ctx);
            assert_eq!(t.dark, dark);
            for (name, m) in &messages {
                for width in [860.0, 420.0, 180.0, 1.0] {
                    let laid = lay_out(ctx, m, width);
                    check(&laid, &t, &format!("{name} at {width}"));
                    assert!(laid.stats.texts > 0 && laid.size.y > 10.0, "{name} at {width}: nothing was laid out");
                }
            }
        });
    }
}

// ---- the corpus in a window ----

/// Its server is the one that wrote the `mx.example.org` line the corpus carries.
fn served_by_example_org(mailbox: &mut Mailbox) {
    for account in &mut mailbox.accounts {
        account.kind = mailcraft_model::AccountKind::Imap;
        account.incoming = Some(mailcraft_model::Server { host: "imap.example.org".into(), port: 993, security: mailcraft_model::Security::Tls });
    }
}

/// A window on a mailbox that holds the corpus, and the id of each message by its name.
fn corpus_app(large: bool) -> (MailApp, Vec<(&'static str, u64)>) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    served_by_example_org(&mut mailbox);
    let inbox = mailbox.inbox().unwrap();
    let mut ids = Vec::new();
    let mut messages = corpus_messages();
    if large {
        messages.push(("large newsletter", large_newsletter()));
    }
    for (name, mut m) in messages {
        m.folder = inbox;
        ids.push((name, mailbox.add_message(m).unwrap()));
    }
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    (app, ids)
}

/// The corpus as a mailbox file, to look at in the app or with the screenshot example:
/// `MAILCRAFT_CORPUS_DIR=<dir> cargo test -p mailcraft-ui-egui write_the_corpus -- --ignored`
/// writes `<dir>/corpus.mailcraft` and `<dir>/corpus-ids.txt` (a message id and its name per line).
#[test]
#[ignore = "writes the corpus as a mailbox file, for looking at it"]
fn write_the_corpus_as_a_mailbox() {
    let Ok(dir) = std::env::var("MAILCRAFT_CORPUS_DIR") else {
        eprintln!("set MAILCRAFT_CORPUS_DIR to the directory to write to");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let (mut app, ids) = corpus_app(true);
    app.execute("file.save", json!({"path": dir.join("corpus.mailcraft")})).unwrap();
    let list: String = ids.iter().map(|(name, id)| format!("{id}\t{name}\n")).collect();
    std::fs::write(dir.join("corpus-ids.txt"), list).unwrap();
}

fn window(app: MailApp, size: (f32, f32)) -> egui_kittest::Harness<'static, MailApp> {
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(size.0, size.1)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(3);
    harness
}

fn id_of(ids: &[(&'static str, u64)], name: &str) -> u64 {
    ids.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no {name}")).1
}

fn find_exact<'a>(laid: &'a Laid, text: &str) -> &'a Item {
    laid.items.iter().find(|i| text_of(i) == text).unwrap_or_else(|| panic!("no text is {text:?}"))
}

fn shown_pictures(laid: &Laid) -> Vec<&Item> {
    laid.items.iter().filter(|i| matches!(i.kind, ItemKind::Picture(_))).collect()
}

fn stand_ins(laid: &Laid) -> Vec<(&str, bool)> {
    laid.items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Placeholder(p) => Some((p.label.as_str(), p.remote)),
            _ => None,
        })
        .collect()
}

/// How many quote bars run past the middle of an item.
fn bars_beside(laid: &Laid, item: &Item) -> usize {
    let y = item.rect.center().y;
    laid.items
        .iter()
        .filter(|i| {
            matches!(i.kind, ItemKind::Fill { .. })
                && (i.rect.width() - 2.0).abs() < 0.01
                && i.rect.min.y <= y
                && y <= i.rect.max.y
                && i.rect.max.x <= item.rect.min.x
        })
        .count()
}

#[test]
fn remote_pictures_are_stand_ins_and_nothing_is_looked_at_for_them() {
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            for name in ["newsletter", "built newsletter", "remote", "many pictures", "calendar notice", "receipt"] {
                let m = corpus_message(name);
                assert!(m.attachments.is_empty(), "{name}");
                let laid = lay_out(ctx, &m, 820.0);
                assert_eq!(laid.stats.pictures, 0, "{name}");
                assert!(shown_pictures(&laid).is_empty(), "{name}");
            }
            let newsletter = lay_out(ctx, &corpus_message("newsletter"), 820.0);
            assert_eq!((newsletter.stats.placeholders, newsletter.stats.remote), (8, 8));
            assert!(stand_ins(&newsletter).contains(&("Two walkers on a ridge at first light", true)));
            // The one a column builder wrote: all nine of its pictures have their stand-in, and
            // its text is there around them.
            let built = lay_out(ctx, &corpus_message("built newsletter"), 820.0);
            assert_eq!((built.stats.placeholders, built.stats.remote), (9, 9));
            assert!(stand_ins(&built).contains(&("A table by a window at dusk, three candles lit", true)));
            assert!(built.stats.texts >= 8, "{:?}", built.stats);
            // Eight pictures survive the parser; one of them is `cid:` with nothing attached.
            let remote = lay_out(ctx, &corpus_message("remote"), 820.0);
            assert_eq!((remote.stats.placeholders, remote.stats.remote), (8, 7));
            assert!(stand_ins(&remote).contains(&("missing part", false)));
            // No picture was kept, because no bytes were ever looked at: there were none to look at.
            assert_eq!(Pictures::of(ctx).lock().unwrap().counts(), (0, 0));
            // A part that claims a web address as its Content-ID does not stand in for that address.
            let mut sneaky = corpus_message("newsletter");
            sneaky.attachments = vec![Attachment {
                content_id: "https://img.fernhollow.example/2026/10/hero-ridge-morning.jpg".into(),
                inline: true,
                ..Attachment::new("hero.png", "image/png", png(8, 8))
            }];
            let laid = lay_out(ctx, &sneaky, 820.0);
            assert_eq!((laid.stats.pictures, laid.stats.remote), (0, 8));
        });
    }
}

#[test]
fn pictures_of_the_message_are_shown_where_they_belong() {
    in_frame(false, |ctx| {
        let m = corpus_message("screenshots");
        let laid = lay_out(ctx, &m, 820.0);
        assert_eq!((laid.stats.pictures, laid.stats.placeholders, laid.stats.remote), (6, 2, 0));
        let pictures = shown_pictures(&laid);
        let sizes: Vec<(f32, f32)> = pictures.iter().map(|i| (i.rect.width(), i.rect.height())).collect();
        // The size asked for, or the picture's own; the logo at half its pixels, as asked.
        assert_eq!(sizes, [(520.0, 300.0), (640.0, 380.0), (16.0, 16.0), (16.0, 16.0), (16.0, 16.0), (56.0, 56.0)]);
        // A screenshot comes under the words before it and above the words after it.
        let (before, after) = (find(&laid, "The export dialog is still wrong"), find(&laid, "The Delimiter box"));
        assert!(before.rect.max.y <= pictures[0].rect.min.y && pictures[0].rect.max.y <= after.rect.min.y);
        // The three small ones stand in one line.
        assert!(pictures[2].rect.min.y == pictures[3].rect.min.y && pictures[3].rect.min.y == pictures[4].rect.min.y);
        assert!(pictures[2].rect.max.x < pictures[3].rect.min.x && pictures[3].rect.max.x < pictures[4].rect.min.x);
        // The logo stands beside the signature.
        let name = find(&laid, "Elias Brandt");
        assert!(pictures[5].rect.max.x <= name.rect.min.x && pictures[5].rect.min.y <= name.rect.min.y && name.rect.min.y < pictures[5].rect.max.y);
        // What is missing or damaged is a stand-in with its description, not an error.
        assert_eq!(stand_ins(&laid), [("Log excerpt", false), ("Damaged capture", false)]);
        // In a narrow pane a picture is scaled to the room, in shape.
        let narrow = lay_out(ctx, &m, 300.0);
        let sizes: Vec<(f32, f32)> = shown_pictures(&narrow).iter().map(|i| (i.rect.width(), i.rect.height().round())).collect();
        assert_eq!(sizes.first(), Some(&(300.0, 173.0)));
        assert_eq!(sizes.get(1), Some(&(300.0, 178.0)));
        assert!(shown_pictures(&narrow).iter().all(|i| i.rect.max.x <= 300.5));
        // The signature's logo in the Outlook reply, at the size it asks for.
        let outlook = lay_out(ctx, &corpus_message("outlook reply"), 820.0);
        assert_eq!(shown_pictures(&outlook).iter().map(|i| (i.rect.width(), i.rect.height())).collect::<Vec<_>>(), [(168.0, 48.0)]);
    });
}

#[test]
fn too_many_pictures_and_too_large_ones_are_stand_ins() {
    in_frame(false, |ctx| {
        // One small picture, referred to far more often than a message may show pictures.
        let html: String = (0..super::images::MAX_PICTURES_PER_MESSAGE + 50).map(|i| format!("<p>{i}</p><img src=\"cid:p@x\">")).collect();
        let mut m = message(0, ("A", "a@example.org"), "Many", Some(html), String::new());
        m.attachments = vec![Attachment { content_id: "p@x".into(), inline: true, ..Attachment::new("p.png", "image/png", png(40, 30)) }];
        let laid = lay_out(ctx, &m, 600.0);
        assert_eq!((laid.stats.pictures, laid.stats.placeholders), (super::images::MAX_PICTURES_PER_MESSAGE, 50));
        // A picture that asks for an absurd size is shown no wider than the room and no taller than the cap.
        let mut m = message(
            0,
            ("A", "a@example.org"),
            "Huge",
            Some("<img src=\"cid:p@x\" width=8000 height=8000><img src=\"cid:p@x\" width=4 height=8192>".into()),
            String::new(),
        );
        m.attachments = vec![Attachment { content_id: "p@x".into(), inline: true, ..Attachment::new("p.png", "image/png", png(40, 30)) }];
        let laid = lay_out(ctx, &m, 600.0);
        for picture in shown_pictures(&laid) {
            assert!(picture.rect.width() <= 600.5 && picture.rect.height() <= 6000.5 && picture.rect.width() >= 1.0, "{:?}", picture.rect);
        }
    });
}

#[test]
fn a_data_table_keeps_its_columns_and_a_layout_table_gives_way() {
    in_frame(false, |ctx| {
        let receipt = corpus_message("receipt");
        let wide = lay_out(ctx, &receipt, 820.0);
        // The amounts stand in one column, right-aligned, a row each.
        let amounts: Vec<&Item> = ["29.00", "83.20", "4.95", "79.83"].iter().map(|a| find_exact(&wide, a)).collect();
        assert!(amounts.windows(2).all(|w| (w[0].rect.max.x - w[1].rect.max.x).abs() < 0.5 && w[0].rect.max.y <= w[1].rect.min.y));
        // The table is as wide as its card (560, centred by the margins its div asks for: #150),
        // and its header row reads across.
        let (item, qty, total) = (find_exact(&wide, "Item"), find_exact(&wide, "Qty"), find_exact(&wide, "Total"));
        assert!(item.rect.min.x > 120.0 && item.rect.min.x < 160.0 && total.rect.max.x > 660.0 && total.rect.max.x <= 700.0);
        assert!(item.rect.min.y == qty.rect.min.y && qty.rect.min.y == total.rect.min.y && item.rect.max.x <= qty.rect.min.x);
        assert_eq!((wide.stats.stacked, wide.size.x), (0, 820.0));
        // Far too little room: it keeps its columns, and the message is wider than the pane (which scrolls).
        let narrow = lay_out(ctx, &receipt, 150.0);
        let (qty, price) = (find_exact(&narrow, "Qty"), find_exact(&narrow, "Price"));
        assert!(qty.rect.min.y == price.rect.min.y && qty.rect.max.x <= price.rect.min.x);
        assert!(narrow.size.x > 160.0 && narrow.stats.stacked == 0, "{:?}", narrow.size);
        // At every width each header word is whole and inside its own cell: a narrow column
        // neither breaks its word nor lets it run under its neighbour.
        let shade = Color32::from_rgb(0xf6, 0xf2, 0xe7);
        for width in [150.0, 200.0, 240.0, 284.0, 330.0, 400.0, 520.0, 640.0, 820.0] {
            let laid = lay_out(ctx, &receipt, width);
            let cells: Vec<egui::Rect> =
                laid.items.iter().filter(|i| matches!(i.kind, ItemKind::Fill { color, .. } if color == shade)).map(|i| i.rect).collect();
            assert_eq!(cells.len(), 4, "at {width}");
            for word in ["Item", "Qty", "Price", "Total"] {
                let item = find_exact(&laid, word);
                let ItemKind::Text(text) = &item.kind else { panic!() };
                let galley = ctx.fonts_mut(|f| f.layout_job((*text.job).clone()));
                assert_eq!(galley.rows.len(), 1, "{word} is broken over lines at {width}");
                let row = galley.rows.first().map(|r| r.rect()).unwrap();
                let (from, to) = (item.rect.min.x + text.anchor + row.min.x, item.rect.min.x + text.anchor + row.max.x);
                let cell = cells
                    .iter()
                    .find(|c| c.contains(egui::pos2((from + to) / 2.0, item.rect.center().y)))
                    .unwrap_or_else(|| panic!("{word} is in no cell at {width}"));
                assert!(cell.min.x <= from + 0.5 && to <= cell.max.x + 0.5, "{word} at {width}: {from}..{to} is not inside {cell:?}");
            }
        }

        let news = corpus_message("newsletter");
        let wide = lay_out(ctx, &news, 820.0);
        // The white column is 600 wide and in the middle of the page colour.
        let column = wide
            .items
            .iter()
            .find(|i| matches!(i.kind, ItemKind::Fill { color, .. } if color == Color32::WHITE) && (i.rect.width() - 600.0).abs() < 0.5);
        assert!(column.is_some_and(|c| (c.rect.center().x - 410.0).abs() < 1.0), "{column:?}");
        // The two products stand side by side there...
        let (a, b) = (find(&wide, "Merino crew"), find(&wide, "Ridge boot"));
        assert!((a.rect.min.y - b.rect.min.y).abs() < 0.5 && a.rect.max.x <= b.rect.min.x);
        assert_eq!((wide.stats.stacked, wide.size.x), (0, 820.0));
        // ...and one under the other where there is no room for two columns; nothing sticks out.
        let narrow = lay_out(ctx, &news, 200.0);
        let (a, b) = (find(&narrow, "Merino crew"), find(&narrow, "Ridge boot"));
        assert!(a.rect.max.y <= b.rect.min.y && narrow.stats.stacked >= 1);
        assert_eq!(narrow.size.x, 200.0);
        // The button is as wide as its words, and in the middle.
        let button = find(&wide, "See the members' weekend");
        assert!(button.rect.width() < 260.0 && (button.rect.center().x - 410.0).abs() < 1.0, "{:?}", button.rect);
    });
}

#[test]
fn quoted_text_has_a_bar_for_every_level() {
    in_frame(false, |ctx| {
        let gmail = lay_out(ctx, &corpus_message("gmail reply"), 700.0);
        for (start, level) in [("Hi Priya", 0), ("Morning Kofi", 1), ("Priya, here are my notes", 2), ("How did the call go?", 3)] {
            let item = find(&gmail, start);
            assert_eq!(bars_beside(&gmail, item), level, "{start}");
            assert!(item.rect.min.x >= level as f32 * 14.0, "{start}");
        }
        // The bars of one quote are one unbroken line each: a bar starts where the one above ends.
        let first = find(&gmail, "Morning Kofi");
        let last = find(&gmail, "How did the call go?");
        let outer: Vec<&Item> =
            gmail.items.iter().filter(|i| matches!(i.kind, ItemKind::Fill { .. }) && i.rect.width() == 2.0 && i.rect.min.x < 5.0).collect();
        assert!(outer.first().is_some_and(|b| b.rect.min.y <= first.rect.min.y) && outer.last().is_some_and(|b| b.rect.max.y >= last.rect.max.y));
        assert!(outer.windows(2).all(|w| (w[0].rect.max.y - w[1].rect.min.y).abs() < 0.5), "the outer bar is broken");
        // The same for a plain-text thread.
        let plain = lay_out(ctx, &corpus_message("plain thread"), 700.0);
        for (start, level) in [("Hi all", 0), ("Thanks Jorge", 1), ("rc2 is up", 2), ("On Fri, 9 Oct", 3), ("Agreed.", 0)] {
            assert_eq!(bars_beside(&plain, find(&plain, start)), level, "{start}");
        }
        // Quoting past what gets a bar still lays out, and stays inside the pane.
        let deep = lay_out(ctx, &corpus_message("deep quotes"), 300.0);
        assert!(deep.size.x <= 300.5);
    });
}

#[test]
fn a_long_message_is_laid_out_a_part_per_frame() {
    in_frame(false, |ctx| {
        let text: String = (0..2_000)
            .map(|i| format!("Paragraph {i} of a very long message, with enough words in it to fill a line or two of the pane.\n\n"))
            .collect();
        let m = message(0, ("A", "a@example.org"), "Long", None, text);
        let doc = Doc::of(&m);
        let pictures = Pictures::of(ctx);
        let mut pictures = pictures.lock().unwrap();
        let mut laid = Laid::new(700.0);
        let mut heights = Vec::new();
        while !laid.done {
            laid.advance(ctx, &doc.blocks, Some(&m), &mut pictures, FRAME_BUDGET);
            heights.push(laid.size.y);
            assert!(heights.len() < 1_000);
        }
        // Several frames, each adding to the bottom; the top is there after the first.
        assert!(heights.len() >= 3 && heights.windows(2).all(|w| w[0] < w[1]), "{heights:?}");
        assert!(heights[0] > 2_000.0);
        drop(pictures);
        // It comes to the same as laying it out in one go.
        let whole = lay_out(ctx, &m, 700.0);
        assert_eq!((laid.items.len(), laid.size, laid.stats), (whole.items.len(), whole.size, whole.stats));
        assert!(laid.items.iter().zip(&whole.items).all(|(a, b)| a.rect == b.rect));

        // One paragraph that is a whole message: cut into pieces, a few of them per frame.
        let huge = message(0, ("A", "a@example.org"), "Huge", None, "word ".repeat(120_000));
        let doc = Doc::of(&huge);
        assert!(doc.blocks.len() > 100 && doc.blocks.iter().all(|b| b.text().len() <= PIECE_CHARS * 2 + 8), "{} blocks", doc.blocks.len());
        assert_eq!(doc.blocks.iter().map(|b| b.text().split_whitespace().count()).sum::<usize>(), 120_000);
        let pictures = Pictures::of(ctx);
        let mut pictures = pictures.lock().unwrap();
        let mut laid = Laid::new(700.0);
        laid.advance(ctx, &doc.blocks, Some(&huge), &mut pictures, FRAME_BUDGET);
        assert!(!laid.done && laid.size.y > 1_000.0);
        // No piece of text kept for painting is larger than a piece.
        for item in &laid.items {
            assert!(text_of(item).len() <= PIECE_CHARS * 2 + 8);
        }
    });
}

#[test]
fn long_text_is_cut_into_pieces_at_white_space() {
    assert_eq!(pieces(&[Span::plain("short")]), [vec![Span::plain("short")]]);
    let cut = pieces(&[Span::plain("word ".repeat(3_000).trim_end())]);
    assert!(cut.len() >= 3);
    for piece in &cut {
        let text: String = piece.iter().map(|s| s.text.as_str()).collect();
        assert!(text.chars().count() <= PIECE_CHARS + 8 && !text.starts_with(' ') && !text.ends_with(' '), "{:?}", text.get(..40));
    }
    // Nothing but the white space at the cuts is lost.
    let words: usize = cut.iter().flatten().map(|s| s.text.split_whitespace().count()).sum();
    assert_eq!(words, 3_000);
    // Text with no white space is cut anyway, and none of it is lost.
    let cut = pieces(&[Span::plain("W".repeat(30_000))]);
    assert!(cut.len() >= 3 && cut.iter().flatten().map(|s| s.text.len()).sum::<usize>() == 30_000);
    // Formatting goes with the text, across cuts and across spans.
    let bold = Span { bold: true, ..Span::plain("b ".repeat(5_000)) };
    let cut = pieces(&[Span::plain("a "), bold, Span::linked("end", "https://example.org")]);
    assert!(cut.iter().flatten().filter(|s| s.bold).count() >= 2);
    assert!(cut.last().and_then(|p| p.last()).is_some_and(|s| s.link.is_some() && s.text == "end"));
    // Characters of every width.
    let cut = pieces(&[Span::plain("é ".repeat(10_000)), Span::plain("\u{1f980}".repeat(10_000))]);
    assert_eq!(cut.iter().flatten().map(|s| s.text.chars().filter(|c| !c.is_whitespace()).count()).sum::<usize>(), 20_000);
    assert!(pieces(&[]).iter().all(|p| p.is_empty()));
}

#[test]
fn a_link_is_under_the_pointer_only_where_its_text_is() {
    in_frame(false, |ctx| {
        let html = "<p>Some words and then <a href=\"https://example.org/there\">a link to follow</a> and more.</p>\
                    <p align=center><a href=\"mailto:ada@example.org\">centred</a></p><p align=right>no link <b>here</b></p>";
        let m = message(0, ("A", "a@example.org"), "Links", Some(html.to_string()), String::new());
        let laid = lay_out(ctx, &m, 600.0);
        for start in ["Some words", "centred", "no link"] {
            let item = find(&laid, start);
            let ItemKind::Text(text) = &item.kind else { panic!() };
            let galley = ctx.fonts_mut(|f| f.layout_job((*text.job).clone()));
            let mut index = 0;
            for row in &galley.rows {
                for (i, glyph) in row.row.glyphs.iter().enumerate() {
                    let point = egui::vec2(row.pos.x + glyph.pos.x + glyph.advance_width / 2.0, row.pos.y + row.row.size.y / 2.0);
                    let wanted = text.links.iter().find(|(range, _)| range.contains(&(index + i))).map(|(_, url)| url.clone());
                    assert_eq!(super::view::link_under(text, &galley, point), wanted, "{start}: character {}", index + i);
                }
                index += row.row.glyphs.len() + usize::from(row.ends_with_newline);
                // Beside the line and under the text there is nothing, however near a link is.
                let beside = egui::vec2(row.rect().max.x + 30.0, row.pos.y + row.row.size.y / 2.0);
                assert_eq!(super::view::link_under(text, &galley, beside), None, "{start}");
            }
            assert_eq!(super::view::link_under(text, &galley, egui::vec2(galley.rect.center().x, galley.rect.max.y + 40.0)), None);
        }
        let ItemKind::Text(text) = &find(&laid, "Some words").kind else { panic!() };
        assert_eq!(text.links.iter().map(|(r, url)| (r.clone(), &**url)).collect::<Vec<_>>(), [(20..36, "https://example.org/there")]);
    });
}

#[test]
fn html_that_shows_nothing_falls_back_to_the_text() {
    let text = |doc: &Doc| doc.blocks.iter().map(|b| b.text()).collect::<Vec<_>>().join("|");
    // Only a remote picture and a script: the text part says more.
    let hollow = message(
        0,
        ("A", "a@example.org"),
        "x",
        Some("<img src=\"https://example.org/a.png\"><script>alert(1)</script>".into()),
        "The text part.".into(),
    );
    let doc = Doc::of(&hollow);
    assert_eq!((text(&doc).as_str(), doc.remote_pictures), ("The text part.", 0));
    // Unless the picture can be had for the asking: then the message is its pictures (a
    // newsletter that is all slices of one picture), shown as stand-ins with the offer above.
    let doc = Doc::of_where(&hollow, true);
    assert_eq!((text(&doc).as_str(), doc.remote_pictures, doc.remote.len()), ("", 1, 1));
    // Not for the sake of a pixel, of what is hidden, or of what is no address on the web.
    for html in [
        "<img src=\"https://example.org/o.gif\" width=1 height=1>",
        "<div style=\"display:none\"><img src=\"https://example.org/a.png\"></div>",
        "<img src=\"cid:never-attached\"><img src=\"/a.png\"><script>alert(1)</script>",
    ] {
        let hollow = message(0, ("A", "a@example.org"), "x", Some(html.into()), "The text part.".into());
        assert_eq!(text(&Doc::of_where(&hollow, true)), "The text part.", "{html}");
    }
    // A picture of the message's own is worth showing.
    let mut own = message(0, ("A", "a@example.org"), "x", Some("<img src=\"cid:a@x\">".into()), "The text part.".into());
    own.attachments = vec![Attachment { content_id: "a@x".into(), inline: true, ..Attachment::new("a.png", "image/png", png(8, 8)) }];
    assert!(matches!(Doc::of(&own).blocks.first().map(|b| &b.kind), Some(mailcraft_html::BlockKind::Image { .. })));
    // With no text part, the HTML is all there is.
    let bare = message(0, ("A", "a@example.org"), "x", Some("<img src=\"https://example.org/a.png\" alt=\"A\">".into()), String::new());
    assert_eq!(Doc::of(&bare).remote_pictures, 1);
    // No HTML: the text, with its addresses made links.
    let plain = message(0, ("A", "a@example.org"), "x", None, "See https://example.org\n\n> quoted".into());
    let doc = Doc::of(&plain);
    assert_eq!((doc.blocks.len(), doc.blocks.last().map(|b| b.quote)), (2, Some(1)));
    assert_eq!(super::blocks_of(&plain), doc.blocks);
    assert!(Doc::of(&Message::default()).blocks.is_empty());
}

#[test]
fn a_body_is_parsed_once_and_again_when_it_changes() {
    in_frame(false, |ctx| {
        let m = Arc::new(Message { id: 7, ..corpus_message("gmail reply") });
        let first = super::view::doc(ctx, &m, Place::Pane);
        assert!(Arc::ptr_eq(&first, &super::view::doc(ctx, &m, Place::Pane)));
        // Flagging a message makes a new message of it; its body is the same one.
        let flagged = Arc::new(Message { flag: mailcraft_model::FlagStatus::Flagged, read: false, ..(*m).clone() });
        assert!(Arc::ptr_eq(&first, &super::view::doc(ctx, &flagged, Place::Pane)));
        // A changed body is parsed again, and what was kept of the old one is dropped.
        let mut edited = (*m).clone();
        edited.body.html = Some("<p>Changed</p>".into());
        let second = super::view::doc(ctx, &Arc::new(edited), Place::Pane);
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!((second.blocks.len(), super::view::kept_count(ctx)), (1, 1));
        // So is one whose attached bytes changed, though it reads the same.
        let mut m = corpus_message("screenshots");
        m.id = 8;
        let before = super::view::doc(ctx, &Arc::new(m.clone()), Place::Pane);
        m.attachments[0].data = Arc::new(png(4, 4));
        assert!(!Arc::ptr_eq(&before, &super::view::doc(ctx, &Arc::new(m), Place::Pane)));
        // Only the last few messages read are kept.
        for id in 100..140 {
            super::view::doc(ctx, &Arc::new(Message { id, ..(*flagged).clone() }), Place::Pane);
        }
        assert!(super::view::kept_count(ctx) <= 4);
    });
}

#[test]
fn the_corpus_shows_in_the_window_in_both_themes_and_sizes() {
    for theme in ["light", "dark"] {
        for size in [(1280.0, 800.0), (380.0, 320.0)] {
            let (mut app, ids) = corpus_app(false);
            app.execute("ui.theme", json!({"value": theme})).unwrap();
            let mut harness = window(app, size);
            for (name, id) in &ids {
                harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
                harness.run_steps(3);
                let shown =
                    super::view::kept(&harness.ctx, *id, |laid, _| (laid.items.len(), laid.size)).unwrap_or_else(|| panic!("{name} was not shown"));
                assert!(shown.0 > 0 && shown.1.y > 0.0, "{theme} {size:?} {name}: {shown:?}");
            }
            assert!(super::view::kept_count(&harness.ctx) <= 4);
        }
    }
}

#[test]
fn pictures_are_decoded_a_frame_at_a_time_and_only_for_the_message_shown() {
    let (app, ids) = corpus_app(false);
    let mut harness = window(app, (1280.0, 1700.0));
    let counts = |harness: &egui_kittest::Harness<'static, MailApp>| Pictures::of(&harness.ctx).lock().unwrap().counts();
    assert_eq!(counts(&harness), (0, 0));
    // A newsletter of remote pictures: nothing to decode, nothing kept.
    harness.state_mut().execute("mail.select", json!({"id": id_of(&ids, "newsletter")})).unwrap();
    harness.run_steps(6);
    assert_eq!(counts(&harness), (0, 0));
    // The screenshots: six attached parts are looked at (one is damaged), five become textures, one in each frame.
    harness.state_mut().execute("mail.select", json!({"id": id_of(&ids, "screenshots")})).unwrap();
    harness.run_steps(2);
    let early = counts(&harness);
    assert!(early.0 == 6 && (1..5).contains(&early.1), "{early:?}");
    harness.run_steps(8);
    assert_eq!(counts(&harness), (6, 5));
    // Going back and forth decodes nothing again.
    harness.state_mut().execute("mail.select", json!({"id": id_of(&ids, "outlook reply")})).unwrap();
    harness.run_steps(4);
    harness.state_mut().execute("mail.select", json!({"id": id_of(&ids, "screenshots")})).unwrap();
    harness.run_steps(4);
    assert_eq!(counts(&harness), (7, 6));
}

#[test]
fn a_link_shows_the_hand_and_opens_when_clicked_and_only_then() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "gmail reply");
    let mut harness = window(app, (1280.0, 800.0));
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(4);
    // Where the first character of the first link is on the screen, and a place in plain text.
    let ctx = harness.ctx.clone();
    let (on_link, url, off_link) = super::view::kept(&ctx, id, |laid, origin| {
        let item = laid.items.iter().find(|i| matches!(&i.kind, ItemKind::Text(t) if !t.links.is_empty())).unwrap();
        let ItemKind::Text(text) = &item.kind else { panic!() };
        let galley = ctx.fonts_mut(|f| f.layout_job((*text.job).clone()));
        let (range, url) = text.links.first().unwrap();
        let (mut index, mut at) = (0, None);
        for row in &galley.rows {
            if let Some(glyph) = range.start.checked_sub(index).and_then(|i| row.row.glyphs.get(i + 1)) {
                at = Some(
                    origin + item.rect.min.to_vec2() + egui::vec2(text.anchor + row.pos.x + glyph.pos.x + 1.0, row.pos.y + row.row.size.y / 2.0),
                );
                break;
            }
            index += row.row.glyphs.len() + usize::from(row.ends_with_newline);
        }
        let plain = find(laid, "Hi Priya");
        (at.unwrap(), url.to_string(), origin + plain.rect.min.to_vec2() + egui::vec2(4.0, 6.0))
    })
    .unwrap();
    assert_eq!(url, "https://docs.juniperlabs.example/migration/plan");
    let opened = |harness: &egui_kittest::Harness<'static, MailApp>| -> Vec<String> {
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            })
            .collect()
    };
    // Over plain text: an ordinary pointer, and a click opens nothing.
    harness.event(egui::Event::PointerMoved(off_link));
    harness.step();
    assert_ne!(harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos: off_link, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
        assert!(opened(&harness).is_empty());
    }
    // Over the link: the hand, and nothing opens until it is clicked.
    harness.event(egui::Event::PointerMoved(on_link));
    harness.step();
    harness.step();
    assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    assert!(opened(&harness).is_empty());
    let mut urls = Vec::new();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos: on_link, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
        urls.extend(opened(&harness));
    }
    assert_eq!(urls, [url]);
}

#[test]
fn the_theme_changes_the_colours_of_a_message_that_is_open() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "outlook reply");
    let mut harness = window(app, (1280.0, 800.0));
    let ink = |harness: &egui_kittest::Harness<'static, MailApp>, start: &str| {
        super::view::kept(&harness.ctx, id, |laid, _| match &find(laid, start).kind {
            ItemKind::Text(text) => text.job.sections.first().map(|s| s.format.color),
            _ => None,
        })
        .flatten()
        .unwrap()
    };
    harness.state_mut().execute("ui.theme", json!({"value": "light"})).unwrap();
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(4);
    let (plain, blue) = (ink(&harness, "Hi Owen"), ink(&harness, "Revised schedule attached"));
    assert_eq!((plain, blue), (Tokens::light().text, Color32::from_rgb(0x1f, 0x49, 0x7d)));
    harness.state_mut().execute("ui.theme", json!({"value": "dark"})).unwrap();
    harness.run_steps(4);
    let (plain, blue) = (ink(&harness, "Hi Owen"), ink(&harness, "Revised schedule attached"));
    assert_eq!(plain, Tokens::dark().text);
    // Outlook's dark blue, turned light for the dark window: still blue, and readable.
    assert!(blue.b() > blue.r() && contrast(blue, Tokens::dark().surface) >= super::colors::TARGET_CONTRAST, "{blue:?}");
}

/// Generous bounds: this is an unoptimised build of this crate, on whatever machine runs the
/// tests. What it takes is printed (`--nocapture`).
#[test]
fn a_two_megabyte_newsletter_opens_without_a_stall() {
    let m = large_newsletter();
    let bytes = m.body.html.as_ref().map_or(0, String::len);
    assert!(bytes >= mailcraft_html::MAX_INPUT);
    in_frame(false, |ctx| {
        let started = std::time::Instant::now();
        let doc = Doc::of(&m);
        let parsed = started.elapsed();
        let pictures = Pictures::of(ctx);
        let mut pictures = pictures.lock().unwrap();
        let started = std::time::Instant::now();
        let mut laid = Laid::new(820.0);
        laid.advance(ctx, &doc.blocks, Some(&m), &mut pictures, usize::MAX);
        let laid_out = started.elapsed();
        eprintln!(
            "large newsletter: {bytes} bytes: parsed in {parsed:?}, laid out in {laid_out:?} ({} items, {} texts, {} tables, {:.0} points tall)",
            laid.items.len(),
            laid.stats.texts,
            laid.stats.tables,
            laid.size.y
        );
        assert!(laid.done && laid.stats.texts > 1_000 && laid.stats.remote > 200);
        assert!(parsed.as_secs() < 10 && laid_out.as_secs() < 20, "parsed in {parsed:?}, laid out in {laid_out:?}");
    });
    // In the window: opening it takes a few frames, and a frame once it is open costs what is in view.
    let (app, ids) = corpus_app(true);
    let id = id_of(&ids, "large newsletter");
    let mut harness = window(app, (1280.0, 800.0));
    let started = std::time::Instant::now();
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    let mut frames = 0;
    while super::view::kept(&harness.ctx, id, |laid, _| laid.done) != Some(true) {
        harness.step();
        frames += 1;
        assert!(frames < 200, "the layout never finished");
    }
    let opened = started.elapsed();
    let started = std::time::Instant::now();
    harness.run_steps(30);
    let frame = started.elapsed() / 30;
    eprintln!("large newsletter in the window: open after {frames} frames and {opened:?}; then {frame:?} a frame");
    assert!(opened.as_secs() < 30 && frame.as_millis() < 500, "open after {opened:?}, then {frame:?} a frame");
}

/// Nothing here can fetch anything: the reading code names no socket, no HTTP client, no file
/// and none of egui's ways of loading a picture from an address. A picture from the web comes
/// through the host's service (`Services::pictures`) or not at all.
#[test]
fn the_reading_pane_names_nothing_that_could_fetch() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.join("reading.rs")];
    files.extend(std::fs::read_dir(src.join("reading")).unwrap().map(|e| e.unwrap().path()));
    let mut checked = 0;
    for path in files {
        // This file names what it forbids (and its own parts are tests like it).
        if path.ends_with("reading/tests.rs") || path.is_dir() {
            continue;
        }
        let code = std::fs::read_to_string(&path).unwrap();
        for forbidden in [
            "std::net",
            "TcpStream",
            "UdpSocket",
            "reqwest",
            "ureq",
            "ehttp",
            "hyper::",
            "web_sys",
            "XmlHttpRequest",
            "fetch(",
            "std::process",
            "egui_extras",
            "install_image_loaders",
            "ImageSource",
            "Image::from_uri",
            "Image::new(",
            "ui.image(",
            "include_image",
            "try_load_",
            "std::fs",
            "File::",
        ] {
            assert!(!code.contains(forbidden), "{} names {forbidden}", path.display());
        }
        checked += 1;
    }
    assert_eq!(checked, 11, "reading.rs, colors, images (and its tests), invitation, layout, remote, select, sharing (and its tests) and view");
}

/// The symbols these files write as text have glyphs in the interface fonts (the check of the
/// whole interface, in `theme.rs`, reads only the files directly under `src`).
#[test]
fn symbols_written_here_have_glyphs() {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::font_definitions());
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("reading");
    let font = crate::theme::regular(12.5);
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.ends_with("tests.rs") || path.is_dir() {
            continue;
        }
        let code = std::fs::read_to_string(&path).unwrap();
        for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
            for (i, part) in line.split('"').enumerate() {
                if i % 2 == 1 {
                    for c in part.chars().filter(|c| !c.is_ascii() && !c.is_alphabetic() && !c.is_whitespace()) {
                        assert!(ctx.fonts_mut(|f| f.has_glyph(&font, c)), "{c} U+{:04X} in {} has no glyph", c as u32, path.display());
                    }
                }
            }
        }
    }
}

// ---- pictures from the web ----

/// A stand-in for the host that downloads pictures: it notes every address it is asked for,
/// and answers at once, or holds its answers until the test lets them go.
#[derive(Clone, Default)]
struct Host {
    asked: Arc<Mutex<Vec<String>>>,
    held: Arc<Mutex<Vec<mpsc::Sender<Result<Vec<u8>, String>>>>>,
}

impl Host {
    /// A host that answers every address at once with what `answer` makes of it.
    fn answering(app: &mut MailApp, answer: impl Fn(&str) -> Result<Vec<u8>, String> + 'static) -> Host {
        let host = Host::default();
        let asked = host.asked.clone();
        app.services.pictures = Some(Box::new(move |url: String| {
            let (tx, rx) = mpsc::channel();
            tx.send(answer(&url)).unwrap();
            asked.lock().unwrap().push(url);
            rx
        }));
        host
    }

    /// A host that answers when the test says so ([`Host::release`]).
    fn holding(app: &mut MailApp) -> Host {
        let host = Host::default();
        let (asked, held) = (host.asked.clone(), host.held.clone());
        app.services.pictures = Some(Box::new(move |url: String| {
            let (tx, rx) = mpsc::channel();
            asked.lock().unwrap().push(url);
            held.lock().unwrap().push(tx);
            rx
        }));
        host
    }

    /// Every address it was asked for, in order.
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }

    /// Answer the `n` requests that have waited longest.
    fn release(&self, n: usize, answer: impl Fn() -> Result<Vec<u8>, String>) {
        let mut held = self.held.lock().unwrap();
        let n = n.min(held.len());
        for waiting in held.drain(..n) {
            let _ = waiting.send(answer());
        }
    }

    /// Stop answering: the requests that wait are dropped.
    fn leave(&self) {
        self.held.lock().unwrap().clear();
    }
}

/// The pictures of the corpus's newsletter, in the order it shows them.
const NEWSLETTER_PICTURES: [&str; 8] = [
    "https://img.fernhollow.example/brand/wordmark-light@2x.png",
    "https://img.fernhollow.example/2026/10/hero-ridge-morning.jpg",
    "https://img.fernhollow.example/2026/10/p-4471-merino-crew.jpg",
    "https://img.fernhollow.example/2026/10/p-5120-ridge-boot.jpg",
    "https://img.fernhollow.example/icons/hut-72.png",
    "https://img.fernhollow.example/icons/soc-photos-32.png",
    "https://img.fernhollow.example/icons/soc-video-32.png",
    "https://img.fernhollow.example/icons/soc-journal-32.png",
];

/// A drawing for each of them, the size the newsletter says it is.
fn newsletter_picture(url: &str) -> Result<Vec<u8>, String> {
    let (w, h) = match url {
        u if u.contains("wordmark") => (220, 44),
        u if u.contains("hero") => (600, 280),
        u if u.contains("/p-") => (260, 260),
        u if u.contains("hut") => (72, 72),
        _ => (32, 32),
    };
    Ok(if url.ends_with(".jpg") { jpeg(w, h) } else { png(w, h) })
}

/// A message whose body is these pictures on the web, one under the other.
fn web_message(n: usize, from: &str, subject: &str, pictures: &[String]) -> Message {
    let html: String = pictures.iter().map(|src| format!("<p>Picture:</p><img src=\"{src}\" width=40 height=30 alt=\"A picture\">")).collect();
    message(n, ("A Sender", from), subject, Some(html), String::new())
}

fn web_pictures(host: &str, count: usize) -> Vec<String> {
    (0..count).map(|i| format!("https://{host}/p/{i}.png")).collect()
}

/// A window on a mailbox that holds `messages`, and their ids.
fn app_with(messages: Vec<Message>) -> (MailApp, Vec<u64>) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    served_by_example_org(&mut mailbox);
    let inbox = mailbox.inbox().unwrap();
    let ids = messages.into_iter().map(|m| mailbox.add_message(Message { folder: inbox, ..m }).unwrap()).collect();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    (app, ids)
}

type Window = egui_kittest::Harness<'static, MailApp>;

fn click_at(harness: &mut Window, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(2);
}

fn select(harness: &mut Window, id: u64) {
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(6);
}

/// What the reading pane shows of a message: pictures, stand-ins, and stand-ins for pictures on the web.
fn shown(harness: &Window, id: u64) -> (usize, usize, usize) {
    super::view::kept(&harness.ctx, id, |laid, _| (laid.stats.pictures, laid.stats.placeholders, laid.stats.remote)).unwrap()
}

/// Whether the window says something that has `text` in it.
fn says(harness: &Window, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

#[test]
fn pictures_on_the_web_are_placeholders_until_the_owner_asks() {
    for theme in ["light", "dark"] {
        let (mut app, ids) = corpus_app(false);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        let host = Host::answering(&mut app, newsletter_picture);
        let id = id_of(&ids, "newsletter");
        let mut harness = window(app, (1280.0, 1000.0));
        select(&mut harness, id);
        // Opened, read, scrolled: nothing was asked for. Eight stand-ins, and a line that says why.
        assert!(host.asked().is_empty());
        assert_eq!(shown(&harness, id), (0, 8, 8));
        assert!(says(&harness, "Pictures not downloaded."));
        assert!(says(&harness, "Always download pictures from hello@news.fernhollow.example"));
        let download = harness.state().hit.rect("downloadPictures", id).expect("no way to ask for the pictures");
        assert!(harness.state().hit.rect("alwaysDownloadPictures", id).is_some());

        // Asked: each picture of the message once, in its order, and they are shown where they belong.
        click_at(&mut harness, download.center());
        harness.run_steps(14);
        assert_eq!(host.asked(), NEWSLETTER_PICTURES);
        assert_eq!(shown(&harness, id), (8, 0, 0));
        assert!(!says(&harness, "Pictures not downloaded") && !says(&harness, "couldn't be downloaded"));
        assert!(harness.state().hit.rect("downloadPictures", id).is_none());
        // At the sizes the message gives them, or in that shape where the pane is narrower.
        let sizes =
            super::view::kept(&harness.ctx, id, |laid, _| shown_pictures(laid).iter().map(|i| (i.rect.width(), i.rect.height())).collect::<Vec<_>>());
        let sizes = sizes.unwrap();
        assert_eq!(sizes[0], (220.0, 44.0));
        assert!(sizes[1].0 > 300.0 && sizes[1].0 <= 600.0 && (sizes[1].0 / sizes[1].1 - 600.0 / 280.0).abs() < 0.01, "{:?}", sizes[1]);
        // They are decoded like any picture of a message, and kept by their address.
        let pictures = Pictures::of(&harness.ctx);
        assert_eq!(pictures.lock().unwrap().remote_counts().0, 8);
        assert!(pictures.lock().unwrap().counts().1 >= 2, "{:?}", pictures.lock().unwrap().counts());

        // It was asked for this message: the next one with pictures is not downloaded with it,
        // and coming back asks for nothing again.
        select(&mut harness, id_of(&ids, "remote"));
        assert_eq!(shown(&harness, id_of(&ids, "remote")), (0, 8, 7));
        select(&mut harness, id);
        harness.run_steps(10);
        assert_eq!(shown(&harness, id), (8, 0, 0));
        assert_eq!(host.asked().len(), 8);
    }
}

#[test]
fn nothing_is_asked_for_without_the_owners_say_so() {
    // Every message of the corpus, the hostile ones too, in the pane and in a window of its own,
    // in both themes, scrolled: the host never hears a thing.
    for theme in ["light", "dark"] {
        let (mut app, ids) = corpus_app(false);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        let host = Host::answering(&mut app, |_| Ok(png(8, 8)));
        let mut harness = window(app, (1100.0, 700.0));
        for (name, id) in &ids {
            select(&mut harness, *id);
            harness.state_mut().execute("ui.openWindow", json!({"message": id})).unwrap();
            harness.event(egui::Event::PointerMoved(egui::pos2(900.0, 500.0)));
            harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -400.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
                source: egui::MouseWheelSource::Unknown,
            });
            harness.run_steps(4);
            harness.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();
            assert!(host.asked().is_empty(), "{theme} {name}: {:?}", host.asked());
        }
        // Commands that are about something else don't count as asking either.
        let news = id_of(&ids, "newsletter");
        select(&mut harness, news);
        for (command, params) in [("mail.flag", json!({})), ("mail.reply", json!({})), ("compose.close", json!({})), ("mail.markUnread", json!({}))] {
            harness.state_mut().execute(command, params).unwrap();
            harness.run_steps(3);
        }
        assert!(host.asked().is_empty());
        assert_eq!(harness.state().remote_pictures.flying(), 0);
    }
}

#[test]
fn a_remembered_sender_and_the_switch_for_all_mail() {
    let shop = "news@shop.example";
    let (mut app, ids) = app_with(vec![
        web_message(0, shop, "First", &web_pictures("a.shop.example", 2)),
        web_message(1, shop, "Second", &web_pictures("b.shop.example", 2)),
        web_message(2, "other@elsewhere.example", "From somebody else", &web_pictures("c.elsewhere.example", 2)),
        web_message(3, shop, "In the junk", &web_pictures("d.shop.example", 2)),
        web_message(4, "NEWS@Shop.Example", "Third, written otherwise", &web_pictures("e.shop.example", 2)),
    ]);
    let (first, second, other, junk, third) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    app.execute("mail.junk", json!({"ids": [junk]})).unwrap();
    let host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    let mut harness = window(app, (1280.0, 800.0));
    let from = |host: &Host, part: &str| host.asked().iter().filter(|url| url.contains(part)).count();

    // "Always download pictures from": the sender is remembered, and this message's pictures come.
    select(&mut harness, first);
    assert!(host.asked().is_empty());
    let always = harness.state().hit.rect("alwaysDownloadPictures", first).unwrap();
    click_at(&mut harness, always.center());
    harness.run_steps(8);
    assert_eq!(harness.state().ui.picture_senders, [shop]);
    assert_eq!((from(&host, "a.shop"), shown(&harness, first)), (2, (2, 0, 0)));
    // So do the next message's from that sender, however the address is written, without asking...
    select(&mut harness, second);
    select(&mut harness, third);
    assert_eq!((from(&host, "b.shop"), from(&host, "e.shop"), shown(&harness, third)), (2, 2, (2, 0, 0)));
    // ...but nobody else's, and nothing that landed in Junk E-mail.
    select(&mut harness, other);
    select(&mut harness, junk);
    assert_eq!((from(&host, "c.elsewhere"), from(&host, "d.shop"), shown(&harness, junk)), (0, 0, (0, 2, 2)));
    // There the owner can still ask for the one message (and is not offered to trust its sender).
    assert!(harness.state().hit.rect("alwaysDownloadPictures", junk).is_none());
    let download = harness.state().hit.rect("downloadPictures", junk).unwrap();
    click_at(&mut harness, download.center());
    harness.run_steps(8);
    assert_eq!((from(&host, "d.shop"), shown(&harness, junk)), (2, (2, 0, 0)));

    // It is kept with the settings, and comes back with them.
    let saved = serde_json::to_value(harness.state().prefs()).unwrap();
    assert_eq!((&saved["pictureSenders"], &saved["downloadPictures"]), (&json!([shop]), &json!(false)));
    let restored: crate::UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.picture_senders, [shop]);
    // Settings from before there was such a thing say nothing, which is "ask me".
    let old: crate::UiState = serde_json::from_value(json!({"tab": "Home"})).unwrap();
    assert!(!old.download_pictures && old.picture_senders.is_empty());

    // File › Options lists the sender, and takes it off again: its pictures are stand-ins once more.
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    assert!(says(&harness, shop) && says(&harness, "Download pictures in mail automatically"));
    harness.state_mut().execute("ui.alwaysDownloadPictures", json!({"sender": " News@SHOP.example ", "value": false})).unwrap();
    harness.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    assert!(harness.state().ui.picture_senders.is_empty());
    let before = host.asked().len();
    select(&mut harness, second);
    assert_eq!((shown(&harness, second), host.asked().len()), ((0, 2, 2), before));
    assert!(says(&harness, "Pictures not downloaded"));

    // The switch for all mail, off until the owner turns it on: then everybody's, but not Junk's.
    assert!(!harness.state().ui.download_pictures);
    assert_eq!(harness.state_mut().execute("ui.downloadPicturesAutomatically", json!({})).unwrap()["value"], true);
    select(&mut harness, other);
    assert_eq!((from(&host, "c.elsewhere"), shown(&harness, other)), (2, (2, 0, 0)));
    let mut later = harness.state().session.mailbox.clone();
    let more = web_message(9, shop, "More junk", &web_pictures("f.shop.example", 2));
    let more = later.add_message(Message { folder: later.message(junk).unwrap().folder, ..more }).unwrap();
    harness.state_mut().session.mailbox = later;
    harness.state_mut().session.mailbox_changed();
    select(&mut harness, more);
    assert_eq!((from(&host, "f.shop"), shown(&harness, more)), (0, (0, 2, 2)));
    // And off again: what is opened from then on waits to be asked.
    harness.state_mut().execute("ui.downloadPicturesAutomatically", json!({"value": false})).unwrap();
    select(&mut harness, other);
    assert_eq!(shown(&harness, other), (0, 2, 2));
}

/// (A8-9) A long display name is cut short, never the address after it.
#[test]
fn a_long_name_does_not_squeeze_the_address_out_of_the_header() {
    let long = "Support Desk ".repeat(9);
    let mut near = web_message(0, "a@evil.example", "Short", &[]);
    near.from = mailcraft_model::Address::new("Support", "a@evil.example");
    let mut far = web_message(1, "a@evil.example", "Long", &[]);
    far.from = mailcraft_model::Address::new(&long, "a@evil.example");
    let (app, ids) = app_with(vec![near, far]);
    let mut harness = window(app, (1280.0, 800.0));
    let width = |harness: &Window| harness.query_by_label("<a@evil.example>").map(|node| node.rect().width());
    select(&mut harness, ids[0]);
    let whole = width(&harness).expect("the address is shown");
    select(&mut harness, ids[1]);
    let squeezed = width(&harness).expect("the address is shown");
    assert!((whole - squeezed).abs() < 1.0, "{whole} against {squeezed}: the address was cut");
}

/// Without the receiving server's word, a remembered sender or domain is not enough.
#[test]
fn a_remembered_sender_needs_the_servers_word() {
    let shop = "news@shop.example";
    let verdict = |value: &str| vec![("Authentication-Results".to_string(), value.to_string())];
    let mut plain = web_message(0, shop, "No verdict", &web_pictures("a.shop.example", 1));
    plain.headers.clear();
    let mut failed = web_message(1, shop, "Failed", &web_pictures("b.shop.example", 1));
    failed.headers = verdict("mx.example.org; dmarc=fail header.from=shop.example");
    // The server's line is on top; one lower down is the sender's own.
    failed.headers.push(("Authentication-Results".into(), "forged.example; dmarc=pass header.from=shop.example".into()));
    let mut wrong = web_message(2, shop, "Another domain", &web_pictures("c.shop.example", 1));
    wrong.headers = verdict("mx.example.org; dmarc=pass header.from=elsewhere.example");
    let good = web_message(3, shop, "Vouched", &web_pictures("d.shop.example", 1));
    let (mut app, ids) = app_with(vec![plain, failed, wrong, good]);
    app.ui.picture_senders = vec![shop.into()];
    app.ui.picture_domains = vec!["shop.example".into()];
    let host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    let mut harness = window(app, (1280.0, 800.0));
    for id in &ids[..3] {
        select(&mut harness, *id);
        assert_eq!(shown(&harness, *id), (0, 1, 1));
        assert!(harness.state().hit.rect("alwaysDownloadPictures", *id).is_none());
        // One message at a time is still the owner's to ask for.
        assert!(harness.state().hit.rect("downloadPictures", *id).is_some());
    }
    assert!(host.asked().is_empty(), "{:?}", host.asked());
    assert!(says(&harness, "the mail server didn't confirm"));
    select(&mut harness, ids[3]);
    assert_eq!((host.asked().len(), shown(&harness, ids[3])), (1, (1, 0, 0)));
}

/// The Kelvin sign is not a "k": a lookalike address is nobody the owner remembered.
#[test]
fn a_lookalike_address_is_not_a_remembered_sender() {
    let ui = crate::UiState { picture_senders: vec!["kim@kite.example".into()], picture_domains: vec!["kite.example".into()], ..Default::default() };
    assert!(super::remote::sender_remembered(&ui, "KIM@Kite.Example"));
    assert!(super::remote::domain_remembered(&ui, "anyone@kite.example"));
    for lookalike in ["\u{212A}im@kite.example", "kim@\u{212A}ite.example", "kim@kite.exampl\u{0435}"] {
        assert!(!super::remote::sender_remembered(&ui, lookalike), "{lookalike}");
        assert!(!super::remote::domain_remembered(&ui, lookalike), "{lookalike}");
    }
    // Nor can one be added.
    let (mut app, _) = app_with(vec![]);
    assert!(app.execute("ui.alwaysDownloadPictures", json!({"sender": "\u{212A}im@kite.example"})).is_err());
    assert!(app.execute("ui.alwaysDownloadPictures", json!({"domain": "\u{212A}ite.example"})).is_err());
    // An entry from before can still be taken off.
    app.ui.picture_senders = vec!["\u{212A}im@kite.example".into()];
    app.execute("ui.alwaysDownloadPictures", json!({"sender": "\u{212A}im@kite.example", "value": false})).unwrap();
    assert!(app.ui.picture_senders.is_empty());
}

/// Mail under Junk E-mail is Junk's, however deep, for the global switch and the lists alike.
#[test]
fn mail_in_a_subfolder_of_junk_is_junk_for_pictures() {
    let shop = "news@shop.example";
    let (mut app, ids) = app_with(vec![web_message(0, shop, "Sorted out", &web_pictures("a.shop.example", 1))]);
    let id = ids[0];
    let account = app.session.mailbox.accounts[0].id;
    let junk = app.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Junk).unwrap();
    let sub = app.session.mailbox.add_folder(account, Some(junk), "Sorted").unwrap();
    let deeper = app.session.mailbox.add_folder(account, Some(sub), "Older").unwrap();
    app.session.mailbox.message_mut(id).unwrap().folder = deeper;
    app.ui.picture_senders = vec![shop.into()];
    app.ui.download_pictures = true;
    let host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    let mut harness = window(app, (1280.0, 800.0));
    select(&mut harness, id);
    assert_eq!(shown(&harness, id), (0, 1, 1));
    assert!(host.asked().is_empty());
}

/// A draft opened again has lost the message it quotes: its pictures are not fetched by the switch.
#[test]
fn the_quote_of_a_reopened_draft_fetches_nothing() {
    let html = "<html><body><p>Mine</p><div id=\"mailcraft-quote\"><p>Theirs</p><img src=\"https://x.example/p.png\" width=40 height=30></div></body></html>";
    let mut draft = web_message(0, "avery@lumen-works.example", "Re: Offer", &[]);
    draft.draft = true;
    draft.body.html = Some(html.into());
    draft.body.text = "Mine\n\n-----Original Message-----\nTheirs".into();
    let (mut app, ids) = app_with(vec![draft]);
    app.ui.download_pictures = true;
    let host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    app.execute("mail.editDraft", json!({"id": ids[0]})).unwrap();
    assert!(app.session.compose.as_ref().is_some_and(|c| c.quote.is_some() && c.source.is_none()));
    let mut harness = window(app, (1280.0, 900.0));
    harness.run_steps(10);
    let quote = harness.state().session.compose.as_ref().and_then(|c| c.quote.clone()).unwrap();
    assert!(quote.html.contains("x.example/p.png"), "{}", quote.html);
    let laid = super::view::kept_at(&harness.ctx, Place::Quote, 0, |laid, _| (laid.done, laid.stats.remote));
    assert_eq!(laid, Some((true, 1)), "the quote is on screen");
    assert!(host.asked().is_empty(), "{:?}", host.asked());
}

/// The control for the test above: the quote of a reply to a message that is known does fetch.
#[test]
fn the_quote_of_a_reply_fetches_when_the_switch_is_on() {
    let (mut app, ids) = app_with(vec![web_message(0, "news@shop.example", "Offer", &web_pictures("a.shop.example", 1))]);
    app.ui.download_pictures = true;
    let host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    app.execute("mail.reply", json!({"id": ids[0]})).unwrap();
    let mut harness = window(app, (1280.0, 900.0));
    harness.run_steps(10);
    assert_eq!(host.asked().len(), 1);
}

#[test]
fn only_what_is_shown_as_a_picture_is_asked_for() {
    // The message that tries every way to phone home, with the owner's say-so for all mail:
    // its pictures are asked for, each once. Not its backgrounds, style sheets, fonts, scripts,
    // frames, sounds or forms, not the picture that says it is one pixel, not the one that is hidden.
    let (mut app, ids) = corpus_app(false);
    let host = Host::answering(&mut app, |_| Ok(png(8, 8)));
    app.execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id_of(&ids, "remote"));
    harness.run_steps(10);
    let asked = host.asked();
    assert_eq!(
        asked,
        [
            "https://evil.example/plain.gif",
            "http://evil.example/insecure.gif",
            // The one written without its scheme, read as `https`.
            "https://evil.example/schemeless.gif",
            "https://evil.example/lazy.gif",
            "https://evil.example/picture-fallback.jpg",
            "https://evil.example/noscript.gif",
            "https://evil.example/mathml.gif",
        ]
    );
    // The newsletter: its eight pictures, and neither of its two tracking pixels, nor its
    // background, nor its fonts.
    select(&mut harness, id_of(&ids, "newsletter"));
    harness.run_steps(10);
    let asked = host.asked();
    assert_eq!(asked.get(7..), Some(&NEWSLETTER_PICTURES.map(String::from)[..]));
    assert!(!asked.iter().any(|url| url.contains("/o/v1/") || url.contains("pixel.") || url.contains("fonts.") || url.contains("bg/leaves")));
    // Every other message of the corpus: nothing but `http` and `https` is ever handed to the host.
    for (_, id) in &ids {
        select(&mut harness, *id);
    }
    // (The scheme as the message wrote it: one of them shouts.)
    let on_the_web = |url: &String| ["https://", "http://"].iter().any(|scheme| url.to_ascii_lowercase().starts_with(scheme));
    assert!(host.asked().iter().all(on_the_web), "{:?}", host.asked());
    assert!(host.asked().iter().any(|url| url.starts_with("HTTPS://img.tallowwick.example/")), "{:?}", host.asked());

    // The second look, on blocks the parser would not have let through.
    let picture = |src: &str, width: Option<u16>, height: Option<u16>| {
        mailcraft_html::Block::new(mailcraft_html::BlockKind::Image { src: src.into(), width, height, link: None }, Vec::new())
    };
    let doc = Doc::from_blocks(vec![
        picture("https://t.example/1x1.gif", Some(1), Some(1)),
        picture("https://t.example/wide.gif", Some(600), Some(2)),
        picture("https://t.example/tall.gif", Some(2), None),
        picture("https://t.example/real.png", Some(3), Some(3)),
        picture(" HTTPS://t.example/unsized.png ", None, None),
        picture("https://t.example/real.png", Some(300), Some(300)),
        picture("cid:own@t.example", None, None),
        picture("mailto:someone@t.example", None, None),
        picture("ftp://t.example/a.png", None, None),
        picture("file:///C:/Users/owner/a.png", None, None),
        picture("data:image/png;base64,AAAA", None, None),
        picture("//t.example/schemeless.png", None, None),
        picture("httpx://t.example/a.png", None, None),
        picture("", None, None),
    ]);
    assert_eq!(doc.remote.iter().map(|url| &**url).collect::<Vec<_>>(), ["https://t.example/real.png", "HTTPS://t.example/unsized.png"]);
    assert_eq!((doc.remote_pictures, doc.remote_left_out), (13, 0));
    for (src, on_the_web) in [
        ("http://a.example/x", true),
        ("HtTpS://a.example/x", true),
        ("https:/a.example", false),
        ("http", false),
        ("", false),
        ("cid:http://x", false),
    ] {
        assert_eq!(super::images::is_remote(src), on_the_web, "{src:?}");
    }
}

#[test]
fn pictures_come_a_few_at_a_time_and_only_for_what_is_on_screen() {
    let (mut app, ids) = corpus_app(false);
    let host = Host::holding(&mut app);
    let (news, plain) = (id_of(&ids, "newsletter"), id_of(&ids, "plain thread"));
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, news);
    harness.state_mut().execute("ui.downloadPictures", json!({"message": news})).unwrap();
    harness.run_steps(12);
    // Four are on their way, and no fifth until one of them is back.
    assert_eq!((host.asked().len(), harness.state().remote_pictures.flying()), (super::remote::AT_A_TIME, 4));
    assert!(says(&harness, "Downloading pictures"));
    // The room each asks for is kept meanwhile, so nothing moves when it comes.
    let before = super::view::kept(&harness.ctx, news, |laid, _| laid.size).unwrap();
    host.release(1, || newsletter_picture("wordmark.png"));
    harness.run_steps(6);
    assert_eq!((host.asked().len(), shown(&harness, news)), (5, (1, 7, 7)));
    assert_eq!(super::view::kept(&harness.ctx, news, |laid, _| laid.size).unwrap(), before);
    // The owner reads something else: what was asked for still arrives, and no more is asked for.
    select(&mut harness, plain);
    host.release(4, || Ok(png(600, 280)));
    harness.run_steps(12);
    assert_eq!((host.asked().len(), harness.state().remote_pictures.flying()), (5, 0));
    // Back at the message, the rest is sent for.
    select(&mut harness, news);
    harness.run_steps(4);
    assert_eq!(host.asked(), NEWSLETTER_PICTURES);
    host.release(8, || Ok(png(32, 32)));
    harness.run_steps(12);
    assert_eq!((host.asked().len(), shown(&harness, news)), (8, (8, 0, 0)));
    assert!(!says(&harness, "Downloading pictures"));
}

#[test]
fn what_the_host_brings_is_checked_and_nothing_is_asked_for_twice() {
    // A picture, a refusal, bytes that are no picture, a picture that claims to be enormous, one
    // that is too heavy, an empty answer: one picture is shown, and each address was tried once.
    let pictures = web_pictures("mixed.example", 6);
    let (mut app, ids) = app_with(vec![web_message(0, "a@mixed.example", "Mixed", &pictures)]);
    let host = Host::answering(&mut app, |url| match url.rsplit('/').next().unwrap_or("") {
        "0.png" => Ok(png(40, 30)),
        "1.png" => Err("the server answered 404 instead of a picture".into()),
        "2.png" => Ok(b"<html><script>alert(1)</script></html>".to_vec()),
        "3.png" => {
            let mut huge = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            huge.extend([0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 8, 6, 0, 0, 0, 0, 0, 0, 0]);
            Ok(huge)
        }
        "4.png" => {
            let mut heavy = png(40, 30);
            heavy.resize(super::remote::MAX_BYTES + 1, 0);
            Ok(heavy)
        }
        _ => Ok(Vec::new()),
    });
    let id = ids[0];
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    harness.state_mut().execute("ui.downloadPictures", json!({})).unwrap();
    harness.run_steps(12);
    assert_eq!((host.asked(), shown(&harness, id)), (pictures.clone(), (1, 5, 5)));
    assert!(says(&harness, "5 pictures couldn't be downloaded"));
    // Read again, in a window of its own as well, a hundred frames on: nothing is tried again.
    harness.state_mut().execute("mail.select", json!({"ids": []})).unwrap();
    select(&mut harness, id);
    harness.state_mut().execute("ui.openWindow", json!({"message": id})).unwrap();
    harness.run_steps(100);
    assert_eq!(host.asked().len(), 6);
    // Asking again is no way round it either.
    harness.state_mut().execute("ui.downloadPictures", json!({"message": id})).unwrap();
    harness.run_steps(10);
    assert_eq!(host.asked().len(), 6);

    // A host that never answers is given up on, and one that goes away is not waited for.
    let pictures = web_pictures("silent.example", 6);
    let (mut app, ids) = app_with(vec![web_message(0, "a@silent.example", "Silent", &pictures)]);
    let host = Host::holding(&mut app);
    app.execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, ids[0]);
    harness.run_steps(8);
    assert_eq!((host.asked().len(), harness.state().remote_pictures.flying()), (4, 4));
    harness.state_mut().remote_pictures.age(120_000.0);
    harness.run_steps(8);
    assert_eq!((host.asked().len(), harness.state().remote_pictures.flying()), (6, 2));
    host.leave();
    harness.run_steps(8);
    assert_eq!((host.asked().len(), harness.state().remote_pictures.flying(), shown(&harness, ids[0])), (6, 0, (0, 6, 6)));
    assert!(says(&harness, "6 pictures couldn't be downloaded"));
    harness.run_steps(40);
    assert_eq!(host.asked().len(), 6);
}

/// A stand-in for what the host keeps of pictures on disk: it notes what it is told to forget.
#[derive(Clone, Default)]
struct KeptStub {
    usage: Arc<Mutex<(usize, u64)>>,
    forgotten: Arc<Mutex<Vec<String>>>,
    cleared: Arc<Mutex<usize>>,
}

impl crate::PictureCache for KeptStub {
    fn usage(&self) -> (usize, u64) {
        *self.usage.lock().unwrap()
    }

    fn forget(&self, url: &str) {
        self.forgotten.lock().unwrap().push(url.to_string());
    }

    fn clear(&self) -> Result<(), String> {
        *self.cleared.lock().unwrap() += 1;
        *self.usage.lock().unwrap() = (0, 0);
        Ok(())
    }
}

/// Where the stand-in for the picture whose address ends in `end` is on screen.
fn stand_in_at(harness: &Window, id: u64, end: &str) -> egui::Pos2 {
    super::view::kept(&harness.ctx, id, |laid, origin| {
        laid.items.iter().find_map(|item| match &item.kind {
            ItemKind::Placeholder(stand_in) if stand_in.src.ends_with(end) => Some(item.rect.translate(origin.to_vec2()).center()),
            _ => None,
        })
    })
    .flatten()
    .unwrap_or_else(|| panic!("no stand-in for {end}"))
}

fn point_at(harness: &mut Window, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.run_steps(4);
}

#[test]
fn a_picture_that_fails_says_why_and_can_be_tried_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let pictures = web_pictures("flaky.example", 5);
    let (mut app, ids) = app_with(vec![web_message(0, "a@flaky.example", "Flaky", &pictures)]);
    let round = Arc::new(AtomicUsize::new(0));
    let turn = round.clone();
    let host = Host::answering(&mut app, move |url| {
        let first = turn.load(Ordering::Relaxed) == 0;
        match url.rsplit('/').next().unwrap_or("") {
            "0.png" => Ok(png(40, 30)),
            "1.png" if first => Err("it is no longer there (404)".into()),
            "1.png" => Ok(png(40, 30)),
            "2.png" if first => Err("the server took too long".into()),
            // The words are the host's, and through it a server's: whatever is in them.
            "2.png" => Err("couldn't reach flaky.example:\r\n the server refused\u{7} the connection.".into()),
            "3.png" => Ok(b"<!DOCTYPE html><html><body>Sign in to see this picture</body></html>".to_vec()),
            // A kind of picture nobody asked for by name, shown all the same.
            _ => Ok(webp(40, 30)),
        }
    });
    let kept = KeptStub::default();
    app.services.picture_cache = Some(Box::new(kept.clone()));
    let id = ids[0];
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    harness.state_mut().execute("ui.downloadPictures", json!({})).unwrap();
    harness.run_steps(12);
    // Two came (one of them WebP); three have a stand-in where they would be, and the bar says so.
    assert_eq!((host.asked(), shown(&harness, id)), (pictures.clone(), (2, 3, 3)));
    assert!(says(&harness, "3 pictures couldn't be downloaded.") && says(&harness, "Point at one to see why."));
    assert!(!says(&harness, "Download pictures") && !says(&harness, "Downloading"));
    // Each says why when it is pointed at, and where it would have come from.
    for (n, why) in [(1, "it is no longer there (404)"), (2, "the server took too long"), (3, "the server sent a web page instead of a picture")] {
        let at = stand_in_at(&harness, id, &format!("/{n}.png"));
        point_at(&mut harness, at);
        assert!(says(&harness, &format!("This picture couldn't be downloaded: {why}.")), "{n}: {why}");
        assert!(says(&harness, &format!("https://flaky.example/p/{n}.png")), "{n}");
    }
    point_at(&mut harness, egui::pos2(4.0, 4.0));
    // What turned out to be no picture is not kept on by the host either.
    assert_eq!(*kept.forgotten.lock().unwrap(), pictures[1..4]);

    // "Try again": the three that failed are asked for once more, and only those.
    kept.forgotten.lock().unwrap().clear();
    round.store(1, Ordering::Relaxed);
    let again = harness.state().hit.rect("retryPictures", id).expect("no way to try again");
    click_at(&mut harness, again.center());
    harness.run_steps(12);
    assert_eq!(host.asked().get(5..), Some(&pictures[1..4]));
    assert_eq!(shown(&harness, id), (3, 2, 2));
    assert!(says(&harness, "2 pictures couldn't be downloaded."));
    // The host was told to forget what it might have kept for them before it was asked again.
    assert_eq!(kept.forgotten.lock().unwrap().get(..3), Some(&pictures[1..4]));
    // The reason is the new one, made one line of.
    let at = stand_in_at(&harness, id, "/2.png");
    point_at(&mut harness, at);
    assert!(says(&harness, "This picture couldn't be downloaded: couldn't reach flaky.example: the server refused the connection."));
    point_at(&mut harness, egui::pos2(4.0, 4.0));
    // Nothing is tried again by itself, however long the message is read.
    harness.run_steps(60);
    assert_eq!(host.asked().len(), 8);
    // By its command, as often as the owner likes; a message with nothing failed asks nothing.
    harness.state_mut().execute("ui.retryPictures", json!({"message": id})).unwrap();
    harness.run_steps(12);
    assert_eq!(host.asked().len(), 10);
    round.store(0, Ordering::Relaxed);
    let (mut app, ids) = app_with(vec![web_message(0, "a@fine.example", "Fine", &web_pictures("fine.example", 3))]);
    let host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    app.execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, ids[0]);
    harness.run_steps(8);
    assert!(harness.state().hit.rect("retryPictures", ids[0]).is_none() && !says(&harness, "Try again"));
    harness.state_mut().execute("ui.retryPictures", json!({})).unwrap();
    harness.run_steps(12);
    assert_eq!((host.asked().len(), shown(&harness, ids[0])), (3, (3, 0, 0)));
}

#[test]
fn everybody_at_a_domain_can_be_trusted_with_pictures() {
    let (mut app, ids) = app_with(vec![
        web_message(0, "news@shop.example", "From the shop", &web_pictures("a.shop.example", 2)),
        web_message(1, "Orders@SHOP.Example", "From its orders desk", &web_pictures("b.shop.example", 2)),
        web_message(2, "news@mail.shop.example", "From a domain under it", &web_pictures("c.shop.example", 2)),
        web_message(3, "news@notshop.example", "From a look-alike", &web_pictures("d.shop.example", 2)),
        web_message(4, "billing@shop.example", "In the junk", &web_pictures("e.shop.example", 2)),
        web_message(5, "shop.example@elsewhere.example", "With the domain for a name", &web_pictures("f.shop.example", 2)),
    ]);
    let (first, second, under, alike, junk, named) = (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);
    app.execute("mail.junk", json!({"ids": [junk]})).unwrap();
    let host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    let mut harness = window(app, (1280.0, 800.0));
    let from = |host: &Host, part: &str| host.asked().iter().filter(|url| url.contains(part)).count();

    // Beside the sender, the bar offers everybody at the sender's domain.
    select(&mut harness, first);
    assert!(says(&harness, "Always download pictures from news@shop.example") && says(&harness, "Always from @shop.example"));
    let everybody = harness.state().hit.rect("alwaysDownloadPicturesDomain", first).expect("no way to trust the domain");
    click_at(&mut harness, everybody.center());
    harness.run_steps(8);
    assert_eq!(harness.state().ui.picture_domains, ["shop.example"]);
    assert!(harness.state().ui.picture_senders.is_empty());
    assert_eq!((from(&host, "a.shop"), shown(&harness, first)), (2, (2, 0, 0)));
    // Somebody else at that domain, however the address is written: without asking.
    select(&mut harness, second);
    harness.run_steps(6);
    assert_eq!((from(&host, "b.shop"), shown(&harness, second)), (2, (2, 0, 0)));
    // That domain, not the ones under it, not one that ends alike, not a name that reads like it.
    for (id, part) in [(under, "c.shop"), (alike, "d.shop"), (named, "f.shop")] {
        select(&mut harness, id);
        harness.run_steps(6);
        assert_eq!((from(&host, part), shown(&harness, id)), (0, (0, 2, 2)), "{part}");
    }
    // And nothing that landed in Junk E-mail, where the domain isn't offered either.
    select(&mut harness, junk);
    harness.run_steps(6);
    assert_eq!((from(&host, "e.shop"), shown(&harness, junk)), (0, (0, 2, 2)));
    assert!(harness.state().hit.rect("alwaysDownloadPicturesDomain", junk).is_none() && harness.state().hit.rect("downloadPictures", junk).is_some());

    // It is kept with the settings, and comes back with them.
    let saved = serde_json::to_value(harness.state().prefs()).unwrap();
    assert_eq!(saved["pictureDomains"], json!(["shop.example"]));
    let restored: crate::UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.picture_domains, ["shop.example"]);
    let old: crate::UiState = serde_json::from_value(json!({"tab": "Home", "pictureSenders": ["a@b.example"]})).unwrap();
    assert!(old.picture_domains.is_empty());

    // File › Options lists it, and takes it off again: its senders' pictures wait to be asked for.
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    assert!(says(&harness, "everybody at @shop.example") && says(&harness, "Pictures are always downloaded from:"));
    let removed = harness.state_mut().execute("ui.alwaysDownloadPictures", json!({"domain": " @Shop.Example. ", "value": false})).unwrap();
    assert_eq!((&removed["domain"], &removed["value"], &removed["domains"]), (&json!("shop.example"), &json!(false), &json!(0)));
    harness.run_steps(3);
    assert!(!says(&harness, "everybody at @shop.example") && says(&harness, "No senders are remembered."));
    harness.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    let before = host.asked().len();
    select(&mut harness, second);
    harness.run_steps(6);
    assert_eq!((shown(&harness, second), host.asked().len()), ((0, 2, 2), before));

    // The command: a domain as it is written after an @, kept once, in lower case.
    let app = harness.state_mut();
    for written in ["Shop.Example", "@shop.example", "someone@SHOP.example", " shop.example. "] {
        assert_eq!(app.execute("ui.alwaysDownloadPictures", json!({"domain": written})).unwrap()["domains"], 1, "{written}");
    }
    assert_eq!(app.ui.picture_domains, ["shop.example"]);
    for bad in [
        json!(""),
        json!("@"),
        json!("not a domain"),
        json!("shop..example"),
        json!(".example"),
        json!("-shop.example"),
        json!("a@b@"),
        json!("shop.example/x"),
        json!("x\r\ny.example"),
        json!(7),
        json!(["shop.example"]),
        json!({"name": "shop.example"}),
        json!("a".repeat(300)),
    ] {
        assert!(app.execute("ui.alwaysDownloadPictures", json!({"domain": bad})).is_err(), "{bad}");
    }
    assert_eq!(app.ui.picture_domains, ["shop.example"]);
    // Only so many of them, and a settings file somebody wrote by hand does no harm.
    for n in 0..super::remote::MAX_DOMAINS + 20 {
        let _ = app.execute("ui.alwaysDownloadPictures", json!({"domain": format!("d{n}.example")}));
    }
    assert_eq!(app.ui.picture_domains.len(), super::remote::MAX_DOMAINS);
    assert!(app.execute("ui.alwaysDownloadPictures", json!({"domain": "one-more.example"})).unwrap_err().contains("full"));
    let mut hostile = app.prefs();
    hostile.picture_domains = (0..40_000).map(|n| if n % 2 == 0 { format!("X{n}.Example") } else { "\u{0}@\n ".repeat(n % 5) }).collect();
    app.apply_prefs(hostile);
    assert!(super::remote::domain_remembered(&app.ui, "anyone@x0.example"));
    assert!(!super::remote::domain_remembered(&app.ui, "anyone@x39998.example"), "past what is looked at");
    assert!(!super::remote::domain_remembered(&app.ui, "") && !super::remote::domain_remembered(&app.ui, "x0.example"));
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
}

#[test]
fn a_message_that_is_all_pictures_offers_them_instead_of_its_text_part() {
    // A sale that is two slices of one picture, each in a cell with no font size, the second
    // with an address that leaves its scheme out; a line of text beside it for programs that
    // show no HTML; and a pixel that counts openings.
    let html = "<table cellpadding=0 cellspacing=0><tr><td style=\"font-size:0;line-height:0\"><a href=\"https://shop.example/sale\">\
                <img src=\"https://img.shop.example/sale/slice-1.png\" width=\"300\" height=\"100\" alt=\"Everything a third off\"></a></td></tr>\
                <tr><td style=\"font-size:0px\"><img src=\"//img.shop.example/sale/slice-2.png\" width=\"300\" height=\"80\"></td></tr></table>\
                <img src=\"https://t.shop.example/o.gif?u=1\" width=1 height=1>";
    let sale = || message(0, ("The Shop", "news@shop.example"), "Sale", Some(html.into()), "Everything a third off. See it in your browser.".into());
    let texts = |harness: &Window, id: u64| super::view::kept(&harness.ctx, id, |laid, _| laid.stats.texts).unwrap();
    // Where pictures can't be downloaded, the text part says more than stand-ins would.
    let (app, ids) = app_with(vec![sale()]);
    let mut harness = window(app, (1280.0, 800.0));
    select(&mut harness, ids[0]);
    assert_eq!((shown(&harness, ids[0]), texts(&harness, ids[0])), ((0, 0, 0), 1));
    assert!(!says(&harness, "Pictures not"));
    // Where they can: the message as it is, stand-ins first, and the offer above it.
    let (mut app, ids) = app_with(vec![sale()]);
    let host = Host::answering(&mut app, |url| Ok(png(300, if url.contains("slice-1") { 100 } else { 80 })));
    let id = ids[0];
    let mut harness = window(app, (1280.0, 800.0));
    select(&mut harness, id);
    assert_eq!((shown(&harness, id), texts(&harness, id)), ((0, 2, 2), 0));
    assert!(says(&harness, "Pictures not downloaded.") && host.asked().is_empty());
    let download = harness.state().hit.rect("downloadPictures", id).expect("no way to ask for the pictures");
    click_at(&mut harness, download.center());
    harness.run_steps(12);
    // Its two pictures, and not its pixel.
    assert_eq!(host.asked(), ["https://img.shop.example/sale/slice-1.png", "https://img.shop.example/sale/slice-2.png"]);
    assert_eq!(shown(&harness, id), (2, 0, 0));
    let rects = super::view::kept(&harness.ctx, id, |laid, _| shown_pictures(laid).iter().map(|i| i.rect).collect::<Vec<_>>()).unwrap();
    assert_eq!(rects.iter().map(|r| (r.width(), r.height())).collect::<Vec<_>>(), [(300.0, 100.0), (300.0, 80.0)]);
    // Slices of one picture: the second stands right on the first, edge to edge, no stripe between.
    assert_eq!((rects[1].min.y, rects[1].min.x), (rects[0].max.y, rects[0].min.x));
}

#[test]
fn a_newsletter_from_a_column_builder_shows_its_pictures_when_asked() {
    for theme in ["light", "dark"] {
        let (mut app, ids) = corpus_app(false);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        let host = Host::answering(&mut app, |url| {
            let (w, h) = match url {
                u if u.contains("wordmark") => (360, 80),
                u if u.contains("hero") => (1200, 500),
                u if u.contains("/products/") => (524, 524),
                u if u.contains("slice-1") => (600, 96),
                u if u.contains("slice-2") => (600, 120),
                u if u.contains("signature") => (264, 96),
                _ => (56, 56),
            };
            Ok(if url.contains(".jpg") { jpeg(w, h) } else { png(w, h) })
        });
        let id = id_of(&ids, "built newsletter");
        let mut harness = window(app, (1280.0, 1000.0));
        select(&mut harness, id);
        // Everything in it is in cells with a font size of nothing: it is all there all the same,
        // nine stand-ins among its text, and the bar that offers the pictures.
        assert_eq!(shown(&harness, id), (0, 9, 9));
        assert!(super::view::kept(&harness.ctx, id, |laid, _| laid.stats.texts).unwrap() >= 8);
        assert!(says(&harness, "Pictures not downloaded.") && says(&harness, "Always from @letters.tallowwick.example"));
        assert!(host.asked().is_empty());
        harness.state_mut().execute("ui.downloadPictures", json!({"message": id})).unwrap();
        harness.run_steps(20);
        // Asked for as the message wrote them (whoever downloads writes them the way the wire
        // has them): the one without its scheme as `https`, the one in capitals as it shouts,
        // the one with spaces with its spaces, the broken line mended.
        assert_eq!(
            host.asked(),
            [
                "https://img.tallowwick.example/brand/wordmark-ink.png",
                "HTTPS://img.tallowwick.example/2026/11/hero-dusk-table.jpg?w=1200&q=72&sig=9c1e_f0a7",
                "https://img.tallowwick.example/2026/11/products/amber glow jar.jpg",
                "https://img.tallowwick.example/2026/11/products/winter-tapers-six.jpg",
                "https://img.tallowwick.example/2026/11/open-day/slice-1.png",
                "https://img.tallowwick.example/2026/11/open-day/slice-2.png",
                "https://img.tallowwick.example/people/signature-maud@2x.png",
                "https://img.tallowwick.example/icons/round-photos.png",
                "https://img.tallowwick.example/icons/round-letters.png",
            ]
        );
        assert_eq!(shown(&harness, id), (9, 0, 0));
        assert!(!says(&harness, "Pictures not downloaded") && !says(&harness, "couldn't be downloaded"));
        // At the sizes the message gives them: from its style where the attributes give none,
        // and in the picture's own shape where it gives a width alone.
        let sizes = super::view::kept(&harness.ctx, id, |laid, _| {
            shown_pictures(laid).iter().map(|i| (i.rect.width().round(), i.rect.height().round())).collect::<Vec<_>>()
        });
        let sizes = sizes.unwrap();
        assert_eq!((sizes[0], sizes[6], sizes[7], sizes[8]), ((180.0, 40.0), (132.0, 48.0), (28.0, 28.0), (28.0, 28.0)));
        assert!(sizes[1].0 > 300.0 && sizes[1].0 <= 600.0 && (sizes[1].0 / sizes[1].1 - 2.4).abs() < 0.02, "{:?}", sizes[1]);
        assert!(sizes[2].0 == sizes[2].1 && sizes[2] == sizes[3] && sizes[2].0 <= 262.0, "{:?}", sizes[2]);
    }
}

#[test]
fn what_is_kept_of_pictures_is_said_and_cleared_in_the_options() {
    let pictures = web_pictures("kept.example", 2);
    let (mut app, ids) = app_with(vec![web_message(0, "a@kept.example", "Kept", &pictures)]);
    let host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    let kept = KeptStub::default();
    *kept.usage.lock().unwrap() = (12, 3_400_000);
    app.services.picture_cache = Some(Box::new(kept.clone()));
    app.execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    let id = ids[0];
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    harness.run_steps(10);
    assert_eq!((host.asked().len(), shown(&harness, id)), (2, (2, 0, 0)));
    // File › Options says how much the host keeps, and has the way to be rid of it.
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    assert!(says(&harness, "12 downloaded pictures are kept on this computer (3.2 MB)."));
    harness.get_by_label("Clear").click();
    harness.run_steps(4);
    assert_eq!(*kept.cleared.lock().unwrap(), 1);
    assert!(says(&harness, "No downloaded pictures are kept on this computer."));
    // What was in memory went with it: the message's pictures are asked for anew.
    harness.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    harness.run_steps(12);
    assert_eq!((host.asked().len(), shown(&harness, id)), (4, (2, 0, 0)));
    // One kept, in the singular and in kilobytes; and the command says how many went.
    *kept.usage.lock().unwrap() = (1, 2_100);
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    assert!(says(&harness, "1 downloaded picture is kept on this computer (3 KB)."));
    assert_eq!(harness.state_mut().execute("ui.clearPictureCache", json!({})).unwrap()["cleared"], 1);
    assert_eq!(*kept.cleared.lock().unwrap(), 2);

    // A host that keeps nothing: no such row, and the command still empties the memory.
    let (mut app, ids) = app_with(vec![web_message(0, "a@kept.example", "Kept", &pictures)]);
    let host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    app.execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, ids[0]);
    harness.run_steps(10);
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    assert!(!says(&harness, "kept on this computer"));
    assert_eq!(harness.state_mut().execute("ui.clearPictureCache", json!({"junk": [1, 2]})).unwrap()["cleared"], 0);
    harness.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    harness.run_steps(12);
    assert_eq!(host.asked().len(), 4);
}

#[test]
fn a_message_gets_only_so_many_pictures_and_so_many_bytes() {
    use super::remote::{MAX_MESSAGE_BYTES, MAX_PER_MESSAGE};
    // More pictures than one message is allowed: the first so many, each once, the rest left out.
    let many = web_pictures("many.example", MAX_PER_MESSAGE + 40);
    let mut twice = many.clone();
    twice.extend(many.iter().take(30).cloned());
    // Heavy ones: three megabytes each of what is not even a picture.
    let heavy = web_pictures("heavy.example", 20);
    let (mut app, ids) = app_with(vec![web_message(0, "a@many.example", "Many", &twice), web_message(1, "a@heavy.example", "Heavy", &heavy)]);
    let host = Host::answering(&mut app, |url| if url.contains("heavy") { Ok(vec![0x55; 3 * 1024 * 1024]) } else { Ok(png(4, 4)) });
    app.execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, ids[0]);
    harness.run_steps(60);
    let asked = host.asked();
    assert_eq!(asked, many[..MAX_PER_MESSAGE]);
    assert_eq!(shown(&harness, ids[0]).0, MAX_PER_MESSAGE + 30, "the pictures that came, wherever the message shows them");
    assert!(says(&harness, "40 pictures couldn't be downloaded"));
    harness.run_steps(30);
    assert_eq!(host.asked().len(), MAX_PER_MESSAGE);

    // Once so many bytes have come for a message, it gets no more.
    select(&mut harness, ids[1]);
    harness.run_steps(60);
    let heavy_asked = host.asked().len() - MAX_PER_MESSAGE;
    assert_eq!(heavy_asked, MAX_MESSAGE_BYTES / (3 * 1024 * 1024), "{heavy_asked} were asked for");
    assert_eq!(shown(&harness, ids[1]), (0, 20, 20));
    // And what is kept of pictures stays within its own bounds whatever comes.
    let kept = Pictures::of(&harness.ctx).lock().unwrap().remote_counts();
    assert!(kept.0 <= super::images::REMOTE_PICTURES && kept.1 <= super::images::REMOTE_BYTES, "{kept:?}");
}

#[test]
fn what_is_kept_of_pictures_from_the_web_is_bounded() {
    use super::images::{REMOTE_BYTES, REMOTE_PICTURES, Remote};
    let mut pictures = Pictures::default();
    let megabyte = Arc::new(vec![0u8; 1024 * 1024]);
    for n in 0..(REMOTE_PICTURES as u64 + 200) {
        let state = if n % 3 == 0 { Remote::Failed(Arc::from("it took too long")) } else { Remote::Ready(megabyte.clone()) };
        pictures.set_remote(Arc::from(format!("https://p.example/{n}")), state, n);
        let (count, bytes) = pictures.remote_counts();
        assert!(count <= REMOTE_PICTURES && bytes <= REMOTE_BYTES, "{count} pictures, {bytes} bytes");
    }
    // The ones asked about longest ago went first; the newest is there.
    assert!(pictures.remote_peek("https://p.example/0").is_none());
    assert!(pictures.remote(&format!("https://p.example/{}", REMOTE_PICTURES + 199), 9_999).is_some());
    // What is on screen in this very frame is never dropped, though that passes the bounds for a moment.
    let mut pictures = Pictures::default();
    for n in 0..REMOTE_PICTURES + 5 {
        pictures.set_remote(Arc::from(format!("https://q.example/{n}")), Remote::Failed(Arc::from("it took too long")), 7);
    }
    assert_eq!(pictures.remote_counts().0, REMOTE_PICTURES + 5);
    pictures.set_remote(Arc::from("https://q.example/next"), Remote::Failed(Arc::from("it took too long")), 8);
    assert_eq!(pictures.remote_counts().0, REMOTE_PICTURES);
}

#[test]
fn hostile_bytes_from_the_host_never_panic() {
    // The same junk the decoder is thrown at, this time by way of the host.
    let pictures = web_pictures("junk.example", 48);
    let (mut app, ids) = app_with(vec![web_message(0, "a@junk.example", "Junk bytes", &pictures)]);
    let originals = [png(24, 16), jpeg(24, 16), gif(24, 16)];
    let host = Host::answering(&mut app, move |url| {
        let n: usize = url.rsplit('/').next().and_then(|name| name.trim_end_matches(".png").parse().ok()).unwrap_or(0);
        let mut bytes = originals[n % 3].clone();
        let mut seed = 0x9e37_79b9_7f4a_7c15u64 ^ (n as u64 + 1).wrapping_mul(0x2545_f491_4f6c_dd1d);
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        // A few bytes changed, some of them cut short, a few left whole.
        if !n.is_multiple_of(8) {
            for _ in 0..1 + next() % 4 {
                let at = (next() % bytes.len() as u64) as usize;
                bytes[at] = next() as u8;
            }
        }
        if n % 4 == 1 {
            let keep = (next() % bytes.len() as u64) as usize;
            bytes.truncate(keep);
        }
        Ok(bytes)
    });
    app.execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, ids[0]);
    harness.run_steps(80);
    assert_eq!(host.asked().len(), 48);
    let (pictures, stand_ins, _) = shown(&harness, ids[0]);
    assert!(pictures >= 6 && pictures + stand_ins == 48, "{pictures} pictures and {stand_ins} stand-ins");
}

#[test]
fn a_build_that_cannot_download_says_so_and_offers_nothing() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "newsletter");
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    assert!(says(&harness, "This build can't download them"));
    assert!(!says(&harness, "Always download pictures") && !says(&harness, "Pictures not downloaded"));
    assert!(harness.state().hit.rect("downloadPictures", id).is_none() && harness.state().hit.rect("alwaysDownloadPictures", id).is_none());
    // The command says why not; the settings can still be set, and change nothing here.
    assert!(harness.state_mut().execute("ui.downloadPictures", json!({"message": id})).unwrap_err().contains("can't download"));
    harness.state_mut().execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    harness.state_mut().execute("ui.alwaysDownloadPictures", json!({"message": id})).unwrap();
    harness.run_steps(6);
    assert_eq!(shown(&harness, id), (0, 8, 8));
    assert_eq!(harness.state().remote_pictures.flying(), 0);
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    assert!(says(&harness, "This build can't download pictures"));
}

#[test]
fn the_picture_commands_take_anything_and_never_panic() {
    let (mut app, ids) = corpus_app(false);
    let host = Host::answering(&mut app, |_| Ok(png(8, 8)));
    let news = id_of(&ids, "newsletter");
    // Nothing selected and nothing named.
    app.execute("mail.select", json!({"ids": []})).unwrap();
    assert!(app.execute("ui.downloadPictures", json!({})).is_err());
    assert!(app.execute("ui.alwaysDownloadPictures", json!({})).is_err());
    let junk = [
        json!(null),
        json!([]),
        json!("text"),
        json!(7),
        json!({"message": "x"}),
        json!({"message": -1}),
        json!({"message": 0}),
        json!({"message": u64::MAX}),
        json!({"message": 1e300}),
        json!({"message": {"id": 1}}),
        json!({"event": "x"}),
        json!({"event": u64::MAX}),
        json!({"window": -3}),
        json!({"message": 1, "event": 1}),
        json!({"sender": 5}),
        json!({"sender": ""}),
        json!({"sender": "no address"}),
        json!({"sender": "a@b\r\nX-Injected: 1"}),
        json!({"sender": format!("{}@example.org", "a".repeat(5_000))}),
        json!({"sender": "ok@example.org", "value": "yes"}),
        json!({"domain": null}),
        json!({"domain": 5}),
        json!({"domain": ""}),
        json!({"domain": "a b"}),
        json!({"domain": "x.example\r\nX-Injected: 1"}),
        json!({"domain": format!("{}.example", "a".repeat(5_000))}),
        json!({"domain": {"name": "x.example"}, "sender": "ok@example.org"}),
        json!({"domain": "ok.example", "value": "yes"}),
        json!({"value": []}),
        json!({"all": "yes"}),
    ];
    for command in [
        "ui.downloadPictures",
        "ui.alwaysDownloadPictures",
        "ui.downloadPicturesAutomatically",
        "ui.retryPictures",
        "ui.clearPictureCache",
        "ui.openWindow",
        "ui.closeWindow",
    ] {
        for params in &junk {
            let _ = app.execute(command, params.clone());
            let _ = app.run(command, params.clone());
        }
    }
    assert!(app.ui.picture_senders.iter().all(|s| s == "ok@example.org"), "{:?}", app.ui.picture_senders);
    assert!(app.ui.picture_domains.iter().all(|d| d == "ok.example"), "{:?}", app.ui.picture_domains);
    app.execute("mail.select", json!({"ids": []})).unwrap();
    assert!(app.execute("ui.retryPictures", json!({})).is_err(), "nothing is selected");
    // A sender is an address, kept once, in lower case, and only so many of them.
    app.ui.picture_senders.clear();
    for written in ["Ada@Example.org", " ada@example.ORG ", "ada@example.org"] {
        assert_eq!(app.execute("ui.alwaysDownloadPictures", json!({"sender": written})).unwrap()["senders"], 1);
    }
    assert_eq!(app.ui.picture_senders, ["ada@example.org"]);
    assert_eq!(app.execute("ui.alwaysDownloadPictures", json!({"message": news})).unwrap()["sender"], "hello@news.fernhollow.example");
    for n in 0..super::remote::MAX_SENDERS + 20 {
        let _ = app.execute("ui.alwaysDownloadPictures", json!({"sender": format!("s{n}@example.org")}));
    }
    assert_eq!(app.ui.picture_senders.len(), super::remote::MAX_SENDERS);
    assert!(app.execute("ui.alwaysDownloadPictures", json!({"sender": "one-more@example.org"})).unwrap_err().contains("full"));
    // A settings file somebody wrote by hand: too many senders, and some that are no addresses.
    let mut hostile = app.prefs();
    hostile.picture_senders = (0..50_000).map(|n| if n % 2 == 0 { format!("x{n}@example.org") } else { "\u{0}\n".repeat(n % 7) }).collect();
    hostile.download_pictures = false;
    app.apply_prefs(hostile);
    let mut harness = window(app, (1280.0, 900.0));
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    harness.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    select(&mut harness, news);
    assert!(host.asked().is_empty());
    assert!(!super::remote::sender_remembered(&harness.state().ui, ""));
    assert!(super::remote::sender_remembered(&harness.state().ui, "X0@Example.org"));
    assert!(!super::remote::sender_remembered(&harness.state().ui, "x49998@example.org"), "past what is looked at");
}

/// The line above a message, a message with its pictures, and a message in a window of its own,
/// in light and dark, as PNG files to look at:
/// `MAILCRAFT_SHOTS_DIR=<dir> cargo test -p mailcraft-ui-egui shots_of_pictures -- --ignored`
#[test]
#[ignore = "writes screenshots, for looking at them"]
fn shots_of_pictures_from_the_web_and_of_a_message_window() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else {
        eprintln!("set MAILCRAFT_SHOTS_DIR to the directory to write to");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    for theme in ["light", "dark"] {
        let (mut app, ids) = corpus_app(false);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        // The newsletter's pictures all come; of the built one's, two don't, and say why.
        let _host = Host::answering(&mut app, |url| match url {
            u if u.contains("fernhollow") => newsletter_picture(u),
            u if u.contains("amber") => Err("it is no longer there (404)".into()),
            u if u.contains("slice-2") => Err("the server took too long".into()),
            u if u.contains("hero") => Ok(jpeg(1200, 500)),
            u if u.contains("wordmark") => Ok(png(360, 80)),
            u if u.contains("/products/") => Ok(jpeg(524, 524)),
            u if u.contains("slice-1") => Ok(png(600, 96)),
            u if u.contains("signature") => Ok(webp(264, 96)),
            _ => Ok(png(56, 56)),
        });
        let kept = KeptStub::default();
        *kept.usage.lock().unwrap() = (37, 9_800_000);
        app.services.picture_cache = Some(Box::new(kept));
        let news = id_of(&ids, "newsletter");
        let built = id_of(&ids, "built newsletter");
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1440.0, 900.0))
            .with_pixels_per_point(2.0)
            .with_step_dt(1.0 / 60.0)
            .wgpu()
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.input_mut().max_texture_side = Some(8192);
        harness.run_steps(4);
        let shot = |harness: &mut Window, name: &str| {
            harness.run_steps(14);
            let path = dir.join(format!("{name}-{theme}.png"));
            harness.render().unwrap().save(&path).unwrap();
            println!("{}", path.display());
        };
        select(&mut harness, news);
        shot(&mut harness, "pictures-1-not-asked-for");
        harness.state_mut().execute("ui.downloadPictures", json!({"message": news})).unwrap();
        shot(&mut harness, "pictures-2-downloaded");
        harness.state_mut().execute("ui.openWindow", json!({"message": news})).unwrap();
        shot(&mut harness, "window-1-message");
        harness.state_mut().execute("ui.openWindow", json!({"message": id_of(&ids, "outlook reply")})).unwrap();
        shot(&mut harness, "window-2-two-messages");
        harness.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();
        select(&mut harness, built);
        shot(&mut harness, "pictures-3-built-not-asked-for");
        harness.state_mut().execute("ui.downloadPictures", json!({"message": built})).unwrap();
        shot(&mut harness, "pictures-4-built-two-failed");
        // Further down, where the two that failed would be: one of them pointed at.
        harness.event(egui::Event::PointerMoved(egui::pos2(1000.0, 600.0)));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -560.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        harness.run_steps(20);
        let at = stand_in_at(&harness, built, "amber glow jar.jpg");
        point_at(&mut harness, at);
        shot(&mut harness, "pictures-5-built-why");
        point_at(&mut harness, egui::pos2(4.0, 4.0));
        harness.state_mut().execute("ui.alwaysDownloadPictures", json!({"message": news})).unwrap();
        harness.state_mut().execute("ui.alwaysDownloadPictures", json!({"domain": "letters.tallowwick.example"})).unwrap();
        harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
        shot(&mut harness, "options-pictures");
    }
}

#[test]
fn a_message_in_the_pane_and_in_a_window_is_laid_out_for_each() {
    let (mut app, ids) = corpus_app(false);
    let host = Host::answering(&mut app, newsletter_picture);
    let (id, other) = (id_of(&ids, "newsletter"), id_of(&ids, "receipt"));
    let mut harness = window(app, (1280.0, 800.0));
    select(&mut harness, id);
    let own = harness.state_mut().execute("ui.openWindow", json!({"message": id})).unwrap()["window"].as_u64().unwrap();
    harness.run_steps(8);
    let at = |harness: &Window, place: Place| {
        super::view::kept_at(&harness.ctx, place, id, |laid, _| (laid.done, laid.width(), laid.stats.pictures, laid.stats.placeholders))
            .unwrap_or_else(|| panic!("nothing is laid out for {place:?}"))
    };
    // The same body, at the width of each place.
    let (in_pane, in_window) = (at(&harness, Place::Pane), at(&harness, Place::Window(own)));
    assert!(in_pane.0 && in_window.0 && in_pane.1 != in_window.1, "{in_pane:?} {in_window:?}");
    assert_eq!((in_pane.2, in_pane.3, in_window.2, in_window.3), (0, 8, 0, 8));
    // Each keeps its layout: neither is made again because the other is there too.
    let begun = super::view::layouts_begun(&harness.ctx);
    harness.run_steps(30);
    assert_eq!(super::view::layouts_begun(&harness.ctx), begun);
    // Its pictures, once the owner asks, are downloaded once and shown in both.
    assert!(host.asked().is_empty());
    harness.state_mut().execute("ui.downloadPictures", json!({"message": id})).unwrap();
    harness.run_steps(16);
    assert_eq!(host.asked(), NEWSLETTER_PICTURES);
    assert_eq!((at(&harness, Place::Pane).2, at(&harness, Place::Window(own)).2), (8, 8));
    let begun = super::view::layouts_begun(&harness.ctx);
    harness.run_steps(30);
    assert_eq!((super::view::layouts_begun(&harness.ctx), host.asked().len()), (begun, 8));
    // A window that is gone leaves nothing behind once another is opened, and what is kept
    // stays within bounds however many come and go.
    harness.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();
    harness.run_steps(4);
    for _ in 0..40 {
        for message in [other, id] {
            harness.state_mut().execute("ui.openWindow", json!({"message": message})).unwrap();
        }
        harness.run_steps(2);
        harness.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();
        harness.run_steps(2);
    }
    assert!(super::view::kept_at(&harness.ctx, Place::Window(own), id, |_, _| ()).is_none());
    assert!(super::view::kept_count(&harness.ctx) <= 4 + 16, "{}", super::view::kept_count(&harness.ctx));
}

#[test]
fn a_say_so_that_is_taken_back_stops_the_asking_at_once() {
    let (mut app, ids) = corpus_app(false);
    let host = Host::holding(&mut app);
    app.execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    let news = id_of(&ids, "newsletter");
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, news);
    harness.run_steps(6);
    assert_eq!((host.asked().len(), harness.state().remote_pictures.flying()), (4, 4));
    // Four more are waiting their turn, which comes when these four are back. The owner turns
    // the switch off first: the four that were asked for arrive, and that is the end of it.
    host.release(4, || Ok(png(32, 32)));
    harness.state_mut().execute("ui.downloadPicturesAutomatically", json!({"value": false})).unwrap();
    harness.run_steps(12);
    assert_eq!((host.asked().len(), harness.state().remote_pictures.flying()), (4, 0));
    // What came is kept, and is not shown while the message may not show pictures.
    assert_eq!(shown(&harness, news), (0, 8, 8));
    assert_eq!(Pictures::of(&harness.ctx).lock().unwrap().remote_counts().0, 4);
    // On again: those four are shown without being asked for again, and the other four are sent for.
    harness.state_mut().execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    harness.run_steps(6);
    assert_eq!((host.asked(), shown(&harness, news).0), (NEWSLETTER_PICTURES.map(String::from).to_vec(), 4));
}

// ---- selecting and copying ----

use super::select::{self, Point, Surface as _, Zone};

/// The whole of a laid-out message as a copy has it.
fn copy_of(laid: &Laid) -> String {
    match (select::clamp(laid, Point::START), select::clamp(laid, Point::END)) {
        (Some(lo), Some(hi)) => select::copied(laid, lo, hi),
        _ => String::new(),
    }
}

/// What Select All and Copy make of each message of the corpus, to read:
/// `cargo test -p mailcraft-ui-egui print_the_copies -- --ignored --nocapture`
#[test]
#[ignore = "prints what a copy of each message is, for reading it"]
fn print_the_copies_of_the_corpus() {
    in_frame(false, |ctx| {
        for (name, m) in corpus_messages() {
            let laid = lay_out(ctx, &m, 820.0);
            let text = copy_of(&laid);
            let shown: String = text.chars().take(2_500).collect();
            eprintln!(
                "==== {name}: {} characters, {} things to select ====\n{}",
                text.chars().count(),
                laid.count(),
                shown.replace('\t', "\u{2192}")
            );
        }
    });
}

/// A generator of numbers that are the same in every run.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Text with white space taken out: what must be the same however a copy breaks its lines.
fn squeezed(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// What a copy of `blocks` must hold, in reading order, written down without the layout: every
/// text with its list marker, and for each picture what `pictures` says it came to (its
/// description when it is not shown and has one).
fn told(blocks: &[mailcraft_html::Block], pictures: &mut dyn Iterator<Item = Option<String>>, out: &mut String) {
    use mailcraft_html::{BlockKind, Marker};
    for block in blocks {
        match &block.kind {
            BlockKind::Table(table) => {
                for cell in table.rows.iter().flat_map(|row| &row.cells) {
                    told(&cell.blocks, pictures, out);
                }
            }
            BlockKind::Image { .. } => out.extend(pictures.next().flatten()),
            BlockKind::Rule => {}
            kind => {
                let text = block.text();
                if text.is_empty() {
                    continue;
                }
                match kind {
                    BlockKind::ListItem { marker: Marker::Number(n), .. } => out.push_str(&format!("{n}.")),
                    BlockKind::ListItem { marker: Marker::Label(label), .. } => out.extend(label.chars().take(12)),
                    BlockKind::ListItem { marker: Marker::Bullet, depth } => out.push(['\u{2022}', '\u{25e6}', '\u{25aa}'][usize::from(depth % 3)]),
                    _ => {}
                }
                out.push_str(&text);
            }
        }
    }
}

#[test]
fn a_copy_of_a_whole_message_is_all_its_text_once_in_reading_order() {
    in_frame(false, |ctx| {
        for (name, m) in corpus_messages() {
            let doc = Doc::of(&m);
            for width in [860.0, 420.0, 180.0] {
                let laid = lay_out(ctx, &m, width);
                // What each picture came to, from the layout; everything else from the blocks.
                let mut pictures = laid.items.iter().filter_map(|item| match &item.kind {
                    ItemKind::Picture(_) => Some(None),
                    ItemKind::Placeholder(p) => Some(p.described.then(|| p.label.clone())),
                    _ => None,
                });
                let mut wanted = String::new();
                told(&doc.blocks, &mut pictures, &mut wanted);
                let copy = copy_of(&laid);
                assert!(squeezed(&copy) == squeezed(&wanted), "{name} at {width}: the copy is not the message's text");
                // Nothing a copy could not be pasted as: no more blank lines than a message
                // may ask for, no break or tab at either end.
                assert!(!copy.contains("\n\n\n\n\n") && !copy.starts_with(['\n', '\t']) && !copy.ends_with('\n'), "{name} at {width}");
                // Every thing that can be selected knows its place, and the places are in order.
                assert_eq!(laid.count(), laid.items.iter().filter(|i| i.seg != super::layout::NO_SEG).count(), "{name}");
                assert!(laid.segs.windows(2).all(|w| w[0] < w[1]), "{name}");
                for (seg, index) in laid.segs.iter().enumerate() {
                    assert_eq!(laid.items[*index as usize].seg as usize, seg, "{name}");
                }
            }
        }
    });
}

#[test]
fn a_copy_reads_like_the_message() {
    in_frame(false, |ctx| {
        let copy = |name: &str, width: f32| copy_of(&lay_out(ctx, &corpus_message(name), width));
        // Lines that follow each other directly, and the blank lines the message has between its paragraphs.
        let gmail = copy("gmail reply", 820.0);
        assert!(gmail.starts_with("Hi Priya,\n\nYes to both. I can bring the projector"), "{:?}", gmail.get(..60));
        assert!(gmail.contains("See you Thursday,\nKofi\n\nKofi Mensah\nPlatform team, Juniper Labs\n+1 555 0117\n\nOn Tue, Oct 6"));
        // List items keep their markers, a tab after each as a word processor writes them.
        assert!(gmail.contains(
            "\n\n\u{2022}\tThe demo should come before the budget slide, not after. People stop listening once they see numbers.\n\u{2022}\tCan we give Tomasz"
        ));
        assert!(gmail.contains("\n1.\tWhere we are (5 min)\n2.\tBudget (10 min)\n3.\tDemo (15 min)\n4.\tQuestions\n"));
        // Quoted text is the text, without marks of ours; a link is its words, not its address.
        assert!(gmail.contains("wrote:\nMorning Kofi,\n\nThanks for the notes."));
        assert!(gmail.contains("the written plan is good") && !gmail.contains("docs.juniperlabs.example"));
        // A table: a line for each row, a tab between its cells.
        let outlook = copy("outlook reply", 820.0);
        let table = "Lane\tCurrent\tProposed\nPort Sorrel to Halden\t1,840.00\t1,915.00\nHalden to Vesterby\t960.00\t1,020.00\nVesterby to Port Sorrel\t2,210.00\t2,210.00\n";
        assert!(outlook.contains(table), "{outlook}");
        // A list in a list: the sign that is painted for each depth.
        assert!(outlook.contains(
            "\u{2022}\tWhether the weekend pick-up window stays open in December:\n\u{25e6}\tSaturday only, as last year\n\u{25e6}\tBoth days"
        ));
        let receipt = copy("receipt", 820.0);
        assert!(receipt.contains("Order\tHB-20418\nPlaced\tMonday 12 October 2026\n"));
        assert!(receipt.contains("\nItem\tQty\tPrice\tTotal\n") && receipt.contains("Brass fountain pen, fine nib\t1\t48.00\t48.00\n"));
        // Cells that hold more than a line are columns of a page: each is lines of its own.
        let news = copy("newsletter", 820.0);
        assert!(
            news.contains("Merino crew\nMidweight, moss or slate\n") && news.contains("for members\nRidge boot, resoled\n\nRidge boot\n"),
            "{news}"
        );
        assert!(!news.contains('\t'), "{news}");
        // A picture that is not shown is what it says it shows; one that is shown is nothing.
        assert!(news.contains("\nTwo walkers on a ridge at first light\nAutumn on the ridge\n") && news.contains("\nPhotos Films Journal\n"));
        let shots = copy("screenshots", 820.0);
        // A picture in a line of text stands in that line, and is copied in it.
        assert!(shots.contains("would not paste: Log excerpt\nAnd this one"), "{shots}");
        assert!(shots.contains("anything: Damaged capture\n"), "{shots}");
        assert!(shots.contains("when I pick CSV:\n\nThe Delimiter box is empty") && !shots.contains("Picture"), "{shots}");
        // A plain-text message comes back as it was written, less the marks of its quotes.
        let plain = copy("plain thread", 820.0);
        let written: String = corpus::PLAIN_THREAD.lines().map(|l| l.trim_start_matches(['>', ' ']).trim_end()).collect::<Vec<_>>().join("\n");
        assert_eq!(squeezed(&plain), squeezed(&written));
        assert!(plain.contains("\n\n  - importer no longer stops at the first malformed row\n  - the --dry-run flag"), "{plain}");
        // A copy does not go by the room there is: a table whose cells stand one under the other
        // in a narrow pane is copied as when they stand side by side.
        for (name, _) in corpus_messages() {
            let wide = copy(name, 820.0);
            for width in [300.0, 150.0] {
                assert!(wide == copy(name, width), "{name} is copied differently at {width}");
            }
        }
    });
}

#[test]
fn a_paragraph_cut_for_its_length_is_copied_whole() {
    in_frame(false, |ctx| {
        // Cut at white space: the space that was dropped at each cut is back.
        let words = "word ".repeat(30_000);
        let m = message(0, ("A", "a@example.org"), "Long", None, words.clone());
        let laid = lay_out(ctx, &m, 700.0);
        assert!(laid.count() > 20);
        assert!(copy_of(&laid) == words.trim_end());
        // Cut inside a word, for want of white space: nothing is put in.
        let m = message(0, ("A", "a@example.org"), "Long", Some(format!("<p>{}</p>", "W".repeat(30_000))), String::new());
        let laid = lay_out(ctx, &m, 700.0);
        assert!(laid.count() >= 3 && copy_of(&laid) == "W".repeat(30_000));
        // Inside a table, where a paragraph is cut as it is laid out, and with other white space.
        let long = "tab\tand line ".repeat(1_500);
        let html = format!("<table border=1><tr><td>before</td><td><pre>{long}</pre></td><td>after</td></tr></table>");
        let m = message(0, ("A", "a@example.org"), "Long", Some(html), String::new());
        let laid = lay_out(ctx, &m, 700.0);
        assert!(laid.count() >= 5);
        let (copy, wanted) = (copy_of(&laid), format!("before\t{long}\tafter"));
        let differs = copy.chars().zip(wanted.chars()).position(|(a, b)| a != b);
        assert!(copy == wanted, "{} and {} characters, different at {differs:?}: {:?}", copy.len(), wanted.len(), copy.get(..80));
        // Its pieces are one paragraph (which is what three clicks take), and no others are.
        let laid = lay_out(ctx, &message(0, ("A", "a@example.org"), "Long", None, format!("First.\n\n{words}\n\nLast.")), 700.0);
        let (from, to) = (1, laid.count() - 2);
        assert!(to > from + 10);
        assert!((0..laid.count()).all(|seg| laid.continues(seg) == (seg > from && seg <= to)));
        assert_eq!(select::copied(&laid, Point { seg: from, at: 0 }, Point { seg: to, at: laid.len(to) }), words.trim_end());
    });
}

#[test]
fn places_and_text_agree_whatever_is_asked() {
    let alphabet: Vec<char> =
        "ab Z9_.,'-\n\t\u{e9}\u{df}\u{301}\u{316}\u{200b}\u{200d}\u{feff}\u{202e}\u{5e9}\u{645}\u{4e2d}\u{1f980}\u{1f469}\u{1f3fd}\u{2019}\u{a0}"
            .chars()
            .collect();
    let mut dice = Dice(0x5eed_1234_abcd_ef01);
    for round in 0..3_000 {
        let length = dice.below(if round % 10 == 0 { 400 } else { 24 });
        let text: String = (0..length).map(|_| alphabet[dice.below(alphabet.len())]).collect();
        let total = text.chars().count();
        // A slice by characters is those characters, wherever it is asked to begin and end.
        let (from, to) = (dice.below(total + 4), dice.below(total + 4));
        let wanted: String = text.chars().skip(from).take(to.saturating_sub(from)).collect();
        assert_eq!(select::char_slice(&text, from, to), wanted, "{text:?} {from}..{to}");
        assert_eq!(select::char_slice(&text, from, usize::MAX), text.chars().skip(from).collect::<String>());
        // A word is a stretch of the text that holds the character it was asked for, and every
        // character in it has the same word.
        let at = dice.below(total + 4);
        let word = select::word_range(&text, at);
        assert!(word.start <= word.end && word.end <= total, "{text:?} at {at}: {word:?}");
        if total > 0 {
            assert!(word.contains(&at.min(total - 1)), "{text:?} at {at}: {word:?}");
            for inside in word.clone() {
                assert_eq!(select::word_range(&text, inside), word, "{text:?} at {inside}");
            }
        } else {
            assert_eq!(word, 0..0);
        }
    }
    for (text, at, word) in [
        ("hello world", 2, "hello"),
        ("hello world", 5, " "),
        ("hello world", 99, "world"),
        ("we don't know", 5, "don't"),
        ("it\u{2019}s 83.20 or 1,000.", 5, "83.20"),
        ("it\u{2019}s 83.20 or 1,000.", 1, "it\u{2019}s"),
        ("it\u{2019}s 83.20 or 1,000.", 14, "1,000"),
        ("end. Next", 3, "."),
        ("a@b.example", 1, "@"),
        ("snake_case-word", 8, "snake_case"),
        ("ze\u{301}\u{316}ro width", 3, "ze\u{301}\u{316}ro"),
        ("\u{1f469}\u{200d}\u{1f680} crab", 1, "\u{1f469}\u{200d}\u{1f680}"),
        ("tabs\t\tand", 5, "\t\t"),
    ] {
        let range = select::word_range(text, at);
        assert_eq!(select::char_slice(text, range.start, range.end), word, "{text:?} at {at}");
    }
    // In messages as they are laid out: any two places, in range or far out of it.
    in_frame(false, |ctx| {
        for name in ["outlook reply", "newsletter", "screenshots", "strange text", "absurd sizes", "unclosed", "many pictures"] {
            let laid = lay_out(ctx, &corpus_message(name), 640.0);
            let count = laid.count();
            let place = |dice: &mut Dice| match dice.below(12) {
                0 => Point::END,
                1 => Point { seg: dice.below(count + 3), at: usize::MAX },
                2 => Point { seg: usize::MAX, at: dice.below(9) },
                _ => {
                    let seg = dice.below(count + 2);
                    Point { seg, at: dice.below(laid.len(seg) + 3) }
                }
            };
            for _ in 0..400 {
                let mut ends = [place(&mut dice), place(&mut dice), place(&mut dice)].map(|p| select::clamp(&laid, p).unwrap());
                ends.sort();
                let [a, b, c] = ends;
                // Clamped places are there, and stay where they are.
                for p in ends {
                    assert!(p.seg < count && p.at <= laid.len(p.seg) && select::clamp(&laid, p) == Some(p), "{name}: {p:?}");
                }
                // A copy in two parts holds what the copy in one holds.
                let (whole, first, second) = (select::copied(&laid, a, c), select::copied(&laid, a, b), select::copied(&laid, b, c));
                assert_eq!(squeezed(&whole), squeezed(&first) + &squeezed(&second), "{name}: {a:?} {b:?} {c:?}");
                assert!(whole.chars().count() >= first.chars().count().max(second.chars().count()));
                // And it is what the parts of the things between say they are.
                for seg in a.seg..=c.seg {
                    let part = select::part(a, c, seg, laid.len(seg));
                    assert!(part.as_ref().is_none_or(|p| p.start < p.end && p.end <= laid.len(seg)), "{name}: {part:?}");
                    if let (Some(part), Some(text)) = (part, laid.text(seg)) {
                        assert!(whole.contains(select::char_slice(text, part.start, part.end)), "{name}: {seg}");
                    }
                }
            }
            // Any point at all is somewhere in the message: on the thing it is on, else the nearest.
            for _ in 0..400 {
                let far = dice.below(5) == 0;
                let spread = |dice: &mut Dice, size: f32| {
                    if far { dice.below(40_000) as f32 - 20_000.0 } else { dice.below(size as usize + 60) as f32 - 30.0 }
                };
                let at = egui::pos2(spread(&mut dice, laid.size.x), spread(&mut dice, laid.size.y));
                let hit = select::hit(ctx, &laid, at).unwrap_or_else(|| panic!("{name}: nothing at {at:?}"));
                assert_eq!(select::clamp(&laid, hit.point), Some(hit.point), "{name}: {at:?}");
                assert!(hit.under.is_none_or(|under| under < laid.len(hit.point.seg)), "{name}: {hit:?}");
                let on = laid.items.iter().any(|item| item.seg != super::layout::NO_SEG && item.rect.contains(at));
                assert!(
                    !on || laid.segment(hit.point.seg).is_some_and(|hit| hit.rect.contains(at)),
                    "{name}: {at:?} is on a text and was taken for another"
                );
            }
            for odd in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, f32::MIN] {
                for at in [egui::pos2(odd, 10.0), egui::pos2(10.0, odd), egui::pos2(odd, odd)] {
                    let hit = select::hit(ctx, &laid, at);
                    assert!(hit.is_none_or(|hit| select::clamp(&laid, hit.point) == Some(hit.point)), "{name}: {at:?}");
                }
            }
        }
        // A message with nothing to select has no places at all.
        let empty = Laid::new(400.0);
        assert!(select::clamp(&empty, Point::START).is_none() && select::hit(ctx, &empty, egui::pos2(5.0, 5.0)).is_none());
        assert_eq!(select::copied(&empty, Point::START, Point::END), "");
    });
}

#[test]
fn selected_text_can_be_read_in_both_themes() {
    // Selected text is painted in one pair of colours whatever the message painted behind it.
    for t in [Tokens::light(), Tokens::dark()] {
        assert!(contrast(t.on_accent, t.accent) >= super::colors::TARGET_CONTRAST, "{:?} on {:?}", t.on_accent, t.accent);
    }
    // And it is painted: the rows of a text that is marked carry the mark and the new colour.
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let t = Tokens::get(ctx);
            let laid = lay_out(ctx, &corpus_message("colours"), 640.0);
            egui::Area::new(egui::Id::unique("marks")).show(ctx, |ui| {
                let marks = select::Marks::new(ui, &t);
                for item in &laid.items {
                    let ItemKind::Text(text) = &item.kind else { continue };
                    let plain = ctx.fonts_mut(|f| f.layout_job((*text.job).clone()));
                    let colours = |galley: &egui::Galley| -> Vec<Color32> {
                        galley.rows.iter().flat_map(|row| row.row.visuals.mesh.vertices.iter().map(|v| v.color)).collect()
                    };
                    // Past the end of the text is the end of it.
                    let mut marked = plain.clone();
                    marks.text(&mut marked, 1..usize::MAX);
                    if text.chars > 1 {
                        assert!(colours(&marked).contains(&t.accent) && colours(&marked).contains(&t.on_accent), "{:?}", text.job.text);
                        assert!(colours(&marked).len() > colours(&plain).len());
                    }
                    // Nothing at all is no mark, and leaves the text as it was.
                    let mut none = plain.clone();
                    marks.text(&mut none, 5..5);
                    marks.text(&mut none, usize::MAX..usize::MAX);
                    assert!(Arc::ptr_eq(&none, &plain));
                }
            });
        });
    }
}

// ---- selecting with the pointer and the keys ----

/// A window whose frames are a sixtieth of a second apart, as on a screen: two clicks, each a
/// frame down and a frame up, are a double click.
fn quick_window(app: MailApp, size: (f32, f32)) -> Window {
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(egui::vec2(size.0, size.1))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(3);
    harness
}

/// Long enough for the next click not to count as one more of the same row of clicks.
fn pause(harness: &mut Window) {
    harness.run_steps(30);
}

fn button(pos: egui::Pos2, button: egui::PointerButton, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::PointerButton { pos, button, pressed, modifiers }
}

/// Press, move there in a few steps, and let go. What the frames on the way asked of the system.
fn drag(harness: &mut Window, from: egui::Pos2, to: egui::Pos2) -> Vec<egui::OutputCommand> {
    let mut asked = Vec::new();
    let mut step = |harness: &mut Window, event: egui::Event| {
        harness.event(event);
        harness.step();
        asked.extend(harness.output().platform_output.commands.iter().cloned());
    };
    step(harness, egui::Event::PointerMoved(from));
    step(harness, button(from, egui::PointerButton::Primary, true, egui::Modifiers::NONE));
    for n in 1..=5 {
        step(harness, egui::Event::PointerMoved(from + (to - from) * (n as f32 / 5.0)));
    }
    step(harness, button(to, egui::PointerButton::Primary, false, egui::Modifiers::NONE));
    asked
}

/// So many clicks in a row at one place, a frame down and a frame up each.
fn clicks(harness: &mut Window, pos: egui::Pos2, count: usize, modifiers: egui::Modifiers) {
    harness.event(egui::Event::ModifiersChanged(modifiers));
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for _ in 0..count {
        for pressed in [true, false] {
            harness.event(button(pos, egui::PointerButton::Primary, pressed, modifiers));
            harness.step();
        }
    }
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    harness.step();
}

/// A click with the second button, and the frames it takes its menu to be there.
fn menu_click(harness: &mut Window, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(button(pos, egui::PointerButton::Secondary, pressed, egui::Modifiers::NONE));
        harness.step();
    }
    harness.run_steps(2);
}

/// Choose the item of the open menu that is called `label`: of all that is so called, what is nearest `near`.
fn choose(harness: &mut Window, label: &str, near: egui::Pos2) {
    let item = harness
        .query_all_by_label(label)
        .min_by(|a, b| a.rect().distance_sq_to_pos(near).total_cmp(&b.rect().distance_sq_to_pos(near)))
        .unwrap_or_else(|| panic!("nothing is called {label}"));
    item.click();
}

/// What an item of a menu is called: its name and its key, as they are read out.
fn keyed(label: &str, key: &str) -> String {
    format!("{label} {}", crate::widgets::key_text(key))
}

/// What the last frame handed to the clipboard, with the line ends of this system made plain.
fn clipboard(harness: &Window) -> Option<String> {
    harness.output().platform_output.commands.iter().rev().find_map(|c| match c {
        egui::OutputCommand::CopyText(text) => Some(text.replace("\r\n", "\n")),
        _ => None,
    })
}

/// Run frames until one hands something to the clipboard, a few at most.
fn copied_within(harness: &mut Window, frames: usize) -> Option<String> {
    for _ in 0..frames {
        harness.step();
        if let Some(text) = clipboard(harness) {
            return Some(text);
        }
    }
    None
}

/// Ctrl+C, as the system hands it on: what was copied.
fn copy_key(harness: &mut Window) -> Option<String> {
    harness.event(egui::Event::Copy);
    harness.step();
    clipboard(harness)
}

fn select_all_key(harness: &mut Window) {
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run_steps(2);
}

fn selection(harness: &Window, place: Place) -> Option<(Zone, Point, Point)> {
    harness.state().text_selection.at(place)
}

/// The text that starts with `start` in the message shown at `place`, and where on the screen
/// the gap in front of its character `n` is (the middle of the line).
fn gap_at(harness: &Window, place: Place, id: u64, start: &str, n: usize) -> (String, egui::Pos2) {
    let ctx = harness.ctx.clone();
    super::view::kept_at(&ctx, place, id, |laid, origin| {
        let item = find(laid, start);
        let ItemKind::Text(text) = &item.kind else { panic!() };
        let galley = ctx.fonts_mut(|f| f.layout_job((*text.job).clone()));
        let at = galley.pos_from_cursor(egui::text::CCursor::new(n));
        (text.job.text.clone(), origin + item.rect.min.to_vec2() + egui::vec2(text.anchor + at.center().x, at.center().y))
    })
    .unwrap_or_else(|| panic!("nothing is laid out at {place:?}"))
}

fn gap(harness: &Window, id: u64, start: &str, n: usize) -> egui::Pos2 {
    gap_at(harness, Place::Pane, id, start, n).1
}

/// The whole of the message in the reading pane as a copy has it, and where its top left is.
fn whole(harness: &Window, id: u64) -> (String, egui::Pos2) {
    super::view::kept(&harness.ctx, id, |laid, origin| (copy_of(laid), origin)).unwrap()
}

fn opened(asked: &[egui::OutputCommand]) -> usize {
    asked.iter().filter(|c| matches!(c, egui::OutputCommand::OpenUrl(_))).count()
}

#[test]
fn a_drag_selects_across_paragraphs_and_ctrl_c_copies() {
    for theme in ["light", "dark"] {
        let (mut app, ids) = corpus_app(false);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        let id = id_of(&ids, "gmail reply");
        let mut harness = window(app, (1280.0, 1400.0));
        select(&mut harness, id);
        // Nothing is selected: Ctrl+C copies nothing, and the command is not to be had.
        assert_eq!(copy_key(&mut harness), None);
        assert!(selection(&harness, Place::Pane).is_none() && !harness.state().enabled("ui.copySelection"));
        // Over the text, the pointer is the one that selects.
        let (from, to) = (gap(&harness, id, "Hi Priya", 3), gap(&harness, id, "Yes to both", 6));
        harness.event(egui::Event::PointerMoved(from));
        harness.run_steps(2);
        assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::Text);
        // A drag from one paragraph into the next: the end of the one, the blank line the
        // message has between them, the start of the other.
        assert_eq!(opened(&drag(&mut harness, from, to)), 0);
        assert_eq!(selection(&harness, Place::Pane).map(|s| s.0), Some(Zone::Body));
        assert_eq!(harness.state().text_selection.focus(), Some(Place::Pane));
        assert!(harness.state().enabled("ui.copySelection"));
        assert_eq!(copy_key(&mut harness).as_deref(), Some("Priya,\n\nYes to"));
        // It stays, frame after frame, and what the window says of it is how much, never what.
        harness.run_steps(5);
        assert_eq!(copy_key(&mut harness).as_deref(), Some("Priya,\n\nYes to"));
        let ctx = harness.ctx.clone();
        let said = crate::control::inspect(harness.state_mut(), &ctx);
        assert_eq!(said["textSelection"], json!([{"window": null, "message": id, "in": "body", "chars": 14, "keys": true}]));
        // Backwards is the same text.
        drag(&mut harness, to, from);
        assert_eq!(copy_key(&mut harness).as_deref(), Some("Priya,\n\nYes to"));
        // From the margin beside a line to the room under the message: all of it from that line on.
        let (all, origin) = whole(&harness, id);
        drag(&mut harness, origin + egui::vec2(-12.0, 4.0), egui::pos2(origin.x + 300.0, 1300.0));
        assert_eq!(copy_key(&mut harness).as_deref(), Some(all.as_str()));
        // Flagging the message makes a new message of it with the same body: the selection is its still.
        harness.state_mut().execute("mail.flag", json!({})).unwrap();
        harness.run_steps(3);
        assert_eq!(copy_key(&mut harness).as_deref(), Some(all.as_str()));
        // The script's way: the same command the key runs.
        assert_eq!(harness.state_mut().execute("ui.copySelection", json!({"pane": true})).unwrap()["chars"], all.chars().count());
        assert_eq!(copied_within(&mut harness, 2).as_deref(), Some(all.as_str()));
    }
}

#[test]
fn a_selection_copies_tables_lists_quotes_and_links_as_text() {
    let (app, ids) = corpus_app(false);
    let (gmail, outlook) = (id_of(&ids, "gmail reply"), id_of(&ids, "outlook reply"));
    let mut harness = window(app, (1280.0, 2000.0));
    // A row of a table: a tab between its cells.
    select(&mut harness, outlook);
    let (from, to) = (gap(&harness, outlook, "Port Sorrel to Halden", 0), gap(&harness, outlook, "1,915.00", 8));
    drag(&mut harness, from, to);
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Port Sorrel to Halden\t1,840.00\t1,915.00"));
    // Two rows, from inside a cell to inside a cell.
    let (from, to) = (gap(&harness, outlook, "Port Sorrel to Halden", 5), gap(&harness, outlook, "960.00", 3));
    drag(&mut harness, from, to);
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Sorrel to Halden\t1,840.00\t1,915.00\nHalden to Vesterby\t960"));
    // A list: an item taken from its start has its marker, one taken from inside does not.
    select(&mut harness, gmail);
    let (first, from) = gap_at(&harness, Place::Pane, gmail, "The demo should come", 0);
    let to = gap(&harness, gmail, "Can we give Tomasz", 6);
    drag(&mut harness, from, to);
    assert_eq!(copy_key(&mut harness), Some(format!("\u{2022}\t{first}\n\u{2022}\tCan we")));
    let from = gap(&harness, gmail, "The demo should come", 4);
    drag(&mut harness, from, to);
    assert_eq!(copy_key(&mut harness), Some(format!("{}\n\u{2022}\tCan we", &first[4..])));
    // Quoted text: its words, with the blank line it has, and no marks of ours.
    let (from, to) = (gap(&harness, gmail, "Morning Kofi", 8), gap(&harness, gmail, "Thanks for the notes", 6));
    drag(&mut harness, from, to);
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Kofi,\n\nThanks"));
    // A link: a drag that begins on it selects its words and follows nothing; the address is
    // not what is copied. A click on it still opens it.
    let ctx = harness.ctx.clone();
    let (start, range, url) = super::view::kept(&ctx, gmail, |laid, _| {
        let item = laid.items.iter().find(|i| matches!(&i.kind, ItemKind::Text(t) if !t.links.is_empty())).unwrap();
        let ItemKind::Text(text) = &item.kind else { panic!() };
        let (range, url) = text.links.first().unwrap();
        (text.job.text.clone(), range.clone(), url.to_string())
    })
    .unwrap();
    let (from, to) = (gap(&harness, gmail, &start, range.start), gap(&harness, gmail, &start, range.end));
    assert_eq!(opened(&drag(&mut harness, from, to)), 0);
    let words: String = start.chars().skip(range.start).take(range.len()).collect();
    assert!(!words.is_empty() && !words.contains("://"), "{words}");
    assert_eq!(copy_key(&mut harness), Some(words));
    harness.event(egui::Event::PointerMoved(from + egui::vec2(3.0, 0.0)));
    harness.run_steps(2);
    assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    let mut asked = Vec::new();
    for pressed in [true, false] {
        harness.event(button(from + egui::vec2(3.0, 0.0), egui::PointerButton::Primary, pressed, egui::Modifiers::NONE));
        harness.step();
        asked.extend(harness.output().platform_output.commands.iter().cloned());
    }
    assert!(matches!(asked.as_slice(), [egui::OutputCommand::OpenUrl(open)] if open.url == url), "{asked:?}");
    // ...and, being a click, lets go of what was selected.
    assert!(selection(&harness, Place::Pane).is_none());
}

#[test]
fn two_clicks_take_a_word_three_a_paragraph_and_shift_moves_the_end() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "gmail reply");
    let mut harness = quick_window(app, (1280.0, 1400.0));
    select(&mut harness, id);
    let (text, _) = gap_at(&harness, Place::Pane, id, "Yes to both", 0);
    let word = text.find("projector").unwrap();
    // On the word's fourth letter, and on the right half of its last: its own word either way.
    for letter in [3, 8] {
        let on = gap(&harness, id, "Yes to both", word + letter) + egui::vec2(if letter == 8 { 4.0 } else { 1.5 }, 0.0);
        clicks(&mut harness, on, 2, egui::Modifiers::NONE);
        assert_eq!(copy_key(&mut harness).as_deref(), Some("projector"), "letter {letter}");
        pause(&mut harness);
    }
    let on = gap(&harness, id, "Yes to both", word + 3) + egui::vec2(1.5, 0.0);
    clicks(&mut harness, on, 3, egui::Modifiers::NONE);
    assert_eq!(copy_key(&mut harness).as_deref(), Some(text.as_str()));
    // One click lets go of it.
    pause(&mut harness);
    clicks(&mut harness, on, 1, egui::Modifiers::NONE);
    assert!(selection(&harness, Place::Pane).is_none());
    assert_eq!(copy_key(&mut harness), None);
    // Shift and a click move the end of what is selected, forwards and back past its start.
    pause(&mut harness);
    let (from, to) = (gap(&harness, id, "Hi Priya", 3), gap(&harness, id, "Hi Priya", 8));
    drag(&mut harness, from, to);
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Priya"));
    pause(&mut harness);
    let (further, back) = (gap(&harness, id, "Yes to both", 3), gap(&harness, id, "Hi Priya", 0));
    clicks(&mut harness, further, 1, egui::Modifiers::SHIFT);
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Priya,\n\nYes"));
    pause(&mut harness);
    clicks(&mut harness, back, 1, egui::Modifiers::SHIFT);
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Hi "));
    // A picture is taken whole by two clicks, and is nothing in a copy when it is shown.
    let shots = id_of(&ids, "screenshots");
    select(&mut harness, shots);
    harness.run_steps(10);
    let picture = super::view::kept(&harness.ctx, shots, |laid, origin| shown_pictures(laid)[0].rect.translate(origin.to_vec2())).unwrap();
    pause(&mut harness);
    clicks(&mut harness, picture.center().min(egui::pos2(1200.0, 1300.0)), 2, egui::Modifiers::NONE);
    let taken = selection(&harness, Place::Pane).unwrap();
    assert_eq!((taken.1.seg, taken.1.at, taken.2.at), (taken.2.seg, 0, 1));
    assert_eq!(copy_key(&mut harness), None);
}

#[test]
fn the_header_is_selected_and_copied_field_by_field_and_across() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "outlook reply");
    let mut harness = quick_window(app, (1280.0, 900.0));
    select(&mut harness, id);
    let m = harness.state().session.mailbox.messages.get(&id).cloned().unwrap();
    let when = mailcraft_time::header_label(m.date, harness.state().session.offset_at(m.date), harness.state().session.prefs.twelve_hour);
    let (subject, sender, address, to) = (
        "RE: Q4 lane pricing: revised schedule",
        "Marta Lindqvist",
        "<marta.lindqvist@tidewater-freight.example>",
        "To: Avery Quinn <avery@lumen-works.example>",
    );
    let field =
        |harness: &Window, text: &str| harness.query_all_by_label(text).next().unwrap_or_else(|| panic!("the header does not say {text}")).rect();
    // From the first letter of a field to its last.
    let ends = |harness: &Window, text: &str| {
        let rect = field(harness, text);
        (rect.left_center() + egui::vec2(1.0, 0.0), rect.right_center() - egui::vec2(1.0, 0.0))
    };
    for text in [subject, sender, address, when.as_str(), to] {
        let (from, to) = ends(&harness, text);
        // The pointer is the one that selects over these, too.
        harness.event(egui::Event::PointerMoved(from));
        harness.run_steps(2);
        assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::Text, "{text}");
        drag(&mut harness, from, to);
        assert_eq!(selection(&harness, Place::Pane).map(|s| s.0), Some(Zone::Header), "{text}");
        assert_eq!(copy_key(&mut harness).as_deref(), Some(text));
        pause(&mut harness);
    }
    // Two clicks take a word of a field, three the field.
    let (on_subject, on_address) = (ends(&harness, subject).0 + egui::vec2(2.0, 0.0), field(&harness, address).center());
    clicks(&mut harness, on_subject, 2, egui::Modifiers::NONE);
    assert_eq!(copy_key(&mut harness).as_deref(), Some("RE"));
    pause(&mut harness);
    clicks(&mut harness, on_address, 3, egui::Modifiers::NONE);
    assert_eq!(copy_key(&mut harness).as_deref(), Some(address));
    // Across fields, in the order they are read in: the name, its address beside it, the date
    // (which stands at the far right of that line), the recipients.
    pause(&mut harness);
    let (of_subject, of_sender, of_date, of_to) = (ends(&harness, subject), ends(&harness, sender), ends(&harness, &when), ends(&harness, to));
    drag(&mut harness, of_sender.0, of_to.1);
    assert_eq!(copy_key(&mut harness), Some(format!("{sender} {address}\n{when}\n{to}")));
    pause(&mut harness);
    drag(&mut harness, of_sender.0, of_date.1);
    assert_eq!(copy_key(&mut harness), Some(format!("{sender} {address}\n{when}")));
    pause(&mut harness);
    drag(&mut harness, of_subject.0, of_to.1);
    assert_eq!(copy_key(&mut harness), Some(format!("{subject}\n{sender} {address}\n{when}\n{to}")));
    // A selection is in the header or in the body: one in the body takes its place, and back.
    pause(&mut harness);
    let (from, to_body) = (gap(&harness, id, "Hi Owen", 0), gap(&harness, id, "Hi Owen", 2));
    drag(&mut harness, from, to_body);
    assert_eq!((selection(&harness, Place::Pane).map(|s| s.0), copy_key(&mut harness).as_deref()), (Some(Zone::Body), Some("Hi")));
    // The menu of a field with nothing selected in the header is about that field.
    pause(&mut harness);
    let on = field(&harness, address).center();
    menu_click(&mut harness, on);
    assert_eq!(selection(&harness, Place::Pane).map(|s| s.0), Some(Zone::Header));
    choose(&mut harness, &keyed("Copy", "Mod+C"), on);
    assert_eq!(copied_within(&mut harness, 3).as_deref(), Some(address));
    // A press in the header that lands on none of its texts lets go.
    pause(&mut harness);
    let beside = egui::pos2(field(&harness, to).right() + 120.0, field(&harness, to).center().y);
    clicks(&mut harness, beside, 1, egui::Modifiers::NONE);
    assert!(selection(&harness, Place::Pane).is_none());
    // A field that is cut short for want of room is copied whole when it is selected to its end.
    harness.set_size(egui::vec2(760.0, 700.0));
    harness.run_steps(4);
    pause(&mut harness);
    let cut_short = harness.query_all_by_label(address).next().map(|node| node.rect()).expect("the address is still said, though not shown whole");
    clicks(&mut harness, cut_short.center(), 3, egui::Modifiers::NONE);
    assert_eq!(copy_key(&mut harness).as_deref(), Some(address));
}

#[test]
fn escape_a_click_and_another_message_let_go_of_the_selection() {
    let (app, ids) = corpus_app(false);
    let (id, other) = (id_of(&ids, "gmail reply"), id_of(&ids, "receipt"));
    let mut harness = window(app, (1280.0, 1400.0));
    select(&mut harness, id);
    let take = |harness: &mut Window| {
        let (from, to) = (gap(harness, id, "Hi Priya", 3), gap(harness, id, "Yes to both", 6));
        drag(harness, from, to);
        assert!(selection(harness, Place::Pane).is_some());
    };
    // Escape.
    take(&mut harness);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(selection(&harness, Place::Pane).is_none() && copy_key(&mut harness).is_none());
    // A click on the room under the message, and one in the margin beside it.
    let origin = whole(&harness, id).1;
    for empty in [egui::pos2(origin.x + 200.0, 1300.0), origin + egui::vec2(-14.0, 30.0)] {
        take(&mut harness);
        click_at(&mut harness, empty);
        assert!(selection(&harness, Place::Pane).is_none(), "{empty:?}");
    }
    // Another message: what was selected in this one is not selected in that one, nor when this one is back.
    take(&mut harness);
    select(&mut harness, other);
    assert!(selection(&harness, Place::Pane).is_none() && copy_key(&mut harness).is_none());
    select(&mut harness, id);
    assert!(selection(&harness, Place::Pane).is_none() && copy_key(&mut harness).is_none());
    // No message at all, then this one again.
    take(&mut harness);
    harness.state_mut().execute("mail.select", json!({"ids": []})).unwrap();
    harness.run_steps(3);
    select(&mut harness, id);
    assert!(selection(&harness, Place::Pane).is_none());
    // The same message with another body (it was edited, or its text came from the server): a
    // selection made in the old one is not carried into the new one.
    take(&mut harness);
    harness.state_mut().session.mailbox.message_mut(id).unwrap().body.html = Some("<p>Short.</p>".into());
    harness.run_steps(3);
    assert!(selection(&harness, Place::Pane).is_none() && copy_key(&mut harness).is_none());
    assert_eq!(whole(&harness, id).0, "Short.");
}

/// Escape in the search box is the box's (#365): a selection in the message being read stays,
/// when Escape is held down too.
#[test]
fn escape_in_the_search_box_leaves_the_selection() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "gmail reply");
    let mut harness = window(app, (1280.0, 1400.0));
    select(&mut harness, id);
    let (from, to) = (gap(&harness, id, "Hi Priya", 3), gap(&harness, id, "Yes to both", 6));
    drag(&mut harness, from, to);
    harness.state_mut().execute("ui.focusSearch", json!({})).unwrap();
    harness.run_steps(3);
    assert!(selection(&harness, Place::Pane).is_some());
    for repeat in [false, true, true] {
        harness.event(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat, modifiers: egui::Modifiers::NONE });
        harness.run_steps(2);
        assert!(selection(&harness, Place::Pane).is_some(), "repeat: {repeat}");
    }
}

#[test]
fn ctrl_a_selects_the_text_of_the_message_when_the_keys_are_its() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "gmail reply");
    let mut harness = window(app, (1280.0, 1400.0));
    select(&mut harness, id);
    let picked = |harness: &Window| harness.state().session.sel.messages.len();
    let (all, origin) = whole(&harness, id);
    // The pointer went down in the message: Ctrl+A is its text, and the list's selection is as it was.
    click_at(&mut harness, origin + egui::vec2(-14.0, 30.0));
    assert_eq!(harness.state().text_selection.focus(), Some(Place::Pane));
    select_all_key(&mut harness);
    assert_eq!(selection(&harness, Place::Pane), Some((Zone::Body, Point::START, Point::END)));
    assert_eq!((picked(&harness), copy_key(&mut harness).as_deref()), (1, Some(all.as_str())));
    // ...wherever the pointer has gone to since, as long as it did not go down elsewhere.
    harness.event(egui::Event::PointerMoved(harness.state().hit.rect("message", id).unwrap().center()));
    harness.run_steps(2);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(selection(&harness, Place::Pane).is_none());
    select_all_key(&mut harness);
    assert_eq!((picked(&harness), selection(&harness, Place::Pane).map(|s| s.2)), (1, Some(Point::END)));
    // A click in the list gives the keys back to it: Ctrl+A is all its messages.
    let row = harness.state().hit.rect("message", id).unwrap().center();
    click_at(&mut harness, row);
    assert_eq!(harness.state().text_selection.focus(), None);
    select_all_key(&mut harness);
    assert_eq!(picked(&harness), ids.len());
    // With the pointer over the message, it is the message's again, without a click.
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(4);
    harness.key_press(egui::Key::Escape);
    harness.event(egui::Event::PointerMoved(origin + egui::vec2(100.0, 40.0)));
    harness.run_steps(3);
    assert!(selection(&harness, Place::Pane).is_none() && harness.state().text_selection.focus().is_none());
    select_all_key(&mut harness);
    assert_eq!((picked(&harness), copy_key(&mut harness).as_deref()), (1, Some(all.as_str())));
    // While the search box has the keyboard, Ctrl+A and Ctrl+C are its own.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    harness.state_mut().execute("ui.focusSearch", json!({})).unwrap();
    harness.run_steps(3);
    if harness.ctx.text_edit_focused() {
        select_all_key(&mut harness);
        assert!(selection(&harness, Place::Pane).is_none());
    }
}

#[test]
fn every_message_of_the_corpus_is_selected_and_copied_as_it_is_laid_out() {
    for theme in ["light", "dark"] {
        let (mut app, ids) = corpus_app(false);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        let mut harness = window(app, (1100.0, 760.0));
        for (name, id) in &ids {
            harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
            let mut frames = 0;
            while super::view::kept(&harness.ctx, *id, |laid, _| laid.done) != Some(true) {
                harness.step();
                frames += 1;
                assert!(frames < 400, "{name} was never laid out");
            }
            harness.run_steps(2);
            let (all, origin) = whole(&harness, *id);
            assert!(!all.is_empty(), "{name}");
            // By the keys: all of it.
            click_at(&mut harness, origin + egui::vec2(-14.0, 6.0));
            select_all_key(&mut harness);
            let copied = copy_key(&mut harness).unwrap_or_else(|| panic!("{theme} {name}: nothing was copied"));
            assert!(copied == all, "{theme} {name}: {} characters were copied of {}", copied.len(), all.len());
            assert_eq!(harness.state().session.sel.messages.len(), 1, "{name}");
            // By a drag from its first line down past the foot of the window, held there while
            // the message scrolls under it: a stretch of it from the top, more than a screenful
            // when there is more.
            harness.key_press(egui::Key::Escape);
            harness.run_steps(2);
            // (From just beside the first thing in it: text begins at the left edge, a picture
            // that leads a message may stand in the middle of its line.)
            let first =
                super::view::kept(&harness.ctx, *id, |laid, _| laid.segment(0).map(|item| item.rect.min.to_vec2())).flatten().unwrap_or_default();
            let beside = origin + first + egui::vec2(-12.0, 2.0);
            harness.event(egui::Event::PointerMoved(beside));
            harness.step();
            harness.event(button(beside, egui::PointerButton::Primary, true, egui::Modifiers::NONE));
            harness.step();
            let below = egui::pos2(origin.x + 500.0, 4_000.0);
            harness.event(egui::Event::PointerMoved(below));
            harness.run_steps(12);
            harness.event(button(below, egui::PointerButton::Primary, false, egui::Modifiers::NONE));
            harness.run_steps(2);
            let part = copy_key(&mut harness).unwrap_or_else(|| panic!("{theme} {name}: a drag copied nothing"));
            assert!(all.starts_with(&part), "{theme} {name}: what a drag from the top copied is not the start of the message");
            let moved = origin.y - whole(&harness, *id).1.y;
            assert!(part.len() * 4 > all.len() || moved > 400.0, "{theme} {name}: {} of {} characters, scrolled {moved}", part.len(), all.len());
        }
    }
}

#[test]
fn a_drag_to_the_edge_scrolls_the_message_under_it() {
    let text: String =
        (0..300).map(|i| format!("Paragraph {i} of a long message, with enough words in it to fill a line or two of the pane.\n\n")).collect();
    let (app, ids) = app_with(vec![message(0, ("A", "a@example.org"), "Long", None, text)]);
    let id = ids[0];
    let mut harness = window(app, (1100.0, 700.0));
    select(&mut harness, id);
    let top = |harness: &Window| whole(harness, id).1.y;
    let (from, start) = (gap(&harness, id, "Paragraph 0 of", 3), top(&harness));
    harness.event(egui::Event::PointerMoved(from));
    harness.step();
    harness.event(button(from, egui::PointerButton::Primary, true, egui::Modifiers::NONE));
    harness.step();
    // In the middle of the pane nothing scrolls.
    harness.event(egui::Event::PointerMoved(from + egui::vec2(60.0, 120.0)));
    harness.run_steps(4);
    assert_eq!(top(&harness), start);
    // At its foot it does, for as long as the pointer is held there, without it moving.
    harness.event(egui::Event::PointerMoved(egui::pos2(from.x + 60.0, 692.0)));
    harness.run_steps(3);
    let after_three = top(&harness);
    harness.run_steps(6);
    let after_nine = top(&harness);
    assert!(after_three < start - 20.0 && after_nine < after_three - 40.0, "{start} {after_three} {after_nine}");
    // Further out it goes faster; back at the top edge it goes back.
    harness.event(egui::Event::PointerMoved(egui::pos2(from.x + 60.0, 2_500.0)));
    harness.run_steps(6);
    let far = top(&harness);
    assert!(after_nine - far > (after_three - after_nine) * 2.0, "{after_nine} {far}");
    harness.event(egui::Event::PointerMoved(egui::pos2(from.x + 60.0, -800.0)));
    harness.run_steps(8);
    assert!(top(&harness) > far + 100.0, "{start} {after_three} {after_nine} {far} {}", top(&harness));
    // Let go at the foot again: everything from where the drag began to where it ended, the
    // paragraphs that were never on screen at the start among them.
    harness.event(egui::Event::PointerMoved(egui::pos2(from.x + 60.0, 2_500.0)));
    harness.run_steps(8);
    harness.event(button(egui::pos2(from.x + 60.0, 2_500.0), egui::PointerButton::Primary, false, egui::Modifiers::NONE));
    harness.run_steps(2);
    let stopped = top(&harness);
    harness.run_steps(5);
    assert_eq!(top(&harness), stopped, "it scrolls on after the button is up");
    let copied = copy_key(&mut harness).unwrap();
    assert!(copied.starts_with("agraph 0 of a long message") && copied.contains("\n\nParagraph 30 of"), "{:?}", copied.get(..60));
}

#[test]
fn a_message_window_has_a_selection_of_its_own() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "gmail reply");
    let mut harness = window(app, (1400.0, 1000.0));
    select(&mut harness, id);
    let (from, to) = (gap(&harness, id, "Hi Priya", 3), gap(&harness, id, "Hi Priya", 8));
    drag(&mut harness, from, to);
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Priya"));
    let pane = selection(&harness, Place::Pane);
    let subjects = |harness: &Window| harness.query_all_by_label("Re: Agenda for Friday").map(|node| node.rect()).collect::<Vec<_>>();
    let in_pane = subjects(&harness);
    // The same message in a window: nothing is selected there.
    let own = harness.state_mut().execute("ui.openWindow", json!({"message": id})).unwrap()["window"].as_u64().unwrap();
    harness.run_steps(8);
    let there = Place::Window(own);
    assert!(selection(&harness, there).is_none());
    let (from, to) = (gap_at(&harness, there, id, "Hi Priya", 0).1, gap_at(&harness, there, id, "Hi Priya", 2).1);
    drag(&mut harness, from, to);
    // The keys are the window's now, and what was selected in the pane is selected there still.
    assert_eq!(harness.state().text_selection.focus(), Some(there));
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Hi"));
    assert_eq!(selection(&harness, Place::Pane), pane);
    // A script says which it means.
    for (params, text) in [(json!({"pane": true}), "Priya"), (json!({"window": own}), "Hi"), (json!({}), "Hi")] {
        harness.state_mut().execute("ui.copySelection", params).unwrap();
        assert_eq!(copied_within(&mut harness, 2).as_deref(), Some(text));
    }
    let ctx = harness.ctx.clone();
    let said = crate::control::inspect(harness.state_mut(), &ctx);
    assert_eq!(said["textSelection"].as_array().map(Vec::len), Some(2), "{}", said["textSelection"]);
    // Ctrl+A is the window's text: neither the pane's nor the messages of the list.
    select_all_key(&mut harness);
    assert_eq!(selection(&harness, there), Some((Zone::Body, Point::START, Point::END)));
    assert_eq!((selection(&harness, Place::Pane), harness.state().session.sel.messages.len()), (pane, 1));
    let all = super::view::kept_at(&harness.ctx, there, id, |laid, _| copy_of(laid)).unwrap();
    assert_eq!(copy_key(&mut harness).as_deref(), Some(all.as_str()));
    // The window's header is its own as well.
    // (Of what says the subject now, the lowest that the pane did not say: under the window's title.)
    let subject = subjects(&harness).into_iter().filter(|rect| !in_pane.contains(rect)).max_by(|a, b| a.min.y.total_cmp(&b.min.y)).unwrap();
    drag(&mut harness, subject.left_center() + egui::vec2(1.0, 0.0), subject.right_center() - egui::vec2(1.0, 0.0));
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Re: Agenda for Friday"));
    // A window that is closed leaves nothing selected behind for the next one.
    harness.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();
    harness.run_steps(4);
    let again = harness.state_mut().execute("ui.openWindow", json!({"message": id})).unwrap()["window"].as_u64().unwrap();
    harness.run_steps(6);
    assert!(selection(&harness, Place::Window(again)).is_none());
    assert_eq!(selection(&harness, Place::Pane), pane);
}

#[test]
fn the_menu_of_the_body_copies_selects_all_and_knows_a_links_address() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "gmail reply");
    let mut harness = window(app, (1280.0, 1400.0));
    select(&mut harness, id);
    let named = |harness: &Window, label: &str| harness.query_all_by_label(label).count();
    let (copy, all_of_it) = (keyed("Copy", "Mod+C"), keyed("Select All", "Mod+A"));
    let before = (named(&harness, &copy), named(&harness, &all_of_it));
    assert_eq!(named(&harness, "Copy Link Address"), 0);
    // On plain text with nothing selected: Copy is there and does nothing, Select All selects.
    let plain = gap(&harness, id, "Hi Priya", 2);
    menu_click(&mut harness, plain);
    assert_eq!((named(&harness, &copy), named(&harness, &all_of_it), named(&harness, "Copy Link Address")), (before.0 + 1, before.1 + 1, 0));
    choose(&mut harness, &copy, plain);
    assert_eq!(copied_within(&mut harness, 3), None);
    menu_click(&mut harness, plain);
    choose(&mut harness, &all_of_it, plain);
    harness.run_steps(3);
    assert_eq!(selection(&harness, Place::Pane), Some((Zone::Body, Point::START, Point::END)));
    assert_eq!(named(&harness, &all_of_it), before.1, "the menu shuts when something is chosen");
    // With a selection, Copy copies it; the menu did not let go of it.
    let all = whole(&harness, id).0;
    menu_click(&mut harness, plain);
    assert_eq!(selection(&harness, Place::Pane).map(|s| s.2), Some(Point::END));
    choose(&mut harness, &copy, plain);
    assert_eq!(copied_within(&mut harness, 3).as_deref(), Some(all.as_str()));
    // On a link: its address as well, the one it really leads to.
    let ctx = harness.ctx.clone();
    let (start, range, url) = super::view::kept(&ctx, id, |laid, _| {
        let item = laid.items.iter().find(|i| matches!(&i.kind, ItemKind::Text(t) if !t.links.is_empty())).unwrap();
        let ItemKind::Text(text) = &item.kind else { panic!() };
        let (range, url) = text.links.first().unwrap();
        (text.job.text.clone(), range.clone(), url.to_string())
    })
    .unwrap();
    let on_link = gap(&harness, id, &start, range.start + 1) + egui::vec2(1.0, 0.0);
    // Over the link the pointer is told where it leads; not once the menu is open there, which
    // the note would lie over.
    harness.event(egui::Event::PointerMoved(on_link));
    harness.run_steps(3);
    let tip = super::view::link_tip(&url);
    assert_eq!(named(&harness, &tip), 1);
    menu_click(&mut harness, on_link);
    assert_eq!((named(&harness, "Copy Link Address"), named(&harness, &tip)), (1, 0));
    choose(&mut harness, "Copy Link Address", on_link);
    assert_eq!(copied_within(&mut harness, 3), Some(url));
    // Nothing was followed for any of that.
    assert!(harness.output().platform_output.commands.iter().all(|c| !matches!(c, egui::OutputCommand::OpenUrl(_))));
}

#[test]
fn pictures_that_arrive_and_a_new_width_keep_what_is_selected() {
    let (mut app, ids) = corpus_app(false);
    let _host = Host::answering(&mut app, newsletter_picture);
    let id = id_of(&ids, "newsletter");
    let mut harness = window(app, (1280.0, 1600.0));
    select(&mut harness, id);
    assert_eq!(shown(&harness, id), (0, 8, 8));
    let (from, to) = (gap(&harness, id, "The first frost", 4), gap(&harness, id, "Members get first pick", 7));
    drag(&mut harness, from, to);
    let taken = selection(&harness, Place::Pane);
    let text = copy_key(&mut harness).unwrap();
    assert!(text.starts_with("first frost came on Tuesday.") && text.ends_with("\nMembers"), "{text}");
    // The pictures come: the message is laid out again around them, and the same words are selected.
    harness.state_mut().execute("ui.downloadPictures", json!({"message": id})).unwrap();
    harness.run_steps(16);
    assert_eq!(shown(&harness, id), (8, 0, 0));
    assert_eq!((selection(&harness, Place::Pane), copy_key(&mut harness)), (taken, Some(text.clone())));
    // So are they at another width, where the lines break elsewhere.
    harness.set_size(egui::vec2(820.0, 1600.0));
    harness.run_steps(6);
    assert_eq!((selection(&harness, Place::Pane), copy_key(&mut harness)), (taken, Some(text)));
    // All of it, with and without its pictures: their descriptions are copied only while they
    // are not shown.
    let all = whole(&harness, id).0;
    assert!(!all.contains("Two walkers on a ridge") && all.contains("Autumn on the ridge"));
    harness.state_mut().execute("ui.selectAllInMessage", json!({"pane": true})).unwrap();
    harness.run_steps(2);
    assert_eq!(copy_key(&mut harness), Some(all));
    let (mut app, ids) = corpus_app(false);
    let host = Host::holding(&mut app);
    let id = id_of(&ids, "newsletter");
    let mut harness = window(app, (1280.0, 1600.0));
    select(&mut harness, id);
    harness.state_mut().execute("ui.selectAllInMessage", json!({"pane": true})).unwrap();
    harness.run_steps(2);
    let before = copy_key(&mut harness).unwrap();
    assert!(before.contains("Two walkers on a ridge at first light"));
    // They arrive one by one under the selection, which stays "everything".
    harness.state_mut().execute("ui.downloadPictures", json!({"message": id})).unwrap();
    for _ in 0..8 {
        harness.run_steps(6);
        host.release(1, || Ok(png(64, 48)));
        harness.run_steps(6);
        assert_eq!(selection(&harness, Place::Pane), Some((Zone::Body, Point::START, Point::END)));
        assert_eq!(copy_key(&mut harness), Some(whole(&harness, id).0));
    }
    assert_eq!(shown(&harness, id).0, 8);
}

#[test]
fn a_copy_of_a_message_that_is_still_being_laid_out_waits_for_all_of_it() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "one paragraph");
    let mut harness = window(app, (1280.0, 800.0));
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(2);
    assert_eq!(super::view::kept(&harness.ctx, id, |laid, _| laid.done), Some(false));
    // Everything is selected, though not everything is there yet; a copy of it is not made of the part that is.
    harness.state_mut().execute("ui.selectAllInMessage", json!({"pane": true})).unwrap();
    assert_eq!(harness.state_mut().execute("ui.copySelection", json!({"pane": true})).unwrap(), json!({"pending": true}));
    let mut copied = None;
    for _ in 0..200 {
        harness.step();
        let done = super::view::kept(&harness.ctx, id, |laid, _| laid.done) == Some(true);
        if let Some(text) = clipboard(&harness) {
            assert!(done, "a part of the message was copied as the whole of it");
            copied = Some(text);
            break;
        }
    }
    let copied = copied.expect("the copy was never made");
    assert!(copied == whole(&harness, id).0 && copied.len() > 1_000_000);
    // It is made once.
    assert_eq!(copied_within(&mut harness, 5), None);
    // A part that is laid out already is copied at once, whatever is still to come.
    let big = id_of(&ids, "many breaks");
    harness.state_mut().execute("mail.select", json!({"id": big})).unwrap();
    harness.run_steps(2);
    assert_eq!(super::view::kept(&harness.ctx, big, |laid, _| laid.done), Some(false));
    let (from, to) = (gap(&harness, big, "line", 0), gap(&harness, big, "line", 4));
    drag(&mut harness, from, to);
    assert_eq!(copy_key(&mut harness).as_deref(), Some("line"));
}

#[test]
fn the_selection_commands_take_anything_and_never_panic() {
    let (mut app, ids) = corpus_app(false);
    // Before there is a window, and with nothing selected.
    for command in ["ui.copySelection", "ui.selectAllInMessage", "ui.copyLinkAddress"] {
        assert!(app.execute(command, json!({})).is_err(), "{command}");
    }
    let junk = [
        json!(null),
        json!([]),
        json!("text"),
        json!(7),
        json!({"window": "x"}),
        json!({"window": -1}),
        json!({"window": 0}),
        json!({"window": u64::MAX}),
        json!({"window": 1e300}),
        json!({"window": {"id": 1}}),
        json!({"pane": "yes"}),
        json!({"pane": false}),
        json!({"pane": true, "window": 3}),
        json!({"url": 5}),
        json!({"url": ""}),
        json!({"url": "javascript:alert(1)"}),
        json!({"url": "cid:part@example.org"}),
        json!({"url": "data:text/html,<script>alert(1)</script>"}),
        json!({"url": "file:///C:/Users/owner/secrets.txt"}),
        json!({"url": "https://example.org/a\r\nX-Injected: 1"}),
        json!({"url": format!("https://example.org/{}", "a".repeat(20_000))}),
        json!({"url": "relative/path"}),
    ];
    let id = id_of(&ids, "strange text");
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    for command in ["ui.copySelection", "ui.selectAllInMessage", "ui.copyLinkAddress"] {
        for params in &junk {
            let _ = harness.state_mut().execute(command, params.clone());
            let _ = harness.state_mut().run(command, params.clone());
            harness.step();
        }
    }
    // Only an address a link may have is copied as one.
    for refused in junk.iter().filter(|p| p.get("url").is_some()) {
        assert!(harness.state_mut().execute("ui.copyLinkAddress", refused.clone()).is_err(), "{refused}");
    }
    assert_eq!(copied_within(&mut harness, 2), None);
    for address in ["https://example.org/page?x=1", "mailto:ada@example.org", "tel:+15550100", " http://example.org/ "] {
        assert_eq!(harness.state_mut().execute("ui.copyLinkAddress", json!({"url": address})).unwrap()["chars"], address.trim().len());
        assert_eq!(copied_within(&mut harness, 2).as_deref(), Some(address.trim()));
    }
    // A window that is not there has nothing to select or copy; nor has a pane that shows no message.
    assert!(harness.state_mut().execute("ui.selectAllInMessage", json!({"window": 77})).is_err());
    assert!(harness.state_mut().execute("ui.copySelection", json!({"window": 77})).is_err());
    harness.state_mut().execute("ui.selectAllInMessage", json!({"pane": true})).unwrap();
    harness.state_mut().execute("mail.select", json!({"ids": []})).unwrap();
    harness.run_steps(3);
    assert!(harness.state_mut().execute("ui.selectAllInMessage", json!({"pane": true})).is_err());
    // Strange text, selected character by character from every place in it: never a panic, and
    // never a cut inside a character.
    select(&mut harness, id);
    let places: Vec<(egui::Pos2, usize)> = super::view::kept(&harness.ctx, id, |laid, origin| {
        laid.items.iter().filter(|i| i.seg != super::layout::NO_SEG).map(|i| (origin + i.rect.min.to_vec2(), i.rect.width() as usize)).collect()
    })
    .unwrap();
    let mut dice = Dice(0xfeed_beef_cafe_d00d);
    for _ in 0..40 {
        let (from, to) = (places[dice.below(places.len())], places[dice.below(places.len())]);
        let at =
            |dice: &mut Dice, (corner, width): (egui::Pos2, usize)| corner + egui::vec2(dice.below(width.min(600) + 1) as f32, dice.below(30) as f32);
        let (from, to) = (at(&mut dice, from), at(&mut dice, to));
        drag(&mut harness, from, to);
        let copied = copy_key(&mut harness);
        assert!(copied.as_ref().is_none_or(|text| whole(&harness, id).0.contains(text.as_str())), "{copied:?}");
        if dice.below(4) == 0 {
            clicks(&mut harness, to, 2 + dice.below(2), egui::Modifiers::NONE);
            let _ = copy_key(&mut harness);
        }
    }
}

// ---- meetings that arrive as mail ----

mod invitation;
mod remote_server;

/// The original under a reply to HTML mail is laid out as the reading pane lays it out (its
/// heading, its list), not as text; and a remote picture in it stays a stand-in until the owner
/// asks for the original's pictures.
#[test]
fn the_original_under_a_reply_is_shown_as_the_reading_pane_shows_it() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0), Services::default());
    app.autosave = false;
    let id = app.session.mailbox.messages.values().find(|m| m.subject.starts_with("Five new arrivals")).map(|m| m.id).unwrap();
    app.execute("mail.select", json!({"id": id})).unwrap();
    app.execute("mail.reply", json!({"id": id})).unwrap();
    let mut harness = window(app, (1280.0, 900.0));
    harness.run_steps(4);
    let (done, heading, bullets) = super::view::kept_at(&harness.ctx, Place::Quote, id, |laid, _| {
        let heading = laid.items.iter().any(|i| text_of(i) == "New on our shelves this month");
        (laid.done, heading, laid.items.iter().filter(|i| text_of(i).contains("coastal birds")).count())
    })
    .expect("the quote is laid out");
    assert!(done && heading && bullets == 1);
    // Nothing of it can be selected or edited: the keys stay with the owner's text.
    assert!(harness.state().text_selection.at(Place::Quote).is_none());
}

/// (#47) In a wide pane a message's text fills it, as the body of a reply does above the
/// original: no measure of its own holds it to one side.
#[test]
fn a_wide_pane_is_filled() {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "receipt");
    let mut harness = window(app, (2400.0, 800.0));
    select(&mut harness, id);
    harness.run_steps(8);
    let width = super::view::kept_at(&harness.ctx, Place::Pane, id, |laid, _| laid.width()).unwrap();
    assert!(width > super::MAX_BODY_WIDTH + 400.0, "{width}");
}

// ---- #150: a centred column, tiles, lines of pictures ----

/// The middle of a thing, up and down.
fn middle(item: &Item) -> f32 {
    item.rect.center().y
}

#[test]
fn a_centred_digest_keeps_its_column_its_tiles_and_its_lines() {
    let m = corpus_message("access digest");
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let laid = lay_out(ctx, &m, 1000.0);
            check(&laid, &Tokens::get(ctx), "access digest");
            assert_eq!(laid.stats.stacked, 0, "{:?}", laid.stats);
            // The 600-pixel column stands in the middle of the pane, and no wider.
            let intro = find(&laid, "Here is what happened");
            assert!(intro.rect.width() <= 600.0 && ((intro.rect.min.x + intro.rect.max.x) / 2.0 - 500.0).abs() < 1.5, "{:?}", intro.rect);
            // Each tile's number has its icon beside it, on its middle, whichever way it is written.
            let pictures = shown_pictures(&laid);
            for number in ["12", "3", "7", "2"] {
                let n = find_exact(&laid, number);
                let icon = pictures
                    .iter()
                    .min_by(|a, b| a.rect.center().distance(n.rect.center()).total_cmp(&b.rect.center().distance(n.rect.center())))
                    .unwrap();
                assert!((middle(icon) - middle(n)).abs() < 2.0, "{number}: {:?} beside {:?}", icon.rect, n.rect);
                let apart = (n.rect.min.x - icon.rect.max.x).max(icon.rect.min.x - n.rect.max.x);
                assert!((-0.5..40.0).contains(&apart), "{number}: {:?} beside {:?}", icon.rect, n.rect);
            }
            // The footer's pictures: two lines of three, side by side, each line centred.
            let small: Vec<&&Item> = pictures.iter().filter(|i| (i.rect.width() - 20.0).abs() < 0.5).collect();
            assert_eq!(small.len(), 6, "{small:?}");
            for line in small.chunks(3) {
                assert!(line.iter().all(|i| (i.rect.min.y - line[0].rect.min.y).abs() < 0.5), "{line:?}");
                assert!(line.windows(2).all(|w| w[1].rect.min.x > w[0].rect.max.x), "{line:?}");
                let centre = (line[0].rect.min.x + line[2].rect.max.x) / 2.0;
                assert!((centre - 500.0).abs() < 2.0, "{line:?}");
            }
            // The table of roles: the long cells wrap in the share they ask for, and every cell
            // of the ruled rows has the same room round it.
            let long = find(&laid, "Privileged authentication");
            let count = find(&laid, "5 activations");
            assert!(long.rect.max.x <= count.rect.min.x + 0.5 && long.rect.height() > 30.0, "{:?} {:?}", long.rect, count.rect);
            assert!((find(&laid, "Security reader").rect.min.x - long.rect.min.x).abs() < 0.5);
            assert!((find(&laid, "3 activations").rect.max.x - count.rect.max.x).abs() < 0.5);
        });
    }
}

#[test]
fn a_row_of_small_pictures_stays_a_row_where_it_fits() {
    let icons: String = (0..6)
        .map(|i| format!("<td style='padding:0 4px'><a href='https://social.example/{i}'><img src='cid:tick-16@quillon.example' width='24' height='24' alt='Link {i}'></a></td>"))
        .collect();
    let html = format!("<table align='center' role='presentation'><tr>{icons}</tr></table>");
    let m = html_message(50, ("Digest", "digest@bellweather.example"), "Icons", &html);
    in_frame(false, |ctx| {
        // Six columns in 400 points: narrow, but what is in them fits.
        let laid = lay_out(ctx, &m, 400.0);
        assert_eq!(laid.stats.stacked, 0);
        let pictures = shown_pictures(&laid);
        assert_eq!(pictures.len(), 6);
        assert!(pictures.iter().all(|p| (p.rect.min.y - pictures[0].rect.min.y).abs() < 0.5), "{pictures:?}");
        // Where they don't fit, they are stacked as before.
        let laid = lay_out(ctx, &m, 100.0);
        assert_eq!(laid.stats.stacked, 1);
    });
}

#[test]
fn a_picture_and_text_in_one_line_go_on_lines_of_their_own_when_short_of_room() {
    let html = "<p style='text-align:center'><img src='cid:quillon-mark@quillon.example' width='32' height='32' alt=''> Bellweather Directory, the weekly digest</p>";
    let m = html_message(51, ("Digest", "digest@bellweather.example"), "Logo", html);
    in_frame(false, |ctx| {
        let wide = lay_out(ctx, &m, 600.0);
        let (logo, name) = (shown_pictures(&wide)[0].rect, find(&wide, "Bellweather Directory").rect);
        assert!((logo.center().y - name.center().y).abs() < 1.0 && logo.max.x < name.min.x, "{logo:?} {name:?}");
        assert!(((logo.min.x + name.max.x) / 2.0 - 300.0).abs() < 2.0, "the line is centred: {logo:?} {name:?}");
        // Too narrow for both: the picture over the text, as before.
        let narrow = lay_out(ctx, &m, 120.0);
        let (logo, name) = (shown_pictures(&narrow)[0].rect, find(&narrow, "Bellweather").rect);
        assert!(logo.max.y <= name.min.y, "{logo:?} {name:?}");
    });
}

/// #180: a message holding U+FFFD, where a character couldn't be read, says why in one line above
/// it, whether the mark is in its subject, its sender or its text; one without says nothing.
#[test]
fn a_character_that_couldnt_be_read_is_explained_once() {
    const SAID: &str = "Some characters in this message couldn't be read";
    let clean = message(0, ("Mila Horvat", "mila@example.org"), "Gvožđe", None, "We’ve met – twice.".into());
    let marked = [
        message(1, ("Mila Horvat", "mila@example.org"), "Gvo\u{fffd}\u{fffd}e", None, "Plain.".into()),
        message(2, ("Mila Horvat\u{fffd}", "mila@example.org"), "Hello", None, "Plain.".into()),
        message(3, ("Mila Horvat", "mila@example.org"), "Hello", None, "We\u{fffd}ve met \u{fffd} twice.".into()),
        message(4, ("Mila Horvat", "mila@example.org"), "Hello", Some("<p>We\u{fffd}ve met</p>".into()), String::new()),
    ];
    assert!(!super::has_unreadable(&clean));
    for m in &marked {
        assert!(super::has_unreadable(m), "{:?}", m.subject);
    }
    let (app, ids) = app_with(std::iter::once(clean).chain(marked).collect());
    let mut harness = window(app, (1100.0, 700.0));
    select(&mut harness, ids[0]);
    assert!(!says(&harness, SAID));
    for id in &ids[1..] {
        select(&mut harness, *id);
        assert_eq!(harness.query_all_by_label_contains(SAID).count(), 1, "message {id}");
    }
}

/// #349: an `<hr>` is a line that can be seen, in light and dark: a bare one, the one above a
/// reply's quoted original (whose own border is ignored), and one inside a quote.
#[test]
fn a_rule_is_drawn() {
    const HTML: &str = "<p>One</p><hr><p>Two</p><hr style=\"border:none;border-top:solid #E1E1E1 1.0pt\"><p>Three</p>\
                        <blockquote><p>Four</p><hr><p>Five</p></blockquote>";
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let laid = lay_out(ctx, &html_message(1, ("Ada", "ada@example.org"), "Rules", HTML), 700.0);
            let paper = if dark { Tokens::dark().surface } else { Tokens::light().surface };
            for (above, below) in [("One", "Two"), ("Two", "Three"), ("Four", "Five")] {
                let (top, bottom) = (find_exact(&laid, above).rect.max.y, find_exact(&laid, below).rect.min.y);
                let (rect, color) = laid
                    .items
                    .iter()
                    .find_map(|i| match i.kind {
                        ItemKind::Fill { color, .. } if i.rect.min.y >= top && i.rect.max.y <= bottom && i.rect.width() > 300.0 => {
                            Some((i.rect, color))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("no rule between {above:?} and {below:?}"));
                assert!(rect.height() >= 1.0, "{rect:?}");
                // (`contrast` doesn't look at alpha: a transparent fill reads as black.)
                assert!(color.a() == 255 && contrast(color, paper) >= 1.2, "{above:?}: {color:?} on {paper:?}");
            }
        });
    }
}

/// #71: a bulletin's padding, margins, dashed and dotted lines, rounded corners and line height are
/// drawn, within the room there is, and its lines stay visible on the paper in light and dark.
#[test]
fn boxes_are_drawn_with_their_room_lines_and_corners() {
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let bulletin = corpus_message("style boxes");
            let laid = lay_out(ctx, &bulletin, 700.0);
            let kinds = |f: &dyn Fn(&ItemKind) -> bool| laid.items.iter().filter(|i| f(&i.kind)).count();
            // The page and the table of leaders are rounded rectangles; the notice, the swap and the divider are dashes.
            assert!(kinds(&|k| matches!(k, ItemKind::Outline { radius, .. } if *radius >= 6.0)) >= 2);
            assert!(kinds(&|k| matches!(k, ItemKind::Dashes { dotted: false, .. })) >= 3, "dashed notice and divider");
            assert!(kinds(&|k| matches!(k, ItemKind::Dashes { dotted: true, .. })) >= 3, "dotted swap");
            // Every line can be seen on the window's paper.
            let paper = if dark { crate::theme::Tokens::dark().surface } else { crate::theme::Tokens::light().surface };
            for item in &laid.items {
                if let ItemKind::Dashes { color, .. } | ItemKind::Outline { color, .. } = &item.kind {
                    assert!(contrast(*color, paper) >= 1.2, "{color:?} on {paper:?}");
                }
            }
            // The text is set in from the card's edge by its padding, and runs on at 1.7 times its size.
            let walk = find_exact(
                &laid,
                "Saturday's walk goes ahead from the old mill car park at nine. Bring a flask, and boots you do not mind getting muddy.",
            );
            assert!(walk.rect.min.x >= 28.0 && walk.rect.max.x <= 700.0 - 28.0, "{:?}", walk.rect);
            if let ItemKind::Text(text) = &walk.kind {
                let line = text.job.sections.first().and_then(|s| s.format.line_height).unwrap_or(0.0);
                assert!((line - (13.0f32 * 1.7).round()).abs() < 0.6, "{line}");
            }
        });
    }
}
