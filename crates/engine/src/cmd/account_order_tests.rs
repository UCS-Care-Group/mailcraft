// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #702: `account.move` puts an account in another place of the folder pane, as the drag does,
//! and every list of the accounts follows; the default account doesn't move with it.

use mailcraft_model::remote::{CalendarsDownload, RemoteCalendar};
use mailcraft_model::{CalendarOwner, Id, Rights};
use serde_json::{Value, json};

use crate::Session;
use crate::agent::{self, Access};

/// Three accounts, A, B and C, in the order they were added; their ids.
fn three() -> (Session, [Id; 3]) {
    let mut s = Session::empty();
    let mut add = |email: &str| s.run("account.add", &json!({"email": email})).unwrap()["id"].as_u64().unwrap();
    let ids = [add("a@example.org"), add("b@example.org"), add("c@example.org")];
    (s, ids)
}

fn listed(answer: &Value, key: &str) -> Vec<Id> {
    answer[key].as_array().unwrap().iter().map(|a| a["id"].as_u64().unwrap()).collect()
}

fn moved(s: &mut Session, params: Value) -> Result<Value, String> {
    s.run("account.move", &params).map_err(|e| e.to_string())
}

/// C goes second: the command says the new order, and account.list, file.account, file.info,
/// folder.list and view.favorites list the accounts so; the default account (new mail is written
/// from it) stays A.
#[test]
fn an_account_is_moved_and_the_lists_follow() {
    let (mut s, [a, b, c]) = three();
    let answer = moved(&mut s, json!({"id": c, "before": b})).unwrap();
    assert_eq!(answer, json!({"moved": true, "order": [a, c, b]}));
    assert_eq!(listed(&s.run("account.list", &json!({})).unwrap(), "accounts"), [a, c, b]);
    assert_eq!(listed(&s.run("file.account", &json!({})).unwrap(), "accounts"), [a, c, b]);
    assert_eq!(listed(&s.run("file.info", &json!({})).unwrap(), "accounts"), [a, c, b]);
    assert_eq!(listed(&s.run("folder.list", &json!({})).unwrap(), "accounts"), [a, c, b]);
    let favorites = s.run("view.favorites", &json!({})).unwrap();
    let ids: Vec<Id> = favorites["accounts"].as_array().unwrap().iter().map(|x| x["account"].as_u64().unwrap()).collect();
    assert_eq!(ids, [a, c, b]);
    assert_eq!(s.mailbox.default_account().map(|x| x.id), Some(a));
    // Last, without `before` (or with null), and to the top.
    assert_eq!(moved(&mut s, json!({"id": a})).unwrap()["order"], json!([c, b, a]));
    assert_eq!(moved(&mut s, json!({"id": b, "before": null})).unwrap()["order"], json!([c, a, b]));
    assert_eq!(moved(&mut s, json!({"id": b, "before": c})).unwrap()["order"], json!([b, c, a]));
    // Where it already is: nothing to undo.
    let answer = moved(&mut s, json!({"id": c, "before": a})).unwrap();
    assert_eq!(answer, json!({"moved": false, "order": [b, c, a]}));
}

/// Undo puts the accounts back as they were, and Redo moves them again.
#[test]
fn undo_puts_the_order_back() {
    let (mut s, [a, b, c]) = three();
    moved(&mut s, json!({"id": c, "before": a})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(listed(&s.run("account.list", &json!({})).unwrap(), "accounts"), [a, b, c]);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(listed(&s.run("account.list", &json!({})).unwrap(), "accounts"), [c, a, b]);
}

/// A shared mailbox stays under its account: it isn't moved by itself or named as the place.
/// Nor is an account put before itself, nor one that isn't here moved; a refusal changes nothing.
#[test]
fn what_cant_be_moved_is_refused() {
    let mut s = Session::empty();
    let local = s.run("account.add", &json!({"email": "a@example.org"})).unwrap()["id"].as_u64().unwrap();
    let owner = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    for params in [
        json!({"id": shared, "before": local}),
        json!({"id": local, "before": shared}),
        json!({"id": local, "before": local}),
        json!({"id": 999_999}),
        json!({"id": local, "before": 999_999}),
        json!({"id": "one"}),
        json!({"id": local, "before": "first"}),
        json!({}),
    ] {
        assert!(moved(&mut s, params.clone()).is_err(), "{params}");
    }
    assert!(s.mailbox.account_order.is_empty());
    // The account it is opened through takes it along.
    assert_eq!(moved(&mut s, json!({"id": owner, "before": local})).unwrap()["order"], json!([owner, shared, local]));
}

/// An agent may move an account without asking, as it may move a folder: nothing leaves this
/// computer, nothing is removed, and Undo puts it back.
#[test]
fn an_agent_moves_an_account_without_asking() {
    assert_eq!(agent::access("account.move"), Some(Access::Safe));
}

/// The list of calendars has each account's groups in the order the accounts are in.
#[test]
fn the_calendars_follow_the_order_of_the_accounts() {
    let mut s = Session::empty();
    let mut add = |email: &str| {
        s.run("account.add", &json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap()["id"].as_u64().unwrap()
    };
    let (ada, bo) = (add("ada@example.org"), add("bo@example.org"));
    for (account, email) in [(ada, "ada@example.org"), (bo, "bo@example.org")] {
        let main = RemoteCalendar {
            id: format!("c-{email}"),
            name: "Calendar".into(),
            group: "My Calendars".into(),
            rights: Rights::Edit,
            owner: Some(CalendarOwner { address: email.into(), name: String::new() }),
            main: true,
            ..Default::default()
        };
        let download = CalendarsDownload { account, email: email.into(), complete: true, calendars: vec![main], ..Default::default() };
        crate::cmd::remote::apply_calendars(&mut s, &download).unwrap();
    }
    let titles = |s: &Session| -> Vec<String> {
        crate::cmd::calendar_settings::calendar_groups(s).into_iter().filter(|g| g.account != 0).map(|g| g.title).collect()
    };
    assert_eq!(titles(&s), ["My Calendars (ada@example.org)", "My Calendars (bo@example.org)"]);
    moved(&mut s, json!({"id": bo, "before": ada})).unwrap();
    assert_eq!(titles(&s), ["My Calendars (bo@example.org)", "My Calendars (ada@example.org)"]);
}

/// The list of calendars has the groups of at most 50 accounts: those of the first 50 that keep a
/// calendar. Work accounts with many shared mailboxes that keep none (60 in all), with an account
/// that keeps one moved below them, don't push that account's calendars out of the list.
#[test]
fn accounts_without_calendars_dont_push_others_out_of_the_list() {
    let mut s = Session::empty();
    let email = "ada@example.org";
    let ada = s.run("account.add", &json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap()["id"]
        .as_u64()
        .unwrap();
    for w in 0..3 {
        let work = s.run("account.add", &json!({"email": format!("work{w}@example.org"), "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        for n in 0..20 {
            s.mailbox.add_shared_mailbox(work, &format!("team{w}-{n}@example.org")).unwrap();
        }
    }
    let main = RemoteCalendar {
        id: "c-ada".into(),
        name: "Calendar".into(),
        group: "My Calendars".into(),
        rights: Rights::Edit,
        owner: Some(CalendarOwner { address: email.into(), name: String::new() }),
        main: true,
        ..Default::default()
    };
    let download = CalendarsDownload { account: ada, email: email.into(), complete: true, calendars: vec![main], ..Default::default() };
    crate::cmd::remote::apply_calendars(&mut s, &download).unwrap();
    moved(&mut s, json!({"id": ada})).unwrap();
    let groups = crate::cmd::calendar_settings::calendar_groups(&s);
    assert!(groups.iter().any(|g| g.account == ada), "{:?}", groups.iter().map(|g| &g.title).collect::<Vec<_>>());
}
