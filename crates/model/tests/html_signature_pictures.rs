// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An HTML signature may bring its own pictures: the HTML the owner wrote and the pictures it
//! names are kept together, through being saved and opened again.

use mailcraft_model::Signature;

/// The smallest PNG signature there is, and some bytes after it: enough for the model, which
/// reads only a picture's kind from its own first bytes, never its pixels.
fn png(extra: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(8 + extra, 7);
    bytes
}

#[test]
fn an_html_signature_keeps_the_picture_it_shows_through_a_save_and_a_reopen() {
    // A rich signature with a picture, then the owner's own HTML, naming that picture.
    let mut sig = Signature::plain_text("Work", "Avery Quinn\nLumen Studio");
    let logo = sig.add_picture("logo.png", png(64), Some("Lumen logo"), Some(0)).unwrap();
    let cid = format!("cid:sig-{logo}@signature.invalid");
    let html = format!(
        "<table><tr><td><img src=\"{cid}\" alt=\"Lumen logo\" width=\"120\"></td>\
         <td><b>Avery Quinn</b><br>Lumen Studio</td></tr></table>"
    );
    let stand_in = sig.plain();

    // The picture stands in its own HTML: kept with it, not refused.
    sig.set_html(Some(html.clone())).expect("an HTML signature keeps the picture its HTML shows");
    assert_eq!(sig.html(), Some(html.as_str()));
    assert_eq!(sig.plain(), stand_in);

    // Saved and opened again: the HTML exactly as written, and the picture still beside it.
    let json = serde_json::to_value(&sig).unwrap();
    assert_eq!(json["html"].as_str(), Some(html.as_str()));
    let back: Signature = serde_json::from_value(json).unwrap();
    assert_eq!(back.html(), Some(html.as_str()));
    assert_eq!(back.pictures().len(), 1);
    let kept = back.pictures().first().expect("the picture it came with");
    assert_eq!((kept.id, kept.data.as_slice()), (logo, png(64).as_slice()));
    assert_eq!(back.plain(), stand_in);
}
