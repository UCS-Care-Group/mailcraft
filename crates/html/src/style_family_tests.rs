// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #600: a `font-family` list is kept whole (capped), for the reading pane to draw the text in the
//! first of its fonts this computer has.

use super::{Css, MAX_FAMILIES, family};
use crate::{Block, BlockKind, parse};

fn font(style: &str) -> Option<String> {
    Css::parse(style).font
}

/// The fonts of the first span of the first block that has text.
fn first_font(html: &str) -> Option<String> {
    fn find(blocks: &[Block]) -> Option<Option<String>> {
        for block in blocks {
            if let Some(span) = block.spans.iter().find(|s| !s.text.trim().is_empty()) {
                return Some(span.font.clone());
            }
            if let BlockKind::Table(table) = &block.kind {
                for cell in table.rows.iter().flat_map(|r| &r.cells) {
                    if let Some(found) = find(&cell.blocks) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
    find(&parse(html)).flatten()
}

#[test]
fn a_font_family_is_its_whole_list() {
    assert_eq!(font("font-family:'Comic Sans MS', cursive").as_deref(), Some("comic sans ms,cursive"));
    assert_eq!(font("font-family:\"Calibri\",sans-serif").as_deref(), Some("calibri,sans-serif"));
    assert_eq!(font("font-family:Georgia, 'Times New Roman', Times, serif").as_deref(), Some("georgia,times new roman,times,serif"));
    // One name alone, as before.
    assert_eq!(font("font-family:Verdana").as_deref(), Some("verdana"));
}

#[test]
fn a_name_that_is_not_a_plain_name_is_left_out_and_the_rest_kept() {
    assert_eq!(font("font-family:url(x);"), None);
    assert_eq!(font(&format!("font-family:{}", "a".repeat(40))), None);
    assert_eq!(font(&format!("font-family:{}, Georgia", "a".repeat(40))).as_deref(), Some("georgia"));
    assert_eq!(font("font-family: url(x), Georgia").as_deref(), Some("georgia"));
    assert_eq!(font("font-family: , ,Georgia,,serif").as_deref(), Some("georgia,serif"));
    // A keyword for every property is no font: the text keeps the fonts of what it is in.
    assert_eq!(font("font-family:inherit"), None);
    assert_eq!(font("font-family:Georgia; font-family:inherit").as_deref(), Some("georgia"));
    assert_eq!(first_font("<div style=\"font-family:Georgia\"><p style=\"font-family:initial\">Hi</p></div>").as_deref(), Some("georgia"));
    // Nothing a name could not hold: no commas, quotes, brackets or slashes get through.
    for junk in ["font-family:\"a(b)\"", "font-family:'a/b'", "font-family:<x>", "font-family:a\\62 c"] {
        assert_eq!(font(junk), None, "{junk}");
    }
}

#[test]
fn a_long_list_is_cut_short() {
    let names: Vec<String> = (0..100).map(|i| format!("face{i}")).collect();
    let list = font(&format!("font-family:{}", names.join(","))).unwrap();
    assert_eq!(list.split(',').count(), MAX_FAMILIES);
    assert!(list.starts_with("face0,face1,"));
}

#[test]
fn a_name_in_another_script_is_kept_in_lower_case() {
    assert_eq!(family("\"メイリオ\", Meiryo").as_deref(), Some("メイリオ,meiryo"));
    assert_eq!(family("Ärger Sans").as_deref(), Some("ärger sans"));
    // Thirty-two characters, not bytes.
    assert!(family(&"é".repeat(32)).is_some());
    assert!(family(&"é".repeat(33)).is_none());
}

#[test]
fn the_font_shorthand_gives_its_families() {
    assert_eq!(font("font: italic bold 12px/1.5 \"Times New Roman\", serif").as_deref(), Some("times new roman,serif"));
    assert_eq!(font("font:14px Arial, Helvetica, sans-serif").as_deref(), Some("arial,helvetica,sans-serif"));
    // A shorthand with no size gives nothing, its family neither.
    assert_eq!(font("font: bold Arial"), None);
    // A later `font-family` wins over the shorthand, and a later shorthand over it.
    assert_eq!(font("font:14px Arial; font-family:Georgia").as_deref(), Some("georgia"));
    assert_eq!(font("font-family:Georgia; font:14px Arial").as_deref(), Some("arial"));
    let css = Css::parse("font: italic bold 7.0pt/1.2 \"Times New Roman\"");
    assert_eq!((css.bold, css.italic, css.font.as_deref()), (Some(true), Some(true), Some("times new roman")));
}

#[test]
fn a_span_carries_the_list_from_a_style_a_sheet_and_a_font_tag() {
    assert_eq!(first_font("<p style=\"font-family:Georgia,serif\">Hello</p>").as_deref(), Some("georgia,serif"));
    assert_eq!(first_font("<p><font face=\"Verdana, Arial\">Hello</font></p>").as_deref(), Some("verdana,arial"));
    let sheet = "<style>.lead { font-family: 'Trebuchet MS', Tahoma, sans-serif; }</style><p class=\"lead\">Hello</p>";
    assert_eq!(first_font(sheet).as_deref(), Some("trebuchet ms,tahoma,sans-serif"));
    // Inherited by what is inside.
    let nested = "<table><tr><td style=\"font-family:Georgia,serif\"><p><b>Hello</b></p></td></tr></table>";
    assert_eq!(first_font(nested).as_deref(), Some("georgia,serif"));
}
