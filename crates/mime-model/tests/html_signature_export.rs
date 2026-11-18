// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An HTML signature is sent as its owner wrote it: the tables of its HTML, their inline styling
//! and its words are kept, and the picture it names by the address it holds inside the signature is
//! sent as the inline part the message gives it.

use mailcraft_mime_model::rich::signature_html;
use mailcraft_model::Signature;
use mailcraft_model::signature::Picture;

/// A one-pixel PNG, whole and valid. Only its kind matters here: the model reads a picture type
/// from its own first bytes, and this fixture says nothing about pixels.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[test]
fn signature_html_keeps_the_nested_tables_styles_and_text_of_an_html_signature_and_rebinds_its_picture() {
    // The words kept beside the HTML are a stand-in, and here they say other words in another
    // shape: lines of text, not the owner's tables. Redrawing the HTML from them cannot pass.
    let mut sig = Signature::plain_text("Work", "Avery Quinn\nLumen Studio");
    let logo = sig.add_picture("logo.png", ONE_PIXEL_PNG.to_vec(), Some("Northwind mark"), Some(0)).expect("a signature takes a PNG");
    // The owner's own HTML: two tables, one inside the other, its styling inline, and the picture
    // named by the address it has inside the signature.
    let own = format!(
        "<table width=\"520\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" \
           style=\"width:520px;background-color:#fdf6e3;padding:12px;font-family:'Fira Code',monospace\">\
         <tr><td>\
           <table cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"background-color:#ffffff;padding:8px\">\
             <tr>\
               <td style=\"padding-right:16px\"><img src=\"cid:sig-{logo}@signature.invalid\" alt=\"Northwind mark\" width=\"120\" height=\"80\" style=\"border:0\"></td>\
               <td style=\"font-family:'Fira Code',monospace\">\
                 <div>Priya Raman - Northwind Atelier</div>\
                 <div>Studio Two, Harbour Lane</div>\
                 <div><a href=\"mailto:hello@example.org\">hello@example.org</a></div>\
               </td>\
             </tr>\
           </table>\
         </td></tr>\
       </table>"
    );
    sig.set_html(Some(own.clone())).expect("the model keeps the HTML of a signature that has pictures");
    assert_eq!(sig.html(), Some(own.as_str()), "the fixture is an HTML signature");

    // The message's own part names for this signature's pictures.
    let out = signature_html(&sig, &|p: &Picture| format!("part-{}@message.invalid", p.id));
    let sent = format!("cid:part-{logo}@message.invalid");

    // The owner's two tables, one inside the other, and no table of the helper's own making.
    assert_eq!(out.matches("<table").count(), 2, "the owner's nested tables: {out}");
    // The inline styling they carry, in whatever spacing and quoting a writer gives it.
    let css: String = out.chars().filter(|c| !c.is_whitespace()).collect();
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
        assert!(css.contains(kept), "the owner's `{kept}` kept in: {out}");
    }
    // The words the owner wrote.
    assert!(out.contains("Priya Raman - Northwind Atelier"), "the owner's first line: {out}");
    assert!(out.contains("Studio Two, Harbour Lane"), "the owner's second line: {out}");
    assert!(out.contains("hello@example.org"), "the owner's address: {out}");
    // The picture, once, as the part this message gives it, at the size the owner declared.
    assert_eq!(out.matches(&sent).count(), 1, "the picture as this message's own part: {out}");
    let sized = css.replace('\'', "\"");
    assert!(sized.contains("width=\"120\""), "the width the owner declared: {out}");
    assert!(sized.contains("height=\"80\""), "the height the owner declared: {out}");
    // Not the address it had inside the signature, and not the stand-in's words in its place.
    assert!(!out.contains("signature.invalid"), "the signature's internal address must not be sent: {out}");
    assert!(!out.contains("Avery Quinn") && !out.contains("Lumen Studio"), "the stand-in is not the owner's HTML: {out}");
}
