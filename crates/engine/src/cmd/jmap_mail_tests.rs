// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A JMAP account (M6), as the engine works out what a host should read and takes in what it
//! brings back, with results made by hand. (Against the crate's JMAP server in the process itself:
//! `apps/mailcraft-cli/tests/jmap_mail.rs`.)

use mailcraft_model::sync::{FolderListing, GraphFetched, GraphFolder, GraphMessage, JmapKnown, MessagePage, ServerMessage};
use mailcraft_model::{Address, FlagStatus, Importance, Message};
use serde_json::json;

use super::*;
use crate::cmd::account::servers_text;

const ME: &str = "ada@example.org";
const SESSION: &str = "https://jmap.example.org/jmap/session";

/// A session with one JMAP account; its id.
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION, "bearer": true}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn folder(id: &str, parent: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), parent: parent.into(), name: name.into(), kind, ..Default::default() }
}

/// The server's mailboxes: the Inbox and its fellows by role, and two of the owner's, one inside
/// the other.
fn mailboxes() -> Vec<GraphFolder> {
    vec![
        folder("M1", "", "Inbox", FolderKind::Inbox),
        folder("M2", "", "Sent", FolderKind::Sent),
        folder("M3", "", "Drafts", FolderKind::Drafts),
        folder("M4", "", "Trash", FolderKind::Deleted),
        folder("M5", "", "Projects", FolderKind::Custom),
        folder("M6", "M5", "Apollo", FolderKind::Custom),
    ]
}

/// An email as a window brings one that isn't here: what a list shows of it.
fn email(id: &str, subject: &str, received: i64) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), size: 1_200, ..Default::default() },
        message_id: format!("{id}@example.org"),
        subject: subject.into(),
        from: Address::new("Zoe Park", "zoe@example.org"),
        to: vec![Address::new("Ada", ME)],
        sent: received - 60,
        received,
        preview: format!("About {subject}"),
        ..Default::default()
    }
}

/// An email as a window brings one that is here: its marks alone.
fn marks(id: &str, read: bool, flagged: bool) -> GraphMessage {
    let flag = if flagged { FlagStatus::Flagged } else { FlagStatus::None };
    GraphMessage { server: ServerMessage { id: id.into(), read, flag, ..Default::default() }, ..Default::default() }
}

fn window(folder: &str, messages: Vec<GraphMessage>, afresh: bool) -> MessagePage {
    MessagePage { folder: folder.into(), messages, afresh, ..Default::default() }
}

/// A piece of a run of the account, more to come.
fn piece(account: Id) -> JmapMailResult {
    JmapMailResult { account, email: ME.into(), partial: true, ..Default::default() }
}

/// A run as a host hands it over: the listing, one piece for each window, and the last piece,
/// which says whether the run went well. What taking in the last piece answered.
fn run(s: &mut Session, account: Id, pages: Vec<MessagePage>, whole: bool) -> Value {
    apply_jmap_mail(s, JmapMailResult { listing: Some(FolderListing { folders: mailboxes(), complete: true }), ..piece(account) }).unwrap();
    for page in pages {
        apply_jmap_mail(s, JmapMailResult { pages: vec![page], ..piece(account) }).unwrap();
    }
    apply_jmap_mail(s, JmapMailResult { whole, partial: false, ..piece(account) }).unwrap()
}

/// The message of the account the server calls `id`.
fn here<'s>(s: &'s Session, account: Id, id: &str) -> Option<&'s Message> {
    s.mailbox.messages.values().map(|m| &**m).find(|m| m.account == account && m.server.as_ref().is_some_and(|server| server.id == id))
}

fn folder_here(s: &Session, account: Id, server: &str) -> Id {
    s.mailbox.folder_by_server(account, server).unwrap()
}

#[test]
fn a_jmap_account_is_made_with_its_server_and_no_mail_servers() {
    let (mut s, account) = session();
    let a = s.mailbox.account(account).unwrap();
    assert_eq!((a.kind, a.incoming.is_none(), a.outgoing.is_none(), a.oauth.is_none()), (AccountKind::Jmap, true, true, true));
    assert_eq!(a.jmap, Some(JmapServer { session: SESSION.into(), bearer: true }));
    // It isn't an IMAP account: no IMAP request is made of it.
    assert!(super::super::account::sync_request(&s, account).is_err());
    let listed = s.run("account.list", &json!({})).unwrap();
    let mine = listed["accounts"].as_array().unwrap().iter().find(|a| a["id"].as_u64() == Some(account)).unwrap().clone();
    assert_eq!(
        (mine["kind"].as_str(), mine["jmap"]["server"].as_str(), mine["jmap"]["bearer"].as_bool()),
        (Some("jmap"), Some("https://jmap.example.org"), Some(true))
    );
    // (The list says where it connects, not the rest of the session's address.)
    assert!(!listed.to_string().contains("/jmap/session"), "{listed}");
    // Plain http to this computer is for tests, and is taken; the secret is a password unless said.
    let local = s.run("account.add", &json!({"email": "bo@example.org", "kind": "jmap", "jmap": {"session": "http://127.0.0.1:8080/jmap"}})).unwrap();
    let local = s.mailbox.account(local["id"].as_u64().unwrap()).unwrap();
    assert_eq!(local.jmap, Some(JmapServer { session: "http://127.0.0.1:8080/jmap".into(), bearer: false }));
}

#[test]
fn a_session_address_that_cant_be_used_makes_no_account() {
    let mut s = Session::empty();
    for jmap in [
        json!({"session": "http://jmap.example.org/jmap"}),
        json!({"session": "https://ada:secret@jmap.example.org/jmap"}),
        json!({"session": "https://jmap.example.org/jmap\r\nHost: elsewhere.example"}),
        json!({"session": ""}),
        json!({}),
        json!({"session": 7}),
        json!({"session": SESSION, "bearer": "yes"}),
        json!(SESSION),
    ] {
        let why = s.run("account.add", &json!({"email": ME, "kind": "jmap", "jmap": jmap})).unwrap_err().to_string();
        assert!(why.starts_with("bad parameters: `jmap"), "{jmap}: {why}");
    }
    // Nor without one at all.
    let why = s.run("account.add", &json!({"email": ME, "kind": "jmap"})).unwrap_err().to_string();
    assert_eq!(why, "bad parameters: `jmap.session` is empty");
    assert!(s.mailbox.accounts.iter().all(|a| a.kind != AccountKind::Jmap));
    assert!(s.run("account.add", &json!({"email": ME, "kind": "carrier pigeon"})).is_err());
}

#[test]
fn an_account_doesnt_turn_into_a_jmap_account_or_out_of_one() {
    let (mut s, account) = session();
    let why = s.run("account.edit", &json!({"id": account, "kind": "imap", "incoming": {"host": "imap.example.org"}})).unwrap_err().to_string();
    assert_eq!(why, "an account becomes a JMAP account, or stops being one, only by being removed and added again");
    let imap = s.run("account.add", &json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("account.edit", &json!({"id": imap, "kind": "jmap", "jmap": {"session": SESSION}})).is_err());
    assert_eq!(s.mailbox.account(imap).map(|a| (a.kind, a.jmap.is_none())), Some((AccountKind::Imap, true)));
    // Its server is a setting like any: the address, and whether the secret is a bearer token.
    s.run("account.edit", &json!({"id": account, "jmap": {"bearer": false}})).unwrap();
    assert_eq!(s.mailbox.account(account).unwrap().jmap, Some(JmapServer { session: SESSION.into(), bearer: false }));
    assert!(s.run("account.edit", &json!({"id": account, "jmap": {"session": "http://jmap.example.org/jmap"}})).is_err());
    assert_eq!(s.mailbox.account(account).unwrap().jmap.as_ref().map(|j| j.session.as_str()), Some(SESSION));
    // Until its mail is here, it may be pointed anywhere; once it is, its session address and its
    // own stay, even for another path on the same server.
    s.run("account.edit", &json!({"id": account, "jmap": {"session": "https://mail.example.net/jmap"}})).unwrap();
    s.run("account.edit", &json!({"id": account, "jmap": {"session": SESSION}})).unwrap();
    run(&mut s, account, vec![], true);
    let why = s.run("account.edit", &json!({"id": account, "jmap": {"session": "https://mail.example.net/jmap"}})).unwrap_err().to_string();
    assert_eq!(why, "a JMAP account's server and address can't be changed once its mail is here: remove it and add the other one");
    assert!(s.run("account.edit", &json!({"id": account, "email": "bo@example.org"})).is_err());
    assert!(s.run("account.edit", &json!({"id": account, "jmap": {"session": "https://jmap.example.org/.well-known/jmap"}})).is_err());
    s.run("account.edit", &json!({"id": account, "jmap": {"session": format!(" {SESSION} "), "bearer": true}, "name": "Ada at work"})).unwrap();
    let a = s.mailbox.account(account).unwrap();
    assert_eq!((a.name.as_str(), a.jmap.as_ref().map(|j| j.session.as_str())), ("Ada at work", Some(SESSION)));
}

#[test]
fn where_a_jmap_account_connects_is_said_without_the_rest_of_its_address() {
    let (mut s, account) = session();
    let said = |s: &Session| servers_text(s.mailbox.account(account).unwrap());
    assert_eq!(said(&s), "jmap https://jmap.example.org");
    s.run("account.edit", &json!({"id": account, "jmap": {"session": "https://JMAP.Example.org:8443/jmap/session?account=ada@example.org"}}))
        .unwrap();
    assert_eq!(said(&s), "jmap https://jmap.example.org:8443");
    // What an agent that asks to add one, or to change where one connects, is asked about.
    let add = json!({"email": "bo@example.org", "kind": "jmap", "jmap": {"session": "https://mail.example.net/jmap"}});
    let asked = crate::agent::describe(&s, "account.add", &add);
    assert!(asked.contains("which connects to jmap https://mail.example.net"), "{asked}");
    let asked = crate::agent::describe(&s, "account.edit", &json!({"id": account, "jmap": {"session": "https://mail.example.net/jmap"}}));
    assert!(asked.contains("from jmap https://jmap.example.org:8443 to jmap https://mail.example.net"), "{asked}");
    // A mailbox file can say anything: one that can't be used is said so, and gets no request.
    s.mailbox.account_mut(account).unwrap().jmap = Some(JmapServer { session: "http://elsewhere.example/jmap".into(), bearer: true });
    assert_eq!(said(&s), "jmap, no server address that can be used");
    assert_eq!(jmap_mail_request(&s, account, false).unwrap_err().to_string(), format!("{ME} has no JMAP server address that can be used"));
    assert!(jmap_fetch_request(&s, account, None, 10).is_err());
    s.mailbox.account_mut(account).unwrap().jmap = None;
    assert!(jmap_mail_request(&s, account, false).is_err());
}

#[test]
fn the_first_run_lists_the_mailboxes_and_reads_every_one() {
    let (mut s, account) = session();
    let r = jmap_mail_request(&s, account, false).unwrap();
    assert_eq!((r.account, r.email.as_str(), r.session.as_str(), r.bearer), (account, ME, SESSION, true));
    assert_eq!((r.window, r.list_folders, r.scope.clone()), (500, true, JmapScope::All));
    assert!(r.known.is_empty() && r.changes.is_empty() && r.sends.is_empty() && !r.background);
    // A light run lists them too, the first time: none of the folders is the server's yet.
    assert!(jmap_mail_request(&s, account, true).unwrap().list_folders);
    // The command answers the same, with what the host says of the run.
    let answered = s.run("sync.jmapRequest", &json!({"light": true, "background": true})).unwrap();
    assert_eq!(
        (answered["account"].as_u64(), answered["background"].as_bool(), answered["listFolders"].as_bool()),
        (Some(account), Some(true), Some(true))
    );
    assert!(answered.get("sends").is_none() && answered.get("changes").is_none(), "{answered}");
    // The window is the owner's Messages to download per folder, from 1 to 500.
    for (limit, window) in [(0, 500), (7, 7), (500, 500), (1_000_000, 500)] {
        s.prefs.sync_limit = limit;
        assert_eq!(jmap_mail_request(&s, account, false).unwrap().window, window, "{limit}");
    }
}

#[test]
fn a_light_run_reads_the_inbox_and_the_folder_on_screen_and_says_what_is_here() {
    let (mut s, account) = session();
    let inbox = vec![email("E1", "One", 1_000), email("E2", "Two", 2_000), email("E3", "Three", 3_000)];
    run(&mut s, account, vec![window("M1", inbox, true), window("M6", vec![email("E9", "Launch", 500)], true)], true);
    s.view.folder = folder_here(&s, account, "M6");
    let light = jmap_mail_request(&s, account, true).unwrap();
    // After a complete listing, a light run lists nothing.
    assert!(!light.list_folders);
    assert_eq!(light.scope, JmapScope::Light { also: vec!["M6".into()] });
    // What is here of the Inbox and of Apollo, newest first.
    let newest = |mailbox: &str, ids: &[&str]| JmapKnown { mailbox: mailbox.into(), ids: ids.iter().map(|id| id.to_string()).collect() };
    assert_eq!(light.known, [newest("M1", &["E3", "E2", "E1"]), newest("M6", &["E9"])]);
    // On the Inbox, nothing besides it.
    s.view.folder = folder_here(&s, account, "M1");
    let light = jmap_mail_request(&s, account, true).unwrap();
    assert_eq!((light.scope, light.known.len()), (JmapScope::Light { also: vec![] }, 1));
    // A run of every mailbox lists them, and says what is here of each, a window of each at most.
    s.prefs.sync_limit = 2;
    let all = jmap_mail_request(&s, account, false).unwrap();
    assert!(all.list_folders);
    assert_eq!(all.known, [newest("M1", &["E3", "E2"]), newest("M6", &["E9"])]);
}

#[test]
fn a_window_brings_new_mail_unfetched_with_its_marks_and_what_was_changed_here_stays() {
    let (mut s, account) = session();
    let mut one = email("E1", "One", 1_000);
    (one.server.read, one.server.answered) = (true, true);
    let mut two = email("E2", "Two", 2_000);
    (two.server.flag, two.server.forwarded) = (FlagStatus::Flagged, true);
    run(&mut s, account, vec![window("M1", vec![one, two], true)], true);
    let inbox = folder_here(&s, account, "M1");
    let m = here(&s, account, "E1").unwrap();
    assert_eq!(
        (m.folder, m.subject.as_str(), m.from.email.as_str(), m.unfetched, m.read, m.answered),
        (inbox, "One", "zoe@example.org", true, true, true)
    );
    assert_eq!(m.body.text, "About One");
    let m = here(&s, account, "E2").unwrap();
    assert_eq!((m.flag, m.forwarded, m.read), (FlagStatus::Flagged, true, false));
    // Read here; meanwhile, elsewhere, the first is flagged and marked unread and unanswered, and the
    // second loses its flag and its forwarded mark. The read mark made here stays (it goes up once
    // changes do); the rest comes down.
    let id = m.id;
    s.mailbox.message_mut(id).unwrap().read = true;
    run(&mut s, account, vec![window("M1", vec![marks("E1", false, true), marks("E2", false, false)], true)], true);
    let m = here(&s, account, "E1").unwrap();
    assert_eq!((m.read, m.flag, m.answered, m.subject.as_str()), (false, FlagStatus::Flagged, false, "One"));
    let m = here(&s, account, "E2").unwrap();
    assert_eq!((m.read, m.flag, m.forwarded), (true, FlagStatus::None, false));
}

#[test]
fn a_complete_window_removes_what_the_server_no_longer_has_once_the_run_went_well() {
    let (mut s, account) = session();
    run(&mut s, account, vec![window("M1", vec![email("E1", "One", 1_000), email("E2", "Two", 2_000)], true)], true);
    let answer = run(&mut s, account, vec![window("M1", vec![marks("E2", false, false)], true)], true);
    assert!(here(&s, account, "E1").is_none() && here(&s, account, "E2").is_some());
    assert_eq!(answer["removedMessages"].as_u64(), Some(1));
}

#[test]
fn nothing_is_removed_for_a_window_read_in_part_or_a_run_that_went_wrong() {
    let (mut s, account) = session();
    let three = vec![email("E1", "One", 1_000), email("E2", "Two", 2_000), email("E3", "Three", 3_000)];
    run(&mut s, account, vec![window("M1", three, true)], true);
    // A window read in part (the mailbox has more than it holds): nothing goes for what it lacks.
    run(&mut s, account, vec![window("M1", vec![marks("E3", false, false)], false)], true);
    assert!(["E1", "E2", "E3"].iter().all(|id| here(&s, account, id).is_some()));
    // A complete window, in a run that told of trouble, was stopped, had the secret turned down,
    // was asked to wait, or says it is whole before its end: nothing goes either.
    let troubles = [
        JmapMailResult { errors: vec!["Couldn't read the mail in 1 mailbox: the server failed".into()], partial: false, ..piece(account) },
        JmapMailResult { stopped: true, partial: false, ..piece(account) },
        JmapMailResult { auth_refused: true, partial: false, ..piece(account) },
        JmapMailResult { busy_for: Some(30), partial: false, ..piece(account) },
        JmapMailResult { partial: true, ..piece(account) },
    ];
    for (n, trouble) in troubles.into_iter().enumerate() {
        apply_jmap_mail(&mut s, JmapMailResult { pages: vec![window("M1", vec![marks("E3", false, false)], true)], ..piece(account) }).unwrap();
        apply_jmap_mail(&mut s, JmapMailResult { whole: true, ..trouble }).unwrap();
        assert!(here(&s, account, "E1").is_some() && here(&s, account, "E2").is_some(), "trouble {n}");
    }
    // The next run that reads the Inbox whole and goes well removes them. (It begins with its
    // request: the one before never came to its end, and what it read is no part of this one.)
    s.run("sync.jmapRequest", &json!({})).unwrap();
    let answer = run(&mut s, account, vec![window("M1", vec![marks("E3", false, false)], true)], true);
    assert_eq!(answer["removedMessages"].as_u64(), Some(2));
    assert!(here(&s, account, "E1").is_none() && here(&s, account, "E3").is_some());
}

#[test]
fn a_run_removes_only_what_its_own_complete_windows_left_out() {
    let (mut s, account) = session();
    let inbox = vec![email("E1", "One", 1), email("E2", "Two", 2)];
    run(&mut s, account, vec![window("M1", inbox, true), window("M6", vec![email("E9", "Launch", 3)], true)], true);
    // A run reads Apollo whole, without E9, and then goes wrong: E9 stays.
    run(&mut s, account, vec![window("M6", vec![], true)], false);
    assert!(here(&s, account, "E9").is_some());
    // A run that goes well but reads only the Inbox: E9 still stays, since no window of this run
    // said Apollo lacks it.
    let answer = run(&mut s, account, vec![window("M1", vec![marks("E1", false, false), marks("E2", false, false)], true)], true);
    assert_eq!(answer["removedMessages"].as_u64(), Some(0));
    assert!(here(&s, account, "E9").is_some());
    // Once a window of Apollo says so again, in a run that goes well, it goes.
    let answer = run(&mut s, account, vec![window("M6", vec![], true)], true);
    assert_eq!(answer["removedMessages"].as_u64(), Some(1));
    assert!(here(&s, account, "E9").is_none() && here(&s, account, "E1").is_some());
}

#[test]
fn a_mailbox_the_server_no_longer_lists_takes_only_its_own_mail_with_it() {
    let (mut s, account) = session();
    run(&mut s, account, vec![window("M3", vec![email("D1", "Draft there", 1)], true), window("M5", vec![email("P1", "Plan", 2)], true)], true);
    let projects = folder_here(&s, account, "M5");
    let drafts = folder_here(&s, account, "M3");
    // Here only: a message imported into Projects, and one moved from Projects to the Inbox.
    let imported = s.mailbox.add_message(Message { folder: projects, subject: "Imported".into(), ..Default::default() }).unwrap();
    let moved = here(&s, account, "P1").unwrap().id;
    let inbox = folder_here(&s, account, "M1");
    s.mailbox.message_mut(moved).unwrap().folder = inbox;
    // The server no longer has Projects (with Apollo in it), nor Drafts.
    let mut fewer = mailboxes();
    fewer.retain(|f| !["M3", "M5", "M6"].contains(&f.id.as_str()));
    let answer =
        apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders: fewer, complete: true }), ..piece(account) }).unwrap();
    // Projects stays for the mail only here, as this computer's own; Drafts stays, a standard
    // folder, without what the server had in it; Apollo goes.
    let p = s.mailbox.folder(projects).unwrap();
    assert!(p.server.is_none() && s.mailbox.message(imported).is_some());
    let d = s.mailbox.folder(drafts).unwrap();
    assert_eq!((d.kind, d.server.is_none()), (FolderKind::Drafts, true));
    assert!(here(&s, account, "D1").is_none() && s.mailbox.folder_by_server(account, "M6").is_none());
    assert_eq!(answer["removedMessages"].as_u64(), Some(1));
    // What was moved out of Projects here is this computer's now, where it was put.
    let m = s.mailbox.message(moved).unwrap();
    assert!(m.server.is_none() && m.folder == inbox);
    // The next listing ties Drafts to the server's drafts again, when it has one.
    apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders: mailboxes(), complete: true }), ..piece(account) }).unwrap();
    assert_eq!(s.mailbox.folder_by_server(account, "M3"), Some(drafts));
}

#[test]
fn a_listing_cut_short_removes_nothing_and_the_next_run_lists_again() {
    let (mut s, account) = session();
    run(&mut s, account, vec![window("M6", vec![email("E9", "Launch", 500)], true)], true);
    // A listing without Apollo, cut short (a mailbox in it has an id that is none): Apollo stays, with
    // its mail, and what complete windows leave out waits for a listing that is whole.
    let mut short = mailboxes();
    short.retain(|f| f.id != "M6");
    short.push(folder("../odd", "", "Odd", FolderKind::Custom));
    apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders: short, complete: true }), ..piece(account) }).unwrap();
    apply_jmap_mail(&mut s, JmapMailResult { pages: vec![window("M6", vec![], true)], ..piece(account) }).unwrap();
    let answer = apply_jmap_mail(&mut s, JmapMailResult { whole: true, partial: false, ..piece(account) }).unwrap();
    assert_eq!(answer["removedMessages"].as_u64(), Some(0));
    assert!(s.mailbox.folder_by_server(account, "M6").is_some() && here(&s, account, "E9").is_some());
    assert!(!s.mailbox.folders.iter().any(|f| f.name == "Odd"));
    assert!(jmap_mail_request(&s, account, true).unwrap().list_folders, "the next run lists again");
    // A complete listing without Apollo: gone, with its mail.
    let mut fewer = mailboxes();
    fewer.retain(|f| f.id != "M6");
    apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders: fewer, complete: true }), ..piece(account) }).unwrap();
    assert!(s.mailbox.folder_by_server(account, "M6").is_none() && here(&s, account, "E9").is_none());
    assert!(!jmap_mail_request(&s, account, true).unwrap().list_folders);
}

#[test]
fn the_listing_makes_the_tree_and_ties_the_special_folders_to_their_mailboxes() {
    let (mut s, account) = session();
    let local_inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let answer =
        apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders: mailboxes(), complete: true }), ..piece(account) }).unwrap();
    assert_eq!(answer["complete"].as_bool(), Some(true));
    assert_eq!(folder_here(&s, account, "M1"), local_inbox);
    let apollo = s.mailbox.folder(folder_here(&s, account, "M6")).unwrap();
    assert_eq!((apollo.name.as_str(), apollo.parent), ("Apollo", Some(folder_here(&s, account, "M5"))));
    assert_eq!(s.mailbox.folder(folder_here(&s, account, "M4")).map(|f| f.kind), Some(FolderKind::Deleted));
    // The Outbox stays this program's: no server's mailbox is it.
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    assert!(s.mailbox.folder(outbox).unwrap().server.is_none());
    let mut odd = mailboxes();
    odd.push(folder("M7", "M7", "Out", FolderKind::Outbox));
    apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders: odd, complete: true }), ..piece(account) }).unwrap();
    let out = s.mailbox.folder(folder_here(&s, account, "M7")).unwrap();
    assert_eq!((out.kind, out.parent), (FolderKind::Custom, None));
    assert!(s.mailbox.folder(outbox).unwrap().server.is_none());
}

#[test]
fn what_a_host_hands_over_is_held_to_sizes_and_to_what_a_jmap_server_says() {
    let (mut s, account) = session();
    apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders: mailboxes(), complete: true }), ..piece(account) }).unwrap();
    let mut odd = email("E1", "", 1_000);
    odd.subject = format!("Hello\r\nBcc: someone@example.org {}", "x".repeat(5_000));
    odd.preview = "p ".repeat(10_000);
    odd.to = (0..2_000).map(|n| Address::new("", format!("p{n}@example.org"))).collect();
    odd.from = Address::new("\u{0}Zoe\r\n", "zoe@example.org");
    odd.server.categories = vec!["Red category".into()];
    odd.server.change_key = "CK".into();
    odd.server.importance = Importance::High;
    odd.server.flag = FlagStatus::Complete;
    odd.server.flag_due = Some(5);
    let messages = vec![odd, email("not an id", "Nope", 1), email(&"E".repeat(256), "Too long", 2)];
    let page = MessagePage {
        folder: "M1".into(),
        messages,
        afresh: true,
        link: "https://elsewhere.example/delta".into(),
        removed: vec!["E1".into()],
        ..Default::default()
    };
    apply_jmap_mail(&mut s, JmapMailResult { pages: vec![page, window("../M1", vec![email("E5", "Nowhere", 3)], true)], ..piece(account) }).unwrap();
    let m = here(&s, account, "E1").unwrap();
    assert!(m.subject.starts_with("Hello Bcc: someone@example.org x") && m.subject.chars().count() == 998, "{}", m.subject.len());
    assert!(m.body.text.chars().count() <= 256 && m.to.len() == 500 && m.from.name == "Zoe");
    assert!(m.categories.is_empty() && m.importance == Importance::Normal && m.flag == FlagStatus::None && m.flag_due.is_none());
    assert!(m.server.as_ref().is_some_and(|server| server.change_key.is_empty() && server.categories.is_empty() && !server.gone));
    assert!(s.mailbox.messages.values().all(|m| !["Nope", "Too long", "Nowhere"].contains(&m.subject.as_str())));
    // A Graph delta link is no JMAP server's: the folder keeps none.
    let inbox = s.mailbox.folder(folder_here(&s, account, "M1")).unwrap();
    assert!(inbox.server.as_ref().is_some_and(|server| server.delta.is_empty()));
    // Problems are lines, a few of them, short; a server is waited for a day at the most.
    let errors: Vec<String> = (0..100).map(|n| format!("Line {n}\r\n{}", "y".repeat(1_000))).collect();
    let answer = apply_jmap_mail(&mut s, JmapMailResult { errors, busy_for: Some(u32::MAX), partial: false, ..piece(account) }).unwrap();
    let lines = answer["errors"].as_array().unwrap();
    assert!(lines.len() <= 33 && lines.iter().all(|l| l.as_str().is_some_and(|l| !l.contains('\n') && l.chars().count() <= 400)), "{answer}");
    assert_eq!(answer["busyFor"].as_u64(), Some(86_400));
}

#[test]
fn a_piece_that_had_to_be_cut_removes_nothing() {
    let (mut s, account) = session();
    run(&mut s, account, vec![window("M1", vec![email("E1", "One", 1)], true)], true);
    // A window of more than 500: what is past them isn't taken, and the window is no complete
    // listing of the Inbox any more.
    let many: Vec<GraphMessage> = (0..501i64).map(|n| email(&format!("N{n}"), "Many", 10 + n)).collect();
    run(&mut s, account, vec![window("M1", many, true)], true);
    assert_eq!(s.mailbox.messages.values().filter(|m| m.subject == "Many").count(), 500);
    assert!(here(&s, account, "E1").is_some());
    // More windows in a piece than a run reads: the piece is no end of a run that went well, and
    // none of its windows says what the Inbox lacks, then or later.
    let pages: Vec<MessagePage> = (0..1_001).map(|_| window("M1", vec![], true)).collect();
    let answer = apply_jmap_mail(&mut s, JmapMailResult { pages, whole: true, partial: false, ..piece(account) }).unwrap();
    assert_eq!(answer["removedMessages"].as_u64(), Some(0));
    assert!(here(&s, account, "E1").is_some_and(|m| !m.server.as_ref().is_some_and(|server| server.gone)));
    assert_eq!(run(&mut s, account, vec![], true)["removedMessages"].as_u64(), Some(0));
    assert!(here(&s, account, "E1").is_some());
}

#[test]
fn what_a_window_says_of_marks_alone_keeps_what_was_known_of_the_rest() {
    let (mut s, account) = session();
    let mut files = email("E1", "Files", 1);
    files.has_attachments = true;
    run(&mut s, account, vec![window("M1", vec![files], true)], true);
    assert!(here(&s, account, "E1").is_some_and(|m| m.unfetched && m.has_attachments()));
    run(&mut s, account, vec![window("M1", vec![marks("E1", true, false)], true)], true);
    let m = here(&s, account, "E1").unwrap();
    assert!(m.read && m.has_attachments() && m.server.as_ref().is_some_and(|server| server.size == 1_200));
}

#[test]
fn an_address_that_would_be_two_people_in_a_reply_is_left_out() {
    let (mut s, account) = session();
    let mut odd = email("E1", "Odd", 1);
    odd.to = vec![Address::new("", "bo@example.org, eve@example.net"), Address::new("Ada", ME), Address::new("", "<eve@example.net>")];
    odd.from = Address::new("Zoe", "zoe@example.org; eve@example.net");
    run(&mut s, account, vec![window("M1", vec![odd], true)], true);
    let m = here(&s, account, "E1").unwrap();
    assert_eq!(m.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), [ME]);
    assert!(m.from.email.is_empty());
}

#[test]
fn marks_for_a_message_removed_here_meanwhile_make_no_message() {
    let (mut s, account) = session();
    run(&mut s, account, vec![window("M1", vec![email("E1", "One", 1)], true)], true);
    let id = here(&s, account, "E1").unwrap().id;
    // (Gone here by other means than a command, which leaves such mail where it is.)
    s.mailbox.forget_message(id).unwrap();
    run(&mut s, account, vec![window("M1", vec![marks("E1", true, false)], true)], true);
    assert!(here(&s, account, "E1").is_none());
    // The next run doesn't say it is here: its window brings it whole.
    assert!(jmap_mail_request(&s, account, false).unwrap().known.is_empty());
}

#[test]
fn an_answer_for_another_account_or_kind_is_refused_and_changes_nothing() {
    let (mut s, account) = session();
    let listing = Some(FolderListing { folders: mailboxes(), complete: true });
    let why = apply_jmap_mail(&mut s, JmapMailResult { email: "bo@example.org".into(), listing: listing.clone(), ..piece(account) }).unwrap_err();
    assert_eq!(why.to_string(), format!("this Send/Receive was for another account, not for {ME}"));
    assert!(apply_jmap_mail(&mut s, JmapMailResult { account: 999_999, ..piece(account) }).is_err());
    assert!(apply_jmap_fetch(&mut s, GraphFetchResult { account, email: "bo@example.org".into(), ..Default::default() }).is_err());
    assert!(!s.mailbox.folders.iter().any(|f| f.account == account && f.server.is_some()));
    // Nor is anything of JMAP made of, or taken in for, an account of another kind.
    let imap = s.run("account.add", &json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let why = apply_jmap_mail(&mut s, JmapMailResult { account: imap, email: String::new(), listing, ..piece(account) }).unwrap_err();
    assert_eq!(why.to_string(), "bo@example.org isn't kept in step over JMAP");
    assert!(apply_jmap_fetch(&mut s, GraphFetchResult { account: imap, ..Default::default() }).is_err());
    assert!(jmap_mail_request(&s, imap, false).is_err() && jmap_fetch_request(&s, imap, None, 10).is_err());
    assert!(!s.mailbox.folders.iter().any(|f| f.server.is_some()));
}

#[test]
fn the_status_says_how_the_run_ended() {
    let (mut s, account) = session();
    let answer = apply_jmap_mail(&mut s, JmapMailResult { auth_refused: true, partial: false, ..piece(account) }).unwrap();
    assert_eq!(answer["authRefused"].as_bool(), Some(true));
    assert_eq!(answer["errors"][0].as_str(), Some(format!("The server turned down the sign-in for {ME}: sign in again").as_str()));
    assert!(s.sync_status.starts_with("The server turned down the sign-in"), "{}", s.sync_status);
    let busy = vec!["Couldn't bring the mail down from the server: busy\r\nnow".to_string()];
    let answer = apply_jmap_mail(&mut s, JmapMailResult { errors: busy, busy_for: Some(120), partial: false, ..piece(account) }).unwrap();
    assert_eq!(
        (answer["busyFor"].as_u64(), answer["errors"][0].as_str()),
        (Some(120), Some("Couldn't bring the mail down from the server: busy now"))
    );
    apply_jmap_mail(&mut s, JmapMailResult { stopped: true, partial: false, ..piece(account) }).unwrap();
    assert!(s.sync_status.starts_with("Stopped; carries on from here next time"), "{}", s.sync_status);
    let answer = run(&mut s, account, vec![], true);
    assert_eq!(answer["summary"].as_str(), Some("Up to date"));
    assert!(s.sync_status.starts_with("Up to date"), "{}", s.sync_status);
}

#[test]
fn a_message_opened_is_fetched_whole_and_the_background_takes_the_inboxs_newest() {
    let (mut s, account) = session();
    let inbox: Vec<GraphMessage> = (1..=45i64).map(|n| email(&format!("E{n}"), "Inbox", n * 100)).collect();
    run(&mut s, account, vec![window("M1", inbox, true), window("M5", vec![email("P1", "Plan", 99_999)], true)], true);
    // One opened, wherever it is: its id, at once.
    let plan = here(&s, account, "P1").unwrap().id;
    let one = jmap_fetch_request(&s, account, Some(plan), 10).unwrap();
    assert_eq!((one.ids.clone(), one.background, one.session.as_str(), one.bearer), (vec!["P1".to_string()], false, SESSION, true));
    // The background: of the Inbox's newest forty, ten at a time, newest first; never Projects'.
    let next = jmap_fetch_request(&s, account, None, 10).unwrap();
    assert_eq!(next.ids, (36..=45).rev().map(|n| format!("E{n}")).collect::<Vec<_>>());
    assert!(next.background);
    let most = jmap_fetch_request(&s, account, None, 100).unwrap();
    assert_eq!(most.ids.len(), 40);
    assert!(!most.ids.iter().any(|id| id == "E5" || id == "P1"));
    let answered = s.run("sync.jmapFetchRequest", &json!({"message": plan})).unwrap();
    assert_eq!(answered["ids"], json!(["P1"]));
    // Fetched: whole, its headers from its own text.
    let mime = b"From: Zoe Park <zoe@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: The whole plan\r\nMessage-ID: <P1@example.org>\r\nContent-Type: text/plain\r\n\r\nAll of it.\r\n".to_vec();
    let fetched = vec![GraphFetched { id: "P1".into(), mime }];
    let answer =
        apply_jmap_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, missing: vec!["E45".into()], ..Default::default() }).unwrap();
    assert_eq!((answer["fetched"].as_u64(), answer["missing"].as_u64()), (Some(1), Some(1)));
    let m = s.mailbox.message(plan).unwrap();
    assert!(!m.unfetched && m.body.text.contains("All of it.") && m.subject == "The whole plan");
    assert!(jmap_fetch_request(&s, account, Some(plan), 10).unwrap().ids.is_empty(), "nothing more to fetch");
    // What the server said it no longer has stays (its window says whether it is gone), and the
    // background doesn't ask for it again this session.
    assert!(here(&s, account, "E45").is_some());
    assert_eq!(jmap_fetch_request(&s, account, None, 1).unwrap().ids, ["E44"]);
}

#[test]
fn a_message_beyond_its_cap_is_passed_over_and_not_asked_for_again() {
    let (mut s, account) = session();
    run(&mut s, account, vec![window("M1", vec![email("E1", "One", 1), email("E2", "Two", 2)], true)], true);
    let fetched = vec![
        GraphFetched { id: "E1".into(), mime: vec![b'x'; (50 << 20) + 1] },
        GraphFetched { id: "E2".into(), mime: Vec::new() },
        GraphFetched { id: "not an id".into(), mime: b"Subject: x\r\n\r\nx".to_vec() },
    ];
    let answer = apply_jmap_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    assert_eq!((answer["fetched"].as_u64(), answer["failed"].as_u64()), (Some(0), Some(2)));
    assert!(here(&s, account, "E1").unwrap().unfetched && here(&s, account, "E2").unwrap().unfetched);
    assert!(jmap_fetch_request(&s, account, None, 10).unwrap().ids.is_empty());
}

#[test]
fn a_draft_here_whole_stays_when_the_servers_copy_goes() {
    let (mut s, account) = session();
    let mut draft = email("D1", "Plan", 1);
    draft.draft = true;
    run(&mut s, account, vec![window("M3", vec![draft], true)], true);
    let id = here(&s, account, "D1").unwrap().id;
    // Opened, so here whole (and perhaps changed here, which goes up only once changes do).
    let mime = b"From: Ada <ada@example.org>\r\nSubject: Plan\r\n\r\nWhat I wrote.\r\n".to_vec();
    let fetched = vec![GraphFetched { id: "D1".into(), mime }];
    apply_jmap_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    // Gone from the server's Drafts: it stays, this computer's own.
    let answer = run(&mut s, account, vec![window("M3", vec![], true)], true);
    assert_eq!(answer["removedMessages"].as_u64(), Some(0));
    let m = s.mailbox.message(id).unwrap();
    assert!(m.server.is_none() && m.draft && m.body.text.contains("What I wrote."));
    // One not fetched yet goes, as any message does.
    let mut other = email("D2", "Notes", 2);
    other.draft = true;
    run(&mut s, account, vec![window("M3", vec![other], true)], true);
    run(&mut s, account, vec![window("M3", vec![], true)], true);
    assert!(here(&s, account, "D2").is_none() && s.mailbox.message(id).is_some());
}

#[test]
fn a_folder_that_stays_keeps_the_folders_above_it() {
    let (mut s, account) = session();
    run(&mut s, account, vec![], true);
    let (projects, apollo) = (folder_here(&s, account, "M5"), folder_here(&s, account, "M6"));
    s.mailbox.add_message(Message { folder: apollo, subject: "Imported".into(), ..Default::default() }).unwrap();
    let mut fewer = mailboxes();
    fewer.retain(|f| f.id != "M5" && f.id != "M6");
    apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders: fewer, complete: true }), ..piece(account) }).unwrap();
    let (p, a) = (s.mailbox.folder(projects).unwrap(), s.mailbox.folder(apollo).unwrap());
    assert!(p.server.is_none() && a.server.is_none() && a.parent == Some(projects));
}

#[test]
fn a_server_address_a_mailbox_file_spoiled_can_be_put_right() {
    let (mut s, account) = session();
    run(&mut s, account, vec![], true);
    s.mailbox.account_mut(account).unwrap().jmap = Some(JmapServer { session: "http://elsewhere.example/jmap".into(), bearer: true });
    // Another setting can still be changed, and the address put right, though its mail is here.
    s.run("account.edit", &json!({"id": account, "name": "Ada"})).unwrap();
    assert!(jmap_mail_request(&s, account, false).is_err());
    s.run("account.edit", &json!({"id": account, "jmap": {"session": SESSION}})).unwrap();
    assert!(jmap_mail_request(&s, account, false).is_ok());
}

/// Until moves and deletions go up to the server, a JMAP account's mail stays where its server has
/// it: each command that would change a message's place says why and changes nothing, and is shown
/// greyed while such a message is selected (or such a folder is in view).
#[test]
fn a_move_or_a_deletion_of_mail_the_server_has_is_refused_until_changes_go_up() {
    let (mut s, account) = session();
    run(&mut s, account, vec![window("M1", vec![email("E1", "One", 1)], true), window("M5", vec![email("P1", "Plan", 2)], true)], true);
    let (one, plan) = (here(&s, account, "E1").unwrap().id, here(&s, account, "P1").unwrap().id);
    let (inbox, projects) = (folder_here(&s, account, "M1"), folder_here(&s, account, "M5"));
    let local = s.mailbox.local_account();
    let elsewhere = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    for (command, params) in [
        ("mail.move", json!({"ids": [one], "folder": projects})),
        ("mail.move", json!({"ids": [one], "folder": elsewhere})),
        ("mail.delete", json!({"ids": [one]})),
        ("mail.deletePermanently", json!({"ids": [one]})),
        ("mail.archive", json!({"ids": [one]})),
        ("mail.junk", json!({"ids": [one]})),
        ("mail.notJunk", json!({"ids": [plan]})),
        ("folder.empty", json!({"id": inbox})),
        ("folder.delete", json!({"id": projects})),
    ] {
        let before = s.mailbox.clone();
        let why = s.run(command, &params).unwrap_err().to_string();
        assert_eq!(why, mailcraft_model::NOT_MOVING_YET, "{command}");
        assert!(s.mailbox == before, "{command} changed the mailbox");
    }
    // Greyed in the window, with the reason.
    s.run("mail.select", &json!({"id": one})).unwrap();
    for command in ["mail.delete", "mail.deletePermanently", "mail.archive", "mail.move", "mail.junk", "mail.notJunk"] {
        assert!(!s.enabled(command), "{command}");
    }
    let why = s.run("mail.delete", &json!({})).unwrap_err().to_string();
    assert_eq!(why, format!("not available: {}", mailcraft_model::NOT_MOVING_YET));
    s.view.folder = inbox;
    assert!(!s.enabled("folder.empty"));
    s.view.folder = projects;
    assert!(!s.enabled("folder.delete"));
    // A rule that would move it leaves it where it is.
    let rule = json!({"name": "Plans", "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": "moveTo", "value": projects}]});
    s.run("rules.add", &rule).unwrap();
    let _ = s.run("rules.run", &json!({"folder": inbox}));
    assert_eq!(s.mailbox.message(one).map(|m| m.folder), Some(inbox));
}

#[test]
fn what_a_jmap_account_holds_only_here_moves_and_goes_as_any_mail() {
    let (mut s, account) = session();
    run(&mut s, account, vec![window("M1", vec![email("E1", "One", 1)], true)], true);
    let (inbox, one) = (folder_here(&s, account, "M1"), here(&s, account, "E1").unwrap().id);
    // Its marks change, and a copy of it goes to a folder of this computer.
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    let local = s.mailbox.local_account();
    let elsewhere = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let copies = s.run("mail.copy", &json!({"ids": [one], "folder": elsewhere})).unwrap();
    assert_eq!(copies["copies"].as_array().map(Vec::len), Some(1));
    // Mail only this computer has, in the account's Inbox: deleted, then deleted for good.
    let mine = s.mailbox.add_message(Message { folder: inbox, subject: "Mine".into(), ..Default::default() }).unwrap();
    s.run("mail.select", &json!({"id": mine})).unwrap();
    assert!(s.enabled("mail.delete"));
    s.run("mail.delete", &json!({"ids": [mine]})).unwrap();
    assert_eq!(s.mailbox.message(mine).map(|m| m.folder), s.mailbox.folder_of_kind(account, FolderKind::Deleted));
    s.run("mail.deletePermanently", &json!({"ids": [mine]})).unwrap();
    assert!(s.mailbox.message(mine).is_none() && s.mailbox.message(one).is_some());
    // And a folder made here, with nothing of the server's in it.
    let ours = s.run("folder.new", &json!({"name": "Ours", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    s.run("folder.delete", &json!({"id": ours})).unwrap();
}

#[test]
fn a_mailbox_that_turns_up_twice_in_a_run_is_read_in_part() {
    let (mut s, account) = session();
    run(&mut s, account, vec![window("M1", vec![email("E1", "One", 1), email("E2", "Two", 2)], true)], true);
    let both = || vec![marks("E1", false, false), marks("E2", false, false)];
    // In two pieces of one run, the second without E1: neither is taken as the whole Inbox.
    let answer = run(&mut s, account, vec![window("M1", both(), true), window("M1", vec![marks("E2", false, false)], true)], true);
    assert_eq!(answer["removedMessages"].as_u64(), Some(0));
    assert!(here(&s, account, "E1").is_some());
    // In one piece, the second without E1: the same.
    let twice = vec![window("M1", both(), true), window("M1", vec![marks("E2", false, false)], true)];
    apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders: mailboxes(), complete: true }), ..piece(account) }).unwrap();
    apply_jmap_mail(&mut s, JmapMailResult { pages: twice, ..piece(account) }).unwrap();
    let answer = apply_jmap_mail(&mut s, JmapMailResult { whole: true, partial: false, ..piece(account) }).unwrap();
    assert_eq!(answer["removedMessages"].as_u64(), Some(0));
    // The next run that reads it once, whole, finds both there.
    assert_eq!(run(&mut s, account, vec![window("M1", both(), true)], true)["removedMessages"].as_u64(), Some(0));
    assert!(here(&s, account, "E1").is_some_and(|m| !m.server.as_ref().is_some_and(|server| server.gone)));
}

#[test]
fn a_batch_fetched_whole_is_held_to_its_size_in_all() {
    let big = |id: &str| GraphFetched { id: id.into(), mime: vec![b'x'; 25 << 20] };
    let fetched = vec![big("E1"), big("E2"), big("E3"), GraphFetched { id: "E4".into(), mime: b"Subject: x\r\n\r\nx".to_vec() }];
    let held = jmap_down::bounded_fetch(GraphFetchResult { fetched, ..Default::default() });
    // From the one that would take it past 64 MB, the rest waits for the next batch: neither taken
    // nor refused. (The first is taken whatever its size, up to 50 MB.)
    assert_eq!(held.fetched.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["E1", "E2"]);
    assert!(held.failed.is_empty());
}

#[test]
fn the_host_side_of_a_jmap_run_is_never_an_agents() {
    for id in ["sync.jmapRequest", "sync.applyJmap", "sync.jmapFetchRequest", "sync.applyJmapFetch"] {
        assert_eq!(crate::agent::access(id), Some(crate::agent::Access::Never), "{id}");
    }
    // Without a JMAP account there is nothing to ask for.
    let mut s = Session::empty();
    assert!(!s.enabled("sync.jmapRequest") && !s.enabled("sync.jmapFetchRequest"));
    assert_eq!(s.run("sync.jmapRequest", &json!({})).unwrap_err().to_string(), "not available: no account is kept in step over JMAP");
}
