// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whatever `signature.import` keeps opens in the editor, and Save of it unchanged either keeps it
//! as the import made it (and it is written back as it is while closed) or is refused for its
//! email compatibility alone, changing nothing: the editor of File › Signatures underlines each
//! error, a `<template>`'s content included, and offers its fixes, so the owner fixes it there.
//! No picture Save would refuse is kept behind such an error: the import judges every picture as
//! Save does, a template around it or not, and refuses the document instead. A refused import
//! leaves the mailbox as it was.
//!
//! The documents are generated deterministically from short lists of parts, which the earlier
//! reviews of the import used to look for such documents: where a `<template>` is opened (inside
//! the elements the quoter leaves out, closed or not), the pictures after it (every kind of
//! address), and what Save's email-compatibility check refuses (a web picture in a format email
//! can't show, a form's controls, a media player, a picture sized by its style alone). A failure
//! names its document; the run is bounded and fast.
//!
//! The people and the pictures are invented; nothing here reads a file, fetches or sends.

use mailcraft_engine::cmd::base64_encode;
use mailcraft_engine::cmd::signature::sendable_pictures;
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
/// What Save says of HTML the email-compatibility checker finds an error in.
const COMPAT_WORDS: &str = "this signature has email compatibility errors to fix before it can be saved";
/// The same picture written into the HTML.
const PNG_DATA: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAACklEQVR4nGMAAQAABQABDQottAAAAABJRU5ErkJggg==";
/// The signature's own picture, given as `logo.png`, put first in half of the documents.
const OWN: &str = "<p>Ana Pereira</p><img src=\"logo.png\" alt=\"Bindery mark\">";

/// Where a `<template>` is opened: nowhere, plainly, and inside each element the quoter leaves
/// out, where its end never comes.
const PREFIXES: &[&str] = &[
    "",
    "<template>",
    "<template><body>",
    "<svg><template></svg>",
    "<math><template></math>",
    "<noscript><template></noscript>",
    "<select><template></select>",
    "<button><template></button>",
    "<object><template></object>",
    "<p hidden><template></p>",
    "<div style=\"display:none\"><template></div>",
    "<template><svg><template></svg></template>",
    "<svg><template><template></svg>",
    "<!-- <template> -->",
    "<script><template></script>",
    "<textarea><template></textarea>",
    "<template></template></template>",
    "<table><tr><td hidden><template></tr></table>",
    "<template><template></template>",
    "<TEMPLATE>",
    "<xmp><template></xmp>",
    "<template><xmp></template>",
];

/// What comes after the prefix: pictures with every kind of address, and what Save's
/// email-compatibility check refuses. `DATA` stands for [`PNG_DATA`].
const PARTS: &[&str] = &[
    "<img src=\"missing.png\">",
    "<img src=\"logo.png\">",
    "<img src=\"./logo.png\">",
    "<img src=\"file:///home/someone/x.png\">",
    "<img src=\"/abs/x.png\">",
    "<img src=\"C:\\x.png\">",
    "<img src=\"//cdn.example/x.png\">",
    "<img src=\"cid:foreign@x.example\">",
    "<img src=\"CID:sig-1@signature.invalid\">",
    "<img src=\" cid:sig-1@signature.invalid \">",
    "<img src=\"cid:sig-2@signature.invalid\">",
    "<img src=\"https://pictures.example.net/a.png\">",
    "<img src=\"HTTPS://pictures.example.net/a.png\">",
    "<img src=\"http://pictures.example.net/a.png\">",
    "<img src=\"https://pictures.example.net/a b.png\">",
    "<img src=\"https://pictures.example.net/a.png\" hidden>",
    "<img src=\"javascript:alert(1)\">",
    "<img src=\"ftp://x.example/a.png\">",
    "<img src=\"\">",
    "<img>",
    "<img src=\"&#99;id:foreign@x.example\">",
    "<img src=\"&#x6c;ogo.png\">",
    "<img src=\"DATA\">",
    "<img src=\"https://pictures.example.net/held.svg\">",
    "<img src=\"https://pictures.example.net/held.webp\">",
    "<img src=\"https://pictures.example.net/a.png\" alt=\"Mark\" style=\"width:120px\">",
    "<form><input></form>",
    "<video></video>",
    "<p>held</p>",
];

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 11, 17).unwrap(), 10, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn import(s: &mut Session, body: &str, logo: bool) -> Result<Value, String> {
    let html = format!("<html><body>{body}</body></html>");
    let assets = if logo { json!([{"name": "logo.png", "data": base64_encode(ONE_PIXEL_PNG)}]) } else { json!([]) };
    s.run("signature.import", &json!({"name": NAME, "html": html, "assets": assets})).map_err(|e| e.to_string())
}

/// The imported signature opened and saved unchanged: `Ok(true)` when the save keeps it as the
/// import made it and it is written back as it is while closed, `Ok(false)` when the save is
/// refused for its email compatibility alone (an error the checker finds, no picture Save would
/// refuse behind it) and changes nothing, and why it is neither otherwise.
fn saved_unchanged(s: &mut Session) -> Result<bool, String> {
    let kept = s.mailbox.signature(NAME).cloned().ok_or_else(|| "not kept".to_string())?;
    let html = kept.html().unwrap_or_default().to_string();
    s.run("signature.open", &json!({"name": NAME})).map_err(|e| format!("open: {e}"))?;
    if let Err(e) = s.run("signature.save", &json!({})) {
        if e.to_string() != COMPAT_WORDS {
            return Err(format!("saved unchanged: {e}"));
        }
        if s.mailbox.signature(NAME) != Some(&kept) {
            return Err("the refused save changed it".into());
        }
        if !mailcraft_html::compat::check(&html).unwrap_or_default().iter().any(|f| f.severity == Severity::Error) {
            return Err("refused for its compatibility, but the checker finds no error".into());
        }
        return sendable_pictures(&kept, &html).map(|()| false).map_err(|e| format!("a picture Save refuses: {e}"));
    }
    if s.mailbox.signature(NAME) != Some(&kept) {
        return Err("the save changed it".into());
    }
    s.run("signature.close", &json!({})).map_err(|e| format!("close: {e}"))?;
    s.run("signature.html", &json!({"name": NAME, "html": html})).map_err(|e| format!("written back: {e}"))?;
    Ok(true)
}

#[test]
fn whatever_the_import_keeps_opens_and_saves_or_names_its_compatibility_errors() {
    let (mut saved, mut to_fix, mut refused, mut failures) = (0usize, 0usize, 0usize, Vec::new());
    for with_own in [false, true] {
        for prefix in PREFIXES {
            for part in PARTS {
                for close in ["", "</template>"] {
                    let part = part.replace("DATA", PNG_DATA);
                    let body = format!("{}{prefix}{part}{close}<p>x</p>", if with_own { OWN } else { "" });
                    let mut s = session();
                    let before = s.mailbox.clone();
                    match import(&mut s, &body, with_own) {
                        Err(why) => {
                            refused += 1;
                            if s.mailbox != before {
                                failures.push(format!("{body}: refused ({why}) but the mailbox changed"));
                            }
                        }
                        Ok(_) => match saved_unchanged(&mut s) {
                            Ok(true) => saved += 1,
                            Ok(false) => to_fix += 1,
                            Err(why) => failures.push(format!("{body}: imported, then {why}")),
                        },
                    }
                }
            }
        }
    }
    let shown: Vec<&String> = failures.iter().take(12).collect();
    assert!(failures.is_empty(), "{} of {} documents: {shown:#?}", failures.len(), saved + to_fix + refused + failures.len());
    // Every outcome is there, so the property is not true for want of documents.
    assert!(saved >= 200 && to_fix >= 200 && refused >= 200, "saved {saved}, to fix {to_fix}, refused {refused}");
}
