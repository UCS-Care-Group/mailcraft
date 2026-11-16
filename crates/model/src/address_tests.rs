// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::*;

#[test]
fn a_name_ending_in_a_quote_survives_the_round_trip() {
    for name in ["Ada \"Countess\"", "Bob \"Bobby\""] {
        let list = vec![Address::new(name, "ada@example.org")];
        assert_eq!(Address::parse_list(&format_list(&list)), list, "{name}");
    }
}
