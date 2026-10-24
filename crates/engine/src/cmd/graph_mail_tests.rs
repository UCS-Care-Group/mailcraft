// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's folders, as the engine works out what goes to Graph and takes in what
//! comes back. (Against the Graph stand-in itself: `apps/mailcraft-cli/tests/graph_folders.rs`.)

use mailcraft_model::sync::FolderChangeDone;
use mailcraft_model::{Message, OAuthProvider};
use serde_json::json;

use super::*;

const ACCOUNT_EMAIL: &str = "owner@example.org";

/// A session with one Microsoft account kept in step through Graph; its id.
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": ACCOUNT_EMAIL, "kind": "microsoft"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    (s, account)
}

fn folder(id: &str, parent: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), parent: parent.into(), name: name.into(), kind, ..Default::default() }
}

/// What the server has to begin with: the special folders, and a few of the owner's.
fn server() -> Vec<GraphFolder> {
    vec![
        folder("in", "", "Inbox", FolderKind::Inbox),
        folder("dr", "", "Drafts", FolderKind::Drafts),
        folder("se", "", "Sent Items", FolderKind::Sent),
        folder("de", "", "Deleted Items", FolderKind::Deleted),
        folder("ju", "", "Junk Email", FolderKind::Junk),
        folder("ou", "", "Outbox", FolderKind::Outbox),
        folder("sa", "in", "Sales/Marketing", FolderKind::Custom),
        folder("ab", "sa", "A\\B", FolderKind::Custom),
        folder("pr", "", "Projects", FolderKind::Custom),
        folder("co", "", "Conversation History", FolderKind::Custom),
    ]
}

fn listed(folders: Vec<GraphFolder>, complete: bool) -> GraphMailResult {
    GraphMailResult { account: 0, email: ACCOUNT_EMAIL.into(), listing: Some(FolderListing { folders, complete }), ..Default::default() }
}

fn take(s: &mut Session, account: Id, mut result: GraphMailResult) -> Value {
    result.account = account;
    apply_graph_mail(s, result).unwrap()
}

fn by_server<'s>(s: &'s Session, account: Id, id: &str) -> &'s Folder {
    let found = s.mailbox.folder_by_server(account, id).unwrap();
    s.mailbox.folder(found).unwrap()
}

#[test]
fn an_account_kept_in_step_through_graph_is_made_when_asked_for() {
    let mut s = Session::empty();
    // An account that names its mail servers is what it always was.
    let imap = s
        .run(
            "account.add",
            &json!({"email": "ada@example.org", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}}),
        )
        .unwrap();
    assert_eq!(s.mailbox.account(imap["id"].as_u64().unwrap()).unwrap().kind, AccountKind::Imap);

    let (s, account) = session();
    let a = s.mailbox.account(account).unwrap();
    assert_eq!((a.kind, a.incoming.is_none(), a.outgoing.is_none()), (AccountKind::Microsoft, true, true));
    assert_eq!(a.oauth.as_ref().map(|o| o.provider), Some(OAuthProvider::Microsoft));
    // It isn't an IMAP account: the IMAP request refuses it, and the Graph one is its own.
    assert!(super::super::account::sync_request(&s, account).is_err());
    let request = graph_mail_request(&s, account).unwrap();
    assert_eq!((request.account, request.email.as_str(), request.changes.len()), (account, ACCOUNT_EMAIL, 0));
}

#[test]
fn the_listing_makes_the_tree_from_parent_ids_and_keeps_names_whole() {
    let (mut s, account) = session();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let answer = take(&mut s, account, listed(server(), true));
    assert_eq!(answer["added"], 4, "{answer}");
    // The special folders made with the account are the server's, by kind: not made twice.
    assert_eq!(s.mailbox.folder_by_server(account, "in"), Some(inbox));
    assert_eq!(s.mailbox.folders.iter().filter(|f| f.account == account && f.kind == FolderKind::Inbox).count(), 1);
    let sales = by_server(&s, account, "sa");
    assert_eq!((sales.name.as_str(), sales.parent), ("Sales/Marketing", Some(inbox)));
    let ab = by_server(&s, account, "ab");
    assert_eq!((ab.name.as_str(), ab.parent), ("A\\B", Some(sales.id)));
    assert_eq!(s.mailbox.folder_path(ab.id), "Inbox/Sales/Marketing/A\\B");
    // Outlook's order: the special folders first, then the rest by name.
    let top: Vec<&str> = s.mailbox.children(account, None).iter().map(|f| f.name.as_str()).collect();
    assert_eq!(top, ["Inbox", "Drafts", "Sent Items", "Deleted Items", "Archive", "Junk Email", "Outbox", "Conversation History", "Projects"]);
    // Taken in twice, it is the same.
    let before = s.mailbox.folders.clone();
    let again = take(&mut s, account, listed(server(), true));
    assert_eq!((again["added"].as_u64(), again["removed"].as_u64(), again["updated"].as_u64()), (Some(0), Some(0), Some(0)));
    assert_eq!(s.mailbox.folders, before);
    // An Archive the server doesn't have stays here (special folders are never removed for it).
    assert!(s.mailbox.folder_of_kind(account, FolderKind::Archive).is_some());
}

#[test]
fn renamed_moved_and_deleted_there_is_so_here_with_its_id_and_its_mail() {
    let (mut s, account) = session();
    take(&mut s, account, listed(server(), true));
    let projects = s.mailbox.folder_by_server(account, "pr").unwrap();
    let m = s.mailbox.add_message(Message { account, folder: projects, subject: "Kick-off".into(), ..Default::default() }).unwrap();
    // Renamed and moved under the Inbox in Outlook.
    let mut there = server();
    there.retain(|f| f.id != "co");
    if let Some(f) = there.iter_mut().find(|f| f.id == "pr") {
        (f.name, f.parent) = ("Projects 2026".into(), "in".into());
    }
    let answer = take(&mut s, account, listed(there.clone(), true));
    let f = s.mailbox.folder(projects).unwrap();
    assert_eq!(
        (f.name.as_str(), f.parent, f.server.as_ref().unwrap().id.as_str()),
        ("Projects 2026", s.mailbox.folder_of_kind(account, FolderKind::Inbox), "pr")
    );
    assert_eq!(s.mailbox.message(m).unwrap().folder, projects, "its mail stays with it");
    // Deleted there: gone here, with its mail (the server has none of it either).
    assert_eq!(answer["removed"], 1);
    assert!(s.mailbox.folder_by_server(account, "co").is_none());
    // Moved into Deleted Items there: so here.
    if let Some(f) = there.iter_mut().find(|f| f.id == "pr") {
        f.parent = "de".into();
    }
    take(&mut s, account, listed(there, true));
    assert_eq!(s.mailbox.folder(projects).unwrap().parent, s.mailbox.folder_of_kind(account, FolderKind::Deleted));
}

#[test]
fn a_listing_that_was_cut_short_removes_nothing() {
    let (mut s, account) = session();
    take(&mut s, account, listed(server(), true));
    let projects = s.mailbox.folder_by_server(account, "pr").unwrap();
    s.mailbox.add_message(Message { account, folder: projects, subject: "Kept".into(), ..Default::default() }).unwrap();
    let mut part = server();
    part.retain(|f| !["pr", "sa", "ab"].contains(&f.id.as_str()));
    let answer = take(&mut s, account, listed(part.clone(), false));
    assert_eq!(answer["removed"], 0);
    assert!(s.mailbox.folder(projects).is_some() && s.mailbox.folder_by_server(account, "ab").is_some());
    assert_eq!(s.mailbox.count(projects), 1);
    // No listing at all (the server was busy) removes nothing either.
    let busy = GraphMailResult { email: ACCOUNT_EMAIL.into(), busy_for: Some(30), errors: vec!["busy".into()], ..Default::default() };
    take(&mut s, account, busy);
    assert!(s.mailbox.folder(projects).is_some());
    // The whole listing does.
    let answer = take(&mut s, account, listed(part, true));
    assert_eq!(answer["removed"], 3);
    assert!(s.mailbox.folder(projects).is_none() && s.mailbox.messages.values().all(|m| m.subject != "Kept"));
}

#[test]
fn what_is_changed_here_goes_up_as_the_difference_from_the_server() {
    let (mut s, account) = session();
    take(&mut s, account, listed(server(), true));
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let projects = s.mailbox.folder_by_server(account, "pr").unwrap();
    let sales = s.mailbox.folder_by_server(account, "sa").unwrap();
    let ab = s.mailbox.folder_by_server(account, "ab").unwrap();
    // A name with a `/` is a name, for a folder of this account.
    let made = s.run("folder.new", &json!({"name": "Receipts/2026", "parent": null, "account": account})).unwrap()["id"].as_u64().unwrap();
    let under = s.run("folder.new", &json!({"name": "Q1", "parent": made})).unwrap()["id"].as_u64().unwrap();
    s.run("folder.rename", &json!({"id": projects, "name": "Projects 2026"})).unwrap();
    s.run("folder.move", &json!({"id": sales, "parent": null})).unwrap();
    s.run("folder.delete", &json!({"id": ab})).unwrap();
    assert_eq!(unpushed(&s, account), 5);
    let request = graph_mail_request(&s, account).unwrap();
    assert_eq!(
        request.changes,
        // Parents before what lies in them.
        [
            FolderChange::Move { folder: sales, id: "sa".into(), parent: String::new() },
            FolderChange::Rename { folder: projects, id: "pr".into(), name: "Projects 2026".into() },
            FolderChange::Create { folder: made, name: "Receipts/2026".into(), parent: String::new(), parent_folder: None },
            // Delete Folder is a move into Deleted Items, as Outlook does.
            FolderChange::Move { folder: ab, id: "ab".into(), parent: "de".into() },
            FolderChange::Create { folder: under, name: "Q1".into(), parent: String::new(), parent_folder: Some(made) },
        ]
    );
    // The server took them all: each is as the server has it now, and nothing is left to send.
    let done = |change: &FolderChange, folder: GraphFolder| FolderChangeDone { change: change.clone(), outcome: ChangeOutcome::Done { folder } };
    let answers = vec![
        done(&request.changes[0], folder("sa", "", "Sales/Marketing", FolderKind::Custom)),
        done(&request.changes[1], folder("pr", "", "Projects 2026", FolderKind::Custom)),
        done(&request.changes[2], folder("rc", "", "Receipts/2026", FolderKind::Custom)),
        done(&request.changes[3], folder("ab", "de", "A\\B", FolderKind::Custom)),
        done(&request.changes[4], folder("q1", "rc", "Q1", FolderKind::Custom)),
    ];
    let mut now = server();
    now.retain(|f| !["sa", "pr", "ab"].contains(&f.id.as_str()));
    now.extend(answers.iter().filter_map(|a| match &a.outcome {
        ChangeOutcome::Done { folder } => Some(folder.clone()),
        _ => None,
    }));
    let answer = take(&mut s, account, GraphMailResult { changes: answers, ..listed(now, true) });
    assert_eq!((answer["pushed"].as_u64(), answer["added"].as_u64(), answer["removed"].as_u64()), (Some(5), Some(0), Some(0)), "{answer}");
    assert_eq!(unpushed(&s, account), 0);
    assert_eq!(s.mailbox.folder(made).unwrap().server.as_ref().map(|f| f.id.as_str()), Some("rc"));
    assert_eq!(s.mailbox.folder(under).unwrap().parent, Some(made));
    assert_eq!(s.mailbox.folder(sales).unwrap().parent, None);
    assert_ne!(s.mailbox.folder(sales).unwrap().parent, Some(inbox));
    // The history stays (R3.5): undoing is another change, sent with the next run.
    assert!(s.can_undo());

    // Deleted for good from Deleted Items: the server deletes it too, from the queue.
    s.run("folder.delete", &json!({"id": ab})).unwrap();
    assert!(s.mailbox.folder(ab).is_none());
    let request = graph_mail_request(&s, account).unwrap();
    assert_eq!(request.changes, [FolderChange::Delete { id: "ab".into() }]);
    let gone = FolderChangeDone { change: request.changes[0].clone(), outcome: ChangeOutcome::Gone };
    take(&mut s, account, GraphMailResult { changes: vec![gone], email: ACCOUNT_EMAIL.into(), ..Default::default() });
    assert!(s.mailbox.queued.is_empty() && unpushed(&s, account) == 0);
}

#[test]
fn here_wins_for_what_was_changed_here_and_the_server_for_the_rest() {
    let (mut s, account) = session();
    take(&mut s, account, listed(server(), true));
    let projects = s.mailbox.folder_by_server(account, "pr").unwrap();
    s.run("folder.rename", &json!({"id": projects, "name": "Mine"})).unwrap();
    // Meanwhile Outlook moved it into the Inbox and named it otherwise.
    let mut there = server();
    if let Some(f) = there.iter_mut().find(|f| f.id == "pr") {
        (f.name, f.parent) = ("Theirs".into(), "in".into());
    }
    take(&mut s, account, listed(there, true));
    let f = s.mailbox.folder(projects).unwrap();
    assert_eq!((f.name.as_str(), f.parent), ("Mine", s.mailbox.folder_of_kind(account, FolderKind::Inbox)));
    assert_eq!(graph_mail_request(&s, account).unwrap().changes, [FolderChange::Rename { folder: projects, id: "pr".into(), name: "Mine".into() }]);
}

#[test]
fn a_change_the_server_refuses_is_put_back_and_said() {
    let (mut s, account) = session();
    take(&mut s, account, listed(server(), true));
    let projects = s.mailbox.folder_by_server(account, "pr").unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.run("folder.rename", &json!({"id": projects, "name": "Renamed"})).unwrap();
    s.run("folder.move", &json!({"id": projects, "parent": inbox})).unwrap();
    let request = graph_mail_request(&s, account).unwrap();
    let refused =
        |change: &FolderChange| FolderChangeDone { change: change.clone(), outcome: ChangeOutcome::Refused { why: "the server says no".into() } };
    let answer = take(
        &mut s,
        account,
        GraphMailResult { email: ACCOUNT_EMAIL.into(), changes: request.changes.iter().map(refused).collect(), ..Default::default() },
    );
    let f = s.mailbox.folder(projects).unwrap();
    assert_eq!((f.name.as_str(), f.parent), ("Projects", None));
    let errors: Vec<&str> = answer["errors"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(errors.len(), 2);
    assert!(errors[0].contains("Couldn't") && errors[0].contains("the server says no"), "{errors:?}");
    assert_eq!(unpushed(&s, account), 0);
}

#[test]
fn an_answer_for_another_account_is_not_taken_in() {
    let (mut s, account) = session();
    let stranger = GraphMailResult { account, email: "someone@example.org".into(), ..listed(server(), true) };
    assert!(apply_graph_mail(&mut s, stranger).is_err());
    assert!(s.mailbox.folder_by_server(account, "pr").is_none());
    // Nor one for an account that isn't kept in step through Graph.
    let local = s.mailbox.accounts.iter().find(|a| a.kind != AccountKind::Microsoft).map(|a| a.id);
    if let Some(local) = local {
        assert!(apply_graph_mail(&mut s, GraphMailResult { account: local, ..Default::default() }).is_err());
    }
}

#[test]
fn a_hostile_listing_changes_nothing_it_shouldnt() {
    let (mut s, account) = session();
    // Loops, strangers for parents, empty ids and names, a folder that is its own parent.
    let folders = vec![
        folder("x", "y", "X", FolderKind::Custom),
        folder("y", "x", "Y", FolderKind::Custom),
        folder("", "", "No id", FolderKind::Custom),
        folder("n", "", "\u{0}\u{1}", FolderKind::Custom),
        folder("self", "self", "Self", FolderKind::Custom),
        folder("x", "", "X again", FolderKind::Custom),
        folder("i2", "", "Second inbox", FolderKind::Inbox),
        folder("in", "", "Inbox", FolderKind::Inbox),
    ];
    take(&mut s, account, listed(folders, true));
    // Every folder's parents end at the top.
    for f in s.mailbox.folders.iter().filter(|f| f.account == account) {
        assert!(s.mailbox.folder_path(f.id).split('/').count() < 10, "{}", f.name);
    }
    assert!(s.mailbox.folder_by_server(account, "n").is_none());
    let x = by_server(&s, account, "x");
    assert_eq!(x.name, "X");
}

#[test]
fn a_mailbox_file_of_0_2_1_opens_as_it_was() {
    // Written by 0.2.1 itself (an IMAP account, a Microsoft account on IMAP, a folder, a
    // message), with what its Send/Receive and the Favorites' choice add as 0.2.1 writes them.
    let bytes = include_bytes!("../../testdata/mailbox-0.2.1.mailcraft");
    let dir = std::env::temp_dir().join(format!("mailcraft-r31-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let open = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        crate::io::open_path(&path)
    };
    let mb = open("mailbox.mailcraft", bytes).unwrap();
    assert_eq!(mb.accounts.len(), 2);
    assert!(mb.accounts.iter().all(|a| a.kind == AccountKind::Imap));
    // What 0.2.1 kept of IMAP is all there; nothing of Graph is made up.
    let inbox = mb.folders.iter().find(|f| f.id == 11).unwrap();
    assert_eq!((inbox.remote.as_str(), inbox.uid_validity, inbox.uid_seen, inbox.server.is_none()), ("INBOX", 7, 3, true));
    let m = mb.message(27).unwrap();
    assert_eq!((m.uid, m.synced.map(|f| f.flagged), m.server.is_none()), (3, Some(true), true));
    assert_eq!((mb.pending.len(), mb.queued.len()), (1, 0));
    assert!(mb.accounts[1].retired_favorites_choice);
    // And written again, it still says nothing of Graph.
    let written = String::from_utf8(crate::io::write_mailbox(&mb).unwrap()).unwrap();
    assert!(!written.contains("\"server\"") && !written.contains("\"queued\""), "{written}");
    // A file that holds nonsense where the new fields go still opens.
    let odd = String::from_utf8(bytes.to_vec()).unwrap().replacen("\"remote\":\"INBOX\"", "\"remote\":\"INBOX\",\"server\":42", 1).replacen(
        "\"rules\":[]",
        "\"rules\":[],\"queued\":[{\"nonsense\":1}]",
        1,
    );
    let mb = open("odd.mailcraft", odd.as_bytes()).unwrap();
    assert!(mb.folders.iter().all(|f| f.server.is_none()) && mb.queued.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The answered and forwarded marks go through the update a JMAP account's window shares (M6).
/// Graph says neither, so a mark set here on a Microsoft account (a reply sent, a message forwarded)
/// stays when a delta brings the message again.
#[test]
fn answered_and_forwarded_set_here_survive_a_delta() {
    use mailcraft_model::sync::{GraphMessage, MessagePage, ServerMessage};
    let (mut s, account) = session();
    take(&mut s, account, listed(server(), true));
    let page = |read: bool| {
        let server = ServerMessage { id: "AAkM1".into(), folder: "in".into(), read, ..Default::default() };
        let message = GraphMessage { server, subject: "Budget".into(), ..Default::default() };
        GraphMailResult {
            email: ACCOUNT_EMAIL.into(),
            pages: vec![MessagePage { folder: "in".into(), messages: vec![message], ..Default::default() }],
            ..Default::default()
        }
    };
    take(&mut s, account, page(false));
    let id = s.mailbox.messages.values().find(|m| m.subject == "Budget").unwrap().id;
    let m = s.mailbox.message_mut(id).unwrap();
    (m.answered, m.forwarded) = (true, true);
    take(&mut s, account, page(true));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!((m.answered, m.forwarded, m.read), (true, true, true));
}
