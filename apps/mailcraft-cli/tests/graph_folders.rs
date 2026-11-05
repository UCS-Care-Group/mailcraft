// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's folders kept in step through Microsoft Graph (R3.1), from end to end: the
//! engine's `graph_mail_request` and `apply_graph_mail` with `mailcraft_net::graph::sync_folders`
//! in between, against the strict Graph stand-in, as the command line and the desktop app run
//! them. Nothing here leaves this machine; every name and id is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_mail, graph_mail_request, unpushed};
use mailcraft_model::{FolderKind, Id, Mailbox, Message};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

/// A session with one Microsoft account kept in step through Graph,
/// and a stand-in for its server. Each test has a mailbox address of its own: the gate that
/// counts a mailbox's requests is the process's.
struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    fn new(address: &str) -> Rig {
        let server = StandIn::start().unwrap();
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let account = s.run("account.add", &json!({"email": address, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        Rig { server, s, account }
    }

    /// One run: what the engine asks for, done by the network side against the stand-in, and
    /// taken in. What taking it in answered.
    fn run(&mut self) -> Value {
        let request = graph_mail_request(&self.s, self.account).unwrap();
        let _routed = self.server.route();
        let result = mailcraft_net::graph::sync_folders(TOKEN, &request);
        apply_graph_mail(&mut self.s, result).unwrap()
    }

    fn here(&self, name: &str) -> &mailcraft_model::Folder {
        self.s.mailbox.folders.iter().find(|f| f.account == self.account && f.name == name).unwrap_or_else(|| panic!("no folder {name} here"))
    }

    fn server_id(&self, name: &str) -> String {
        self.here(name).server.as_ref().map(|s| s.id.clone()).unwrap_or_default()
    }
}

#[test]
fn folders_made_here_are_there_whole_under_their_parent() {
    let mut rig = Rig::new("made-here@example.org");
    let first = rig.run();
    assert_eq!(first["errors"], json!([]), "{first}");
    let inbox = rig.s.mailbox.folder_of_kind(rig.account, FolderKind::Inbox).unwrap();
    assert!(rig.s.mailbox.folder(inbox).unwrap().server.is_some(), "the Inbox is the server's");
    let sales = rig.s.run("folder.new", &json!({"name": "Sales/Marketing", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    rig.s.run("folder.new", &json!({"name": "A\\B", "parent": sales})).unwrap();
    assert_eq!(unpushed(&rig.s, rig.account), 2);
    let answer = rig.run();
    assert_eq!((answer["pushed"].as_u64(), answer["errors"].clone()), (Some(2), json!([])), "{answer}");
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    let state = rig.server.state();
    let there = state.folder_named("Sales/Marketing").unwrap();
    assert_eq!(there.parent, state.well_known("inbox"));
    assert_eq!(state.folder_named("A\\B").unwrap().parent, there.id);
    assert_eq!(rig.server_id("Sales/Marketing"), there.id);
    drop(state);
    // Nothing is made twice by the next run.
    rig.run();
    assert_eq!(rig.server.state().folders.iter().filter(|f| f.name == "Sales/Marketing").count(), 1);
    assert_eq!(rig.s.mailbox.folders.iter().filter(|f| f.name == "Sales/Marketing").count(), 1);
}

#[test]
fn folders_renamed_moved_and_deleted_there_are_so_here_with_their_id_and_mail() {
    let mut rig = Rig::new("changed-there@example.org");
    let (projects, archive_me, old) = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        (state.add_folder("Projects", Some(&inbox)), state.add_folder("Archive me", None), state.add_folder("Old", None))
    };
    rig.run();
    let here = rig.here("Projects").id;
    let kept = rig.s.mailbox.add_message(Message { account: rig.account, folder: here, subject: "Kick-off".into(), ..Default::default() }).unwrap();
    let in_old = rig
        .s
        .mailbox
        .add_message(Message { account: rig.account, folder: rig.here("Old").id, subject: "Gone with it".into(), ..Default::default() })
        .unwrap();
    {
        // Another program renames one, moves one into Deleted Items, and deletes one for good.
        let mut state = rig.server.state();
        let deleted = state.well_known("deleteditems");
        state.rename_folder(&projects, "Projects 2026");
        let top = state.well_known("msgfolderroot");
        state.move_folder(&projects, &top);
        state.move_folder(&archive_me, &deleted);
        state.remove_folder(&old);
    }
    let answer = rig.run();
    assert_eq!(answer["removed"], 1, "{answer}");
    let f = rig.s.mailbox.folder(here).unwrap();
    assert_eq!((f.name.as_str(), f.parent, f.server.as_ref().unwrap().id.as_str()), ("Projects 2026", None, projects.as_str()));
    assert_eq!(rig.s.mailbox.message(kept).unwrap().folder, here, "its mail stays with it");
    assert_eq!(rig.here("Archive me").parent, rig.s.mailbox.folder_of_kind(rig.account, FolderKind::Deleted));
    assert!(rig.s.mailbox.message(in_old).is_none() && !rig.s.mailbox.folders.iter().any(|f| f.name == "Old"));
}

#[test]
fn folders_renamed_moved_and_deleted_here_are_so_there() {
    let mut rig = Rig::new("changed-here@example.org");
    let (a, b, c) = {
        let mut state = rig.server.state();
        (state.add_folder("Alpha", None), state.add_folder("Beta", None), state.add_folder("Gamma", None))
    };
    rig.run();
    let inbox = rig.s.mailbox.folder_of_kind(rig.account, FolderKind::Inbox).unwrap();
    let (alpha, beta, gamma) = (rig.here("Alpha").id, rig.here("Beta").id, rig.here("Gamma").id);
    rig.s.run("folder.rename", &json!({"id": alpha, "name": "Alpha/One"})).unwrap();
    rig.s.run("folder.move", &json!({"id": beta, "parent": inbox})).unwrap();
    rig.s.run("folder.delete", &json!({"id": gamma})).unwrap();
    let answer = rig.run();
    assert_eq!((answer["pushed"].as_u64(), answer["errors"].clone()), (Some(3), json!([])), "{answer}");
    {
        let state = rig.server.state();
        let named = |id: &str| state.folders.iter().find(|f| f.id == id).cloned().unwrap();
        assert_eq!(named(&a).name, "Alpha/One");
        assert_eq!(named(&b).parent, state.well_known("inbox"));
        // Delete Folder: into Deleted Items, as Outlook does.
        assert_eq!(named(&c).parent, state.well_known("deleteditems"));
    }
    // And from there, for good: the server deletes it too.
    rig.s.run("folder.delete", &json!({"id": gamma})).unwrap();
    rig.run();
    assert!(!rig.server.state().folders.iter().any(|f| f.id == c));
    assert_eq!(unpushed(&rig.s, rig.account), 0);
}

#[test]
fn a_listing_cut_short_removes_nothing() {
    let mut rig = Rig::new("cut-short@example.org");
    let projects = {
        let mut state = rig.server.state();
        let projects = state.add_folder("Projects", None);
        state.add_folder("Apollo", Some(&projects));
        projects
    };
    rig.run();
    let apollo = rig.here("Apollo").id;
    rig.s.mailbox.add_message(Message { account: rig.account, folder: apollo, subject: "Launch".into(), ..Default::default() }).unwrap();
    // The server can't list what is in Projects just now.
    rig.server.state().broken_lists.push(projects);
    let answer = rig.run();
    assert_eq!((answer["complete"].as_bool(), answer["removed"].as_u64()), (Some(false), Some(0)), "{answer}");
    assert!(rig.s.mailbox.folder(apollo).is_some() && rig.s.mailbox.count(apollo) == 1);
}

#[test]
fn a_busy_server_pauses_and_the_run_resumes() {
    let mut rig = Rig::new("busy@example.org");
    rig.server.state().add_folder("Projects", None);
    {
        let mut state = rig.server.state();
        state.fail_next.push(("GET /v1.0/me/mailFolders".into(), 429, "TooManyRequests".into()));
        state.retry_after = Some(1);
    }
    let started = Instant::now();
    let answer = rig.run();
    assert!(started.elapsed() >= Duration::from_millis(950), "the server's second was waited out");
    assert_eq!((answer["errors"].clone(), answer["complete"].as_bool()), (json!([]), Some(true)), "{answer}");
    assert!(rig.s.mailbox.folders.iter().any(|f| f.name == "Projects"));
    // Every request asked for immutable ids.
    assert!(rig.server.state().requests.iter().all(|asked| asked.header("prefer").contains("IdType=\"ImmutableId\"")));
}

#[test]
fn a_folder_marked_as_the_servers_own_is_left_alone_in_a_run_whose_extra_names_fail() {
    let mut rig = Rig::new("own-kept@example.org");
    let own = {
        let mut state = rig.server.state();
        let own = state.well_known("conversationhistory");
        // (Most of the server's own are hidden; this one is shown, as Exchange shows it.)
        if let Some(folder) = state.folders.iter_mut().find(|f| f.id == own) {
            folder.hidden = false;
        }
        own
    };
    // An earlier run marked it (#756).
    let first = rig.run();
    assert_eq!(first["errors"], json!([]), "{first}");
    let co = rig.here("Conversation History").id;
    assert!(rig.s.mailbox.folder(co).unwrap().server_owned());
    // A rename and a move left behind (an earlier version's, say: the commands refuse them now).
    let deleted = rig.s.mailbox.folder_of_kind(rig.account, FolderKind::Deleted);
    let folder = rig.s.mailbox.folder_mut(co).unwrap();
    folder.name = "Mine".into();
    folder.parent = deleted;
    assert!(rig.s.run("folder.rename", &json!({"id": co, "name": "Mine too"})).is_err());
    assert!(rig.s.run("folder.delete", &json!({"id": co})).is_err());
    // This run the server fails the question of which folders are its own, past the special ones:
    // the special folders' batch (asked again one at a time) and that of the others.
    for _ in 0..2 {
        rig.server.state().fail_next.push(("POST /v1.0/$batch".into(), 500, "ServiceUnavailable".into()));
    }
    let from = rig.server.state().requests.len();
    let answer = rig.run();
    assert_eq!(answer["errors"], json!([]), "{answer}");
    // Nothing but reads went to the server about it, and it is as the server has it.
    let state = rig.server.state();
    let asked = state.requests[from..].iter().filter(|a| a.method != "GET" && a.target.contains(&own)).count();
    assert_eq!(asked, 0, "no rename, move or delete was sent");
    let there = state.folders.iter().find(|f| f.id == own).cloned().unwrap();
    assert_eq!((there.name.as_str(), there.parent.as_str()), ("Conversation History", state.well_known("msgfolderroot").as_str()));
    drop(state);
    let here = rig.s.mailbox.folder(co).unwrap();
    assert!(here.server_owned(), "one run that couldn't tell doesn't clear the mark");
    assert_eq!((here.name.as_str(), here.parent), ("Conversation History", None), "the listing put it back");
    assert_eq!(unpushed(&rig.s, rig.account), 0);
}
