// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::sync_driver::*;

const P: Pacing = Pacing::TASKS;

#[test]
fn patience_doubles_and_is_capped() {
    assert_eq!(P.patience_ms(1), 30_000.0);
    assert_eq!(P.patience_ms(2), 60_000.0);
    assert_eq!(P.patience_ms(3), 120_000.0);
    assert_eq!(P.patience_ms(6), 900_000.0);
    assert_eq!(P.patience_ms(u32::MAX), 900_000.0);
    // No failures: a first wait, which nothing asks for.
    assert_eq!(P.patience_ms(0), 30_000.0);
}

#[test]
fn an_account_never_asked_for_is_due_and_then_waits_for_the_refresh_time() {
    let mut book = JobBook::default();
    assert!(book.due(&P, 1, 0, 0.0));
    book.started(1, 0.0);
    assert!(!book.due(&P, 1, 0, 1_000.0));
    assert!(book.due(&P, 1, 0, P.refresh_ms));
    // Another account is its own.
    assert!(book.due(&P, 2, 0, 1_000.0));
}

#[test]
fn a_change_goes_after_it_has_settled() {
    let mut book = JobBook::default();
    book.started(1, 0.0);
    assert!(!book.due(&P, 1, 2, 10_000.0));
    assert!(!book.due(&P, 1, 2, 12_000.0));
    assert!(book.due(&P, 1, 2, 13_000.0));
    // A further change starts the quiet time again.
    assert!(!book.due(&P, 1, 3, 14_000.0));
    assert!(book.due(&P, 1, 3, 17_000.0));
}

#[test]
fn a_failure_holds_the_job_back_until_the_patience_is_over() {
    let mut book = JobBook::default();
    book.started(1, 100_000.0);
    book.failed(1, 100_000.0);
    assert_eq!(book.failures(1), 1);
    assert!(!book.due(&P, 1, 5, 120_000.0));
    book.started(1, 0.0);
    assert!(!book.due(&P, 1, 5, 120_000.0));
    assert!(book.due(&P, 1, 5, 131_000.0));
    book.failed(1, 131_000.0);
    assert!(!book.due(&P, 1, 5, 131_000.0 + 59_000.0));
    book.worked(1);
    assert_eq!(book.failures(1), 0);
    assert!(book.due(&P, 1, 5, 200_000.0));
    book.forget_checked(1);
    assert!(book.due(&P, 1, 0, 200_001.0));
}

#[test]
fn permissions_block_as_they_did_in_the_window() {
    assert!(!Permission::blocks(None, None));
    assert!(!Permission::blocks(Some(&Permission::Given), None));
    for p in [Permission::Needed, Permission::Asking, Permission::Declined, Permission::Refused("no".into())] {
        assert!(Permission::blocks(Some(&p), Some("r")));
    }
    let over = Permission::SignInOver(fingerprint("old"));
    assert!(Permission::blocks(Some(&over), None));
    assert!(Permission::blocks(Some(&over), Some("")));
    assert!(Permission::blocks(Some(&over), Some("old")));
    assert!(!Permission::blocks(Some(&over), Some("new")));
}
