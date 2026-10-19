//! A JMAP account's mailboxes and newest messages, from end to end: the engine's `jmap_mail_request`, then
//! `mailcraft_jmap::sync_mail` over the in-process JMAP server (driven by `block_on`, as a host on one
//! thread drives it), then `apply_jmap_mail` for each piece as it comes. Nothing here leaves this machine;
//! every name, address and id is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::jmap_mail::{apply_jmap_mail, jmap_mail_request};
use mailcraft_jmap::standin::{Profile, StandIn, TOKEN};
use mailcraft_jmap::{Auth, Client, block_on, failed_run, sync_mail};
use mailcraft_model::sync::JmapMailResult;
use mailcraft_model::{FlagStatus, FolderKind, Id, Mailbox, Message};
use serde_json::{Value, json};

const ADDRESS: &str = "ada@example.org";
/// The address of the server's session, with the owner's address in its query as some servers have it.
const SESSION: &str = "https://jmap.standin.example/jmap/session?account=ada%40example.org";

/// A session with one JMAP account, and an in-process server for it.
struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    fn new() -> Rig {
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let added = s.run("account.add", &json!({"email": ADDRESS, "kind": "jmap", "jmap": {"session": SESSION, "bearer": true}})).unwrap();
        Rig { server: StandIn::new(), account: added["id"].as_u64().unwrap(), s }
    }

    /// The server's mailboxes: the Inbox and the others by role, and a folder of the owner's with one in it.
    fn mailboxes(&self) -> Vec<String> {
        self.server.with(|d| {
            let projects = d.mailbox("Projects", None, None);
            vec![
                d.mailbox("Inbox", None, Some("inbox")),
                d.mailbox("Sent", None, Some("sent")),
                d.mailbox("Trash", None, Some("trash")),
                d.mailbox("Apollo", Some(&projects), None),
                projects,
            ]
        })
    }

    /// A message in `mailbox`, as a person's mail program wrote it: its id on the server.
    fn deliver(&self, mailbox: &str, subject: &str, keywords: &[&str], received: &str) -> String {
        let raw = format!(
            "From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ADDRESS}>\r\nSubject: {subject}\r\nMessage-ID: <{subject}@example.org>\r\nDate: Thu, 15 Oct 2026 09:30:00 +0200\r\n\r\nHello about {subject}.\r\n"
        );
        self.server.with(|d| d.deliver(mailbox, raw.as_bytes(), keywords, received))
    }

    /// One run, as a host does it: what the engine asks for (`full`: every mailbox; else the Inbox and `also`),
    /// done against the server, each piece taken in as it arrives. What taking each in answered.
    fn run(&mut self, full: bool, also: &[String]) -> Vec<Value> {
        let request = jmap_mail_request(&self.s, self.account, full, also, false).unwrap();
        let client = block_on(Client::connect(self.server.clone(), &request.session, &Auth::bearer(TOKEN))).unwrap();
        let mut answers = Vec::new();
        let (s, cancel) = (&mut self.s, AtomicBool::new(false));
        block_on(sync_mail(&client, &request, &cancel, &mut |piece| answers.push(apply_jmap_mail(s, piece).unwrap())));
        answers
    }

    fn folder(&self, name: &str) -> Id {
        self.s.mailbox.folders.iter().find(|f| f.account == self.account && f.name == name).unwrap_or_else(|| panic!("no folder {name} here")).id
    }

    /// The messages in the folder `name`, newest first.
    fn mail(&self, name: &str) -> Vec<&Message> {
        let folder = self.folder(name);
        let mut found: Vec<&Message> = self.s.mailbox.messages.values().map(|m| m.as_ref()).filter(|m| m.folder == folder).collect();
        found.sort_by_key(|m| std::cmp::Reverse(m.received));
        found
    }
}

fn subjects(messages: &[&Message]) -> Vec<String> {
    messages.iter().map(|m| m.subject.clone()).collect()
}

fn server_ids<'a>(messages: &[&'a Message]) -> Vec<&'a str> {
    messages.iter().map(|m| m.server.as_ref().map_or("", |s| s.id.as_str())).collect()
}

/// What a first run must leave in the mailbox, whatever kind of server it ran against.
fn first_run(rig: &mut Rig) {
    let [inbox, sent, trash, apollo, projects] = rig.mailboxes().try_into().unwrap();
    rig.deliver(&inbox, "first", &["$seen"], "2026-10-14T09:00:00Z");
    rig.deliver(&inbox, "second", &[], "2026-10-15T06:00:00Z");
    rig.deliver(&inbox, "third", &["$flagged", "$answered"], "2026-10-15T07:30:00Z");
    rig.deliver(&sent, "outgoing", &["$seen"], "2026-10-15T08:00:00Z");
    rig.deliver(&apollo, "launch", &[], "2026-10-15T09:00:00Z");
    let answers = rig.run(false, &[]);

    // The pieces: the folders, a window for each of the five mailboxes (the Inbox first), the end.
    assert_eq!(answers.len(), 1 + 5 + 1);
    // Made: the two of the owner's. Tied by role: the account's own Inbox, Sent and Deleted Items.
    assert_eq!((answers[0]["added"].as_u64(), answers[0]["updated"].as_u64()), (Some(2), Some(3)), "{}", answers[0]);
    assert!(answers.iter().all(|a| a["errors"] == json!([])), "{answers:?}");
    assert_eq!(answers[1]["new"], 3, "the Inbox first");
    assert_eq!(answers.iter().map(|a| a["new"].as_u64().unwrap_or(0)).sum::<u64>(), 5);
    assert_eq!(answers.last().unwrap()["authRefused"], false);
    assert!(rig.s.sync_status.contains('—') && !rig.s.sync_status.contains("refused"), "{}", rig.s.sync_status);

    // The folders: the account's own Inbox and Sent are the server's, by role; the others are made, in their place.
    let account = rig.account;
    let tied = |name: &str| rig.s.mailbox.folder(rig.folder(name)).unwrap().server.as_ref().map(|s| s.id.clone());
    assert_eq!((tied("Inbox"), tied("Sent"), tied("Trash")), (Some(inbox.clone()), Some(sent.clone()), Some(trash.clone())));
    assert_eq!(rig.s.mailbox.folder_of_kind(account, FolderKind::Inbox), Some(rig.folder("Inbox")));
    assert_eq!(rig.s.mailbox.folder(rig.folder("Apollo")).unwrap().parent, Some(rig.folder("Projects")));
    assert_eq!(rig.s.mailbox.folder(rig.folder("Projects")).unwrap().server.as_ref().map(|s| s.id.clone()), Some(projects));

    // The Inbox: the newest messages, not fetched, each with its preview and what the server says of it.
    let inbox_mail = rig.mail("Inbox");
    assert_eq!(subjects(&inbox_mail), ["third", "second", "first"]);
    assert!(inbox_mail.iter().all(|m| m.unfetched && m.body.html.is_none()), "headers and a preview, not the message");
    assert_eq!(inbox_mail[0].body.text, "Hello about third.");
    assert_eq!((inbox_mail[0].from.name.as_str(), inbox_mail[0].from.email.as_str()), ("Zoe Park", "zoe@example.org"));
    assert_eq!((inbox_mail[0].to[0].name.as_str(), inbox_mail[0].to[0].email.as_str()), ("Ada", ADDRESS));
    assert_eq!(inbox_mail[0].message_id, "third@example.org");
    // Sent at 09:30 two hours ahead of UTC, and arrived at 07:30 UTC: the same moment.
    let at = mailcraft_time::iso_to_timestamp("2026-10-15T07:30:00Z", 0).unwrap();
    assert_eq!((inbox_mail[0].date, inbox_mail[0].received), (at, at));
    assert_eq!((inbox_mail[2].read, inbox_mail[1].read, inbox_mail[0].read), (true, false, false));
    assert_eq!((inbox_mail[0].flag, inbox_mail[1].flag), (FlagStatus::Flagged, FlagStatus::None));
    assert!(inbox_mail[0].answered, "the server says it was answered");
    assert!(!inbox_mail[1].answered && !inbox_mail[0].forwarded);
    assert_eq!(subjects(&rig.mail("Sent")), ["outgoing"]);
    assert_eq!(subjects(&rig.mail("Apollo")), ["launch"]);
    assert!(rig.mail("Trash").is_empty() && rig.mail("Projects").is_empty());
}

#[test]
fn a_first_run_brings_the_folders_and_the_newest_messages_unfetched_with_their_previews() {
    let mut rig = Rig::new();
    first_run(&mut rig);
    // Tied now, the account asks for the Inbox alone, and says which messages it has so that it is sent only their marks.
    let request = jmap_mail_request(&rig.s, rig.account, false, &[], false).unwrap();
    assert!(!request.list_folders);
    let inbox = rig.s.mailbox.folder(rig.folder("Inbox")).unwrap().server.as_ref().map(|s| s.id.clone()).unwrap();
    let known: Vec<(&str, usize)> = request.known.iter().map(|k| (k.mailbox.as_str(), k.ids.len())).collect();
    assert!(known.contains(&(inbox.as_str(), 3)), "{known:?}");
    // No address of the owner's, no secret and no mail in what shows of the request or the state of the run.
    let shown = format!("{request:?} {}", rig.s.sync_status);
    assert!(!shown.contains('@') && !shown.contains("example.org") && !shown.contains("account="), "{shown}");
}

#[test]
fn a_server_that_names_messages_by_where_they_are_gives_the_same_mailbox() {
    let mut standard = Rig::new();
    first_run(&mut standard);
    let mut by_place = Rig::new();
    by_place.server.with(|d| d.profile = Profile::by_place());
    first_run(&mut by_place);
    let summary = |rig: &Rig| -> Vec<(String, Vec<String>)> {
        ["Inbox", "Sent", "Apollo"].into_iter().map(|name| (name.to_string(), subjects(&rig.mail(name)))).collect()
    };
    assert_eq!(summary(&standard), summary(&by_place));
}

#[test]
fn a_second_run_changes_a_keyword_and_removes_a_message_the_server_no_longer_lists() {
    let mut rig = Rig::new();
    first_run(&mut rig);
    let before = rig.mail("Inbox").iter().map(|m| (m.subject.clone(), m.id)).collect::<Vec<_>>();
    let id_of = |subject: &str| before.iter().find(|(s, _)| s == subject).unwrap().1;
    let (first, second, third) = (id_of("first"), id_of("second"), id_of("third"));
    let inbox = rig.s.mailbox.folder(rig.folder("Inbox")).unwrap().server.as_ref().map(|s| s.id.clone()).unwrap();
    // Another client reads the second, deletes the first and flags nothing; a fourth message comes.
    let (first_on_server, second_on_server) = {
        let ids = server_ids(&rig.mail("Inbox"));
        (ids[2].to_string(), ids[1].to_string())
    };
    rig.server.with(|d| {
        assert!(d.set_keyword(&second_on_server, "$seen", true));
        assert!(d.remove(&first_on_server));
    });
    rig.deliver(&inbox, "fourth", &[], "2026-10-16T07:00:00Z");
    rig.server.with(|d| d.seen.clear());
    let answers = rig.run(false, &[]);

    // The window is the whole Inbox, so what it lacks is gone here too.
    let last = answers.last().unwrap();
    assert_eq!(last["errors"], json!([]));
    assert_eq!(answers[0]["new"], 1, "the Inbox is the first window of a run that reads the Inbox alone");
    assert_eq!(answers[0]["changed"], 1);
    // (It is marked gone by the window and removed when the run ends whole.)
    assert_eq!((answers[0]["removedMessages"].as_u64(), last["removedMessages"].as_u64()), (Some(0), Some(1)));
    let now = rig.mail("Inbox");
    assert_eq!(subjects(&now), ["fourth", "third", "second"]);
    assert!(rig.s.mailbox.message(first).is_none(), "the one the server no longer has");
    assert!(rig.s.mailbox.message(second).unwrap().read, "read elsewhere, read here");
    assert!(rig.s.mailbox.message(third).is_some_and(|m| m.flag == FlagStatus::Flagged));
    assert!(now[0].unfetched && now[0].body.text == "Hello about fourth.", "the new one has its headers and preview");
    // The work: the list of mailboxes, the window (the newest ids with their keywords), and the headers of the one message this mailbox lacked.
    let asked = rig.server.asked();
    let shape: Vec<(&str, Option<usize>)> = asked.iter().map(|a| (a.method.as_str(), a.ids)).collect();
    assert_eq!(shape, [("Mailbox/get", None), ("Email/query", None), ("Email/get", Some(3)), ("Email/get", Some(1))]);
    assert!(asked[3].properties.iter().any(|p| p == "headers") && !asked[2].properties.iter().any(|p| p == "headers"));
    // Other mailboxes were not read.
    assert_eq!(subjects(&rig.mail("Sent")), ["outgoing"]);
}

#[test]
fn a_window_cut_short_adds_and_updates_but_removes_nothing() {
    let mut rig = Rig::new();
    let [inbox, ..] = <[String; 5]>::try_from(rig.mailboxes()).unwrap();
    for n in 0..6 {
        rig.deliver(&inbox, &format!("m{n}"), &[], &format!("2026-10-15T0{n}:00:00Z"));
    }
    rig.run(true, &[]);
    assert_eq!(rig.mail("Inbox").len(), 6);
    // A window of three of the six: the server loses the oldest and the newest, and a new one arrives.
    rig.s.prefs.sync_limit = 3;
    let (oldest, newest) = {
        let ids = server_ids(&rig.mail("Inbox"));
        (ids[5].to_string(), ids[0].to_string())
    };
    rig.server.with(|d| (d.remove(&oldest), d.remove(&newest)));
    rig.deliver(&inbox, "late", &[], "2026-10-16T00:00:00Z");
    let answers = rig.run(false, &[]);
    assert_eq!(answers.last().unwrap()["errors"], json!([]));
    // The newest three of what the server has are read; what is outside them is neither added nor removed.
    let now = subjects(&rig.mail("Inbox"));
    assert!(now.contains(&"late".to_string()) && now.contains(&"m5".to_string()) && now.contains(&"m0".to_string()), "{now:?}");
    assert_eq!(answers[0]["removedMessages"], 0);
    // The window grows to hold everything: now the two it lost are gone here too.
    rig.s.prefs.sync_limit = 500;
    rig.run(false, &[]);
    let now = subjects(&rig.mail("Inbox"));
    assert!(!now.contains(&"m5".to_string()) && !now.contains(&"m0".to_string()), "{now:?}");
    assert_eq!(now.len(), 5);
}

#[test]
fn the_folder_on_screen_is_read_with_the_inbox_and_the_others_wait() {
    let mut rig = Rig::new();
    let [inbox, sent, _, apollo, _] = <[String; 5]>::try_from(rig.mailboxes()).unwrap();
    rig.deliver(&inbox, "in", &[], "2026-10-15T06:00:00Z");
    rig.deliver(&sent, "out", &[], "2026-10-15T07:00:00Z");
    rig.deliver(&apollo, "up", &[], "2026-10-15T08:00:00Z");
    rig.run(true, &[]);
    rig.deliver(&sent, "out2", &[], "2026-10-16T07:00:00Z");
    rig.deliver(&apollo, "up2", &[], "2026-10-16T08:00:00Z");
    // The owner is looking at Sent: the Inbox and Sent are read, Apollo is not.
    let answers = rig.run(false, std::slice::from_ref(&sent));
    assert_eq!(answers.iter().map(|a| a["new"].as_u64().unwrap_or(0)).sum::<u64>(), 1);
    assert_eq!(subjects(&rig.mail("Sent")), ["out2", "out"]);
    assert_eq!(subjects(&rig.mail("Apollo")), ["up"]);
    // The next full run brings the rest.
    rig.run(true, &[]);
    assert_eq!(subjects(&rig.mail("Apollo")), ["up2", "up"]);
}

#[test]
fn a_server_that_refuses_the_credentials_is_told_to_the_owner_not_retried() {
    let mut rig = Rig::new();
    rig.mailboxes();
    // The token expires between the sign-in and the run.
    let request = jmap_mail_request(&rig.s, rig.account, true, &[], false).unwrap();
    let client = block_on(Client::connect(rig.server.clone(), &request.session, &Auth::bearer(TOKEN))).unwrap();
    rig.server.with(|d| d.token = Some("renewed".into()));
    let mut pieces: Vec<JmapMailResult> = Vec::new();
    block_on(sync_mail(&client, &request, &AtomicBool::new(false), &mut |piece| pieces.push(piece)));
    assert_eq!(pieces.len(), 1);
    let answer = apply_jmap_mail(&mut rig.s, pieces.remove(0)).unwrap();
    assert_eq!(answer["authRefused"], true);
    assert!(rig.s.sync_status.starts_with("The server refused the sign-in"), "{}", rig.s.sync_status);

    // The sign-in itself is refused: there is no client, and the run ends all the same.
    let error = block_on(Client::connect(rig.server.clone(), &request.session, &Auth::bearer(TOKEN))).unwrap_err();
    let ended = failed_run(&request, &error);
    let answer = apply_jmap_mail(&mut rig.s, ended).unwrap();
    assert_eq!(answer["authRefused"], true);
    assert!(rig.mail_in_all().is_empty(), "nothing came of it");
}

impl Rig {
    fn mail_in_all(&self) -> Vec<&Message> {
        self.s.mailbox.messages.values().map(|m| m.as_ref()).filter(|m| m.account == self.account).collect()
    }
}

#[test]
fn a_busy_server_is_left_alone_and_what_came_before_is_kept() {
    let mut rig = Rig::new();
    let [inbox, sent, ..] = <[String; 5]>::try_from(rig.mailboxes()).unwrap();
    rig.deliver(&inbox, "in", &[], "2026-10-15T06:00:00Z");
    rig.deliver(&sent, "out", &[], "2026-10-15T07:00:00Z");
    // The list, the Inbox's window and its message go through; the next request is told to wait.
    rig.server.with(|d| d.refuse_next = [None, None, None, Some((429, Some(45)))].into());
    let answers = rig.run(true, &[]);
    assert_eq!(subjects(&rig.mail("Inbox")), ["in"]);
    assert!(rig.mail("Sent").is_empty());
    let last = answers.last().unwrap();
    assert_eq!(last["busyFor"], 45);
    assert!(last["errors"][0].as_str().unwrap().contains("busy"), "{last}");
    // The next run goes on from there.
    rig.run(true, &[]);
    assert_eq!(subjects(&rig.mail("Sent")), ["out"]);
}

#[test]
fn one_mailbox_that_fails_does_not_keep_the_others_from_coming() {
    let mut rig = Rig::new();
    let [inbox, sent, _, apollo, _] = <[String; 5]>::try_from(rig.mailboxes()).unwrap();
    rig.deliver(&inbox, "in", &[], "2026-10-15T06:00:00Z");
    rig.deliver(&sent, "out", &[], "2026-10-15T07:00:00Z");
    rig.deliver(&apollo, "up", &[], "2026-10-15T08:00:00Z");
    rig.server.with(|d| d.broken_mailboxes.insert(sent.clone(), "serverFail"));
    let answers = rig.run(true, &[]);
    assert_eq!(subjects(&rig.mail("Inbox")), ["in"]);
    assert_eq!(subjects(&rig.mail("Apollo")), ["up"]);
    assert!(rig.mail("Sent").is_empty());
    let errors = answers.last().unwrap()["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    // The trouble is said, with no name of a mailbox, an address or a secret in it.
    let said = errors[0].as_str().unwrap();
    assert!(said.starts_with("Couldn't read the mail in 1 mailbox"), "{said}");
    assert!(!said.contains(&sent) && !said.contains("example.org") && !said.contains(TOKEN), "{said}");
    assert!(!rig.s.sync_status.contains("Up to date"), "{}", rig.s.sync_status);
    // The mail that did come stays, and the mailbox that failed is read when the server can.
    rig.server.with(|d| d.broken_mailboxes.clear());
    rig.run(true, &[]);
    assert_eq!(subjects(&rig.mail("Sent")), ["out"]);
}

#[test]
fn a_message_the_server_names_anew_is_the_same_message_and_not_a_new_one() {
    let mut rig = Rig::new();
    let [inbox, ..] = <[String; 5]>::try_from(rig.mailboxes()).unwrap();
    let old = rig.deliver(&inbox, "moved", &["$seen"], "2026-10-15T06:00:00Z");
    rig.deliver(&inbox, "stays", &[], "2026-10-15T07:00:00Z");
    rig.run(true, &[]);
    let here = rig.mail("Inbox").iter().find(|m| m.subject == "moved").map(|m| m.id).unwrap();
    // The server gives the same message another id (some name a message by where it lies, and a move changes that).
    assert!(rig.server.with(|d| d.remove(&old)));
    let renamed = rig.deliver(&inbox, "moved", &["$seen"], "2026-10-15T06:00:00Z");
    let answers = rig.run(false, &[]);
    // It is taken back by its Message-ID: nothing new, nothing removed, the id is the server's new one.
    let total = |key: &str| answers.iter().map(|a| a[key].as_u64().unwrap_or(0)).sum::<u64>();
    assert_eq!((total("new"), total("removedMessages")), (0, 0), "{answers:?}");
    assert_eq!(rig.s.mailbox.message(here).and_then(|m| m.server.as_ref()).map(|s| s.id.clone()), Some(renamed));
    assert_eq!(subjects(&rig.mail("Inbox")), ["stays", "moved"]);
}

#[test]
fn a_mailbox_the_server_no_longer_has_goes_with_its_mail_and_one_it_emptied_is_emptied_here() {
    let mut rig = Rig::new();
    let [inbox, sent, _, apollo, projects] = <[String; 5]>::try_from(rig.mailboxes()).unwrap();
    rig.deliver(&inbox, "in", &[], "2026-10-15T06:00:00Z");
    let out = rig.deliver(&sent, "out", &[], "2026-10-15T07:00:00Z");
    rig.deliver(&apollo, "up", &[], "2026-10-15T08:00:00Z");
    rig.run(true, &[]);
    assert_eq!((rig.mail("Sent").len(), rig.mail("Apollo").len()), (1, 1));
    // Another client deletes the folders Projects and Apollo, and empties Sent.
    rig.server.with(|d| {
        d.mailboxes.retain(|m| m.id != apollo && m.id != projects);
        d.emails.retain(|e| !e.mailboxes.contains(&apollo));
        assert!(d.remove(&out));
    });
    let answers = rig.run(true, &[]);
    assert_eq!(answers[0]["removed"], 2, "{}", answers[0]);
    assert!(!rig.s.mailbox.folders.iter().any(|f| f.account == rig.account && (f.name == "Apollo" || f.name == "Projects")));
    assert!(rig.mail("Sent").is_empty(), "a complete window of nothing empties the folder here");
    assert_eq!(subjects(&rig.mail("Inbox")), ["in"]);
    assert!(rig.mail_in_all().iter().all(|m| m.subject != "up" && m.subject != "out"));
}

#[test]
fn a_message_the_server_files_in_a_second_mailbox_is_kept_by_a_run_that_does_not_read_that_one() {
    let mut rig = Rig::new();
    let [inbox, _, _, apollo, projects] = <[String; 5]>::try_from(rig.mailboxes()).unwrap();
    rig.deliver(&inbox, "in", &[], "2026-10-15T06:00:00Z");
    let launch = rig.deliver(&apollo, "launch", &[], "2026-10-15T09:00:00Z");
    rig.run(true, &[]);
    assert_eq!(subjects(&rig.mail("Apollo")), ["launch"]);
    // The server files it in Projects too, whose id is the smaller: Projects lists it, and Apollo no longer does.
    assert!(projects < apollo && rig.server.with(|d| d.add_to_mailbox(&launch, &projects)));
    // A run that reads the Inbox and Apollo alone does not hear of it from Projects, and Apollo's window is not
    // the whole of Apollo: the message is not taken for gone.
    let answers = rig.run(false, std::slice::from_ref(&apollo));
    assert_eq!(answers.iter().map(|a| a["removedMessages"].as_u64().unwrap_or(0)).sum::<u64>(), 0, "{answers:?}");
    assert_eq!(subjects(&rig.mail("Apollo")), ["launch"]);
    // A run that reads Projects as well moves it there, once.
    rig.run(true, &[]);
    assert_eq!(subjects(&rig.mail("Projects")), ["launch"]);
    assert!(rig.mail("Apollo").is_empty());
    assert_eq!(rig.mail_in_all().iter().filter(|m| m.subject == "launch").count(), 1);
}

#[test]
fn a_message_moved_on_the_server_is_kept_where_a_run_reads_both_ends_and_comes_back_new_where_it_read_one() {
    // A server whose ids survive a move, and one that names a message by where it lies (a move is a new id).
    for by_place in [false, true] {
        let mut rig = Rig::new();
        if by_place {
            rig.server.with(|d| d.profile = Profile::by_place());
        }
        let [inbox, _, _, apollo, _] = <[String; 5]>::try_from(rig.mailboxes()).unwrap();
        let moved = rig.deliver(&inbox, "moved", &["$seen"], "2026-10-15T06:00:00Z");
        let stays = rig.deliver(&inbox, "stays", &[], "2026-10-15T07:00:00Z");
        rig.run(true, &[]);
        let local = |rig: &Rig, subject: &str| rig.mail_in_all().iter().find(|m| m.subject == subject).map(|m| m.id).unwrap();
        let (moved_here, stays_here) = (local(&rig, "moved"), local(&rig, "stays"));
        let removed = |answers: &[Value]| answers.iter().map(|a| a["removedMessages"].as_u64().unwrap_or(0)).sum::<u64>();

        // Another client moves a message to Apollo. A run that reads both mailboxes has the Inbox without it and
        // Apollo with it. Where ids survive a move it is the same message here (what is known of it is kept);
        // where they do not, the old one is gone from the server and the one in Apollo is a new message.
        let now = rig.server.with(|d| d.move_to(&moved, &apollo)).unwrap();
        assert_eq!(now != moved, by_place);
        let answers = rig.run(true, &[]);
        assert_eq!(answers.last().unwrap()["errors"], json!([]));
        assert_eq!(subjects(&rig.mail("Inbox")), ["stays"], "by place: {by_place}");
        assert_eq!(subjects(&rig.mail("Apollo")), ["moved"], "by place: {by_place}");
        assert_eq!(local(&rig, "moved") == moved_here, !by_place, "by place: {by_place}");
        assert_eq!(removed(&answers), u64::from(by_place), "by place: {by_place}");

        // Another is moved, and the run reads only the Inbox, which is empty now and says so: the message is not
        // there, and this run does not know where it went. It is taken out of the Inbox here, and comes back, as a
        // new message, when a run reads Apollo.
        rig.server.with(|d| d.move_to(&stays, &apollo)).unwrap();
        let answers = rig.run(false, &[]);
        assert_eq!(removed(&answers), 1, "by place: {by_place}");
        assert!(rig.mail("Inbox").is_empty() && rig.s.mailbox.message(stays_here).is_none());
        assert_eq!(subjects(&rig.mail("Apollo")), ["moved"], "Apollo was not read");
        let answers = rig.run(true, &[]);
        assert_eq!(answers.iter().map(|a| a["new"].as_u64().unwrap_or(0)).sum::<u64>(), 1, "by place: {by_place}");
        assert_eq!(subjects(&rig.mail("Apollo")), ["stays", "moved"]);
    }
}
