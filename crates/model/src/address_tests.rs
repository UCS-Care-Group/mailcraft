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

/// A name written back quoted, with a quote, a backslash, a comma or a semicolon in it, reads back
/// as the same one name (#1262).
#[test]
fn a_name_written_back_quoted_reads_back_the_same() {
    for typed in ["O\"Brien, Pat", "Smith, John", "Doe; Jane", "back\\slash, too", "\"", "plain"] {
        let line = Address::new("", typed).quoted();
        assert_eq!(Address::parse_list(&line), [Address::new("", typed)], "{line}");
        let twice = format!("{line}; {}", Address::new("", typed).quoted());
        assert_eq!(Address::parse_list(&twice).len(), 2, "{twice}");
    }
    assert_eq!(Address::parse("\"O\\\"Brien, Pat\""), Some(Address::new("", "O\"Brien, Pat")));
    assert_eq!(Address::parse("\"a@example.org\""), Some(Address::new("", "a@example.org")));
}
