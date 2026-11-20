// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The HTML signature the composer shows as it goes out (`compose::signature`): the message that
//! draw makes must hold every picture its HTML names as an inline part of that same message,
//! under the Content-ID the HTML names there, with the picture's own bytes.
//!
//! One synthetic fixture and one headless window: a signature seeded in the model and made the
//! account's default (the way the engine's saved-draft tracer seeds one), `mail.new`, and the
//! composer's own `signature` drawn inside `MailApp::ui`. What is asserted is the message that
//! draw kept — `compose` caches it in this `egui::Context` under `mailcraft-compose-signature` —
//! read back the way the reading pane reads it: its `cid:` pictures, found in its display blocks,
//! resolved to the message's inline parts.
//!
//! The synthetic image is checked by its payload and attachment binding, not painted pixels.

use std::sync::Arc;

use mailcraft_html::BlockKind;
use mailcraft_model::{Message, Signature};
use serde_json::json;

use super::editor;
use crate::compose::tests::{app, window};

/// A one-pixel PNG, whole and valid: a picture by its own first bytes, the same fixture the MIME
/// tests and the engine's saved-draft tracer use. Nothing here says anything about pixels.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

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
fn the_composers_html_signature_preview_binds_its_picture_to_an_inline_part() {
    // The signature as the model is seeded with one: plain text, the picture, then the owner's
    // own HTML naming it by the address the model itself gives it. (`signature.html` refuses a
    // signature that has embedded pictures and is the legacy conversion, not this fixture.)
    let mut sig = Signature::plain_text("Work", "Priya Raman\nNorthwind Atelier");
    let logo = sig.add_picture("northwind-mark.png", ONE_PIXEL_PNG.to_vec(), Some("Northwind mark"), Some(0)).expect("a signature takes a PNG");
    let picture = sig.picture(logo).expect("the picture just added");
    let own = own_html(&mailcraft_engine::mime::rich::internal_cid(picture));
    sig.set_html(Some(own.clone())).expect("the model keeps the HTML of a signature that has its pictures");

    let mut a = app();
    a.session.mailbox.signatures.push(sig);
    let account = a.session.mailbox.default_account().expect("the sample mailbox has an account").id;
    a.execute("signature.defaults", json!({"account": account, "new": "Work"})).expect("the account takes the signature for new messages");
    a.execute("mail.new", json!({"account": account, "to": "maya@lumen.example", "body": "Thanks,"})).expect("a new message opens");

    // What the composer has to be in for `compose::signature` to draw the signature as it goes
    // out, rather than the link that offers it: an unedited HTML signature in an HTML message,
    // still shown as the HTML and not chosen to be edited as text.
    assert!(editor::unedited_html_signature(&a).is_some(), "the fixture is not an unedited HTML signature ending the message");
    assert!(editor::html_signature(&a).is_some(), "the composer would offer to show the signature as sent, not draw it");
    let html = editor::unedited_html_signature(&a).expect("the signature's own HTML").to_string();

    // The headless window: `MailApp::ui` draws the composer, in whose body `compose::signature` runs.
    let mut h = window(a);
    h.run_steps(4);
    let shown = h.state().hit.rect("composeSignature", 0);
    assert!(shown.is_some_and(|r| r.height() > 0.0), "the composer drew no HTML signature preview at all: {shown:?}");

    // What that draw made, as it left it: `compose::signature` keeps the message in this context
    // under `mailcraft-compose-signature`, one entry for each signature's HTML it has drawn.
    type Kept = Vec<(egui::Id, Arc<Message>)>;
    let key = egui::Id::unique("mailcraft-compose-signature");
    let kept = h
        .ctx
        .data(|d| d.get_temp::<Kept>(key))
        .unwrap_or_else(|| panic!("the composer kept no preview message at all (no frame reached `compose::signature`): {key:?}"));
    let message = kept
        .iter()
        .find(|(of, _)| *of == egui::Id::unique(&html))
        .map(|(_, m)| m.clone())
        .unwrap_or_else(|| panic!("the preview kept is not this signature's: {} entries, none for its HTML", kept.len()));

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
    // nothing in a message (the saved draft asserts the same of what goes out).
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

/// The message the composer's preview keeps for the signature drawn now: the entry
/// `compose::signature` itself finds first under its own identity for this signature's HTML — read
/// as that code keeps it, one entry for each signature's HTML it has drawn. Exactly one entry may
/// bear that identity: the draw shows the first of them, so a second for the same HTML could be
/// the fresh message drawn beside the stale one the pane is actually given.
fn kept_preview(h: &crate::compose::tests::Window) -> Arc<Message> {
    type Kept = Vec<(egui::Id, Arc<Message>)>;
    let key = egui::Id::unique("mailcraft-compose-signature");
    let html = editor::unedited_html_signature(h.state()).expect("the signature's own HTML").to_string();
    let this = egui::Id::unique(&html);
    let kept = h
        .ctx
        .data(|d| d.get_temp::<Kept>(key))
        .unwrap_or_else(|| panic!("the composer kept no preview message at all (no frame reached `compose::signature`): {key:?}"));
    let matching = kept.iter().filter(|(of, _)| *of == this).count();
    assert_eq!(matching, 1, "the composer kept {matching} entries for one signature's HTML, and it draws the first: {} entries in all", kept.len());
    kept.iter()
        .find(|(of, _)| *of == this)
        .map(|(_, m)| Arc::clone(m))
        .unwrap_or_else(|| panic!("the composer kept no preview of this signature: {} entries, none for its HTML", kept.len()))
}

#[test]
fn the_composers_html_signature_preview_refreshes_the_picture_when_its_bytes_change() {
    // Two signatures that differ only in what their picture draws: the same words, the same
    // layout, the same picture id, width, description, file name and type, and the same owner's
    // HTML. The composer's kept preview is keyed by that HTML alone (`Id::unique(&html)`), so
    // choosing the other signature in the message would draw the first one's picture unless what
    // the message is keyed by includes the picture.
    let mut work = Signature::plain_text("Work", "Priya Raman\nNorthwind Atelier");
    let logo = work.add_picture("northwind-mark.png", ONE_PIXEL_PNG.to_vec(), Some("Northwind mark"), Some(0)).expect("a signature takes a PNG");
    let picture = work.picture(logo).expect("the picture just added");
    assert_eq!(picture.id, 1, "the fixture's picture is not the first of its signature");
    // Its address, owned, before the panel below takes `work` mutably: `picture` borrows it.
    let original_cid = mailcraft_engine::mime::rich::internal_cid(picture);
    let own = own_html(&original_cid);
    work.set_html(Some(own.clone())).expect("the model keeps the HTML of a signature that has its pictures");
    let brief = signature_with("Work (red mark)", OTHER_ONE_PIXEL_PNG, &own);
    assert_eq!(
        mailcraft_engine::mime::rich::internal_cid(brief.picture(1).expect("the other signature's picture")),
        original_cid,
        "the two signatures' HTML does not name a picture by the same address, so this is not the stale-preview case"
    );

    let mut a = app();
    a.session.mailbox.signatures.push(work);
    a.session.mailbox.signatures.push(brief);
    let account = a.session.mailbox.default_account().expect("the sample mailbox has an account").id;
    a.execute("signature.defaults", json!({"account": account, "new": "Work"})).expect("the account takes the signature for new messages");
    a.execute("mail.new", json!({"account": account, "to": "maya@lumen.example", "body": "Thanks,"})).expect("a new message opens");
    assert_eq!(editor::unedited_html_signature(&a).expect("the signature's own HTML"), own.as_str(), "the draft does not hold the fixture's HTML");

    let mut h = window(a);
    h.run_steps(4);
    // Frame one: the composer drew the first signature, with its own picture.
    assert!(h.state().hit.rect("composeSignature", 0).is_some_and(|r| r.height() > 0.0), "the composer drew no HTML signature preview at all");
    let first = kept_preview(&h);
    assert_bound_to(&first, ONE_PIXEL_PNG, "frame one");
    {
        let snapshot = h.state().session.compose.as_ref().and_then(|c| c.signature.as_ref()).expect("the draft's signature snapshot");
        assert_eq!(snapshot.name, "Work", "the draft's signature is not the account's default one");
        assert!(!snapshot.as_text, "the first frame's signature is chosen to be edited as text");
        assert!(snapshot.html.is_some(), "the first frame's signature has no HTML to show as it goes out");
    }
    let html_a = editor::unedited_html_signature(h.state()).expect("the signature's own HTML").to_string();

    // An unchanged frame draws that same signature again: the message kept is reused, not made
    // again every frame. Nothing here is relied on by the checks below — it is the control that
    // says the preview is a cache and not a message rebuilt on every step, which every check
    // below would pass while the composer did the work each frame.
    h.run_steps(4);
    let again = kept_preview(&h);
    assert!(
        Arc::ptr_eq(&first, &again),
        "unchanged frames made a new preview message instead of the one kept: the preview is drawn again every frame rather than reused"
    );

    // Choose the other signature, by the command the message's Signature menu runs: the draft's
    // signature becomes it, its pictures with it.
    h.state_mut().execute("compose.signature", json!({"name": "Work (red mark)"})).expect("the draft takes the other signature");
    let inserted = h.state().session.compose.as_ref().and_then(|c| c.signature.as_ref()).expect("the draft's signature snapshot");
    assert_eq!(inserted.name, "Work (red mark)", "the draft's signature is not the one asked for");
    assert!(!inserted.as_text, "choosing the signature left it chosen to be edited as text");
    assert!(inserted.html.is_some(), "choosing the signature left it with no HTML to show as it goes out");
    let changed = inserted.pictures.first().expect("the fixture's picture");
    assert_eq!(changed.id, 1, "the two signatures do not carry the picture by the same id");
    assert_eq!(changed.width, None, "the two signatures do not show the picture at the same width");
    assert_eq!(changed.alt, "Northwind mark", "the two signatures do not describe the picture the same way");
    assert_eq!(changed.name, "northwind-mark.png", "the two signatures' pictures do not have the same file name");
    assert_eq!(changed.mime, "image/png", "the two signatures' pictures are not the same type");
    assert_eq!(changed.data.as_slice(), OTHER_ONE_PIXEL_PNG, "the draft did not take the other signature's picture");
    assert!(
        h.state().session.compose.as_ref().is_some_and(|c| c.signature_unedited()),
        "choosing the signature left it edited, so the composer would draw its simpler copy"
    );

    // The HTML is what it was: that is the risk. Nothing the old identity reads tells the two
    // signatures apart.
    let html_b = editor::unedited_html_signature(h.state()).expect("the signature's own HTML after the switch").to_string();
    assert_eq!(html_b, html_a, "the two signatures' HTML is not identical, so this is not the stale-preview case");
    assert_eq!(
        egui::Id::unique(&html_a),
        egui::Id::unique(&html_b),
        "the HTML alone, which the kept preview is keyed by, does not tell the two signatures apart"
    );

    // Frame two: the composer draws the signature the draft now holds.
    h.run_steps(4);
    assert!(
        h.state().hit.rect("composeSignature", 0).is_some_and(|r| r.height() > 0.0),
        "the composer drew no HTML signature preview after the draft took the other signature"
    );
    let second = kept_preview(&h);
    assert_bound_to(&second, OTHER_ONE_PIXEL_PNG, "frame two");
    assert!(
        inline_parts(&second).iter().all(|(_, bytes)| bytes.as_slice() != ONE_PIXEL_PNG),
        "the preview still holds the first signature's picture after the draft took the other one: the picture it draws did not follow its bytes"
    );
    // It is not the message kept for the first signature either: a preview that follows the bytes
    // is a new message, not the old one read under a newer name.
    assert!(
        !Arc::ptr_eq(&first, &second),
        "the preview shown after the draft took the other signature is the very message kept for the first one, whose pictures cannot have followed"
    );
    // And unchanged frames now reuse that one, as they did before.
    h.run_steps(4);
    let held = kept_preview(&h);
    assert!(
        Arc::ptr_eq(&second, &held),
        "unchanged frames after the other signature was drawn made a new preview message instead of the one kept: the preview is drawn again every frame rather than reused"
    );
}

/// An HTML signature with a style sheet: the composer's preview shows it as it goes out, its class
/// rule (scoped to the signature) giving its words their colour, as the message sent gives them.
#[test]
fn the_composers_preview_shows_what_the_signatures_style_sheet_gives() {
    let own = "<html><head><style>.studio-card { color: #1f4e79; }</style></head><body><div class=\"studio-card\">Northwind Studio card</div><div>Priya Raman</div></body></html>";
    let mut sig = Signature::plain_text("Northwind Work", "Northwind Studio card\nPriya Raman");
    sig.set_html(Some(own.to_string())).expect("the model keeps the HTML");
    let mut a = app();
    a.session.mailbox.signatures.push(sig);
    let account = a.session.mailbox.default_account().expect("the sample mailbox has an account").id;
    a.execute("signature.defaults", json!({"account": account, "new": "Northwind Work"})).expect("the account takes the signature for new messages");
    a.execute("mail.new", json!({"account": account, "to": "maya@lumen.example", "body": "Thanks,"})).expect("a new message opens");
    assert_eq!(editor::unedited_html_signature(&a), Some(own), "fixture guard: the HTML signature went in unedited");
    let mut h = window(a);
    h.run_steps(4);
    let message = kept_preview(&h);
    fn span<'b>(blocks: &'b [mailcraft_html::Block], text: &str) -> Option<&'b mailcraft_html::Span> {
        blocks.iter().find_map(|b| match &b.kind {
            BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).find_map(|c| span(&c.blocks, text)),
            _ => b.spans.iter().find(|s| s.text.contains(text)),
        })
    }
    let blocks = crate::reading::blocks_of(&message);
    let card = span(&blocks, "Northwind Studio card").expect("the preview shows the signature's words");
    assert_eq!(
        card.color,
        Some(mailcraft_html::Rgb(0x1f, 0x4e, 0x79)),
        "the composer's preview drops the signature's style sheet: {:?}",
        message.body.html
    );
    let name = span(&blocks, "Priya Raman").expect("the preview shows the signature's words");
    assert_eq!(name.color, None, "the rule reaches past its class");
}
