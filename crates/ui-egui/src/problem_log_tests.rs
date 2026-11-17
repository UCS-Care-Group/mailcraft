// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which Send/Receive problem texts reach the log (`problem_log`).

use mailcraft_i18n::{Arg, Text};

use crate::problem_log::{MAX_CHARS, MAX_LOGGED, MAX_SEEN, ProblemLog, masked};

fn s(t: &str) -> Vec<String> {
    vec![t.to_string()]
}

fn many(n: usize, from: usize) -> Vec<String> {
    (from..from + n)
        .map(|i| format!("problem number {}", "x".repeat(i % 7) + &"-".repeat(i) + &format!("{}", (b'a' + (i % 26) as u8) as char)))
        .collect()
}

#[test]
fn a_text_is_logged_once_and_again_when_it_changes() {
    let mut log = ProblemLog::default();
    let a = vec!["folder gone".to_string(), "server busy".to_string()];
    assert_eq!(log.fresh(1, "mail", &a), (a.clone(), 0));
    assert_eq!(log.fresh(1, "mail", &a), (vec![], 0));
    assert_eq!(log.fresh(1, "mail", &s("server said 5xx")), (s("server said 5xx"), 0));
    // The first two are still known: alternating texts say nothing more.
    assert_eq!(log.fresh(1, "mail", &a), (vec![], 0));
    // Another account is its own.
    assert_eq!(log.fresh(2, "mail", &a).0.len(), 2);
}

#[test]
fn pieces_alternating_over_two_runs_log_nothing_in_the_second() {
    let mut log = ProblemLog::default();
    for run in 0..2 {
        for text in ["A went wrong", "B went wrong"] {
            let (said, _) = log.fresh(1, "mail", &s(text));
            assert_eq!(said.len(), usize::from(run == 0), "run {run}: {text}");
        }
        log.end_run(1, "mail");
    }
}

#[test]
fn a_count_or_a_time_in_a_text_does_not_make_it_new() {
    let mut log = ProblemLog::default();
    assert_eq!(log.fresh(1, "mail", &s("Command failed (0.001 + 0.000 secs), 12 tries")).0.len(), 1);
    assert!(log.fresh(1, "mail", &s("Command failed (0.254 + 1.300 secs), 7 tries")).0.is_empty());
}

#[test]
fn one_job_does_not_clobber_another_and_only_its_own_clean_finish_forgets() {
    let mut log = ProblemLog::default();
    log.fresh(1, "mail", &s("mail trouble"));
    log.fresh(1, "calendar", &s("calendar trouble"));
    // A clean calendar run says nothing of mail.
    log.forget(1, "calendar");
    assert!(log.fresh(1, "mail", &s("mail trouble")).0.is_empty());
    assert_eq!(log.fresh(1, "calendar", &s("calendar trouble")).0.len(), 1);
}

#[test]
fn the_run_cap_holds_across_calls_and_returns_with_the_next_run() {
    let mut log = ProblemLog::default();
    let mut said = 0;
    let mut more = 0;
    for call in 0..4 {
        let (texts, over) = log.fresh(1, "mail", &many(10, call * 10));
        said += texts.len();
        more += over;
    }
    assert_eq!((said, more), (MAX_LOGGED, 40 - MAX_LOGGED));
    log.end_run(1, "mail");
    // What wasn't said is said in the next run.
    assert_eq!(log.fresh(1, "mail", &many(10, 20)).0.len(), 10);
}

#[test]
fn what_is_remembered_is_bounded_and_the_oldest_goes_first() {
    let mut log = ProblemLog::default();
    for run in 0..(MAX_SEEN / MAX_LOGGED + 2) {
        log.fresh(1, "mail", &many(MAX_LOGGED, run * MAX_LOGGED));
        log.end_run(1, "mail");
    }
    // The first text has been let go of, the last has not.
    assert_eq!(log.fresh(1, "mail", &many(1, 0)).0.len(), 1);
    log.end_run(1, "mail");
    assert!(log.fresh(1, "mail", &many(1, (MAX_SEEN / MAX_LOGGED + 1) * MAX_LOGGED)).0.is_empty());
}

#[test]
fn a_text_is_cut_at_a_character_boundary_on_one_line_without_long_base64() {
    let mut log = ProblemLog::default();
    let long = format!("é\n{} AHVzZXJAZXhhbXBsZS5vcmcAaHVudGVyMg== ok", "ü".repeat(MAX_CHARS * 2));
    let (logged, _) = log.fresh(1, "mail", &[long]);
    assert_eq!(logged[0].chars().count(), MAX_CHARS);
    assert!(!logged[0].contains('\n'));
    let (logged, _) = log.fresh(1, "mail", &s("AUTH PLAIN AHVzZXJAZXhhbXBsZS5vcmcAaHVudGVyMg== sent"));
    assert_eq!(logged[0], "AUTH PLAIN sent");
}

#[test]
fn every_value_of_a_text_is_put_aside_but_a_number() {
    let text = Text {
        template: "Couldn't send \"{subject}\" ({n}): {why}",
        args: vec![
            ("subject", Arg::Plain("Re: \"Offer\" for Bob Smith".into())),
            ("n", Arg::Plain("3".into())),
            ("why", Arg::Said(Box::new(Text { template: "refused {who}", args: vec![("who", Arg::Plain("ada@example.org".into()))] }))),
        ],
    };
    assert_eq!(masked(&text), "Couldn't send \"…\" (…): refused …");
}

#[test]
fn a_servers_code_is_kept_and_its_words_are_not() {
    let said = |words: &str| Text { template: "the server said: {reason}", args: vec![("reason", Arg::Said(Box::new(Text::verbatim(words))))] };
    for (words, want) in [
        ("[OVERQUOTA] Quota exceeded for ada@example.org", "the server said: [OVERQUOTA]…"),
        ("[TRYCREATE] no folder Taxes", "the server said: [TRYCREATE]…"),
        ("[AUTHENTICATIONFAILED] bad", "the server said: [AUTHENTICATIONFAILED]…"),
        ("552 5.3.4 Message too big for Bob", "the server said: 552 5.3.4…"),
        ("550 No such user ada@example.org", "the server said: 550…"),
        ("450 5.1.1.1 odd", "the server said: 450…"),
        ("[lower] no", "the server said: …"),
        ("[HUNTER2-SECRET-PW] refused", "the server said: …"),
        ("[OVERQUOTA hunter2] refused", "the server said: …"),
        ("[ada@example.org] no", "the server said: …"),
        ("5501 no", "the server said: …"),
        ("Taxes is gone [OVERQUOTA]", "the server said: …"),
        ("[é] no", "the server said: …"),
    ] {
        assert_eq!(masked(&said(words)), want, "{words}");
    }
}

#[test]
fn a_value_given_to_a_template_never_keeps_a_code_shape() {
    let text = Text { template: "Couldn't read \"{name}\"", args: vec![("name", Arg::Plain("[JOHN-SMITH] 2026".into()))] };
    assert_eq!(masked(&text), "Couldn't read \"…\"");
    let text =
        Text { template: "the server said: {reason}", args: vec![("reason", Arg::Said(Box::new(Text::verbatim("[OVERQUOTA] Quota exceeded"))))] };
    assert_eq!(masked(&text), "the server said: [OVERQUOTA]…");
}

#[test]
fn different_reply_codes_are_different_problems_but_counts_are_not() {
    let mut log = ProblemLog::default();
    assert_eq!(log.fresh(1, "mail", &s("the server said: 550…")).0.len(), 1);
    assert_eq!(log.fresh(1, "mail", &s("the server said: 552 5.3.4…")).0.len(), 1);
    assert_eq!(log.fresh(1, "mail", &s("the server said: 553 5.3.4…")).0.len(), 1);
    assert!(log.fresh(1, "mail", &s("the server said: 550…")).0.is_empty());
    assert_eq!(log.fresh(1, "mail", &s("the server said: [OVERQUOTA]…")).0.len(), 1);
    assert_eq!(log.fresh(1, "mail", &s("the server said: [TRYCREATE]…")).0.len(), 1);
}
