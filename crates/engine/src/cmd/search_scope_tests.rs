// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #493: where a search looks: the folder in view, it and its subfolders, the mailbox in view
//! (its account), or every mailbox.

use mailcraft_model::{FolderKind, Id, Message};
use serde_json::{Value, json};

use crate::Session;

/// Two local accounts, each with its standard folders, and in the first a folder under its
/// Inbox and one under that.
struct Two {
    s: Session,
    inbox: Id,
    under: Id,
    deeper: Id,
    other_inbox: Id,
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn id_of(v: &Value) -> Id {
    v["id"].as_u64().unwrap()
}

fn two() -> Two {
    let mut s = Session::empty();
    let first = id_of(&run(&mut s, "account.add", json!({"email": "ada@example.org"})));
    let second = id_of(&run(&mut s, "account.add", json!({"email": "bob@example.net"})));
    let inbox = s.mailbox.folder_of_kind(first, FolderKind::Inbox).unwrap();
    let other_inbox = s.mailbox.folder_of_kind(second, FolderKind::Inbox).unwrap();
    let under = id_of(&run(&mut s, "folder.new", json!({"name": "Projects", "parent": inbox})));
    let deeper = id_of(&run(&mut s, "folder.new", json!({"name": "Atlas", "parent": under})));
    run(&mut s, "folder.select", json!({"id": inbox}));
    Two { s, inbox, under, deeper, other_inbox }
}

fn put(s: &mut Session, folder: Id, subject: &str) -> Id {
    let m = Message { folder, subject: subject.into(), date: 1_790_000_000, ..Default::default() };
    s.mailbox.add_message(m).unwrap()
}

/// What a search for `query` with `scope` lists, by subject, A to Z.
fn found(s: &mut Session, query: &str, scope: &str) -> Vec<String> {
    run(s, "view.search", json!({"query": query, "scope": scope}));
    let ids: Vec<Id> = s.rows().iter().flat_map(|r| r.members.clone()).collect();
    let mut subjects: Vec<String> = ids.iter().filter_map(|id| s.mailbox.message(*id)).map(|m| m.subject.clone()).collect();
    subjects.sort();
    subjects
}

#[test]
fn subfolders_finds_mail_under_the_folder_in_view_and_current_folder_does_not() {
    let mut t = two();
    let s = &mut t.s;
    put(s, t.inbox, "budget in the inbox");
    put(s, t.under, "budget in projects");
    put(s, t.deeper, "budget in atlas");
    put(s, t.other_inbox, "budget elsewhere");
    assert_eq!(found(s, "budget", "folder"), ["budget in the inbox"]);
    assert_eq!(found(s, "budget", "subfolders"), ["budget in atlas", "budget in projects", "budget in the inbox"]);
    assert_eq!(s.view.scope, crate::Scope::Subfolders);
    // From the folder under the Inbox: it and what is under it, not the Inbox above.
    run(s, "folder.select", json!({"id": t.under}));
    assert_eq!(found(s, "budget", "subfolders"), ["budget in atlas", "budget in projects"]);
    // With nothing to look for the list is the folder's own, whatever the scope says.
    assert_eq!(found(s, "", "subfolders"), ["budget in projects"]);
}

#[test]
fn current_mailbox_finds_mail_in_every_folder_of_its_account_and_none_of_another() {
    let mut t = two();
    let s = &mut t.s;
    let account = s.mailbox.folder(t.inbox).unwrap().account;
    let sent = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    let other_sent = s.mailbox.folder_of_kind(s.mailbox.folder(t.other_inbox).unwrap().account, FolderKind::Sent).unwrap();
    put(s, t.inbox, "lunch in the inbox");
    put(s, t.deeper, "lunch in atlas");
    put(s, sent, "lunch sent");
    put(s, t.other_inbox, "lunch elsewhere");
    put(s, other_sent, "lunch sent elsewhere");
    let ours = ["lunch in atlas", "lunch in the inbox", "lunch sent"];
    assert_eq!(found(s, "lunch", "mailbox"), ours);
    // From any folder of the account, the same.
    run(s, "folder.select", json!({"id": t.under}));
    assert_eq!(found(s, "lunch", "mailbox"), ours);
    // From the other account's, its own.
    run(s, "folder.select", json!({"id": t.other_inbox}));
    assert_eq!(found(s, "lunch", "mailbox"), ["lunch elsewhere", "lunch sent elsewhere"]);
    assert_eq!(found(s, "lunch", "everywhere").len(), 5);
}

#[test]
fn current_mailbox_leaves_out_deleted_items_and_junk_as_all_mailboxes_does() {
    let mut t = two();
    let s = &mut t.s;
    let account = s.mailbox.folder(t.inbox).unwrap().account;
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let junk = s.mailbox.folder_of_kind(account, FolderKind::Junk).unwrap();
    let under_deleted = id_of(&run(s, "folder.new", json!({"name": "Old", "parent": deleted})));
    put(s, t.inbox, "report kept");
    put(s, deleted, "report deleted");
    put(s, under_deleted, "report deleted long ago");
    put(s, junk, "report junk");
    assert_eq!(found(s, "report", "mailbox"), ["report kept"]);
    assert_eq!(found(s, "report", "everywhere"), ["report kept"]);
    // Looked for in Deleted Items itself, they are found.
    run(s, "folder.select", json!({"id": deleted}));
    assert_eq!(found(s, "report", "subfolders"), ["report deleted", "report deleted long ago"]);
}

#[test]
fn view_search_refuses_a_scope_it_does_not_know() {
    let mut t = two();
    let s = &mut t.s;
    run(s, "view.search", json!({"query": "budget", "scope": "mailbox"}));
    for scope in ["", "Subfolder", "account", "all", "MAILBOX ", "folders"] {
        let refused = s.run("view.search", &json!({"query": "lunch", "scope": scope}));
        assert!(refused.is_err(), "{scope:?}");
        // A refused search leaves the one before as it was.
        assert_eq!((s.view.search.as_str(), s.view.scope), ("budget", crate::Scope::Mailbox), "{scope:?}");
    }
    for (code, scope) in [
        ("folder", crate::Scope::Folder),
        ("subfolders", crate::Scope::Subfolders),
        ("mailbox", crate::Scope::Mailbox),
        ("everywhere", crate::Scope::Everywhere),
    ] {
        assert_eq!(run(s, "view.search", json!({"query": "budget", "scope": code}))["scope"], code);
        assert_eq!(s.view.scope, scope);
    }
    // Clearing the search goes back to the folder in view.
    run(s, "view.search", json!({"query": "budget", "scope": "subfolders"}));
    run(s, "view.clearSearch", json!({}));
    assert_eq!(s.view.scope, crate::Scope::Folder);
}
