// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Quoting a message whose pictures go out under Content-IDs of their own: only a picture the
//! message holds is rebound, and only in the address its `<img>` names it by.

use crate::{quotable, quotable_with_rebound_cids};

/// `html` quoted, the two pictures these tests hold written as `part-<n>@mailcraft.invalid`; any
/// other address is left as it was.
fn bound(html: &str) -> String {
    quotable_with_rebound_cids(html, &mut |cid: &str| match cid {
        "logo@example.org" => Some("part-1@mailcraft.invalid".to_string()),
        "mark@example.org" => Some("part-2@mailcraft.invalid".to_string()),
        _ => None,
    })
}

#[test]
fn only_the_address_a_picture_is_named_by_is_rebound() {
    let html = "<p>logo@example.org</p><a href=\"cid:logo@example.org\">logo@example.org</a>\
                <img src=\"cid:logo@example.org\" alt=\"logo@example.org\" title=\"cid:logo@example.org\">\
                <img src=\"cid:elsewhere@example.org\" alt=\"mark@example.org\">";
    assert_eq!(
        bound(html),
        "<p>logo@example.org</p><a href=\"cid:logo@example.org\">logo@example.org</a>\
         <img src=\"cid:part-1@mailcraft.invalid\" alt=\"logo@example.org\" title=\"cid:logo@example.org\">\
         <img src=\"cid:elsewhere@example.org\" alt=\"mark@example.org\">"
    );
}

#[test]
fn the_scheme_is_read_in_any_letter_case() {
    for html in ["<img src=\"CID:logo@example.org\">", "<img src=\" cid:logo@example.org \">"] {
        assert_eq!(bound(html), "<img src=\"cid:part-1@mailcraft.invalid\">", "{html}");
    }
}

#[test]
fn an_answer_that_is_no_content_id_leaves_the_picture_out() {
    let html = "<p>before</p><img src=\"cid:logo@example.org\" alt=\"Logo\"><p>after</p>";
    for answer in ["", "a b", "a\"b", "a<b", "a\nb"] {
        let mut map = |_: &str| Some(answer.to_string());
        assert_eq!(quotable_with_rebound_cids(html, &mut map), "<p>before</p><p>after</p>", "{answer:?}");
    }
    // And one longer than an address may be.
    let long = "a".repeat(mailcraft_link::MAX_LINK.saturating_add(1));
    let mut map = |_: &str| Some(long.clone());
    assert_eq!(quotable_with_rebound_cids("<img src=\"cid:logo@example.org\">", &mut map), "");
}

#[test]
fn an_answer_is_written_escaped_in_the_address() {
    let mut map = |_: &str| Some("a&b'c@example.org".to_string());
    let out = quotable_with_rebound_cids("<img src=\"cid:logo@example.org\">", &mut map);
    assert_eq!(out, "<img src=\"cid:a&amp;b&#39;c@example.org\">");
}

#[test]
fn without_an_answer_the_quote_is_as_quotable_wrote_it() {
    let html = "<table><tr><td><img src=\"cid:logo@example.org\" width=\"120\"></td></tr></table>\
                <img src=\"https://example.org/p.png\">";
    // The address it holds stays, once, and the table is not taken apart.
    let as_before = quotable_with_rebound_cids(html, &mut |_| None);
    assert_eq!(as_before, quotable(html));
    assert_eq!((as_before.matches("cid:logo@example.org").count(), as_before.matches("<table").count()), (1, 1));
}

#[test]
fn the_map_is_given_each_picture_the_message_holds_and_nothing_else() {
    let html = "<img src=\"cid:logo@example.org\"><img src=\"https://example.org/p.png\">\
                <img src=\"cid:mark@example.org\">";
    let mut seen: Vec<String> = Vec::new();
    let out = quotable_with_rebound_cids(html, &mut |cid: &str| {
        seen.push(cid.to_string());
        Some(format!("part-{}@mailcraft.invalid", seen.len()))
    });
    assert_eq!(seen, vec!["logo@example.org".to_string(), "mark@example.org".to_string()]);
    assert_eq!(out, "<img src=\"cid:part-1@mailcraft.invalid\"><img src=\"https://example.org/p.png\"><img src=\"cid:part-2@mailcraft.invalid\">");
}
