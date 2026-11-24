// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Review of #830: one stroke far wider than the curve it follows made usvg's stroking, which it
//! does as it builds the tree to find the paths' bounds, take seconds to minutes; the drawing
//! was accepted, and was read on the window's thread.

use std::time::{Duration, Instant};

use super::images;

/// The review's drawing: `n` tiny cubics stroked `width` wide, in a 100x100 viewBox.
fn wide(n: usize, width: &str) -> Vec<u8> {
    drawn(&format!("<path d=\"M0 0{}\" fill=\"none\" stroke=\"black\" stroke-width=\"{width}\"/>", " c1 0-1 1 0 1".repeat(n)))
}

/// A 100x100 drawing of `body`.
fn drawn(body: &str) -> Vec<u8> {
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\" width=\"100\" height=\"100\">{body}</svg>").into_bytes()
}

/// How long a drawing that is drawn may take: far over what it takes (a debug build on a busy
/// machine), far under what an unbounded stroke takes (minutes), so that only that is caught.
const DRAWN: Duration = Duration::from_secs(20);

/// How long reading `data` takes and what comes of it.
fn read(data: &[u8]) -> (Duration, Option<[u32; 2]>) {
    let start = Instant::now();
    let probed = images::probe(data);
    (start.elapsed(), probed)
}

#[test]
fn review_11_a_stroke_far_wider_than_the_drawing_is_refused_at_once() {
    for n in [100, 1000] {
        let (took, probed) = read(&wide(n, "1e8"));
        assert!(took < Duration::from_secs(1), "{n} cubics took {took:?}");
        assert_eq!(probed, None, "{n} cubics");
    }
}

#[test]
fn review_11_a_wide_stroke_is_refused_however_it_is_said() {
    let path = |style: &str| format!("<path d=\"M0 0{}\" {style}/>", " c1 0-1 1 0 1".repeat(1000));
    for style in [
        "fill=\"none\" stroke=\"black\" style=\"stroke-width:1e8\"",
        "fill=\"none\" stroke=\"black\" stroke-width=\"1e8px\"",
        "fill=\"none\" stroke=\"black\" stroke-width=\"1e7cm\"",
        "fill=\"none\" stroke=\"black\" stroke-width=\"1e9%\"",
    ] {
        let (took, probed) = read(&drawn(&path(style)));
        assert!(took < Duration::from_secs(1) && probed.is_none(), "{style}: {took:?} {probed:?}");
    }
    // A group's width is the width of what is in it.
    let (took, probed) = read(&drawn(&format!("<g fill=\"none\" stroke=\"black\" stroke-width=\"1e8\">{}</g>", path(""))));
    assert!(took < Duration::from_secs(1) && probed.is_none(), "{took:?} {probed:?}");
    // The same drawing in a drawing as large as the stroke is wide: no more than 10,000 units.
    let big = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1e9 1e9\">{}</svg>",
        path("fill=\"none\" stroke=\"black\" stroke-width=\"1e8\"")
    );
    assert_eq!(read(big.as_bytes()).1, None);
}

/// Strokes as they are drawn: wide for a stroke (up to a few times the drawing), icons' round
/// ones, and nothing wider than is worth reading.
#[test]
fn review_11_strokes_that_are_drawn_are_still_drawn() {
    for width in ["1", "8", "100", "400"] {
        let (took, probed) = read(&wide(100, width));
        assert!(took < DRAWN && probed == Some([100, 100]), "{width}: {took:?} {probed:?}");
    }
    let icon = drawn(
        "<path d=\"M10 50h80M50 10c20 20 20 60 0 80\" fill=\"none\" stroke=\"#000\" stroke-width=\"12\" stroke-linecap=\"round\" \
         stroke-linejoin=\"round\"/><circle cx=\"50\" cy=\"50\" r=\"40\" fill=\"none\" stroke=\"#000\" stroke-width=\"2\"/>",
    );
    assert_eq!(read(&icon).1, Some([100, 100]));
    // Over the drawing's own size a stroke only paints it all.
    assert_eq!(read(&wide(10, "401")).1, None);
}

/// The width weighed against the curve: the same stroke is refused (or counted for more) for
/// pieces too small for it, wherever they are, however they are written.
#[test]
fn review_11_a_stroke_is_weighed_by_its_width_over_the_curve() {
    // Absolute coordinates far from the origin, each curve a hair across: only their differences
    // tell.
    let dusty = "C50.000000001 50 50.000000002 50.000000001 50.000000003 50.000000002 ".repeat(200);
    let (took, probed) = read(&drawn(&format!("<path d=\"M50 50 {dusty}\" fill=\"none\" stroke=\"black\" stroke-width=\"90\"/>")));
    assert!(took < Duration::from_secs(1) && probed.is_none(), "{took:?} {probed:?}");
    // Little curves, a stroke 400 wide over curves a hundredth across: drawn, and quickly; ones
    // far smaller are dust.
    let small = |size: &str, n: usize| {
        let cubic = format!(" c{size} 0-{size} {size} 0 {size}");
        drawn(&format!("<path d=\"M0 0{}\" fill=\"none\" stroke=\"black\" stroke-width=\"400\"/>", cubic.repeat(n)))
    };
    let (took, probed) = read(&small("0.01", 400));
    assert!(took < DRAWN && probed.is_some(), "{took:?} {probed:?}");
    let (took, probed) = read(&small("0.0000001", 400));
    assert!(took < Duration::from_secs(1) && probed.is_none(), "{took:?} {probed:?}");
    // Shapes of no size at all stroked wide.
    let rects = "<rect width=\"0.0000001\" height=\"0.0000001\" fill=\"none\" stroke=\"black\" stroke-width=\"90\"/>".repeat(40);
    let (took, probed) = read(&drawn(&rects));
    assert!(took < Duration::from_secs(1) && probed.is_none(), "{took:?} {probed:?}");
    // Dust in a drawing that isn't stroked is nothing to usvg.
    let filled = "<rect width=\"0.0000001\" height=\"0.0000001\" fill=\"black\"/>".repeat(40);
    assert_eq!(read(&drawn(&filled)).1, Some([100, 100]));
}

/// A stroked path of `n` tiny cubics with `attributes` (its `style`, `stroke-width`, ...).
fn styled(n: usize, attributes: &str) -> Vec<u8> {
    drawn(&format!("<path d=\"M0 0{}\" fill=\"none\" {attributes}/>", " c1 0-1 1 0 1".repeat(n)))
}

/// Review of #830, second round: the width was taken from the first `stroke-width` of a `style`,
/// but usvg lets a later one replace it, so `stroke-width:1;stroke-width:1e8` was counted as 1 and
/// read as 1e8. Every declaration is weighed now, the widest counting, whatever the order.
#[test]
fn review_12_a_later_wide_duplicate_in_a_style_cannot_reach_usvg() {
    for style in [
        "stroke:black;stroke-width:1;stroke-width:1e8",
        "stroke:black;stroke-width:1;stroke-width:10001",
        "stroke:black; stroke-width : 1 ; stroke-width : 1e8 ; stroke-width:2",
        "stroke:black;stroke-width:1e8;stroke-width:1",
        "stroke:black;stroke-width:1;stroke-width:1e8 !important",
        "stroke:black;stroke-width:1e8!important;stroke-width:1",
        "stroke:black;stroke-width:1;stroke-width:1e8 !IMPORTANT",
    ] {
        let (took, probed) = read(&styled(100, &format!("style=\"{style}\"")));
        assert!(took < Duration::from_secs(1) && probed.is_none(), "{style}: {took:?} {probed:?}");
    }
}

/// The attribute and the style are both weighed (the style wins in usvg, but nothing here says
/// which is which), and a width that is no length cannot be bounded.
#[test]
fn review_12_an_attribute_and_a_style_are_both_weighed_and_a_broken_width_is_refused() {
    for attributes in [
        "stroke=\"black\" stroke-width=\"1e8\" style=\"stroke-width:1\"",
        "stroke=\"black\" stroke-width=\"1\" style=\"stroke-width:1e8\"",
        "stroke=\"black\" style=\"stroke-width:1;stroke-width:wide\"",
        "stroke=\"black\" style=\"stroke-width:1;stroke-width:\"",
        "stroke=\"black\" stroke-width=\"\" style=\"stroke-width:1\"",
    ] {
        let (took, probed) = read(&styled(100, attributes));
        assert!(took < Duration::from_secs(1) && probed.is_none(), "{attributes}: {took:?} {probed:?}");
    }
}

/// Ordinary styles still draw: one width, the same width said twice, an important one, a width in
/// an attribute with other things in the style.
#[test]
fn review_12_ordinary_styles_are_still_drawn() {
    for attributes in [
        "style=\"stroke:black;stroke-width:8\"",
        "style=\"stroke:black;stroke-width:8;stroke-width:8\"",
        "style=\"stroke:black;stroke-width:2;stroke-width:8 !important\"",
        "style=\"stroke:black;stroke-width:2;stroke-width:8 !IMPORTANT\"",
        "stroke=\"black\" stroke-width=\"8\" style=\"stroke-linecap:round;opacity:0.5\"",
        "stroke=\"black\" style=\"stroke-width:400\"",
    ] {
        let (took, probed) = read(&styled(100, attributes));
        assert!(took < DRAWN && probed == Some([100, 100]), "{attributes}: {took:?} {probed:?}");
    }
}

/// A drawing whose root also binds the prefix `s` to SVG's own namespace, with `body` in it.
fn prefixed(body: &str) -> Vec<u8> {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:s=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\" width=\"100\" height=\"100\">{body}</svg>"
    )
    .into_bytes()
}

/// Review of #830, third round: usvg reads an attribute in SVG's namespace by its local name, but
/// the bounds read only those of no namespace, so `s:stroke-width="1e8"` (and `s:style`, `s:d`,
/// `s:width`, ...) went past them. Such an attribute is refused.
#[test]
fn review_12_an_attribute_in_svgs_namespace_cannot_go_round_the_bounds() {
    let cubics = " c1 0-1 1 0 1".repeat(1000);
    for body in [
        format!("<path d=\"M0 0{cubics}\" fill=\"none\" stroke=\"black\" s:stroke-width=\"1e8\"/>"),
        format!("<path d=\"M0 0{cubics}\" fill=\"none\" stroke=\"black\" stroke-width=\"1\" s:stroke-width=\"1e8\"/>"),
        format!("<path d=\"M0 0{cubics}\" fill=\"none\" stroke=\"black\" s:style=\"stroke-width:1e8\"/>"),
        format!("<path s:d=\"M0 0{cubics}\" fill=\"none\" stroke=\"black\" stroke-width=\"90\"/>"),
        "<rect s:width=\"0.0000001\" s:height=\"0.0000001\" fill=\"none\" stroke=\"black\" stroke-width=\"90\"/>".repeat(40),
        "<circle s:r=\"0.0000001\" fill=\"none\" stroke=\"black\" stroke-width=\"90\"/>".repeat(40),
        format!("<polyline s:points=\"0 0 {}\" fill=\"none\" stroke=\"black\" stroke-width=\"90\"/>", "1 1 0 0 ".repeat(500)),
    ] {
        let (took, probed) = read(&prefixed(&body));
        assert!(took < Duration::from_secs(1) && probed.is_none(), "{body:.80}: {took:?} {probed:?}");
    }
    // The same drawing with the prefix bound and not used is drawn.
    let plain = format!("<path d=\"M0 0{}\" fill=\"none\" stroke=\"black\" stroke-width=\"8\"/>", " c1 0-1 1 0 1".repeat(100));
    assert_eq!(read(&prefixed(&plain)).1, Some([100, 100]));
}

/// `!important` in any case is taken off the width: a valid one is drawn, a wide one refused.
#[test]
fn review_12_important_is_read_in_any_case() {
    for style in ["stroke-width:1e8 !Important", "stroke-width:1e8!IMPORTANT", "stroke-width:1;stroke-width:1e8 !iMpOrTaNt"] {
        let (took, probed) = read(&styled(100, &format!("stroke=\"black\" style=\"{style}\"")));
        assert!(took < Duration::from_secs(1) && probed.is_none(), "{style}: {took:?} {probed:?}");
    }
    assert_eq!(read(&styled(100, "stroke=\"black\" style=\"stroke-width:8 !ImPoRtAnT\"")).1, Some([100, 100]));
    // A value shorter than the word, or with a multi-byte character where it would start, is no panic.
    for width in ["", "!", "imp", "ä", "äimportant", "important", "1é"] {
        let _ = read(&styled(2, &format!("stroke=\"black\" style=\"stroke-width:{width}\"")));
    }
}
