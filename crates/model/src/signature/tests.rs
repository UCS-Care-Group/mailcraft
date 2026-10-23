// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use proptest::prelude::*;

use super::*;
use crate::rich::{Font, Format, OBJECT};

/// The smallest PNG signature there is, and some bytes after it.
fn png(extra: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(8 + extra, 7);
    bytes
}

#[test]
fn a_plain_signature_of_an_earlier_version_opens_as_rich_text() {
    let sig: Signature = serde_json::from_str(r#"{"name": "Standard", "text": "Avery Quinn\nLumen Studio"}"#).unwrap();
    assert_eq!((sig.name.as_str(), sig.plain().as_str(), sig.pictures().len()), ("Standard", "Avery Quinn\nLumen Studio", 0));
    assert!(!sig.body().has_formatting());
}

#[test]
fn a_rich_signature_with_pictures_round_trips_and_keeps_its_words_for_earlier_versions() {
    let mut sig = Signature::plain_text("Work", "Avery Quinn\nLumen Studio");
    let mut body = sig.body().clone();
    body.format(0, 11, &Format::Color(Some("#1F6FEB".into()))).unwrap();
    body.format(0, 11, &Format::Font(Some(Font::Georgia))).unwrap();
    sig.set_body(body).unwrap();
    let logo = sig.add_picture("lumen-logo.png", png(100), None, Some(0)).unwrap();
    let badge = sig.add_picture("badge.gif", b"GIF89a....".to_vec(), Some("Certified"), None).unwrap();
    sig.set_picture(logo, Some(Some(900)), None).unwrap();
    sig.layout = Layout::PictureLeft;
    let json = serde_json::to_value(&sig).unwrap();
    // An earlier version reads `name` and `text`, and finds the words there.
    assert_eq!(json["text"], "[lumen logo]Avery Quinn\nLumen Studio[Certified]");
    let back: Signature = serde_json::from_value(json).unwrap();
    assert_eq!(back, sig);
    let p = back.picture(logo).unwrap();
    assert_eq!((p.mime.as_str(), p.width, p.alt.as_str()), ("image/png", Some(MAX_WIDTH), "lumen logo"));
    assert_eq!(back.picture(badge).unwrap().mime, "image/gif");
    assert_eq!(back.body().text().chars().filter(|c| *c == OBJECT).count(), 2);
    assert_eq!(back.body().style_at(3).color.as_deref(), Some("#1f6feb"));
}

#[test]
fn only_pictures_by_their_own_bytes_and_within_the_caps_are_taken() {
    let mut sig = Signature::plain_text("S", "x");
    // WebP, and something calling itself a PNG.
    for bytes in [b"RIFF\x04\0\0\0WEBPVP8 ".to_vec(), b"not a picture.png".to_vec()] {
        assert!(sig.add_picture("a.png", bytes, None, None).unwrap_err().to_string().contains("PNG, JPEG or GIF"));
    }
    assert!(sig.add_picture("big.png", png(MAX_PICTURE_BYTES), None, None).unwrap_err().to_string().contains("256 KB"));
    sig.add_picture("a.png", png(MAX_PICTURE_BYTES - 8), None, None).unwrap();
    sig.add_picture("b.png", png(MAX_PICTURE_BYTES - 8), None, None).unwrap();
    assert!(sig.add_picture("c.png", png(1), None, None).unwrap_err().to_string().contains("512 KB together"));
    let mut many = Signature::plain_text("M", "x");
    for n in 0..MAX_PICTURES {
        many.add_picture(&format!("{n}.png"), png(10), None, None).unwrap();
    }
    assert!(many.add_picture("one-more.png", png(10), None, None).unwrap_err().to_string().contains("at most 10"));
    assert!(sig.add_picture("ok.png", png(1), None, None).is_err() && sig.pictures().len() == 2);
}

#[test]
fn text_and_pictures_always_agree() {
    let mut sig = Signature::plain_text("S", "Avery");
    let id = sig.add_picture("logo.png", png(10), None, None).unwrap();
    // Deleting the picture's place in the text lets it go.
    sig.set_body(RichText::from_plain("Avery")).unwrap();
    assert!(sig.pictures().is_empty());
    let id2 = sig.add_picture("logo.png", png(10), None, None).unwrap();
    assert_ne!(sig.picture(id2), None);
    sig.remove_picture(id2).unwrap();
    assert!(sig.pictures().is_empty() && !sig.body().text().contains(OBJECT) && sig.remove_picture(id).is_err());
    // A file whose text names a picture it doesn't have, or holds pictures that aren't pictures.
    let json = r#"{"name": "S", "body": {"paragraphs": [{"runs": [{"text": "x", "picture": 9}, {"text": "Avery"}]}]},
        "pictures": [{"id": 3, "name": "evil.png", "mime": "image/png", "data": "PGh0bWw+", "alt": "x"}]}"#;
    let sig: Signature = serde_json::from_str(json).unwrap();
    assert_eq!((sig.plain().as_str(), sig.pictures().len()), ("Avery", 0));
}

#[test]
fn junk_in_a_signature_never_stops_the_mailbox_opening() {
    for json in [
        r#"{"name": 5}"#,
        r#"{"name": "S", "body": "nonsense", "pictures": 7, "layout": "sideways", "details": []}"#,
        r#"{"name": "S", "pictures": [{"id": 1, "data": "!!!"}, 4, null]}"#,
    ] {
        // Either it opens (cleaned) or the field is refused; never a panic.
        let _ = serde_json::from_str::<Signature>(json);
    }
    let sig: Signature = serde_json::from_str(r#"{"name": "S", "body": "nonsense", "layout": "sideways", "details": []}"#).unwrap();
    assert_eq!((sig.layout, sig.plain().as_str()), (Layout::Inline, ""));
}

proptest! {
    /// Whatever pictures and text a file holds, what opens agrees with itself and stays in the caps.
    #[test]
    fn any_stored_signature_opens_consistent(
        ids in proptest::collection::vec(0u32..20, 0..15),
        sizes in proptest::collection::vec(0usize..300_000, 0..15),
        words in "[a-z ]{0,20}",
    ) {
        let runs: Vec<serde_json::Value> = ids.iter().map(|id| serde_json::json!({"text": "x", "picture": id}))
            .chain(std::iter::once(serde_json::json!({"text": words}))).collect();
        let pictures: Vec<serde_json::Value> = ids.iter().zip(&sizes).map(|(id, n)| {
            serde_json::json!({"id": id, "data": mailcraft_charset::base64::STANDARD.encode(png(*n)), "alt": "a"})
        }).collect();
        let json = serde_json::json!({"name": "S", "body": {"paragraphs": [{"runs": runs}]}, "pictures": pictures});
        let sig: Signature = serde_json::from_value(json).unwrap();
        prop_assert!(sig.pictures().len() <= MAX_PICTURES && sig.picture_bytes() <= MAX_PICTURES_BYTES);
        prop_assert!(sig.pictures().iter().all(|p| p.data.len() <= MAX_PICTURE_BYTES));
        let held: std::collections::HashSet<u32> = sig.pictures().iter().map(|p| p.id).collect();
        let used: std::collections::HashSet<u32> = sig.body().pictures().into_iter().collect();
        prop_assert_eq!(held, used);
    }
}
