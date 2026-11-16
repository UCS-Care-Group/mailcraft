// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

/// The texts `literals` gathers from `src`, as a window file.
fn gathered(src: &str) -> Vec<String> {
    let mut texts = Texts::new();
    literals(&lex(src), "crates/ui-egui/src/example.rs", &BTreeMap::new(), &BTreeMap::new(), &mut texts);
    texts.into_keys().collect()
}

#[test]
fn a_text_marked_for_later_is_gathered_wherever_it_stands() {
    let src = r##"
        const WHY: &str = tl_later!("Give it a title first");
        let tip = if pinned { tl_later!("Unpin") } else { tl_later!("Pin") };
        const COLORS: [(&str, &str); 1] = [(tl_later!("Dark Red"), "#c00000")];
    "##;
    assert_eq!(gathered(src), ["Dark Red", "Give it a title first", "Pin", "Unpin"]);
}

#[test]
fn a_bare_literal_that_reaches_tl_through_a_variable_is_not() {
    let src = r#"let note = if offline { "No account is connected to a mail server yet." } else { "" }; tl!(note);"#;
    assert!(gathered(src).is_empty());
}
