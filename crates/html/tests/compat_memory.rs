// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The email-compatibility checker's memory stays bounded at the most it reads (AGENTS.md › Never
//! crash: cap allocations). It keeps each declaration it reads with a copy of its name and value,
//! so what it holds grows with the document: when it read up to [`MAX_INPUT_WITH_PICTURES`]
//! (62 MiB), a document of short declarations took about 2.7 GB. It now refuses anything over
//! [`MAX_INPUT`] (2 MiB, far above a signature's 64 KB), where the same documents take under
//! 100 MB.
//!
//! The peak is this process's own (`VmHWM` in `/proc/self/status`, so on Linux alone), which is
//! why this file holds one test: another one running beside it would count too.

use mailcraft_html::compat::{CheckError, check};
use mailcraft_html::{MAX_INPUT, MAX_INPUT_WITH_PICTURES};

/// The most this process may have held at once, the documents themselves included.
const MAX_PEAK_BYTES: u64 = 150 * 1024 * 1024;

/// The process's peak resident memory so far, in bytes, where the system says it.
fn peak_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// `head`, as many `unit`s as fit, and `tail`, in at most `limit` bytes, written in place so that
/// the document is all the fixture holds.
fn fill((head, unit, tail): (&str, &str, &str), limit: usize) -> String {
    let n = limit.saturating_sub(head.len() + tail.len()) / unit.len();
    let mut html = String::with_capacity(limit);
    html.push_str(head);
    std::iter::repeat_n(unit, n).for_each(|u| html.push_str(u));
    html.push_str(tail);
    assert!(html.len() <= limit && html.len() + unit.len() > limit, "fixture guard: the document fills its limit");
    html
}

/// The documents that made the checker hold the most for their size: a start, the part repeated
/// to fill the limit, and an end. Each is made when it is checked, one at a time.
const HEAVIEST: [(&str, (&str, &str, &str)); 5] = [
    ("a sheet of short declarations", ("<style>a{", "b:c;", "}</style>")),
    ("a style attribute of short declarations", ("<p style=\"", "b:c;", "\">x</p>")),
    ("the same with a character reference", ("<p style=\"&amp;", "b:c;", "\">x</p>")),
    ("a sheet of short rules", ("<style>", "a{b:c}", "</style>")),
    ("short rules in @media", ("<style>@media screen{", "a{b:c}", "}</style>")),
];

/// Many custom properties of their own names: each one a definition of the checker's index.
fn own_properties(limit: usize) -> String {
    let mut html = String::from("<style>a{");
    let mut n = 0u64;
    while html.len() + 40 < limit {
        html.push_str(&format!("--p{n}:x;"));
        n += 1;
    }
    html.push_str("}</style>");
    html
}

#[test]
fn the_checker_holds_little_at_the_most_it_reads_and_refuses_more() {
    let Some(before) = peak_bytes() else {
        eprintln!("no VmHWM on this system: the peak is not measured here");
        return;
    };
    for (name, parts) in HEAVIEST {
        let html = fill(parts, MAX_INPUT);
        assert!(check(&html).is_ok(), "{name} is checked");
        let peak = peak_bytes().unwrap_or(u64::MAX);
        assert!(peak <= MAX_PEAK_BYTES, "{name} of {} bytes: a peak of {peak} bytes (from {before})", html.len());
    }
    let html = own_properties(MAX_INPUT);
    assert_eq!(check(&html), Err(CheckError::TooManyFindings), "a definition is a finding each");
    drop(html);
    // The most a message is read at, pictures and all: refused before anything is kept.
    for (name, parts) in HEAVIEST {
        let html = fill(parts, MAX_INPUT_WITH_PICTURES);
        assert_eq!(check(&html), Err(CheckError::OverInput), "{name} of {} bytes is refused", html.len());
        let peak = peak_bytes().unwrap_or(u64::MAX);
        assert!(peak <= MAX_PEAK_BYTES, "{name} of {} bytes: a peak of {peak} bytes (from {before})", html.len());
    }
}
