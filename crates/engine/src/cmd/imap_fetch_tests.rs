// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail to keep offline for IMAP accounts (#653): older mail goes back to its headers and
//! preview, is fetched whole again by its UID, and is never moved or copied as what is here of it.

use mailcraft_model::sync::{GraphFetchResult, GraphFetched, ImapFetchKey};
use mailcraft_model::{Body, FolderKind, Id, KeepOffline, Mailbox, Message};
use serde_json::json;

use crate::Session;

/// 2027-01-15.
const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

/// A session at [`NOW`] with an IMAP account keeping `keep`, whose Inbox follows the server's
/// `INBOX` (UIDVALIDITY 7) and holds a message whole received each of `days_ago`, by UID from 1.
fn aged(keep: &str, days_ago: &[i64]) -> (Session, Id, Vec<Id>) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account =
        s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    s.run("account.edit", &json!({"id": account, "keepOffline": keep})).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let f = s.mailbox.folder_mut(inbox).unwrap();
    (f.remote, f.uid_validity) = ("INBOX".into(), 7);
    let ids = days_ago
        .iter()
        .zip(1u32..)
        .map(|(d, uid)| {
            let body = Body { text: format!("Received {d} days ago. ").repeat(100), html: None };
            let m = Message { account, folder: inbox, uid, received: NOW - d * DAY, subject: format!("Day {d}"), body, ..Default::default() };
            s.mailbox.add_message(m).unwrap()
        })
        .collect();
    (s, account, ids)
}

fn mime(subject: &str) -> Vec<u8> {
    format!("From: Bob <bob@example.org>\r\nSubject: {subject}\r\nMessage-ID: <x@example.org>\r\n\r\nThe whole of {subject}, fetched again.\r\n")
        .into_bytes()
}

fn key(uid: u32) -> String {
    ImapFetchKey { remote: "INBOX".into(), uid_validity: 7, uid }.write()
}

#[test]
fn an_imap_account_keeps_a_year_and_older_mail_goes_back_to_its_preview() {
    let (mut s, account, ids) = aged("12m", &[1, 400]);
    assert_eq!(s.mailbox.keep_offline_of(account), KeepOffline::Months12);
    let size = s.run("account.offlineSize", &json!({"account": account, "keepOffline": "1m"})).unwrap();
    assert_eq!(size["messages"], 1);
    let said = s.run("account.thinOffline", &json!({"account": account})).unwrap();
    assert_eq!(said["thinned"], 1, "{said}");
    let (recent, old) = (s.mailbox.message(ids[0]).unwrap(), s.mailbox.message(ids[1]).unwrap());
    assert!(!recent.unfetched && recent.body.text.len() > 1_000);
    assert!(old.unfetched && old.body.text.starts_with("Received 400 days ago.") && old.body.text.chars().count() <= 256, "{:?}", old.body.text);
    // Where it is and what the server knows it by stay.
    assert_eq!((old.uid, old.subject.as_str()), (2, "Day 400"));
    assert!(s.mailbox.fetchable(old));
}

#[test]
fn a_message_fetched_by_its_uid_is_whole_again() {
    let (mut s, account, ids) = aged("1m", &[1, 400, 500]);
    s.run("account.thinOffline", &json!({})).unwrap();
    // Opened: that one is asked for at once, by mailbox, validity and UID.
    let asked = s.run("sync.imapFetchRequest", &json!({"account": account, "message": ids[1]})).unwrap();
    assert_eq!(asked["ids"], json!([key(2)]));
    assert_eq!(asked["account"]["email"], "ada@mail.example");
    assert!(asked.get("background").is_none());
    // A message that is whole here is not asked for.
    assert_eq!(s.run("sync.imapFetchRequest", &json!({"account": account, "message": ids[0]})).unwrap()["ids"], json!([]));
    // Outside the window, the background leaves them for when they are opened.
    assert_eq!(s.run("sync.imapFetchRequest", &json!({"account": account})).unwrap()["ids"], json!([]));
    let result = GraphFetchResult {
        account,
        email: "ada@mail.example".into(),
        fetched: vec![GraphFetched { id: key(2), mime: mime("Day 400") }],
        ..Default::default()
    };
    let said = s.run("sync.applyImapFetch", &json!({"result": result})).unwrap();
    assert_eq!(said["fetched"], 1, "{said}");
    let m = s.mailbox.message(ids[1]).unwrap();
    assert!(!m.unfetched && m.body.text.contains("fetched again"));
    assert_eq!((m.uid, m.folder), (2, s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap()));
    // Opened this session, it stays whole until the mailbox is opened again.
    assert_eq!(s.run("account.thinOffline", &json!({})).unwrap()["thinned"], 0);
    assert!(s.mailbox.message(ids[2]).unwrap().unfetched);
}

#[test]
fn a_longer_window_fetches_the_newly_covered_mail_newest_first() {
    let (mut s, account, _) = aged("1m", &[1, 400, 500, 3_000]);
    s.run("account.thinOffline", &json!({})).unwrap();
    s.run("account.edit", &json!({"id": account, "keepOffline": "2y"})).unwrap();
    let asked = s.run("sync.imapFetchRequest", &json!({"account": account})).unwrap();
    assert_eq!(asked["ids"], json!([key(2), key(3)]));
    assert_eq!(asked["background"], true);
    // A message refused this session is passed over.
    let result = GraphFetchResult { account, email: "ada@mail.example".into(), failed: vec![key(2)], ..Default::default() };
    s.run("sync.applyImapFetch", &json!({"result": result})).unwrap();
    assert_eq!(s.run("sync.imapFetchRequest", &json!({"account": account})).unwrap()["ids"], json!([key(3)]));
    // Nor one gone from the server meanwhile: the next Send/Receive takes it away here.
    let result = GraphFetchResult { account, email: "ada@mail.example".into(), missing: vec![key(3)], ..Default::default() };
    s.run("sync.applyImapFetch", &json!({"result": result})).unwrap();
    assert_eq!(s.run("sync.imapFetchRequest", &json!({"account": account})).unwrap()["ids"], json!([]));
}

#[test]
fn what_came_for_another_mailbox_or_another_account_is_not_taken_in() {
    let (mut s, account, ids) = aged("1m", &[400]);
    s.run("account.thinOffline", &json!({})).unwrap();
    // Numbered anew since it was asked for: the UID is another message's.
    let stale = ImapFetchKey { remote: "INBOX".into(), uid_validity: 6, uid: 1 }.write();
    let result = GraphFetchResult {
        account,
        email: "ada@mail.example".into(),
        fetched: vec![GraphFetched { id: stale, mime: mime("Other") }],
        ..Default::default()
    };
    assert_eq!(s.run("sync.applyImapFetch", &json!({"result": result})).unwrap()["fetched"], 0);
    assert!(s.mailbox.message(ids[0]).unwrap().unfetched);
    let result = GraphFetchResult {
        account,
        email: "someone@else.example".into(),
        fetched: vec![GraphFetched { id: key(1), mime: mime("Other") }],
        ..Default::default()
    };
    assert!(s.run("sync.applyImapFetch", &json!({"result": result})).is_err());
    assert!(s.mailbox.message(ids[0]).unwrap().unfetched);
}

#[test]
fn a_preview_only_imap_message_is_neither_moved_nor_copied_out_of_its_account() {
    let (mut s, account, ids) = aged("1m", &[400]);
    s.run("account.thinOffline", &json!({})).unwrap();
    let local = s.run("account.add", &json!({"email": "here@mail.example"})).unwrap()["id"].as_u64().unwrap();
    let local_inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let moved = s.run("mail.move", &json!({"id": ids[0], "folder": local_inbox}));
    assert!(moved.unwrap_err().to_string().contains("isn't downloaded yet"));
    let copied = s.run("mail.copy", &json!({"id": ids[0], "folder": local_inbox}));
    assert!(copied.unwrap_err().to_string().contains("isn't downloaded yet"));
    // Nor into another IMAP account: the server's copy, the whole of it, is not to be deleted.
    let other =
        s.run("account.add", &json!({"email": "bea@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let other_inbox = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(other_inbox).unwrap().remote = "INBOX".into();
    let moved = s.run("mail.move", &json!({"id": ids[0], "folder": other_inbox}));
    assert!(moved.unwrap_err().to_string().contains("isn't downloaded yet"));
    assert!(s.mailbox.pending.is_empty() && s.mailbox.queued.is_empty());
    assert_eq!(s.mailbox.message(ids[0]).unwrap().folder, s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap());
    // Within its account it moves, as the server is told.
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    s.mailbox.folder_mut(archive).unwrap().remote = "Archive".into();
    s.run("mail.move", &json!({"id": ids[0], "folder": archive})).unwrap();
    assert_eq!(s.mailbox.message(ids[0]).unwrap().folder, archive);
    assert_eq!(s.mailbox.pending.len(), 1);
}

/// What an Outbox sends is the message, never the preview of one (#653): a message that was
/// thinned to its headers and preview can't be moved or copied into the IMAP Outbox, can't be
/// sent again from it, and is left out of what a Send/Receive sends.
#[test]
fn a_preview_is_not_sent_as_the_message() {
    use mailcraft_model::MOVED_INTO_OUTBOX;
    let (mut s, account, ids) = aged("1m", &[1, 400]);
    s.run("account.thinOffline", &json!({})).unwrap();
    let (recent, old) = (ids[0], ids[1]);
    assert!(s.mailbox.message(old).unwrap().unfetched && !s.mailbox.message(recent).unwrap().unfetched);
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let sent = |s: &Session| super::account::sync_request(s, account).unwrap().outbox.iter().map(|m| m.id).collect::<Vec<_>>();
    // Not into the Outbox, by a move or a copy.
    for command in ["mail.move", "mail.copy"] {
        let refused = s.run(command, &json!({"ids": [old], "folder": outbox})).unwrap_err();
        assert!(refused.to_string().contains("isn't downloaded yet"), "{command}: {refused}");
    }
    assert_eq!(s.mailbox.messages_in(outbox).count(), 0);
    assert!(s.mailbox.pending.is_empty() && s.mailbox.queued.is_empty());
    // A whole one still goes, is held, and goes with Send Again.
    s.run("mail.move", &json!({"ids": [recent], "folder": outbox})).unwrap();
    assert_eq!(s.mailbox.message(recent).unwrap().held, MOVED_INTO_OUTBOX);
    assert!(sent(&s).is_empty());
    s.run("mail.sendAgain", &json!({"id": recent})).unwrap();
    assert_eq!(sent(&s), vec![recent]);

    // One that got there some other way (a file from before, a listing) isn't sent again
    // either, nor sent by a run that is told of nothing else.
    let m = s.mailbox.message_mut(old).unwrap();
    (m.folder, m.held) = (outbox, MOVED_INTO_OUTBOX.to_string());
    let refused = s.run("mail.sendAgain", &json!({"id": old})).unwrap_err();
    assert!(refused.to_string().contains("isn't downloaded yet"), "{refused}");
    s.mailbox.message_mut(old).unwrap().held.clear();
    assert_eq!(sent(&s), vec![recent], "a preview is not asked to be sent");
}

#[test]
fn imap_accounts_from_before_keep_all_their_mail_when_the_mailbox_opens() {
    let (mut s, account, ids) = aged("12m", &[400]);
    // A mailbox made by this version keeps the window chosen in it.
    super::file::after_opening(&mut s);
    assert_eq!(s.mailbox.keep_offline_of(account), KeepOffline::Months12);
    // As a file from before reads.
    s.mailbox.imap_windowed = false;
    let after = super::file::after_opening(&mut s);
    assert_eq!(after.fetched_again(), 0);
    assert_eq!(s.mailbox.keep_offline_of(account), KeepOffline::All);
    assert_eq!(s.run("account.thinOffline", &json!({})).unwrap()["thinned"], 0);
    assert!(!s.mailbox.message(ids[0]).unwrap().unfetched);
    // Once: a window chosen after it is kept.
    s.run("account.edit", &json!({"id": account, "keepOffline": "1m"})).unwrap();
    super::file::after_opening(&mut s);
    assert_eq!(s.mailbox.keep_offline_of(account), KeepOffline::Month1);
}

#[test]
fn mail_that_came_as_its_start_is_kept_as_its_headers_and_preview() {
    use mailcraft_model::sync::{FolderDelta, SyncResult};
    let (mut s, account, _) = aged("1m", &[]);
    // What a synchronisation asks for: the window, by when mail arrived.
    let asked = super::account::sync_request(&s, account).unwrap();
    assert_eq!(asked.keep_since, KeepOffline::Month1.since(NOW));
    let start = Message {
        uid: 9,
        subject: "Old".into(),
        received: NOW - 400 * DAY,
        unfetched: true,
        attachments_on_server: true,
        body: Body { text: "The start of a long letter. ".repeat(500), html: None },
        ..Default::default()
    };
    let delta = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 9, new: vec![start], ..Default::default() };
    let result = SyncResult { account, email: "ada@mail.example".into(), folders: vec![delta], ..Default::default() };
    super::account::apply_sync(&mut s, result).unwrap();
    let m = s.mailbox.messages.values().find(|m| m.subject == "Old").unwrap();
    assert!(m.unfetched && m.has_attachments());
    assert!(
        m.body.text.starts_with("The start of a long letter.") && m.body.text.chars().count() <= mailcraft_model::PREVIEW_CHARS + 1,
        "{}",
        m.body.text.len()
    );
    assert_eq!(s.mailbox.imap_key(m), Some(ImapFetchKey { remote: "INBOX".into(), uid_validity: 7, uid: 9 }));
    // Opened, it is asked for by its UID.
    let id = m.id;
    assert_eq!(s.run("sync.imapFetchRequest", &json!({"account": account, "message": id})).unwrap()["ids"], json!([key(9)]));
}

#[test]
fn mail_of_a_mailbox_the_server_no_longer_lists_keeps_its_text() {
    use mailcraft_model::sync::{ListedFolder, SyncResult};
    let (mut s, account, ids) = aged("1m", &[400]);
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    // A mailbox another program deletes or renames on the server.
    let projects = s.mailbox.add_folder(account, None, "Projects").unwrap();
    s.mailbox.folder_mut(projects).unwrap().remote = "Projects".into();
    let body = Body { text: "Received 400 days ago. ".repeat(100), html: None };
    let old = Message { account, folder: projects, uid: 3, received: NOW - 400 * DAY, subject: "Kept".into(), body, ..Default::default() };
    let kept = s.mailbox.add_message(old).unwrap();
    let listing = |names: &[&str], all: bool| SyncResult {
        account,
        email: "ada@mail.example".into(),
        listed: names.iter().map(|n| ListedFolder { remote: (*n).into(), name: (*n).into(), ..Default::default() }).collect(),
        listed_all: all,
        ..Default::default()
    };
    // Listed to its end without it: not the server's any more.
    super::account::apply_sync(&mut s, listing(&["INBOX"], true)).unwrap();
    assert!(s.mailbox.folder(projects).unwrap().unlisted && !s.mailbox.folder(inbox).unwrap().unlisted);
    s.run("account.thinOffline", &json!({"account": account})).unwrap();
    assert!(s.mailbox.message(ids[0]).unwrap().unfetched, "the Inbox's old mail goes back to its preview");
    let m = s.mailbox.message(kept).unwrap();
    assert!(!m.unfetched && m.body.text.len() > 1_000, "what is here may be all there is of it");
    // A listing cut short says nothing of what it leaves out.
    super::account::apply_sync(&mut s, listing(&["INBOX"], false)).unwrap();
    assert!(s.mailbox.folder(projects).unwrap().unlisted);
    // Named again: the server's, as before.
    super::account::apply_sync(&mut s, listing(&["INBOX", "Projects"], false)).unwrap();
    assert!(!s.mailbox.folder(projects).unwrap().unlisted);
    s.run("account.thinOffline", &json!({"account": account})).unwrap();
    assert!(s.mailbox.message(kept).unwrap().unfetched);
}

#[test]
fn a_rule_that_cant_move_a_preview_out_of_its_account_says_so() {
    use mailcraft_model::sync::{FolderDelta, SyncResult};
    let (mut s, account, _) = aged("1m", &[]);
    let local = s.run("account.add", &json!({"email": "here@mail.example"})).unwrap()["id"].as_u64().unwrap();
    let local_inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let rule = json!({"name": "To here", "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "Old"}], "actions": [{"type": "moveTo", "value": local_inbox}]});
    s.run("rules.add", &rule).unwrap();
    let start = Message {
        uid: 9,
        subject: "Old".into(),
        received: NOW - 400 * DAY,
        unfetched: true,
        body: Body { text: "The start of a long letter. ".repeat(500), html: None },
        ..Default::default()
    };
    let delta = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 9, new: vec![start], ..Default::default() };
    let result = SyncResult { account, email: "ada@mail.example".into(), folders: vec![delta], ..Default::default() };
    let said = super::account::apply_sync(&mut s, result).unwrap();
    // It stays in its Inbox, as its server has it, and the owner is told why.
    let m = s.mailbox.messages.values().find(|m| m.subject == "Old").unwrap();
    assert_eq!(m.folder, s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap());
    let errors = said["errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| e.as_str().unwrap().contains("The rule \"To here\" couldn't move a message")), "{said}");
}
