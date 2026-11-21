// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A dialog's error is this command's refusal. An earlier command's words stay out of it, and
//! are not thrown away when the two say different things.

use mailcraft_engine::CmdError;
use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

#[test]
fn a_different_refusal_is_not_said_in_an_earlier_command_s_words() {
    let mut app = MailApp::new(Session::empty(), Services::default());
    let earlier = CmdError::Failed("there are already 50 saved searches".into());
    app.last_error = Some(earlier.clone());
    let shown = app.words_of("the name is empty");
    assert_eq!(shown, "the name is empty");
    assert_eq!(app.last_error, Some(earlier));
}

#[test]
fn the_command_that_just_failed_is_the_one_the_dialog_says() {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.last_error = Some(CmdError::Failed("there are already 50 saved searches".into()));
    let err = app.execute("no.such", json!({})).expect_err("an unknown command is refused");
    assert!(!err.contains("saved searches"), "{err}");
    let shown = app.words_of(&err);
    assert!(!shown.contains("saved searches"), "{shown}");
    assert_ne!(app.last_error.as_ref().map(|e| e.to_string()).as_deref(), Some("there are already 50 saved searches"));
}
