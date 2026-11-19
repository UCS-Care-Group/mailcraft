// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The signature page's sample preview (`signatures::preview`): the message it keeps under
//! `mailcraft-signature-preview` must hold every picture its HTML names as an inline part of that
//! same message, under the Content-ID the HTML names there, with the picture's own bytes.
//!
//! One synthetic fixture and one headless window: a signature seeded in the mailbox and opened by
//! `signature.open`, File › Signatures drawn by `MailApp::ui`, and the message that draw kept read
//! back the way the reading pane reads it: its `cid:` pictures, found in its display blocks,
//! resolved to the message's inline parts.
//!
//! The synthetic image is checked by its payload and attachment binding, not painted pixels.

use std::sync::Arc;

use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_html::BlockKind;
use mailcraft_model::{Message, Signature};
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A one-pixel PNG, whole and valid: a picture by its own first bytes, the same fixture the MIME
/// tests and the engine's saved-draft tracer use. Nothing here says anything about pixels.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The app over the sample mailbox, nothing happening by itself.
fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

/// The whole window, laid out a few frames: File › Signatures, in whose page `signatures::preview`
/// runs.
fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

/// The owner's own HTML: a table with their words, inline styling, and the picture named by the
/// `cid:` address the model gives a signature's picture (`rich::internal_cid`), which is how the
/// app writes one. Written 120 by 80 so that it is no picture "too small to see", which quoting
/// leaves out (nothing here is a claim about its pixels).
fn own_html(cid: &str) -> String {
    format!(
        "<div><img src=\"cid:{cid}\" alt=\"Northwind mark\" width=\"120\" height=\"80\" style=\"border:0\"></div>\
         <table cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"background-color:#fdf6e3;padding:12px\">\
         <tr><td><div>Priya Raman - Northwind Atelier</div><div>Studio Two, Harbour Lane</div></td></tr></table>"
    )
}

/// Each `cid:` address the message's own HTML names a picture by, as the reading pane finds it in
/// the blocks it lays the message out from.
fn picture_sources(message: &Message) -> Vec<String> {
    mailcraft_html::images(&crate::reading::blocks_of(message))
        .iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Image { src, .. } => Some(src.clone()),
            _ => None,
        })
        .collect()
}

/// The inline picture parts of `message`, by Content-ID and bytes.
fn inline_parts(message: &Message) -> Vec<(String, Vec<u8>)> {
    message.attachments.iter().filter(|a| a.inline && a.mime == "image/png").map(|a| (a.content_id.clone(), a.data.to_vec())).collect()
}

#[test]
fn the_signature_pages_preview_binds_its_picture_to_an_inline_part() {
    // The signature as the model is seeded with one: plain text, the picture, then the owner's
    // own HTML naming it by the address the model itself gives it.
    let mut sig = Signature::plain_text("Northwind Work", "Priya Raman\nNorthwind Atelier");
    let logo = sig.add_picture("northwind-mark.png", ONE_PIXEL_PNG.to_vec(), Some("Northwind mark"), Some(0)).expect("a signature takes a PNG");
    let picture = sig.picture(logo).expect("the picture just added");
    let own = own_html(&mailcraft_engine::mime::rich::internal_cid(picture));
    sig.set_html(Some(own.clone())).expect("the model keeps the HTML of a signature that has its pictures");

    let mut a = app();
    a.session.mailbox.signatures.push(sig);
    a.execute("signature.open", json!({"name": "Northwind Work"})).expect("the signature opens in the page's editor");
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).expect("File shows the signatures page");
    assert!(a.session.signature_edit.is_some(), "the fixture opened no signature for the page to preview");
    assert_eq!(a.ui.backstage_page, "signatures", "the fixture is not on the signatures page");

    let mut h = window(a);
    h.run_steps(2);
    // The page is the one drawn: its list of signatures, which stands beside the one open.
    assert!(
        h.state().hit.rect("signature", 0).is_some_and(|r| r.height() > 0.0),
        "the signatures page drew no list of signatures, so its preview never ran"
    );

    // What that draw made, as it left it: `signatures::preview` keeps the message in this context
    // under `mailcraft-signature-preview`, with the stamp it was made from.
    let key = egui::Id::unique("mailcraft-signature-preview");
    let kept = h
        .ctx
        .data(|d| d.get_temp::<(u64, Arc<Message>)>(key))
        .unwrap_or_else(|| panic!("the signature page kept no preview message at all (no frame reached `signatures::preview`): {key:?}"));
    let message = kept.1;

    // The preview is the owner's signature, and it names a picture: otherwise there is nothing to
    // bind and the rest of this test would hold of an empty message.
    let preview_html = message.body.html.as_deref().expect("the preview message is HTML");
    assert!(preview_html.contains("Northwind Atelier"), "the preview is not the owner's signature: {preview_html}");
    let sources = picture_sources(&message);
    assert!(!sources.is_empty(), "the preview's HTML names no picture at all, so binding it would prove nothing: {preview_html}");

    // The picture is a part of that message, with its own bytes...
    let parts = inline_parts(&message);
    assert!(
        !parts.is_empty(),
        "the preview message holds no inline picture part, so the address its HTML names can only dangle: {:?}",
        message.attachments.iter().map(|a| (&a.name, &a.mime, &a.content_id, a.inline)).collect::<Vec<_>>()
    );
    assert!(
        parts.iter().any(|(_, bytes)| bytes.as_slice() == ONE_PIXEL_PNG),
        "the preview's inline parts are not the signature's picture: {:?}",
        parts.iter().map(|(id, bytes)| (id, bytes.len())).collect::<Vec<_>>()
    );

    // ...and every picture its HTML names is that part: the pane can resolve each to a Content-ID
    // of this same message, holding the picture's own bytes. That is the binding, not a search of
    // the text for something that looks like one.
    for src in &sources {
        let part = crate::reading::images::attachment(&message, src).unwrap_or_else(|| {
            panic!("the pane cannot resolve the picture the preview names, `{src}`, to a part of the preview message: {:?}", parts)
        });
        assert_eq!(part.data.as_slice(), ONE_PIXEL_PNG, "`{src}` resolves to a part that is not the picture's own bytes");
    }
    for (id, _) in &parts {
        assert!(
            sources.iter().any(|src| crate::reading::images::attachment(&message, src).is_some_and(|a| a.content_id == *id)),
            "the preview holds an inline part `{id}` that no picture of its HTML names"
        );
    }
    // And it keeps no address that stands for the picture inside the signature: that one names
    // nothing in a message (the saved draft and the composer assert the same of what goes out).
    assert!(
        !preview_html.contains("signature.invalid"),
        "the preview keeps the signature's own address, which stands for it nowhere else: {preview_html}"
    );
}

/// A one-pixel PNG that is not [`ONE_PIXEL_PNG`]: eight-bit RGBA, opaque red, made here with
/// Python's `struct` and `zlib` (each chunk's CRC checked and the IDAT inflated to the one pixel
/// below, so it is a whole picture and not a damaged one). An original: no vendor's bytes.
const OTHER_ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x56, 0xc7, 0x2f, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A signature of the same words, layout, picture id, width, description, file name and type as
/// the one made beside it, holding `png` and the owner's own HTML `html`: what the model makes of
/// a second signature whose picture was replaced with other bytes.
fn signature_with(name: &str, png: &[u8], html: &str) -> Signature {
    let mut sig = Signature::plain_text(name, "Priya Raman\nNorthwind Atelier");
    let logo = sig.add_picture("northwind-mark.png", png.to_vec(), Some("Northwind mark"), Some(0)).expect("a signature takes a PNG");
    assert_eq!(sig.picture(logo).expect("the picture just added").id, 1, "the fixture's picture is not the first of its signature");
    sig.set_html(Some(html.to_string())).expect("the model keeps the HTML of a signature that has its pictures");
    sig
}

/// `message` is the owner's signature with its picture bound to an inline part of itself holding
/// exactly `png`: what the reading pane reads out of it, and `when` says which frame it came from.
fn assert_bound_to(message: &Message, png: &[u8], when: &str) {
    let html = message.body.html.as_deref().unwrap_or_else(|| panic!("{when}: the preview message is not HTML"));
    assert!(html.contains("Northwind Atelier"), "{when}: the preview is not the owner's signature: {html}");
    let sources = picture_sources(message);
    assert!(!sources.is_empty(), "{when}: the preview's HTML names no picture at all, so binding it would prove nothing: {html}");
    let parts = inline_parts(message);
    assert!(
        !parts.is_empty(),
        "{when}: the preview message holds no inline picture part, so the address its HTML names can only dangle: {:?}",
        message.attachments.iter().map(|a| (&a.name, &a.mime, &a.content_id, a.inline)).collect::<Vec<_>>()
    );
    assert!(
        parts.iter().any(|(_, bytes)| bytes.as_slice() == png),
        "{when}: the preview's inline parts are not the signature's picture: {:?}",
        parts.iter().map(|(id, bytes)| (id, bytes.len())).collect::<Vec<_>>()
    );
    for src in &sources {
        let part = crate::reading::images::attachment(message, src)
            .unwrap_or_else(|| panic!("{when}: the pane cannot resolve the picture the preview names, `{src}`, to a part of it: {parts:?}"));
        assert_eq!(part.data.as_slice(), png, "{when}: `{src}` resolves to a part that is not the picture's own bytes");
    }
    for (id, _) in &parts {
        assert!(
            sources.iter().any(|src| crate::reading::images::attachment(message, src).is_some_and(|a| a.content_id == *id)),
            "{when}: the preview holds an inline part `{id}` that no picture of its HTML names"
        );
    }
    assert!(!html.contains("signature.invalid"), "{when}: the preview keeps the signature's own address, which stands for it nowhere else: {html}");
}

/// What `signatures::preview` hashes to tell whether the message it kept is still this signature's:
/// the text's fingerprint, the layout, each picture's id, width and description, and the owner's
/// HTML. Recomputed here from the model, as that code does, so that a test can say what the page's
/// identity does and does not see.
type Stamp = (u64, String, Vec<(u32, Option<u16>, String)>, Option<String>);

fn preview_stamp(sig: &Signature) -> Stamp {
    (
        mailcraft_model::signature::text_hash(sig.body()),
        format!("{:?}", sig.layout),
        sig.pictures().iter().map(|p| (p.id, p.width, p.alt.clone())).collect(),
        sig.html().map(str::to_string),
    )
}

/// The signature the page has open, stamped as `signatures::preview` stamps it: its name, whether
/// it is edited, that stamp, and whether its HTML survived the text it is drawn beside.
fn open_signature(h: &Harness<'static, MailApp>) -> (String, bool, Stamp, bool) {
    let e = h.state().session.signature_edit.as_ref().expect("a signature is open in the page");
    let mut sig = e.signature.clone();
    sig.set_body(e.body.text().clone()).expect("the open signature's text");
    (e.signature.name.clone(), e.dirty, preview_stamp(&sig), sig.html().is_some())
}

#[test]
fn the_signature_pages_preview_refreshes_the_picture_when_its_bytes_change() {
    // Two signatures the page's preview stamp cannot tell apart: the same words, layout, picture
    // id, width, description and owner's HTML, and a picture that differs only in its bytes.
    // Opening the second would keep the first one's message, and its picture, unless the message
    // kept is tied to the picture.
    let mut work = Signature::plain_text("Northwind Work", "Priya Raman\nNorthwind Atelier");
    let logo = work.add_picture("northwind-mark.png", ONE_PIXEL_PNG.to_vec(), Some("Northwind mark"), Some(0)).expect("a signature takes a PNG");
    let picture = work.picture(logo).expect("the picture just added");
    assert_eq!(picture.id, 1, "the fixture's picture is not the first of its signature");
    let own = own_html(&mailcraft_engine::mime::rich::internal_cid(picture));
    work.set_html(Some(own.clone())).expect("the model keeps the HTML of a signature that has its pictures");
    let brief = signature_with("Northwind Work (red mark)", OTHER_ONE_PIXEL_PNG, &own);

    let mut a = app();
    a.session.mailbox.signatures.push(work);
    a.session.mailbox.signatures.push(brief);
    a.execute("signature.open", json!({"name": "Northwind Work"})).expect("the first signature opens in the page's editor");
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).expect("File shows the signatures page");

    let mut h = window(a);
    h.run_steps(2);
    assert!(
        h.state().hit.rect("signature", 0).is_some_and(|r| r.height() > 0.0),
        "the signatures page drew no list of signatures, so its preview never ran"
    );
    // The message the page kept, as it left it: `signatures::preview` keeps it in this context
    // under `mailcraft-signature-preview`, with the stamp it was made from.
    let key = egui::Id::unique("mailcraft-signature-preview");
    let read = |h: &Harness<'static, MailApp>| -> (u64, Arc<Message>) {
        h.ctx
            .data(|d| d.get_temp::<(u64, Arc<Message>)>(key))
            .unwrap_or_else(|| panic!("the signature page kept no preview message at all (no frame reached `signatures::preview`): {key:?}"))
    };

    // Frame one: the page's sample of the first signature, with its own picture.
    let (_, first) = read(&h);
    assert_bound_to(&first, ONE_PIXEL_PNG, "frame one");
    let (name_a, dirty_a, stamp_a, html_kept_a) = open_signature(&h);
    assert_eq!(name_a, "Northwind Work", "the page has not the fixture's first signature open");
    assert_eq!(stamp_a.3.as_deref(), Some(own.as_str()), "the open signature's HTML is not the fixture's");
    assert!(html_kept_a, "setting the open signature's text let its HTML go");
    assert!(!dirty_a, "the fixture's first signature is edited, so this is not an untouched page");

    // An unchanged frame draws that same signature again: the message kept is reused, not made
    // again every frame. Nothing below relies on it — it is the control that says the page's
    // preview is a cache and not a message rebuilt on every step, which every check below would
    // pass while the page did the work each frame.
    h.run_steps(2);
    let (_, again) = read(&h);
    assert!(
        Arc::ptr_eq(&first, &again),
        "unchanged frames made a new preview message instead of the one kept: the preview is drawn again every frame rather than reused"
    );

    // Open the other signature, as the page's own list does when one is chosen: nothing to save.
    h.state_mut().execute("signature.open", json!({"name": "Northwind Work (red mark)"})).expect("the page opens the other signature");
    let (name_b, dirty_b, stamp_b, html_kept_b) = open_signature(&h);
    assert_eq!(name_b, "Northwind Work (red mark)", "the page has not the fixture's second signature open");
    assert!(!dirty_b, "the fixture's second signature is edited, so this is not an untouched page");
    assert!(html_kept_b, "setting the second signature's text let its HTML go");
    let open = h.state().session.signature_edit.as_ref().expect("a signature is open");
    let changed = open.signature.pictures().first().expect("the fixture's picture");
    assert_eq!(changed.data.as_slice(), OTHER_ONE_PIXEL_PNG, "the page did not open the signature with the other picture");
    // The stamp is the same for both, recomputed from the model rather than read from the preview:
    // that is the risk, and this says what the page's identity cannot see.
    assert_eq!(stamp_b, stamp_a, "the two signatures do not look the same to the page's preview stamp, so this is not the stale-preview case");

    // Frame two: the page draws the signature it has open.
    h.run_steps(2);
    assert!(
        h.state().hit.rect("signature", 0).is_some_and(|r| r.height() > 0.0),
        "the signatures page drew no list of signatures after the other signature was opened"
    );
    let (_, second) = read(&h);
    assert_bound_to(&second, OTHER_ONE_PIXEL_PNG, "frame two");
    assert!(
        inline_parts(&second).iter().all(|(_, bytes)| bytes.as_slice() != ONE_PIXEL_PNG),
        "the page's preview still holds the first signature's picture after the other one was opened: the picture it draws did not follow its bytes"
    );
    // It is not the message kept for the first signature either: a preview that follows the bytes
    // is a new message, not the old one read under a newer name.
    assert!(
        !Arc::ptr_eq(&first, &second),
        "the preview shown after the other signature was opened is the very message kept for the first one, whose pictures cannot have followed"
    );
    // And unchanged frames now reuse that one, as they did before.
    h.run_steps(2);
    let (_, held) = read(&h);
    assert!(
        Arc::ptr_eq(&second, &held),
        "unchanged frames after the other signature was drawn made a new preview message instead of the one kept: the preview is drawn again every frame rather than reused"
    );
}

/// An HTML signature with a style sheet: the preview shows it as it goes out, its class rule (scoped
/// to the signature) giving its words their colour, as the message sent gives them.
#[test]
fn the_preview_shows_what_the_signatures_style_sheet_gives() {
    let own = "<html><head><style>.studio-card { color: #1f4e79; }</style></head><body><div class=\"studio-card\">Northwind Studio card</div><div>Priya Raman</div></body></html>";
    let mut sig = Signature::plain_text("Northwind Work", "Northwind Studio card\nPriya Raman");
    sig.set_html(Some(own.to_string())).expect("the model keeps the HTML");
    let mut a = app();
    a.session.mailbox.signatures.push(sig);
    a.execute("signature.open", json!({"name": "Northwind Work"})).expect("the signature opens in the page's editor");
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).expect("File shows the signatures page");
    let mut h = window(a);
    h.run_steps(2);
    let key = egui::Id::unique("mailcraft-signature-preview");
    let message = h.ctx.data(|d| d.get_temp::<(u64, Arc<Message>)>(key)).expect("the page kept a preview message").1;
    fn span<'b>(blocks: &'b [mailcraft_html::Block], text: &str) -> Option<&'b mailcraft_html::Span> {
        blocks.iter().find_map(|b| match &b.kind {
            BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).find_map(|c| span(&c.blocks, text)),
            _ => b.spans.iter().find(|s| s.text.contains(text)),
        })
    }
    let blocks = crate::reading::blocks_of(&message);
    let card = span(&blocks, "Northwind Studio card").expect("the preview shows the signature's words");
    assert_eq!(card.color, Some(mailcraft_html::Rgb(0x1f, 0x4e, 0x79)), "the preview drops the signature's style sheet: {:?}", message.body.html);
    let name = span(&blocks, "Priya Raman").expect("the preview shows the signature's words");
    assert_eq!(name.color, None, "the rule reaches past its class");
}
