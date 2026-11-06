// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A name from outside in the meeting dialogs, as a name is shown (#454).

use super::*;

#[test]
fn a_name_loses_what_shows_nothing() {
    assert_eq!(shown_name("Bo\u{202e} Marsh\u{200b}\u{e0041}", "bo@example.org"), "Bo Marsh");
    assert_eq!(shown_name("  Bo\t\r\n Marsh ", "bo@example.org"), "Bo Marsh");
    assert_eq!(shown_name("Bo 👩\u{200d}💻", "bo@example.org"), "Bo 👩\u{200d}💻");
}

#[test]
fn the_address_stands_in_for_a_name_of_nothing_that_shows() {
    assert_eq!(shown_name("\u{3164}\u{200b}\u{2066}", "bo@example.org"), "bo@example.org");
    assert_eq!(shown_name("", " bo@example.org\u{feff} "), "bo@example.org");
    assert_eq!(shown_name("\u{2800}", "\u{e0041}"), "Somebody");
    assert_eq!(shown_name("", ""), "Somebody");
}

#[test]
fn a_name_is_cut_at_80_with_the_ellipsis_inside() {
    assert_eq!(shown_name(&"b".repeat(300), "bo@example.org"), format!("{}…", "b".repeat(79)));
    assert_eq!(shown_name(&"b".repeat(80), "bo@example.org"), "b".repeat(80));
    assert_eq!(shown_name("", &format!("{}@example.org", "b".repeat(300))), format!("{}…", "b".repeat(79)));
}
