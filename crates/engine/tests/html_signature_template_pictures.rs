// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A picture's address in an HTML signature is judged wherever it stands, a `<template>` around it
//! or not: a `<template>` that an element the writer leaves out whole holds (an `svg`, a `math`, a
//! `noscript`, a `select`) ends with that element, so a picture after it goes out, and is judged.
//! A `cid:` the signature doesn't own, a file's name and a `file:` address are refused where the
//! HTML is kept, closed or open, and the signature is left as it was.
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
/// The signature's own picture, as the import names it.
const OWN: &str = "<p>Ana Pereira</p><img src=\"cid:sig-1@signature.invalid\" alt=\"Bindery mark\" width=\"120\">";

/// What follows the signature's own picture: each names, after a `<template>` that the element
/// around it ends (or in a template, for the last), a picture the signature can't send.
const REFUSED: [&str; 8] = [
    "<svg><template></svg><img src=\"cid:foreign@x.example\">",
    "<noscript><template></noscript><img src=\"cid:foreign@x.example\">",
    "<select><template></select><img src=\"cid:foreign@x.example\">",
    "<p hidden><template></p><img src=\"cid:foreign@x.example\">",
    "<svg><template></svg><img src=\"other.png\">",
    "<noscript><template></noscript><img src=\"other.png\">",
    "<math><template></math><img src=\"file:///home/someone/other.png\">",
    "<template><img src=\"cid:foreign@x.example\"></template>",
];

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A session with an imported HTML signature that owns one picture.
fn imported() -> Session {
    let now = DateTime::new(Date::new(2026, 11, 17).unwrap(), 10, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let html = "<html><body><p>Ana Pereira</p><img src=\"logo.png\" alt=\"Bindery mark\" width=\"120\"></body></html>";
    run(&mut s, "signature.import", json!({"name": NAME, "html": html, "assets": [{"name": "logo.png", "data": base64_encode(ONE_PIXEL_PNG)}]}));
    let sig = s.mailbox.signature(NAME).expect("imported");
    assert!(sig.html().is_some_and(|h| h.contains("cid:sig-1@signature.invalid")), "fixture guard: the picture's address: {:?}", sig.html());
    s
}

fn document(tail: &str) -> String {
    format!("<html><body>{OWN}{tail}</body></html>")
}

#[test]
fn a_picture_after_a_template_an_element_left_out_whole_ended_is_refused_when_closed() {
    for tail in REFUSED {
        let mut s = imported();
        let before = s.mailbox.clone();
        let r = s.run("signature.html", &json!({"name": NAME, "html": document(tail)}));
        assert!(r.is_err(), "{tail}: accepted: {r:?}");
        assert!(s.mailbox == before, "{tail}: the mailbox is as it was");
    }
}

#[test]
fn a_picture_after_a_template_an_element_left_out_whole_ended_is_refused_at_the_save() {
    for tail in REFUSED {
        let mut s = imported();
        run(&mut s, "signature.open", json!({"name": NAME}));
        let before = s.mailbox.signature(NAME).cloned();
        // Refused at once (a `cid:` the signature doesn't own), or else at the save.
        let refused = s.run("signature.html", &json!({"html": document(tail)})).is_err() || s.run("signature.save", &json!({})).is_err();
        assert!(refused, "{tail}: saved");
        assert_eq!(s.mailbox.signature(NAME).cloned(), before, "{tail}: the kept signature is as it was");
    }
}

#[test]
fn a_web_picture_after_such_a_template_is_kept() {
    // Control: the same shapes with a picture on the web, which the signature may send.
    for open in [false, true] {
        let mut s = imported();
        let html = document("<svg><template></svg><img src=\"https://pictures.example.net/after.png\">");
        if open {
            run(&mut s, "signature.open", json!({"name": NAME}));
            run(&mut s, "signature.html", json!({"html": html}));
            run(&mut s, "signature.save", json!({}));
        } else {
            run(&mut s, "signature.html", json!({"name": NAME, "html": html}));
        }
        let kept = s.mailbox.signature(NAME).and_then(|x| x.html()).unwrap_or_default().to_string();
        assert!(kept.contains("https://pictures.example.net/after.png"), "open={open}: kept: {kept}");
    }
}
