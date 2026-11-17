// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Categories dialog's palette.

use super::*;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

#[test]
fn the_palette_has_twelve_colours_and_offers_a_free_one() {
    let distinct: std::collections::BTreeSet<String> = PALETTE.iter().map(|c| c.hex()).collect();
    assert_eq!(distinct.len(), 12);
    // The six categories a mailbox starts with take red to green, blue and purple.
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    assert_eq!(unused_color(&s.mailbox.categories), Rgb(0x1A, 0x9B, 0xA6));
    assert_eq!(unused_color(&[]), PALETTE[0]);
    let all: Vec<Category> = PALETTE.iter().map(|c| Category { name: c.hex(), color: *c }).collect();
    assert_eq!(unused_color(&all), PALETTE[0]);
    // The row at the bottom adds a category in that colour.
    let mut f = CategoriesForm::new(&s, &json!({})).unwrap();
    f.new_name = "Travel".into();
    assert_eq!(f.add_command(), ("category.set", json!({"name": "Travel", "color": "#1A9BA6"})));
    let (command, params) = f.add_command();
    s.run(command, &params).unwrap();
    assert_eq!(s.mailbox.category("travel").map(|c| c.color), Some(Rgb(0x1A, 0x9B, 0xA6)));
    assert_eq!(unused_color(&s.mailbox.categories), Rgb(0xC2, 0x4E, 0x9A));
    // A hostile name is the command's to clean.
    f.new_name = "\u{0}\u{202e}é\n".repeat(40_000);
    let (command, params) = f.add_command();
    s.run(command, &params).unwrap();
    assert!(s.mailbox.categories.iter().all(|c| c.name.chars().count() <= mailcraft_model::MAX_NAME));
}
