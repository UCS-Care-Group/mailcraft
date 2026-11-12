// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which Send/Receive problem texts reach the log (`problem_log`).

use crate::problem_log::{MAX_CHARS, MAX_LOGGED, forget, fresh};

fn texts(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("problem number {i}")).collect()
}

#[test]
fn a_text_is_logged_once_and_again_when_it_changes() {
    let mut seen = Vec::new();
    let a = vec!["folder gone".to_string(), "server busy".to_string()];
    assert_eq!(fresh(&mut seen, 1, &a), (a.clone(), 0));
    assert_eq!(fresh(&mut seen, 1, &a), (vec![], 0));
    let b = vec!["folder gone".to_string(), "server said 5xx".to_string()];
    assert_eq!(fresh(&mut seen, 1, &b), (vec!["server said 5xx".to_string()], 0));
    // Another account is its own.
    assert_eq!(fresh(&mut seen, 2, &a).0.len(), 2);
}

#[test]
fn a_clean_run_makes_a_returning_problem_new() {
    let mut seen = Vec::new();
    let a = vec!["folder gone".to_string()];
    fresh(&mut seen, 1, &a);
    forget(&mut seen, 1);
    assert_eq!(fresh(&mut seen, 1, &a).0, a);
}

#[test]
fn a_run_is_capped_and_says_how_many_more() {
    let mut seen = Vec::new();
    let (logged, more) = fresh(&mut seen, 1, &texts(MAX_LOGGED + 5));
    assert_eq!((logged.len(), more), (MAX_LOGGED, 5));
}

#[test]
fn a_text_is_cut_at_a_character_boundary_on_one_line() {
    let mut seen = Vec::new();
    let long = format!("é\n{}", "ü".repeat(MAX_CHARS * 2));
    let (logged, _) = fresh(&mut seen, 1, &[long]);
    assert_eq!(logged[0].chars().count(), MAX_CHARS);
    assert!(!logged[0].contains('\n'));
}
