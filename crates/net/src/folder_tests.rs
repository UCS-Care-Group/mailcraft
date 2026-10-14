//! Folders made, renamed, moved and deleted here, pushed to the fake IMAP server
//! ([`crate::push::folders`]), and the whole list of folders a run says it read.

use mailcraft_model::sync::{FolderOp, FolderPushed, FolderState, SyncRequest, SyncResult};
use mailcraft_model::{Account, AccountKind, FolderKind, Id, Security, Server};

use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};
use crate::sync;

const PASSWORD: &str = "hunter2";
const PROJECTS: Id = 10;
const APOLLO: Id = 11;

fn message(uid: u32, subject: &str) -> FakeMessage {
    let body = format!(
        "From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\n\
         Message-ID: <{uid}.{}@example.org>\r\n\r\nThe body of {subject}.\r\n",
        subject.len()
    );
    FakeMessage { uid, flags: String::new(), internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body: body.into_bytes(), claimed_size: None }
}

fn mailbox(raw: &str, flags: &str, uid_validity: u32, messages: Vec<FakeMessage>) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity, messages, refuse_open: false, next_uid: 0 }
}

/// An Exchange-like mailbox: Deleted Items, and a folder of the owner's with one inside it.
fn server() -> FakeImap {
    Fake::imap(ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        mailboxes: vec![
            mailbox("INBOX", "\\HasNoChildren", 11, vec![message(1, "One"), message(2, "Two")]),
            mailbox("Sent Items", "\\HasNoChildren \\Sent", 21, Vec::new()),
            mailbox("Deleted Items", "\\HasChildren \\Trash", 41, Vec::new()),
            mailbox("Deleted Items/Old", "\\HasNoChildren", 42, vec![message(1, "Old one")]),
            mailbox("Projects", "\\HasChildren", 31, vec![message(1, "Project one")]),
            mailbox("Projects/Apollo", "\\HasNoChildren", 32, Vec::new()),
        ],
        ..Default::default()
    })
}

fn account(imap: &FakeImap) -> Account {
    Account {
        id: 7,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        username: "ada".into(),
        ..Default::default()
    }
}

fn folder(id: Id, remote: &str, kind: FolderKind, uid_validity: u32, known_uids: Vec<u32>) -> FolderState {
    let uid_seen = known_uids.iter().copied().max().unwrap_or(0);
    FolderState { id, remote: remote.into(), kind, uid_validity, uid_seen, known_uids, mark: Default::default() }
}

/// The folders as a mailbox that is up to date with [`server`] has them.
fn synced() -> Vec<FolderState> {
    vec![
        folder(1, "INBOX", FolderKind::Inbox, 11, vec![1, 2]),
        folder(2, "Sent Items", FolderKind::Sent, 21, vec![]),
        folder(3, "Deleted Items", FolderKind::Deleted, 41, vec![]),
        folder(4, "Deleted Items/Old", FolderKind::Custom, 42, vec![1]),
        folder(PROJECTS, "Projects", FolderKind::Custom, 31, vec![1]),
        folder(APOLLO, "Projects/Apollo", FolderKind::Custom, 32, vec![]),
    ]
}

fn pushing(imap: &FakeImap, folders: Vec<FolderState>, folder_ops: Vec<FolderOp>) -> SyncRequest {
    SyncRequest { account: account(imap), password: PASSWORD.into(), folders, folder_ops, ..Default::default() }
}

fn rename(folder: Id, from: &str, parent: Option<&str>, name: &str) -> FolderOp {
    FolderOp::Rename { folder, from: from.into(), parent: parent.map(str::to_string), name: name.into() }
}

fn done(folder: Id, from: &str, to: &str) -> FolderPushed {
    FolderPushed { folder, from: from.into(), to: to.into(), delimiter: "/".into(), refused: false }
}

fn refused(folder: Id, from: &str) -> FolderPushed {
    FolderPushed { folder, from: from.into(), to: String::new(), delimiter: "/".into(), refused: true }
}

fn names(imap: &FakeImap) -> Vec<String> {
    let mut names: Vec<String> = imap.world().mailboxes.iter().map(|m| m.raw.clone()).collect();
    names.sort();
    names
}

/// The commands that change folders, without their tags.
fn folder_commands(imap: &FakeImap) -> Vec<String> {
    let world = imap.world();
    let commands = world.log.iter().filter_map(|line| line.split_once(' ').map(|(_, command)| command.to_string()));
    commands.filter(|c| ["CREATE ", "RENAME ", "DELETE ", "SUBSCRIBE ", "UNSUBSCRIBE "].iter().any(|verb| c.starts_with(verb))).collect()
}

fn delta_id(result: &SyncResult, remote: &str) -> Option<Option<Id>> {
    result.folders.iter().find(|f| f.remote == remote).map(|f| f.id)
}

#[test]
fn a_run_says_when_it_read_the_whole_list() {
    let imap = server();
    let result = sync(&pushing(&imap, synced(), Vec::new()));
    assert_eq!(result.errors, Vec::<String>::new());
    assert!(result.listed_whole);
    assert_eq!(result.listed.len(), 6);
    // A list that can't be read says nothing about what the server has.
    imap.world().refuse.push("LIST".into());
    let result = sync(&pushing(&imap, synced(), Vec::new()));
    assert!(!result.listed_whole && result.listed.is_empty());
    // Nor does one without the Inbox: that is no whole list.
    imap.world().refuse.clear();
    imap.world().mailboxes.retain(|m| m.raw != "INBOX");
    let result = sync(&pushing(&imap, synced(), Vec::new()));
    assert!(!result.listed_whole && !result.listed.is_empty(), "{result:?}");
}

#[test]
fn a_deleted_folder_goes_into_deleted_items_on_the_server_as_outlook_does() {
    let imap = server();
    let ops = vec![rename(PROJECTS, "Projects", Some("Deleted Items"), "Projects")];
    let result = sync(&pushing(&imap, synced(), ops));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.folders_pushed, [done(PROJECTS, "Projects", "Deleted Items/Projects")]);
    assert_eq!(
        folder_commands(&imap),
        ["RENAME \"Projects\" \"Deleted Items/Projects\"", "UNSUBSCRIBE \"Projects\"", "SUBSCRIBE \"Deleted Items/Projects\""]
    );
    assert_eq!(
        names(&imap),
        ["Deleted Items", "Deleted Items/Old", "Deleted Items/Projects", "Deleted Items/Projects/Apollo", "INBOX", "Sent Items"]
    );
    // The rest of the run knows the mailboxes by their new names: their news is the same
    // folders', and nothing comes down again.
    assert_eq!(delta_id(&result, "Deleted Items/Projects"), Some(Some(PROJECTS)));
    assert_eq!(delta_id(&result, "Deleted Items/Projects/Apollo"), Some(Some(APOLLO)));
    assert_eq!(result.new_count(), 0);
    let listed = result.listed.iter().find(|f| f.remote == "Deleted Items/Projects").unwrap();
    assert_eq!((listed.parent.as_str(), listed.name.as_str()), ("Deleted Items", "Projects"));
    assert!(result.listed_whole && result.pushed_count() == 1);
    assert_eq!(imap.world().connections, 2);
}

#[test]
fn renames_and_moves_reach_the_server_and_the_folders_inside_go_with_them() {
    let imap = server();
    let ops = vec![
        // Renamed where it is, then the folder inside it renamed: that one is found by the name it has now.
        rename(PROJECTS, "Projects", None, "Plans"),
        rename(APOLLO, "Projects/Apollo", None, "Artemis"),
    ];
    let result = sync(&pushing(&imap, synced(), ops));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.folders_pushed, [done(PROJECTS, "Projects", "Plans"), done(APOLLO, "Plans/Apollo", "Plans/Artemis")]);
    assert_eq!(names(&imap), ["Deleted Items", "Deleted Items/Old", "INBOX", "Plans", "Plans/Artemis", "Sent Items"]);
    assert_eq!((delta_id(&result, "Plans"), delta_id(&result, "Plans/Artemis")), (Some(Some(PROJECTS)), Some(Some(APOLLO))));

    // Moved into the Inbox, and back to the top.
    let imap = server();
    let result = sync(&pushing(&imap, synced(), vec![rename(PROJECTS, "Projects", Some("INBOX"), "Projects")]));
    assert_eq!(result.folders_pushed, [done(PROJECTS, "Projects", "INBOX/Projects")]);
    assert!(names(&imap).contains(&"INBOX/Projects/Apollo".to_string()));
    let mut folders = synced();
    folders.iter_mut().find(|f| f.id == PROJECTS).unwrap().remote = "INBOX/Projects".into();
    folders.iter_mut().find(|f| f.id == APOLLO).unwrap().remote = "INBOX/Projects/Apollo".into();
    let result = sync(&pushing(&imap, folders, vec![rename(PROJECTS, "INBOX/Projects", Some(""), "Projects")]));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.folders_pushed, [done(PROJECTS, "INBOX/Projects", "Projects")]);
    assert_eq!(names(&imap), ["Deleted Items", "Deleted Items/Old", "INBOX", "Projects", "Projects/Apollo", "Sent Items"]);
    assert_eq!(result.new_count(), 0);
}

#[test]
fn what_the_server_refuses_is_said_and_not_asked_for_again() {
    let imap = server();
    let ops = vec![
        // There is a folder of that name there already.
        rename(PROJECTS, "Projects", None, "Sent Items"),
        // A folder can't go into itself.
        rename(PROJECTS, "Projects", Some("Projects/Apollo"), "Projects"),
        // Gone from the server meanwhile.
        rename(APOLLO, "Nowhere", None, "Somewhere"),
    ];
    let result = sync(&pushing(&imap, synced(), ops));
    assert_eq!(result.folders_pushed, [refused(PROJECTS, "Projects"), refused(PROJECTS, "Projects"), refused(APOLLO, "Nowhere")]);
    assert_eq!(result.errors.len(), 3, "{:?}", result.errors);
    assert!(result.errors[0].contains("there is a folder of that name there already"), "{:?}", result.errors);
    assert!(folder_commands(&imap).is_empty());
    // The server saying no is told too, and the run goes on.
    imap.world().refuse.push("RENAME".into());
    let result = sync(&pushing(&imap, synced(), vec![rename(PROJECTS, "Projects", None, "Plans")]));
    assert_eq!(result.folders_pushed, [refused(PROJECTS, "Projects")]);
    assert!(result.errors[0].starts_with("Couldn't move or rename the folder \"Projects\" on the server"), "{:?}", result.errors);
    assert_eq!(delta_id(&result, "Projects"), Some(Some(PROJECTS)));
    // A lost connection leaves the change to be asked for again.
    imap.world().refuse.clear();
    imap.world().hang_up_on = Some("RENAME".into());
    let result = sync(&pushing(&imap, synced(), vec![rename(PROJECTS, "Projects", None, "Plans")]));
    assert!(result.folders_pushed.is_empty() && !result.errors.is_empty());
}

#[test]
fn only_a_folder_inside_deleted_items_is_deleted_for_good() {
    let imap = server();
    let mut folders = synced();
    folders.retain(|f| f.id != 4);
    let ops = vec![
        FolderOp::Delete { remote: "Deleted Items/Old".into() },
        // Not in Deleted Items there: it stays on the server.
        FolderOp::Delete { remote: "Projects".into() },
        FolderOp::Delete { remote: "INBOX".into() },
        // Gone already: nothing to do.
        FolderOp::Delete { remote: "Deleted Items/Gone".into() },
    ];
    let result = sync(&pushing(&imap, folders, ops));
    let pushed: Vec<(&str, bool)> = result.folders_pushed.iter().map(|f| (f.from.as_str(), f.refused)).collect();
    assert_eq!(pushed, [("Deleted Items/Old", false), ("Projects", true), ("INBOX", true), ("Deleted Items/Gone", false)]);
    assert_eq!(folder_commands(&imap), ["DELETE \"Deleted Items/Old\"", "UNSUBSCRIBE \"Deleted Items/Old\""]);
    assert_eq!(names(&imap), ["Deleted Items", "INBOX", "Projects", "Projects/Apollo", "Sent Items"]);
    assert_eq!(result.errors.len(), 2);
    assert!(result.errors.iter().all(|e| e.contains("it was deleted here only")), "{:?}", result.errors);
}

#[test]
fn a_folder_made_here_gets_its_mailbox_with_a_name_the_server_takes() {
    let imap = server();
    let mut folders = synced();
    folders.push(folder(20, "", FolderKind::Custom, 0, vec![]));
    folders.push(folder(21, "", FolderKind::Custom, 0, vec![]));
    let ops = vec![
        // No line breaks, and the character that parts the levels can't be in a name.
        FolderOp::Create { folder: 20, parent: "Projects".into(), name: "Q3 / Plän\r\nA9 LOGOUT".into() },
        FolderOp::Create { folder: 21, parent: String::new(), name: "\r\n".into() },
    ];
    let result = sync(&pushing(&imap, folders, ops));
    assert_eq!(result.folders_pushed, [done(20, "", "Projects/Q3 _ PlänA9 LOGOUT"), refused(21, "")]);
    assert_eq!(folder_commands(&imap), ["CREATE \"Projects/Q3 _ Pl&AOQ-nA9 LOGOUT\"", "SUBSCRIBE \"Projects/Q3 _ Pl&AOQ-nA9 LOGOUT\""]);
    // The new mailbox's news is the folder's.
    assert_eq!(delta_id(&result, "Projects/Q3 _ PlänA9 LOGOUT"), Some(Some(20)));
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
}
