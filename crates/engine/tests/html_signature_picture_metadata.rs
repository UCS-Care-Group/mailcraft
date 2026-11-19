// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An HTML signature's pictures are sized, described and taken out in its HTML, which is what
//! goes out: `signature.picture.set` and `signature.picture.remove` refuse such a signature,
//! closed or open, and change nothing, rather than change a picture's record that the HTML then
//! contradicts (a size and a description the message doesn't show, or a payload gone that the
//! HTML still names). A rich signature's pictures are changed by them as before. And a picture
//! address in the HTML written with a character reference (`&#64;` for `@`) names the same
//! picture as the literal one, for the check and for the binding alike.
//!
//! The people and the picture are invented; nothing here reads a file, fetches or sends.

use mailcraft_engine::cmd::base64_encode;
use mailcraft_engine::{Session, sample};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// A one-pixel PNG, whole and valid: the fixture the other signature import tests use.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
const NAME: &str = "Lisbon Bindery";
/// What both commands say of an HTML signature.
const REFUSAL: &str = "an HTML signature's pictures are sized, described and taken out in its HTML";

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A session with an imported HTML signature that owns one picture, and that picture's id.
fn imported() -> (Session, u64) {
    let now = DateTime::new(Date::new(2026, 11, 17).unwrap(), 10, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let html = "<html><body><p>Ana Pereira</p><img src=\"logo.png\" alt=\"Bindery mark\" width=\"120\"></body></html>";
    run(&mut s, "signature.import", json!({"name": NAME, "html": html, "assets": [{"name": "logo.png", "data": base64_encode(ONE_PIXEL_PNG)}]}));
    let sig = s.mailbox.signature(NAME).expect("imported");
    assert!(sig.html().is_some() && sig.pictures().len() == 1, "fixture guard: an HTML signature with one picture");
    let id = u64::from(sig.pictures()[0].id);
    (s, id)
}

#[test]
fn a_closed_html_signatures_picture_is_not_changed_beside_its_html() {
    for (id, params) in [
        ("signature.picture.set", json!({"name": NAME, "width": 60, "alt": "Another mark"})),
        ("signature.picture.set", json!({"name": NAME, "width": null})),
        ("signature.picture.remove", json!({"name": NAME})),
    ] {
        let (mut s, picture) = imported();
        let mut params = params;
        params["id"] = json!(picture);
        let before = s.mailbox.clone();
        let err = s.run(id, &params).expect_err("refused");
        assert_eq!(err.to_string(), REFUSAL, "{id} {params}");
        assert!(s.mailbox == before, "{id} {params}: the mailbox is as it was");
    }
}

#[test]
fn an_open_html_signatures_picture_is_not_changed_beside_its_html() {
    for (id, params) in [("signature.picture.set", json!({"width": 60, "alt": "Another mark"})), ("signature.picture.remove", json!({}))] {
        let (mut s, picture) = imported();
        run(&mut s, "signature.open", json!({"name": NAME}));
        let mut params = params;
        params["id"] = json!(picture);
        let (before, editor) = (s.mailbox.clone(), s.signature_edit.clone());
        let err = s.run(id, &params).expect_err("refused");
        assert_eq!(err.to_string(), REFUSAL, "{id} {params}");
        assert!(s.mailbox == before, "{id}: the mailbox is as it was");
        assert_eq!(s.signature_edit, editor, "{id}: the open signature is as it was");
    }
}

#[test]
fn a_picture_address_written_with_a_character_reference_is_the_signatures_own() {
    for open in [false, true] {
        let (mut s, _) = imported();
        let stored = s.mailbox.signature(NAME).and_then(|x| x.html()).expect("its HTML").to_string();
        assert!(stored.contains("cid:sig-1@signature.invalid"), "fixture guard: {stored}");
        let encoded = stored.replace("cid:sig-1@signature.invalid", "cid:sig-1&#64;signature.invalid");
        let params = if open {
            run(&mut s, "signature.open", json!({"name": NAME}));
            json!({"html": encoded})
        } else {
            json!({"name": NAME, "html": encoded})
        };
        run(&mut s, "signature.html", params);
        if open {
            run(&mut s, "signature.save", json!({}));
        }
        let sig = s.mailbox.signature(NAME).expect("kept");
        assert_eq!(sig.html(), Some(encoded.as_str()), "open={open}: the HTML is kept as written");
        assert_eq!(sig.pictures().len(), 1, "open={open}: the signature keeps its picture");
        assert_eq!(sig.body().pictures(), [1], "open={open}: its text holds the picture where the HTML shows it");
    }
    // A picture the signature doesn't own stays refused, however it is written.
    let (mut s, _) = imported();
    let stored = s.mailbox.signature(NAME).and_then(|x| x.html()).expect("its HTML").to_string();
    let foreign = stored.replace("cid:sig-1@signature.invalid", "cid:sig-9&#64;signature.invalid");
    let err = s.run("signature.html", &json!({"name": NAME, "html": foreign})).expect_err("refused");
    assert_eq!(err.to_string(), "this HTML names a picture this signature doesn't have");
}
