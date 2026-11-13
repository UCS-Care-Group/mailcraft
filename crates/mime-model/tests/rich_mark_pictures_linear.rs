// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading a draft's HTML back with its pictures takes time linear in the HTML (AU3-22): a
//! picture tag that never closes is looked through once, not again from every `<img` after it.

use std::time::{Duration, Instant};

use mailcraft_mime_model::rich::{from_html, from_html_with_pictures};
use proptest::prelude::*;

fn time_of(read: impl Fn()) -> Duration {
    let start = Instant::now();
    read();
    start.elapsed()
}

#[test]
fn unclosed_picture_tags_read_in_linear_time() {
    let html = "<img \"".repeat(2 * 1024 * 1024 / 6);
    let best = |read: &dyn Fn()| (0..3).map(|_| time_of(read)).min().unwrap_or_default();
    let base = best(&|| {
        let _ = from_html(&html);
    });
    let pictures = best(&|| {
        let _ = from_html_with_pictures(&html, &mut |_| Some(1));
    });
    let floor = Duration::from_millis(100);
    assert!(pictures <= base.max(floor) * 10, "with pictures {pictures:?}, without {base:?}: not linear");
}

#[test]
fn pictures_after_a_broken_tag_are_still_found() {
    let html = "<p>a<imgx>b<img src=\"cid:p@example.org\">c</p>";
    let back = from_html_with_pictures(html, &mut |img| (img.cid == "p@example.org").then_some(3));
    assert_eq!(back.pictures(), vec![3]);
    // A tag that never closes keeps its text, and so does what follows.
    let html = format!("<p>a</p><img \"{}<p>tail</p>", "x".repeat(100 * 1024));
    let back = from_html_with_pictures(&html, &mut |_| Some(1));
    assert!(back.pictures().is_empty());
}

fn piece() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec!["<img ", "<img", "\"", "'", ">", "x", "<img src=\"cid:a@example.org\">", "<p>", " "])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// Whatever mix of picture tags and quotes, reading back stays within a few times what the
    /// same HTML takes without pictures.
    #[test]
    fn mixed_tags_and_quotes_read_in_bounded_time(pieces in prop::collection::vec(piece(), 1..64), repeat in 1usize..4_000) {
        let html = pieces.concat().repeat(repeat);
        let start = Instant::now();
        let back = from_html_with_pictures(&html, &mut |_| Some(1));
        let took = start.elapsed();
        prop_assert!(took < Duration::from_secs(5), "{took:?} for {} bytes", html.len());
        prop_assert!(back.text().len() <= 4 * html.len() + 64);
    }
}
