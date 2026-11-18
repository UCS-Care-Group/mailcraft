// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #601: pictures behind a box (`background="…"`, `background-image` and the rest).

use crate::bg::{BgCss, url_in};
use crate::data_tests::{PNG, b64};
use crate::style::Css;
use crate::{
    BgOffset, BgPicture, BgRepeat, BgSize, Block, BlockKind, Length, MAX_BG_OFFSET_PX, MAX_DATA_PICTURES, MAX_INPUT, bg_pictures, parse, parse_text,
    quotable,
};

fn png() -> String {
    format!("data:image/png;base64,{}", b64(&PNG))
}

/// The pictures behind the boxes of `html`, in reading order.
fn backs(html: &str) -> Vec<BgPicture> {
    bg_pictures(&parse(html)).into_iter().cloned().collect()
}

fn srcs(html: &str) -> Vec<String> {
    backs(html).into_iter().map(|b| b.src).collect()
}

fn bg(style: &str) -> BgCss {
    Css::parse(style).bg
}

#[test]
fn the_address_keeps_its_letter_case_and_loses_its_quotes() {
    assert_eq!(url_in("url(https://Example.org/Bg.PNG)").0.as_deref(), Some("https://Example.org/Bg.PNG"));
    assert_eq!(url_in("URL( 'cid:Image001.JPG@01D' ) no-repeat").0.as_deref(), Some("cid:Image001.JPG@01D"));
    assert_eq!(url_in("url(\"a b).png\")").0.as_deref(), Some("a b).png"));
    for none in ["none", "url()", "url('')", "url(https://example.org/never-closed", "linear-gradient(#fff,#000)"] {
        assert_eq!(url_in(none).0, None, "{none}");
    }
    assert_eq!(bg("background-image:url(https://Example.org/A.png)").image, Some(Some("https://Example.org/A.png".into())));
    let written = format!("background-image:url({})", png());
    assert_eq!(bg(&written).image, Some(Some(png())));
}

#[test]
fn the_shorthand_gives_every_part_and_the_longhands_one_each() {
    let all = bg("background:#123456 url(https://example.org/a.png) no-repeat right bottom / cover");
    assert_eq!(all.image, Some(Some("https://example.org/a.png".into())));
    assert_eq!(all.repeat, Some(BgRepeat::None));
    assert_eq!(all.position, Some([BgOffset::Percent(100), BgOffset::Percent(100)]));
    assert_eq!(all.size, Some(BgSize::Cover));
    assert_eq!(Css::parse("background:#123456 url(https://example.org/a.png) no-repeat").background, Some(crate::Rgb(0x12, 0x34, 0x56)));
    // What the shorthand leaves out is the default.
    let bare = bg("background:url(a.png)");
    assert_eq!((bare.repeat, bare.position, bare.size), (Some(BgRepeat::Both), Some([BgOffset::Percent(0); 2]), Some(BgSize::Auto)));
    // A colour after it leaves the picture; a shorthand after a picture without one takes it away.
    assert_eq!(bg("background-image:url(a.png);background-color:#fff").image, Some(Some("a.png".into())));
    assert_eq!(bg("background-image:url(a.png);background:#fff").image, Some(None));
    assert_eq!(bg("background-image:url(a.png);background-image:none").image, Some(None));
    let parts = bg("background-repeat:repeat-x;background-position:center 20px;background-size:50% auto");
    assert_eq!(parts.repeat, Some(BgRepeat::X));
    assert_eq!(parts.position, Some([BgOffset::Percent(50), BgOffset::Px(20)]));
    assert_eq!(parts.size, Some(BgSize::Given { width: Some(Length::Percent(50)), height: None }));
}

#[test]
fn repeating_positions_and_sizes_are_read_within_bounds() {
    let repeat = |v: &str| bg(&format!("background-repeat:{v}")).repeat;
    assert_eq!(repeat("no-repeat"), Some(BgRepeat::None));
    assert_eq!(repeat("repeat-y"), Some(BgRepeat::Y));
    assert_eq!(repeat("repeat no-repeat"), Some(BgRepeat::X));
    assert_eq!(repeat("round"), Some(BgRepeat::Both));
    assert_eq!(repeat("sideways"), None);
    let at = |v: &str| bg(&format!("background-position:{v}")).position;
    assert_eq!(at("top"), Some([BgOffset::Percent(50), BgOffset::Percent(0)]));
    assert_eq!(at("bottom left"), Some([BgOffset::Percent(0), BgOffset::Percent(100)]));
    assert_eq!(at("center"), Some([BgOffset::Percent(50); 2]));
    assert_eq!(at("-40px 250%"), Some([BgOffset::Px(0), BgOffset::Percent(100)]));
    assert_eq!(at("99999px 0"), Some([BgOffset::Px(MAX_BG_OFFSET_PX), BgOffset::Px(0)]));
    assert_eq!(at("right 10px top 5px"), Some([BgOffset::Percent(100), BgOffset::Percent(0)]));
    assert_eq!(at("nowhere"), None);
    let size = |v: &str| bg(&format!("background-size:{v}")).size;
    assert_eq!(size("contain"), Some(BgSize::Contain));
    assert_eq!(size("auto"), Some(BgSize::Auto));
    assert_eq!(size("600px 200px"), Some(BgSize::Given { width: Some(Length::Px(600)), height: Some(Length::Px(200)) }));
    assert_eq!(size("auto 100%"), Some(BgSize::Given { width: None, height: Some(Length::Percent(100)) }));
    assert_eq!(size("0px"), None);
    assert_eq!(size("huge"), None);
    // The first layer of several that has a picture: a gradient over it does not hide it.
    assert_eq!(bg("background-image:url(a.png), url(b.png)").image, Some(Some("a.png".into())));
    assert_eq!(bg("background-image:linear-gradient(rgba(0,0,0,.4),rgba(0,0,0,.4)), url(b.png)").image, Some(Some("b.png".into())));
    assert_eq!(bg("background-image:none, url(b.png)").image, Some(Some("b.png".into())));
    assert_eq!(bg("background-image:linear-gradient(#fff,#000), radial-gradient(#fff,#000)").image, Some(None));
    let all = bg("background:linear-gradient(#fff 0, #000 100%), url(b.png) no-repeat right bottom / cover, #123456");
    assert_eq!(all.image, Some(Some("b.png".into())));
    assert_eq!((all.repeat, all.size), (Some(BgRepeat::None), Some(BgSize::Cover)));
    assert_eq!(all.position, Some([BgOffset::Percent(100), BgOffset::Percent(100)]));
    // The repeating, position and size that follow are those of that layer; a shorter list starts over.
    let after = bg("background-image:linear-gradient(#fff,#000), url(b.png);background-repeat:repeat-x, no-repeat;background-size:cover");
    assert_eq!((after.repeat, after.size), (Some(BgRepeat::None), Some(BgSize::Cover)));
    let after = bg("background-image:linear-gradient(#fff,#000), url(b.png), url(c.png);background-position:left top, right bottom");
    assert_eq!(after.image, Some(Some("b.png".into())));
    assert_eq!(after.position, Some([BgOffset::Percent(100), BgOffset::Percent(100)]));
    // Layers are looked at up to a bound: a picture past it is none.
    let far = format!("background-image:{}url(z.png)", "linear-gradient(#fff,#000),".repeat(40));
    assert_eq!(bg(&far).image, Some(None));
}

#[test]
fn a_picture_below_a_gradient_is_behind_the_box() {
    assert_eq!(srcs("<div style=\"background-image:linear-gradient(#fff,#eee),url(cid:paper@example.org)\">Inside</div>"), ["cid:paper@example.org"]);
}

#[test]
fn a_long_background_keeps_its_colour() {
    let long = format!("background:#123456 url(data:image/png;base64,{}) no-repeat", "A".repeat(1500));
    assert!(long.len() > 1024);
    assert_eq!(Css::parse(&long).background, Some(crate::Rgb(0x12, 0x34, 0x56)));
    // In a style sheet's rule too (an element takes only so much of a sheet, so a smaller one).
    let long = format!("background:#123456 url(data:image/png;base64,{}) no-repeat", "A".repeat(1100));
    let sheet = format!("<style>.x{{{long}}}</style><table><tr><td class=\"x\">a</td><td>b</td></tr></table>");
    let blocks = parse(&sheet);
    let BlockKind::Table(table) = &blocks[0].kind else { panic!("{blocks:?}") };
    assert_eq!(table.rows[0].cells[0].background, Some(crate::Rgb(0x12, 0x34, 0x56)));
    // One that is long for what is not a picture is ignored as ever.
    let words = format!("background:#123456 {}", "no-repeat ".repeat(200));
    assert_eq!(Css::parse(&words).background, None);
}

#[test]
fn a_cell_a_table_and_the_body_have_their_pictures_behind_them() {
    let html = format!(
        "<body background=\"{}\"><table background=\"cid:table@example.org\" width=600>\
         <tr><td background=\"cid:Hero@example.org\" style=\"background-repeat:no-repeat;background-position:center top\">Hello</td>\
         <td style=\"background:url('cid:cell@example.org') repeat-x\">World</td></tr></table></body>",
        png()
    );
    assert_eq!(srcs(&html), ["data:image/png", "cid:table@example.org", "cid:Hero@example.org", "cid:cell@example.org"]);
    let backs = backs(&html);
    assert_eq!(backs[2].repeat, BgRepeat::None);
    assert_eq!(backs[2].position, [BgOffset::Percent(50), BgOffset::Percent(0)]);
    assert_eq!(backs[3].repeat, BgRepeat::X);
    // A style's picture beats the attribute, and its `none` takes the attribute's away.
    assert_eq!(srcs("<table><tr><td background=\"cid:a\" style=\"background-image:url(cid:b)\">x</td><td>y</td></tr></table>"), ["cid:b"]);
    assert!(srcs("<table><tr><td background=\"cid:a\" style=\"background:none\">x</td><td>y</td></tr></table>").is_empty());
    // A block with a picture behind it is a box of its own, as one with a colour is.
    let blocks = parse("<div style=\"background-image:url(cid:d@example.org);padding:8px\">Inside</div>");
    assert!(matches!(&blocks[..], [Block { kind: BlockKind::Table(_), .. }]), "{blocks:?}");
    assert_eq!(srcs("<div style=\"background-image:url(cid:d@example.org)\">Inside</div>"), ["cid:d@example.org"]);
}

/// Every way a box can be given a picture from the web, on a table, a cell, a block and the body.
const WEB: [&str; 14] = [
    "<table background=\"https://t.example/a.png\"><tr><td>x</td><td>y</td></tr></table>",
    "<table style=\"background-image:url(http://t.example/a.png)\"><tr><td>x</td><td>y</td></tr></table>",
    "<table><tr><td background=\"http://t.example/a.png\">x</td><td>y</td></tr></table>",
    "<table><tr><td style=\"background:#fff url('https://t.example/a.png') repeat-x\">x</td><td>y</td></tr></table>",
    "<table><tr><td background=\"//t.example/a.png\">x</td><td>y</td></tr></table>",
    "<table><tr><td background=\"HTTPS://t.example/a.png\">x</td></tr></table>",
    "<table><tr><td background=\"https://t.example/a.png\"></td></tr></table>",
    "<table><tr><td style=\"background-image:url(https://t.example/a.png)\" width=600 height=300>&nbsp;</td></tr></table>",
    "<table border=1><tr><td background=\"https://t.example/a.png\"></td><td></td></tr></table>",
    "<div style=\"background-image:url(https://t.example/a.png);padding:8px\">Inside</div>",
    "<div style=\"background:url(https://t.example/a.png)\">Inside</div>",
    "<body background=\"https://t.example/a.png\"><p>Inside</p></body>",
    "<body style=\"background-image:url(https://t.example/a.png)\"><p>Inside</p></body>",
    "<style>.m{background:#003 url(https://t.example/a.png) no-repeat}</style><table><tr><td class=\"m\">x</td><td>y</td></tr></table><div class=\"m\">z</div>",
];

#[test]
fn a_picture_on_the_web_is_never_kept_behind_a_box() {
    for html in WEB {
        let blocks = parse(html);
        assert!(bg_pictures(&blocks).is_empty(), "{html}");
        // (Nothing of the address is anywhere in the result.)
        assert!(!format!("{blocks:?}").contains("t.example"), "{html}");
    }
    // The attribute does not fall back to the web when the style names none, and a picture in
    // the message beside one on the web is the only one kept.
    assert!(srcs("<table><tr><td background=\"https://t.example/a.png\" style=\"background-image:none\">x</td><td>y</td></tr></table>").is_empty());
    assert_eq!(
        srcs(
            "<table background=\"https://t.example/a.png\"><tr><td background=\"cid:a\">x</td><td background=\"https://t.example/b.png\">y</td></tr></table>"
        ),
        ["cid:a"]
    );
    // A block with only such a picture is no box: it is as it would be without.
    let blocks = parse("<div style=\"background-image:url(https://t.example/a.png)\">Inside</div>");
    assert!(blocks.iter().all(|b| !matches!(b.kind, BlockKind::Table(_))), "{blocks:?}");
}

#[test]
fn only_a_picture_that_may_be_shown_is_kept() {
    for bad in ["javascript:alert(1)", "file:///etc/passwd", "x.png", "/images/x.png", "mailto:a@example.org", "about:blank", "java\nscript:x"] {
        let attribute = format!("<table><tr><td background=\"{bad}\">x</td><td>y</td></tr></table>");
        let style = format!("<table><tr><td style=\"background-image:url('{bad}')\">x</td><td>y</td></tr></table>");
        assert!(srcs(&attribute).is_empty() && srcs(&style).is_empty(), "{bad}");
    }
    // Not on a paragraph, a row or a picture, which have no such attribute.
    assert!(srcs("<p background=\"cid:a\">x</p><table><tr background=\"cid:b\"><td>x</td><td>y</td></tr></table>").is_empty());
}

#[test]
fn a_picture_written_into_the_message_is_decoded_and_counted_with_the_others() {
    let html = format!("<table><tr><td background=\"{}\">x</td><td>y</td></tr></table>", png());
    let back = &backs(&html)[0];
    assert_eq!(back.src, "data:image/png");
    assert_eq!(back.data.as_deref(), Some(&PNG[..]));
    // In a style too, its base64 left as it was written.
    let back = &backs(&format!("<div style=\"background:url({}) no-repeat\">x</div>", png()))[0];
    assert_eq!(back.data.as_deref(), Some(&PNG[..]));
    // Plain text doesn't decode it, and what isn't a picture of ours is no picture.
    assert!(bg_pictures(&parse_text(&html)).is_empty());
    assert!(srcs("<table><tr><td background=\"data:image/svg+xml;base64,PHN2Zz4=\">x</td><td>y</td></tr></table>").is_empty());
    // Past the pictures a message may have written into it, there is none behind the box.
    let many = format!("{}{html}", format!("<img src=\"{}\">", png()).repeat(MAX_DATA_PICTURES));
    assert!(bg_pictures(&parse(&many)).is_empty());
}

#[test]
fn a_cell_with_only_a_picture_behind_it_is_kept() {
    let blocks = parse("<table><tr><td background=\"cid:spacer\" width=200 height=100></td><td>Text</td></tr></table>");
    let BlockKind::Table(table) = &blocks[0].kind else { panic!("{blocks:?}") };
    assert_eq!(table.rows[0].cells.len(), 2);
    assert!(table.rows[0].cells[0].bg_picture.is_some());
    // And a table of one cell with a picture behind it stays a table.
    let blocks = parse("<table><tr><td background=\"cid:hero\">Hello</td></tr></table>");
    assert!(matches!(&blocks[0].kind, BlockKind::Table(t) if t.rows[0].cells[0].bg_picture.is_some()), "{blocks:?}");
}

#[test]
fn a_style_sheet_puts_a_picture_behind_a_box() {
    let html = "<style>.hero{background:#003 url(cid:hero.jpg@example.org) no-repeat center / cover}</style>\
                <table><tr><td class=\"hero\">Big news</td><td>x</td></tr></table>";
    let backs = backs(html);
    assert_eq!(backs.len(), 1);
    assert_eq!(backs[0].src, "cid:hero.jpg@example.org");
    assert_eq!((backs[0].repeat, backs[0].size), (BgRepeat::None, BgSize::Cover));
}

#[test]
fn the_picture_behind_a_box_does_not_use_up_the_text() {
    let big = format!("data:image/png;base64,{}{}", b64(&PNG), "A".repeat(MAX_INPUT));
    let html = format!("<table><tr><td background=\"{big}\">x</td><td>y</td></tr></table><p>The end</p>");
    let blocks = parse(&html);
    assert!(blocks.iter().any(|b| b.text() == "The end"), "{:?}", blocks.last());
}

#[test]
fn a_reply_carries_no_picture_behind_a_box() {
    let quoted = quotable("<table background=\"cid:a@t.example\"><tr><td style=\"background-image:url(cid:b@t.example)\">x</td></tr></table>");
    assert!(!quoted.contains("t.example"), "{quoted}");
}

#[test]
fn what_is_hidden_has_no_picture_behind_it_and_asks_for_none() {
    for html in [
        "<div style=\"display:none\"><table background=\"cid:h1.png\"><tr><td background=\"cid:h2.png\">a</td><td>b</td></tr></table></div>",
        "<table><tr><td style=\"display:none\" background=\"cid:hc.png\">a</td><td>b</td></tr></table>",
        "<table><tr style=\"display:none\"><td background=\"cid:hr.png\">a</td><td>b</td></tr></table>",
        "<table hidden background=\"cid:ht.png\"><tr><td>a</td><td>b</td></tr></table>",
        "<div style=\"visibility:hidden;background-image:url(cid:hd.png)\">a</div>",
        "<style>.m{display:none}</style><table class=\"m\"><tr><td style=\"background:url(cid:hs.png)\">a</td><td>b</td></tr></table>",
    ] {
        assert!(srcs(html).is_empty(), "{html}");
    }
    // Nor does it use up what the message's written pictures may take: hidden copies are not decoded.
    let hidden = format!("<td background=\"{}\" style=\"display:none\">x</td>", png()).repeat(MAX_DATA_PICTURES);
    let html =
        format!("<table><tr>{hidden}<td>y</td></tr></table><img src=\"{0}\"><table><tr><td background=\"{0}\">z</td><td>w</td></tr></table>", png());
    let blocks = parse(&html);
    assert!(crate::images(&blocks).iter().all(|b| matches!(b.kind, BlockKind::Image { data: Some(_), .. })));
    assert_eq!(bg_pictures(&blocks).len(), 1);
}

#[test]
fn a_box_two_pixels_or_less_across_has_no_picture_behind_it() {
    for tiny in [
        "<table><tr><td background=\"cid:px.gif\" width=1 height=1></td><td>x</td></tr></table>",
        "<table background=\"cid:px.gif\" height=\"2\"><tr><td>x</td><td>y</td></tr></table>",
        "<div style=\"width:1px;background-image:url(cid:px.gif)\">x</div>",
    ] {
        assert!(srcs(tiny).is_empty(), "{tiny}");
    }
    assert_eq!(srcs("<table><tr><td background=\"cid:a\" width=3>x</td><td>y</td></tr></table>"), ["cid:a"]);
}

#[test]
fn a_slash_inside_a_colour_is_not_the_one_before_the_size() {
    let all = bg("background:rgb(0 0 0 / 50%) url(x.png) center / contain no-repeat");
    assert_eq!((all.position, all.size, all.repeat), (Some([BgOffset::Percent(50); 2]), Some(BgSize::Contain), Some(BgRepeat::None)));
    let all = bg("background:url(x.png) hsl(120 50% 50% / .5) right");
    assert_eq!(all.position, Some([BgOffset::Percent(100), BgOffset::Percent(50)]));
}

#[test]
fn a_picture_that_may_not_be_shown_makes_no_box_on_a_line() {
    for bad in ["x.png", "javascript:alert(1)"] {
        let html = format!(
            "<div><span style=\"display:inline-block;background-image:url({bad})\">A</span> <span style=\"display:inline-block\">B</span></div>"
        );
        let blocks = parse(&html);
        assert!(!blocks.iter().any(|b| matches!(&b.kind, BlockKind::Table(t) if t.inline)), "{bad}: {blocks:?}");
    }
}

#[test]
fn a_size_of_two_pixels_or_less_in_a_style_wins_over_a_larger_attribute() {
    // Any of the sizes given, as for an `<img>`: the style's pixel is the size it is drawn at.
    for tiny in [
        "<table><tr><td width=100 style=\"width:1px;height:1px\" background=\"cid:px.gif\">x</td><td>y</td></tr></table>",
        "<table><tr><td height=50 style=\"height:2px\" background=\"cid:px.gif\">x</td><td>y</td></tr></table>",
        "<table><tr><td width=1 style=\"width:300px\" background=\"cid:px.gif\">x</td><td>y</td></tr></table>",
    ] {
        assert!(srcs(tiny).is_empty(), "{tiny}");
    }
}

#[test]
fn a_cell_that_holds_nothing_keeps_a_picture_of_the_message() {
    // Nothing is asked of anyone for it, so there is nothing to hold back: one that is in the
    // message is behind the cell as it is behind any other.
    assert_eq!(srcs("<table><tr><td background=\"cid:e\"></td></tr></table>"), ["cid:e"]);
    assert_eq!(srcs("<table><tr><td background=\"cid:s\" width=200></td><td>Text</td></tr></table>"), ["cid:s"]);
}

#[test]
fn a_picture_written_into_a_cell_or_a_table_that_is_not_kept_takes_nothing_from_the_others() {
    // A table nested too deep to be one is not made, nor are its cells: their pictures are not
    // decoded, so the one after them still has what the message may hold.
    let cells = format!("<td background=\"{}\">x</td>", png()).repeat(MAX_DATA_PICTURES);
    let deep = format!("{}<table background=\"{}\"><tr>{cells}</tr></table>", "<table><tr><td>".repeat(crate::MAX_TABLE_DEPTH + 2), png());
    let blocks = parse(&format!("{deep}<img src=\"{0}\">", png()));
    let shown = crate::images(&blocks);
    assert!(!shown.is_empty() && shown.iter().all(|b| matches!(b.kind, BlockKind::Image { data: Some(_), .. })), "{shown:?}");
}
