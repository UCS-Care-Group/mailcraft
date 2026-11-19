// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1655: the folder search (Move to Folder, Copy to Folder, Move Items) finds a folder from the
//! start of each word typed, in any order: "Sum 2025" finds "Summer 2025".

use crate::ribbon::FolderRow;

fn row(name: &str) -> FolderRow {
    FolderRow {
        id: mailcraft_model::Id::default(),
        name: name.to_string(),
        path: name.to_string(),
        depth: 0,
        fold: mailcraft_model::words::fold(name),
        account: String::new(),
        heading: None,
    }
}

fn found(name: &str, query: &str) -> bool {
    let wanted = mailcraft_model::words::fold(query);
    let words = mailcraft_model::words::words(query);
    wanted.is_empty() || row(name).matches(&wanted, &words)
}

#[test]
fn each_word_starts_a_word_of_the_name() {
    assert!(found("Summer 2025", "Sum 2025"));
    assert!(found("Summer 2025", "2025 sum"));
    assert!(!found("Summer 2025", "Sum 2024"));
    assert!(!found("Summer 2025", "Win 2025"));
}

#[test]
fn what_matched_before_still_matches() {
    assert!(found("Summer 2025", "mer 20"));
    assert!(found("Inbox", "BOX"));
    assert!(found("Inbox", ""));
    assert!(found("Inbox", "   "));
}

#[test]
fn accents_and_hostile_text() {
    assert!(found("Été 2025", "ét"));
    assert!(found("Été 2025", "ete 20"));
    assert!(found("Été 2025", "2025 ét"));
    assert!(!found("Été 2025", "été 2024"));
    let long = "ü ".repeat(5_000);
    assert!(!found("Été 2025", &long));
}
