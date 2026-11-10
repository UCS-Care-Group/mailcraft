// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A window's own row of commands: which labels and buttons give way first.

use super::*;

fn item(key: &'static str, section: u8, rank: u8, plain: bool) -> Item {
    Item {
        key,
        icon: "save",
        label: key.to_string(),
        tip: key.to_string(),
        section,
        rank,
        plain,
        menu: false,
        checked: false,
        primary: false,
        on: true,
    }
}

#[test]
fn labels_give_way_before_buttons_and_what_matters_most_last() {
    let items = [item("save", 0, 1, false), item("delete", 0, 3, true), item("showAs", 1, 5, false), item("private", 1, 6, true)];
    // Each button is 40 wide, and 100 with its label.
    let width = |_: usize, labelled: bool| if labelled { 100.0 } else { 40.0 };
    let all = |available: f32| fit(&items, &width, available);
    // Everything fits: 4 × 100, two gaps of 2 and a rule's 9.
    assert_eq!(all(413.0), (vec![(0, true), (1, true), (2, true), (3, true)], vec![]));
    // A little less: the labels of what its icon explains go, the last first.
    assert_eq!(all(412.0).0, [(0, true), (1, true), (2, true), (3, false)]);
    assert_eq!(all(300.0).0, [(0, true), (1, false), (2, true), (3, false)]);
    // Then the other labels, all but the first rank's.
    assert_eq!(all(240.0), (vec![(0, true), (1, false), (2, false), (3, false)], vec![]));
    // Then buttons go behind `…`, which takes room of its own; the first rank stays.
    assert_eq!(all(230.0), (vec![(0, true), (1, false), (2, false)], vec![3]));
    assert_eq!(all(150.0), (vec![(0, true)], vec![1, 2, 3]));
    // Last its label, and then the button itself.
    assert_eq!(all(100.0), (vec![(0, false)], vec![1, 2, 3]));
    assert_eq!(all(0.0), (vec![], vec![0, 1, 2, 3]));
    // Nothing to fit, and nonsense for a width.
    assert_eq!(fit(&[], &width, 100.0), (vec![], vec![]));
    assert_eq!(all(f32::NAN).1.len(), 4);
    assert_ne!(hit_id(1, "save"), hit_id(2, "save"));
}
