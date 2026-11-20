// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;

use mailcraft_model::Attachment;

use super::compose::match_attachment;

#[test]
fn match_attachment_resolves_bracketed_percent_and_filename() {
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15c4\0\0\0\nIDATx\x9cc\0\x01\0\0\x05\0\x01\r\n-\xb4\0\0\0\0IEND\xaeB`\x82";
    let a1 = Attachment {
        name: "logo.png".into(),
        mime: "image/png".into(),
        content_id: "<logo123@example.com>".into(),
        inline: true,
        data: Arc::new(png.to_vec()),
        kept: None,
    };
    let a2 = Attachment {
        name: "photo.jpg".into(),
        mime: "image/jpeg".into(),
        content_id: "photo-unique".into(),
        inline: true,
        data: Arc::new(png.to_vec()),
        kept: None,
    };
    let candidates = vec![&a1, &a2];

    // Match with brackets and scheme
    assert_eq!(match_attachment(&candidates, "logo123@example.com").map(|a| a.name.as_str()), Some("logo.png"));
    assert_eq!(match_attachment(&candidates, "<logo123@example.com>").map(|a| a.name.as_str()), Some("logo.png"));
    assert_eq!(match_attachment(&candidates, "cid:logo123@example.com").map(|a| a.name.as_str()), Some("logo.png"));

    // Match exact or by filename fallback
    assert_eq!(match_attachment(&candidates, "photo-unique").map(|a| a.name.as_str()), Some("photo.jpg"));
    assert_eq!(match_attachment(&candidates, "photo.jpg").map(|a| a.name.as_str()), Some("photo.jpg"));
}

