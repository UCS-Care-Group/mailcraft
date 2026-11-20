// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's own road into `signature.import`: File › Signatures' Import HTML…, the picture
//! files the document names — each one picked by the owner — and one Import at the end; and the
//! scripted road, `MailApp::execute`, which opens no picker at all.
//!
//! The first test is the tracer, in one headless window: a signature seeded in the mailbox and
//! made the account's default, opened in the page's editor and left with unsaved words; Import
//! HTML… asks the host's picker once (purpose `signatureHtml`, with a request token), a document
//! with three `<img>`s and two distinct picture files arrives through `Services::picked` as the
//! host delivers it, the page shows each logical resource name as a slot of its own, each slot is
//! picked (purpose `signatureAsset`, a token of its own), a new name is typed in the page's own
//! field, and Import stores exactly one signature. Nothing else moves: not the signature that was
//! there, not the account's defaults, not the signature open in the editor with its unsaved
//! words, not a message being written.
//!
//! The second test is the control the first must not be confused with: `MailApp::execute`
//! (`signature.import` as a script or an agent asks it) refuses for its own parameters' sake,
//! with no picker, no dialog and no signature — whatever `path`, `default` or other fields the
//! caller adds.
//!
//! The two pictures are the synthetic one-pixel PNGs the MIME and signature fixtures use, both
//! whole, valid and different from one another. Nothing here reads a file, opens a dialog or says
//! anything about pixels.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::{Session, SignatureEdit};
use mailcraft_model::{Mailbox, Signature};
use serde_json::json;

use crate::{Delivered, Inbox, MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The window every frame is laid out in: the size the other windows of these tests use, made
/// taller so that the page's own controls are on screen where the owner can press them.
const SIZE: (f32, f32) = (1280.0, 900.0);

/// The signature that is there before the import, and the words typed into it but never saved.
const OLD_NAME: &str = "Northwind Work";
const OLD_TEXT: &str = "Priya Raman\nNorthwind Atelier";
const UNSAVED: &str = " UNSAVED-WIP-4242";

/// The name the owner types in the page's field before the final Import, and the file the HTML
/// came in as (its stem is offered as a starting name, which the owner may replace).
const NEW_NAME: &str = "Northwind import";
const HTML_FILE: &str = "northwind-letterhead.html";

/// A one-pixel PNG, whole and valid: a picture by its own first bytes, the same fixture the MIME
/// and signature-page tests use. Nothing here says anything about pixels.
const LIGHT_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A one-pixel PNG that is not [`LIGHT_PNG`]: eight-bit RGBA, opaque red, the other fixture of the
/// signature page's own tests. An original: no vendor's bytes.
const DARK_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x56, 0xc7, 0x2f, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The owner's own document, as they saved it: a doctype, a head and a title, nested tables with
/// their styling in `style` attributes, white space as they left it, the owner's words and a link,
/// and three `<img>`s naming three addresses: one picture named twice (once by the same name with
/// a `./` before it) and a second picture, whose file's own name is the first one's.
const OWN_HTML: &str = r#"<!DOCTYPE html>
<HTML lang="en"><head><meta charset="utf-8">
<title>Northwind letterhead</title>
</head>
<body class="letterhead" id="northwind-root" style="margin:0;background-color:#ffffff">
<table width="520" cellpadding="0" cellspacing="0" border="0" style="width:520px;background-color:#fdf6e3;padding:12px"><tr>
  <td style="vertical-align:top;padding-right:16px"><img src="light/logo.png" alt="Northwind mark" width="120" height="80" style="border:0"></td>
  <td style="font-family:Georgia,serif">
    <table cellpadding="0" cellspacing="0" border="0" style="background-color:#ffffff;padding:8px"><tr>
      <td><div>Priya Raman - Northwind Atelier</div>
          <div>Studio Two, Harbour Lane</div>
          <div><a href="mailto:hello@example.org">hello@example.org</a></div></td>
    </tr></table>
  </td>
</tr></table>
<div style="padding-top:10px"><img src="./light/logo.png" alt="Northwind mark, smaller" width="64" height="42" style="border:0"></div>
<div style="background-color:#1b1b1b;padding:6px"><img src="dark/logo.png" alt="Northwind in the dark" width="80" height="60" style="border:0"></div>
</body></HTML>"#;

/// The three picture addresses the document names, in source order, and the two resource names
/// the owner must supply for them: the `./` spelling is the same resource.
const HTML_SOURCES: [&str; 3] = ["light/logo.png", "./light/logo.png", "dark/logo.png"];
const RESOURCES: [&str; 2] = ["light/logo.png", "dark/logo.png"];

/// The app over the sample mailbox, nothing happening by itself.
fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

/// The whole window, laid out a few frames: File › Signatures as `MailApp::ui` draws it.
fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(SIZE.0, SIZE.1)).build_ui_state(
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

/// What the host's picker was asked for: the purpose and the request token (`None` when the ask
/// names no request at all).
type Picks = Arc<Mutex<Vec<(String, Option<u64>)>>>;

/// The owner's window with a host that records what its picker was opened for and never shows a
/// native dialog; files come in through `Services::picked`, as the browser's picker delivers them.
fn window_with_picker(mut app: MailApp) -> (Harness<'static, MailApp>, Picks, Inbox, Arc<AtomicUsize>) {
    let picks: Picks = Default::default();
    let log = picks.clone();
    app.services.open_async = Some(Box::new(move |purpose, asking| {
        log.lock().unwrap_or_else(|e| e.into_inner()).push((purpose.to_string(), asking));
    }));
    let native = Arc::new(AtomicUsize::new(0));
    let counted = native.clone();
    app.services.file_dialog = Some(Box::new(move |_req| {
        counted.fetch_add(1, Ordering::SeqCst);
        // Nothing is shown and nothing is answered: the sender is dropped as this returns, so the
        // receiver finds the channel disconnected, the fallback of a host whose dialog went away.
        // The owner's road must never come here (the counter is asserted to stay at zero); a host
        // that took it is counted, not answered as if a file had been picked.
        let (_never, answer) = std::sync::mpsc::channel();
        answer
    }));
    let picked: Inbox = Default::default();
    app.services.picked = Some(picked.clone());
    (window(app), picks, picked, native)
}

/// Where the owner's control `key` with `id` was drawn this frame, and that it is on screen where
/// the owner can press it.
fn hit_rect(h: &Harness<'static, MailApp>, key: &str, id: u64) -> egui::Rect {
    let at = h.state().hit.rect(key, id).unwrap_or_else(|| panic!("the signatures page drew no `{key}` {id}: the owner has no way to do this here"));
    assert!(
        at.top() >= 0.0 && at.bottom() <= SIZE.1 && at.left() >= 0.0 && at.right() <= SIZE.0,
        "`{key}` {id} is drawn off screen at {at:?}, where the owner cannot press it"
    );
    at
}

/// The owner's own press: the pointer moved there, the button down, and up.
fn click(h: &mut Harness<'static, MailApp>, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

/// One file the host's picker delivered, as the host delivers it: its name, its bytes, and what
/// the picker it came in was opened for.
fn delivered(picked: &Inbox, name: &str, bytes: &[u8], purpose: &str, asking: Option<u64>, pick: u64) {
    picked.lock().unwrap_or_else(|e| e.into_inner()).push(Delivered::picked(name.to_string(), bytes.to_vec(), purpose, asking, pick));
}

/// What the import must leave exactly as it was found: the whole mailbox; the signature open in
/// the editor, whole (its identity, the signature as it was opened, its words with their
/// selection, formatting and undo, and whether they are unsaved); and any message being written.
/// The mailbox's signatures, each account's defaults and the messages are kept apart as well, so
/// that a failure says which of them moved before the whole mailbox is compared.
#[derive(Clone, Debug, PartialEq)]
struct Frozen {
    mailbox: Mailbox,
    signatures: Vec<Signature>,
    editor: Option<SignatureEdit>,
    defaults: Vec<(u64, String, String)>,
    composing: Vec<u64>,
    messages: usize,
}

fn frozen(h: &Harness<'static, MailApp>) -> Frozen {
    let s = &h.state().session;
    Frozen {
        mailbox: s.mailbox.clone(),
        signatures: s.mailbox.signatures.clone(),
        editor: s.signature_edit.clone(),
        defaults: s.mailbox.accounts.iter().map(|a| (a.id, a.signature.clone(), a.reply_signature().to_string())).collect(),
        composing: s.all_composing().map(|c| c.id).collect(),
        messages: s.mailbox.messages.len(),
    }
}

/// Nothing of what the import must not touch has moved.
fn untouched(h: &Harness<'static, MailApp>, before: &Frozen, when: &str) {
    let now = frozen(h);
    assert_eq!(now.signatures, before.signatures, "{when}: the mailbox's signatures changed");
    assert_eq!(
        now.editor, before.editor,
        "{when}: the signature open in the editor changed (its identity, the signature, its words, selection, formatting or unsaved state)"
    );
    assert_eq!(now.defaults, before.defaults, "{when}: an account's default signatures changed");
    assert_eq!(now.composing, before.composing, "{when}: a message being written changed");
    assert_eq!(now.messages, before.messages, "{when}: the mailbox's messages changed");
    assert!(now.mailbox == before.mailbox, "{when}: the mailbox changed");
}

/// The fixture's seed as the settled window holds it, before anything is frozen: the signature
/// open in the editor is still the one the fixture opened, with unsaved changes, its own words and
/// the words typed into it; the account's defaults are the fixture's signature; and no message is
/// being written. A fresh or empty editor here would make every later comparison vacuous.
fn seeded(h: &Harness<'static, MailApp>, editor: &str) {
    let s = &h.state().session;
    assert_eq!(
        s.signature_edit.as_ref().map(|e| e.signature.name.as_str()),
        Some(editor),
        "the settled window has another signature open in the editor, or none"
    );
    let open = s.signature_edit.as_ref().expect("the settled window has a signature open in the editor");
    assert!(open.dirty, "the settled window's open signature has no unsaved changes");
    let words = open.body.text().text();
    assert!(words.contains("Priya Raman"), "the open signature lost its own words as the window settled: {words:?}");
    assert!(words.contains("UNSAVED-WIP-4242"), "the open signature lost the words typed into it as the window settled: {words:?}");
    let account = s.mailbox.default_account().expect("the sample mailbox has an account");
    assert_eq!(account.signature, OLD_NAME, "the account's signature for new messages is not the fixture's");
    assert_eq!(account.reply_signature(), OLD_NAME, "the account's signature for replies and forwards is not the fixture's");
    assert!(s.compose_id().is_none(), "the settled window has a message open for writing");
    assert!(s.all_composing().next().is_none(), "a message is being written in the settled window");
}

/// The `src` values of `html`'s `<img>` elements, each with the byte range of its value: read by
/// the markup's own delimiters, in source order, so that what is compared is each span itself and
/// not how often some text appears.
fn src_values(html: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(found) = html[at..].find("src=\"") {
        let start = at + found + 5;
        let Some(close) = html[start..].find('"') else { break };
        let end = start + close;
        out.push((start..end, html[start..end].to_string()));
        at = end + 1;
    }
    out
}

/// The page's own name field, pressed and typed in as the owner does: whatever name the page
/// offered is cleared from the keyboard at the end of the field, and `text` is typed over it.
fn type_name(h: &mut Harness<'static, MailApp>, text: &str) {
    let field = hit_rect(h, "signatureHtmlImportName", 0);
    click(h, egui::pos2(field.right() - 4.0, field.center().y));
    // The caret is at the end of what the page offered; a word delete takes it away, and the same
    // the other way takes away anything left to the right of where the press landed.
    let ctrl = egui::Modifiers { ctrl: true, command: true, ..egui::Modifiers::NONE };
    for (key, modifiers) in [(egui::Key::Backspace, ctrl), (egui::Key::Delete, ctrl)] {
        for _ in 0..12 {
            for pressed in [true, false] {
                h.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
                h.step();
            }
        }
    }
    h.event(egui::Event::Text(text.into()));
    h.run_steps(2);
}

/// The owner's flow, end to end: Import HTML…, the document, each named picture, a name of their
/// own, Import.
#[test]
fn the_owner_imports_html_and_picks_each_named_picture() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut a = app();
    // A signature that is the account's default, opened in the page's editor, with the owner's
    // words typed into it and not saved.
    a.execute("signature.set", json!({"name": OLD_NAME, "text": OLD_TEXT, "default": true})).expect("the fixture makes its signature");
    a.execute("signature.open", json!({"name": OLD_NAME})).expect("the fixture opens it in the editor");
    a.execute("compose.type", json!({"text": UNSAVED})).expect("the owner types words they have not saved");
    let editor = a.session.signature_edit.as_ref().map(|e| e.signature.name.clone()).expect("a signature is open in the editor");
    assert!(a.session.signature_edit.as_ref().is_some_and(|e| e.dirty), "the fixture's signature has no unsaved words");
    assert!(a.session.compose_id().is_none(), "the fixture started a message");
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).expect("File shows the signatures page");

    let (mut h, picks, picked, native) = window_with_picker(a);
    h.run_steps(2);
    assert_eq!(h.state().ui.backstage_page, "signatures", "the fixture is not on the signatures page");
    assert!(
        h.state().hit.rect("signature", 0).is_some_and(|r| r.height() > 0.0),
        "the signatures page drew no list of signatures, so nothing of its page ran"
    );
    seeded(&h, &editor);
    let before = frozen(&h);

    // The owner presses Import HTML…: one file is asked for, for that and nothing else yet.
    let button = hit_rect(&h, "signatureHtmlImport", 0).center();
    assert!(h.query_by_label_contains("Import HTML").is_some(), "the page's import button says nothing of importing HTML");
    click(&mut h, button);
    assert_eq!(native.load(Ordering::SeqCst), 0, "Import HTML… opened a native file dialog on the owner's screen");
    let asked = picks.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(asked.len(), 1, "Import HTML… asked for {:?}, and should have asked once for the document", asked);
    assert_eq!(asked[0].0, "signatureHtml", "the picker was opened for `{}`, not for the document", asked[0].0);
    let html_ticket = asked[0].1.unwrap_or_else(|| panic!("the document was asked for with no request token: {asked:?}"));
    untouched(&h, &before, "after the owner pressed Import HTML…");

    // The document arrives, as the host delivers a picked file: the page shows its two logical
    // picture names as slots of their own, and asks for nothing yet.
    delivered(&picked, HTML_FILE, OWN_HTML.as_bytes(), "signatureHtml", Some(html_ticket), 1);
    h.run_steps(3);
    untouched(&h, &before, "after the HTML document was delivered");
    let first_slot = hit_rect(&h, "signatureHtmlImportAsset", 0);
    let second_slot = hit_rect(&h, "signatureHtmlImportAsset", 1);
    assert!(!first_slot.intersects(second_slot), "the two picture slots are drawn over one another: {first_slot:?}, {second_slot:?}");
    for resource in RESOURCES {
        assert!(
            h.query_by_label_contains(resource).is_some(),
            "the page does not show the logical picture name `{resource}`, so the owner cannot see which file it wants"
        );
    }
    assert_eq!(picks.lock().unwrap_or_else(|e| e.into_inner()).len(), 1, "the page asked for a picture before the owner chose one");

    // The owner types the name the new signature is to have.
    type_name(&mut h, NEW_NAME);
    untouched(&h, &before, "after the owner typed the new signature's name");

    // Each slot, picked by the owner: a pick of its own, with a token of its own; and the host's
    // answer for it arrives under that slot's own name, `logo.png`, as both files are called.
    let first_slot = hit_rect(&h, "signatureHtmlImportAsset", 0);
    click(&mut h, first_slot.center());
    let asked = picks.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(asked.len(), 2, "the first slot asked for {:?}, and should have asked once for its picture", asked);
    assert_eq!(asked[1].0, "signatureAsset", "the first slot's picker was opened for `{}`", asked[1].0);
    let light_ticket = asked[1].1.unwrap_or_else(|| panic!("the first picture was asked for with no request token: {asked:?}"));
    assert_ne!(light_ticket, html_ticket, "the picture's pick has the document's own request token: {asked:?}");
    delivered(&picked, "logo.png", LIGHT_PNG, "signatureAsset", Some(light_ticket), 2);
    h.run_steps(3);
    untouched(&h, &before, "after the first picture was delivered");

    let second_slot = hit_rect(&h, "signatureHtmlImportAsset", 1);
    click(&mut h, second_slot.center());
    let asked = picks.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(asked.len(), 3, "the second slot asked for {:?}, and should have asked once for its picture", asked);
    assert_eq!(asked[2].0, "signatureAsset", "the second slot's picker was opened for `{}`", asked[2].0);
    let dark_ticket = asked[2].1.unwrap_or_else(|| panic!("the second picture was asked for with no request token: {asked:?}"));
    assert_ne!(dark_ticket, html_ticket, "the second picture's pick has the document's own request token: {asked:?}");
    assert!(dark_ticket > light_ticket, "the picks' request tokens are not increasing ({light_ticket}, {dark_ticket}): {asked:?}");
    delivered(&picked, "logo.png", DARK_PNG, "signatureAsset", Some(dark_ticket), 3);
    h.run_steps(3);
    untouched(&h, &before, "after both pictures were delivered, with nothing left to choose");
    assert_eq!(picks.lock().unwrap_or_else(|e| e.into_inner()).len(), 3, "delivering a picture asked for another file");

    // The owner presses Import: one signature is stored, and only what the import must add moves.
    let just_before = frozen(&h);
    let commit = hit_rect(&h, "signatureHtmlImportCommit", 0).center();
    assert!(h.query_by_label("Import").is_some(), "the page's last button is not one that says Import");
    click(&mut h, commit);
    assert_eq!(native.load(Ordering::SeqCst), 0, "the final Import opened a native file dialog");
    let after = frozen(&h);
    let names: Vec<String> = after.signatures.iter().map(|s| s.name.clone()).collect();
    assert_eq!(
        after.signatures.len(),
        just_before.signatures.len() + 1,
        "the final Import stored {} signatures, not one: {names:?}",
        after.signatures.len()
    );
    assert!(names.iter().any(|n| n == NEW_NAME), "the new signature is not named `{NEW_NAME}`, as the owner typed it: {names:?}");
    for old in &just_before.signatures {
        assert!(after.signatures.contains(old), "the signature that was there changed: {:?} is not among {names:?}", old.name);
    }
    assert_eq!(
        after.editor, just_before.editor,
        "the import changed the signature open in the editor (its words, selection, formatting or unsaved state), or switched to another"
    );
    assert_eq!(after.defaults, just_before.defaults, "the import changed an account's default signatures");
    assert_eq!(after.composing, just_before.composing, "the import started or changed a message being written");
    assert_eq!(after.messages, just_before.messages, "the import changed the mailbox's messages");
    assert_eq!(
        h.state().session.signature_edit.as_ref().map(|e| e.signature.name.as_str()),
        Some(editor.as_str()),
        "the import closed or replaced the signature open in the editor"
    );

    // What was stored: the owner's document every byte of it but its three picture addresses,
    // each written as the address the signature holds that picture under; and the two pictures,
    // each kept once with its own bytes, though both files are called `logo.png`.
    let stored = h
        .state()
        .session
        .mailbox
        .signature(NEW_NAME)
        .unwrap_or_else(|| panic!("the import stored no signature named `{NEW_NAME}`: {names:?}"))
        .clone();
    // The whole mailbox: what it was the moment before Import, with the stored signature appended
    // to its signatures, and nothing else of it changed.
    let mut expected_mailbox = just_before.mailbox.clone();
    expected_mailbox.signatures.push(stored.clone());
    assert!(h.state().session.mailbox == expected_mailbox, "the final Import changed the mailbox beyond appending the one new signature");
    let spans = src_values(OWN_HTML);
    assert_eq!(spans.len(), 3, "the fixture's document does not name the three pictures it is meant to: {spans:?}");
    let values: Vec<&str> = spans.iter().map(|(_, value)| value.as_str()).collect();
    assert_eq!(values, HTML_SOURCES, "the fixture's document names other pictures than the three it is meant to");
    assert_eq!(
        mailcraft_mime_model::signature_import::required_assets(OWN_HTML).expect("the fixture's document is one the importer reads"),
        RESOURCES.iter().map(|r| (*r).to_string()).collect::<Vec<String>>(),
        "the fixture's document does not need the two logical resources, in the order the page's slots are numbered in"
    );
    assert_eq!(stored.pictures().len(), 2, "the signature holds {} pictures, not the document's two", stored.pictures().len());
    let light = stored.pictures().first().expect("the first picture").clone();
    let dark = stored.pictures().get(1).expect("the second picture").clone();
    assert_eq!(light.data.as_slice(), LIGHT_PNG, "the first picture is not the first file's own bytes");
    assert_eq!(dark.data.as_slice(), DARK_PNG, "the second picture is not the second file's own bytes");
    assert_eq!((light.name.as_str(), dark.name.as_str()), ("logo.png", "logo.png"), "a picture is not named by its file's own name");
    assert_ne!(light.id, dark.id, "the two pictures are one picture");
    let mut expected = String::with_capacity(OWN_HTML.len());
    let mut at = 0usize;
    for (i, (span, _)) in spans.iter().enumerate() {
        // The first two places name the first picture; the third names the second.
        let picture = stored.picture(if i == 2 { dark.id } else { light.id }).expect("the picture the document names");
        expected.push_str(&OWN_HTML[at..span.start]);
        expected.push_str("cid:");
        expected.push_str(&mailcraft_engine::mime::rich::internal_cid(picture));
        at = span.end;
    }
    expected.push_str(&OWN_HTML[at..]);
    assert_eq!(
        stored.html(),
        Some(expected.as_str()),
        "the stored document is not the owner's own, every byte of it but its three picture addresses"
    );
    // The same document and the same two files, through the constructor the command itself uses:
    // what the window stored is what the producer makes of those inputs.
    let assets = [
        mailcraft_mime_model::signature_import::Asset { name: RESOURCES[0].to_string(), data: LIGHT_PNG.to_vec() },
        mailcraft_mime_model::signature_import::Asset { name: RESOURCES[1].to_string(), data: DARK_PNG.to_vec() },
    ];
    let produced =
        mailcraft_mime_model::signature_import::import(NEW_NAME, OWN_HTML, &assets).expect("the constructor makes a signature of this document");
    assert_eq!(stored.html(), produced.html(), "the window's signature is not the one the constructor makes of the same document and files");
    assert_eq!(stored.pictures(), produced.pictures(), "the window's pictures are not the constructor's, place for place");
}

/// The scripted road: `signature.import` as a script or an agent asks it takes its own three
/// parameters and nothing else, opens no picker and no dialog, and stores nothing.
#[test]
fn a_scripted_import_never_opens_a_picker() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut a = app();
    a.execute("signature.set", json!({"name": OLD_NAME, "text": OLD_TEXT, "default": true})).expect("the fixture makes its signature");
    a.execute("signature.open", json!({"name": OLD_NAME})).expect("the fixture opens it in the editor");
    a.execute("compose.type", json!({"text": UNSAVED})).expect("the owner types words they have not saved");
    let editor = a.session.signature_edit.as_ref().map(|e| e.signature.name.clone()).expect("a signature is open in the editor");
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).expect("File shows the signatures page");

    let (mut h, picks, picked, native) = window_with_picker(a);
    h.run_steps(2);
    seeded(&h, &editor);
    let before = frozen(&h);

    // No document, a path, and a caller's own flags: each refused for its own parameters' sake,
    // none of them a way to a picker.
    let cases = [
        (json!({}), "`name` (string) is required"),
        (json!({"path": "/synthetic/never-read/northwind-letterhead.html"}), "`path` is no parameter here"),
        (json!({"name": NEW_NAME, "html": "<p>Northwind</p>", "assets": [], "default": true}), "`default` is no parameter here"),
    ];
    for (params, reason) in cases {
        let err =
            h.state_mut().execute("signature.import", params.clone()).expect_err("a scripted import without its document's own data is refused");
        assert!(err.contains(reason), "`signature.import` {params} was refused for another reason: {err}");
        assert!(
            !err.contains("signatureHtml") && !err.contains("signatureAsset"),
            "the refusal of `signature.import` {params} names a picker's purpose: {err}"
        );
    }

    assert_eq!(native.load(Ordering::SeqCst), 0, "a scripted import opened a native file dialog");
    let asked = picks.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert!(asked.is_empty(), "a scripted import opened the picker: {asked:?}");
    assert!(picked.lock().unwrap_or_else(|e| e.into_inner()).is_empty(), "a scripted import delivered files by itself");
    assert!(h.state().file_dialog.is_none(), "a scripted import left a file dialog waiting");
    assert!(h.state().dialog.is_none(), "a scripted import opened a dialog");
    let names: Vec<String> = h.state().session.mailbox.signatures.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names.len(), before.signatures.len(), "a scripted import stored a signature: {names:?}");
    assert!(h.state().session.signature_edit.is_some(), "the fixture's signature is no longer open in the editor");
    untouched(&h, &before, "after three scripted imports");
}
