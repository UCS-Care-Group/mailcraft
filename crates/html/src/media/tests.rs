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
fn the_width_is_the_width_of_the_body_to_the_pixel() {
    assert_eq!(Env::for_pane(600.4, false).width(), 600);
    assert_eq!(Env::for_pane(0.0, false).width(), 0);
    assert_ne!(Env::for_pane(600.0, true), Env::for_pane(600.0, false));
    // Hostile widths.
    for w in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0, 1e30, 0.0] {
        let _ = Env::for_pane(w, true);
    }
    assert_eq!(Env::for_pane(f32::NAN, false), Env::default());
    assert_eq!(Env::for_pane(1e30, false).width(), 16_384);
}

/// The copy shown between the usual breakpoints is the one for the width (#599 review): a width
/// is tested as it is, not as the class it falls in.
#[test]
fn a_query_is_tested_against_the_width_there_is() {
    let copy = |breakpoint: &str, width: f32| {
        let html = format!(
            "<style>.m{{display:none}} @media ({breakpoint}){{.m{{display:block}} .d{{display:none}}}}</style><p class=d>Desktop</p><p class=m>Mobile</p>"
        );
        parse_in(&html, &at(width, false)).blocks.iter().map(|b| b.text()).collect::<Vec<_>>().join("+")
    };
    // `max-width:596px` is Mobile up to 596 and Desktop from 597 (it was Mobile to 599).
    for w in [500.0, 596.0] {
        assert_eq!(copy("max-width:596px", w), "Mobile", "{w}");
    }
    for w in [597.0, 598.0, 599.0, 600.0, 640.0] {
        assert_eq!(copy("max-width:596px", w), "Desktop", "{w}");
    }
    // `max-width:520px`: not Mobile at 521 to 539.
    for w in [521.0, 530.0, 539.0] {
        assert_eq!(copy("max-width:520px", w), "Desktop", "{w}");
    }
    assert_eq!(copy("max-width:520px", 520.0), "Mobile");
    // `min-width:660px` applies at 665 (it was not applied between 641 and 667 at that width).
    assert_eq!(copy("min-width:660px", 665.0), "Mobile");
    assert_eq!(copy("min-width:660px", 659.0), "Desktop");
    assert_eq!(copy("min-width:660px", 660.0), "Mobile");
}

/// A message's look is kept by what its own conditions say: widths that turn none of them give
/// the same look and need no new blocks.
#[test]
fn panes_with_the_same_verdicts_have_the_same_look() {
    let html = "<style>.m{display:none} @media (max-width:596px){.m{display:block}} @media (min-width:660px){p{color:#00f}} @media (max-width:520px){h1{font-size:12px}}</style><h1>T</h1><p class=m>M</p><p>D</p>";
    let lists = crate::media_lists(html);
    assert_eq!(lists, ["(max-width:596px)", "(min-width:660px)", "(max-width:520px)"]);
    let v = |w: f32| crate::media_verdicts(&lists, &at(w, false));
    for w in [521.0, 560.0, 596.0] {
        assert_eq!(v(w), v(521.0), "{w}");
    }
    for w in [597.0, 600.0, 659.0] {
        assert_eq!(v(w), v(597.0), "{w}");
    }
    assert_ne!(v(596.0), v(597.0));
    assert_ne!(v(659.0), v(660.0));
    assert_ne!(v(520.0), v(521.0));
    // Same verdicts, same blocks.
    assert_eq!(parse_in(html, &at(661.0, false)).blocks, parse_in(html, &at(2000.0, false)).blocks);
    assert_eq!(parse_in(html, &at(100.0, false)).blocks, parse_in(html, &at(300.0, false)).blocks);
    assert_ne!(parse_in(html, &at(596.0, false)).blocks, parse_in(html, &at(597.0, false)).blocks);
    // The theme is a verdict of a list that asks for it.
    let dark = "<style>@media (prefers-color-scheme: dark){p{color:#eee}}</style><p>x</p>";
    let lists = crate::media_lists(dark);
    assert_ne!(crate::media_verdicts(&lists, &at(700.0, true)), crate::media_verdicts(&lists, &at(700.0, false)));
}

/// What can change a message's look is found the way the reader finds it, whatever the spacing
/// of the markup (#599 review).
#[test]
fn media_lists_are_found_as_the_reader_finds_them() {
    for html in [
        "<style media = \"(max-width: 600px)\">p{color:red}</style>",
        "<style media\n=\n\"(prefers-color-scheme: dark)\">p{color:red}</style>",
        "<STYLE MEDIA='(max-width:1px)'>p{}</STYLE>",
        "<style media=print>p{}</style>",
        "<style>@MEDIA (max-width:1px){p{color:red}}</style>",
        "<style>@media/*x*/(max-width:1px){p{color:red}}</style>",
        "<style>p{color:red} @media screen{@media (max-width:1px){p{color:red}}}</style>",
    ] {
        assert!(!crate::media_lists(html).is_empty(), "{html}");
    }
    for html in [
        "<style>p{color:red}</style><p>x</p>",
        "<p style='color:red'>x</p>",
        "<style media=''>p{}</style>",
        "<template><style>@media print{p{}}</style></template>",
        "<style>@font-face{font-family:x} @import 'a.css';</style>",
        "<p>@media (max-width:1px)</p>",
    ] {
        assert!(crate::media_lists(html).is_empty(), "{html}");
    }
    // Each once, and capped.
    assert_eq!(crate::media_lists("<style>@media print{} @media print{} @media  print {}</style>"), ["print"]);
    let many: String = (0..2000).map(|i| format!("@media (max-width:{i}px){{}}")).collect();
    assert_eq!(crate::media_lists(&format!("<style>{many}</style>")).len(), MAX_LISTS);
    // And the style that has its `media` in spaces is read by the pane's width.
    let html = "<style media = \"(max-width: 600px)\">p{color:#00f}</style><p>x</p>";
    assert_eq!(parse_in(html, &at(500.0, false)).blocks[0].spans[0].color, Some(Rgb(0, 0, 255)));
    assert_eq!(parse_in(html, &at(700.0, false)).blocks[0].spans[0].color, None);
}

/// A dark design is one that gives a colour or a background to something shown (#599 review).
#[test]
fn the_dark_design_needs_a_dark_rule_that_gives_colours_to_something_shown() {
    let dark = |html: &str| parse_in(html, &at(700.0, true)).dark_design;
    // A logo swap is no design for dark windows.
    assert!(!dark(
        "<style>.d{display:none} @media (prefers-color-scheme: dark){.d{display:block !important} .l{display:none !important}}</style><p class=l>x</p><p class=d>y</p>"
    ));
    // Rules for the dark modes of mail programs that never match.
    assert!(!dark(
        "<style>[data-ogsc] p{color:#fff !important} [data-ogsb] td{background-color:#111 !important} @media (prefers-color-scheme: dark){[data-ogsc] .x{color:#fff}}</style><p>x</p>"
    ));
    // A dark rule for something that is not there, or not shown.
    assert!(!dark("<style>@media (prefers-color-scheme: dark){.nope{color:#fff}}</style><p>x</p>"));
    assert!(!dark("<style>@media (prefers-color-scheme: dark){.h{color:#fff}}</style><p class=h style='display:none'>x</p><p>y</p>"));
    // A colour, a background or a background colour given to what is shown is one.
    for rule in ["color:#eee", "background:#111", "background-color:#111"] {
        assert!(dark(&format!("<style>@media (prefers-color-scheme: dark){{p{{{rule}}}}}</style><p>x</p>")), "{rule}");
    }
    // `<style media>` counts as `@media`.
    assert!(dark("<style media='(prefers-color-scheme: dark)'>p{color:#eee}</style><p>x</p>"));
    assert!(dark("<style media='screen and (prefers-color-scheme: dark)'>p{background-color:#111}</style><p>x</p>"));
    assert!(!dark("<style media='(prefers-color-scheme: dark)'>p{display:none}</style><p>x</p>"));
    // Never in a light window.
    assert!(!parse_in("<style>@media (prefers-color-scheme: dark){p{color:#eee}}</style><p>x</p>", &at(700.0, false)).dark_design);
}

#[test]
fn a_comment_in_a_prelude_is_white_space_and_only_needs_a_type() {
    assert!(holds("/*x*/ screen /* y */ and /**/ (max-width:600px)", 500.0, false));
    assert!(holds("screen/**/and/**/(max-width:600px)", 500.0, false));
    assert!(!holds("screen and (max-width:600px) /* never closed", 700.0, false));
    assert!(holds("screen and (max-width:600px) /* never closed", 500.0, false));
    // `only` and `not` stand before a type.
    assert!(!holds("only (max-width:600px)", 500.0, false));
    assert!(holds("only screen and (max-width:600px)", 500.0, false));
    assert!(holds("not (max-width:600px)", 700.0, false));
    let html = "<style>@media /*x*/ screen and (max-width:600px){p{color:#00f}}</style><p>x</p>";
    assert_eq!(parse_in(html, &at(500.0, false)).blocks[0].spans[0].color, Some(Rgb(0, 0, 255)));
    let html = "<style>@media only (max-width:600px){p{color:#00f}}</style><p>x</p>";
    assert_eq!(parse_in(html, &at(500.0, false)).blocks[0].spans[0].color, None);
}

/// Hidden text is joined as visible text is (#599 review).
#[test]
fn hidden_words_are_joined_as_shown_text_is() {
    let hidden = |html: &str| text_and_hidden(&format!("<div style='display:none'>{html}</div><p>x</p>")).1;
    assert_eq!(hidden("<span>Hel</span><span>lo</span>"), "Hello");
    assert_eq!(hidden("<b>Hel</b>lo <i>wor</i>ld"), "Hello world");
    assert_eq!(hidden("<span>Hello</span> <span>world</span>"), "Hello world");
    assert_eq!(hidden("<span>Hello </span><span>world</span>"), "Hello world");
    assert_eq!(hidden("<p>one</p><p>two</p>"), "one two");
    assert_eq!(hidden("one<br>two"), "one two");
    assert_eq!(hidden("line\nbreak"), "line break");
    assert_eq!(hidden("Pre&zwnj;header&nbsp;&nbsp;text"), "Preheader text");
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

/// The dark design needs a shown element, whatever hides an element around it, and a dark
/// colour that wins (#599 review).
#[test]
fn the_dark_design_needs_the_element_and_its_ancestors_shown_and_the_dark_colour_to_win() {
    let dark = |html: &str| parse_in(html, &at(700.0, true)).dark_design;
    let rule = "<style>@media (prefers-color-scheme: dark){.h{color:#fff}}";
    // A mobile-only block hidden by its own style, or by the sheet, is not a design.
    assert!(!dark(&format!("{rule}</style><div style='display:none'><p class=h>x</p></div><p>y</p>")));
    assert!(!dark(&format!("{rule} .hide{{display:none}}</style><div class=hide><section><p class=h>x</p></section></div><p>y</p>")));
    // The same block shown is.
    assert!(dark(&format!("{rule}</style><div><p class=h>x</p></div><p>y</p>")));
    // A later `!important` colour beats the dark one: the sender's dark colour is not used.
    assert!(!dark("<style>@media (prefers-color-scheme: dark){p{color:#fff}} p{color:#000 !important}</style><p>x</p>"));
    // A dark colour that is `!important` wins over a later plain one; a later plain one wins over a plain dark one.
    assert!(dark("<style>@media (prefers-color-scheme: dark){p{color:#fff !important}} p{color:#000}</style><p>x</p>"));
    assert!(!dark("<style>@media (prefers-color-scheme: dark){p{color:#fff}} p.q{color:#000}</style><p class=q>x</p>"));
    // Another property winning does not matter: the dark background wins.
    assert!(dark("<style>@media (prefers-color-scheme: dark){p{background:#111}} p{color:#000 !important}</style><p>x</p>"));
}

/// What is not a node of the tree (a picture, a rule, a break), what is left out past the caps and
/// what has an inline style of its own do not leave a design behind (#599 review).
#[test]
fn the_dark_design_is_for_the_element_itself_shown_and_winning() {
    let dark = |html: &str| parse_in(html, &at(700.0, true)).dark_design;
    let rule = "<style>@media (prefers-color-scheme: dark){.h{background:#000}}";
    // Elements that are not pushed as nodes, hidden by their own style or by the sheet.
    assert!(!dark(&format!("{rule}</style><img class=h style='display:none' alt=a><p>y</p>")));
    assert!(!dark(&format!("{rule} .n{{display:none}}</style><hr class='h n'><p>y</p>")));
    assert!(!dark(&format!("{rule}</style><br class=h style='display:none'><p>y</p>")));
    assert!(dark(&format!("{rule}</style><hr class=h><p>y</p>")));
    // The `hidden` attribute and a picture of a pixel are left out of the blocks too.
    assert!(!dark(&format!("{rule}</style><img class=h hidden alt=a><p>y</p>")));
    assert!(!dark(&format!("{rule}</style><hr class=h hidden><p>y</p>")));
    assert!(!dark(&format!("{rule}</style><img class=h width=1 height=1 src='https://x.example/p.gif'><p>y</p>")));
    assert!(!dark(&format!("{rule}</style><div hidden><p class=h>x</p></div><p>y</p>")));
    // Nor is what the builder skips: a template, the head, the depth it stops at.
    assert!(!dark(&format!("{rule}</style><template><p class=h>x</p></template><p>y</p>")));
    assert!(!dark(&format!("{rule}</style><template class=h><p>x</p></template><p>y</p>")));
    assert!(!dark(&format!("{rule}</style><head class=h><meta class=h></head><p>y</p>")));
    let deep = "<div>".repeat(200);
    assert!(!dark(&format!("{rule}</style>{deep}<p class=h>x</p>")));
    assert!(dark(&format!("{rule}</style><div><p class=h>x</p></div>")));
    // A wrapper that shows nothing itself but holds pictures, and an empty box, are left as they are.
    assert!(dark(&format!("{rule}</style><div class=h style='font-size:0'><img src='https://x.example/a.png' alt=a></div>")));
    assert!(dark(&format!("{rule}</style><div class=h></div><p>y</p>")));
    // Nothing is carried over from the last element styled when the sheet stops styling.
    let many = "<i></i>".repeat(59_998);
    assert!(!dark(&format!("{rule}</style>{many}<div style='display:none'><b class=h>x</b></div><p>y</p>")));
    // `background` and `background-color` are one: a plain dark background loses to the other's `!important`.
    assert!(!dark("<style>@media (prefers-color-scheme: dark){p{background:#111}} p{background-color:#fff !important}</style><p>x</p>"));
    assert!(dark("<style>@media (prefers-color-scheme: dark){p{background:#111 !important}} p{background-color:#fff}</style><p>x</p>"));
    // The element's own style beats a plain dark declaration, not an `!important` one.
    assert!(!dark("<style>@media (prefers-color-scheme: dark){p{color:#fff}}</style><p style='color:#000'>x</p>"));
    assert!(!dark("<style>@media (prefers-color-scheme: dark){p{background:#111}}</style><p style='background-color:#fff'>x</p>"));
    assert!(dark("<style>@media (prefers-color-scheme: dark){p{color:#fff !important}}</style><p style='color:#000'>x</p>"));
    assert!(dark("<style>@media (prefers-color-scheme: dark){p{color:#fff}}</style><p style='margin:0'>x</p>"));
}

/// A message's sheets are read for the lists the same way whatever their `media` (#599 review):
/// a big sheet that is not for this pane still uses the room all of them share, so the lists
/// found are those of the sheets read, and a look changes exactly when a verdict does.
#[test]
fn the_lists_and_the_cascade_read_the_same_sheets() {
    let big = format!("<style media=\"print\">{}</style>", ".z{color:red}".repeat(6_000));
    assert!(big.len() > crate::sheet::MAX_SHEET);
    let html = format!(
        "{big}<style>.m{{display:none}} @media (max-width:600px){{.m{{display:block}} .d{{display:none}}}}</style><p class=d>Desktop</p><p class=m>Mobile</p>"
    );
    let lists = crate::media_lists(&html);
    let (narrow, wide) = (at(500.0, false), at(900.0, false));
    // Whether the lists tell a narrow pane from a wide one is whether the message looks different in them.
    let differ = crate::media_verdicts(&lists, &narrow) != crate::media_verdicts(&lists, &wide);
    assert_eq!(differ, parse_in(&html, &narrow).blocks != parse_in(&html, &wide).blocks, "{lists:?}");
    // And in the ordinary case (the sheet is read) both say the pane matters.
    let small = html.replace(&big, "<style media=\"print\">.z{color:red}</style>");
    let lists = crate::media_lists(&small);
    assert!(crate::media_verdicts(&lists, &narrow) != crate::media_verdicts(&lists, &wide));
    assert!(parse_in(&small, &narrow).blocks != parse_in(&small, &wide).blocks);
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
    fn any_width_is_a_whole_number_of_pixels(w in proptest::num::f32::ANY, dark in proptest::bool::ANY) {
        let e = Env::for_pane(w, dark);
        proptest::prop_assert_eq!(e.dark(), dark);
        if (0.0..16_384.0).contains(&w) {
            proptest::prop_assert_eq!(e.width(), w as u16);
        }
    }
}
