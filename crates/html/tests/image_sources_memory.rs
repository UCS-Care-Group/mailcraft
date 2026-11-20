// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Listing a document's pictures holds little beyond the document, up to the most it reads
//! (AGENTS.md › Never crash: cap allocations). To know which pictures a `<template>` holds, the
//! lister reads the document as the signature writer does; that reading once also wrote what the
//! writer would make of it and then let it go, so a document of text that needs escaping took
//! about ten times its own size: some 0.6 GB at [`MAX_INPUT_WITH_PICTURES`] (62 MiB), the most
//! [`image_sources`] reads. Nothing is written now, and what is listed is the same.
//!
//! The peak is this process's own (`VmHWM` in `/proc/self/status`, so on Linux alone), which is
//! why this file holds one test: another one running beside it would count too.

use mailcraft_html::{ImageSourceError, MAX_INPUT_WITH_PICTURES, image_sources};

/// The most this process may have held at once: the document itself (62 MiB) and a little more
/// (about 68 MB was measured; writing even the end tags of the last case took some 150 MB).
const MAX_PEAK_BYTES: u64 = 100 * 1024 * 1024;

/// What every document ends with: a picture a template holds, and one after it.
const TAIL: &str = "<template><img src=\"https://pictures.example.net/held.png\"></template><img src=\"https://pictures.example.net/out.png\">";

/// The process's peak resident memory so far, in bytes, where the system says it.
fn peak_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// `head`, as many `unit`s as fit, spaces, then `close` and [`TAIL`]: exactly `limit` bytes.
fn fill((head, unit, close): (&str, &str, &str), limit: usize) -> String {
    let ends = head.len() + close.len() + TAIL.len();
    let n = limit.saturating_sub(ends) / unit.len();
    let mut html = String::with_capacity(limit + 1);
    html.push_str(head);
    std::iter::repeat_n(unit, n).for_each(|u| html.push_str(u));
    while html.len() + close.len() + TAIL.len() < limit {
        html.push(' ');
    }
    html.push_str(close);
    html.push_str(TAIL);
    assert_eq!(html.len(), limit, "fixture guard: the document is at the limit");
    html
}

/// The documents whose reading wrote the most for their size: text that escapes to six times its
/// length, the same after a `<body>` (which starts the reading again), an attribute the writer
/// keeps, and paragraphs that end one another. A start, the part repeated, and an end.
const HEAVIEST: [(&str, (&str, &str, &str)); 4] = [
    ("text that needs escaping", ("<p>", "\"\">&", "</p>")),
    ("the same after a body", ("<head></head><body><p>", "\"\">&", "</p>")),
    ("a kept attribute that needs escaping", ("<a title='", "\"\"\"\"", "'>x</a>")),
    ("paragraphs that end one another", ("", "<p>", "")),
];

#[test]
fn listing_pictures_holds_little_at_the_most_it_reads_and_refuses_more() {
    let Some(before) = peak_bytes() else {
        eprintln!("no VmHWM on this system: the peak is not measured here");
        return;
    };
    for (name, parts) in HEAVIEST {
        let mut html = fill(parts, MAX_INPUT_WITH_PICTURES);
        let sources = image_sources(&html).unwrap_or_else(|e| panic!("{name}: listed: {e:?}"));
        let found: Vec<(&str, bool)> = sources.iter().map(|s| (s.src.as_str(), s.in_template)).collect();
        assert_eq!(
            found,
            [("https://pictures.example.net/held.png", true), ("https://pictures.example.net/out.png", false)],
            "{name}: the pictures, and which one a template holds"
        );
        let peak = peak_bytes().unwrap_or(u64::MAX);
        assert!(peak <= MAX_PEAK_BYTES, "{name} of {} bytes: a peak of {peak} bytes (from {before})", html.len());
        // One byte more is more than the lister reads: refused, before anything is kept.
        html.insert(0, ' ');
        assert_eq!(image_sources(&html).map(|s| s.len()), Err(ImageSourceError::OverInput), "{name}: one byte over");
        drop(html);
        let peak = peak_bytes().unwrap_or(u64::MAX);
        assert!(peak <= MAX_PEAK_BYTES, "{name}, one byte over: a peak of {peak} bytes (from {before})");
    }
}
