// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #276: an IMAP folder whose STATUS is what it was when it was last read whole is passed over by
//! a Send/Receive: its mailbox isn't opened and its messages aren't listed.

use mailcraft_model::sync::FolderState;
use mailcraft_model::{AccountKind, ImapStatus, Server};

use super::*;
use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};

const PASSWORD: &str = "hunter2";

fn message(uid: u32, flags: &str) -> FakeMessage {
    FakeMessage {
        uid,
        flags: flags.into(),
        internal_date: " 1-Jul-2003 09:52:37 +0000".into(),
        body: format!("From: Bob <bob@example.org>\r\nSubject: Hello {uid}\r\nMessage-ID: <m{uid}@example.org>\r\n\r\nHi.\r\n").into_bytes(),
        claimed_size: None,
    }
}

fn mailbox(raw: &str, flags: &str, uids: &[u32]) -> FakeMailbox {
    FakeMailbox {
        raw: raw.into(),
        flags: flags.into(),
        uid_validity: 7,
        messages: uids.iter().map(|uid| message(*uid, "")).collect(),
        refuse_open: false,
        next_uid: 0,
    }
}

/// An Inbox, Sent, and `folders` more, each with three messages.
fn world(folders: usize) -> ImapWorld {
    let mut mailboxes = vec![mailbox("INBOX", "\\HasNoChildren", &[1, 2]), mailbox("Sent", "\\Sent", &[1, 2, 3])];
    for n in 0..folders {
        mailboxes.push(mailbox(&format!("Label{n}"), "\\HasNoChildren", &[1, 2, 3]));
    }
    ImapWorld { user: "ada".into(), password: PASSWORD.into(), mailboxes, ..Default::default() }
}

fn request(imap: &FakeImap) -> SyncRequest {
    let account = Account {
        id: 7,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        username: "ada".into(),
        ..Default::default()
    };
    SyncRequest { account, password: PASSWORD.into(), ..Default::default() }
}

/// The folders as a mailbox would hold them after `result` was taken in, with `at` as the time
/// of the reading.
fn states(result: &SyncResult, at: i64) -> Vec<FolderState> {
    let mut by_remote: BTreeMap<String, FolderState> = BTreeMap::new();
    for (n, delta) in result.folders.iter().enumerate() {
        let state = by_remote.entry(delta.remote.clone()).or_insert_with(|| FolderState {
            id: 100 + n as Id,
            remote: delta.remote.clone(),
            kind: delta.kind,
            uid_validity: delta.uid_validity,
            ..Default::default()
        });
        state.uid_seen = state.uid_seen.max(delta.uid_seen);
        state.uid_from = delta.uid_from.max(state.uid_from);
        state.known_uids.extend(delta.new.iter().map(|m| m.uid));
        if delta.complete {
            state.status = delta.status.map(|status| ImapStatus { at, ..status });
        }
    }
    by_remote.into_values().collect()
}

/// How many mailboxes the server was asked to open.
fn opened(imap: &FakeImap) -> usize {
    imap.world().log.iter().filter(|line| line.contains(" EXAMINE ") || line.contains(" SELECT ")).count()
}

fn run(request: &SyncRequest) -> SyncResult {
    let mut whole = SyncResult::default();
    sync_as(request, Limits::in_pieces(), Host::default(), &AtomicBool::new(false), &mut |piece| whole.absorb(piece));
    whole
}

#[test]
fn a_folder_read_whole_and_unchanged_is_passed_over() {
    let imap = Fake::imap(world(6));
    let mut request = request(&imap);
    let first = run(&request);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(first.timings.skipped, 0);
    assert_eq!(opened(&imap), 8);
    // The Inbox is read every time; its status is never kept.
    assert!(first.folders.iter().filter(|f| f.remote == "INBOX").all(|f| f.status.is_none()));

    request.folders = states(&first, 1_000);
    let second = run(&request);
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(second.timings.skipped, 7);
    // Only the Inbox was opened again.
    assert_eq!(opened(&imap), 9);
    assert_eq!(second.folders.len(), 1);
    // Fewer requests than the first run: the folders' statuses came in one go.
    assert!(second.timings.requests() < first.timings.requests() / 2, "{} against {}", second.timings.requests(), first.timings.requests());
}

#[test]
fn a_folder_that_changed_is_read_and_the_others_still_are_not() {
    let imap = Fake::imap(world(3));
    let mut request = request(&imap);
    request.folders = states(&run(&request), 1_000);
    {
        let mut world = imap.world();
        // New mail in one, a message read in another, one removed from a third.
        world.mailbox("Label0").messages.push(message(4, ""));
        world.mailbox("Label1").messages[0].flags = "\\Seen".into();
        world.mailbox("Label2").messages.pop();
    }
    let result = run(&request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.timings.skipped, 1, "only Sent is as it was");
    let remotes: Vec<&str> = result.folders.iter().map(|f| f.remote.as_str()).collect();
    assert!(["Label0", "Label1", "Label2"].iter().all(|r| remotes.contains(r)), "{remotes:?}");
    assert!(!remotes.contains(&"Sent"));
    let label2 = result.folders.iter().find(|f| f.remote == "Label2").unwrap();
    assert_eq!(label2.removed, [3]);
}

#[test]
fn a_new_numbering_is_read_again() {
    let imap = Fake::imap(world(1));
    let mut request = request(&imap);
    request.folders = states(&run(&request), 1_000);
    imap.world().mailbox("Label0").uid_validity = 8;
    let result = run(&request);
    assert!(result.folders.iter().any(|f| f.remote == "Label0" && f.uid_validity == 8));
    assert_eq!(result.timings.skipped, 1);
}

#[test]
fn what_must_still_be_done_is_not_passed_over() {
    let imap = Fake::imap(world(2));
    let mut request = request(&imap);
    request.folders = states(&run(&request), 1_000);
    // A message of Label0 is to be downloaded again; Label1 is read for a different limit; Sent
    // has no status kept.
    for f in &mut request.folders {
        match f.remote.as_str() {
            "Label0" => f.again = vec![1],
            "Sent" => f.status = None,
            _ => {}
        }
    }
    request.max_per_folder = 50;
    let result = run(&request);
    assert_eq!(result.timings.skipped, 0);
    let remotes: Vec<&str> = result.folders.iter().map(|f| f.remote.as_str()).collect();
    assert!(["Label0", "Label1", "Sent"].iter().all(|r| remotes.contains(r)), "{remotes:?}");
}

#[test]
fn a_server_without_status_has_every_folder_read() {
    let mut world = world(3);
    world.no_status = true;
    let imap = Fake::imap(world);
    let mut request = request(&imap);
    let first = run(&request);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    request.folders = states(&first, 1_000);
    let second = run(&request);
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(second.timings.skipped, 0);
    assert_eq!(second.folders.len(), 5);
}

#[test]
fn a_folder_that_was_cut_short_keeps_no_status() {
    let imap = Fake::imap(world(1));
    let request = request(&imap);
    // Fewer bytes than a message has: the folder can't be read to its end.
    let mut limits = Limits::in_pieces();
    limits.messages = 1;
    let mut whole = SyncResult::default();
    sync_as(&request, limits, Host::default(), &AtomicBool::new(false), &mut |piece| whole.absorb(piece));
    let cut: Vec<_> = whole.folders.iter().filter(|f| f.remote == "Label0").collect();
    assert!(cut.iter().all(|f| !f.complete && f.status.is_none()));
}

#[test]
fn a_folder_this_run_changed_on_the_server_is_read_whatever_its_counts_say() {
    use mailcraft_model::sync::RemoteOp;
    // A message deleted here is marked, not expunged, on a server without UIDPLUS: no count of
    // its folder moves, and the folder has to be read to show it gone.
    let imap = Fake::imap(world(1));
    let mut request = request(&imap);
    request.folders = states(&run(&request), 1_000);
    request.ops = vec![RemoteOp::Delete { from: "Label0".into(), uid: 2 }];
    let result = run(&request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.folders.iter().any(|f| f.remote == "Label0"), "the folder the change was made in was passed over");
    // Sent, which nothing touched, still is passed over.
    assert!(!result.folders.iter().any(|f| f.remote == "Sent"));
    assert_eq!(result.timings.skipped, 1);
}
