// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent and the owner share one window (#620): the agent names the messages it acts on, and
//! reading doesn't move the owner's folder, search or selection.

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_engine::model::FolderKind;
use serde_json::json;

use crate::Headless;
use crate::tools::{Policy, call_tool};

fn sample() -> Session {
    Session::new(mailcraft_engine::sample::mailbox(1_791_993_600, 0))
}

fn failed(h: &mut Headless, pol: &Policy, command: &str, params: serde_json::Value) -> Option<String> {
    let r = call_tool(h, pol, "execute", &json!({"command": command, "params": params}));
    r.is_error.then(|| r.content[0]["text"].as_str().unwrap_or("").to_string())
}

#[test]
fn an_agent_names_the_messages_it_deletes() {
    let mut h = Headless::new(sample(), Class::ALL.to_vec());
    let pol = Policy { files: None, allow: Class::ALL.to_vec() };
    let ids: Vec<u64> = h.session.mailbox.messages.values().filter(|m| !m.draft).map(|m| m.id).take(2).collect();
    let (selected, named) = (ids[0], ids[1]);
    h.session.sel.messages = vec![selected];
    // Nothing named: the owner's selection is not the agent's to delete.
    for params in [json!({}), json!({"ids": []}), json!({"id": null})] {
        let e = failed(&mut h, &pol, "mail.delete", params);
        assert!(e.is_some_and(|e| e.contains("needs `ids`")));
    }
    assert!(h.session.mailbox.message(selected).is_some_and(|m| !in_bin(&h.session, m.folder)));
    // Named, it works.
    assert!(failed(&mut h, &pol, "mail.delete", json!({"ids": [named]})).is_none());
    assert!(h.session.mailbox.message(named).is_none_or(|m| in_bin(&h.session, m.folder)));
    assert!(h.session.mailbox.message(selected).is_some_and(|m| !in_bin(&h.session, m.folder)));
}

fn in_bin(s: &Session, folder: u64) -> bool {
    s.mailbox.folder(folder).is_some_and(|f| f.kind == FolderKind::Deleted)
}

#[test]
fn the_owners_delete_still_uses_the_selection() {
    let mut s = sample();
    let id = s.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    s.sel.messages = vec![id];
    s.run("mail.delete", &json!({})).unwrap();
    assert!(s.mailbox.message(id).is_none_or(|m| in_bin(&s, m.folder)));
}

#[test]
fn listing_messages_leaves_the_owners_folder_search_and_selection() {
    let mut h = Headless::new(sample(), Vec::new());
    let pol = Policy { files: None, allow: Vec::new() };
    let folder_of = |h: &Headless, kind| h.session.mailbox.folders.iter().find(|f| f.kind == kind).unwrap().id;
    let (inbox, sent) = (folder_of(&h, FolderKind::Inbox), folder_of(&h, FolderKind::Sent));
    h.session.run("folder.select", &json!({"id": inbox})).unwrap();
    h.session.run("view.search", &json!({"query": "roadmap"})).unwrap();
    let id = h.session.rows().first().map(|r| r.id).unwrap();
    h.session.run("mail.select", &json!({"id": id})).unwrap();
    let (folder, search, sel) = (h.session.view.folder, h.session.view.search.clone(), h.session.sel.clone());
    for args in [json!({"folder": sent, "query": "is:unread"}), json!({"folder": "Inbox"}), json!({"query": "atlas"}), json!({})] {
        let r = call_tool(&mut h, &pol, "list_messages", &args);
        assert!(!r.is_error, "{:?}", r.content);
        assert_eq!((h.session.view.folder, &h.session.view.search, &h.session.sel), (folder, &search, &sel), "{args}");
    }
    // And it still answers for the folder it was asked about.
    let r = call_tool(&mut h, &pol, "list_messages", &json!({"folder": "Inbox", "query": "is:unread"}));
    let text = r.content[0]["text"].as_str().unwrap_or("");
    assert!(text.contains("\"messages\""), "{text}");
}
