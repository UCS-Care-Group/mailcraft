// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The signature importer takes anything (AGENTS.md › Never crash): arbitrary text and HTML with
//! pictures of every kind of address, with any files beside it, is refused or made a signature,
//! never a panic. What `required_assets` asks for is distinct, bounded and exactly what `import`
//! then takes: given one file for each name, it neither misses one nor finds one unused, and
//! keeps one picture for each. The fuzz target `signature_html` reads the same way.

use mailcraft_mime_model::signature_import::{Asset, ImportError, MAX_ASSET_NAME, import, required_assets};
use mailcraft_model::signature::{MAX_HTML, MAX_PICTURES};
use proptest::prelude::*;

/// A one-pixel PNG, whole and valid: only its kind matters here.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// One piece of a signature: pictures named every way an owner's file may name them, and the
/// markup around them.
fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => prop::sample::select(vec![
            "<img src=\"a.png\">", "<img src=\"dir/b.png\">", "<img src=\"./a.png\">", "<img src=\"dir/../a.png\">", "<img src=\"a%20b.png\">",
            "<img src=\"%zz.png\">", "<img src=\"../x.png\">", "<img src=\"/abs.png\">", "<img src=\"C:\\x.png\">", "<img src=\"a.png?x\">",
            "<img src=\"a.png#x\">", "<img src=\"//host.example/x.png\">", "<img src=\"https://example.org/x.png\">", "<img src=\"HTTP://example.org/y.png\">",
            "<img src=\"cid:x@example.org\">", "<img src=\"data:image/png;base64,AA\">", "<img src=\"file:///x.png\">", "<img src=\" a.png \">",
            "<img src=\"&#x61;.png\">", "<img src=\"\u{e9}.png\">", "<img src=\"\">", "<img>", "<img src=a.png width=1 height=1>",
            "<img style=\"display:none\" src=\"a.png\">", "<img hidden src=\"a.png\">", "<img src=\"a.png\" width=\"2\" height=\"2\">",
        ])
        .prop_map(str::to_string),
        2 => prop::sample::select(vec![
            "<p>", "</p>", "<table><tr><td>", "</td></tr></table>", "<div style=\"display:none\">", "</div>", "<base href=x>",
            "<style>p{color:red}</style>", "<!--", "-->", "<", "\"", "Ana Pereira",
        ])
        .prop_map(str::to_string),
        1 => any::<char>().prop_map(String::from),
    ]
}

/// Arbitrary text, or a document made of [`piece`]s.
fn document() -> impl Strategy<Value = String> {
    prop_oneof![any::<String>(), prop::collection::vec(piece(), 0..24).prop_map(|pieces| pieces.concat())]
}

fn png(name: &str) -> Asset {
    Asset { name: name.to_string(), data: ONE_PIXEL_PNG.to_vec() }
}

proptest! {
    #[test]
    fn the_importer_takes_anything(html in document(), names in prop::collection::vec("[a-z./%]{0,12}", 0..4)) {
        if let Ok(needed) = required_assets(&html) {
            prop_assert!(needed.len() <= MAX_PICTURES);
            for (i, name) in needed.iter().enumerate() {
                prop_assert!(name.len() <= MAX_ASSET_NAME);
                prop_assert!(!needed.get(..i).unwrap_or_default().contains(name), "{name:?} asked for twice");
            }
            let assets: Vec<Asset> = needed.iter().map(|name| png(name)).collect();
            match import("Card", &html, &assets) {
                Ok(signature) => {
                    prop_assert!(signature.html().is_some_and(|h| h.len() <= MAX_HTML));
                    prop_assert_eq!(signature.pictures().len(), assets.len());
                }
                Err(why) => prop_assert!(
                    !matches!(why, ImportError::MissingAsset | ImportError::UnusedAsset | ImportError::DuplicateAsset),
                    "the files asked for were refused as {why:?} for {html:?}"
                ),
            }
        }
        // Any files at all beside it: refused or kept, never a panic.
        let assets: Vec<Asset> = names.iter().map(|name| png(name)).collect();
        let _ = import("Card", &html, &assets);
    }
}
