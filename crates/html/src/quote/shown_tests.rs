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
    let carried = quotable_with_pictures(html, &mut |_, _| Some("p@example.org".into()), &[Env::default()]);
    [shown(&plain), caps(&quote_to_text(&plain)), shown(&carried), caps(&quote_to_text(&carried))]
}

/// The quote of a message read in a pane of the width and theme `env`, as HTML and as text.
fn quoted_in(html: &str, env: &Env) -> [String; 2] {
    let carried = quotable_with_pictures(html, &mut |_, _| Some("p@example.org".into()), &[*env]);
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
        let carried = quotable_with_pictures(&html, &mut |_, _| None, &[]);
        for q in [shown(&carried), caps(&quote_to_text(&carried))] {
            assert_eq!(q, "AC", "{html}");
            for env in envs() {
                assert!(within(&q, &shown_in(&html, &env)), "{html} {env:?}");
            }
        }
    }
    // Without `@media`, as any pane.
    let carried = quotable_with_pictures("<p>A</p><p hidden>B</p><p>C</p>", &mut |_, _| None, &[]);
    assert_eq!(shown(&carried), "AC");
}

/// Hidden at every real pane width, shown only at widths nobody reads at, with more narrow
/// breakpoints in front than looks are told apart.
fn padded() -> String {
    let pads: String = (1..=20).map(|n| format!("@media (max-width:{}px){{.n{n}{{color:#00f}}}}", n * 10)).collect();
    format!(
        "<style>{pads}@media (min-width:200px) and (max-width:1279px){{.x{{display:none}}}}@media (min-width:1281px){{.x{{display:none}}}}</style>\
         <p>A</p><p class=x>XXX</p><p>C</p>"
    )
}

/// A message with more looks than are told apart, read nowhere, is not quoted at all (what one
/// of the others shows can't be known); read somewhere, as each place read it.
#[test]
fn a_message_with_more_looks_than_are_told_is_quoted_as_read_or_not_at_all() {
    let html = padded();
    assert!(!crate::media_envs(&crate::media_lists(&html)).1, "more looks than are told apart");
    for width in [800.0, 1000.0, 1600.0, 1920.0] {
        assert_eq!(shown_in(&html, &Env::for_pane(width, false)), "AC", "{width}");
    }
    let carried = quotable_with_pictures(&html, &mut |_, _| None, &[]);
    assert_eq!(carried, "");
    let wide = Env::for_pane(1600.0, false);
    let carried = quotable_with_pictures(&html, &mut |_, _| None, &[wide]);
    assert_eq!(shown(&carried), "AC");
    // Two places: only what both showed.
    let html2 = "<style>@media (max-width:999px){.w{display:none}}</style><p>A</p><p class=w>WIDE</p>";
    let carried = quotable_with_pictures(html2, &mut |_, _| None, &[wide, Env::for_pane(800.0, false)]);
    assert_eq!(shown(&carried), "A");
    let carried = quotable_with_pictures(html2, &mut |_, _| None, &[wide, Env::for_pane(1200.0, true)]);
    assert_eq!(shown(&carried), "AWIDE");
}

/// A message whose `@media` hides words at every real width, written so that a second reading of
/// the lists might miss it (comments in the features), read nowhere: the words are not quoted.
#[test]
fn what_every_real_pane_hides_is_not_quoted_however_its_media_is_written() {
    let html = "<style>@media (min-width:/**/200px) and (max-width:/**/1279px){.x{display:none}}@media (min-width:/**/1281px){.x{display:none}}</style>\
                <p>A</p><p class=x>XXX</p><p>C</p>";
    for width in [320.0, 800.0, 1000.0, 1600.0, 1920.0] {
        for dark in [false, true] {
            assert_eq!(shown_in(html, &Env::for_pane(width, dark)), "AC", "{width}");
        }
    }
    let carried = quotable_with_pictures(html, &mut |_, _| None, &[]);
    assert!(!carried.contains("XXX"), "{carried}");
}

/// The markers two looks both show are found in one walk: many of them take no longer than a
/// few, give or take.
#[test]
fn the_markers_every_look_shows_are_found_in_one_walk() {
    let a: Vec<(usize, String)> = (0..20_000).map(|n| (n * 2, format!("{n}."))).collect();
    let b: Vec<(usize, String)> = (0..20_000).map(|n| (n * 3, format!("{}.", n * 3 / 2))).collect();
    let started = std::time::Instant::now();
    let out = crate::blocks::both(a.clone(), &b);
    assert!(started.elapsed() < std::time::Duration::from_millis(500), "{:?}", started.elapsed());
    let slow: Vec<_> = a.into_iter().filter(|x| b.contains(x)).collect();
    assert_eq!(out, slow);
    assert_eq!(out.len(), 6_667);
    assert_eq!(crate::blocks::both(vec![(1, "a".into()), (1, "b".into())], &[(1, "b".into())]), [(1, "b".to_string())]);
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
        ("A<svg></p><template>B</template>C", "AC"),
        ("A<math></br><template>B</template>C", "AC"),
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
/// (one in the message), in the HTML or the plain text; a picture on the web, which the pane shows as its
/// alt text until it is downloaded, keeps it there.
#[test]
fn tooltips_and_the_alt_text_of_drawn_pictures_are_not_quoted() {
    let html =
        "<p title=\"TIP\">A</p><img src=\"cid:logo@example.org\" alt=\"LOGO\" title=\"TIP\"><img src=\"https://example.org/a.png\" alt=\"WEB\">";
    let out = quotable(html);
    assert!(!out.contains("title") && !out.contains("TIP"), "{out}");
    assert!(!out.contains("LOGO"), "{out}");
    assert_eq!(caps(&quote_to_text(&out)), "AWEB");
    let carried = quotable_with_pictures(
        &format!("<img src=\"data:image/png;base64,{}\" alt=\"CHART\">", crate::data_tests::b64(&crate::data_tests::PNG)),
        &mut |_, _| Some("p@example.org".into()),
        &[Env::default()],
    );
    assert!(carried.contains("cid:") && !carried.contains("CHART"), "{carried}");
}

fn png() -> String {
    format!("data:image/png;base64,{}", crate::data_tests::b64(&crate::data_tests::PNG))
}

/// The pictures of `blocks` the pane draws, in order: a web picture by its address, one written
/// into the message (decoded) or attached to it as "in the message".
fn drawn(blocks: &[crate::Block]) -> Vec<String> {
    crate::images(blocks)
        .into_iter()
        .filter_map(|b| match &b.kind {
            crate::BlockKind::Image { src, data, .. } if crate::is_embedded(src) => {
                (!src.starts_with("data:") || data.is_some()).then(|| "in the message".to_string())
            }
            crate::BlockKind::Image { src, .. } => Some(src.clone()),
            _ => None,
        })
        .collect()
}

/// The pictures a quote carries are pictures the pane drew, in order, and its words (a picture's
/// description too) are words the pane showed.
fn assert_pictures_within(html: &str, envs: &[Env]) {
    let views: Vec<Env> = if envs.is_empty() { crate::media_envs(&crate::media_lists(html)).0 } else { envs.to_vec() };
    let carried = quotable_with_pictures(html, &mut |_, _| Some("p@example.org".into()), envs);
    let quoted = drawn(&parse(&carried));
    for env in views {
        let read = parse_in(html, &env).blocks;
        let pane = drawn(&read);
        let mut left = pane.iter();
        assert!(quoted.iter().all(|q| left.any(|p| p == q)), "{html:?} {env:?}: drawn {pane:?}, quoted {quoted:?}");
        let seen = caps(&blocks_to_text(&read));
        for q in [shown(&carried), caps(&quote_to_text(&carried))] {
            assert!(within(&q, &seen), "{html:?} {env:?}: shown {seen:?}, quoted {q:?}");
        }
    }
}

/// A picture the pane doesn't draw is not carried, nor its description: one a style sheet makes
/// too small to see, one past the pictures written into a message that are decoded (those inside
/// a button count), one past the blocks the pane reads, one some look of the message hides.
#[test]
fn only_the_pictures_the_pane_drew_are_carried() {
    for html in [
        "<style>img.t{width:1px}</style><p>HELLO</p><img class=t src=\"https://img.example/x.png\" alt=\"SECRET\">",
        "<style>img.t{height:0}</style><p>HELLO</p><img class=t src=\"https://img.example/x.png\" alt=\"SECRET\">",
        "<style>img.t{width:2px;height:40px}</style><p>HELLO</p><img class=t src=\"https://img.example/x.png\" alt=\"SECRET\">",
    ] {
        let carried = quotable_with_pictures(html, &mut |_, _| Some("p@example.org".into()), &[Env::default()]);
        assert!(!carried.contains("img.example") && !carried.contains("SECRET"), "{carried}");
        assert_eq!(caps(&quote_to_text(&carried)), "HELLO");
        assert_pictures_within(html, &[Env::default()]);
    }
    // 32 decoded in buttons; the 33rd is past what is decoded, and drawn without its picture.
    let buttons: String = (0..crate::MAX_DATA_PICTURES).map(|_| format!("<button><img src=\"{}\"></button>", png())).collect();
    let html = format!("{buttons}<p>HELLO</p><img src=\"{}\" alt=\"ZED\">", png());
    let mut handed = 0;
    let carried = quotable_with_pictures(
        &html,
        &mut |_, _| {
            handed += 1;
            Some("p@example.org".into())
        },
        &[Env::default()],
    );
    assert_eq!(handed, 0, "{carried}");
    assert!(!carried.contains("cid:") && !carried.contains("ZED"), "{carried}");
    // Past the blocks the pane reads.
    let html = format!("{}B<img src=\"https://img.example/x.png\" alt=\"SECRET\">", "<p>A</p>".repeat(crate::MAX_BLOCKS - 1));
    let carried = quotable_with_pictures(&html, &mut |_, _| None, &[Env::default()]);
    assert!(!carried.contains("img.example") && !carried.contains("SECRET"));
    // Shown in some looks only: not in a quote of every look, whichever order they are in.
    let html = "<style>@media (max-width:600px){img.n{display:none}} @media (prefers-color-scheme:dark){img.d{display:none}}</style>\
                <p>HELLO</p><img class=n src=\"https://img.example/n.png\" alt=NARROW><img class=d src=\"https://img.example/d.png\" alt=DARK>";
    let narrow_dark = Env::for_pane(400.0, true);
    for envs in [&[][..], &[narrow_dark, Env::default()][..], &[Env::default(), narrow_dark][..]] {
        let carried = quotable_with_pictures(html, &mut |_, _| None, envs);
        assert!(!carried.contains("img.example") && !carried.contains("NARROW") && !carried.contains("DARK"), "{envs:?}: {carried}");
        assert_pictures_within(html, envs);
    }
    // Drawn: carried, and the description of one in the message is not (it isn't shown).
    let html = format!("<p>HELLO</p><img src=\"{}\" alt=\"ZED\"><img src=\"https://img.example/w.png\" alt=\"WEB\">", png());
    let carried = quotable_with_pictures(&html, &mut |_, _| Some("p@example.org".into()), &[Env::default()]);
    assert!(carried.contains("cid:p@example.org") && !carried.contains("ZED") && carried.contains("alt=\"WEB\""), "{carried}");
    // No tooltips of any kind.
    let carried = quotable("<table summary=SUMMARY><tr><td abbr=ABBR>A</td></tr></table>");
    assert!(!carried.contains("SUMMARY") && !carried.contains("ABBR"), "{carried}");
}

/// Text of a size the reading pane shows (a percentage under a large parent) is quoted.
#[test]
fn text_the_pane_shows_at_its_largest_size_is_quoted() {
    let html = "<p style=\"font-size:44px\">BIG <span style=\"font-size:5%\">SMALL</span></p>";
    assert_eq!(shown(html), caps(&blocks_to_text(&parse(html))));
    assert_eq!(quoted(html)[0], shown(html), "{:?}", quoted(html));
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

    /// A reading that decodes the pictures written into the message and one that only counts
    /// them agree on what is shown, token by token, and on which pictures are drawn and taken.
    #[test]
    fn decoding_or_counting_the_pictures_the_pane_shows_the_same(
        parts in proptest::collection::vec(
            proptest::prop_oneof![2 => proptest::sample::select(pictures()), 1 => proptest::sample::select(soup())],
            0..40,
        ),
        width in 1.0f32..2400.0,
        dark in proptest::prelude::any::<bool>(),
    ) {
        let html: String = parts.concat();
        let env = Env::for_pane(width, dark);
        let (decoded, counted) = (blocks::seen(&html, &env, true), blocks::seen(&html, &env, false));
        proptest::prop_assert_eq!(&decoded.shown, &counted.shown, "{:?}", html);
        proptest::prop_assert_eq!(decoded.cut, counted.cut, "{:?}", html);
        proptest::prop_assert_eq!(&decoded.after, &counted.after, "{:?}", html);
        let marks = |s: &blocks::Seen| s.drawn.iter().map(|(at, d)| (*at, d.taken)).collect::<Vec<_>>();
        proptest::prop_assert_eq!(marks(&decoded), marks(&counted), "{:?}", html);
        proptest::prop_assert!(counted.drawn.iter().all(|(_, d)| d.picture.is_none()));
    }

    /// With pictures in it, on the web or written into the message, sized by a style sheet, in
    /// buttons: the quote carries only pictures the pane drew, and no words it didn't show.
    #[test]
    fn a_quote_carries_only_pictures_the_pane_drew(
        parts in proptest::collection::vec(
            proptest::prop_oneof![2 => proptest::sample::select(pictures()), 1 => proptest::sample::select(soup())],
            0..40,
        ),
        width in 1.0f32..2400.0,
        dark in proptest::prelude::any::<bool>(),
        read in 0usize..3,
    ) {
        let html: String = parts.concat();
        let envs = match read {
            0 => vec![],
            1 => vec![Env::for_pane(width, dark)],
            _ => vec![Env::for_pane(width, dark), Env::default()],
        };
        assert_pictures_within(&html, &envs);
    }

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

/// Pictures for [`soup`]: on the web and written into the message, each with a description, a
/// style sheet that sizes some to nothing, buttons around them, words.
fn pictures() -> Vec<String> {
    let mut parts = Vec::new();
    parts.extend([
        "A".to_string(),
        "<p>".to_string(),
        "<style>@media (max-width:800px){img.n{display:none}}</style>".to_string(),
        "<img src=\"https://p.example/e.png\" alt=\"E\">".to_string(),
        "<img class=t src=\"https://p.example/f.png\" alt=\"F\">".to_string(),
        "<img class=z src=\"https://p.example/g.png\" alt=\"G\">".to_string(),
        "<img class=n src=\"https://p.example/n.png\" alt=\"N\">".to_string(),
        format!("<img src=\"{}\" alt=\"H\">", png()),
        format!("<img class=t src=\"{}\" alt=\"J\">", png()),
        "<style>img.t{width:1px}</style>".to_string(),
        "<style>img.z{height:0}</style>".to_string(),
        "<button>".to_string(),
        "</button>".to_string(),
        "<img src=\"data:image/png;base64,AAAA\" alt=\"K\">".to_string(),
        "<div style=\"background-image:url(data:image/png;base64,AAAA)\">".to_string(),
        format!("<div style=\"background-image:url({})\">", png()),
        "</div>".to_string(),
    ]);
    parts
}
