// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #600 in the reading pane: a message is laid out again once the font it asks for has arrived.
//! The rest of what decides which font a message is drawn in is `crate::fonts`' own tests.

use super::Place;
use super::layout::ItemKind;
use super::tests::{app_with, select, web_message, window};
use super::view::with_laid;
use crate::font_index::font_index_tests::FontsDir;
use crate::fonts::{Store, family_of};

/// A font that is certainly one: the bundled Inter, standing in for an installed face.
const REGULAR: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");

#[test]
fn the_reading_pane_lays_a_message_out_again_once_its_font_has_come() {
    let dir = FontsDir::new("pane");
    dir.put("inter.ttf", REGULAR);
    let mut message = web_message(1, "news@example.org", "In a font of its own", &[]);
    message.body.html = Some("<p style=\"font-family:'No Such Font', Inter, sans-serif\">Set in Inter</p>".into());
    let (app, ids) = app_with(vec![message]);
    let mut harness = window(app, (900.0, 700.0));
    Store::use_index(&harness.ctx, dir.index());
    select(&mut harness, ids[0]);
    harness.run_steps(4);
    let families = with_laid(&harness.ctx, Place::Pane, ids[0], |laid| {
        laid.items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Text(text) => Some(text.job.sections.iter().map(|s| s.format.font_id.family.clone()).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>()
    })
    .unwrap();
    assert_eq!(families, vec![family_of("inter", false)]);
}
