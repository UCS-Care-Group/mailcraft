// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A backslash before a CR LF pair in a quoted CSS string is one escaped line break, as before an
//! LF alone: CSS reads CR LF as one line break before it reads escapes. The string goes on, and a
//! `}` inside it ends no rule. An unescaped CR LF still ends the string.

use mailcraft_html::style_sheet_rules;

#[test]
fn an_escaped_cr_lf_goes_on_with_the_string_as_an_escaped_lf_does() {
    let lf = "p { font-family: \"a\\\n}\"; color: red } q { color: blue }";
    let crlf = "p { font-family: \"a\\\r\n}\"; color: red } q { color: blue }";
    assert_eq!(style_sheet_rules(lf), 2, "fixture guard: two rules with an escaped LF");
    assert_eq!(style_sheet_rules(crlf), 2, "an escaped CR LF ended the string");
    let q = "p { font-family: \"a\\\r\n\"; } q { color: blue }";
    assert_eq!(style_sheet_rules(q), 2);
}
