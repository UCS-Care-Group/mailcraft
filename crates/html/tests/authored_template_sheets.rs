// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A style sheet after a `<template>` that is never closed (one an `svg` held, say) is read by
//! none of the readers of a signature's sheets: the writer writes none of it, the checker judges
//! none of it, and the reading pane's reader takes none of it. They agree, so nothing of such a
//! sheet goes out unjudged, and what the preview shows is what goes out. (Unlike a picture, which
//! the writer's own reading of the elements writes after `</svg>`: a sheet is taken by every reader
//! the one same way.)
//!
//! The documents are invented; nothing here is fetched.

use mailcraft_html::compat::{Severity, check};
use mailcraft_html::{AuthoredScope, authored_fragment_with_rebound_cids, media_lists};

#[test]
fn the_writer_the_checker_and_the_reader_take_the_same_sheets_after_a_template() {
    let scope = AuthoredScope::new("mc-test").expect("a scope class");
    for prefix in ["", "<template></template>", "<svg><template></svg>", "<noscript><template></noscript>"] {
        // Read when no template is left open before it.
        let read = !prefix.contains("</svg>") && !prefix.contains("</noscript>");
        for (sheet, refused) in [("p { color: #1f4e79 }", false), (r#"body[class="card"] p { color: #1f4e79 }"#, true)] {
            let html = format!("{prefix}<style>{sheet}</style><p>Priya Raman</p>");
            let written = authored_fragment_with_rebound_cids(&html, &scope, &mut |cid: &str| Some(cid.to_string()));
            let errors = check(&html).unwrap_or_else(|e| panic!("{html}: {e}")).iter().any(|f| f.severity == Severity::Error);
            assert_eq!(written.is_err(), errors, "{html}: the writer and the checker disagree: {written:?}");
            assert_eq!(written.is_err(), read && refused, "{html}: {written:?}");
            if let Ok(fragment) = &written {
                assert_eq!(fragment.head.contains("1f4e79"), read, "{html}: head {:?}", fragment.head);
            }
        }
        let html = format!("{prefix}<style media=\"screen and (max-width: 600px)\">p {{ color: #1f4e79 }}</style><p>Priya Raman</p>");
        assert_eq!(media_lists(&html).is_empty(), !read, "{html}: the reader takes the sheet as the writer does");
    }
}
