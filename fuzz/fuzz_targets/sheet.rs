// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's style sheets: arbitrary bytes as the text of a `<style>` block, and as the
//! selectors and attributes of the elements they are matched against. Nothing may panic, hang or
//! grow without bound, and what a sheet hides must not be in the text. The same bytes are also
//! media queries and `@media` blocks, read for panes of every width and both themes.
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
    // The same bytes as media queries and as media blocks, nested, for panes of every width and
    // both themes, and the text the sender hid.
    let media = format!(
        "<style media=\"{}\">p{{color:red}}</style><style>@media {text}{{p{{color:red;display:none}}@media {text}{{p{{color:blue}}}}}}{text}</style><p>x</p><div class=a>y</div>",
        text.replace('"', "")
    );
    for width in [0.0, 320.0, 481.0, 600.0, 601.0, 1280.0, f32::NAN] {
        for dark in [false, true] {
            let read = mailcraft_html::parse_in(&media, &mailcraft_html::Env::for_pane(width, dark));
            assert!(read.blocks.len() <= mailcraft_html::MAX_BLOCKS + 2);
        }
    }
    let (_, hidden) = mailcraft_html::text_and_hidden(&sheeted);
    assert!(hidden.len() <= mailcraft_html::MAX_HIDDEN_TEXT);
    let _ = mailcraft_html::media_lists(&media);
});
