// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An HTML signature goes out as its owner wrote it: its own HTML, cleaned, its words and styling
//! kept, and only the picture it names bound to the part this message sends it as.

use mailcraft_model::signature::{Layout, Picture, Signature};

use super::*;

/// A picture's first bytes and nothing else: the model reads only its kind from them. No pixel is
/// claimed here, and nothing is decoded.
fn png() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\nIDAT".to_vec()
}

/// A signature holding one picture, and the address its own HTML gives it.
fn with_picture() -> (Signature, u32, String) {
    let mut sig = Signature::plain_text("Work", "Avery Quinn\nLumen Studio");
    let id = sig.add_picture("logo.png", png(), Some("Lumen logo"), Some(0)).expect("a signature takes a PNG");
    let cid = internal_cid(sig.picture(id).expect("the picture it just took"));
    (sig, id, cid)
}

#[test]
fn an_html_signature_is_its_own_html_with_only_its_picture_bound() {
    let (mut sig, id, cid) = with_picture();
    let own = format!(
        "<table><tr><td><img src=\"cid:{cid}\" alt=\"Lumen logo\" width=\"120\"></td>\
         <td><p>Lumen Studio</p><p>cid:{cid} in words</p></td></tr></table>"
    );
    sig.set_html(Some(own)).expect("the model keeps the HTML of a signature with a picture");
    let out = signature_html(&sig, &|p: &Picture| format!("part-{}@message.invalid", p.id));
    // The picture once, as the part this message gives it.
    let sent = format!("cid:part-{id}@message.invalid");
    assert_eq!(out.matches(&sent).count(), 1, "{out}");
    // The owner's tables, their declared size and their words, and the address in the words as it
    // was written: only a picture's `src` is rebound.
    assert!(out.contains("<table") && out.contains("width=\"120\""), "{out}");
    assert!(out.contains("Lumen Studio"), "{out}");
    assert!(out.contains(&format!("cid:{cid} in words")), "{out}");
    // The stand-in text is not what is sent.
    assert!(!out.contains("Avery Quinn"), "{out}");
}

#[test]
fn a_picture_the_signature_does_not_hold_and_every_other_reference_keep_their_addresses() {
    let (mut sig, id, cid) = with_picture();
    // The id after this one belongs to no picture of this signature.
    let gone = internal_cid(&Picture { id: id.saturating_add(1), ..Default::default() });
    let own = format!("<p><a href=\"cid:{cid}\">{cid}</a></p><img src=\"cid:{gone}\" alt=\"{cid}\">");
    sig.set_html(Some(own)).expect("the model keeps the HTML of a signature with a picture");
    let out = signature_html(&sig, &|p: &Picture| format!("part-{}@message.invalid", p.id));
    // The link, its words, the description and the picture it does not hold: all as written.
    assert!(out.contains(&format!("href=\"cid:{cid}\"")), "{out}");
    assert!(out.contains(&format!(">{cid}</a>")), "{out}");
    assert!(out.contains(&format!("src=\"cid:{gone}\"")), "{out}");
    assert!(out.contains(&format!("alt=\"{cid}\"")), "{out}");
    assert!(!out.contains("part-"), "nothing was bound: {out}");
}

#[test]
fn a_hostile_html_signature_is_cleaned_as_a_message_is() {
    let (mut sig, id, cid) = with_picture();
    let own = format!(
        "<style>p{{color:red}}</style><script>fetch('https://example.org')</script>\
         <p onclick=\"steal()\">Lumen Studio</p>\
         <img src=\"cid:{cid}\" onerror=\"steal()\" style=\"background:url(https://example.org/x)\">\
         <a href=\"javascript:steal()\">here</a><div style=\"display:none\">hidden words</div>"
    );
    sig.set_html(Some(own)).expect("the model keeps the HTML of a signature with a picture");
    let out = signature_html(&sig, &|p: &Picture| format!("part-{}@message.invalid", p.id));
    let lower = out.to_ascii_lowercase();
    for bad in ["<script", "<style", "onclick", "onerror", "javascript:", "url(", "hidden words"] {
        assert!(!lower.contains(bad), "`{bad}` in {out}");
    }
    // What is sound stays: the picture, bound, and the words.
    assert!(out.contains(&format!("cid:part-{id}@message.invalid")), "{out}");
    assert!(out.contains("Lumen Studio") && out.contains(">here</a>"), "{out}");
}

#[test]
fn a_rich_signature_is_written_from_its_text_as_before() {
    let (mut sig, id, _) = with_picture();
    sig.set_picture(id, Some(Some(120)), None).expect("a width");
    sig.layout = Layout::PictureLeft;
    assert!(sig.html().is_none());
    let out = signature_html(&sig, &|p: &Picture| format!("part-{}@message.invalid", p.id));
    let head = "<table cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr><td style=\"vertical-align:top;padding-right:12px\">";
    assert!(out.starts_with(head), "{out}");
    assert!(out.contains(&format!("<img src=\"cid:part-{id}@message.invalid\" width=\"120\"")), "{out}");
}
