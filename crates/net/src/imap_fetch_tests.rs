// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fetch_imap` against the fake IMAP server (#653): messages fetched whole by mailbox,
//! UIDVALIDITY and UID, without a change on the server.

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{ImapFetchKey, ImapFetchRequest};
use mailcraft_model::{Account, AccountKind, Security, Server};

use super::fetch_imap;
use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};

const PASSWORD: &str = "hunter2";

fn eml(subject: &str) -> Vec<u8> {
    format!("From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nMessage-ID: <same@example.org>\r\n\r\nThe body of {subject}.\r\n")
        .into_bytes()
}

fn message(uid: u32, subject: &str) -> FakeMessage {
    FakeMessage { uid, flags: String::new(), internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body: eml(subject), claimed_size: None }
}

fn mailbox(raw: &str, flags: &str, uid_validity: u32, messages: Vec<FakeMessage>) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity, messages, refuse_open: false, next_uid: 0 }
}

/// Gmail's way: the same message under a label and in All Mail, under other UIDs.
fn server() -> FakeImap {
    Fake::imap(ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        mailboxes: vec![
            mailbox("INBOX", "\\HasNoChildren", 11, vec![message(1, "One"), message(4, "Four")]),
            mailbox("[Gmail]/All Mail", "\\HasNoChildren \\All", 51, vec![message(9, "Four in All Mail")]),
        ],
        ..Default::default()
    })
}

fn account(imap: &FakeImap) -> Account {
    Account {
        id: 7,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        username: "ada".into(),
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        ..Default::default()
    }
}

fn key(remote: &str, uid_validity: u32, uid: u32) -> String {
    ImapFetchKey { remote: remote.into(), uid_validity, uid }.write()
}

fn fetch(imap: &FakeImap, ids: Vec<String>) -> mailcraft_model::sync::GraphFetchResult {
    fetch_imap(&ImapFetchRequest { account: account(imap), ids, background: false }, PASSWORD, &AtomicBool::new(false))
}

#[test]
fn a_message_comes_whole_by_its_uid_and_nothing_changes_there() {
    let imap = server();
    let result = fetch(&imap, vec![key("INBOX", 11, 4)]);
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!((result.account, result.email.as_str()), (7, "ada@example.org"));
    assert_eq!(result.fetched.len(), 1);
    assert_eq!(result.fetched[0].id, key("INBOX", 11, 4));
    assert_eq!(result.fetched[0].mime, eml("Four"));
    let world = imap.world();
    // Looked at, never changed: EXAMINE, and BODY.PEEK so that `\Seen` is not set.
    assert!(world.log.iter().any(|line| line.ends_with("EXAMINE INBOX") || line.ends_with("EXAMINE \"INBOX\"")), "{:?}", world.log);
    assert!(world.log.iter().any(|line| line.contains("UID FETCH 4 (UID FLAGS INTERNALDATE BODY.PEEK[])")));
    assert!(!world.log.iter().any(|line| line.contains("SELECT") || line.contains("STORE") || line.contains("EXPUNGE")));
    assert!(world.mailboxes[0].messages.iter().all(|m| !m.has("\\Seen")));
}

#[test]
fn copies_under_labels_come_each_from_its_own_mailbox() {
    let imap = server();
    let result = fetch(&imap, vec![key("INBOX", 11, 4), key("[Gmail]/All Mail", 51, 9)]);
    assert_eq!(result.errors, Vec::<String>::new());
    let got: Vec<(&str, Vec<u8>)> = result.fetched.iter().map(|f| (f.id.as_str(), f.mime.clone())).collect();
    let (inbox, all) = (key("INBOX", 11, 4), key("[Gmail]/All Mail", 51, 9));
    assert_eq!(got, vec![(inbox.as_str(), eml("Four")), (all.as_str(), eml("Four in All Mail"))]);
    assert_eq!(imap.world().connections, 1);
}

#[test]
fn a_mailbox_numbered_anew_is_not_asked_for_its_uids() {
    let imap = server();
    let result = fetch(&imap, vec![key("INBOX", 10, 4)]);
    assert!(result.fetched.is_empty());
    assert_eq!(result.failed, vec![key("INBOX", 10, 4)]);
    assert!(!imap.world().log.iter().any(|line| line.contains("BODY.PEEK")));
}

#[test]
fn what_is_gone_is_missing_and_junk_is_refused() {
    let imap = server();
    let result = fetch(&imap, vec![key("INBOX", 11, 3), key("Gone", 2, 1), "nothing".into()]);
    assert!(result.fetched.is_empty());
    assert_eq!(result.missing, vec![key("INBOX", 11, 3), key("Gone", 2, 1)]);
    assert_eq!(result.failed, vec!["nothing".to_string()]);
    assert!(!result.offline);
}

#[test]
fn one_too_large_to_take_is_refused_and_the_rest_comes() {
    let imap = server();
    imap.world().mailboxes[0].messages[1].claimed_size = Some(200 * 1024 * 1024);
    let result = fetch(&imap, vec![key("INBOX", 11, 4), key("INBOX", 11, 1)]);
    assert_eq!(result.failed, vec![key("INBOX", 11, 4)]);
    assert_eq!(result.fetched.iter().map(|f| f.id.clone()).collect::<Vec<_>>(), vec![key("INBOX", 11, 1)]);
    assert!(!imap.world().log.iter().any(|line| line.contains("UID FETCH 4 (UID FLAGS")));
}

#[test]
fn one_whose_size_the_server_doesnt_say_is_not_downloaded_and_asked_for_again() {
    // A size not known is no size under the bounds: nothing is downloaded whole on trust. Nor
    // is it refused: the server said nothing of the message, and it is asked for again.
    let imap = server();
    imap.world().refuse = vec!["RFC822.SIZE".into()];
    let result = fetch(&imap, vec![key("INBOX", 11, 4), key("INBOX", 11, 1)]);
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors[0].starts_with("Couldn't read the sizes of the messages in \"INBOX\""), "{:?}", result.errors);
    assert!(result.failed.is_empty() && result.missing.is_empty(), "{:?} {:?}", result.failed, result.missing);
    assert!(result.fetched.is_empty() && !result.offline);
    assert!(!imap.world().log.iter().any(|line| line.contains("BODY.PEEK[]")));
}

#[test]
fn a_connection_lost_while_the_sizes_come_is_offline_and_refuses_nothing() {
    // A break in the middle is no word on the messages: none of them is passed over for the
    // rest of the session, and the mailboxes after it are not asked for on a dead connection.
    let imap = server();
    imap.world().hang_up_on = Some("RFC822.SIZE".into());
    let result = fetch(&imap, vec![key("INBOX", 11, 4), key("INBOX", 11, 1), key("[Gmail]/All Mail", 51, 9)]);
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors[0].starts_with("Couldn't read the sizes of the messages in \"INBOX\""), "{:?}", result.errors);
    assert!(result.failed.is_empty() && result.missing.is_empty(), "{:?} {:?}", result.failed, result.missing);
    assert!(result.fetched.is_empty() && result.offline);
    assert!(!imap.world().log.iter().any(|line| line.contains("EXAMINE \"[Gmail]/All Mail\"")));
}

#[test]
fn a_connection_lost_while_the_folders_are_listed_is_offline_and_a_refused_list_is_not() {
    // As a break while a mailbox opens or the sizes come: nothing is refused, and the account is
    // offline only when the server was lost, not when it said no.
    for (lost, offline) in [(true, true), (false, false)] {
        let imap = server();
        if lost {
            imap.world().hang_up_on = Some("LIST".into());
        } else {
            imap.world().refuse = vec!["LIST".into()];
        }
        let result = fetch(&imap, vec![key("INBOX", 11, 4)]);
        assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
        assert!(result.errors[0].starts_with("Couldn't read the list of folders"), "{:?}", result.errors);
        assert!(result.failed.is_empty() && result.missing.is_empty(), "{:?} {:?}", result.failed, result.missing);
        assert!(result.fetched.is_empty());
        assert_eq!(result.offline, offline, "lost: {lost}");
    }
}

#[test]
fn a_server_that_is_not_there_is_offline_and_a_wrong_password_is_said_without_it() {
    let imap = server();
    let mut lost = account(&imap);
    lost.incoming = Some(Server { host: "127.0.0.1".into(), port: 1, security: Security::None });
    let result = fetch_imap(&ImapFetchRequest { account: lost, ids: vec![key("INBOX", 11, 4)], background: true }, PASSWORD, &AtomicBool::new(false));
    assert!(result.offline && result.fetched.is_empty());
    let wrong = "wrong-secret-77";
    let result =
        fetch_imap(&ImapFetchRequest { account: account(&imap), ids: vec![key("INBOX", 11, 4)], background: false }, wrong, &AtomicBool::new(false));
    assert!(!result.offline && result.fetched.is_empty() && !result.errors.is_empty());
    assert!(result.errors.iter().all(|line| !line.contains(wrong)), "{:?}", result.errors);
}

#[test]
fn nothing_asked_for_opens_no_connection_and_cancel_stops_it() {
    let imap = server();
    let result = fetch(&imap, Vec::new());
    assert_eq!(result.fetched.len() + result.errors.len(), 0);
    let result = fetch_imap(
        &ImapFetchRequest { account: account(&imap), ids: vec![key("INBOX", 11, 4)], background: false },
        PASSWORD,
        &AtomicBool::new(true),
    );
    assert!(result.fetched.is_empty());
    assert_eq!(imap.world().connections, 0);
}
