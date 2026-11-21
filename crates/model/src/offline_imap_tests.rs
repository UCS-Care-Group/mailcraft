// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail to keep offline for IMAP accounts (#653): which of their messages are within, how the
//! server knows one, and IMAP accounts from before the window keeping all their mail.

use super::*;
use crate::sync::ImapFetchKey;
use crate::{Account, Folder, Pending};

const NOW: Timestamp = 1_792_000_000; // 2026-10-15
const OLD: Timestamp = NOW - 400 * 86_400;

/// An IMAP account keeping `keep`, its Inbox following the server's `INBOX` (UIDVALIDITY 7).
fn mailbox(keep: KeepOffline) -> (Mailbox, Id, Id) {
    let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
    let account = mb.add_account(Account { email: "noor@example.org".into(), kind: AccountKind::Imap, keep_offline: keep, ..Default::default() });
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let f = mb.folder_mut(inbox).unwrap();
    (f.remote, f.uid_validity) = ("INBOX".into(), 7);
    (mb, account, inbox)
}

fn message(account: Id, folder: Id, uid: u32, received: Timestamp) -> Message {
    Message { account, folder, uid, received, ..Default::default() }
}

#[test]
fn imap_mail_outside_by_received_date() {
    let (mb, account, inbox) = mailbox(KeepOffline::Months12);
    let w = OfflineWindow::of(&mb, NOW);
    assert!(!w.keeps_all());
    assert!(w.within(&message(account, inbox, 5, NOW - 86_400)));
    assert!(!w.within(&message(account, inbox, 5, OLD)), "older than twelve months");
    assert!(w.within(&message(account, inbox, 5, 0)), "no date");
    assert_eq!(w.since_of(account), KeepOffline::Months12.since(NOW));
    // All keeps every message, as before.
    let (mb, account, inbox) = mailbox(KeepOffline::All);
    assert!(OfflineWindow::of(&mb, NOW).keeps_all());
    assert!(OfflineWindow::of(&mb, NOW).within(&message(account, inbox, 5, OLD)));
}

#[test]
fn what_an_imap_server_lacks_is_always_within() {
    let (mut mb, account, inbox) = mailbox(KeepOffline::Days3);
    let drafts = mb.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox).unwrap();
    mb.folder_mut(drafts).unwrap().remote = "Drafts".into();
    let here_only = mb.alloc_id();
    mb.folders.push(Folder { id: here_only, account, name: "Here only".into(), ..Default::default() });
    // Deleted or renamed on the server by another program: its last listing didn't name it.
    let unlisted = mb.alloc_id();
    mb.folders.push(Folder { id: unlisted, account, name: "Gone there".into(), remote: "Gone there".into(), unlisted: true, ..Default::default() });
    let w = OfflineWindow::of(&mb, NOW);
    let base = message(account, inbox, 5, OLD);
    assert!(!w.within(&base), "the plain case is outside");
    for folder in [drafts, outbox, here_only, unlisted] {
        assert!(w.within(&Message { folder, ..base.clone() }), "folder {folder}");
    }
    assert!(w.within(&Message { uid: 0, ..base.clone() }), "no UID: moved here, not there yet");
    assert!(w.within(&Message { draft: true, ..base.clone() }));
    assert!(w.within(&Message { held: "held".into(), ..base.clone() }));
    assert!(w.within(&Message { incomplete: true, ..base.clone() }));
    assert!(w.within(&Message { not_downloaded: 30_000_000, ..base.clone() }), "too large to download: the note stays");
    assert!(w.within(&Message { refetch: true, ..base.clone() }), "waits to be downloaded again (#650)");
    assert!(w.within(&Message { returned: 2, ..base.clone() }), "put back after a refused copy: may be all there is (#451)");
}

#[test]
fn an_imap_message_moving_out_of_its_account_is_within() {
    let (mut mb, account, inbox) = mailbox(KeepOffline::Days3);
    let local = mb.accounts[0].id;
    let local_inbox = mb.folder_of_kind(local, FolderKind::Inbox).unwrap();
    // Moved into a local folder: its original goes from the server only once it is safe here.
    let id = mb.add_message(message(account, inbox, 5, OLD)).unwrap();
    let moved = mb.add_message(Message { account: local, folder: local_inbox, ..message(local, local_inbox, 0, OLD) }).unwrap();
    mb.pending.push(Pending::Move { account, message: moved, from: "INBOX".into(), uid: 9, synced: None, to: local_inbox });
    let w = OfflineWindow::of(&mb, NOW);
    assert!(!w.within(mb.message(id).unwrap()));
    assert!(w.within(mb.message(moved).unwrap()));
}

#[test]
fn the_server_knows_an_imap_message_by_mailbox_validity_and_uid() {
    let (mut mb, account, inbox) = mailbox(KeepOffline::Months3);
    let m = message(account, inbox, 42, OLD);
    let key = mb.imap_key(&m).unwrap();
    assert_eq!(key, ImapFetchKey { remote: "INBOX".into(), uid_validity: 7, uid: 42 });
    assert_eq!(ImapFetchKey::read(&key.write()), Some(key));
    // A path with the hierarchy's delimiter in it reads back whole.
    let nested = ImapFetchKey { remote: "[Gmail]/All Mail".into(), uid_validity: 1, uid: 3 };
    assert_eq!(ImapFetchKey::read(&nested.write()), Some(nested));
    for junk in ["", "7", "7/42", "7/0/INBOX", "x/1/INBOX", "7/42/"] {
        assert_eq!(ImapFetchKey::read(junk), None, "{junk:?}");
    }
    assert_eq!(mb.imap_key(&Message { uid: 0, ..m.clone() }), None);
    // Fetchable only while it isn't whole.
    assert!(!mb.fetchable(&m));
    assert!(mb.fetchable(&Message { unfetched: true, ..m.clone() }));
    // A local account's message has no key.
    let local = mb.accounts[0].id;
    let local_inbox = mb.folder_of_kind(local, FolderKind::Inbox).unwrap();
    mb.folder_mut(local_inbox).unwrap().remote = "INBOX".into();
    assert_eq!(mb.imap_key(&message(local, local_inbox, 42, OLD)), None);
}

#[test]
fn imap_accounts_from_before_keep_all_their_mail_once() {
    let (mut mb, account, _) = mailbox(KeepOffline::Months12);
    // A new mailbox has it from the start: a window chosen there is never undone by it.
    assert!(mb.imap_windowed);
    assert_eq!(mb.keep_imap_mail_as_it_was(), 0);
    assert_eq!(mb.keep_offline_of(account), KeepOffline::Months12);
    // As a file from before reads.
    mb.imap_windowed = false;
    assert_eq!(mb.keep_imap_mail_as_it_was(), 1);
    assert_eq!(mb.keep_offline_of(account), KeepOffline::All);
    assert!(mb.imap_windowed);
    // Once only: a window chosen since stays.
    mb.accounts.iter_mut().find(|a| a.id == account).unwrap().keep_offline = KeepOffline::Months3;
    assert_eq!(mb.keep_imap_mail_as_it_was(), 0);
    assert_eq!(mb.keep_offline_of(account), KeepOffline::Months3);
    // Kept in the file; a file from before has none.
    let json = serde_json::to_string(&mb).unwrap();
    assert!(json.contains(r#""imapWindowed":true"#), "{json}");
    assert!(!serde_json::from_str::<Mailbox>("{}").unwrap().imap_windowed);
}

#[test]
fn a_microsoft_account_keeps_its_window_through_the_once() {
    let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
    let ms = mb.add_account(Account {
        email: "noor@example.org".into(),
        kind: AccountKind::Microsoft,
        keep_offline: KeepOffline::Week1,
        ..Default::default()
    });
    assert_eq!(mb.keep_imap_mail_as_it_was(), 0);
    assert_eq!(mb.keep_offline_of(ms), KeepOffline::Week1);
}
