// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #276: an IMAP account whose Inbox the mailbox already has reads it before the folders are
//! listed, so that new mail is on screen first; one with changes to push, or a first run, goes in
//! its old order.

use mailcraft_model::sync::{FolderState, RemoteOp};
use mailcraft_model::{AccountKind, Server};

use super::*;
use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};

const PASSWORD: &str = "hunter2";

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
        messages: uids.iter().map(|u| message(*u)).collect(),
        refuse_open: false,
        next_uid: 0,
    }
}

fn world() -> ImapWorld {
    let mailboxes = vec![mailbox("INBOX", "\\HasNoChildren", &[1, 2]), mailbox("Sent", "\\Sent", &[1]), mailbox("Archive", "\\Archive", &[1, 2, 3])];
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

/// What a run says, piece by piece.
fn pieces(request: &SyncRequest) -> Vec<SyncResult> {
    let mut all = Vec::new();
    sync_as(request, Limits::in_pieces(), Host::default(), &AtomicBool::new(false), &mut |piece| all.push(piece));
    all
}

fn whole(pieces: &[SyncResult]) -> SyncResult {
    let mut whole = SyncResult::default();
    for piece in pieces {
        whole.absorb(piece.clone());
    }
    whole
}

/// The mailbox's folders after `pieces` were taken in.
fn states(pieces: &[SyncResult]) -> Vec<FolderState> {
    let mut by_remote: BTreeMap<String, FolderState> = BTreeMap::new();
    for (n, delta) in pieces.iter().flat_map(|p| &p.folders).enumerate() {
        let state = by_remote.entry(delta.remote.clone()).or_insert_with(|| FolderState {
            id: 100 + n as Id,
            remote: delta.remote.clone(),
            kind: delta.kind,
            uid_validity: delta.uid_validity,
            ..Default::default()
        });
        state.uid_seen = state.uid_seen.max(delta.uid_seen);
        state.known_uids.extend(delta.new.iter().map(|m| m.uid));
    }
    by_remote.into_values().collect()
}

/// Where in the commands received the first with `text` in it comes.
fn at(imap: &FakeImap, text: &str) -> Option<usize> {
    imap.world().log.iter().position(|line| line.contains(text))
}

#[test]
fn the_inbox_is_read_before_the_folders_are_listed() {
    let imap = Fake::imap(world());
    let mut request = request(&imap);
    let first = pieces(&request);
    // A first run has no Inbox here yet: the folders are listed first, as ever.
    assert!(at(&imap, " LIST ") < at(&imap, " EXAMINE "));
    request.folders = states(&first);
    imap.world().mailbox("INBOX").messages.push(message(3));
    imap.world().log.clear();

    let second = pieces(&request);
    assert!(at(&imap, " EXAMINE \"INBOX\"") < at(&imap, " LIST "), "{:?}", imap.world().log);
    // The Inbox is read once, not again in its turn among the folders.
    assert_eq!(imap.world().log.iter().filter(|line| line.contains(" EXAMINE \"INBOX\"")).count(), 1);
    // Its new mail comes in a piece before the one that says what the folders are.
    let with_mail = second.iter().position(|p| p.folders.iter().any(|f| f.remote == "INBOX" && f.new.iter().any(|m| m.uid == 3)));
    let listing = second.iter().position(|p| !p.listed.is_empty());
    assert!(with_mail.is_some() && with_mail < listing, "{with_mail:?} {listing:?}");
    let all = whole(&second);
    assert!(all.errors.is_empty(), "{:?}", all.errors);
    // Everything else was read as before, and the Inbox once.
    assert_eq!(all.folders.iter().filter(|f| f.remote == "INBOX").count(), 1);
    assert!(all.folders.iter().any(|f| f.remote == "Archive") && all.folders.iter().any(|f| f.remote == "Sent"));
    assert_eq!(all.new_count(), 1);
}

#[test]
fn a_run_with_changes_to_push_goes_in_its_old_order() {
    let imap = Fake::imap(world());
    let mut request = request(&imap);
    request.folders = states(&pieces(&request));
    imap.world().log.clear();
    request.ops = vec![RemoteOp::Flags { folder: "INBOX".into(), uid: 1, read: Some(true), flagged: None, answered: None }];
    let run = whole(&pieces(&request));
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    assert!(at(&imap, " LIST ") < at(&imap, " EXAMINE "), "{:?}", imap.world().log);
    // The change went up, and the Inbox shows it.
    assert!(imap.world().mailbox("INBOX").messages.iter().any(|m| m.uid == 1 && m.has("\\Seen")));
}
