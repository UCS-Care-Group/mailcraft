// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A JMAP account (M6) from end to end: the engine's `jmap_mail_request` and `apply_jmap_mail`, and
//! the fetch of a message whole, with `mailcraft_jmap`'s run in between, against the crate's JMAP
//! server in the process (`stand-in`), as a host runs them, piece by piece. Nothing here leaves the
//! process; every name, address and id is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::jmap_mail::{apply_jmap_fetch, apply_jmap_mail, jmap_fetch_request, jmap_mail_request};
use mailcraft_jmap::standin::{Profile, SESSION, StandIn, TOKEN};
use mailcraft_jmap::{Connection, Credentials, block_on, failed_run, fetch, sync_mail};
use mailcraft_model::{FlagStatus, FolderKind, Id, Mailbox, Message};
use serde_json::{Value, json};

const ME: &str = "ada@example.org";

/// A message as a mail program writes one.
fn mail(subject: &str) -> Vec<u8> {
    format!(
        "From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: {subject}\r\nMessage-ID: <{subject}@example.org>\r\nDate: Thu, 15 Oct 2026 09:30:00 +0200\r\n\r\nHello about {subject}.\r\n"
    )
    .into_bytes()
}

/// A session with one JMAP account, and the server it reads: an Inbox and its fellows by role, and
/// Apollo inside Projects.
struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
    inbox: String,
    projects: String,
    apollo: String,
}

impl Rig {
    fn new() -> Rig {
        let server = StandIn::new();
        let (inbox, projects, apollo) = server.with(|d| {
            let inbox = d.mailbox("Inbox", None, Some("inbox"));
            d.mailbox("Sent", None, Some("sent"));
            d.mailbox("Trash", None, Some("trash"));
            let projects = d.mailbox("Projects", None, None);
            let apollo = d.mailbox("Apollo", Some(&projects), None);
            (inbox, projects, apollo)
        });
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let added = s.run("account.add", &json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION, "bearer": true}})).unwrap();
        Rig { server, s, account: added["id"].as_u64().unwrap(), inbox, projects, apollo }
    }

    /// An email in `mailbox` with these keywords, arriving at the `n`th minute of a day: its id.
    fn deliver(&self, mailbox: &str, subject: &str, keywords: &[&str], n: usize) -> String {
        let at = format!("2026-10-15T{:02}:{:02}:00Z", n / 60 % 24, n % 60);
        self.server.with(|d| d.deliver(mailbox, &mail(subject), keywords, &at))
    }

    fn connect(&self) -> Result<Connection<StandIn>, mailcraft_jmap::Error> {
        block_on(Connection::connect(self.server.clone(), SESSION, Credentials::bearer(TOKEN).unwrap()))
    }

    /// One run as a host makes it: what the engine asks for, read from the server, each piece taken in
    /// as it comes (a sign-in that fails is the run's one piece). What taking in each piece answered.
    fn run(&mut self, light: bool) -> Vec<Value> {
        let request = jmap_mail_request(&self.s, self.account, light).unwrap();
        let mut pieces = Vec::new();
        match self.connect() {
            Ok(connection) => block_on(sync_mail(&connection, &request, &AtomicBool::new(false), &mut |piece| pieces.push(piece))),
            Err(error) => pieces.push(failed_run(&request, &error)),
        }
        pieces.into_iter().map(|piece| apply_jmap_mail(&mut self.s, piece).unwrap()).collect()
    }

    /// The message of the account with this subject.
    fn here(&self, subject: &str) -> Option<&Message> {
        self.s.mailbox.messages.values().map(|m| &**m).find(|m| m.account == self.account && m.subject == subject)
    }

    /// The folder the server's mailbox `id` is here.
    fn folder(&self, id: &str) -> Id {
        self.s.mailbox.folder_by_server(self.account, id).unwrap()
    }
}

#[test]
fn the_first_run_brings_the_folders_and_the_newest_mail_and_the_next_asks_for_marks_alone() {
    let mut rig = Rig::new();
    rig.deliver(&rig.inbox, "Kickoff", &["$seen", "$answered"], 1);
    rig.deliver(&rig.inbox, "Budget", &["$flagged"], 2);
    rig.deliver(&rig.apollo, "Launch", &["$forwarded"], 3);
    let answers = rig.run(false);
    assert_eq!(answers.last().unwrap()["errors"], json!([]), "{answers:?}");
    assert_eq!(answers.iter().filter_map(|a| a["new"].as_u64()).sum::<u64>(), 3);
    // The tree, and the Inbox here the server's.
    let apollo = rig.s.mailbox.folder(rig.folder(&rig.apollo)).unwrap();
    assert_eq!((apollo.name.as_str(), apollo.parent), ("Apollo", Some(rig.folder(&rig.projects))));
    assert_eq!(rig.s.mailbox.folder_of_kind(rig.account, FolderKind::Inbox), Some(rig.folder(&rig.inbox)));
    // Each message with what a list shows of it and its marks, not fetched yet.
    let kickoff = rig.here("Kickoff").unwrap();
    let shown = (kickoff.from.email.as_str(), kickoff.read, kickoff.answered, kickoff.unfetched, kickoff.folder);
    assert_eq!(shown, ("zoe@example.org", true, true, true, rig.folder(&rig.inbox)));
    assert_eq!(rig.here("Budget").unwrap().flag, FlagStatus::Flagged);
    let launch = rig.here("Launch").unwrap();
    assert!(launch.forwarded && launch.folder == rig.folder(&rig.apollo));
    // The next run asks for the marks of what is here: no header field again.
    let before = rig.server.asked().len();
    rig.run(false);
    let asked = rig.server.asked();
    assert!(asked.iter().skip(before).all(|a| !a.properties.iter().any(|p| p == "headers")), "{:?}", &asked[before..]);
    assert_eq!(rig.s.mailbox.messages.values().filter(|m| m.account == rig.account).count(), 3);
}

#[test]
fn marks_changed_there_come_down_and_one_changed_here_stays() {
    let mut rig = Rig::new();
    let kickoff = rig.deliver(&rig.inbox, "Kickoff", &[], 1);
    let budget = rig.deliver(&rig.inbox, "Budget", &[], 2);
    rig.run(false);
    let id = rig.here("Budget").unwrap().id;
    rig.s.mailbox.message_mut(id).unwrap().read = true;
    rig.server.with(|d| {
        d.set_keyword(&kickoff, "$seen", true);
        d.set_keyword(&kickoff, "$flagged", true);
        d.set_keyword(&budget, "$flagged", true);
    });
    rig.run(true);
    let k = rig.here("Kickoff").unwrap();
    assert!(k.read && k.flag == FlagStatus::Flagged);
    // Unread there all along, read here: the read mark made here stays, the flag comes down.
    let b = rig.here("Budget").unwrap();
    assert!(b.read && b.flag == FlagStatus::Flagged);
}

#[test]
fn what_the_server_removed_goes_here_and_a_window_in_part_removes_nothing() {
    let mut rig = Rig::new();
    let ids: Vec<String> = (1..=3).map(|n| rig.deliver(&rig.inbox, &format!("Note{n}"), &[], n)).collect();
    rig.run(false);
    rig.server.with(|d| d.remove(&ids[0]));
    rig.run(false);
    assert!(rig.here("Note1").is_none() && rig.here("Note2").is_some());
    // A window of one, and an Inbox of two: read in part, nothing goes for what it lacks.
    rig.deliver(&rig.inbox, "Note4", &[], 4);
    rig.s.prefs.sync_limit = 1;
    rig.server.with(|d| d.remove(&ids[1]));
    rig.run(false);
    assert!(rig.here("Note2").is_some() && rig.here("Note3").is_some() && rig.here("Note4").is_some());
}

#[test]
fn a_run_that_goes_wrong_removes_nothing_and_the_next_that_goes_well_does() {
    let mut rig = Rig::new();
    let first = rig.deliver(&rig.inbox, "First", &[], 1);
    rig.deliver(&rig.apollo, "Launch", &[], 2);
    rig.run(false);
    // Removed there, and Apollo can't be read this time.
    rig.server.with(|d| {
        d.remove(&first);
        d.broken_mailboxes.insert(rig.apollo.clone(), "serverFail");
    });
    let answers = rig.run(false);
    assert!(answers.last().unwrap()["errors"].as_array().is_some_and(|lines| !lines.is_empty()), "{answers:?}");
    assert!(rig.here("First").is_some() && rig.here("Launch").is_some());
    rig.server.with(|d| d.broken_mailboxes.clear());
    rig.run(false);
    assert!(rig.here("First").is_none() && rig.here("Launch").is_some());
}

#[test]
fn a_secret_turned_down_is_said_and_nothing_goes() {
    let mut rig = Rig::new();
    let kept = rig.deliver(&rig.inbox, "Kept", &[], 1);
    rig.run(false);
    // In the middle of a run: the mailboxes come, then the token is gone.
    rig.server.with(|d| {
        d.remove(&kept);
        d.drop_token_after = Some(1);
    });
    let answers = rig.run(false);
    let last = answers.last().unwrap();
    assert_eq!(last["authRefused"].as_bool(), Some(true), "{answers:?}");
    assert!(last["errors"][0].as_str().is_some_and(|line| line.starts_with("The server turned down the sign-in")), "{last}");
    assert!(rig.s.sync_status.starts_with("The server turned down the sign-in"), "{}", rig.s.sync_status);
    assert!(rig.here("Kept").is_some());
    // Before it begins: the same.
    let answers = rig.run(false);
    assert_eq!((answers.len(), answers[0]["authRefused"].as_bool()), (1, Some(true)));
    assert!(rig.here("Kept").is_some());
}

#[test]
fn a_message_moved_on_a_server_that_names_mail_by_place_leaves_where_it_was_and_comes_where_it_went() {
    let mut rig = Rig::new();
    rig.server.with(|d| d.profile = Profile::by_place());
    let id = rig.deliver(&rig.inbox, "Moving", &["$seen"], 1);
    rig.run(false);
    let moved = rig.server.with(|d| d.move_to(&id, &rig.projects)).unwrap();
    assert_ne!(moved, id);
    rig.run(false);
    let m = rig.here("Moving").unwrap();
    assert_eq!((m.folder, m.server.as_ref().map(|s| s.id.as_str()), m.read), (rig.folder(&rig.projects), Some(moved.as_str()), true));
    assert_eq!(rig.s.mailbox.messages.values().filter(|m| m.subject == "Moving").count(), 1);
}

#[test]
fn a_message_opened_comes_whole_and_the_background_takes_the_inboxs_newest() {
    let mut rig = Rig::new();
    rig.deliver(&rig.inbox, "Agenda", &[], 1);
    rig.deliver(&rig.apollo, "Plan", &[], 2);
    rig.run(false);
    let fetch_whole = |rig: &mut Rig, message: Option<Id>| {
        let request = jmap_fetch_request(&rig.s, rig.account, message, 10).unwrap();
        let connection = rig.connect().unwrap();
        let result = block_on(fetch::fetch_messages(&connection, &request, &AtomicBool::new(false)));
        apply_jmap_fetch(&mut rig.s, result).unwrap()
    };
    // Apollo's, opened: whole at once.
    let plan = rig.here("Plan").unwrap().id;
    assert_eq!(fetch_whole(&mut rig, Some(plan))["fetched"].as_u64(), Some(1));
    let m = rig.s.mailbox.message(plan).unwrap();
    assert!(!m.unfetched && m.body.text.contains("Hello about Plan."), "{}", m.body.text);
    // The background: the Inbox's, and then nothing.
    assert_eq!(fetch_whole(&mut rig, None)["fetched"].as_u64(), Some(1));
    assert!(!rig.here("Agenda").unwrap().unfetched);
    assert!(jmap_fetch_request(&rig.s, rig.account, None, 10).unwrap().ids.is_empty());
}
