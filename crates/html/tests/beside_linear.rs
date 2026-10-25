// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A picture followed by a long run of blank spans must parse in time linear in the input
//! (AU2-01): the spaces beside the picture are dropped in one pass, not one span at a time.

use std::time::{Duration, Instant};

use mailcraft_html::parse;

fn blanks(bytes: usize) -> String {
    let unit = "<i>&nbsp;</i>&nbsp;";
    unit.repeat(bytes / unit.len())
}

fn time_of(html: &str) -> Duration {
    let start = Instant::now();
    let _ = parse(html);
    start.elapsed()
}

#[test]
fn blank_spans_beside_a_picture_parse_in_linear_time() {
    let run = blanks(2 * 1024 * 1024);
    let with_picture = format!("<p><img src=\"cid:a@example.org\" width=\"40\" height=\"40\">{run}y</p>");
    let control = format!("<p>{run}y</p>");
    let best = |html: &str| (0..3).map(|_| time_of(html)).min().unwrap_or_default();
    let base = best(&control);
    let picture = best(&with_picture);
    let floor = Duration::from_millis(50);
    assert!(picture <= base.max(floor) * 8, "picture run took {picture:?}, control {base:?}: not linear");
}

#[test]
fn text_after_a_picture_keeps_its_place_beside_it() {
    let blocks = parse("<p><img src=\"cid:a@example.org\">&nbsp;<i>&nbsp;</i>&nbsp;y</p>");
    assert!(blocks.iter().any(|b| b.beside && b.text() == "y"), "{blocks:?}");
}
