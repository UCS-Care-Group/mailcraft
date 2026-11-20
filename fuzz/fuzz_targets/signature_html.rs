// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A signature's HTML as its owner may write or import it: the email-compatibility checker, the
//! reader of its pictures' sources, the writer that scopes its style sheets for a message, and
//! the importer with one file for each picture the document asks for. Nothing may panic, and
//! what each answers stays inside the text it read (the property tests in `crates/html/tests/
//! signature_readers.rs` and `crates/mime-model/tests/signature_import_props.rs` hold the same).
#![no_main]

use mailcraft_html::compat::{MAX_FINDINGS, MAX_FIX_BYTES, check};
use mailcraft_html::{AuthoredScope, MAX_IMAGE_SOURCES, authored_fragment_with_rebound_cids, image_sources, unescape};
use mailcraft_mime_model::signature_import::{Asset, import, required_assets};

/// A one-pixel PNG, whole and valid: only its kind matters here.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let html = String::from_utf8_lossy(data);

    if let Ok(found) = check(&html) {
        assert!(found.len() <= MAX_FINDINGS);
        assert!(found.windows(2).all(|w| (w[0].range.start, w[0].range.end) <= (w[1].range.start, w[1].range.end)), "findings out of order");
        for f in &found {
            assert!(html.get(f.range.clone()).is_some(), "{:?} is not a range of the text", f.range);
            assert_eq!(f.severity, f.rule.severity());
            if let Some(fix) = &f.fix {
                assert!(html.get(fix.range.clone()).is_some(), "{:?} is not a range of the text", fix.range);
                assert!(fix.replacement.len() <= MAX_FIX_BYTES, "a fix is longer than a fix may be");
            }
        }
    }

    if let Ok(sources) = image_sources(&html) {
        assert!(sources.len() <= MAX_IMAGE_SOURCES);
        let mut after = 0usize;
        for source in &sources {
            let written = html.get(source.value_range.clone()).expect("a source's range is in the text");
            assert!(source.value_range.start >= after, "sources out of order");
            after = source.value_range.end;
            assert_eq!(unescape(written), source.src);
        }
    }

    let scope = AuthoredScope::new("mc-fuzz").expect("a valid scope");
    if let Ok(fragment) = authored_fragment_with_rebound_cids(&html, &scope, &mut |cid: &str| Some(format!("{cid}.rebound"))) {
        assert!(fragment.body.starts_with("<div class=\"mc-fuzz") && fragment.body.ends_with("</div>"), "the body is not wrapped");
        let head = fragment.head.to_ascii_lowercase();
        assert!(head.is_empty() || (head.starts_with("<style") && head.ends_with("</style>")), "the head is not style elements");
        for refused in ["@import", "url(", "<script", "\\"] {
            assert!(!head.contains(refused), "{refused} written in the head");
        }
    }

    if let Ok(needed) = required_assets(&html) {
        let assets: Vec<Asset> = needed.iter().map(|name| Asset { name: name.clone(), data: ONE_PIXEL_PNG.to_vec() }).collect();
        if let Ok(signature) = import("Fuzz", &html, &assets) {
            assert_eq!(signature.pictures().len(), assets.len(), "a picture asked for was not kept");
        }
    }
});
