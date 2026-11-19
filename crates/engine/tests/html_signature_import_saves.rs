// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What `signature.import` keeps can be saved as it is: its pictures are judged as
//! `signature.save` judges them, a `<template>` around one or not. A picture a template holds asks
//! for no file and goes out in no message, but its address is still the signature's HTML, so a
//! file's name, a `file:` address, a `cid:` the signature doesn't own or a picture written into
//! the HTML (`data:`, which Save's email-compatibility check refuses with no fix) is refused at
//! the import instead of making a signature that can be opened and never saved again. It is
//! refused in words of its own: Save's tell the owner to bring the picture's file in with the
//! import, which can't help a picture the import never binds. A picture outside a template keeps
//! Save's words. Anything else Save's email-compatibility check refuses, in a template or not (a
//! web picture in a format email can't show, a form's controls, a media player, a picture sized
//! by its style alone), is imported all the same, to be fixed in the editor of File › Signatures,
//! which shows a template's content as source and underlines each error: Save refuses it until
//! then, and changes nothing.
//!
//! The people and the pictures are invented; nothing here reads a file, fetches or sends.

use mailcraft_engine::cmd::base64_encode;
use mailcraft_engine::{Session, sample};
use mailcraft_html::compat::Severity;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// A one-pixel PNG, whole and valid: the fixture the other signature import tests use.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
const NAME: &str = "Lisbon Bindery";

/// What the import says of a picture a `<template>` holds that the signature can't keep.
const HELD_WORDS: &str = "this HTML names a picture inside a <template> that is neither on the web nor one of this signature's own: such a picture is never sent, so take it out or give it a web address (https://…)";
/// What Save says of a picture that is neither on the web nor one of the signature's own.
const SAVE_WORDS: &str = "this HTML names a picture that is neither on the web nor one of this signature's own: give it a web address (https://…), or name the picture's file relative to the HTML file and bring both in with Import HTML…";

/// Documents whose picture outside any `<template>` is neither on the web nor the signature's own,
/// a template beside it or not: Save's words stay Save's.
const OUTSIDE: [&str; 3] = [
    "<img src=\"missing.png\"><p>x</p>",
    "<img src=\"file:///home/someone/missing.png\"><p>x</p>",
    "<p>x</p><template><b>held</b></template><img src=\"missing.png\">",
];

/// What Save says of a picture written into the HTML (`data:`): the compatibility check's refusal.
const COMPAT_WORDS: &str = "this signature has email compatibility errors to fix before it can be saved";

/// Documents whose `<template>` holds a picture a signature can't keep, with the picture files
/// given beside each (`logo.png`, or none) and the start of what Save says of it.
const HELD_REFUSED: [(&str, bool, &str); 6] = [
    ("<template><img src=\"missing.png\"></template><p>x</p>", false, "this HTML names a picture"),
    ("<template><img src=\"file:///home/someone/missing.png\"></template><p>x</p>", false, "this HTML names a picture"),
    ("<template><img src=\"cid:foreign@x.example\"></template><p>x</p>", false, "this HTML names a picture"),
    (
        "<p>Ana Pereira</p><img src=\"logo.png\" alt=\"Bindery mark\"><template><img src=\"missing.png\"></template>",
        true,
        "this HTML names a picture",
    ),
    ("<p>Ana Pereira</p><img src=\"logo.png\" alt=\"Bindery mark\"><template><img src=\"logo.png\"></template>", true, "this HTML names a picture"),
    // A picture written into the HTML: Save's compatibility check refuses it, with no fix.
    (
        "<template><img src=\"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAACklEQVR4nGMAAQAABQABDQottAAAAABJRU5ErkJggg==\"></template><p>x</p>",
        false,
        COMPAT_WORDS,
    ),
];

/// Documents Save's email-compatibility check refuses for something other than a picture written
/// into the HTML, each with the same document fixed as the editor's findings say: what a
/// `<template>` holds, which the editor shows as source, and what is outside one.
const COMPAT_KEPT: [(&str, &str); 9] = [
    (
        "<template><img src=\"https://pictures.example.net/held.svg\"></template><p>x</p>",
        "<template><img src=\"https://pictures.example.net/held.png\"></template><p>x</p>",
    ),
    (
        "<template><img src=\"https://pictures.example.net/held.webp\"></template><p>x</p>",
        "<template><img src=\"https://pictures.example.net/held.png\"></template><p>x</p>",
    ),
    ("<template><form><input></form></template><p>x</p>", "<template><p>held</p></template><p>x</p>"),
    ("<template><video></video></template><p>x</p>", "<p>x</p>"),
    ("<select><option>Lisbon</option></select><p>x</p>", "<p>Lisbon</p><p>x</p>"),
    ("<button>Write to us</button><p>x</p>", "<p>Write to us</p><p>x</p>"),
    ("<object data=\"https://pictures.example.net/mark.png\"></object><p>x</p>", "<p>x</p>"),
    ("<textarea>Ana Pereira</textarea><p>x</p>", "<p>Ana Pereira</p><p>x</p>"),
    (
        "<p>Ana Pereira</p><img src=\"https://pictures.example.net/mark.png\" alt=\"Bindery mark\" style=\"width:120px\">",
        "<p>Ana Pereira</p><img width=\"120\" src=\"https://pictures.example.net/mark.png\" alt=\"Bindery mark\" style=\"width:120px\">",
    ),
];

/// Documents the import keeps, each with whether `logo.png` is given beside it: with and without
/// pictures, a template without pictures, and a template holding a picture on the web.
const KEPT: [(&str, bool); 6] = [
    ("<p>Ana Pereira</p>", false),
    ("<p>Ana Pereira</p><img src=\"logo.png\" alt=\"Bindery mark\" width=\"120\">", true),
    ("<p>Ana Pereira</p><img src=\"https://pictures.example.net/mark.png\" alt=\"Bindery mark\">", false),
    ("<template><p>Not shown</p></template><p>Ana Pereira</p>", false),
    ("<template><img src=\"https://pictures.example.net/held.png\"></template><p>Ana Pereira</p>", false),
    ("<p>Ana Pereira</p><img src=\"logo.png\" alt=\"Bindery mark\"><template><b>held</b></template>", true),
];

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 11, 17).unwrap(), 10, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn import(s: &mut Session, name: &str, body: &str, logo: bool) -> Result<Value, String> {
    let html = format!("<html><body>{body}</body></html>");
    let assets = if logo { json!([{"name": "logo.png", "data": base64_encode(ONE_PIXEL_PNG)}]) } else { json!([]) };
    s.run("signature.import", &json!({"name": name, "html": html, "assets": assets})).map_err(|e| e.to_string())
}

/// What `signature.save` says of `body` written into an open signature that owns no pictures (or
/// `signature.html` at once, where it is refused there).
fn save_says(body: &str) -> String {
    let mut s = session();
    import(&mut s, "Plain", "<p>x</p>", false).unwrap();
    s.run("signature.open", &json!({"name": "Plain"})).unwrap();
    let html = format!("<html><body>{body}</body></html>");
    let refused = match s.run("signature.html", &json!({"html": html})) {
        Err(e) => Err(e),
        Ok(_) => s.run("signature.save", &json!({})),
    };
    refused.map(|v| panic!("{body}: saved: {v}")).unwrap_or_else(|e| e.to_string())
}

#[test]
fn an_import_whose_template_holds_a_picture_save_refuses_is_refused_in_words_of_its_own() {
    for (body, logo, save_words) in HELD_REFUSED {
        let mut s = session();
        let before = s.mailbox.clone();
        let refused = import(&mut s, NAME, body, logo).map(|v| format!("imported: {v}"));
        let why = refused.err().unwrap_or_else(|| panic!("{body}: imported"));
        // Not Save's remedy: a file given beside the document is never bound to a picture a
        // template holds (where its name is also one the import was given, too).
        assert_eq!(why, HELD_WORDS, "{body}: the import's own words");
        assert!(s.mailbox == before, "{body}: nothing is kept");
        assert!(s.mailbox.signature(NAME).is_none(), "{body}: no signature is made");
        // Save would refuse the same document as before, in its own words.
        let without_own = body.replace("<img src=\"logo.png\" alt=\"Bindery mark\">", "");
        let save = save_says(&without_own);
        assert!(save.starts_with(save_words) && save != HELD_WORDS, "{body}: Save says {save}");
    }
}

#[test]
fn a_picture_outside_a_template_keeps_saves_words() {
    for body in OUTSIDE {
        assert_eq!(save_says(body), SAVE_WORDS, "{body}: Save's words");
    }
    // The import binds a picture outside a template to a file it was given, so a missing one is
    // its own refusal, as before, even beside a template whose picture is refused.
    let mut s = session();
    let both = "<img src=\"missing.png\"><template><img src=\"missing.png\"></template>";
    let why = import(&mut s, NAME, both, false).err().unwrap_or_else(|| panic!("{both}: imported"));
    assert!(why.contains("names no supplied file") && why != HELD_WORDS, "{both}: {why}");
    assert!(s.mailbox.signature(NAME).is_none(), "{both}: no signature is made");
}

#[test]
fn an_imported_signature_opens_and_saves_unchanged() {
    for (body, logo) in KEPT {
        let mut s = session();
        import(&mut s, NAME, body, logo).unwrap_or_else(|e| panic!("{body}: refused: {e}"));
        let kept = s.mailbox.signature(NAME).cloned().unwrap_or_else(|| panic!("{body}: kept"));
        s.run("signature.open", &json!({"name": NAME})).unwrap_or_else(|e| panic!("{body}: open: {e}"));
        s.run("signature.save", &json!({})).unwrap_or_else(|e| panic!("{body}: saved unchanged: {e}"));
        assert_eq!(s.mailbox.signature(NAME), Some(&kept), "{body}: the save keeps what the import made");
        // And written again as it is, while closed, as `signature.html` keeps HTML.
        let html = kept.html().unwrap_or_default().to_string();
        s.run("signature.close", &json!({})).unwrap_or_else(|e| panic!("{body}: close: {e}"));
        s.run("signature.html", &json!({"name": NAME, "html": html})).unwrap_or_else(|e| panic!("{body}: written again: {e}"));
    }
}

#[test]
fn an_import_save_would_refuse_for_its_compatibility_is_kept_and_fixed_in_the_editor() {
    for (body, fixed) in COMPAT_KEPT {
        // What the document stands on: Save refuses it, with the compatibility check's words.
        assert_eq!(save_says(body), COMPAT_WORDS, "{body}: Save's words");
        let mut s = session();
        import(&mut s, NAME, body, false).unwrap_or_else(|e| panic!("{body}: refused: {e}"));
        let kept = s.mailbox.signature(NAME).cloned().unwrap_or_else(|| panic!("{body}: kept"));
        let html = kept.html().unwrap_or_else(|| panic!("{body}: an HTML signature")).to_string();
        // The editor underlines what the checker finds in it, a template's content included.
        assert!(has_error(&html), "{body}: the checker finds an error in what was kept: {html}");
        s.run("signature.open", &json!({"name": NAME})).unwrap_or_else(|e| panic!("{body}: open: {e}"));
        let why = s.run("signature.save", &json!({})).err().unwrap_or_else(|| panic!("{body}: saved unchanged"));
        assert_eq!(why.to_string(), COMPAT_WORDS, "{body}: Save names the compatibility errors");
        assert_eq!(s.mailbox.signature(NAME), Some(&kept), "{body}: the refused save changes nothing");
        // The remedy: the HTML fixed in the editor, which then saves.
        let edit = format!("<html><body>{fixed}</body></html>");
        s.run("signature.html", &json!({"html": edit})).unwrap_or_else(|e| panic!("{fixed}: edited: {e}"));
        s.run("signature.save", &json!({})).unwrap_or_else(|e| panic!("{fixed}: saved: {e}"));
        let saved = s.mailbox.signature(NAME).and_then(|g| g.html()).unwrap_or_default().to_string();
        assert!(saved != html && !has_error(&saved), "{fixed}: the fixed HTML is kept: {saved}");
    }
}

/// Whether the email-compatibility checker finds an error in `html` (what Save refuses).
fn has_error(html: &str) -> bool {
    mailcraft_html::compat::check(html).unwrap_or_default().iter().any(|f| f.severity == Severity::Error)
}
