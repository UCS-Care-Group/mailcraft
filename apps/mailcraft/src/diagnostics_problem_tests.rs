// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Send/Receive problem's text, as the log carries it, comes out of the copied diagnostics with
//! the address, the host and the quoted name taken out.

use crate::diagnostics::{report, scrub};

#[test]
fn a_logged_problem_text_is_redacted_in_the_report() {
    let dir = std::env::temp_dir().join(format!("mailcraft-diag-problem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // The second has quotes of its own, the third an odd number of them: what scrub and redact do
    // with such text can't be what keeps the words out, so the lines are made without them
    // (`problem_log`); this checks what the pattern rules do with an ordinary one.
    let text = "Couldn't move \"Private/Taxes\" for ada@example.org on imap.example.net:993: no";
    // As the log's writer writes it (`logging.rs`).
    let line = format!("2026-11-12T10:00:00Z WARN [main] mailcraft_ui_egui: {}\n", scrub(&format!("send/receive, account 10: problem: {text}")));
    std::fs::write(dir.join("mailcraft.log"), line).unwrap();
    let out = report("0.0.0", "test", Some(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.contains("problem: Couldn't move"), "{out}");
    for leak in ["Private/Taxes", "ada@example.org", "example.org", "imap.example.net"] {
        assert!(!out.contains(leak), "{leak} in {out}");
    }
}

/// A name with a quote of its own, as the producers now write it (`'` in place of `"`), is taken
/// out whole.
#[test]
fn a_name_with_quotes_inside_is_redacted_when_they_are_turned_to_apostrophes() {
    let line = scrub("send/receive, account 10: problem: Couldn't send \"Re: 'Offer' for Bob Smith\": refused");
    let out = crate::diagnostics::redact_line(&format!("2026-11-12T10:00:00Z WARN [main] t: {line}"));
    for leak in ["Offer", "Bob", "Smith"] {
        assert!(!out.contains(leak), "{leak} in {out}");
    }
}
