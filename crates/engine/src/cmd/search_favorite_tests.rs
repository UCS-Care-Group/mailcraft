// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A search saved from Advanced Search sits at the top of Favorites and comes back out.

use mailcraft_model::MAX_SEARCH_FAVORITES;
use serde_json::json;

use crate::{Session, sample};

fn session() -> Session {
    let now = mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

#[test]
fn a_saved_search_goes_to_the_front_and_comes_back() {
    let mut s = session();
    let first = s.run("search.favorite", &json!({"name": "From Ada", "query": "from:ada", "scope": "everywhere"})).unwrap();
    let second = s.run("search.favorite", &json!({"name": " Unread ", "query": "is:unread"})).unwrap();
    assert_eq!(second["scope"], "folder");
    assert_eq!(second["name"], "Unread");
    let ids: Vec<_> = s.mailbox.search_favorites.iter().map(|f| f.id).collect();
    assert_eq!(ids, vec![second["id"].as_u64().unwrap(), first["id"].as_u64().unwrap()]);

    s.run("search.unfavorite", &json!({"id": second["id"]})).unwrap();
    assert_eq!(s.mailbox.search_favorites.len(), 1);
    assert_eq!(s.mailbox.search_favorites[0].name, "From Ada");
    let padded = s.run("search.favorite", &json!({"name": "Box", "query": "from:ada", "scope": " Mailbox "})).unwrap();
    assert_eq!(padded["scope"], "mailbox");
    assert_eq!(s.mailbox.search_favorites[0].scope, "mailbox");
    assert_eq!(s.mailbox.search_favorites[0].folder, s.view.folder);
}

#[test]
fn a_saved_search_keeps_the_folder_and_is_kept_once() {
    let mut s = session();
    let inbox = s.view.folder;
    let account = s.mailbox.folder(inbox).unwrap().account;
    let sent = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Sent).unwrap();
    let saved = s.run("search.favorite", &json!({"name": "Ada in Inbox", "query": "from:ada"})).unwrap();
    assert_eq!(saved["folder"], json!(inbox));
    s.run("folder.select", &json!({"id": sent})).unwrap();
    assert_eq!(s.mailbox.search_favorites[0].folder, inbox);
    s.run("search.favorite", &json!({"name": "Ada in Sent", "query": "from:ada"})).unwrap();
    assert_eq!(s.mailbox.search_favorites[0].folder, sent);
    assert!(s.run("search.favorite", &json!({"name": "Again", "query": "from:ada"})).is_err());
    assert!(s.run("search.favorite", &json!({"name": "Empty", "query": "   "})).is_err());
    assert_eq!(s.mailbox.search_favorites.len(), 2);
}

#[test]
fn a_bad_scope_or_an_empty_name_changes_nothing() {
    let mut s = session();
    s.run("search.favorite", &json!({"name": "Ada", "query": "from:ada"})).unwrap();
    let before = s.mailbox.search_favorites.clone();
    assert!(s.run("search.favorite", &json!({"name": "No", "query": "x", "scope": "inbox"})).is_err());
    assert!(s.run("search.favorite", &json!({"name": "   ", "query": "x"})).is_err());
    assert!(s.run("search.unfavorite", &json!({"id": 0})).is_err());
    assert_eq!(s.mailbox.search_favorites, before);
}

#[test]
fn saving_undoes_and_the_fifty_first_is_refused() {
    let mut s = session();
    s.run("search.favorite", &json!({"name": "Ada", "query": "from:ada"})).unwrap();
    assert!(s.undo());
    assert!(s.mailbox.search_favorites.is_empty());

    for n in 0..MAX_SEARCH_FAVORITES {
        s.run("search.favorite", &json!({"name": format!("S{n}"), "query": format!("is:unread {n}")})).unwrap();
    }
    assert!(s.run("search.favorite", &json!({"name": "One more", "query": "x"})).is_err());
    assert_eq!(s.mailbox.search_favorites.len(), MAX_SEARCH_FAVORITES);
    assert_eq!(s.mailbox.search_favorites[0].name, format!("S{}", MAX_SEARCH_FAVORITES - 1));
}
