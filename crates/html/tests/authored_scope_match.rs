// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A rule the writer of an authored signature scopes still matches the elements it matched: an
//! attribute test of the signature's `<body>` (which the wrapper can't pass as the body did) is
//! refused, the body's classes are kept with their spaces that aren't HTML's, and a selector keeps
//! the characters that aren't CSS white space at its ends.
//!
//! The documents are invented; nothing here is fetched.

use mailcraft_html::compat::check;
use mailcraft_html::{AuthoredError, AuthoredFragment, AuthoredScope, authored_fragment_with_rebound_cids};

fn write(html: &str) -> Result<AuthoredFragment, AuthoredError> {
    let scope = AuthoredScope::new("mc-test").expect("a scope class");
    authored_fragment_with_rebound_cids(html, &scope, &mut |cid: &str| Some(cid.to_string()))
}

fn document(sheet: &str, body: &str) -> String {
    format!("<html><head><style>{sheet}</style></head>{body}</html>")
}

#[test]
fn an_attribute_test_of_the_body_is_refused_and_the_checker_says_so() {
    for (sheet, body) in [
        (r#"body[class="card"] > p { color: red }"#, r#"<body class="card"><p>Priya Raman</p></body>"#),
        (r#"body[class] p { color: red }"#, r#"<body class="card"><p>Priya Raman</p></body>"#),
        (r#"body[dir="rtl"] p { color: red }"#, r#"<body dir="rtl"><p>Priya Raman</p></body>"#),
    ] {
        let html = document(sheet, body);
        assert_eq!(write(&html), Err(AuthoredError::Unsupported), "{sheet}");
        let ids: Vec<&str> = check(&html).expect("checked").iter().map(|f| f.rule.id()).collect();
        assert!(ids.contains(&"MC-CSS-001"), "{sheet}: refused at save: {ids:?}");
    }
    // Control: a class test of the body keeps its match on the wrapper.
    let kept = write(&document("body.card > p { color: red }", r#"<body class="card"><p>Priya Raman</p></body>"#)).expect("written");
    assert!(kept.head.contains("div.mc-test.card > p { color: red }"), "{}", kept.head);
}

#[test]
fn the_body_classes_keep_a_space_that_is_not_htmls() {
    let html = document("body.card > p { color: red }", "<body class=\"card\u{a0}\"><p>Priya Raman</p></body>");
    let written = write(&html).expect("written");
    assert!(written.body.starts_with("<div class=\"mc-test card\u{a0}\">"), "the body's one class is not `card`: {}", written.body);
    // HTML's own spaces round the classes go, as before.
    let written = write(&document("body.card > p { color: red }", "<body class=\" card \t\"><p>Priya Raman</p></body>")).expect("written");
    assert!(written.body.starts_with("<div class=\"mc-test card\">"), "{}", written.body);
}

#[test]
fn a_selector_keeps_a_character_at_its_end_that_is_not_css_white_space() {
    let html = document(".card\u{1680} { color: red }", "<body><p class=\"card\">Priya Raman</p></body>");
    let written = write(&html).expect("written");
    assert!(written.head.contains(".mc-test .card\u{1680}"), "the rule now names the class `card`: {}", written.head);
    // CSS's own white space round a selector goes, as before.
    let written = write(&document(" \t.card\n { color: red }", "<body><p class=\"card\">Priya Raman</p></body>")).expect("written");
    assert!(written.head.contains(".mc-test .card, .mc-test.card { color: red }"), "{}", written.head);
}
