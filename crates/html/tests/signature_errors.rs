// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The errors of the signature readers are errors like the crate's others: each says in a few
//! words why (`Display`) without any of the document's text, and is a `std::error::Error`.

use mailcraft_html::compat::CheckError;
use mailcraft_html::{AuthoredError, ImageSourceError};

fn is_error<E: std::error::Error>(_: &E) {}

fn texts<E: std::error::Error>(errors: &[E]) -> Vec<String> {
    errors.iter().inspect(|e| is_error(*e)).map(ToString::to_string).collect()
}

fn assert_distinct_and_said(texts: &[String]) {
    for (i, text) in texts.iter().enumerate() {
        assert!(!text.trim().is_empty(), "an error says nothing");
        assert!(!texts.get(..i).unwrap_or_default().contains(text), "two errors say {text:?}");
    }
}

#[test]
fn every_check_error_says_why() {
    let said =
        texts(&[CheckError::OverInput, CheckError::UnclosedTag, CheckError::AttributeOverflow, CheckError::BadRange, CheckError::TooManyFindings]);
    assert_distinct_and_said(&said);
    assert_eq!(CheckError::TooManyFindings.to_string(), "the document gives more findings than are listed");
}

#[test]
fn every_authored_error_says_why() {
    let said = texts(&[
        AuthoredError::Truncated,
        AuthoredError::Unclosed,
        AuthoredError::Unsupported,
        AuthoredError::Risky,
        AuthoredError::TooLarge,
        AuthoredError::ScopeCollides,
        AuthoredError::InvalidScope,
    ]);
    assert_distinct_and_said(&said);
    assert_eq!(AuthoredError::ScopeCollides.to_string(), "the scope class is already in the signature");
}

#[test]
fn the_picture_readers_errors_are_unchanged() {
    assert_eq!(ImageSourceError::UnsupportedBase.to_string(), "a base element sets the address pictures resolve against");
}
