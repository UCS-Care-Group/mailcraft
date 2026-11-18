// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Texts within texts, and words said as they are (#872).
use crate::{Arg, Lang, Text};

#[test]
fn a_text_within_a_text_is_said_in_the_language_and_english_reads_as_one_line() {
    let why = said!("the server doesn't take a password on this connection");
    let line = said!("Couldn't check for new mail: {why}", why = Arg::said(said!("sign-in failed: {reason}", reason = Arg::said(why))));
    assert_eq!(line.to_string(), "Couldn't check for new mail: sign-in failed: the server doesn't take a password on this connection");
    let pseudo = line.in_lang(Lang::Pseudo);
    assert!(pseudo.starts_with("Cóúldn't") && pseudo.contains("sígn-ín") && pseudo.contains("pásswórd"), "{pseudo}");
}

#[test]
fn verbatim_words_are_said_as_they_are_in_every_language() {
    let server = Text::verbatim("Basic authentication is disabled. {why}");
    assert!(server.is_verbatim());
    assert_eq!(server.to_string(), "Basic authentication is disabled. {why}");
    assert_eq!(server.in_lang(Lang::Pseudo), "Basic authentication is disabled. {why}");
    let line = said!("sign-in failed: {reason}", reason = Arg::said(server.clone()));
    assert_eq!(line.in_lang(Lang::Pseudo), "sígn-ín fáíléd: Basic authentication is disabled. {why}");
    assert_eq!(Text::from("as it is"), Text::verbatim("as it is"));
    assert_eq!(Text::from(String::from("as it is")), Text::verbatim("as it is"));
}

#[test]
fn plain_values_are_changed_all_the_way_down() {
    let line = said!(
        "{what}: {why}",
        what = "a secret here",
        why = Arg::said(said!("sign-in failed: {reason}", reason = Arg::said(Text::verbatim("no user with the password secret"))))
    );
    let blanked = line.map_plain(&|s: String| s.replace("secret", "***"));
    assert_eq!(blanked.to_string(), "a *** here: sign-in failed: no user with the password ***");
    assert!(!blanked.in_lang(Lang::Pseudo).contains("secret"));
}
