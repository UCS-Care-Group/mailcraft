// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every picture source a signature's HTML names is listed, each with whether a `<template>` holds
//! it as the writer reads the document: the writer leaves such a picture out of every message, so
//! nothing needs its file, but its address is still there to be judged. A `<template>` that an
//! element the writer leaves out whole holds (or a hidden one) ends where that element ends, as
//! the writer reads it, so a picture after it is not held by it.
//!
//! The documents are invented; nothing here is fetched.

use mailcraft_html::{ImageSourceError, image_sources, quotable};

fn sources(html: &str) -> Result<Vec<(String, bool)>, ImageSourceError> {
    image_sources(html).map(|v| v.into_iter().map(|s| (s.src, s.in_template)).collect())
}

fn held(src: &str, in_template: bool) -> (String, bool) {
    (src.to_string(), in_template)
}

#[test]
fn a_picture_in_a_template_is_listed_as_held_by_it() {
    assert_eq!(sources(r#"<template><img src="a.png"></template><img src="b.png">"#), Ok(vec![held("a.png", true), held("b.png", false)]));
    assert_eq!(
        sources(r#"<template><template></template><img src="c.png"></template><img src="d.png">"#),
        Ok(vec![held("c.png", true), held("d.png", false)])
    );
    assert_eq!(sources(r#"<TEMPLATE><img src="e.png"></TEMPLATE><img src="f.png">"#), Ok(vec![held("e.png", true), held("f.png", false)]));
    // An end tag with no template open changes nothing.
    assert_eq!(sources(r#"</template><img src="g.png">"#), Ok(vec![held("g.png", false)]));
}

/// Documents where a `<template>` is opened inside something the writer leaves out whole, or
/// where the writer's reading ends it otherwise; `{img}` is the picture after it.
const ENDED: [&str; 11] = [
    "<svg><template></svg>{img}",
    "<math><template></math>{img}",
    "<noscript><template></noscript>{img}",
    "<select><template></select>{img}",
    "<button><template></button>{img}",
    "<object><template></object>{img}",
    "<iframe><template></iframe>{img}",
    "<head><template></head><body>{img}</body>",
    "<p hidden><template></p>{img}",
    "<div style=\"display:none\"><template></div>{img}",
    "<svg><template><template></svg>{img}",
];

#[test]
fn a_template_an_element_left_out_whole_holds_ends_with_that_element() {
    let src = "https://pictures.example.net/after.png";
    for shape in ENDED {
        let html = shape.replace("{img}", &format!("<img src=\"{src}\">"));
        assert!(quotable(&html).contains(src), "fixture guard: the writer writes the picture: {html}");
        assert_eq!(sources(&html), Ok(vec![held(src, false)]), "{html}");
    }
}

#[test]
fn a_picture_left_out_for_another_reason_is_not_held_by_a_template() {
    // The writer leaves these out too, but no template holds them: whoever asks for files sees
    // them, and refuses a hidden picture as such.
    let src = "https://pictures.example.net/hidden.png";
    for shape in ["<p hidden>{img}</p>", "<div style=\"display:none\">{img}</div>", "<svg>{img}</svg>", "<noscript>{img}</noscript>"] {
        let html = shape.replace("{img}", &format!("<img src=\"{src}\">"));
        assert!(!quotable(&html).contains(src), "fixture guard: the writer leaves the picture out: {html}");
        assert_eq!(sources(&html), Ok(vec![held(src, false)]), "{html}");
    }
}

#[test]
fn a_picture_is_held_by_a_template_exactly_when_the_writer_leaves_it_out_for_that() {
    let shapes = [
        "<template>{a}</template>{b}",
        "<template><svg></svg>{a}</template>{b}",
        "<template><p hidden></p>{a}</template>{b}",
        "<template><template>{a}</template>{a}</template>{b}",
        "<svg><template></svg>{b}<template>{a}</template>",
        "<template>{a}",
    ];
    let (a, b) = ("https://pictures.example.net/a.png", "https://pictures.example.net/b.png");
    for shape in shapes {
        let html = shape.replace("{a}", &format!("<img src=\"{a}\">")).replace("{b}", &format!("<img src=\"{b}\">"));
        let written = quotable(&html);
        let listed = sources(&html).unwrap_or_else(|e| panic!("{html}: {e}"));
        assert!(!listed.is_empty(), "{html}: every picture is listed");
        for (src, in_template) in &listed {
            assert_eq!(*in_template, !written.contains(src.as_str()), "{html}: {src} (written: {written})");
        }
    }
}

#[test]
fn an_authored_body_inside_a_template_does_not_end_that_template() {
    let src = "https://pictures.example.net/inert.png";
    let html = format!("<p>BEFORE</p><template><body><img src=\"{src}\"></template><p>AFTER</p>");
    assert_eq!(sources(&html), Ok(vec![held(src, true)]));
    let scope = mailcraft_html::AuthoredScope::new("mc-authored-test").unwrap();
    let authored = mailcraft_html::authored_fragment_with_rebound_cids(&html, &scope, &mut |cid| Some(cid.to_string())).unwrap();
    assert!(authored.body.contains("BEFORE") && authored.body.contains("AFTER"));
    assert!(!authored.body.contains(src), "the authored lister and writer disagreed");
}
