// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folder Properties (#314): how many messages a folder, its subfolders and an account hold, how
//! many unread, and how much space they take, with the totals adding up, the sizes not known here
//! counted apart, and a tangled or huge folder tree walked with a bound.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{Body, Folder, FolderCounts, FolderKind, Id, Mailbox, Message, NOT_DOWNLOADED, ServerFolder};
use serde_json::{Value, json};

use super::folder::known_size;
use crate::Session;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn run(s: &mut Session, params: Value) -> Value {
    s.run("folder.properties", &params).unwrap_or_else(|e| panic!("folder.properties {params}: {e}"))
}

fn rows(answer: &Value) -> &Vec<Value> {
    answer["rows"].as_array().unwrap()
}

fn n(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or_else(|| panic!("{key} in {v}"))
}

/// Each row's total is what it holds itself and its subfolders' totals; the answer's total is
/// its top rows' totals.
fn assert_adds_up(answer: &Value) {
    let rows = rows(answer);
    for key in ["messages", "unread", "size", "unknownSize"] {
        for row in rows {
            let under: u64 = rows.iter().filter(|r| r["parent"] == row["id"]).map(|r| n(&r["total"], key)).sum();
            assert_eq!(n(&row["total"], key), n(row, key) + under, "{key} of {}", row["path"]);
        }
        let top: u64 = rows.iter().filter(|r| r["depth"] == 0).map(|r| n(&r["total"], key)).sum();
        assert_eq!(n(&answer["total"], key), top, "{key}");
    }
}

#[test]
fn the_sample_mailbox_adds_up_folder_by_folder_and_account_by_account() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let mut everything = 0;
    for account in s.mailbox.accounts.iter().map(|a| a.id).collect::<Vec<Id>>() {
        let answer = run(&mut s, json!({"account": account}));
        assert_eq!(answer["account"], account);
        assert_eq!(answer["folder"], Value::Null);
        assert_eq!(answer["more"], 0);
        assert_adds_up(&answer);
        // Every folder of the account, once, each with what it holds.
        let rows = rows(&answer);
        assert_eq!(rows.len(), s.mailbox.folders.iter().filter(|f| f.account == account).count());
        for row in rows {
            let id = row["id"].as_u64().unwrap();
            assert_eq!(n(row, "messages"), s.mailbox.count(id) as u64, "{}", row["path"]);
            assert_eq!(n(row, "unread"), s.mailbox.unread_count(id) as u64, "{}", row["path"]);
            let size: u64 = s.mailbox.messages_in(id).filter_map(|m| known_size(m)).sum();
            assert_eq!(n(row, "size"), size, "{}", row["path"]);
            assert_eq!(row["path"], s.mailbox.folder_path(id));
        }
        // In the pane's order: the Inbox leads, and a folder comes right before what is under it.
        if let Some(inbox) = s.mailbox.folder_of_kind(account, FolderKind::Inbox) {
            assert_eq!(rows[0]["id"], inbox);
        }
        for (i, row) in rows.iter().enumerate().skip(1) {
            if let Some(parent) = row["parent"].as_u64() {
                let at = rows.iter().position(|r| r["id"] == parent).unwrap();
                assert!(at < i && rows[at + 1..i].iter().all(|r| n(r, "depth") > n(&rows[at], "depth")), "{}", row["path"]);
            }
        }
        everything += n(&answer["total"], "messages");
    }
    assert_eq!(everything, s.mailbox.messages.len() as u64);
    assert!(everything > 0);
}

#[test]
fn a_folder_answers_for_itself_and_what_is_under_it() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let inbox = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Inbox).unwrap();
    let projects = run_cmd(&mut s, "folder.new", json!({"name": "Quarterly", "parent": inbox}));
    let apollo = run_cmd(&mut s, "folder.new", json!({"name": "Budgets", "parent": projects}));
    let first = s.mailbox.messages_in(inbox).map(|m| m.id).next().unwrap();
    s.run("mail.move", &json!({"ids": [first], "folder": apollo})).unwrap();
    s.run("folder.select", &json!({"id": projects})).unwrap();
    // Without an id, the folder in view.
    let answer = run(&mut s, json!({}));
    assert_eq!(answer["folder"], projects);
    assert_eq!(answer["name"], "Quarterly");
    let rows = rows(&answer);
    assert_eq!(rows.iter().map(|r| r["id"].as_u64().unwrap()).collect::<Vec<_>>(), [projects, apollo]);
    assert_eq!((rows[0]["depth"].clone(), rows[1]["depth"].clone()), (json!(0), json!(1)));
    assert_eq!((n(&rows[0], "messages"), n(&rows[0]["total"], "messages")), (0, 1));
    assert_eq!(n(&answer["total"], "messages"), 1);
    assert_adds_up(&answer);
    // The Inbox counts what is in its subfolders in its total, not in its own.
    let answer = run(&mut s, json!({"id": inbox}));
    assert_eq!(n(&rows_of(&answer, inbox), "messages"), s.mailbox.count(inbox) as u64);
    let under: usize = s.mailbox.subtree(inbox).iter().map(|f| s.mailbox.count(*f)).sum();
    assert!(under > s.mailbox.count(inbox));
    assert_eq!(n(&answer["total"], "messages"), under as u64);
    assert_adds_up(&answer);
}

fn run_cmd(s: &mut Session, id: &str, params: Value) -> Id {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))["id"].as_u64().unwrap()
}

fn rows_of(answer: &Value, id: Id) -> Value {
    rows(answer).iter().find(|r| r["id"] == id).cloned().unwrap_or_else(|| panic!("no row {id} in {answer}"))
}

#[test]
fn what_isnt_there_or_isnt_one_thing_is_refused() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let account = s.mailbox.accounts[0].id;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    for params in [
        json!({"id": 999_999}),
        json!({"account": 999_999}),
        json!({"id": inbox, "account": account}),
        json!({"id": "inbox"}),
        json!({"account": "first"}),
        json!({"account": -1}),
        json!({"id": [inbox]}),
    ] {
        assert!(s.run("folder.properties", &params).is_err(), "{params}");
    }
    // (A null is no name.)
    assert_eq!(run(&mut s, json!({"id": inbox, "account": null}))["folder"], inbox);
    assert_eq!(run(&mut s, json!({"id": null, "account": account}))["account"], account);
}

/// A Microsoft account's Inbox (on the server, which last counted 7 messages and 2 unread) and a
/// folder of its own (only here).
fn microsoft() -> (Session, Id, Id, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let counts = Some(FolderCounts { total: 7, unread: 2 });
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), counts, ..Default::default() });
    let here = run_cmd(&mut s, "folder.new", json!({"name": "Here", "parent": null, "account": account}));
    (s, account, inbox, here)
}

fn add(s: &mut Session, folder: Id, m: Message) -> Id {
    s.mailbox.add_message(Message { folder, received: NOW, ..m }).unwrap()
}

#[test]
fn sizes_not_known_here_are_counted_apart() {
    let (mut s, account, inbox, here) = microsoft();
    let server = |size: u64| Some(ServerMessage { id: format!("M{size}"), folder: "in".into(), size, ..Default::default() });
    let body = Body { text: "x".repeat(3_000), html: None };
    // Whole: what is here.
    let whole = add(&mut s, inbox, Message { body: body.clone(), server: server(0), read: true, ..Default::default() });
    // Only its headers here, and its size said by the server.
    add(&mut s, inbox, Message { body: Body { text: "preview".into(), html: None }, server: server(50_000), unfetched: true, ..Default::default() });
    // Only its headers, the server said no size (Graph's delta): not known.
    add(&mut s, inbox, Message { body: Body { text: "preview".into(), html: None }, server: server(0), unfetched: true, ..Default::default() });
    // Gone from the server before it came: not known either.
    add(&mut s, inbox, Message { body: Body { text: "preview".into(), html: None }, server: server(0), incomplete: true, ..Default::default() });
    // An IMAP server's message too large to download: the size its mark gives isn't believed.
    let mark = Message { headers: vec![(NOT_DOWNLOADED.to_string(), "size=999999999".to_string())], read: true, ..Default::default() };
    add(&mut s, here, mark);
    // Whole here, and marked to be fetched again for an unreadable character (#185): the
    // server said no size, but what is here is all of it.
    let html = Body { text: "y".repeat(2_000), html: Some("<p>".repeat(1_000)) };
    let again = add(&mut s, here, Message { body: html, server: server(0), unfetched: true, read: true, ..Default::default() });
    let again_size = s.mailbox.message(again).unwrap().size();
    assert_eq!(known_size(s.mailbox.message(again).unwrap()), Some(again_size));
    let whole_size = s.mailbox.message(whole).unwrap().size();
    assert!(whole_size >= 3_000);
    let answer = run(&mut s, json!({"account": account}));
    let inbox_row = rows_of(&answer, inbox);
    assert_eq!((n(&inbox_row, "messages"), n(&inbox_row, "unread")), (4, 3));
    assert_eq!(n(&inbox_row, "size"), whole_size + 50_000);
    assert_eq!(n(&inbox_row, "unknownSize"), 2);
    // What the server last counted, beside what is here.
    assert_eq!(inbox_row["server"], json!({"messages": 7, "unread": 2}));
    let here_row = rows_of(&answer, here);
    assert_eq!((n(&here_row, "messages"), n(&here_row, "size"), n(&here_row, "unknownSize")), (2, again_size, 1));
    assert!(here_row.get("server").is_none(), "a folder only here has no server counts: {here_row}");
    assert_eq!(n(&answer["total"], "unknownSize"), 3);
    assert_eq!(n(&answer["total"], "size"), whole_size + 50_000 + again_size);
    assert_adds_up(&answer);
}

#[test]
fn a_tangled_or_huge_folder_tree_is_walked_with_a_bound() {
    let (mut s, account, inbox, _) = microsoft();
    // A chain of folders deeper than any the pane shows, a message at its bottom.
    let mut parent = inbox;
    let mut chain = Vec::new();
    for depth in 0..60 {
        let id = 10_000_000 + depth;
        s.mailbox.folders.push(Folder { id, account, parent: Some(parent), name: format!("Level {depth}"), ..Default::default() });
        chain.push(id);
        parent = id;
    }
    add(&mut s, parent, Message { read: true, ..Default::default() });
    // Two folders each the other's parent, which nothing reaches from the top.
    s.mailbox.folders.push(Folder { id: 20_000_001, account, parent: Some(20_000_002), name: "Loop A".into(), ..Default::default() });
    s.mailbox.folders.push(Folder { id: 20_000_002, account, parent: Some(20_000_001), name: "Loop B".into(), ..Default::default() });
    let answer = run(&mut s, json!({"id": inbox}));
    let deepest = rows(&answer).iter().map(|r| n(r, "depth")).max().unwrap();
    assert_eq!(deepest, 32);
    assert_eq!(n(&answer["total"], "messages"), 0, "the message lies deeper than the walk");
    assert_adds_up(&answer);
    // A folder named deep down is walked to where the pane stops, not 32 folders below it:
    // Level 9 lies 10 under the top, so 22 more.
    let answer = run(&mut s, json!({"id": chain[9]}));
    assert_eq!(rows(&answer).iter().map(|r| n(r, "depth")).max().unwrap(), 22);
    assert_eq!(rows(&answer)[0]["path"], s.mailbox.folder_path(chain[9]));
    assert_adds_up(&answer);
    // Named, a folder of the loop answers for itself alone: with no top above it, it lies
    // deeper than the pane goes (and the pane doesn't list it).
    let answer = run(&mut s, json!({"id": 20_000_001}));
    assert_eq!(rows(&answer).iter().map(|r| r["id"].clone()).collect::<Vec<_>>(), [json!(20_000_001)]);
    assert_adds_up(&answer);
    // An account with more folders than are answered says how many were left out.
    for i in 0..10_050 {
        s.mailbox.folders.push(Folder { id: 30_000_000 + i, account, name: format!("Folder {i:05}"), ..Default::default() });
    }
    // The last of them, past the rows answered, still counts in the total.
    add(&mut s, 30_000_000 + 10_049, Message { read: false, ..Default::default() });
    let answer = run(&mut s, json!({"account": account}));
    assert_eq!(rows(&answer).len(), 10_000);
    assert!(rows(&answer).iter().all(|r| r["id"] != 30_000_000 + 10_049));
    let all = s.mailbox.folders.iter().filter(|f| f.account == account && !f.name.starts_with("Loop")).count() as u64;
    assert_eq!(n(&answer, "more"), all - 10_000 - (60 - 32));
    assert_eq!((n(&answer["total"], "messages"), n(&answer["total"], "unread")), (1, 1));
}

#[test]
fn an_agent_may_ask_and_the_answer_changes_nothing() {
    let (mut s, account, _, _) = microsoft();
    assert_eq!(crate::agent::classify(&s, "folder.properties", &json!({"account": account})), crate::agent::Access::Safe);
    let before = s.mailbox.clone();
    run(&mut s, json!({"account": account}));
    assert_eq!(s.mailbox, before);
    assert!(!s.registry.get("folder.properties").unwrap().mutates);
}
