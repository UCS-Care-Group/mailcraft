// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::{Rgb, parse_in, text_and_hidden, to_text};

fn at(width: f32, dark: bool) -> Env {
    Env::for_pane(width, dark)
}

fn holds(list: &str, width: f32, dark: bool) -> bool {
    test(list, &at(width, dark)).holds
}

#[test]
fn types_and_and_not_only() {
    for list in [
        "",
        "all",
        "screen",
        "ALL",
        " Screen ",
        "only screen",
        "only screen and (min-width: 0)",
        "not print",
        "not tv",
        "screen, print",
        "print, all",
    ] {
        assert!(holds(list, 700.0, false), "{list:?}");
    }
    for list in [
        "print",
        "tv",
        "speech",
        "not screen",
        "not all",
        "print and (min-width: 0)",
        "screen and",
        "and",
        "only",
        "not",
        "screen screen",
        "screen or print",
    ] {
        assert!(!holds(list, 700.0, false), "{list:?}");
    }
}

#[test]
fn widths_in_px_em_pt_and_rem() {
    // 600 is a break: the pane at 600 is in a class of its own, and 601 in the next.
    assert!(holds("(max-width: 600px)", 600.0, false));
    assert!(!holds("(max-width: 600px)", 601.0, false));
    assert!(holds("(min-width: 601px)", 601.0, false));
    assert!(!holds("(min-width: 601px)", 600.0, false));
    assert!(holds("(min-width: 600px)", 600.0, false) && holds("(min-width: 600px)", 900.0, false));
    assert!(holds("(max-width: 37.5em)", 600.0, false) && !holds("(max-width: 37.5em)", 640.0, false));
    assert!(holds("(max-width: 2.5rem)", 40.0, false) && !holds("(max-width: 2.5rem)", 400.0, false));
    assert!(holds("(max-width: 450pt)", 600.0, false) && !holds("(max-width: 450pt)", 601.0, false));
    assert!(holds("(width: 600px)", 600.0, false) && !holds("(width: 600px)", 601.0, false));
    assert!(holds("(max-device-width: 480px)", 400.0, false) && !holds("(max-device-width: 480px)", 481.0, false));
    assert!(holds("(max-width: 0)", 0.0, false));
    // The query's letter case and its spacing do not matter.
    assert!(holds("ONLY SCREEN AND ( MAX-WIDTH : 480PX )".to_ascii_lowercase().as_str(), 400.0, false));
    assert!(holds("screen and(max-width:480px)", 400.0, false));
    assert!(holds("screen   and   (min-width:200px)   and   (max-width:480px)", 400.0, false));
    assert!(!holds("(min-width:200px) and (max-width:480px)", 500.0, false));
}

#[test]
fn the_colour_scheme_is_the_windows() {
    assert!(holds("(prefers-color-scheme: dark)", 700.0, true) && !holds("(prefers-color-scheme: dark)", 700.0, false));
    assert!(holds("(prefers-color-scheme: light)", 700.0, false) && !holds("(prefers-color-scheme: light)", 700.0, true));
    assert!(test("screen and (prefers-color-scheme: dark)", &at(700.0, true)).dark);
    assert!(!test("(prefers-color-scheme: light)", &at(700.0, false)).dark);
    // `not` of the dark design is not a dark design.
    assert!(!test("not (prefers-color-scheme: dark)", &at(700.0, false)).dark);
    assert!(holds("not (prefers-color-scheme: dark)", 700.0, false));
    // Only one of a list has to hold.
    assert!(holds("print, (prefers-color-scheme: dark)", 700.0, true));
}

#[test]
fn what_is_not_understood_is_false_and_not_does_not_make_it_true() {
    for list in [
        "(orientation: portrait)",
        "(min-resolution: 2dppx)",
        "(-webkit-min-device-pixel-ratio: 2)",
        "(hover: hover)",
        "(prefers-reduced-motion: reduce)",
        "(min-width)",
        "(min-width: wide)",
        "(min-width: 10furlongs)",
        "(min-width: 600)",
        "(width >= 600px)",
        "(400px <= width <= 700px)",
        "(min-width: 600px) or (max-width: 100px)",
        "(min-width: 600px",
        "min-width: 600px",
        "screen and ((min-width: 600px))",
        "not (orientation: portrait)",
        "not screen and (orientation: portrait)",
        "(prefers-color-scheme: sepia)",
        "!!!",
        "screen and (min-width: 1px) and",
        "(((((",
        ")))))",
    ] {
        assert!(!holds(list, 700.0, false), "{list:?}");
    }
}

#[test]
fn the_list_and_the_query_are_capped() {
    // Nine queries: the ninth is not looked at.
    let ninth = format!("{}screen", "print,".repeat(MAX_QUERIES));
    assert!(!holds(&ninth, 700.0, false));
    let eighth = format!("{}screen", "print,".repeat(MAX_QUERIES - 1));
    assert!(holds(&eighth, 700.0, false));
    // Nine features in one query: false, however true they are.
    let eight = ["(min-width: 0)"; MAX_FEATURES].join(" and ");
    assert!(holds(&eight, 700.0, false));
    let nine = ["(min-width: 0)"; MAX_FEATURES + 1].join(" and ");
    assert!(!holds(&nine, 700.0, false));
    // A list over the byte cap is false.
    let long = format!("screen,{}", "print,".repeat(MAX_QUERY_BYTES / 6 + 1));
    assert!(!holds(&long, 700.0, false));
    let huge = format!("{}screen", "print,".repeat(100_000));
    assert!(!holds(&huge, 700.0, false));
}

#[test]
fn the_classes_are_few_and_decide_the_common_breakpoints() {
    let classes: std::collections::HashSet<u16> = (0..=4000).map(|w| Env::for_pane(w as f32, false).width()).collect();
    assert!(classes.len() <= 45, "{} classes", classes.len());
    assert!(classes.len() >= 20);
    // Every width of a class has its least width as the class, which never exceeds it.
    for w in 0..=2000u16 {
        let e = Env::for_pane(f32::from(w), false);
        assert!(e.width() <= w || w >= 1280);
    }
    // A pane's class never moves for a fraction of a point, and wider is never narrower.
    assert_eq!(Env::for_pane(600.4, false), Env::for_pane(600.0, false));
    let mut last = 0;
    for w in 0..3000 {
        let now = Env::for_pane(w as f32, false).width();
        assert!(now >= last);
        last = now;
    }
    // The theme is part of the class.
    assert_ne!(Env::for_pane(600.0, true), Env::for_pane(600.0, false));
    // Hostile widths.
    for w in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0, 1e30, 0.0] {
        let _ = Env::for_pane(w, true);
    }
    assert_eq!(Env::for_pane(f32::NAN, false), Env::default());
}

#[test]
fn media_blocks_in_a_sheet_are_read_for_the_pane() {
    let html = "<style>p{color:#f00} @media (max-width:480px){p{color:#00f}} @media (min-width:481px){p{color:#0f0}}</style><p>x</p>";
    let colour = |w: f32| parse_in(html, &at(w, false)).blocks[0].spans[0].color;
    assert_eq!(colour(400.0), Some(Rgb(0, 0, 255)));
    assert_eq!(colour(480.0), Some(Rgb(0, 0, 255)));
    assert_eq!(colour(481.0), Some(Rgb(0, 255, 0)));
    assert_eq!(colour(1000.0), Some(Rgb(0, 255, 0)));
    // The order of the sheet decides between rules of one specificity, in and out of a block.
    let html = "<style>@media screen{p{color:#00f}} p{color:#f00}</style><p>x</p>";
    assert_eq!(parse_in(html, &at(700.0, false)).blocks[0].spans[0].color, Some(Rgb(255, 0, 0)));
}

#[test]
fn a_style_element_with_a_media_attribute_is_read_for_the_pane() {
    let html = "<style media='(max-width:480px)'>p{color:#00f}</style><style media='screen and (min-width: 481px)'>p{color:#0f0}</style><style media=print>p{color:#f0f}</style><p>x</p>";
    let colour = |w: f32| parse_in(html, &at(w, false)).blocks[0].spans[0].color;
    assert_eq!(colour(300.0), Some(Rgb(0, 0, 255)));
    assert_eq!(colour(700.0), Some(Rgb(0, 255, 0)));
}

#[test]
fn media_inside_media_is_read_to_a_depth() {
    let nest = |levels: usize| {
        let open = "@media screen{".repeat(levels);
        let close = "}".repeat(levels);
        format!("<style>{open}p{{color:#00f}}{close}</style><p>x</p>")
    };
    let colour = |html: &str| parse_in(html, &Env::default()).blocks[0].spans[0].color;
    for levels in 1..=MAX_NESTING {
        assert_eq!(colour(&nest(levels)), Some(Rgb(0, 0, 255)), "{levels} deep");
    }
    for levels in [MAX_NESTING + 1, MAX_NESTING + 2, 50, 5_000] {
        assert_eq!(colour(&nest(levels)), None, "{levels} deep");
    }
    // Nested conditions are both needed.
    let html = "<style>@media (max-width:600px){@media (min-width:400px){p{color:#00f}}}</style><p>x</p>";
    assert_eq!(parse_in(html, &at(500.0, false)).blocks[0].spans[0].color, Some(Rgb(0, 0, 255)));
    assert_eq!(parse_in(html, &at(300.0, false)).blocks[0].spans[0].color, None);
    assert_eq!(parse_in(html, &at(700.0, false)).blocks[0].spans[0].color, None);
}

#[test]
fn too_many_media_blocks_are_left_out_after_the_cap() {
    let blocks: String = (0..MAX_LISTS + 50).map(|i| format!("@media screen{{.c{i}{{color:#00f}}}}")).collect();
    let html = format!("<style>{blocks}</style><p class=c0>a</p><p class=c{}>b</p><p class=c{}>c</p>", MAX_LISTS - 1, MAX_LISTS + 10);
    let read = parse_in(&html, &Env::default());
    let colours: Vec<_> = read.blocks.iter().map(|b| b.spans[0].color).collect();
    assert_eq!(colours, [Some(Rgb(0, 0, 255)), Some(Rgb(0, 0, 255)), None]);
}

#[test]
fn the_dark_design_is_found_only_in_a_dark_window() {
    let html = "<style>p{color:#222} @media (prefers-color-scheme: dark){p{color:#eee}}</style><p>x</p>";
    let dark = parse_in(html, &at(700.0, true));
    let light = parse_in(html, &at(700.0, false));
    assert!(dark.dark_design && !light.dark_design);
    assert_eq!(dark.blocks[0].spans[0].color, Some(Rgb(0xee, 0xee, 0xee)));
    assert_eq!(light.blocks[0].spans[0].color, Some(Rgb(0x22, 0x22, 0x22)));
    // A block for dark windows that has nothing the whitelist reads is no design.
    let empty = parse_in("<style>@media (prefers-color-scheme: dark){p{behavior:x}}</style><p>x</p>", &at(700.0, true));
    assert!(!empty.dark_design);
    // A block for light windows is not a dark design in a dark window; in a light one neither.
    let lit = parse_in("<style>@media (prefers-color-scheme: light){p{color:#eee}}</style><p>x</p>", &at(700.0, false));
    assert!(!lit.dark_design);
}

#[test]
fn depends_on_pane_finds_media() {
    assert!(crate::depends_on_pane("<style>@media (max-width:1px){p{color:red}}</style>"));
    assert!(crate::depends_on_pane("<style>@MEDIA screen{}</style>"));
    assert!(crate::depends_on_pane("<style media=print>p{}</style>"));
    assert!(!crate::depends_on_pane("<style>p{color:red}</style><p>x</p>"));
    assert!(!crate::depends_on_pane("<p style='color:red'>x</p>"));
}

#[test]
fn what_the_sender_hid_is_kept_apart_from_the_text() {
    let html = "<style>.pre{display:none} .gone{visibility:hidden}</style>\
        <div class=pre>Preheader&nbsp;&zwnj;&nbsp;&zwnj; words</div>\
        <p>Shown text.</p><p class=gone>Hidden by a rule.</p>\
        <p style='font-size:0'>Tiny.</p><div style='display:none'>Hidden <b>inline</b> style</div>\
        <div hidden>Hidden attribute</div><template>Not in a template</template>";
    let (text, hidden) = text_and_hidden(html);
    assert_eq!(text, "Shown text.");
    assert_eq!(text, to_text(html));
    for word in ["Preheader words", "Hidden by a rule.", "Tiny.", "Hidden inline style", "Hidden attribute"] {
        assert!(hidden.contains(word), "{word:?} in {hidden:?}");
    }
    assert!(!hidden.contains("Shown") && !hidden.contains("template") && !hidden.contains('\u{200c}'), "{hidden:?}");
    // The reading blocks never have it.
    let shown = parse_in(html, &Env::default()).blocks.iter().map(|b| b.text()).collect::<String>();
    assert_eq!(shown, "Shown text.");
}

#[test]
fn hidden_text_is_capped() {
    let html = format!("<div style='display:none'>{}</div><p>x</p>", "word ".repeat(100_000));
    let (_, hidden) = text_and_hidden(&html);
    assert!(hidden.len() <= crate::MAX_HIDDEN_TEXT, "{}", hidden.len());
    assert!(hidden.len() > crate::MAX_HIDDEN_TEXT - 16);
    let (_, none) = text_and_hidden("<p>x</p>");
    assert_eq!(none, "");
}

#[test]
fn a_message_that_shows_nothing_still_has_its_hidden_text() {
    let (text, hidden) = text_and_hidden("<div style='display:none'>Only for search</div>");
    assert_eq!((text.as_str(), hidden.as_str()), ("", "Only for search"));
}

proptest::proptest! {
    #[test]
    fn any_bytes_are_a_media_list(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..700)) {
        let text = String::from_utf8_lossy(&bytes);
        for dark in [false, true] {
            let _ = test(&text, &at(480.0, dark));
        }
        let html = format!("<style>@media {text}{{p{{color:red}}}}</style><style media=\"{}\">p{{color:blue}}</style><p>x</p>", text.replace('"', ""));
        let _ = parse_in(&html, &at(600.0, true));
    }

    #[test]
    fn any_query_like_text_is_a_media_list(list in "[a-z(): ,0-9.\\-]{0,300}") {
        let verdict = test(&list, &at(500.0, true));
        // A list that holds is one of the queries, which hold alone.
        if verdict.holds && !list.contains(',') {
            proptest::prop_assert!(test(&list, &at(500.0, true)).holds);
        }
        let html = format!("<style>@media {list}{{p{{color:red}}}}</style><p>x</p>");
        let _ = parse_in(&html, &at(500.0, false));
    }

    #[test]
    fn any_width_is_a_class_that_never_exceeds_it(w in proptest::num::f32::ANY, dark in proptest::bool::ANY) {
        let e = Env::for_pane(w, dark);
        proptest::prop_assert!(e.width() <= 1280);
        proptest::prop_assert_eq!(e.dark(), dark);
        if (0.0..1280.0).contains(&w) {
            proptest::prop_assert!(f32::from(e.width()) <= w);
        }
    }
}
