// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn free_events_keep_the_category_tint_and_hollow_edge() {
    let free = Event { show_as: ShowAs::Free, categories: vec!["Red category".into()], ..Default::default() };
    let busy = Event { show_as: ShowAs::Busy, ..free.clone() };
    let uncategorized = Event { categories: Vec::new(), ..free.clone() };
    for t in [Tokens::light(), Tokens::dark()] {
        for dim in [false, true] {
            let tinted = look(&free, t.red, t.blue, &t, false, dim);
            assert_eq!(tinted.fill, look(&busy, t.red, t.blue, &t, false, dim).fill);
            assert!(tinted.kind == Edge::Hollow && tinted.outline);
        }
        assert_eq!(look(&uncategorized, t.blue, t.blue, &t, false, false).fill, mix(t.surface, t.blue, t.data_soft(false) * 0.2));
    }
}
