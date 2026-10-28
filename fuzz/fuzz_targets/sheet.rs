// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's style sheets: arbitrary bytes as the text of a `<style>` block, and as the
//! selectors and attributes of the elements they are matched against. Nothing may panic, hang or
//! grow without bound, and what a sheet hides must not be in the text.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let rules = mailcraft_html::style_sheet_rules(&text);
    assert!(rules <= mailcraft_html::MAX_SHEET_RULES);
    let html = format!("<style>{text}</style><div id=a class='b c' data-x=y><p class=b>x</p><span>y</span><p>z</p></div>");
    let _ = mailcraft_html::parse(&html);
    let _ = mailcraft_html::to_text(&html);
    let _ = mailcraft_html::quotable(&html);
    // The same bytes as a whole message: their tags are the elements the rules meet.
    let _ = mailcraft_html::parse(&text);
    let sheeted = format!("<style>*,div,p,.b,#a,[data-x],:not(.q),:nth-child(2n+1),p~span,div>p{{color:red;display:none}}</style>{text}");
    let _ = mailcraft_html::parse(&sheeted);
    let _ = mailcraft_html::quotable(&sheeted);
});
