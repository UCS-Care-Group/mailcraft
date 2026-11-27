// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1966: the room a message asks for in three old ways, drawn: `cellpadding`, Outlook's own
//! `mso-` spacing, and a bar of colour that holds nothing but has a height.
//!
//! The people and addresses are invented.

use super::{find, html_message, in_frame, lay_out};
use crate::reading::layout::{ItemKind, Laid};

fn laid(ctx: &egui::Context, html: &str) -> Laid {
    lay_out(ctx, &html_message(0, ("Fern Hollow", "orders@example.org"), "Your order", html), 600.0)
}

/// Where the text that starts with `start` is, from the top left of the message.
fn at(ctx: &egui::Context, html: &str, start: &str) -> egui::Pos2 {
    find(&laid(ctx, html), start).rect.min
}

#[test]
fn cellpadding_sets_in_the_text_of_cells_that_give_no_padding() {
    in_frame(false, |ctx| {
        let table = |pad: &str| format!("<table border=1 cellspacing=0 cellpadding={pad}><tr><td>Seed trays</td><td>£24.00</td></tr></table>");
        let (none, ten) = (at(ctx, &table("0"), "Seed trays"), at(ctx, &table("10"), "Seed trays"));
        assert!((ten.x - none.x - 10.0).abs() < 1.0 && (ten.y - none.y - 10.0).abs() < 1.0, "{none:?} {ten:?}");
        // A cell's own padding wins.
        let own = at(
            ctx,
            "<table border=1 cellspacing=0 cellpadding=10><tr><td style='padding:0'>Seed trays</td><td>£24.00</td></tr></table>",
            "Seed trays",
        );
        assert!((own.x - none.x).abs() < 1.0, "{own:?} {none:?}");
    });
}

#[test]
fn outlooks_own_padding_is_drawn_where_the_plain_one_is_missing() {
    in_frame(false, |ctx| {
        let cell = |style: &str| format!("<table border=1 cellspacing=0><tr><td style='{style}'>Order 42</td><td>Due</td></tr></table>");
        let alt = at(ctx, &cell("mso-padding-alt:6pt 12pt"), "Order 42");
        let plain = at(ctx, &cell("padding:6pt 12pt"), "Order 42");
        assert!((alt - plain).length() < 0.5, "{alt:?} {plain:?}");
        let both = at(ctx, &cell("padding:0;mso-padding-alt:40px"), "Order 42");
        assert!((both - at(ctx, &cell("padding:0"), "Order 42")).length() < 0.5);
    });
}

#[test]
fn an_empty_coloured_block_with_a_height_is_a_bar_above_its_card() {
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let laid = laid(ctx, "<div style='height:4px;background:#2da44e'></div><div style='border:1px solid #d0d7de'>Resolved</div>");
            let text = find(&laid, "Resolved").rect;
            let bars: Vec<egui::Rect> = laid
                .items
                .iter()
                .filter(|i| matches!(i.kind, ItemKind::Fill { .. }) && (i.rect.height() - 4.0).abs() < 0.5 && i.rect.width() > 500.0)
                .map(|i| i.rect)
                .collect();
            assert_eq!(bars.len(), 1, "{:?}", laid.items);
            assert!(bars.iter().all(|bar| bar.max.y <= text.min.y), "{bars:?} {text:?}");
        });
    }
}

#[test]
fn a_height_without_colour_makes_no_room() {
    in_frame(false, |ctx| {
        let plain = at(ctx, "<p>One</p><p>Two</p>", "Two");
        let spaced = at(ctx, "<p>One</p><div style='height:60px'></div><p>Two</p>", "Two");
        assert!((plain.y - spaced.y).abs() < 0.5, "{plain:?} {spaced:?}");
    });
}
