// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::Directory;
use mailcraft_model::remote::FavoritesOutcome;

use super::favorites::*;
use super::{FAVORITES_EVERY_MS, FAVORITES_REFUSED_MS};

#[test]
fn the_favorites_are_read_at_first_then_every_quarter_of_an_hour_and_when_the_owner_asks() {
    let mut f = FavoritesDriver::default();
    assert!(f.wanted(1, 0.0, false));
    assert!(!f.wanted(1, 1_000.0, false));
    assert!(f.wanted(1, FAVORITES_EVERY_MS + 1.0, false));
    assert!(!f.are_due(1, FAVORITES_EVERY_MS + 2.0, false));
    f.owner_asked(1);
    assert!(f.are_due(1, FAVORITES_EVERY_MS + 2.0, false));
    assert!(f.wanted(1, FAVORITES_EVERY_MS + 2.0, false));
    assert!(!f.are_due(1, FAVORITES_EVERY_MS + 3.0, false), "asking is used up by a read");
}

#[test]
fn a_server_that_said_no_is_left_alone_for_a_day_unless_the_owner_asks() {
    let mut f = FavoritesDriver::default();
    f.wanted(1, 0.0, false);
    let by = f.took_in(1, FavoritesOutcome::NotPermitted, "", Directory::Organisation, 0.0);
    assert!(f.is_uncovered(1));
    assert_eq!(f.notes.get(&1), Some(&Note::Offer), "never connected: the offer");
    assert!(by.is_none());
    assert!(!f.are_due(1, FAVORITES_EVERY_MS + 1.0, false));
    assert!(!f.are_due(1, FAVORITES_REFUSED_MS - 1.0, true), "a Send/Receive doesn't override it");
    f.owner_asked(1);
    assert!(f.are_due(1, 10.0, false));
}

#[test]
fn a_refusal_of_a_connected_account_names_who_has_to_act() {
    let mut f = FavoritesDriver::default();
    f.job_said(1, Some(true));
    assert!(!f.is_uncovered(1) && f.connected.contains(&1));
    let by = f.took_in(1, FavoritesOutcome::NotPermitted, "", Directory::Organisation, 5.0);
    assert_eq!(by.as_deref(), Some(NEEDS_APPROVAL));
    let by = f.took_in(1, FavoritesOutcome::NotPermitted, "", Directory::Personal, 5.0);
    assert_eq!(by.as_deref(), Some(REFUSED_FOR_ACCOUNT));
    let by = f.took_in(1, FavoritesOutcome::NotPermitted, REFUSED_BY_ORGANISATION, Directory::Unknown, 5.0);
    assert_eq!(by.as_deref(), Some(REFUSED_BY_ORGANISATION));
    // What the owner was told stays said when the answer has no words of its own.
    assert!(f.refused(1, NEEDS_APPROVAL, Directory::Organisation, 6.0));
    assert!(f.took_in(1, FavoritesOutcome::NotPermitted, "", Directory::Organisation, 7.0).is_none());
}

#[test]
fn a_refusal_is_for_the_pane_only_when_it_is_not_the_owners_to_change() {
    let mut f = FavoritesDriver::default();
    assert!(!f.refused(1, "The page was closed", Directory::Unknown, 1.0));
    assert!(f.notes.is_empty() && f.missing.is_empty());
    assert!(f.refused(1, NEEDS_APPROVAL, Directory::Unknown, 1.0));
    assert_eq!(f.notes.get(&1), Some(&Note::Approval));
    assert_eq!(f.directories.get(&1), Some(&Directory::Organisation), "told to find an administrator: an organisation's");
    assert_eq!(f.notes[&1].steps(), [Step::CopyMessage, Step::Help]);
    assert!(f.refused(2, REFUSED_FOR_ACCOUNT, Directory::Personal, 1.0));
    assert!(f.notes[&2].steps().is_empty() && !f.directories.contains_key(&2));
}

#[test]
fn a_failed_read_says_why_in_plain_words_and_a_read_clears_it() {
    let mut f = FavoritesDriver::default();
    f.took_in(1, FavoritesOutcome::Failed, "  ", Directory::Unknown, 0.0);
    assert_eq!(f.notes[&1], Note::Failed("Couldn't read the list".into()));
    f.took_in(1, FavoritesOutcome::Failed, &format!("a\u{7}{}", "b".repeat(100)), Directory::Unknown, 0.0);
    match &f.notes[&1] {
        Note::Failed(why) => assert!(why.chars().count() == 60 && !why.contains('\u{7}')),
        other => panic!("{other:?}"),
    }
    f.took_in(1, FavoritesOutcome::Read, "", Directory::Unknown, 0.0);
    assert!(f.notes.is_empty() && f.missing.is_empty());
}

#[test]
fn the_directory_a_sign_in_named_is_kept_for_the_run_and_forgotten_with_the_account() {
    let mut f = FavoritesDriver::default();
    f.note_directory(0, "contoso.example");
    f.note_directory(1, "");
    assert!(f.directories.is_empty());
    f.note_directory(1, Directory::PERSONAL_TENANT);
    assert_eq!(f.directories.get(&1), Some(&Directory::Personal));
    f.note_directory(2, &"a".repeat(65));
    f.signed_in(1);
    f.forget(1);
    assert!(!f.directories.contains_key(&1) && !f.connected.contains(&1));
}

#[test]
fn the_codes_and_words_of_the_notes_are_as_the_control_channel_had_them() {
    assert_eq!(
        (Note::Offer.code(), Note::Approval.code(), Note::Refused("x".into()).code(), Note::Failed("y".into()).code()),
        ("offer", "approval", "refused", "failed")
    );
    assert_eq!(Note::Refused("x".into()).text(), "x");
    assert_eq!((Step::CopyMessage.code(), Step::Help.code()), ("copyMessage", "help"));
    assert!(!Step::Help.label().is_empty() && !Step::CopyMessage.about().is_empty());
}
