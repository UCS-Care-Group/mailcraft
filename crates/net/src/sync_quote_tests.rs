// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A subject or a name shown between quotes has no `"` or `\` of its own, so that a log's quote
//! scrubbing can't be thrown off by it.

use super::*;

#[test]
fn a_subject_and_a_name_have_no_quote_marks_of_their_own() {
    let message = Message { subject: "Re: \"Offer\" for Bob \\ Smith".into(), ..Default::default() };
    let subject = subject_of(&message);
    assert!(!subject.contains('"') && !subject.contains('\\'), "{subject}");
    assert_eq!(subject, "Re: 'Offer' for Bob ' Smith");
    assert_eq!(shown("Work \"Taxes\"\\x"), "Work 'Taxes''x");
}
