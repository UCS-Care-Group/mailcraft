// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::write::file_name_params;
use crate::{Attachment, Body, Message, WriteOptions, parse, write};
use std::sync::Arc;

/// The ASCII `filename="…"` stand-in of the parameters.
fn stand_in(params: &str) -> &str {
    let rest = params.strip_prefix("; filename=\"").unwrap_or("");
    rest.split('"').next().unwrap_or("")
}

#[test]
fn stand_in_keeps_the_extension_of_a_long_name() {
    let p = file_name_params(&format!("{}.pdf", "é".repeat(100)));
    let s = stand_in(&p);
    assert!(s.ends_with(".pdf") && s.is_ascii() && s.len() <= 52, "{p}");
    let p = file_name_params(&format!("{}.docx", "Quarterly-report-".repeat(20)));
    assert!(stand_in(&p).ends_with(".docx") && stand_in(&p).len() <= 52, "{p}");
}

#[test]
fn stand_in_of_an_all_non_ascii_name() {
    assert_eq!(stand_in(&file_name_params("日本語.pdf")), "___.pdf");
    assert_eq!(stand_in(&file_name_params("日本語")), "___");
}

#[test]
fn stand_in_with_no_extension_or_many_dots() {
    let p = file_name_params(&"ü".repeat(100));
    assert!(stand_in(&p).len() <= 52 && !stand_in(&p).is_empty(), "{p}");
    let p = file_name_params(&format!("{}.tar.gz", "ü".repeat(100)));
    assert!(stand_in(&p).ends_with(".gz"), "{p}");
    assert_eq!(stand_in(&file_name_params("ü.v1.2.pdf")), "_.v1.2.pdf");
}

#[test]
fn stand_in_with_a_non_ascii_or_huge_extension_stays_short_and_ascii() {
    let p = file_name_params("rapport.dokümént");
    assert!(stand_in(&p).is_ascii() && stand_in(&p).starts_with("rapport."), "{p}");
    let p = file_name_params(&format!("ü.{}", "x".repeat(500)));
    assert!(stand_in(&p).len() <= 52 && stand_in(&p).is_ascii(), "{p}");
}

#[test]
fn encoded_name_is_whole_and_ascii() {
    let name = format!("{}.pdf", "日本語".repeat(40));
    let p = file_name_params(&name);
    assert!(p.is_ascii() && p.contains("filename*0*=utf-8''"), "{p}");
}

#[test]
fn written_attachment_survives_hostile_names() {
    for name in ["rés\r\numé\0.pdf", "\r\n\0", &format!("{}.docx", "ß".repeat(300))] {
        let mut m = Message { body: Body { text: "hi".into(), html: None }, ..Message::default() };
        m.attachments = vec![Attachment {
            name: name.into(),
            mime: "application/pdf".into(),
            content_id: String::new(),
            inline: false,
            data: Arc::new(vec![1, 2, 3]),
        }];
        let out = write(&m, &WriteOptions::default());
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("\0") && !text.lines().any(|l| l.starts_with("\0")));
        assert!(!text.contains("\r\numé"), "header injection");
        let back = parse(&out);
        let clean: String = name.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).collect();
        if !clean.is_empty() {
            assert_eq!(back.attachments.first().map(|a| a.name.as_str()), Some(clean.as_str()));
        }
    }
}
