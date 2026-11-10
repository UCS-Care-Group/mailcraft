// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #599: a message thinned to its preview (outside the mail to keep offline) loses what its
//! sender hid with the text it came from.

use mailcraft_model::{Body, Message};

use super::*;

#[test]
fn a_thinned_message_has_no_hidden_text() {
    let mut m = Message {
        body: Body { text: "All of it".into(), html: Some("<p>All of it</p>".into()) },
        hidden_text: "Preview: only the parts that were hidden".into(),
        ..Default::default()
    };
    assert!(mailcraft_model::search::Query::parse("hidden").matches(&m, 0));
    thin_message(&mut m, "All of".into());
    assert!(m.hidden_text.is_empty());
    assert_eq!(m.body.text, "All of");
    // Nothing of it is found any more, by search or by a rule.
    assert!(!mailcraft_model::search::Query::parse("hidden").matches(&m, 0));
    assert!(!mailcraft_model::Condition::BodyContains("hidden".into()).matches(&m));
}
