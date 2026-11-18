// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Zimbra account (Z3), as the engine works out what a host should read and takes in what it
//! brings back, with results made by hand. (Against `mailcraft-zimbra`'s stand-in: the desktop
//! app's runner.)

use mailcraft_model::sync::{FolderListing, GraphFetched, GraphFolder, GraphMessage, ServerMessage, ZimbraMessagePage};
use mailcraft_model::{Address, FlagStatus};
use serde_json::json;

use super::*;

const ME: &str = "ada@example.org";
const URL: &str = "https://zimbra.example.org";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    s.zimbra = true;
    let added = s.run("account.add", &json!({"email": ME, "kind": "zimbra", "zimbra": {"url": URL}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn folder(id: &str, parent: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), parent: parent.into(), name: name.into(), kind, ..Default::default() }
}

fn listing() -> FolderListing {
    FolderListing {
        folders: vec![
            folder("2", "", "Inbox", FolderKind::Inbox),
            folder("5", "", "Sent", FolderKind::Sent),
            folder("3", "", "Trash", FolderKind::Deleted),
            folder("300", "2", "Projects", FolderKind::Custom),
        ],
        complete: true,
    }
}

fn mail(id: &str, folder: &str, subject: &str, read: bool) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: folder.into(), read, size: 1_200, ..Default::default() },
        message_id: format!("{id}@example.org"),
        subject: subject.into(),
        from: Address::new("Bob", "bob@example.org"),
        to: vec![Address::new("", ME)],
        received: 1_700_000_000,
        sent: 1_700_000_000,
        preview: format!("Preview of {id}"),
        ..Default::default()
    }
}

fn result(account: Id, from: &str) -> ZimbraMailResult {
    ZimbraMailResult { account, email: ME.into(), url: URL.into(), from: from.into(), ..Default::default() }
}

fn apply(s: &mut Session, r: ZimbraMailResult) -> Value {
    s.run("sync.applyZimbra", &json!({"result": r})).unwrap()
}

fn subjects(s: &Session, account: Id, kind_or_name: &str) -> Vec<String> {
    let folder = s.mailbox.folders.iter().find(|f| f.account == account && f.name == kind_or_name).map(|f| f.id).unwrap();
    let mut out: Vec<String> = s.mailbox.messages.values().filter(|m| m.folder == folder).map(|m| m.subject.clone()).collect();
    out.sort();
    out
}

/// A first read: the folders, then each folder afresh, then the token in the last piece.
fn first_read(s: &mut Session, account: Id) {
    let mut listed = result(account, "");
    (listed.listing, listed.well_known, listed.partial) = (Some(listing()), vec!["2".into(), "3".into(), "5".into()], true);
    apply(s, listed);
    let mut inbox = result(account, "");
    inbox.pages = vec![ZimbraMessagePage {
        folder: "2".into(),
        messages: vec![mail("257", "2", "Hello", false), mail("258", "2", "Later", true)],
        afresh: true,
        last: true,
        ..Default::default()
    }];
    inbox.partial = true;
    apply(s, inbox);
    let mut projects = result(account, "");
    projects.pages = vec![ZimbraMessagePage {
        folder: "300".into(),
        messages: vec![mail("259", "300", "Filed", true)],
        afresh: true,
        last: true,
        ..Default::default()
    }];
    projects.partial = true;
    apply(s, projects);
    let mut last = result(account, "");
    (last.token, last.whole) = ("4711".into(), true);
    apply(s, last);
}

#[test]
fn the_request_says_where_the_server_is_and_where_its_changes_go_on_from() {
    let (mut s, account) = session();
    let request = s.run("sync.zimbraRequest", &json!({"account": account})).unwrap();
    assert_eq!((request["url"].as_str(), request["email"].as_str(), request.get("token")), (Some(URL), Some(ME), None));
    assert!(request.get("password").is_none());
    s.mailbox.account_mut(account).unwrap().zimbra.as_mut().unwrap().token = "4711".into();
    assert_eq!(s.run("sync.zimbraRequest", &json!({"account": account})).unwrap()["token"], "4711");
    // A token that is no token is not gone on from.
    s.mailbox.account_mut(account).unwrap().zimbra.as_mut().unwrap().token = "47 11".into();
    assert!(s.run("sync.zimbraRequest", &json!({"account": account})).unwrap().get("token").is_none());
    s.zimbra = false;
    assert!(s.run("sync.zimbraRequest", &json!({"account": account})).is_err());
}

#[test]
fn a_first_read_brings_the_folders_and_their_mail_and_keeps_the_token() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
    assert_eq!(subjects(&s, account, "Projects"), ["Filed"]);
    let projects = s.mailbox.folders.iter().find(|f| f.account == account && f.name == "Projects").unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert_eq!(projects.parent, Some(inbox));
    assert!(s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap().well_known);
    assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4711");
    let hello = s.mailbox.messages.values().find(|m| m.subject == "Hello").unwrap();
    assert!(!hello.read && hello.server.as_ref().is_some_and(|sv| sv.id == "257"));
}

#[test]
fn a_delta_moves_marks_and_removes_by_server_id() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let mut delta = result(account, "4711");
    delta.pages = vec![ZimbraMessagePage {
        messages: vec![mail("257", "300", "Hello", true), mail("260", "2", "Brand new", false)],
        removed: vec!["258".into()],
        last: true,
        ..Default::default()
    }];
    (delta.token, delta.whole) = ("4712".into(), true);
    let said = apply(&mut s, delta);
    assert_eq!(said["new"], 1);
    assert_eq!(subjects(&s, account, "Inbox"), ["Brand new"]);
    assert_eq!(subjects(&s, account, "Projects"), ["Filed", "Hello"]);
    let hello = s.mailbox.messages.values().find(|m| m.subject == "Hello").unwrap();
    assert!(hello.read);
    assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4712");
}

#[test]
fn a_piece_from_another_token_or_server_is_refused_and_a_cut_run_keeps_no_token() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    // Went on from a token the account isn't at.
    let mut stale = result(account, "1");
    stale.pages = vec![ZimbraMessagePage { removed: vec!["257".into()], last: true, ..Default::default() }];
    assert!(s.run("sync.applyZimbra", &json!({"result": stale})).is_err());
    let mut elsewhere = result(account, "4711");
    elsewhere.url = "https://other.example.org".into();
    assert!(s.run("sync.applyZimbra", &json!({"result": elsewhere})).is_err());
    assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
    // A run that went wrong keeps the mailbox at its token, and removes nothing.
    let mut removal = result(account, "4711");
    removal.pages = vec![ZimbraMessagePage { removed: vec!["257".into()], last: true, ..Default::default() }];
    removal.partial = true;
    apply(&mut s, removal);
    let mut failed = result(account, "4711");
    (failed.token, failed.errors) = ("4712".into(), vec!["Couldn't send and receive".into()]);
    apply(&mut s, failed);
    assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4711");
    assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
}

#[test]
fn hostile_pieces_are_held_to_their_sizes() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let mut odd = result(account, "4711");
    let mut bad = mail("bad\u{0}id", "2", "x", true);
    bad.server.folder = "2".into();
    odd.pages = vec![ZimbraMessagePage { messages: vec![bad, mail("261", "2", "Line\r\nbreak", false)], last: true, ..Default::default() }];
    (odd.token, odd.whole) = ("4712".into(), true);
    assert!(s.run("sync.applyZimbra", &json!({"result": odd.clone()})).is_err());
    // Past the model's check, a piece that had to be cut keeps no token.
    odd.pages[0].messages.remove(0);
    odd.pages[0].messages[0].server.categories = vec!["\u{7}".into()];
    apply(&mut s, odd);
    assert!(subjects(&s, account, "Inbox").contains(&"Line break".to_string()));
}

#[test]
fn messages_are_fetched_whole_by_server_id() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let hello = s.mailbox.messages.values().find(|m| m.subject == "Hello").map(|m| m.id).unwrap();
    let asked = s.run("sync.zimbraFetchRequest", &json!({"account": account, "message": hello})).unwrap();
    assert_eq!(asked["ids"], json!(["257"]));
    assert_eq!(asked["url"], URL);
    let mime =
        b"From: Bob <bob@example.org>\r\nTo: ada@example.org\r\nSubject: Hello\r\nMessage-ID: <257@example.org>\r\n\r\nThe whole text.\r\n".to_vec();
    let fetched = GraphFetchResult { account, email: ME.into(), fetched: vec![GraphFetched { id: "257".into(), mime }], ..Default::default() };
    s.run("sync.applyZimbraFetch", &json!({"result": fetched})).unwrap();
    let hello = s.mailbox.message(hello).unwrap();
    assert!(hello.body.text.contains("The whole text"));
    assert_eq!(hello.flag, FlagStatus::None);
}
