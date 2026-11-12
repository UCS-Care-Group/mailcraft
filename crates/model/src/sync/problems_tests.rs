// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A run's problems, told as texts for the window and as English for everybody else (#872).
use mailcraft_i18n::{Arg, Lang, Text, said};

use super::SyncResult;

fn refused() -> Text {
    said!("Couldn't check for new mail: {why}", why = Arg::said(said!("sign-in failed: {reason}", reason = Arg::said(Text::verbatim("no")))))
}

#[test]
fn a_problem_is_told_in_english_and_kept_as_its_text() {
    let mut result = SyncResult::default();
    result.problem(refused());
    result.errors.push("a line told in English only".into());
    assert_eq!(result.errors, ["Couldn't check for new mail: sign-in failed: no", "a line told in English only"]);
    let problems = result.problems();
    assert_eq!(problems.len(), 2);
    assert_eq!(problems[0], refused());
    assert!(problems[1].is_verbatim() && problems[1].to_string() == "a line told in English only");
    assert_eq!(result.summary(), "Couldn't check for new mail: sign-in failed: no");
}

#[test]
fn a_line_changed_after_it_was_told_is_said_as_it_now_stands() {
    let mut result = SyncResult::default();
    result.problem(refused());
    result.errors[0] = result.errors[0].replace("no", "***");
    let problems = result.problems();
    assert!(problems[0].is_verbatim());
    assert_eq!(problems[0].in_lang(Lang::Pseudo), result.errors[0]);
}

#[test]
fn the_texts_never_cross_as_json_and_pieces_keep_them_together() {
    let mut first = SyncResult { partial: true, ..Default::default() };
    first.problem(refused());
    let json = serde_json::to_value(&first).unwrap();
    assert_eq!(json["errors"], serde_json::json!(["Couldn't check for new mail: sign-in failed: no"]));
    assert!(json.get("said").is_none(), "{json}");
    let back: SyncResult = serde_json::from_value(json).unwrap();
    assert!(back.said.is_empty() && back.problems()[0].is_verbatim());
    let mut later = SyncResult::default();
    later.problem(said!("Couldn't send mail"));
    first.absorb(later);
    assert_eq!(first.problems(), [refused(), said!("Couldn't send mail")]);
}
