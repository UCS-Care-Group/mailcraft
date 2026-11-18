// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1581: one message that can't be downloaded doesn't stop the others, or the folders after it;
//! it is tried a few runs and then left out, said once, until the owner asks for it again.

use mailcraft_model::sync::{FolderState, MAX_TRIES};
use mailcraft_model::{AccountKind, Server};

use super::*;
use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};

const PASSWORD: &str = "hunter2";
const BAD: u32 = 7;

fn message(uid: u32) -> FakeMessage {
    FakeMessage {
        uid,
        flags: String::new(),
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
        messages: uids.iter().copied().map(message).collect(),
        refuse_open: false,
        next_uid: 0,
    }
}

/// An Inbox, a Sent folder, and Label0 (where UID 7 is) and Label1 after it.
fn world() -> ImapWorld {
    let mailboxes = vec![
        mailbox("INBOX", "\\HasNoChildren", &[1, 2]),
        mailbox("Sent", "\\Sent", &[1, 2, 3]),
        mailbox("Label0", "\\HasNoChildren", &[1, BAD, 9]),
        mailbox("Label1", "\\HasNoChildren", &[1, 2, 3]),
    ];
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

fn run(request: &SyncRequest) -> SyncResult {
    let mut whole = SyncResult::default();
    sync_as(request, Limits::in_pieces(), Host::default(), &AtomicBool::new(false), &mut |piece| whole.absorb(piece));
    whole
}

/// What the mailbox would hold of each folder after `result` was taken in: how far it was read,
/// what it has, and (as the engine counts them) which messages failed in how many runs.
fn carry(request: &mut SyncRequest, result: &SyncResult) {
    for delta in &result.folders {
        let at = request.folders.iter().position(|f| f.remote == delta.remote).unwrap_or_else(|| {
            request.folders.push(FolderState {
                id: 100 + request.folders.len() as Id,
                remote: delta.remote.clone(),
                kind: delta.kind,
                ..Default::default()
            });
            request.folders.len() - 1
        });
        let state = &mut request.folders[at];
        state.uid_validity = delta.uid_validity;
        state.uid_seen = state.uid_seen.max(delta.uid_seen);
        state.uid_from = delta.uid_from.max(state.uid_from);
        state.known_uids.extend(delta.new.iter().map(|m| m.uid));
        for uid in &delta.new {
            state.failing.remove(&uid.uid);
        }
        for uid in &delta.failed {
            let tries = state.failing.entry(*uid).or_insert(0);
            *tries = tries.saturating_add(1).min(MAX_TRIES);
        }
    }
}

fn new_in(result: &SyncResult, remote: &str) -> Vec<u32> {
    result.folders.iter().filter(|f| f.remote == remote).flat_map(|f| f.new.iter().map(|m| m.uid)).collect()
}

fn failed_in(result: &SyncResult, remote: &str) -> Vec<u32> {
    result.folders.iter().filter(|f| f.remote == remote).flat_map(|f| f.failed.iter().copied()).collect()
}

/// How many times the server was asked for the whole of `uid`.
fn asked_for(imap: &FakeImap, uid: u32) -> usize {
    let fetch = format!("UID FETCH {uid} ");
    imap.world().log.iter().filter(|line| line.contains(&fetch) && line.contains("BODY.PEEK[]")).count()
}

#[test]
fn a_message_that_breaks_the_connection_does_not_stop_the_rest() {
    let imap = Fake::imap(world());
    imap.world().hang_up_at_uid = Some(BAD);
    let result = run(&request(&imap));
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors[0].starts_with("Couldn't download a message from \"Label0\""), "{:?}", result.errors);
    // The messages around it came, and so did the folders after it.
    assert_eq!(new_in(&result, "Label0"), [1, 9]);
    assert_eq!(new_in(&result, "Label1"), [1, 2, 3]);
    assert_eq!(new_in(&result, "Sent"), [1, 2, 3]);
    // It is held against that message alone.
    assert_eq!(failed_in(&result, "Label0"), [BAD]);
    assert!(result.folders.iter().filter(|f| f.remote != "Label0").all(|f| f.failed.is_empty()));
}

#[test]
fn a_message_the_server_refuses_is_counted_and_the_rest_come() {
    let imap = Fake::imap(world());
    imap.world().refuse_uid = Some(BAD);
    let result = run(&request(&imap));
    assert_eq!(new_in(&result, "Label0"), [1, 9]);
    assert_eq!(new_in(&result, "Label1"), [1, 2, 3]);
    assert_eq!(failed_in(&result, "Label0"), [BAD]);
}

#[test]
fn a_message_that_keeps_failing_is_left_out_after_a_few_runs_and_said_once() {
    let imap = Fake::imap(world());
    imap.world().hang_up_at_uid = Some(BAD);
    let mut request = request(&imap);
    let mut said_left_out = 0;
    for run_no in 1..=MAX_TRIES {
        let result = run(&request);
        assert_eq!(failed_in(&result, "Label0"), [BAD], "run {run_no}");
        said_left_out += result.errors.iter().filter(|e| e.contains("Leaving out 1 message of")).count();
        carry(&mut request, &result);
    }
    assert_eq!(said_left_out, 1, "said at the last try, and only then");
    assert_eq!(asked_for(&imap, BAD), usize::from(MAX_TRIES));

    // Then it is passed over: no new try, no error, the folder is read whole, nothing after it waits.
    let after = run(&request);
    assert_eq!(after.errors, Vec::<String>::new());
    assert_eq!(asked_for(&imap, BAD), usize::from(MAX_TRIES));
    let label0 = after.folders.iter().find(|f| f.remote == "Label0").unwrap();
    assert!(label0.complete && label0.failed.is_empty() && label0.uid_seen == 9, "{label0:?}");
    // Nothing was lost on the server for it.
    assert!(imap.world().mailbox("Label0").messages.iter().any(|m| m.uid == BAD));
}

#[test]
fn a_message_left_out_is_tried_again_when_its_count_is_cleared() {
    let imap = Fake::imap(world());
    imap.world().hang_up_at_uid = Some(BAD);
    let mut request = request(&imap);
    for _ in 0..MAX_TRIES {
        let result = run(&request);
        carry(&mut request, &result);
    }
    // A run that leaves it out reads the folder on to its end, and the folders are read whole.
    let past = run(&request);
    carry(&mut request, &past);
    request.folders.iter_mut().for_each(|f| f.status = None);
    // The server is mended, and the owner asks (`sync.retryFailed` takes the counts back to 0).
    imap.world().hang_up_at_uid = None;
    request.folders.iter_mut().flat_map(|f| f.failing.values_mut()).for_each(|tries| *tries = 0);
    let result = run(&request);
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(new_in(&result, "Label0"), [BAD]);
}

#[test]
fn a_message_to_be_tried_again_is_not_passed_over_for_an_unchanged_folder() {
    let imap = Fake::imap(world());
    let mut request = request(&imap);
    let first = run(&request);
    carry(&mut request, &first);
    for f in &mut request.folders {
        f.status = first.folders.iter().find(|d| d.remote == f.remote).and_then(|d| d.status);
    }
    imap.world().hang_up_at_uid = None;
    let label0 = request.folders.iter_mut().find(|f| f.remote == "Label0").unwrap();
    // As if its message 9 had failed and been taken up again: the folder is as the server's counts say.
    label0.known_uids.retain(|uid| *uid != 9);
    label0.failing.insert(9, 0);
    let result = run(&request);
    assert_eq!(new_in(&result, "Label0"), [9]);
}

#[test]
fn counts_of_a_folder_numbered_anew_mean_nothing() {
    let imap = Fake::imap(world());
    let mut request = request(&imap);
    let first = run(&request);
    carry(&mut request, &first);
    // The server numbers Label0 anew, and a message of the new numbering has the UID that had failed.
    imap.world().mailbox("Label0").uid_validity = 8;
    let label0 = request.folders.iter_mut().find(|f| f.remote == "Label0").unwrap();
    label0.failing.insert(BAD, MAX_TRIES);
    let result = run(&request);
    assert_eq!(new_in(&result, "Label0"), [1, BAD, 9]);
}

#[test]
fn a_line_that_is_bad_for_every_message_stops_the_run_and_blames_nobody() {
    let imap = Fake::imap(world());
    imap.world().hang_up_on = Some("BODY.PEEK[]".into());
    let result = run(&request(&imap));
    // The first message, a fresh connection, the next message: that is a bad line.
    assert!(result.errors.len() > 1, "{:?}", result.errors);
    assert!(result.folders.iter().all(|f| f.failed.is_empty() && f.new.is_empty()), "{:?}", result.folders);
    assert!(!result.folders.iter().any(|f| f.remote == "Label1"));
}

#[test]
fn counts_of_messages_the_server_lost_or_the_limit_leaves_out_are_dropped() {
    let imap = Fake::imap(world());
    let mut request = request(&imap);
    let first = run(&request);
    carry(&mut request, &first);
    let label0 = request.folders.iter_mut().find(|f| f.remote == "Label0").unwrap();
    // 4 is not on the server any more; 1 is below what a limit of two takes.
    label0.failing.extend([(4, 1), (1, 1)]);
    label0.known_uids.retain(|uid| *uid != 1);
    request.max_per_folder = 2;
    let result = run(&request);
    let label0 = result.folders.iter().find(|f| f.remote == "Label0").unwrap();
    let mut gone = label0.failing_gone.clone();
    gone.sort_unstable();
    assert_eq!(gone, [1, 4]);
    assert!(label0.new.is_empty() && label0.removed.is_empty(), "{label0:?}");
}

/// Label0 with two messages in a row that break the connection, one cut and one hung up.
fn adjacent_world() -> ImapWorld {
    let mut world = world();
    world.mailbox("Label0").messages = [1, 6, 7, 9].into_iter().map(message).collect();
    world
}

#[test]
fn adjacent_messages_that_break_the_connection_are_both_left_out_and_the_rest_come() {
    let imap = Fake::imap(adjacent_world());
    imap.world().cut_at_uid = Some(6);
    imap.world().hang_up_at_uid = Some(7);
    let mut request = request(&imap);
    for run_no in 1..=MAX_TRIES {
        let result = run(&request);
        assert_eq!(failed_in(&result, "Label0"), [6, 7], "run {run_no}");
        if run_no == 1 {
            assert_eq!(new_in(&result, "Label0"), [1, 9]);
            assert_eq!(new_in(&result, "Label1"), [1, 2, 3]);
        }
        carry(&mut request, &result);
    }
    let after = run(&request);
    assert_eq!(after.errors, Vec::<String>::new());
    let label0 = after.folders.iter().find(|f| f.remote == "Label0").unwrap();
    assert!(label0.complete && label0.failed.is_empty(), "{label0:?}");
}

#[test]
fn a_server_that_refuses_every_download_for_a_while_blames_nobody() {
    let imap = Fake::imap(world());
    imap.world().refuse = vec!["BODY.PEEK[]".into()];
    let mut request = request(&imap);
    for run_no in 1..=MAX_TRIES + 1 {
        let result = run(&request);
        assert!(result.folders.iter().all(|f| f.failed.is_empty()), "run {run_no}: {:?}", result.folders);
        carry(&mut request, &result);
    }
    imap.world().refuse.clear();
    let result = run(&request);
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(new_in(&result, "INBOX"), [1, 2]);
    assert_eq!(new_in(&result, "Label0"), [1, BAD, 9]);
    assert_eq!(new_in(&result, "Label1"), [1, 2, 3]);
}
