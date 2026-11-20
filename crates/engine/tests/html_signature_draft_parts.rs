// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An HTML signature kept in a draft, read back from the mailbox: its own tables and inline
//! styling are there, and every picture its HTML names is an inline part of that same saved
//! message under the Content-ID the HTML names there, with the picture's own bytes — the
//! binding the message goes out with. A draft must never keep the address a picture has inside
//! the signature itself (`sig-<id>@signature.invalid`), which stands for it there and nowhere
//! else.
//!
//! One producer-to-consumer tracer, through the commands the app runs: a signature seeded in the
//! mailbox, an account default, `mail.new`, `compose.saveDraft`, and the saved message read back
//! from the mailbox and through the MIME writer and parser. The people, hosts and picture are
//! invented; the picture is the synthetic one-pixel PNG the MIME tests use, and says nothing of
//! pixels.

use mailcraft_engine::{Session, sample};
use mailcraft_mime_model::WriteOptions;
use mailcraft_mime_model::rich::{ImgRef, from_html_with_pictures};
use mailcraft_model::{FolderKind, Signature};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// A one-pixel PNG, whole and valid: a picture by its own first bytes, the same fixture the MIME
/// integration tests use. Nothing here says anything about pixels.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The owner's own HTML: two tables, one inside the other, their styling inline, and the picture
/// named by `cid` — the address the model gives a signature's picture, as the app reads it.
fn own_html(cid: &str) -> String {
    format!(
        "<table width=\"520\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" \
           style=\"width:520px;background-color:#fdf6e3;padding:12px;font-family:'Fira Code',monospace\">\
         <tr><td>\
           <table cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"background-color:#ffffff;padding:8px\">\
             <tr>\
               <td style=\"padding-right:16px\"><img src=\"cid:{cid}\" alt=\"Northwind mark\" width=\"120\" height=\"80\" style=\"border:0\"></td>\
               <td style=\"font-family:'Fira Code',monospace\">\
                 <div>Priya Raman - Northwind Atelier</div>\
                 <div>Studio Two, Harbour Lane</div>\
                 <div><a href=\"mailto:hello@example.org\">hello@example.org</a></div>\
               </td>\
             </tr>\
           </table>\
         </td></tr>\
       </table>"
    )
}

/// Every picture `html` names by a `cid:` address, as the real HTML reader of a draft of this
/// program finds them: by what it reads of each `<img>`, not by a search of the text.
fn image_cids(html: &str) -> Vec<String> {
    let mut cids = Vec::new();
    let mut claim = |img: &ImgRef| {
        cids.push(img.cid.clone());
        Some(0)
    };
    let _ = from_html_with_pictures(html, &mut claim);
    cids
}

/// Every picture `html` names is an inline part of the same message, under the Content-ID it
/// names there, with the picture's own bytes; no address the signature keeps for itself is left
/// in the HTML, and no inline picture part is left unnamed.
fn assert_pictures_bound(label: &str, html: &str, parts: &[(String, Vec<u8>)]) {
    let cids = image_cids(html);
    assert!(!cids.is_empty(), "{label}: the HTML names no picture at all: {html}");
    assert!(
        parts.iter().any(|(_, bytes)| bytes.as_slice() == ONE_PIXEL_PNG),
        "{label}: the signature's picture is no inline part with its own bytes: {parts:?}"
    );
    for cid in &cids {
        assert!(
            !cid.contains("signature.invalid"),
            "{label}: the HTML keeps the signature's own address `{cid}` instead of a Content-ID of this message; its inline parts are {parts:?}: {html}"
        );
        let part = parts
            .iter()
            .find(|(id, _)| id == cid)
            .unwrap_or_else(|| panic!("{label}: `{cid}` is no Content-ID of an inline part of this message: {parts:?}"));
        assert_eq!(part.1.as_slice(), ONE_PIXEL_PNG, "{label}: the inline part `{cid}` is not the picture's own bytes");
    }
    for (id, _) in parts {
        assert!(cids.contains(id), "{label}: the inline part `{id}` is named by no picture of the HTML: {html}");
    }
}

#[test]
fn a_saved_draft_binds_an_html_signatures_picture_to_its_inline_part() {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().unwrap().id;

    // The signature as the model is seeded with one: plain text, then the picture, then the
    // owner's own HTML, naming the picture by the address the model itself gives it. (Set either
    // of those the other way round and the model refuses the fixture, not the behaviour tested.)
    let mut sig = Signature::plain_text("Work", "Priya Raman\nNorthwind Atelier");
    let logo = sig.add_picture("northwind-mark.png", ONE_PIXEL_PNG.to_vec(), Some("Northwind mark"), Some(0)).expect("a signature takes a PNG");
    let picture = sig.picture(logo).expect("the picture just added");
    let own = own_html(&mailcraft_mime_model::rich::internal_cid(picture));
    sig.set_html(Some(own.clone())).expect("the model keeps the HTML of a signature that has its pictures");
    assert_eq!(sig.html(), Some(own.as_str()), "the fixture is an HTML signature");
    s.mailbox.signatures.push(sig);
    run(&mut s, "signature.defaults", json!({"account": account, "new": "Work"}));

    // A new message, which the account's signature goes into, saved as a draft by the command the
    // app's Save Draft runs.
    run(&mut s, "mail.new", json!({"account": account, "body": "Thanks,"}));
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");

    // What the mailbox kept of it, read back by that id: not what the test put in.
    let draft = s.mailbox.message(saved).expect("the saved draft is in the mailbox");
    assert!(draft.draft, "saved as a draft");
    assert_eq!(s.mailbox.folder(draft.folder).map(|f| f.kind), Some(FolderKind::Drafts), "saved in Drafts");
    let html = draft.body.html.as_deref().expect("an HTML message keeps its HTML");
    assert!(html.contains("Priya Raman - Northwind Atelier"), "the owner's words are kept: {html}");

    // The owner's own tables, one inside the other, and the styling they carry.
    assert_eq!(html.matches("<table").count(), 2, "the owner's nested tables, and none of the helper's own: {html}");
    let css: String = html.chars().filter(|c| !c.is_whitespace()).collect();
    for kept in [
        "background-color:#fdf6e3",
        "padding:12px",
        "width:520px",
        "font-family:",
        "FiraCode",
        "monospace",
        "background-color:#ffffff",
        "padding-right:16px",
    ] {
        assert!(css.contains(kept), "the owner's `{kept}` kept in: {html}");
    }

    // The pictures of the draft: the signature's one, an inline part of it with its own bytes.
    let parts: Vec<(String, Vec<u8>)> =
        draft.attachments.iter().filter(|a| a.inline && a.mime == "image/png").map(|a| (a.content_id.clone(), a.data.to_vec())).collect();
    assert_eq!(parts.len(), 1, "the signature's one picture is an inline part of the draft: {:?}", draft.attachments);
    // The tracer itself: the HTML's picture addresses are those parts' Content-IDs, in this message.
    assert_pictures_bound("the saved draft", html, &parts);

    // And it is the binding a draft is written as, too: the same addresses on the wire.
    let wire = mailcraft_mime_model::write(draft, &WriteOptions { include_bcc: true, ..Default::default() });
    let parsed = mailcraft_mime_model::parse(&wire);
    let wire_html = parsed.body.html.as_deref().expect("the written draft keeps its HTML");
    let wire_parts: Vec<(String, Vec<u8>)> =
        parsed.attachments.iter().filter(|a| a.inline && a.mime == "image/png").map(|a| (a.content_id.clone(), a.data.to_vec())).collect();
    assert_eq!(wire_parts.len(), 1, "the picture written as an inline part: {:?}", parsed.attachments);
    assert_pictures_bound("the written draft", wire_html, &wire_parts);
}
