// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Categorize menu has one row for each category, whatever the alphabet of the capitals its
//! name is written in (#790): the lists compare names so, and so does the menu.

use mailcraft_model::{Category, Mailbox, Rgb};

use super::category_rows;

#[test]
fn a_name_an_item_carries_in_other_letters_of_any_alphabet_is_the_listed_rows() {
    let mut mailbox = Mailbox::default();
    let (green, red) = (Rgb(0x2E, 0x8B, 0x57), Rgb(0xD1, 0x3B, 0x3B));
    mailbox.categories = vec![Category { name: "Ärger".into(), color: green }, Category { name: "Проект".into(), color: red }];
    let carried = ["ärger".to_string(), "ПРОЕКТ".to_string(), "Garden".to_string()];
    let names: Vec<String> = category_rows(&mailbox, &[], &carried).into_iter().map(|c| c.name).collect();
    assert_eq!(names, ["Ärger", "Проект", "Garden"]);
}
