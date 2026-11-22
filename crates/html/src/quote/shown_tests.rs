// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the reading pane doesn't show is never quoted: the quote and the pane agree on where
//! what is left out starts and ends, however the elements that leave it out nest.

use super::*;
use crate::{blocks_to_text, parse, parse_in, quote_to_text};

/// The capital letters of `text`, in order. The words these tests write are capitals; a list's
/// markers, a table's bars and the white space the two readers lay out differently are not.
fn caps(text: &str) -> String {
    text.chars().filter(char::is_ascii_uppercase).collect()
}

fn shown(html: &str) -> String {
    caps(&blocks_to_text(&parse(html)))
}

/// What a pane of the width and theme `env` shows.
fn shown_in(html: &str, env: &Env) -> String {
    caps(&blocks_to_text(&parse_in(html, env).blocks))
}

/// What each form of the quote reads: its HTML as the compose window and a recipient read it,
/// and its plain text, without and with the pictures written into the message carried.
fn quoted(html: &str) -> [String; 4] {
    let plain = quotable(html);
    let carried = quotable_with_pictures(html, &mut |_, _| Some("p@example.org".into()), Some(&Env::default()));
    [shown(&plain), caps(&quote_to_text(&plain)), shown(&carried), caps(&quote_to_text(&carried))]
}

/// The quote of a message read in a pane of the width and theme `env`, as HTML and as text.
fn quoted_in(html: &str, env: &Env) -> [String; 2] {
    let carried = quotable_with_pictures(html, &mut |_, _| Some("p@example.org".into()), Some(env));
    [shown(&carried), caps(&quote_to_text(&carried))]
}

/// Whether `small` is `big` with letters left out.
fn within(small: &str, big: &str) -> bool {
    let mut big = big.chars();
    small.chars().all(|c| big.any(|b| b == c))
}

/// Nothing is quoted that the reading pane doesn't show; and when the pane left out `B` (what is
/// inside) but shows `then` (what comes after), the quote ends what was left out there too.
fn assert_agree(html: &str, then: &str) {
    let seen = shown(html);
    for q in quoted(html) {
        assert!(within(&q, &seen), "{html:?}: shown {seen:?}, quoted {q:?}");
        if !seen.contains('B') && seen.contains(then) {
            assert!(q.contains(then), "{html:?}: shown {seen:?}, quoted {q:?}");
        }
    }
}

/// Nothing is quoted that the reading pane doesn't show.
fn assert_never_more(html: &str) {
    let seen = shown(html);
    for q in quoted(html) {
        assert!(within(&q, &seen), "{html:?}: shown {seen:?}, quoted {q:?}");
    }
}

/// Every way an element is left out with what is inside it, as a start tag and an end tag: every
/// element the quote leaves out whole, and every element it carries, hidden by its `hidden`
/// attribute and by its style, and Outlook's written-out list marker.
fn left_out() -> Vec<(String, String)> {
    let mut kinds: Vec<(String, String)> = DROPPED.iter().map(|n| (format!("<{n}>"), format!("</{n}>"))).collect();
    for n in KEPT {
        kinds.push((format!("<{n} hidden>"), format!("</{n}>")));
        kinds.push((format!("<{n} style=\"display:none\">"), format!("</{n}>")));
    }
    kinds.push(("<span style=\"visibility:hidden\">".into(), "</span>".into()));
    kinds.push(("<span style=\"mso-list:Ignore\">".into(), "</span>".into()));
    kinds
}

/// The advisory's shapes: a `<template>` started inside an element left out whole.
#[test]
fn a_template_inside_what_is_left_out_ends_where_the_pane_ends_it() {
    for (open, close) in [
        ("<svg>", "</svg>"),
        ("<math>", "</math>"),
        ("<noscript>", "</noscript>"),
        ("<p hidden>", "</p>"),
        ("<div style=\"display:none\">", "</div>"),
        ("<select>", "</select>"),
        ("<button>", "</button>"),
        ("<object>", "</object>"),
    ] {
        let html = format!("<html><body><p>HELLO</p>{open}<template>{close}<p>APPROVED</p></body></html>");
        assert_never_more(&html);
        assert!(shown(&html).starts_with("HELLO"), "{html}");
    }
    // Where nothing is left out, all of it is shown and quoted.
    for middle in ["", "<template></template>", "<svg><template></template></svg>"] {
        let html = format!("<p>HELLO</p>{middle}<p>APPROVED</p>");
        assert_eq!(shown(&html), "HELLOAPPROVED", "{html}");
        assert!(quoted(&html).iter().all(|q| q == "HELLOAPPROVED"), "{html}");
    }
    // A template left open hides the rest from both.
    let html = "<p>HELLO</p><template><p>APPROVED</p>";
    assert_eq!(shown(html), "HELLO");
    assert!(quoted(html).iter().all(|q| q == "HELLO"));
}

/// Every kind of what is left out, with every kind inside it: closed, left open, ended across, and
/// a stray `</template>`. The kinds come from the quote's own lists, so one added there is tested.
#[test]
fn every_kind_left_out_inside_every_other_agrees() {
    let kinds = left_out();
    for (open, close) in &kinds {
        assert_agree(&format!("A{open}B</template>{close}C"), "C");
        assert_agree(&format!("A{open}B</template>C"), "C");
        for (inner, inner_close) in &kinds {
            assert_agree(&format!("A{open}{inner}B{inner_close}{close}C"), "C");
            assert_agree(&format!("A{open}{inner}B{close}C"), "C");
            assert_agree(&format!("A{open}{inner}B{close}C{inner_close}D"), "C");
            assert_agree(&format!("A{open}{inner}<template>B{close}C"), "C");
        }
    }
}

/// The same in the places whose end tags end more than themselves: a table's row and a list.
#[test]
fn what_is_left_out_in_tables_and_lists_agrees() {
    for (open, close) in left_out() {
        for html in [
            format!("<table><tr><td>A</td>{open}<td>B</td><td>C{close}<td>D</td></tr><tr><td>E</td></tr></table>F"),
            format!("<table><tr><td>A{open}B</tr><tr><td>C</table>D"),
            format!("<table><tr><td hidden>A{open}<td>B</table>C"),
            format!("<ul><li>A{open}<li>B</ul>C<ol><li>D{open}<ol><li>E</ol>F</ol>G"),
            format!("<p>A{open}<p>B<div>C</div>{close}D"),
            format!("<td hidden><table><tr><td>A{open}B</td></tr></table></td>C"),
            format!("<li hidden>A<ul><li>B{open}C</li></ul></li>D"),
        ] {
            assert_never_more(&html);
        }
    }
}

/// Deeper than either reader follows, what is left out stays out of both.
#[test]
fn what_is_left_out_past_the_depth_caps_agrees() {
    for (open, close) in left_out() {
        for depth in [MAX_DEPTH - 1, MAX_DEPTH, MAX_DEPTH + 1, 300] {
            let deep = "<div>".repeat(depth);
            assert_never_more(&format!("{deep}A{open}B{close}C"));
            assert_never_more(&format!("A{}B{}C", open.repeat(depth), close.repeat(depth)));
            assert_never_more(&format!("A{}<template>B{}C", open.repeat(depth), close.repeat(depth)));
        }
    }
}

/// What a style sheet hides, or what a `<body>` that comes inside what is left out starts.
#[test]
fn a_style_sheet_and_a_body_inside_what_is_left_out_agree() {
    for html in [
        "<style>.x{display:none}</style>A<div class=x>B<template></div>C",
        "<style>.x{font-size:0}</style>A<span class=x>B</span>C",
        "A<template><body>B</template>C",
        "A<div hidden><body>B</div>C",
        "A<span style=\"mso-list:Ignore\">LONG LONG LONG</span>B",
    ] {
        assert_never_more(html);
    }
}

/// Outlook's written-out list marker is quoted as the pane shows it: a number, a label or a
/// bullet's one character, and only with its item's text; any other is left out.
#[test]
fn a_written_out_marker_is_quoted_only_as_it_is_shown() {
    let item = |marker: &str, text: &str| {
        format!("<p class=MsoListParagraph style=\"mso-list:l0 level1 lfo1\"><span style=\"mso-list:Ignore\">{marker}</span>{text}</p>")
    };
    for marker in ["1.", "IV.", "B)", "&middot;", "<span style=\"font:7pt\">o</span>&nbsp;&nbsp;"] {
        let html = item(marker, "ITEM");
        assert_never_more(&html);
        let out = quotable(&html);
        assert!(out.contains("mso-list:Ignore") && out.contains("ITEM"), "{out}");
        assert_eq!(quotable(&out), out);
    }
    assert!(quotable(&item("1.", "ITEM")).contains(">1.\u{a0}</span>"));
    // A marker the pane makes again (a bullet of its own words, one cut short) is quoted as the
    // pane shows it, so the recipient sees the list as the owner did.
    for (marker, shown) in [("PAY ACCOUNT NOW.", "\u{2022}"), (&format!("{}SECRET", "&#0000000000032;".repeat(17)), "\u{2022}"), ("12.", "12.")] {
        let html = item(marker, "ITEM");
        assert_never_more(&html);
        let out = quotable(&html);
        assert!(out.contains(&format!("<span style=\"mso-list:Ignore\">{shown}\u{a0}</span>ITEM")), "{out}");
        assert_eq!(quotable(&out), out);
        assert_eq!(crate::to_text(&out), crate::to_text(&html));
    }
    for html in [item("AB.", ""), format!("{}<p>C</p>", item("AB.", "")), format!("<span style=\"mso-list:Ignore\">AB.</span>{}", item("", "ITEM"))] {
        assert_never_more(&html);
    }
}

/// The widths a pane is read at around the ones messages test, in both themes.
fn envs() -> Vec<Env> {
    [400.0, 600.0, 601.0, 800.0, 1279.0, 1280.0, 1920.0].into_iter().flat_map(|w| [Env::for_pane(w, false), Env::for_pane(w, true)]).collect()
}

/// What a message's `@media` hides in the pane it was read in is not quoted: the quote reads
/// it at that pane's width and in its theme.
#[test]
fn what_media_hid_in_the_pane_read_in_is_not_quoted() {
    for rule in [
        "@media (max-width:1279px){.x{display:none}}",
        "@media (min-width:1280px){.x{display:none}}",
        "@media (max-width:600px){.x{display:none}}",
        "@media screen and (min-width:601px){.x{visibility:hidden}}",
        "@media (prefers-color-scheme:dark){.x{display:none}}",
        "@media (prefers-color-scheme:light){.x{display:none}}",
        "@media (prefers-color-scheme:dark) and (max-width:800px){.x{display:none}}",
    ] {
        let html = format!("<style>{rule}</style><p>A</p><p class=x>BBB</p><template class=x></template><p>C</p>");
        for env in envs() {
            let seen = shown_in(&html, &env);
            for q in quoted_in(&html, &env) {
                assert!(within(&q, &seen), "{html:?} {env:?}: shown {seen:?}, quoted {q:?}");
                assert_eq!(q.contains("BBB"), seen.contains("BBB"), "{html:?} {env:?}: shown {seen:?}, quoted {q:?}");
            }
        }
    }
}

/// A message no pane has said how it read is quoted as every look its `@media` gives it shows:
/// nothing any pane would hide.
#[test]
fn a_message_read_nowhere_is_quoted_as_every_pane_shows_it() {
    for rule in [
        "@media (max-width:1279px){.x{display:none}}",
        "@media (min-width:1280px){.x{display:none}}",
        "@media (prefers-color-scheme:dark){.x{display:none}}",
        "@media (prefers-color-scheme:dark) and (max-width:800px){.x{display:none}}",
    ] {
        let html = format!("<style>{rule}</style><p>A</p><p class=x>BBB</p><p>C</p>");
        let carried = quotable_with_pictures(&html, &mut |_, _| None, None);
        for q in [shown(&carried), caps(&quote_to_text(&carried))] {
            assert_eq!(q, "AC", "{html}");
            for env in envs() {
                assert!(within(&q, &shown_in(&html, &env)), "{html} {env:?}");
            }
        }
    }
    // Without `@media`, as any pane.
    let carried = quotable_with_pictures("<p>A</p><p hidden>B</p><p>C</p>", &mut |_, _| None, None);
    assert_eq!(shown(&carried), "AC");
}

/// `<svg/>` and `<math/>` are written closed; a `/` at the end of an unquoted value is the
/// value's (`<svg x=b/>` is open).
#[test]
fn only_a_tag_written_closed_is_self_closing() {
    use crate::tokenizer::{Token, Tokenizer};
    for (tag, closed) in [
        ("<svg/>", true),
        ("<svg data-x=b />", true),
        ("<svg data-x=\"b\"/>", true),
        ("<svg data-x='b'/>", true),
        ("<svg hidden/>", true),
        ("<svg data-x=b/>", false),
        ("<svg data-x=b>", false),
        ("<svg>", false),
        ("<math/>", true),
        ("<math data-x=b/>", false),
    ] {
        let Some(Token::Open(t)) = Tokenizer::new(tag).next() else { panic!("{tag}") };
        assert_eq!(t.self_closing, closed, "{tag}");
    }
    // An open SVG keeps a template in it from being one; a closed one doesn't.
    assert_eq!(shown("A<svg data-x=b/>B<template>C</template></svg>D<template>E</template>F"), "ABCDF");
    assert_eq!(shown("A<svg data-x=b />B<template>C</template>D"), "ABD");
}

/// `</svg>` ends the innermost SVG element and what is open in it, `</math>` the MathML one, and
/// an HTML element with no place in them ends them both, as in a browser; the quote ends them
/// where the pane does.
#[test]
fn svg_and_math_end_where_a_browser_ends_them() {
    for (html, both) in [
        ("A<svg><math></svg><template>B</template>C", "AC"),
        ("A<math><svg></math><template>B</template>C", "AC"),
        ("A<svg><p>X<template>B</template>C", "AXC"),
        ("A<math><div>X</div><template>B</template>C", "AXC"),
        ("A<svg><font color=red>X<template>B</template>C", "AXC"),
        ("A<svg><svg><span>X</span><template>B</template>C", "AXC"),
    ] {
        assert_eq!(shown(html), both, "{html}");
        assert!(quoted(html).iter().all(|q| q == both), "{html}: {:?}", quoted(html));
    }
    // An end tag of the other kind ends nothing: what follows is still in the MathML.
    for html in ["A<math></svg><template>B</template>C", "A<svg><font>X<template>B</template>C"] {
        assert_never_more(html);
    }
}

/// A `<template>` inside SVG or MathML is no template, and one inside what is hidden ends with
/// it: what comes after is shown, and quoted.
#[test]
fn a_template_in_svg_or_in_what_is_hidden_hides_nothing_after_it() {
    for middle in ["<svg><template></svg>", "<math><template></math>", "<p hidden><template></p>", "<div style=\"display:none\"><template></div>"] {
        let html = format!("<p>HELLO</p>{middle}<p>APPROVED</p>");
        assert_eq!(shown(&html), "HELLOAPPROVED", "{html}");
        assert!(quoted(&html).iter().all(|q| q == "HELLOAPPROVED"), "{html}");
    }
    // An SVG or MathML element ends with what holds it, or at once when written `<svg/>`: a
    // template after it hides what is in it again, in the pane and the quote alike.
    for before in ["<svg/>", "<math />", "<p hidden><svg></p>", "<div><svg></div>", "<span><math></span>", "<table><tr><td><svg></td></tr></table>"] {
        let html = format!("A{before}<template>B</template>C");
        assert_eq!(shown(&html), "AC", "{html}");
        assert!(quoted(&html).iter().all(|q| q == "AC"), "{html}");
    }
    // A template outside them still hides what is in it, and a stray end changes nothing.
    assert_eq!(shown("A<template>B</template>C</template>D"), "ACD");
    assert_eq!(shown("A<svg></svg><template>B</template>C"), "AC");
}

/// A picture's tooltip (`title`) is not carried, nor the alt text of a picture the pane draws
/// (one in the message) in the plain text; a picture on the web, which the pane shows as its
/// alt text until it is downloaded, keeps it there.
#[test]
fn tooltips_and_the_alt_text_of_drawn_pictures_are_not_quoted() {
    let html =
        "<p title=\"TIP\">A</p><img src=\"cid:logo@example.org\" alt=\"LOGO\" title=\"TIP\"><img src=\"https://example.org/a.png\" alt=\"WEB\">";
    let out = quotable(html);
    assert!(!out.contains("title") && !out.contains("TIP"), "{out}");
    assert!(out.contains("alt=\"LOGO\""), "{out}");
    assert_eq!(caps(&quote_to_text(&out)), "AWEB");
    let carried = quotable_with_pictures(
        &format!("<img src=\"data:image/png;base64,{}\" alt=\"CHART\">", crate::data_tests::b64(&crate::data_tests::PNG)),
        &mut |_, _| Some("p@example.org".into()),
        Some(&Env::default()),
    );
    assert!(carried.contains("CHART") && !quote_to_text(&carried).contains("CHART"), "{carried}");
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

    /// Whatever the pane's width and theme, the quote of a message read there says nothing the
    /// pane doesn't show.
    #[test]
    fn at_any_width_and_theme_the_quote_says_nothing_the_pane_hides(
        parts in proptest::collection::vec(proptest::sample::select(soup()), 0..32),
        width in 1.0f32..2400.0,
        dark in proptest::prelude::any::<bool>(),
    ) {
        let html: String = parts.concat();
        let env = Env::for_pane(width, dark);
        let seen = shown_in(&html, &env);
        for q in quoted_in(&html, &env) {
            proptest::prop_assert!(within(&q, &seen), "{:?} {:?}: shown {:?}, quoted {:?}", html, env, seen, q);
        }
    }

    /// Whatever the HTML, the quote and its plain text say nothing the reading pane doesn't show.
    #[test]
    fn tag_soup_quotes_nothing_the_pane_hides(parts in proptest::collection::vec(proptest::sample::select(soup()), 0..48)) {
        let html: String = parts.concat();
        let seen = shown(&html);
        for q in quoted(&html) {
            proptest::prop_assert!(within(&q, &seen), "{:?}: shown {:?}, quoted {:?}", html, seen, q);
        }
    }

    /// The same for any string made of the characters markup is made of.
    #[test]
    fn any_input_quotes_nothing_the_pane_hides(s in "[<>/ =\"a-z:;ABC]{0,160}") {
        let seen = shown(&s);
        for q in quoted(&s) {
            proptest::prop_assert!(within(&q, &seen), "{:?}: shown {:?}, quoted {:?}", s, seen, q);
        }
    }
}

/// The pieces the tag soup is made of: every kind left out, open and closed, the elements whose
/// ends end others, and words.
fn soup() -> Vec<String> {
    let mut parts = Vec::new();
    for (open, close) in left_out() {
        parts.push(open);
        parts.push(close);
    }
    for tag in ["template", "p", "div", "td", "tr", "table", "li", "ul", "body", "xmp", "textarea", "pre", "span", "b"] {
        parts.push(format!("<{tag}>"));
        parts.push(format!("</{tag}>"));
    }
    parts.extend(
        [
            "A",
            "B",
            "C",
            "D",
            " ",
            "<style>.h{display:none}</style>",
            "<p class=h>",
            "<br>",
            "<style>@media (max-width:800px){.n{display:none}}@media (prefers-color-scheme:dark){.d{display:none}}</style>",
            "<div class=n>",
            "<span class=d>",
            "<p class=MsoListParagraph style=\"mso-list:l0 level1 lfo1\">",
            "<span style=\"mso-list:Ignore\">",
            "1.",
        ]
        .map(String::from),
    );
    parts
}
