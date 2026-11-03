// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The cost of reading `@media` (#599): the budget holds at every width and theme.
use super::tests::assert_within_budget_in;
use super::*;

/// A megabyte of mail whose sheet is a wall of `@media` blocks, nested and not, true and false:
/// at any one width and theme the look costs what the budget says.
#[test]
fn a_wall_of_media_blocks_costs_what_the_budget_says_at_every_width_and_theme() {
    let mut sheet = String::new();
    let mut k = 0usize;
    while sheet.len() < MAX_SHEET - 400 {
        let query = match k % 5 {
            0 => "(max-width: 480px)",
            1 => "(min-width: 481px) and (prefers-color-scheme: dark)",
            2 => "screen and (min-width: 0)",
            3 => "print",
            _ => "(prefers-color-scheme: dark), (max-width: 320px)",
        };
        let colour = k * 7919 % 0xff_ffff;
        sheet += &format!("@media {query}{{.c{a}, p.k{a} > span{{color:#{colour:06x};display:block}} .c{a} i{{font-weight:bold}}}}", a = k % 40);
        k += 1;
    }
    let html = format!("<style>{sheet}</style>{}", "<p class='k3 c3'><span class='c4 c5'>x</span><i class=c7>y</i></p>".repeat(14_000));
    assert!(html.len() > 800_000, "{}", html.len());
    for (width, dark) in [(300.0, true), (479.0, false), (1000.0, true), (1000.0, false)] {
        assert_within_budget_in(&format!("media wall {width} {dark}"), &html, &Env::for_pane(width, dark));
    }
}

/// Reading a sheet for a pane costs about what reading it for another does: a sheet that is all
/// `@media` is scanned once per level of nesting at most, never more.
#[test]
fn nesting_does_not_multiply_the_work() {
    let body = ".a{color:#f00}".repeat(MAX_SHEET / 3 / 14);
    let started = std::time::Instant::now();
    for levels in 1..=media::MAX_NESTING + 4 {
        let sheet = format!("{}{body}{}", "@media screen{".repeat(levels), "}".repeat(levels));
        let _ = rules(&sheet);
    }
    assert!(started.elapsed().as_secs() < 4, "{:?}", started.elapsed());
}
