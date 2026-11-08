// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #893: an account's colour is a dot after its name in the folder pane, and a stripe down the
//! left edge of its mail's rows where the list holds more than one account's mail (a search of
//! every mailbox); a list of one folder stays as it was.

use mailcraft_engine::Session;
use mailcraft_model::FolderKind;
use serde_json::json;

use super::*;
use crate::people_view::testing;

type Window = egui_kittest::Harness<'static, MailApp>;

/// The sample mailbox with a second account, given a colour as a new account is, and one of the
/// sample's messages moved into its Inbox; the window on the first account's Inbox, the two
/// accounts and the moved message.
fn window() -> (Window, Id, Id, Id) {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(testing::now(), 0), testing::now(), 0);
    let first = s.mailbox.accounts[0].id;
    let second = s.run("account.add", &json!({"email": "me@second.example"})).unwrap()["id"].as_u64().unwrap();
    s.run("account.edit", &json!({"id": first, "color": "#2F6FD0"})).unwrap();
    let inbox = s.mailbox.folder_of_kind(first, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(second, FolderKind::Inbox).unwrap();
    let moved = s.mailbox.messages_in(inbox).map(|m| m.id).next().unwrap();
    s.run("mail.move", &json!({"id": moved, "folder": theirs})).unwrap();
    let mut harness = testing::window_on(s.mailbox, "view.mail");
    harness.state_mut().auto_sync = false;
    harness.state_mut().execute("folder.select", json!({"id": inbox})).unwrap();
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    (harness, first, second, moved)
}

/// The rows drawn with a stripe, by message.
fn striped(harness: &mut Window) -> Vec<Id> {
    let rows = harness.state_mut().session.rows();
    rows.iter().filter(|row| harness.state().hit.rect("messageAccountColor", row.id).is_some()).map(|row| row.id).collect()
}

#[test]
fn each_account_heading_wears_its_colour() {
    let (mut harness, first, second, _) = window();
    assert_eq!(harness.state().session.mailbox.account(second).unwrap().color, Some(mailcraft_model::PALETTE[0]));
    for account in [first, second] {
        let heading = harness.state().hit.rect("folderHeading", account).unwrap();
        let dot = harness.state().hit.rect("accountColor", account).unwrap_or_else(|| panic!("no dot for {account}"));
        assert!(heading.contains_rect(dot), "{dot:?} in {heading:?}");
    }
    // Taken away, the dot goes.
    harness.state_mut().execute("account.edit", json!({"id": second, "color": null})).unwrap();
    harness.run_steps(3);
    assert!(harness.state().hit.rect("accountColor", second).is_none());
}

#[test]
fn rows_wear_their_accounts_colour_only_in_a_search_of_every_mailbox() {
    let (mut harness, first, _, moved) = window();
    // One folder: the owner knows whose mail it is.
    assert!(striped(&mut harness).is_empty());
    harness.state_mut().execute("view.search", json!({"query": "e", "scope": "everywhere"})).unwrap();
    harness.run_steps(3);
    let rows = striped(&mut harness);
    assert!(rows.contains(&moved), "{rows:?}");
    let of_first = rows.iter().filter(|id| harness.state().session.mailbox.message(**id).is_some_and(|m| m.account == first)).count();
    assert!(of_first > 0, "{rows:?}");
    // The stripe is at the row's left edge, inside it.
    let row = harness.state().hit.rect("message", moved).unwrap();
    let stripe = harness.state().hit.rect("messageAccountColor", moved).unwrap();
    assert!(row.contains_rect(stripe) && stripe.min.x - row.min.x < 4.0, "{stripe:?} in {row:?}");
    // An account without a colour has no stripe on its mail.
    harness.state_mut().execute("account.edit", json!({"id": first, "color": null})).unwrap();
    harness.run_steps(3);
    assert_eq!(striped(&mut harness), [moved]);
    // A search of the mailbox in view is one account's mail.
    harness.state_mut().execute("view.search", json!({"query": "e", "scope": "mailbox"})).unwrap();
    harness.run_steps(3);
    assert!(striped(&mut harness).is_empty());
}

#[test]
fn one_account_never_mixes() {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(testing::now(), 0), testing::now(), 0);
    s.run("view.search", &json!({"query": "e", "scope": "everywhere"})).unwrap();
    assert!(!mixes_accounts(&s));
    s.run("account.add", &json!({"email": "me@second.example"})).unwrap();
    assert!(mixes_accounts(&s));
    s.run("view.clearSearch", &json!({})).unwrap();
    assert!(!mixes_accounts(&s));
}
