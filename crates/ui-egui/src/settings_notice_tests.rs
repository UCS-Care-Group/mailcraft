// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The line that says the settings file couldn't be read.

use egui_kittest::kittest::Queryable as _;
use serde_json::json;

use super::SettingsUnread;
use crate::MailApp;
use crate::people_view::testing;

fn window(unread: Option<SettingsUnread>) -> egui_kittest::Harness<'static, MailApp> {
    let mut w = testing::window("view.mail");
    w.state_mut().settings_unread = unread;
    w.run_steps(3);
    w
}

#[test]
fn the_line_names_the_file_kept_and_goes_when_put_away() {
    let mut w = window(Some(SettingsUnread::SetAside("ui.json.damaged-2".into())));
    assert!(w.query_by_label_contains("ui.json.damaged-2").is_some(), "the line names the copy kept");
    assert_eq!(w.state_mut().execute("ui.dismissSettingsNotice", json!({})).unwrap()["dismissed"], true);
    w.run_steps(2);
    assert!(w.query_by_label_contains("ui.json.damaged").is_none());
    assert_eq!(w.state_mut().execute("ui.dismissSettingsNotice", json!({})).unwrap()["dismissed"], false);
}

#[test]
fn a_file_left_in_place_is_said_too() {
    let w = window(Some(SettingsUnread::LeftInPlace));
    assert!(w.query_by_label_contains("won't be overwritten").is_some());
}

#[test]
fn nothing_is_said_when_the_settings_were_read() {
    let w = window(None);
    assert!(w.query_by_label_contains("settings file").is_none());
}

#[test]
fn a_file_that_couldnt_be_read_is_not_said_to_be_unrenamable() {
    let w = window(Some(SettingsUnread::CouldntRead));
    assert!(w.query_by_label_contains("It was left as it is").is_some());
    assert!(w.query_by_label_contains("couldn't be renamed").is_none());
}
