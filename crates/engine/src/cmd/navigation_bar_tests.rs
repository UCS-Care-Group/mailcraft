// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1174: `view.navigationBar` says where the switch between Mail, Calendar, People and Tasks
//! is: the bar down the window's left edge, or a row at the foot of the pane on the left.

use serde_json::json;

use crate::Session;

/// The command answers where the switch is now, and moves it; a value it doesn't know, or none
/// at all, is refused and leaves it where it was.
#[test]
fn the_switch_moves_between_the_bar_and_the_foot() {
    let mut s = Session::empty();
    assert!(!s.view.nav_foot, "the bar, as it has always been");
    assert_eq!(s.run("view.navigationBar", &json!({"value": "foot"})).unwrap(), json!({"value": "foot"}));
    assert!(s.view.nav_foot);
    assert_eq!(s.run("view.navigationBar", &json!({"value": "bar"})).unwrap(), json!({"value": "bar"}));
    assert!(!s.view.nav_foot);

    s.view.nav_foot = true;
    for junk in [json!({}), json!({"value": "side"}), json!({"value": ""}), json!({"value": 1}), json!({"value": null}), json!({"value": true})] {
        assert!(s.run("view.navigationBar", &junk).is_err(), "{junk}");
        assert!(s.view.nav_foot, "{junk}: it stays where it was");
    }
}

/// Settings written before this was kept start with the bar.
#[test]
fn settings_from_before_start_with_the_bar() {
    let view: crate::ViewState = serde_json::from_value(json!({})).unwrap();
    assert!(!view.nav_foot);
    let kept: crate::ViewState = serde_json::from_value(json!({"navFoot": true})).unwrap();
    assert!(kept.nav_foot, "and what was kept comes back");
}
