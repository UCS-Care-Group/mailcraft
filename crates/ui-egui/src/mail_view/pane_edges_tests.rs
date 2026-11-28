// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #2041: the pane at the window's left has one width in every module, and the dots of the
//! accounts' colours stand in one column at its right.

use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::people_view::testing;

type Window = egui_kittest::Harness<'static, MailApp>;

/// The sample mailbox with a second account, both with a colour, in a window of its own.
fn window() -> (Window, Id, Id) {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(testing::now(), 0), testing::now(), 0);
    let first = s.mailbox.accounts[0].id;
    let second = s.run("account.add", &json!({"email": "me@second.example"})).unwrap()["id"].as_u64().unwrap();
    s.run("account.edit", &json!({"id": first, "color": "#2F6FD0"})).unwrap();
    let mut harness = testing::window_on(s.mailbox, "view.mail");
    harness.state_mut().session.driver.set_auto_sync(false);
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    (harness, first, second)
}

#[test]
fn the_dots_of_the_accounts_colours_stand_in_one_column() {
    let (harness, first, second) = window();
    let x = |account| harness.state().hit.rect("accountColor", account).unwrap_or_else(|| panic!("no dot for {account}")).center().x;
    assert_eq!(x(first), x(second), "the dots follow the end of the names, not one column");
    // At the pane's right, not beside a name.
    let heading = harness.state().hit.rect("folderHeading", first).unwrap();
    assert!(heading.max.x - x(first) < 30.0, "{} from the pane's right", heading.max.x - x(first));
}

#[test]
fn the_pane_at_the_left_has_the_same_width_in_every_module() {
    let (mut harness, _, _) = window();
    harness.state_mut().ui.folder_pane_width = 300.0;
    harness.run_steps(3);
    let width = harness.state().ui.folder_pane_width;
    for module in ["view.calendar", "view.people", "view.tasks", "view.mail"] {
        harness.state_mut().execute(module, json!({})).unwrap();
        harness.run_steps(3);
        let now = harness.state().ui.folder_pane_width;
        assert!((now - width).abs() < 1.0, "{module}: the pane is {now} wide, it was {width}");
    }
}
