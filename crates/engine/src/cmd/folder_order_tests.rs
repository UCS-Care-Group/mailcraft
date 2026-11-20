// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1514: `folder.reorder` puts a folder in another place among its siblings and
//! `folder.reorderFavorite` a favourite kept in MailCraft among the others, as the drags do; the
//! order is kept here, never sent to a server, and Outlook's own Favorites stay in its order.

use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use crate::Session;
use crate::agent::{self, Access};

/// One account with Projects, Clients and Notes at its top level; their ids and the account's.
fn session() -> (Session, Id, [Id; 3]) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "a@example.org"})).unwrap()["id"].as_u64().unwrap();
    let mut new = |name: &str| s.run("folder.new", &json!({"name": name, "account": account, "parent": null})).unwrap()["id"].as_u64().unwrap();
    let ids = [new("Projects"), new("Clients"), new("Notes")];
    (s, account, ids)
}

fn run(s: &mut Session, cmd: &str, params: Value) -> Result<Value, String> {
    s.run(cmd, &params).map_err(|e| e.to_string())
}

/// The ids of an account's top-level folders as folder.list gives them, custom ones only.
fn top_custom(s: &mut Session, account: Id) -> Vec<Id> {
    let list = s.run("folder.list", &json!({})).unwrap();
    let entry = list["accounts"].as_array().unwrap().iter().find(|a| a["id"] == json!(account)).unwrap().clone();
    entry["folders"].as_array().unwrap().iter().filter(|f| f["kind"] == json!("custom")).map(|f| f["id"].as_u64().unwrap()).collect()
}

/// Notes goes first among the custom folders: the answer says the siblings' new order, and
/// folder.list follows; Undo puts it back in one step and Redo moves it again.
#[test]
fn a_folder_is_moved_and_undo_puts_it_back() {
    let (mut s, account, [projects, clients, notes]) = session();
    assert_eq!(top_custom(&mut s, account), [clients, notes, projects]);
    let answer = run(&mut s, "folder.reorder", json!({"id": notes, "before": clients})).unwrap();
    assert_eq!(answer["moved"], json!(true));
    let order: Vec<Id> = answer["order"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    // (Inbox, Drafts, Sent Items and Deleted Items lead, then the rest.)
    assert_eq!(order.first(), s.mailbox.folder_of_kind(account, FolderKind::Inbox).as_ref());
    assert_eq!(order.get(4), Some(&archive), "{order:?}");
    assert!(order.windows(2).any(|w| w == [notes, clients]));
    assert_eq!(top_custom(&mut s, account), [notes, clients, projects]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(top_custom(&mut s, account), [clients, notes, projects]);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(top_custom(&mut s, account), [notes, clients, projects]);
    // Last, without `before` or with null; where it already is, nothing moves.
    assert_eq!(run(&mut s, "folder.reorder", json!({"id": notes})).unwrap()["moved"], json!(true));
    assert_eq!(run(&mut s, "folder.reorder", json!({"id": notes, "before": null})).unwrap()["moved"], json!(false));
    assert_eq!(top_custom(&mut s, account), [clients, projects, notes]);
}

/// What can't be moved is refused, and a refusal changes nothing.
#[test]
fn what_cant_be_moved_is_refused() {
    let (mut s, account, [projects, clients, _]) = session();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let sub = run(&mut s, "folder.new", json!({"name": "Sub", "parent": projects})).unwrap()["id"].as_u64().unwrap();
    let was = s.mailbox.folders.clone();
    for params in [
        json!({"id": inbox}),
        json!({"id": projects, "before": inbox}),
        json!({"id": projects, "before": projects}),
        json!({"id": sub, "before": clients}),
        json!({"id": 999_999}),
        json!({"id": projects, "before": "first"}),
        json!({"id": "one"}),
        json!({}),
    ] {
        assert!(run(&mut s, "folder.reorder", params.clone()).is_err(), "{params}");
    }
    assert_eq!(s.mailbox.folders, was);
}

fn favorites(s: &mut Session) -> Vec<Id> {
    s.run("view.favorites", &json!({})).unwrap()["favorites"].as_array().unwrap().iter().map(|f| f["id"].as_u64().unwrap()).collect()
}

/// The favourites kept in MailCraft move among themselves; one taken off and put back goes last.
#[test]
fn a_favorite_is_moved_and_undo_puts_it_back() {
    let (mut s, account, [projects, clients, notes]) = session();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    for id in [projects, clients, notes] {
        run(&mut s, "folder.favorite", json!({"id": id, "value": true})).unwrap();
    }
    assert_eq!(favorites(&mut s), [inbox, projects, clients, notes]);
    let answer = run(&mut s, "folder.reorderFavorite", json!({"id": notes, "before": inbox})).unwrap();
    assert_eq!(answer, json!({"moved": true, "favorites": [notes, inbox, projects, clients]}));
    assert_eq!(favorites(&mut s), [notes, inbox, projects, clients]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(favorites(&mut s), [inbox, projects, clients, notes]);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(run(&mut s, "folder.reorderFavorite", json!({"id": projects, "before": clients})).unwrap()["moved"], json!(false));
    // Off and on again: last.
    run(&mut s, "folder.favorite", json!({"id": notes, "value": false})).unwrap();
    run(&mut s, "folder.favorite", json!({"id": notes, "value": true})).unwrap();
    assert_eq!(favorites(&mut s), [inbox, projects, clients, notes]);
    // Not a favourite, or not here: refused.
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    for params in
        [json!({"id": archive}), json!({"id": notes, "before": archive}), json!({"id": notes, "before": notes}), json!({"id": 999_999}), json!({})]
    {
        assert!(run(&mut s, "folder.reorderFavorite", params.clone()).is_err(), "{params}");
    }
}

/// An account whose Favorites are Outlook's keeps Outlook's order: its favourites aren't moved
/// here, nor named as the place for another.
#[test]
fn outlooks_favorites_keep_outlooks_order() {
    let mut s = Session::empty();
    let own = s.run("account.add", &json!({"email": "a@example.org"})).unwrap()["id"].as_u64().unwrap();
    let ms = s.run("account.add", &json!({"email": "m@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let (m_inbox, m_sent) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(ms, FolderKind::Sent).unwrap());
    s.mailbox.set_server_favorites(ms, Some(vec![m_sent, m_inbox]));
    let a_inbox = s.mailbox.folder_of_kind(own, FolderKind::Inbox).unwrap();
    let projects = run(&mut s, "folder.new", json!({"name": "Projects", "account": own, "parent": null})).unwrap()["id"].as_u64().unwrap();
    run(&mut s, "folder.favorite", json!({"id": projects, "value": true})).unwrap();
    assert_eq!(favorites(&mut s), [m_sent, m_inbox, a_inbox, projects]);
    assert!(run(&mut s, "folder.reorderFavorite", json!({"id": m_inbox, "before": m_sent})).is_err());
    assert!(run(&mut s, "folder.reorderFavorite", json!({"id": projects, "before": m_sent})).is_err());
    assert_eq!(run(&mut s, "folder.reorderFavorite", json!({"id": projects, "before": a_inbox})).unwrap()["moved"], json!(true));
    assert_eq!(favorites(&mut s), [m_sent, m_inbox, projects, a_inbox]);
}

/// An agent may reorder without asking, as it may move an account: nothing leaves this
/// computer, nothing is removed, and Undo puts it back.
#[test]
fn an_agent_reorders_without_asking() {
    assert_eq!(agent::access("folder.reorder"), Some(Access::Safe));
    assert_eq!(agent::access("folder.reorderFavorite"), Some(Access::Safe));
}

/// A Microsoft account's folders move too, and nothing waits to go to its server for it; Undo
/// puts them back.
#[test]
fn a_microsoft_accounts_folder_moves_here_alone() {
    let mut s = Session::empty();
    let ms = s.run("account.add", &json!({"email": "m@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let mut new = |name: &str| s.run("folder.new", &json!({"name": name, "account": ms, "parent": null})).unwrap()["id"].as_u64().unwrap();
    let (x, y) = (new("X"), new("Y"));
    let (pending, queued) = (s.mailbox.pending.clone(), s.mailbox.queued.clone());
    assert_eq!(run(&mut s, "folder.reorder", json!({"id": y, "before": x})).unwrap()["moved"], json!(true));
    assert_eq!(top_custom(&mut s, ms), [y, x]);
    assert_eq!((&s.mailbox.pending, &s.mailbox.queued), (&pending, &queued));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(top_custom(&mut s, ms), [x, y]);
}

/// A `before` past the 1000 the pane lists still lands right before it, not before the last one
/// listed.
#[test]
fn a_favorite_is_moved_before_one_past_the_cap() {
    let (mut s, account, _) = session();
    let ids: Vec<Id> = (0..1_002)
        .map(|n| s.run("folder.new", &json!({"name": format!("F{n:04}"), "account": account, "parent": null})).unwrap()["id"].as_u64().unwrap())
        .collect();
    for f in s.mailbox.folders.iter_mut().filter(|f| ids.contains(&f.id)) {
        f.favorite = true;
    }
    let (first, last) = (ids[0], ids[1_001]);
    run(&mut s, "folder.reorderFavorite", json!({"id": first, "before": last})).unwrap();
    let place = |s: &Session, id: Id| s.mailbox.folder(id).unwrap().favorite_place.unwrap();
    assert_eq!(place(&s, first) + 1, place(&s, last));
    assert!(place(&s, ids[1_000]) < place(&s, first));
}
