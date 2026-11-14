// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn ids(src: &str) -> Vec<String> {
    scan(src).into_iter().map(|s| s.id).collect()
}

#[test]
fn finds_t_on_any_receiver() {
    assert_eq!(ids(r#"Text(core.t("Loading…")); val x = t("Inbox")"#), vec!["Loading…", "Inbox"]);
}

#[test]
fn finds_both_forms_of_tn() {
    let s = scan(r#"core.tn(n, "{n} message", "{n} messages")"#);
    assert_eq!(s, vec![Said { id: "{n} message".into(), plural: Some("{n} messages".into()) }]);
}

#[test]
fn tn_with_a_call_as_its_count_still_finds_the_texts() {
    let s = scan(r#"tn(rows.size.toULong(), "{n} row", "{n} rows")"#);
    assert_eq!(s[0].plural.as_deref(), Some("{n} rows"));
}

#[test]
fn words_that_end_in_t_are_not_calls() {
    assert_eq!(ids(r#"get("x"); format("y"); at("z"); rt("w"); x.print("v")"#), Vec::<String>::new());
}

#[test]
fn a_variable_or_a_template_is_skipped() {
    assert!(ids(r#"t(name); t("Hello $name"); t("a" + b); t("${x}")"#).is_empty());
}

#[test]
fn escapes_are_read() {
    assert_eq!(ids(r#"t("say \"hi\"\n… \$5")"#), vec!["say \"hi\"\n… $5"]);
}

#[test]
fn comments_and_strings_hide_calls() {
    let src = "// t(\"no\")\n/* t(\"nor\") */ val s = \"t(\\\"in a string\\\")\"; t(\"yes\")";
    assert_eq!(ids(src), vec!["yes"]);
}

#[test]
fn junk_never_panics() {
    for src in ["t(", "t(\"", "tn(", "tn(1,", "\"", "/*", "t(\"\\", "t(\"\\u12", "tn(1, \"a\")", "t()", "\\"] {
        let _ = scan(src);
    }
}

#[test]
fn white_space_may_stand_before_the_parenthesis() {
    assert_eq!(ids("core.t (\"Inbox\"); core.t\n(\"Sent\")"), vec!["Inbox", "Sent"]);
    let s = scan("tn (n, \"{n} row\", \"{n} rows\")");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].plural.as_deref(), Some("{n} rows"));
    assert!(ids("t \"x\"; tx (\"y\")").is_empty());
}

#[test]
fn a_parenthesized_count_keeps_its_slot() {
    let s = scan("tn((n), \"{n} row\", \"{n} rows\"); tn([n], \"{n} a\", \"{n} b\")");
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].id, "{n} row");
    assert_eq!(s[0].plural.as_deref(), Some("{n} rows"));
    assert_eq!(s[1].id, "{n} a");
}

#[test]
fn triple_quoted_strings_are_read() {
    assert_eq!(ids("t(\"\"\"Inbox\"\"\")"), vec!["Inbox"]);
    assert_eq!(ids("t(\"\"\"say \"hi\" \\n\"\"\")"), vec!["say \"hi\" \\n"]);
    assert!(ids("t(\"\"\"Hello $name\"\"\")").is_empty());
    assert_eq!(ids("val s = \"\"\"t(\"no\")\"\"\"; t(\"yes\")"), vec!["yes"]);
    assert_eq!(ids("t(\"\")"), vec![""]);
}
