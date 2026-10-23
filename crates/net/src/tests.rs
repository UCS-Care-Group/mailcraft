// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `test_account`, `send`, `sync` and `sync_with` end to end, against the fake servers on loopback.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_model::sync::{
    Created, Deleted, FlagsPushed, FolderState, Moved, RemoteFlags, RemoteOp, SyncPhase, SyncRequest, SyncResult, SyncStage,
};
use mailcraft_model::{Account, AccountKind, Address, FlagStatus, FolderKind, Id, Message, OAuth, Security, Server};
use mailcraft_proto::ProtoError;

use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, FakeSmtp, ImapWorld, SmtpWorld};
use crate::sync::{Host, Limits, sync_as, sync_within};
use crate::transport::Patience;
use crate::{NetError, send, sync, sync_with, test_account};

const PASSWORD: &str = "hunter2";
/// The access token of the accounts that sign in with OAuth.
const TOKEN: &str = "eyJ0eXAi.tok-en_123.c2ln";
/// `Date:` of every test message.
const SENT_AT: i64 = 1_057_049_557;
/// `INTERNALDATE` of every test message: an hour later.
const ARRIVED_AT: i64 = SENT_AT + 3_600;

fn eml(subject: &str) -> Vec<u8> {
    format!(
        "From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\n\
         Message-ID: <{}@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe body of {subject}.\r\n",
        subject.to_lowercase().replace(' ', "-")
    )
    .into_bytes()
}

fn message(uid: u32, flags: &str, subject: &str) -> FakeMessage {
    FakeMessage { uid, flags: flags.into(), internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body: eml(subject), claimed_size: None }
}

fn mailbox(raw: &str, flags: &str, uid_validity: u32, messages: Vec<FakeMessage>) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity, messages, refuse_open: false, next_uid: 0 }
}

/// A small account: an Inbox with a message on its way out, flagged and plainly named folders,
/// a folder that can't be opened and a view of everything.
fn imap_world() -> ImapWorld {
    ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        mailboxes: vec![
            mailbox("Projects", "\\HasNoChildren", 31, vec![message(1, "", "Project one")]),
            mailbox(
                "INBOX",
                "\\HasNoChildren",
                11,
                vec![
                    message(1, "\\Seen", "One"),
                    message(2, "\\Flagged \\Answered", "Two"),
                    message(5, "\\Draft $Forwarded", "Five"),
                    message(6, "\\Deleted", "Six"),
                ],
            ),
            mailbox("[Gmail]", "\\Noselect \\HasChildren", 0, Vec::new()),
            mailbox("[Gmail]/All Mail", "\\HasNoChildren \\All", 51, vec![message(1, "", "Everything")]),
            mailbox("Sent Items", "\\HasNoChildren \\Sent", 21, vec![message(3, "\\Seen", "Sent three")]),
            mailbox("Entw&APw-rfe", "\\Drafts", 41, Vec::new()),
        ],
        ..Default::default()
    }
}

fn smtp_world() -> SmtpWorld {
    SmtpWorld { user: "ada".into(), password: PASSWORD.into(), auth: true, ..Default::default() }
}

fn account(imap: Option<&FakeImap>, smtp: Option<&FakeSmtp>) -> Account {
    fn server<W>(fake: &Fake<W>) -> Server {
        Server { host: "127.0.0.1".into(), port: fake.port, security: Security::None }
    }
    Account {
        id: 7,
        name: "Ada".into(),
        email: "ada@example.org".into(),
        display_name: "Ada Lovelace".into(),
        kind: AccountKind::Imap,
        incoming: imap.map(server),
        outgoing: smtp.map(server),
        username: "ada".into(),
        signature: String::new(),
        oauth: None,
        ..Default::default()
    }
}

/// The same account signing in with OAuth, under its address.
fn oauth_account(imap: Option<&FakeImap>, smtp: Option<&FakeSmtp>) -> Account {
    Account { username: String::new(), oauth: Some(OAuth { client_id: "client-1".into(), ..Default::default() }), ..account(imap, smtp) }
}

/// Servers that know the account of [`oauth_account`] by its token as well as by its password.
fn token_servers() -> (FakeImap, FakeSmtp) {
    let imap = Fake::imap(ImapWorld { user: "ada@example.org".into(), token: TOKEN.into(), ..imap_world() });
    let smtp = Fake::smtp(SmtpWorld { user: "ada@example.org".into(), token: TOKEN.into(), ..smtp_world() });
    (imap, smtp)
}

fn hello() -> Message {
    Message { id: 9, to: vec![Address::new("Bob", "bob@example.org")], subject: "Hello".into(), ..Default::default() }
}

/// The folders a new mailbox has: one of each standard kind, none tied to the server yet.
fn local_folders() -> Vec<FolderState> {
    FolderKind::STANDARD.iter().zip(100..).map(|(kind, id)| FolderState { id, kind: *kind, ..Default::default() }).collect()
}

fn request(account: Account, folders: Vec<FolderState>) -> SyncRequest {
    SyncRequest { account, password: PASSWORD.into(), folders, ..Default::default() }
}

/// What the mailbox's folders look like once `result` is applied, as the engine would do it.
fn applied(before: &[FolderState], result: &SyncResult) -> Vec<FolderState> {
    let mut folders = before.to_vec();
    // A folder that only existed here follows its new mailbox, and a moved message has the UID
    // the server said it has now.
    for made in &result.created {
        folders.iter_mut().find(|f| f.id == made.folder).unwrap().remote = made.remote.clone();
    }
    for moved in result.moved.iter().filter(|moved| moved.new_uid != 0) {
        folders.iter_mut().find(|f| f.id == moved.to_folder).unwrap().known_uids.push(moved.new_uid);
    }
    for delta in &result.folders {
        let known = delta.id.and_then(|id| folders.iter().position(|f| f.id == id));
        let at = match known.or_else(|| folders.iter().position(|f| f.remote == delta.remote)) {
            Some(at) => at,
            None => {
                folders.push(FolderState { id: 1_000 + folders.len() as u64, kind: delta.kind, ..Default::default() });
                folders.len() - 1
            }
        };
        let folder = &mut folders[at];
        // The count of how far a mailbox was read never goes back, unless it was made anew.
        let anew = folder.uid_validity != 0 && delta.uid_validity != 0 && folder.uid_validity != delta.uid_validity;
        folder.remote = delta.remote.clone();
        folder.uid_validity = delta.uid_validity;
        folder.uid_seen = if anew { delta.uid_seen } else { delta.uid_seen.max(folder.uid_seen) };
        if anew || delta.uid_from != 0 {
            folder.uid_from = delta.uid_from;
        }
        folder.known_uids.retain(|uid| !anew && !delta.removed.contains(uid));
        // (A message that came down to be matched with one already here has its UID now too.)
        folder.known_uids.extend(delta.new.iter().map(|m| m.uid));
    }
    folders
}

fn uids(messages: &[Message]) -> Vec<u32> {
    messages.iter().map(|m| m.uid).collect()
}

fn remotes(result: &SyncResult) -> Vec<&str> {
    result.folders.iter().map(|f| f.remote.as_str()).collect()
}

fn inbox(world: &mut ImapWorld) -> &mut FakeMailbox {
    world.mailboxes.iter_mut().find(|m| m.raw == "INBOX").unwrap()
}

/// How many whole messages the server was asked for.
fn downloads(imap: &FakeImap) -> usize {
    imap.world().log.iter().filter(|line| line.contains("BODY.PEEK[]")).count()
}

#[test]
fn the_first_sync_downloads_everything() {
    let imap = Fake::imap(imap_world());
    let result = sync(&request(account(Some(&imap), None), local_folders()));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.account, 7);
    assert!(result.sent.is_empty());
    // The Inbox first, then the other standard folders, then the rest; no view, nothing unopenable.
    assert_eq!(remotes(&result), ["INBOX", "Sent Items", "Entwürfe", "Projects"]);
    assert_eq!(result.new_count(), 5);

    let inbox = &result.folders[0];
    assert_eq!((inbox.id, inbox.kind, inbox.uid_validity, inbox.uid_seen), (Some(100), FolderKind::Inbox, 11, 6));
    // The message marked as deleted stays on the server.
    assert_eq!(uids(&inbox.new), [1, 2, 5]);
    assert!(inbox.flags.is_empty() && inbox.removed.is_empty());

    let one = &inbox.new[0];
    assert_eq!((one.subject.as_str(), one.from.email.as_str(), one.message_id.as_str()), ("One", "bob@example.org", "one@example.org"));
    assert!(one.body.text.contains("The body of One."), "{:?}", one.body.text);
    assert_eq!((one.date, one.received), (SENT_AT, ARRIVED_AT));
    assert_eq!((one.id, one.account, one.folder), (0, 0, 0));
    assert_eq!((one.read, one.flag, one.answered, one.draft, one.forwarded), (true, FlagStatus::None, false, false, false));
    let two = &inbox.new[1];
    assert_eq!((two.read, two.flag, two.answered, two.draft), (false, FlagStatus::Flagged, true, false));
    let five = &inbox.new[2];
    assert_eq!((five.read, five.flag, five.draft, five.forwarded), (false, FlagStatus::None, true, true));

    let sent = &result.folders[1];
    assert_eq!((sent.id, sent.kind, sent.uid_validity, sent.uid_seen, uids(&sent.new)), (Some(102), FolderKind::Sent, 21, 3, vec![3]));
    let drafts = &result.folders[2];
    assert_eq!((drafts.id, drafts.kind, drafts.uid_validity, drafts.uid_seen, drafts.new.len()), (Some(101), FolderKind::Drafts, 41, 0, 0));
    let projects = &result.folders[3];
    assert_eq!((projects.id, projects.kind, projects.uid_seen, uids(&projects.new)), (None, FolderKind::Custom, 1, vec![1]));

    let world = imap.world();
    assert_eq!(world.connections, 1);
    // Mailboxes are only looked at, never changed; the name goes back as the server wrote it.
    assert!(world.log.iter().any(|line| line.ends_with("EXAMINE \"Entw&APw-rfe\"")));
    assert!(
        !world
            .log
            .iter()
            .any(|line| line.contains("SELECT") || line.contains("STORE") || line.contains("All Mail") || line.contains("EXAMINE \"[Gmail]\""))
    );
    assert!(world.log.last().unwrap().ends_with("LOGOUT"));
}

#[test]
fn the_next_sync_brings_only_changes() {
    let imap = Fake::imap(imap_world());
    let account = account(Some(&imap), None);
    let first = sync(&request(account.clone(), local_folders()));
    let folders = applied(&local_folders(), &first);
    assert_eq!(folders.iter().find(|f| f.remote == "INBOX").unwrap().known_uids, [1, 2, 5]);
    let downloaded = downloads(&imap);

    // Nothing happened on the server: nothing is downloaded again.
    let quiet = sync(&request(account.clone(), folders.clone()));
    assert_eq!((quiet.errors.len(), quiet.new_count(), downloads(&imap)), (0, 0, downloaded));
    assert_eq!(remotes(&quiet), ["INBOX", "Sent Items", "Entwürfe", "Projects"]);
    // Every folder is the local one it became.
    assert_eq!(quiet.folders.iter().map(|f| f.id).collect::<Vec<_>>(), [Some(100), Some(102), Some(101), Some(1_007)]);
    assert!(quiet.folders.iter().all(|f| f.removed.is_empty()));
    assert_eq!(quiet.summary(), "Up to date");

    // One was read elsewhere and flagged, one was deleted, the one marked as deleted was expunged.
    {
        let mut world = imap.world();
        let inbox = inbox(&mut world);
        inbox.messages.retain(|m| m.uid != 2 && m.uid != 6);
        inbox.messages[0].flags = "\\Flagged".into();
    }
    let changed = sync(&request(account.clone(), folders.clone()));
    assert_eq!(changed.errors, Vec::<String>::new());
    let delta = &changed.folders[0];
    assert_eq!((delta.new.len(), delta.uid_seen, delta.removed.as_slice()), (0, 5, &[2][..]));
    assert_eq!(
        delta.flags,
        [
            RemoteFlags { uid: 1, read: false, flagged: true, answered: false, draft: false },
            RemoteFlags { uid: 5, read: false, flagged: false, answered: false, draft: true }
        ]
    );
    assert_eq!(downloads(&imap), downloaded);

    // A new message arrives; another is marked as deleted, which is as good as gone.
    let folders = applied(&folders, &changed);
    {
        let mut world = imap.world();
        let inbox = inbox(&mut world);
        inbox.messages.push(message(7, "", "Seven"));
        inbox.messages[0].flags = "\\Seen \\Deleted".into();
    }
    let third = sync(&request(account, folders));
    let delta = &third.folders[0];
    assert_eq!((uids(&delta.new), delta.uid_seen, delta.removed.as_slice()), (vec![7], 7, &[1][..]));
    assert_eq!(delta.new[0].subject, "Seven");
    assert_eq!(delta.flags.iter().map(|f| f.uid).collect::<Vec<_>>(), [5]);
    assert_eq!(downloads(&imap), downloaded + 1);
    assert_eq!(third.summary(), "1 new message");
}

#[test]
fn a_new_uid_validity_starts_the_folder_over() {
    let imap = Fake::imap(imap_world());
    let account = account(Some(&imap), None);
    let folders = applied(&local_folders(), &sync(&request(account.clone(), local_folders())));
    {
        let mut world = imap.world();
        let inbox = inbox(&mut world);
        inbox.uid_validity = 12;
        inbox.messages = vec![message(1, "\\Seen", "New one"), message(2, "", "New two")];
    }
    let result = sync(&request(account, folders));
    assert_eq!(result.errors, Vec::<String>::new());
    let delta = &result.folders[0];
    assert_eq!((delta.id, delta.uid_validity, delta.uid_seen), (Some(100), 12, 2));
    // Everything we had is gone, and what is there now is new, whatever its UIDs.
    assert_eq!(delta.removed, [1, 2, 5]);
    assert!(delta.flags.is_empty());
    assert_eq!(delta.new.iter().map(|m| (m.uid, m.subject.as_str(), m.read)).collect::<Vec<_>>(), [(1, "New one", true), (2, "New two", false)]);
    // The other folders are as they were.
    assert!(result.folders.iter().skip(1).all(|f| f.new.is_empty() && f.removed.is_empty()));
}

#[test]
fn a_wrong_password_is_an_error_line_without_the_password() {
    let imap = Fake::imap(imap_world());
    let smtp = Fake::smtp(smtp_world());
    let account = account(Some(&imap), Some(&smtp));
    let mut request = request(account.clone(), local_folders());
    request.password = "wrong-horse".into();
    request.outbox = vec![Message { id: 9, to: vec![Address::new("Bob", "bob@example.org")], subject: "Hello".into(), ..Default::default() }];
    let result = sync(&request);
    assert_eq!(result.errors.len(), 2, "{:?}", result.errors);
    assert!(result.errors[0].starts_with("Couldn't send mail: sign-in failed: "), "{}", result.errors[0]);
    assert!(result.errors[1].starts_with("Couldn't check for new mail: sign-in failed: "), "{}", result.errors[1]);
    assert!(result.folders.is_empty() && result.sent.is_empty());
    assert!(!format!("{result:?} {request:?}").contains("wrong-horse"));
    // The servers did get it, and wrote it back in their answers.
    assert!(imap.world().log.iter().any(|line| line.contains("wrong-horse")));
    assert!(smtp.world().delivered.is_empty());

    let error = test_account(&account, "wrong-horse").unwrap_err();
    assert!(matches!(error, NetError::Proto(ProtoError::Auth(_))), "{error:?}");
    assert!(!format!("{error} {error:?}").contains("wrong-horse"));
    assert!(error.to_string().contains("***"), "{error}");

    // The same when only the outgoing server minds.
    smtp.world().password = "another".into();
    let error = test_account(&account, PASSWORD).unwrap_err();
    assert!(matches!(error, NetError::Proto(ProtoError::Auth(_))), "{error:?}");
    assert!(!format!("{error} {error:?}").contains(PASSWORD));
    let message = Message { to: vec![Address::new("Bob", "bob@example.org")], ..Default::default() };
    let error = send(&account, PASSWORD, &message).unwrap_err();
    assert!(matches!(error, NetError::Proto(ProtoError::Auth(_))), "{error:?}");
    assert!(!format!("{error} {error:?}").contains(PASSWORD));
}

#[test]
fn an_oauth_account_signs_in_with_its_token() {
    let (imap, smtp) = token_servers();
    let account = oauth_account(Some(&imap), Some(&smtp));
    assert_eq!(test_account(&account, TOKEN), Ok(()));
    {
        let (imap, smtp) = (imap.world(), smtp.world());
        assert_eq!((imap.connections, smtp.connections, smtp.sign_ins), (1, 1, 1));
        // Asked first, as the server didn't say the token may come with the command.
        assert_eq!(imap.log[0], "A0001 AUTHENTICATE XOAUTH2");
        assert!(smtp.log.iter().any(|line| line.starts_with("AUTH XOAUTH2 ")), "{:?}", smtp.log);
    }

    // Mail goes out and comes in on the request's token; its password is for other accounts.
    let mut request = request(account.clone(), local_folders());
    request.access_token = TOKEN.into();
    request.outbox = vec![hello()];
    let result = sync(&request);
    assert_eq!(result.errors, Vec::<String>::new());
    // (The sixth is the copy of what was sent, kept on the server and read back from there.)
    assert_eq!((result.sent.as_slice(), result.filed.as_slice(), result.new_count()), (&[9][..], &[9][..], 6));
    assert_eq!(send(&account, TOKEN, &hello()), Ok(()));
    assert_eq!(smtp.world().delivered.len(), 2);

    // With the command, for a server that takes it that way.
    imap.world().sasl_ir = true;
    assert_eq!(test_account(&oauth_account(Some(&imap), None), TOKEN), Ok(()));
    assert!(imap.world().log.iter().any(|line| line.starts_with("A0001 AUTHENTICATE XOAUTH2 ")));

    // Never the password, nor the ways of signing in that go with one.
    let (imap, smtp) = (imap.world(), smtp.world());
    let lines = || imap.log.iter().chain(smtp.log.iter());
    assert!(!lines().any(|line| line.contains("LOGIN") || line.contains("AUTH PLAIN") || line.contains(PASSWORD)));
    // Nor the token as it is: it travels in its envelope.
    assert!(!lines().any(|line| line.contains(TOKEN)));
}

#[test]
fn a_token_of_real_length_signs_in() {
    // A couple of thousand characters: too long for one line of SMTP.
    let token = format!("eyJ0eXAi.{}.c2ln", "x".repeat(2_000));
    let (imap, smtp) = token_servers();
    imap.world().token = token.clone();
    smtp.world().token = token.clone();
    assert_eq!(test_account(&oauth_account(Some(&imap), Some(&smtp)), &token), Ok(()));
    assert_eq!(smtp.world().sign_ins, 1);
    assert!(smtp.world().log.iter().any(|line| line == "AUTH XOAUTH2"));
    imap.world().sasl_ir = true;
    assert_eq!(test_account(&oauth_account(Some(&imap), None), &token), Ok(()));
}

#[test]
fn a_refused_token_is_an_error_without_the_token() {
    let (imap, smtp) = token_servers();
    let account = oauth_account(Some(&imap), Some(&smtp));
    // One that has expired, say: the servers don't tell why.
    let stale = "eyJ0eXAi.stale-token.c2ln";
    let error = test_account(&account, stale).unwrap_err();
    assert!(matches!(error, NetError::Proto(ProtoError::Auth(_))), "{error:?}");
    assert!(!format!("{error} {error:?}").contains(stale));
    assert_eq!(error.to_string(), "sign-in failed: [AUTHENTICATIONFAILED] no user with the token ***");
    let error = send(&account, stale, &hello()).unwrap_err();
    assert_eq!(error.to_string(), "sign-in failed: 535 5.7.3 no user with the token ***");
    assert!(!format!("{error:?}").contains(stale));

    let mut request = request(account.clone(), local_folders());
    request.access_token = stale.into();
    request.outbox = vec![hello()];
    let result = sync(&request);
    assert_eq!(
        result.errors,
        [
            "Couldn't send mail: sign-in failed: 535 5.7.3 no user with the token ***",
            "Couldn't check for new mail: sign-in failed: [AUTHENTICATIONFAILED] no user with the token ***"
        ]
    );
    assert!(result.folders.is_empty() && result.sent.is_empty());
    assert!(!format!("{result:?} {request:?}").contains(stale));
    // The same when the token went with the command.
    imap.world().sasl_ir = true;
    let error = test_account(&oauth_account(Some(&imap), None), stale).unwrap_err();
    assert_eq!(error.to_string(), "sign-in failed: [AUTHENTICATIONFAILED] no user with the token ***");
    // The right token for somebody else.
    let mut other = oauth_account(Some(&imap), None);
    other.email = "eve@example.org".into();
    assert!(matches!(test_account(&other, TOKEN), Err(NetError::Proto(ProtoError::Auth(_)))));
    assert!(smtp.world().delivered.is_empty());
    assert_eq!(smtp.world().sign_ins, 0);
}

#[test]
fn an_oauth_account_without_a_token_is_told_to_sign_in_first() {
    let (imap, smtp) = token_servers();
    let account = oauth_account(Some(&imap), Some(&smtp));
    let sign_in = NetError::Config("sign in to ada@example.org first".into());
    assert_eq!(test_account(&account, ""), Err(sign_in.clone()));
    assert_eq!(test_account(&oauth_account(None, Some(&smtp)), ""), Err(sign_in.clone()));
    assert_eq!(send(&account, "", &hello()), Err(sign_in));
    // A password is no help to an account that doesn't sign in with one.
    let mut request = request(account, local_folders());
    request.outbox = vec![hello()];
    assert_eq!(request.secret(), "");
    let result = sync(&request);
    assert_eq!(
        result.errors,
        ["Couldn't send mail: sign in to ada@example.org first", "Couldn't check for new mail: sign in to ada@example.org first"]
    );
    assert!(result.folders.is_empty() && result.sent.is_empty());
    // Nobody was connected to.
    assert_eq!((imap.world().connections, smtp.world().connections), (0, 0));
    let nameless = Account { email: " \r\n".into(), ..oauth_account(Some(&imap), None) };
    assert!(matches!(test_account(&nameless, ""), Err(NetError::Config(_))));
}

#[test]
fn a_server_that_takes_no_tokens_is_not_shown_one() {
    // Servers that only know passwords.
    let imap = Fake::imap(ImapWorld { user: "ada@example.org".into(), ..imap_world() });
    let smtp = Fake::smtp(SmtpWorld { user: "ada@example.org".into(), ..smtp_world() });
    let error = test_account(&oauth_account(Some(&imap), None), TOKEN).unwrap_err();
    assert_eq!(error, NetError::Proto(ProtoError::Auth("[CANNOT] no such way to sign in".into())));
    // Not even one that would take anything with the command.
    imap.world().sasl_ir = true;
    assert!(matches!(test_account(&oauth_account(Some(&imap), None), TOKEN), Err(NetError::Proto(ProtoError::Auth(_)))));
    assert_eq!(imap.world().log, ["A0001 AUTHENTICATE XOAUTH2", "A0001 AUTHENTICATE XOAUTH2"]);

    let error = send(&oauth_account(None, Some(&smtp)), TOKEN, &hello()).unwrap_err();
    assert_eq!(error, NetError::Proto(ProtoError::Auth("the server offers no way to sign in with an access token".into())));
    // Nor is the mail sent unsigned, or with the token as if it were a password.
    assert_eq!(smtp.world().log, ["EHLO [127.0.0.1]"]);
    // A server that asks for no sign-in at all is no place for an account that needs one.
    let open = Fake::smtp(SmtpWorld { auth: false, ..smtp_world() });
    assert!(matches!(send(&oauth_account(None, Some(&open)), TOKEN, &hello()), Err(NetError::Proto(ProtoError::Auth(_)))));
    assert!(open.world().delivered.is_empty());
}

#[test]
fn accounts_are_tested_on_both_servers() {
    let imap = Fake::imap(imap_world());
    let smtp = Fake::smtp(smtp_world());
    assert_eq!(test_account(&account(Some(&imap), Some(&smtp)), PASSWORD), Ok(()));
    {
        let (imap, smtp) = (imap.world(), smtp.world());
        assert_eq!((imap.connections, smtp.connections, smtp.sign_ins), (1, 1, 1));
        assert!(imap.log.iter().any(|line| line.contains("LOGIN")));
        assert!(smtp.delivered.is_empty());
    }
    // Either server alone, and a name instead of a number for this machine.
    assert_eq!(test_account(&account(Some(&imap), None), PASSWORD), Ok(()));
    let mut by_name = account(None, Some(&smtp));
    by_name.outgoing.as_mut().unwrap().host = "localhost".into();
    assert_eq!(test_account(&by_name, PASSWORD), Ok(()));
    assert!(matches!(test_account(&account(None, None), PASSWORD), Err(NetError::Config(_))));
    // The e-mail address signs in when there is no user name.
    imap.world().user = "ada@example.org".into();
    let mut nameless = account(Some(&imap), None);
    nameless.username.clear();
    assert_eq!(test_account(&nameless, PASSWORD), Ok(()));
}

#[test]
fn no_password_goes_out_unencrypted_to_another_machine() {
    let mut elsewhere = account(None, None);
    elsewhere.incoming = Some(Server { host: "mail.example.org".into(), port: 0, security: Security::None });
    elsewhere.outgoing = Some(Server { host: "192.0.2.25".into(), port: 0, security: Security::None });
    let message = Message { id: 3, to: vec![Address::new("Bob", "bob@example.org")], subject: "Hello".into(), ..Default::default() };

    let started = std::time::Instant::now();
    assert!(matches!(test_account(&elsewhere, PASSWORD), Err(NetError::Config(why)) if why.contains("mail.example.org")));
    assert!(matches!(send(&elsewhere, PASSWORD, &message), Err(NetError::Config(why)) if why.contains("192.0.2.25")));
    let mut request = request(elsewhere, local_folders());
    request.outbox = vec![message];
    let result = sync(&request);
    assert_eq!(result.errors.len(), 2, "{:?}", result.errors);
    assert!(result.errors[0].starts_with("Couldn't send mail: 192.0.2.25 is set to use no encryption"), "{}", result.errors[0]);
    assert!(result.errors[1].starts_with("Couldn't check for new mail: mail.example.org is set to use no encryption"), "{}", result.errors[1]);
    assert!(result.sent.is_empty() && result.folders.is_empty());
    // Refused outright: no name was looked up, no connection waited for.
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn starttls_is_never_given_up_for_plain_text() {
    // Servers that don't offer it.
    let imap = Fake::imap(imap_world());
    let smtp = Fake::smtp(smtp_world());
    let mut strict = account(Some(&imap), Some(&smtp));
    strict.incoming.as_mut().unwrap().security = Security::StartTls;
    strict.outgoing.as_mut().unwrap().security = Security::StartTls;
    let error = test_account(&strict, PASSWORD).unwrap_err();
    assert_eq!(error, NetError::Tls { host: "127.0.0.1".into(), reason: "the server doesn't offer STARTTLS".into() });
    let mut request = request(strict.clone(), local_folders());
    request.outbox = vec![Message { id: 3, to: vec![Address::new("Bob", "bob@example.org")], ..Default::default() }];
    let result = sync(&request);
    assert_eq!(
        result.errors,
        [
            "Couldn't send mail: couldn't make a secure connection to 127.0.0.1: the server doesn't offer STARTTLS",
            "Couldn't check for new mail: couldn't make a secure connection to 127.0.0.1: the server doesn't offer STARTTLS"
        ]
    );

    // Servers that offer it and then don't shake hands.
    imap.world().offer_starttls = true;
    smtp.world().offer_starttls = true;
    assert!(matches!(test_account(&strict, PASSWORD), Err(NetError::Tls { .. })));
    strict.incoming = None;
    assert!(matches!(test_account(&strict, PASSWORD), Err(NetError::Tls { .. })));

    // Not once did a password or a message go out.
    assert!(!imap.world().log.iter().any(|line| line.contains("LOGIN")));
    assert!(!smtp.world().log.iter().any(|line| line.starts_with("AUTH") || line.starts_with("MAIL")));
    assert!(smtp.world().log.iter().any(|line| line == "STARTTLS"));
}

#[test]
fn the_outbox_is_sent_in_one_session() {
    let smtp = Fake::smtp(SmtpWorld { refuse: vec!["nobody@example.org".into()], ..smtp_world() });
    let to = |addresses: &[&str]| addresses.iter().map(|a| Address::new("", *a)).collect::<Vec<_>>();
    let mut request = request(account(None, Some(&smtp)), local_folders());
    let lunch = Message {
        id: 501,
        message_id: "lunch@example.org".into(),
        from: Address::new("Ada Lovelace", "ada@example.org"),
        to: to(&["bob@example.org"]),
        cc: to(&["carol@example.org"]),
        bcc: to(&["erin@example.org", "BOB@example.org"]),
        subject: "Lunch".into(),
        date: SENT_AT,
        body: mailcraft_model::Body { text: "See you at noon.\r\n.\r\n.. two dots\r\n".into(), html: None },
        ..Default::default()
    };
    // What the message is on the wire, were its Bcc line left out.
    let written = mailcraft_mime::write(
        &lunch,
        &mailcraft_mime::WriteOptions { include_bcc: false, offset_minutes: mailcraft_time::local_offset_minutes(SENT_AT) },
    );
    request.outbox = vec![
        lunch,
        Message { id: 502, to: to(&["dave@example.org", "nobody@example.org"]), subject: "Refused\r\n one".into(), ..Default::default() },
        Message { id: 503, subject: "To nobody".into(), ..Default::default() },
        Message { id: 504, to: to(&["dave@example.org"]), subject: "Last".into(), ..Default::default() },
    ];
    let result = sync(&request);
    assert_eq!(result.sent, [501, 504]);
    assert_eq!(
        result.errors,
        [
            "Couldn't send \"Refused one\": the server said: nobody@example.org: 550 5.1.1 no such user",
            "Couldn't send \"To nobody\": the message has no recipients"
        ]
    );
    assert!(result.folders.is_empty());
    assert_eq!(result.summary(), result.errors[0]);

    let world = smtp.world();
    assert_eq!((world.connections, world.sign_ins, world.delivered.len()), (1, 1, 2));
    // The client says nothing about this machine.
    assert_eq!(world.log[0], "EHLO [127.0.0.1]");
    assert_eq!(world.log.last().unwrap(), "QUIT");
    let lunch = &world.delivered[0];
    assert_eq!(lunch.from, "ada@example.org");
    // Bcc recipients get the message, and nobody reads that they did.
    assert_eq!(lunch.recipients, ["bob@example.org", "carol@example.org", "erin@example.org"]);
    let text = lunch.data.to_lowercase();
    assert!(!text.contains("bcc") && !text.contains("erin@example.org"), "{}", lunch.data);
    assert!(text.contains("subject: lunch") && text.contains("carol@example.org"), "{}", lunch.data);
    // It arrives byte for byte as it was written, lines of dots and all.
    assert_eq!(lunch.data.trim_end(), String::from_utf8_lossy(&written).trim_end());
    assert_eq!(world.delivered[1].recipients, ["dave@example.org"]);
    // A message without a date is dated when it is sent.
    assert!(world.delivered[1].data.to_lowercase().contains("date: "));
    assert!(!world.delivered[1].data.contains("1970"), "{}", world.delivered[1].data);
}

#[test]
fn an_outbox_without_a_server_says_so() {
    let mut request = request(account(None, None), local_folders());
    request.outbox = vec![Message { id: 1, to: vec![Address::new("", "bob@example.org")], ..Default::default() }];
    let result = sync(&request);
    assert_eq!(result.errors, ["Couldn't send mail: the account has no outgoing server"]);
    assert!(result.sent.is_empty());
    // And with nothing to do, nothing is done.
    request.outbox.clear();
    assert_eq!(sync(&request), SyncResult { account: 7, email: "ada@example.org".into(), ..Default::default() });
}

#[test]
fn one_message_is_sent() {
    let smtp = Fake::smtp(SmtpWorld { auth: false, ..smtp_world() });
    let account = account(None, Some(&smtp));
    let message = Message {
        to: vec![Address::new("Bob", "bob@example.org")],
        bcc: vec![Address::new("Erin", "erin@example.org")],
        subject: "Hello".into(),
        date: SENT_AT,
        ..Default::default()
    };
    assert_eq!(send(&account, PASSWORD, &message), Ok(()));
    {
        let world = smtp.world();
        assert_eq!(world.delivered.len(), 1);
        assert_eq!(world.delivered[0].recipients, ["bob@example.org", "erin@example.org"]);
        // A server that doesn't ask for a sign-in isn't given the password.
        assert!(!world.log.iter().any(|line| line.starts_with("AUTH")));
    }
    // What can't be sent is refused before any connection is made.
    assert!(matches!(send(&account, PASSWORD, &Message::default()), Err(NetError::Config(_))));
    assert_eq!(smtp.world().connections, 1);
    smtp.world().refuse = vec!["bob@example.org".into()];
    assert_eq!(send(&account, PASSWORD, &message), Err(NetError::Proto(ProtoError::Rejected("bob@example.org: 550 5.1.1 no such user".into()))));
    assert!(matches!(send(&Account::default(), PASSWORD, &message), Err(NetError::Config(_))));
}

#[test]
fn only_the_newest_are_downloaded_when_there_is_a_limit() {
    let imap = Fake::imap(imap_world());
    let mut request = request(account(Some(&imap), None), local_folders());
    request.max_per_folder = 2;
    let result = sync(&request);
    assert_eq!(result.errors, Vec::<String>::new());
    let inbox = &result.folders[0];
    // Read down to the oldest the limit allows.
    assert_eq!((uids(&inbox.new), inbox.uid_seen, inbox.uid_from), (vec![2, 5], 6, 2));
    // The older one is passed over for as long as the limit stays.
    request.folders = applied(&local_folders(), &result);
    assert_eq!(sync(&request).new_count(), 0);
    // Raised, it lets the older one in, and nothing twice.
    request.max_per_folder = 0;
    let more = sync(&request);
    assert_eq!((more.new_count(), uids(&delta(&more, "INBOX").new), delta(&more, "INBOX").uid_seen), (1, vec![1], 6));
}

#[test]
fn one_bad_mailbox_or_message_does_not_stop_the_rest() {
    let imap = Fake::imap(imap_world());
    {
        let mut world = imap.world();
        world.mailboxes.iter_mut().find(|m| m.raw == "Sent Items").unwrap().refuse_open = true;
        world.refuse_uid = Some(2);
    }
    let account = account(Some(&imap), None);
    let result = sync(&request(account.clone(), local_folders()));
    assert_eq!(
        result.errors,
        [
            "Couldn't download a message from \"INBOX\": the server said: [SERVERBUG] that can't be done",
            "Couldn't open \"Sent Items\": the server said: [NONEXISTENT] no such mailbox"
        ]
    );
    assert_eq!(remotes(&result), ["INBOX", "Entwürfe", "Projects"]);
    let inbox = &result.folders[0];
    // What did arrive is kept, and the next run starts again at the one that didn't.
    assert_eq!((uids(&inbox.new), inbox.uid_seen), (vec![1, 5], 1));
    assert_eq!(uids(&result.folders[2].new), [1]);

    imap.world().refuse_uid = None;
    let again = sync(&request(account, applied(&local_folders(), &result)));
    assert_eq!(again.errors.len(), 1);
    let inbox = &again.folders[0];
    assert_eq!((uids(&inbox.new), inbox.uid_seen), (vec![2], 6));
    assert_eq!(inbox.flags.iter().map(|f| f.uid).collect::<Vec<_>>(), [1, 5]);
}

#[test]
fn a_lost_connection_keeps_what_arrived() {
    let imap = Fake::imap(imap_world());
    imap.world().hang_up_at_uid = Some(2);
    let account = account(Some(&imap), None);
    let result = sync(&request(account.clone(), local_folders()));
    assert_eq!(result.errors, ["Couldn't download a message from \"INBOX\": connection problem: the server closed the connection"]);
    // The newest came first; the rest, and the other folders, wait for the next run.
    assert_eq!(remotes(&result), ["INBOX"]);
    assert_eq!((uids(&result.folders[0].new), result.folders[0].uid_seen), (vec![5], 0));

    imap.world().hang_up_at_uid = None;
    let again = sync(&request(account, applied(&local_folders(), &result)));
    assert_eq!(again.errors, Vec::<String>::new());
    assert_eq!((uids(&again.folders[0].new), again.folders[0].uid_seen), (vec![1, 2], 6));
    assert_eq!(remotes(&again), ["INBOX", "Sent Items", "Entwürfe", "Projects"]);
    assert_eq!(again.new_count(), 4);
}

#[test]
fn of_a_message_too_large_to_take_the_header_is_kept_with_a_note() {
    let imap = Fake::imap(imap_world());
    inbox(&mut imap.world()).messages[1].claimed_size = Some(60 * 1024 * 1024);
    let account = account(Some(&imap), None);
    let result = sync(&request(account.clone(), local_folders()));
    // Not a problem to tell: the message is there, and says what it is.
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!((uids(&result.folders[0].new), result.folders[0].uid_seen, result.folders.len()), (vec![1, 2, 5], 6, 4));
    let two = &result.folders[0].new[1];
    // Who, what and when, from the header; its flags and when it arrived, from the server.
    assert_eq!((two.subject.as_str(), two.from.email.as_str(), two.message_id.as_str()), ("Two", "bob@example.org", "two@example.org"));
    assert_eq!((two.date, two.received, two.flag, two.answered, two.read), (SENT_AT, ARRIVED_AT, FlagStatus::Flagged, true, false));
    // In place of its text, for the reading pane to show.
    assert!(two.body.text.starts_with("This message is too large to download: it is 60 MB on the server"), "{}", two.body.text);
    assert!(two.body.text.contains("up to 25 MB") && two.body.text.contains("still on the server"), "{}", two.body.text);
    assert!(!two.body.text.contains("The body of Two") && two.body.html.is_none() && two.attachments.is_empty());
    // A program knows it by its header, which says how large the message is.
    assert_eq!(two.headers.iter().find(|(name, _)| name == "X-MailCraft-Not-Downloaded").map(|(_, size)| size.as_str()), Some("62914560"));
    assert!(result.folders[0].new[0].headers.iter().all(|(name, _)| name != "X-MailCraft-Not-Downloaded"));
    // Only its header was asked for.
    assert_eq!(sent(&imap, "UID FETCH 2 (UID FLAGS INTERNALDATE BODY.PEEK[HEADER])"), 1);
    assert_eq!((sent(&imap, "UID FETCH 2 (UID FLAGS INTERNALDATE BODY.PEEK[])"), downloads(&imap)), (0, 4));

    // It has its UID like any other message: the next run asks for nothing, and its flags follow.
    let folders = applied(&local_folders(), &result);
    inbox(&mut imap.world()).messages[1].flags = "\\Seen".into();
    let again = sync(&request(account, folders));
    assert_eq!((again.errors.len(), again.new_count(), downloads(&imap), sent(&imap, "BODY.PEEK[HEADER]")), (0, 0, 4, 1));
    assert_eq!(again.folders[0].flags[1], RemoteFlags { uid: 2, read: true, flagged: false, answered: false, draft: false });
}

#[test]
fn what_counts_as_too_large_goes_by_what_the_server_says() {
    // Every message is larger than this run takes: none is downloaded whole.
    let imap = Fake::imap(imap_world());
    let small = Limits { largest: 100, ..Limits::kept_whole() };
    let result = sync_within(&request(account(Some(&imap), None), local_folders()), small.clone());
    assert_eq!((result.errors.len(), result.new_count(), downloads(&imap), sent(&imap, "BODY.PEEK[HEADER]")), (0, 5, 0, 5));
    assert!(result.folders.iter().flat_map(|f| &f.new).all(|m| m.body.text.contains("too large to download") && !m.subject.is_empty()));
    // A server that won't say how large its messages are gets the benefit of the doubt.
    let imap = Fake::imap(imap_world());
    imap.world().refuse = vec!["RFC822.SIZE".into()];
    let result = sync_within(&request(account(Some(&imap), None), local_folders()), small.clone());
    assert_eq!((result.errors.len(), result.new_count(), downloads(&imap), sent(&imap, "BODY.PEEK[HEADER]")), (0, 5, 5, 0));
    // A header the server won't hand over is asked for again by the next run, like any message.
    let imap = Fake::imap(imap_world());
    imap.world().refuse = vec!["UID FETCH 2 (UID FLAGS INTERNALDATE BODY.PEEK[HEADER])".into()];
    let result = sync_within(&request(account(Some(&imap), None), local_folders()), small);
    assert_eq!(result.errors, ["Couldn't download a message from \"INBOX\": the server said: [SERVERBUG] that can't be done"]);
    assert_eq!((uids(&result.folders[0].new), result.folders[0].uid_seen), (vec![1, 5], 1));
}

#[test]
fn a_run_downloads_only_so_much() {
    let imap = Fake::imap(imap_world());
    let account = account(Some(&imap), None);
    let size = eml("One").len();
    // Room for the newest message of the Inbox and not quite another.
    let result =
        sync_within(&request(account.clone(), local_folders()), Limits { bytes: size + size / 2, messages: usize::MAX, ..Limits::kept_whole() });
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(remotes(&result), ["INBOX", "Sent Items", "Entwürfe", "Projects"]);
    assert_eq!((uids(&result.folders[0].new), result.folders[0].uid_seen), (vec![5], 0));
    assert_eq!((result.folders[1].new.len(), result.folders[1].uid_seen), (0, 2));
    assert_eq!((result.folders[3].new.len(), result.folders[3].uid_seen), (0, 0));

    // Or for so many messages, however small.
    let folders = applied(&local_folders(), &result);
    let second = sync_within(&request(account.clone(), folders.clone()), Limits { bytes: usize::MAX, messages: 3, ..Limits::kept_whole() });
    assert_eq!(uids(&second.folders[0].new), [1, 2]);
    assert_eq!(uids(&second.folders[1].new), [3]);
    assert_eq!((second.folders[3].new.len(), second.folders[3].uid_seen), (0, 0));

    // The next run brings the rest.
    let third = sync(&request(account, applied(&folders, &second)));
    assert_eq!(third.errors, Vec::<String>::new());
    assert_eq!(third.folders.iter().map(|f| uids(&f.new)).collect::<Vec<_>>(), [vec![], vec![], vec![], vec![1]]);
    assert_eq!(third.folders.iter().map(|f| f.uid_seen).collect::<Vec<_>>(), [6, 3, 0, 1]);
}

#[test]
fn a_server_that_is_not_there_is_an_error_line() {
    // A port that was just free: nothing listens there.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut nowhere = account(None, None);
    nowhere.incoming = Some(Server { host: "127.0.0.1".into(), port, security: Security::None });
    let result = sync(&request(nowhere.clone(), local_folders()));
    assert_eq!(result.errors.len(), 1);
    assert!(result.errors[0].starts_with("Couldn't check for new mail: couldn't reach 127.0.0.1: "), "{}", result.errors[0]);
    assert!(matches!(test_account(&nowhere, PASSWORD), Err(NetError::Connect { .. })));
}

// ---- pushing what was changed here ----

/// The local folder that follows "Projects" once the first run is applied, and the one for deleted mail.
const PROJECTS: Id = 1_007;
const DELETED: Id = 103;

/// The server of [`imap_world`] with a mailbox for deleted mail, and what it can do.
fn two_way(can_move: bool, uidplus: bool) -> FakeImap {
    let mut world = imap_world();
    world.mailboxes.push(mailbox("Trash", "\\HasNoChildren \\Trash", 61, Vec::new()));
    (world.can_move, world.uidplus) = (can_move, uidplus);
    Fake::imap(world)
}

/// The folders as a first run leaves them, when nothing was changed here yet.
fn synced_folders_of(imap: &FakeImap) -> Vec<FolderState> {
    let first = sync(&request(account(Some(imap), None), local_folders()));
    assert_eq!(first.errors, Vec::<String>::new());
    applied(&local_folders(), &first)
}

/// [`synced_folders_of`] a server made by [`two_way`].
fn synced_folders(imap: &FakeImap) -> Vec<FolderState> {
    let folders = synced_folders_of(imap);
    assert_eq!(folders.iter().find(|f| f.remote == "Projects").map(|f| f.id), Some(PROJECTS));
    assert_eq!(folders.iter().find(|f| f.remote == "Trash").map(|f| f.id), Some(DELETED));
    folders
}

/// The folders once messages were moved out of `remote` or deleted there: here they have no UID in it any more.
fn without(mut folders: Vec<FolderState>, remote: &str, uids: &[u32]) -> Vec<FolderState> {
    folders.iter_mut().find(|f| f.remote == remote).unwrap().known_uids.retain(|uid| !uids.contains(uid));
    folders
}

fn pushing(imap: &FakeImap, folders: &[FolderState], ops: Vec<RemoteOp>) -> SyncRequest {
    SyncRequest { ops, ..request(account(Some(imap), None), folders.to_vec()) }
}

fn flags(folder: &str, uid: u32, read: Option<bool>, flagged: Option<bool>, answered: Option<bool>) -> RemoteOp {
    RemoteOp::Flags { folder: folder.into(), uid, read, flagged, answered }
}

fn moving(message: Id, from: &str, uid: u32, to: &str, to_folder: Id) -> RemoteOp {
    RemoteOp::Move { message, from: from.into(), uid, to: to.into(), to_folder, create: Vec::new() }
}

fn deleting(from: &str, uid: u32) -> RemoteOp {
    RemoteOp::Delete { from: from.into(), uid }
}

fn moved(message: Id, from: &str, uid: u32, to_folder: Id, new_uid: u32, uid_validity: u32) -> Moved {
    Moved { message, from: from.into(), uid, to_folder, new_uid, uid_validity }
}

fn deleted(from: &str, uid: u32) -> Deleted {
    Deleted { from: from.into(), uid }
}

/// The messages of a server mailbox: UID and flags.
fn on_server(imap: &FakeImap, raw: &str) -> Vec<(u32, String)> {
    imap.world().mailbox(raw).messages.iter().map(|m| (m.uid, m.flags.clone())).collect()
}

fn server_uids(imap: &FakeImap, raw: &str) -> Vec<u32> {
    imap.world().mailbox(raw).uids()
}

/// How many commands the server got with `text` in them.
fn sent(imap: &FakeImap, text: &str) -> usize {
    imap.world().sent(text)
}

fn delta<'a>(result: &'a SyncResult, remote: &str) -> &'a mailcraft_model::sync::FolderDelta {
    result.folders.iter().find(|f| f.remote == remote).unwrap()
}

/// The commands that change something on the server, without their tags.
fn changes(imap: &FakeImap) -> Vec<String> {
    let changing = ["STORE", "MOVE", "COPY", "EXPUNGE", "CREATE", "SUBSCRIBE", "APPEND", "DELETE", "RENAME", "CLOSE"];
    let world = imap.world();
    let commands = world.log.iter().filter_map(|line| line.split_once(' ').map(|(_, command)| command.to_string()));
    commands.filter(|command| changing.iter().any(|word| command.split(' ').take(2).any(|w| w == *word))).collect()
}

/// Nothing was ever sent that takes messages another program marked as deleted.
fn never_swept(imap: &FakeImap) -> bool {
    !imap.world().log.iter().any(|line| line.ends_with(" EXPUNGE") || line.ends_with(" CLOSE"))
}

#[test]
fn flags_changed_here_are_pushed_and_no_others_touched() {
    let imap = two_way(true, true);
    let folders = synced_folders(&imap);
    let downloaded = downloads(&imap);
    let ops = vec![
        flags("INBOX", 1, Some(false), Some(true), None),
        flags("INBOX", 2, Some(true), None, Some(false)),
        flags("INBOX", 5, Some(true), None, None),
        flags("Projects", 1, None, Some(true), Some(true)),
    ];
    let result = sync(&pushing(&imap, &folders, ops));
    assert_eq!(result.errors, Vec::<String>::new());
    let pushed = |folder: &str, uid, read, flagged, answered| FlagsPushed { folder: folder.into(), uid, read, flagged, answered };
    assert_eq!(
        result.flagged,
        [
            pushed("INBOX", 1, Some(false), Some(true), None),
            pushed("INBOX", 2, Some(true), None, Some(false)),
            pushed("INBOX", 5, Some(true), None, None),
            pushed("Projects", 1, None, Some(true), Some(true)),
        ]
    );
    assert!(result.moved.is_empty() && result.deleted.is_empty() && result.created.is_empty());
    // Each flag that was named is as asked; every other one is as it was, also on the message
    // another program marked as deleted.
    assert_eq!(
        on_server(&imap, "INBOX"),
        [(1, "\\Flagged".into()), (2, "\\Flagged \\Seen".into()), (5, "\\Draft $Forwarded \\Seen".into()), (6, "\\Deleted".into())]
    );
    assert_eq!(on_server(&imap, "Projects"), [(1, "\\Flagged \\Answered".into())]);
    // One command per flag and direction, for all the messages that need it.
    assert_eq!(
        changes(&imap),
        [
            "UID STORE 1 -FLAGS.SILENT (\\Seen)",
            "UID STORE 1 +FLAGS.SILENT (\\Flagged)",
            "UID STORE 2,5 +FLAGS.SILENT (\\Seen)",
            "UID STORE 2 -FLAGS.SILENT (\\Answered)",
            "UID STORE 1 +FLAGS.SILENT (\\Flagged)",
            "UID STORE 1 +FLAGS.SILENT (\\Answered)",
        ]
    );
    // What is read afterwards shows it.
    assert_eq!(
        delta(&result, "INBOX").flags,
        [
            RemoteFlags { uid: 1, read: false, flagged: true, answered: false, draft: false },
            RemoteFlags { uid: 2, read: true, flagged: true, answered: false, draft: false },
            RemoteFlags { uid: 5, read: true, flagged: false, answered: false, draft: true },
        ]
    );
    assert_eq!((result.new_count(), downloads(&imap)), (0, downloaded));
    // The next run has nothing to push and finds everything as it left it.
    let again = sync(&pushing(&imap, &applied(&folders, &result), Vec::new()));
    assert_eq!((again.errors.len(), again.pushed_count(), changes(&imap).len()), (0, 0, 6));
    assert_eq!(delta(&again, "INBOX").flags, delta(&result, "INBOX").flags);
}

#[test]
fn a_move_is_one_step_on_a_server_that_can() {
    let imap = two_way(true, true);
    let folders = without(synced_folders(&imap), "INBOX", &[1, 2]);
    let downloaded = downloads(&imap);
    let ops = vec![moving(901, "INBOX", 1, "Projects", PROJECTS), moving(902, "INBOX", 2, "Trash", DELETED)];
    let result = sync(&pushing(&imap, &folders, ops));
    assert_eq!(result.errors, Vec::<String>::new());
    // Each is where it was moved to, under the UID the server said, with the flags it had.
    assert_eq!(result.moved, [moved(901, "INBOX", 1, PROJECTS, 2, 31), moved(902, "INBOX", 2, DELETED, 1, 61)]);
    assert_eq!(on_server(&imap, "INBOX"), [(5, "\\Draft $Forwarded".into()), (6, "\\Deleted".into())]);
    assert_eq!(on_server(&imap, "Projects"), [(1, String::new()), (2, "\\Seen".into())]);
    assert_eq!(on_server(&imap, "Trash"), [(1, "\\Flagged \\Answered".into())]);
    assert_eq!(changes(&imap), ["UID MOVE 1 \"Projects\"", "UID MOVE 2 \"Trash\""]);
    // They are not downloaded again: their new UIDs are known, and their flags come with the rest.
    let projects = delta(&result, "Projects");
    assert_eq!((projects.new.len(), projects.uid_seen, projects.removed.len()), (0, 2, 0));
    assert_eq!(projects.flags.iter().map(|f| (f.uid, f.read)).collect::<Vec<_>>(), [(1, false), (2, true)]);
    assert_eq!(delta(&result, "Trash").flags.iter().map(|f| (f.uid, f.flagged)).collect::<Vec<_>>(), [(1, true)]);
    assert!(delta(&result, "INBOX").removed.is_empty());
    assert_eq!((result.new_count(), downloads(&imap)), (0, downloaded));
    // Nor by the next run.
    let again = sync(&pushing(&imap, &applied(&folders, &result), Vec::new()));
    assert_eq!((again.errors.len(), again.new_count(), downloads(&imap)), (0, 0, downloaded));
    assert!(again.folders.iter().all(|f| f.removed.is_empty()));
}

#[test]
fn a_moved_message_comes_down_to_be_matched_when_the_server_does_not_say_where_it_went() {
    // MOVE, but no word about the new UID.
    let imap = two_way(true, false);
    let folders = without(synced_folders(&imap), "INBOX", &[1]);
    let downloaded = downloads(&imap);
    let result = sync(&pushing(&imap, &folders, vec![moving(901, "INBOX", 1, "Projects", PROJECTS)]));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.moved, [moved(901, "INBOX", 1, PROJECTS, 0, 0)]);
    assert_eq!(changes(&imap), ["UID MOVE 1 \"Projects\""]);
    assert_eq!(on_server(&imap, "Projects"), [(1, String::new()), (2, "\\Seen".into())]);
    // The same message, by its Message-ID: the mailbox matches it with the one it moved.
    let projects = delta(&result, "Projects");
    assert_eq!(projects.new.iter().map(|m| (m.uid, m.message_id.as_str(), m.read)).collect::<Vec<_>>(), [(2, "one@example.org", true)]);
    assert_eq!(downloads(&imap), downloaded + 1);
    // Once matched it has its UID, and nothing comes twice.
    let again = sync(&pushing(&imap, &applied(&folders, &result), Vec::new()));
    assert_eq!((again.errors.len(), again.new_count(), downloads(&imap)), (0, 0, downloaded + 1));
}

#[test]
fn without_move_a_message_is_copied_and_the_original_marked() {
    // Neither MOVE nor UIDPLUS: the original can only be marked as deleted.
    let imap = two_way(false, false);
    let folders = without(synced_folders(&imap), "INBOX", &[1]);
    let result = sync(&pushing(&imap, &folders, vec![moving(901, "INBOX", 1, "Projects", PROJECTS)]));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.moved, [moved(901, "INBOX", 1, PROJECTS, 0, 0)]);
    assert_eq!(changes(&imap), ["UID COPY 1 \"Projects\"", "UID STORE 1 +FLAGS.SILENT (\\Deleted)"]);
    assert!(never_swept(&imap));
    let marked = |imap: &FakeImap| on_server(imap, "INBOX").iter().map(|(uid, flags)| (*uid, flags.contains("\\Deleted"))).collect::<Vec<_>>();
    assert_eq!(marked(&imap), [(1, true), (2, false), (5, false), (6, true)]);
    assert_eq!(server_uids(&imap, "Projects"), [1, 2]);
    // The marked original is as good as gone, and the copy comes down to be matched.
    assert!(delta(&result, "INBOX").flags.iter().all(|f| f.uid != 1));
    assert_eq!(delta(&result, "Projects").new.iter().map(|m| (m.uid, m.message_id.as_str())).collect::<Vec<_>>(), [(2, "one@example.org")]);
    let again = sync(&pushing(&imap, &applied(&folders, &result), Vec::new()));
    assert_eq!((again.errors.len(), again.new_count(), again.pushed_count()), (0, 0, 0));
    assert!(again.folders.iter().all(|f| f.removed.is_empty()));

    // UIDPLUS without MOVE: the copy says where it is, and the original alone is removed. The
    // message another program marked as deleted stays for that program to deal with.
    let imap = two_way(false, true);
    let folders = without(synced_folders(&imap), "INBOX", &[1, 2]);
    let downloaded = downloads(&imap);
    let ops = vec![moving(901, "INBOX", 1, "Projects", PROJECTS), moving(902, "INBOX", 2, "Projects", PROJECTS)];
    let result = sync(&pushing(&imap, &folders, ops));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.moved, [moved(901, "INBOX", 1, PROJECTS, 2, 31), moved(902, "INBOX", 2, PROJECTS, 3, 31)]);
    assert_eq!(changes(&imap), ["UID COPY 1:2 \"Projects\"", "UID STORE 1:2 +FLAGS.SILENT (\\Deleted)", "UID EXPUNGE 1:2"]);
    assert!(never_swept(&imap));
    assert_eq!(on_server(&imap, "INBOX"), [(5, "\\Draft $Forwarded".into()), (6, "\\Deleted".into())]);
    assert_eq!(server_uids(&imap, "Projects"), [1, 2, 3]);
    assert_eq!((result.new_count(), downloads(&imap)), (0, downloaded));
}

#[test]
fn a_folder_that_only_exists_here_gets_its_mailbox_first() {
    let imap = two_way(true, true);
    let mut folders = without(synced_folders(&imap), "INBOX", &[1, 2, 5]);
    folders.extend([FolderState { id: 500, ..Default::default() }, FolderState { id: 501, ..Default::default() }]);
    let create = |message, uid, to: &str, to_folder, names: &[&str]| RemoteOp::Move {
        message,
        from: "INBOX".into(),
        uid,
        to: to.into(),
        to_folder,
        create: names.iter().map(|n| n.to_string()).collect(),
    };
    // Two messages for a folder two levels down from the top, one for a folder inside "Projects".
    let ops = vec![
        create(901, 1, "", 500, &["Receipts", "Zürich / 2026"]),
        create(902, 2, "", 500, &["Receipts", "Zürich / 2026"]),
        create(903, 5, "Projects", 501, &["Apollo"]),
    ];
    let result = sync(&pushing(&imap, &folders, ops));
    assert_eq!(result.errors, Vec::<String>::new());
    // Made once each, under the server's separator (which a name can't hold) and in its encoding.
    assert_eq!(
        result.created,
        [Created { folder: 500, remote: "Receipts/Zürich _ 2026".into() }, Created { folder: 501, remote: "Projects/Apollo".into() }]
    );
    assert_eq!(
        changes(&imap),
        [
            "CREATE \"Receipts/Z&APw-rich _ 2026\"",
            "SUBSCRIBE \"Receipts/Z&APw-rich _ 2026\"",
            "UID MOVE 1:2 \"Receipts/Z&APw-rich _ 2026\"",
            "CREATE \"Projects/Apollo\"",
            "SUBSCRIBE \"Projects/Apollo\"",
            "UID MOVE 5 \"Projects/Apollo\"",
        ]
    );
    assert_eq!(imap.world().subscribed, ["Receipts/Z&APw-rich _ 2026", "Projects/Apollo"]);
    assert_eq!(result.moved, [moved(901, "INBOX", 1, 500, 1, 907), moved(902, "INBOX", 2, 500, 2, 907), moved(903, "INBOX", 5, 501, 1, 908)]);
    assert_eq!(server_uids(&imap, "Receipts/Z&APw-rich _ 2026"), [1, 2]);
    assert_eq!(on_server(&imap, "INBOX"), [(6, "\\Deleted".into())]);
    // From the next run on, the new mailboxes are read like any other, by the folders they were made for.
    let downloaded = downloads(&imap);
    let folders = applied(&folders, &result);
    let again = sync(&pushing(&imap, &folders, Vec::new()));
    assert_eq!((again.errors.len(), again.new_count(), downloads(&imap)), (0, 0, downloaded));
    assert_eq!(delta(&again, "Receipts/Zürich _ 2026").id, Some(500));
    assert_eq!(delta(&again, "Projects/Apollo").flags.len(), 1);
    assert_eq!(changes(&imap).len(), 6);

    // A server that parts the levels with dots, and a name with one in it.
    let imap = two_way(true, true);
    imap.world().delimiter = Some('.');
    let mut folders = without(synced_folders(&imap), "INBOX", &[1]);
    folders.push(FolderState { id: 500, ..Default::default() });
    let result = sync(&pushing(&imap, &folders, vec![create(901, 1, "Projects", 500, &["v1.2"])]));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.created, [Created { folder: 500, remote: "Projects.v1_2".into() }]);
    assert_eq!(server_uids(&imap, "Projects.v1_2"), [1]);
}

#[test]
fn a_mailbox_that_is_there_already_is_used_or_left_alone() {
    let imap = two_way(true, true);
    let mut folders = without(synced_folders(&imap), "INBOX", &[1, 2]);
    folders.extend([FolderState { id: 500, ..Default::default() }, FolderState { id: 501, ..Default::default() }]);
    // Made by another program since the last run: nobody here follows it yet.
    imap.world().mailboxes.push(mailbox("Extra", "\\HasNoChildren", 71, Vec::new()));
    let create = |message, uid, to_folder, name: &str| RemoteOp::Move {
        message,
        from: "INBOX".into(),
        uid,
        to: String::new(),
        to_folder,
        create: vec![name.into()],
    };
    // "Projects" is another folder's mailbox: a second folder can't have it.
    let result = sync(&pushing(&imap, &folders, vec![create(901, 1, 500, "Extra"), create(902, 2, 501, "Projects")]));
    assert_eq!(result.errors, ["Couldn't make the folder \"Projects\" on the server: it has another folder of that name"]);
    assert_eq!(result.created, [Created { folder: 500, remote: "Extra".into() }]);
    assert_eq!(result.moved, [moved(901, "INBOX", 1, 500, 1, 71)]);
    assert_eq!(changes(&imap), ["UID MOVE 1 \"Extra\""]);
    // The message for it is where it was, and waits.
    assert_eq!(server_uids(&imap, "INBOX"), [2, 5, 6]);
    assert_eq!(server_uids(&imap, "Projects"), [1]);

    // A destination that is gone from the server: its messages wait too, and are not lost.
    let imap = two_way(true, true);
    let folders = without(synced_folders(&imap), "INBOX", &[1, 2]);
    imap.world().mailboxes.retain(|m| m.raw != "Projects");
    let ops = vec![moving(901, "INBOX", 1, "Projects", PROJECTS), moving(902, "INBOX", 2, "Projects", PROJECTS)];
    let result = sync(&pushing(&imap, &folders, ops));
    assert_eq!(result.errors, ["Couldn't move messages to \"Projects\": the folder is no longer on the server"]);
    assert!(result.moved.is_empty() && changes(&imap).is_empty());
    assert_eq!(server_uids(&imap, "INBOX"), [1, 2, 5, 6]);

    // A refused CREATE is said once, however many messages wait for the folder.
    let imap = two_way(true, true);
    let mut folders = without(synced_folders(&imap), "INBOX", &[1, 2]);
    folders.push(FolderState { id: 500, ..Default::default() });
    imap.world().refuse = vec!["CREATE".into()];
    let result = sync(&pushing(&imap, &folders, vec![create(901, 1, 500, "New"), create(902, 2, 500, "New")]));
    assert_eq!(result.errors, ["Couldn't make the folder \"New\" on the server: the server said: [SERVERBUG] that can't be done"]);
    assert!(result.moved.is_empty() && result.created.is_empty());
    assert_eq!((server_uids(&imap, "INBOX"), sent(&imap, "CREATE")), (vec![1, 2, 5, 6], 1));
}

#[test]
fn a_deletion_takes_the_deleted_message_and_no_other() {
    let imap = two_way(true, true);
    let folders = without(synced_folders(&imap), "INBOX", &[2]);
    let result = sync(&pushing(&imap, &folders, vec![deleting("INBOX", 2)]));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.deleted, [deleted("INBOX", 2)]);
    assert_eq!(changes(&imap), ["UID STORE 2 +FLAGS.SILENT (\\Deleted)", "UID EXPUNGE 2"]);
    // The one another program marked stays for that program to remove.
    assert_eq!(on_server(&imap, "INBOX"), [(1, "\\Seen".into()), (5, "\\Draft $Forwarded".into()), (6, "\\Deleted".into())]);
    assert!(never_swept(&imap));
    assert!(delta(&result, "INBOX").removed.is_empty());

    // A server that can't remove one message keeps it marked: no mail program shows it, and
    // the sweep that would take other programs' deletions is not sent.
    let imap = two_way(true, false);
    let folders = without(synced_folders(&imap), "INBOX", &[2]);
    let result = sync(&pushing(&imap, &folders, vec![deleting("INBOX", 2)]));
    assert_eq!((result.errors.len(), result.deleted.as_slice()), (0, &[deleted("INBOX", 2)][..]));
    assert_eq!(changes(&imap), ["UID STORE 2 +FLAGS.SILENT (\\Deleted)"]);
    let marked: Vec<(u32, bool)> = on_server(&imap, "INBOX").iter().map(|(uid, flags)| (*uid, flags.contains("\\Deleted"))).collect();
    assert_eq!(marked, [(1, false), (2, true), (5, false), (6, true)]);
    assert!(delta(&result, "INBOX").flags.iter().all(|f| f.uid != 2));
}

/// The local Archive folder of [`local_folders`].
const ARCHIVE: Id = 104;

/// A Gmail account (as Google describes it for IMAP): labels as mailboxes, and every message that
/// is not in the trash also in all mail, which has one message archived already.
fn gmail() -> FakeImap {
    let inbox = vec![message(1, "\\Seen", "One"), message(2, "", "Two"), message(3, "", "Three")];
    let mut all = inbox.clone();
    all.push(message(4, "\\Seen", "Archived before"));
    Fake::imap(ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        mailboxes: vec![
            mailbox("INBOX", "\\HasNoChildren", 11, inbox),
            mailbox("[Gmail]", "\\Noselect \\HasChildren", 0, Vec::new()),
            mailbox("[Gmail]/All Mail", "\\HasNoChildren \\All", 51, all),
            mailbox("[Gmail]/Sent Mail", "\\HasNoChildren \\Sent", 21, Vec::new()),
            mailbox("[Gmail]/Trash", "\\HasNoChildren \\Trash", 61, Vec::new()),
        ],
        can_move: true,
        uidplus: true,
        gmail: true,
        ..Default::default()
    })
}

fn archiving(message: Id, uid: u32, create: &str) -> RemoteOp {
    RemoteOp::Move { message, from: "INBOX".into(), uid, to: String::new(), to_folder: ARCHIVE, create: vec![create.into()] }
}

#[test]
fn archiving_at_gmail_takes_the_inbox_label_away_and_makes_none() {
    let imap = gmail();
    let folders = without(synced_folders_of(&imap), "INBOX", &[1]);
    // All mail is no folder here: nothing follows it, and nothing came down from it.
    assert!(folders.iter().all(|f| !f.remote.contains("All Mail")));
    assert_eq!(folders.iter().find(|f| f.id == ARCHIVE).map(|f| f.remote.as_str()), Some(""));
    let downloaded = downloads(&imap);
    let result = sync(&pushing(&imap, &folders, vec![archiving(901, 1, "Archive")]));
    assert_eq!(result.errors, Vec::<String>::new());
    // Out of the Inbox, and in all mail once, under the UID it has there; no label was made.
    assert_eq!(changes(&imap), ["UID MOVE 1 \"[Gmail]/All Mail\""]);
    assert_eq!(sent(&imap, "CREATE") + sent(&imap, "SUBSCRIBE"), 0);
    assert_eq!(server_uids(&imap, "INBOX"), [2, 3]);
    assert_eq!(server_uids(&imap, "[Gmail]/All Mail"), [1, 2, 3, 4]);
    // The Archive folder follows all mail from now on; the message has its UID there.
    assert_eq!(result.created, [Created { folder: ARCHIVE, remote: "[Gmail]/All Mail".into() }]);
    assert_eq!(result.moved, [moved(901, "INBOX", 1, ARCHIVE, 1, 51)]);
    assert!(remotes(&result).iter().all(|remote| !remote.contains("All Mail")));
    assert_eq!((result.new_count(), downloads(&imap)), (0, downloaded));

    // The next runs read the other mailboxes, never all mail, and archive into it by its name.
    let mut folders = applied(&folders, &result);
    folders.iter_mut().find(|f| f.id == ARCHIVE).unwrap().uid_validity = 51;
    for _ in 0..2 {
        let again = sync(&pushing(&imap, &folders, Vec::new()));
        assert_eq!((again.errors.len(), again.new_count(), again.pushed_count(), downloads(&imap)), (0, 0, 0, downloaded));
        assert!(remotes(&again).iter().all(|remote| !remote.contains("All Mail")));
        assert!(again.folders.iter().all(|f| f.removed.is_empty()));
    }
    let folders = without(folders, "INBOX", &[2]);
    let result = sync(&pushing(&imap, &folders, vec![moving(902, "INBOX", 2, "[Gmail]/All Mail", ARCHIVE)]));
    assert_eq!((result.errors.len(), result.created.len()), (0, 0));
    assert_eq!(result.moved, [moved(902, "INBOX", 2, ARCHIVE, 2, 51)]);
    assert_eq!(server_uids(&imap, "INBOX"), [3]);
    assert_eq!(server_uids(&imap, "[Gmail]/All Mail"), [1, 2, 3, 4]);

    // Back to the Inbox: the label is added again, and the message is still in all mail.
    let result = sync(&pushing(
        &imap,
        &folders,
        vec![RemoteOp::Move { message: 901, from: "[Gmail]/All Mail".into(), uid: 1, to: "INBOX".into(), to_folder: 100, create: Vec::new() }],
    ));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.moved, [moved(901, "[Gmail]/All Mail", 1, 100, 4, 11)]);
    assert_eq!(server_uids(&imap, "INBOX"), [3, 4]);
    assert_eq!(server_uids(&imap, "[Gmail]/All Mail"), [1, 2, 3, 4]);
    assert_eq!(sent(&imap, "CREATE"), 0);
}

#[test]
fn deleting_for_good_at_gmail_goes_through_the_trash() {
    let imap = gmail();
    let mut folders = without(synced_folders_of(&imap), "INBOX", &[2]);
    // One archived before, which this folder follows all mail for.
    let archive = folders.iter_mut().find(|f| f.id == ARCHIVE).unwrap();
    (archive.remote, archive.uid_validity, archive.known_uids) = ("[Gmail]/All Mail".into(), 51, vec![4]);
    let folders = without(folders, "[Gmail]/All Mail", &[4]);
    let result = sync(&pushing(&imap, &folders, vec![deleting("INBOX", 2), deleting("[Gmail]/All Mail", 4)]));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.deleted, [deleted("INBOX", 2), deleted("[Gmail]/All Mail", 4)]);
    assert_eq!(
        changes(&imap),
        [
            "UID MOVE 2 \"[Gmail]/Trash\"",
            "UID STORE 1 +FLAGS.SILENT (\\Deleted)",
            "UID EXPUNGE 1",
            "UID MOVE 4 \"[Gmail]/Trash\"",
            "UID STORE 2 +FLAGS.SILENT (\\Deleted)",
            "UID EXPUNGE 2",
        ]
    );
    // Gone from every label, from all mail and from the trash.
    assert_eq!(server_uids(&imap, "INBOX"), [1, 3]);
    assert_eq!(server_uids(&imap, "[Gmail]/All Mail"), [1, 3]);
    assert!(server_uids(&imap, "[Gmail]/Trash").is_empty());
    assert!(never_swept(&imap));
    // Nothing comes down for it.
    let downloaded = downloads(&imap);
    let again = sync(&pushing(&imap, &applied(&folders, &result), Vec::new()));
    assert_eq!((again.errors.len(), again.new_count(), downloads(&imap)), (0, 0, downloaded));

    // What is in the trash already is deleted there, as at any server.
    let imap = gmail();
    imap.world().mailbox("[Gmail]/Trash").messages.push(message(1, "", "Thrown away"));
    let folders = without(synced_folders_of(&imap), "[Gmail]/Trash", &[1]);
    let result = sync(&pushing(&imap, &folders, vec![deleting("[Gmail]/Trash", 1)]));
    assert_eq!((result.errors.len(), result.deleted.as_slice()), (0, &[deleted("[Gmail]/Trash", 1)][..]));
    assert_eq!(changes(&imap), ["UID STORE 1 +FLAGS.SILENT (\\Deleted)", "UID EXPUNGE 1"]);
}

#[test]
fn a_server_that_is_not_gmail_keeps_its_ways_with_all_mail() {
    // All mail, but no word that the server is Gmail's: Archive is made, and a deletion is in place.
    let imap = two_way(true, true);
    let folders = without(synced_folders(&imap), "INBOX", &[1, 2]);
    let result = sync(&pushing(&imap, &folders, vec![archiving(901, 1, "Archive"), deleting("INBOX", 2)]));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.created, [Created { folder: ARCHIVE, remote: "Archive".into() }]);
    assert_eq!(
        changes(&imap),
        ["CREATE \"Archive\"", "SUBSCRIBE \"Archive\"", "UID MOVE 1 \"Archive\"", "UID STORE 2 +FLAGS.SILENT (\\Deleted)", "UID EXPUNGE 2"]
    );
    assert_eq!(server_uids(&imap, "[Gmail]/All Mail"), [1]);

    // Gmail with a mailbox for archived mail of its own (made by the owner, or by an earlier
    // version): that one is used, as before.
    let imap = gmail();
    imap.world().mailboxes.push(mailbox("Archive", "\\HasNoChildren", 71, Vec::new()));
    let folders = without(synced_folders_of(&imap), "INBOX", &[1]);
    let archive = folders.iter().find(|f| f.id == ARCHIVE).unwrap();
    assert_eq!((archive.remote.as_str(), archive.uid_validity), ("Archive", 71));
    let result = sync(&pushing(&imap, &folders, vec![moving(901, "INBOX", 1, "Archive", ARCHIVE)]));
    assert_eq!((result.errors.len(), changes(&imap)), (0, vec!["UID MOVE 1 \"Archive\"".to_string()]));
}

#[test]
fn what_is_gone_from_the_server_leaves_nothing_to_do() {
    let imap = two_way(true, true);
    let mut folders = synced_folders(&imap);
    // A folder whose mailbox the server no longer has.
    folders.push(FolderState { id: 600, remote: "Old".into(), uid_validity: 5, ..Default::default() });
    let ops = vec![
        // Never there, or removed by another program meanwhile; and one another program marked as deleted.
        moving(901, "INBOX", 99, "Projects", PROJECTS),
        moving(902, "INBOX", 6, "Projects", PROJECTS),
        deleting("INBOX", 98),
        flags("INBOX", 97, Some(true), None, None),
        moving(903, "Old", 4, "Projects", PROJECTS),
        deleting("Old", 5),
        flags("Old", 6, Some(true), None, None),
    ];
    let result = sync(&pushing(&imap, &folders, ops));
    // A message that is no longer in its mailbox: finished, not failed. Nobody is told, and
    // nothing is asked again.
    assert_eq!(result.moved, [moved(901, "INBOX", 99, PROJECTS, 0, 0), moved(902, "INBOX", 6, PROJECTS, 0, 0)]);
    assert_eq!(result.deleted, [deleted("INBOX", 98)]);
    // The server takes a flag for a message it doesn't have without a word.
    assert_eq!(result.flagged.iter().map(|f| (f.folder.as_str(), f.uid)).collect::<Vec<_>>(), [("INBOX", 97)]);
    assert_eq!(changes(&imap), ["UID STORE 97 +FLAGS.SILENT (\\Seen)"]);
    assert_eq!((server_uids(&imap, "INBOX"), server_uids(&imap, "Projects")), (vec![1, 2, 5, 6], vec![1]));
    // A whole mailbox that isn't in the server's list is another matter: the list may be short
    // this once. Its changes are not called done; they wait, and the owner is told why.
    assert_eq!(result.errors, ["Couldn't bring \"Old\" up to date: the folder is not on the server"]);
}

#[test]
fn a_failed_step_keeps_the_rest_of_that_mailbox_waiting() {
    let imap = two_way(true, true);
    let folders = without(synced_folders(&imap), "INBOX", &[1, 2]);
    let ops = || {
        vec![
            flags("INBOX", 5, Some(true), None, None),
            moving(901, "INBOX", 1, "Projects", PROJECTS),
            deleting("INBOX", 2),
            flags("Projects", 1, Some(true), None, None),
        ]
    };
    imap.world().refuse = vec!["UID MOVE".into()];
    let result = sync(&pushing(&imap, &folders, ops()));
    assert_eq!(result.errors, ["Couldn't move messages from \"INBOX\" to \"Projects\": the server said: [SERVERBUG] that can't be done"]);
    // The flags went through before; the deletion after it was not tried; the other mailbox was.
    assert_eq!(result.flagged.iter().map(|f| (f.folder.as_str(), f.uid)).collect::<Vec<_>>(), [("INBOX", 5), ("Projects", 1)]);
    assert!(result.moved.is_empty() && result.deleted.is_empty());
    assert_eq!(server_uids(&imap, "INBOX"), [1, 2, 5, 6]);
    assert!(!changes(&imap).iter().any(|command| command.contains("\\Deleted")));
    // The rest of the run went on as usual.
    assert_eq!(result.folders.len(), 5);

    // Flags that are refused stop the mailbox as well.
    imap.world().refuse = vec!["UID STORE 5".into()];
    let result = sync(&pushing(&imap, &folders, ops()));
    assert_eq!(result.errors, ["Couldn't change flags in \"INBOX\": the server said: [SERVERBUG] that can't be done"]);
    assert_eq!((result.flagged.len(), result.moved.len(), result.deleted.len()), (1, 0, 0));
    // So does a mailbox that can't be opened, and one that won't say what it has.
    for (refused, said) in [("SELECT \"INBOX\"", "Couldn't open \"INBOX\" to bring it up to date"), ("UID FETCH 1:2 ", "Couldn't read \"INBOX\"")] {
        imap.world().refuse = vec![refused.into()];
        let result = sync(&pushing(&imap, &folders, ops()));
        assert!(result.errors.len() == 1 && result.errors[0].starts_with(said), "{:?}", result.errors);
        assert!(result.moved.is_empty() && result.deleted.is_empty());
    }
    assert_eq!(server_uids(&imap, "INBOX"), [1, 2, 5, 6]);

    // A connection lost in the middle: what was done is said, everything else waits, the
    // download included.
    imap.world().refuse.clear();
    imap.world().hang_up_on = Some("UID MOVE".into());
    let result = sync(&pushing(&imap, &folders, ops()));
    assert_eq!(result.errors, ["Couldn't move messages from \"INBOX\" to \"Projects\": connection problem: the server closed the connection"]);
    assert_eq!((result.flagged.len(), result.moved.len(), result.deleted.len(), result.folders.len()), (1, 0, 0, 0));
    assert_eq!(server_uids(&imap, "INBOX"), [1, 2, 5, 6]);
    assert!(never_swept(&imap));

    // When the server is itself again, the same requests go through.
    imap.world().hang_up_on = None;
    let result = sync(&pushing(&imap, &folders, ops()));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!((result.flagged.len(), result.moved.len(), result.deleted.len()), (2, 1, 1));
    assert_eq!((server_uids(&imap, "INBOX"), server_uids(&imap, "Projects")), (vec![5, 6], vec![1, 2]));
}

#[test]
fn nothing_is_pushed_into_a_mailbox_that_was_made_anew() {
    let imap = two_way(true, true);
    let folders = without(synced_folders(&imap), "INBOX", &[1]);
    {
        let mut world = imap.world();
        let inbox = inbox(&mut world);
        inbox.uid_validity = 12;
        inbox.messages = vec![message(1, "", "Another one"), message(2, "", "Another two")];
    }
    // The UIDs meant other messages when these were asked for.
    let ops = || vec![flags("INBOX", 2, Some(true), None, None), moving(901, "INBOX", 1, "Projects", PROJECTS), deleting("INBOX", 2)];
    let result = sync(&pushing(&imap, &folders, ops()));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.pushed_count(), 0);
    assert!(changes(&imap).is_empty());
    assert_eq!(on_server(&imap, "INBOX"), [(1, String::new()), (2, String::new())]);
    assert_eq!(server_uids(&imap, "Projects"), [1]);
    // The download says so: everything from the old life goes, what is there now is new.
    let inbox_delta = delta(&result, "INBOX");
    assert_eq!((inbox_delta.uid_validity, inbox_delta.removed.as_slice(), inbox_delta.new.len()), (12, &[2, 5][..], 2));

    // Nor into one whose UIDs were never learned here.
    let imap = two_way(true, true);
    let mut folders = synced_folders(&imap);
    folders.iter_mut().find(|f| f.remote == "INBOX").unwrap().uid_validity = 0;
    let result = sync(&pushing(&imap, &folders, ops()));
    assert_eq!((result.errors.len(), result.pushed_count()), (0, 0));
    assert!(changes(&imap).is_empty());
    // Nor into one no folder here follows at all.
    let ops = vec![deleting("[Gmail]/All Mail", 1), flags("[Gmail]/All Mail", 1, Some(true), None, None)];
    let result = sync(&pushing(&imap, &synced_folders(&imap), ops));
    assert!(result.pushed_count() == 0 && changes(&imap).is_empty(), "{result:?}");
    assert_eq!(server_uids(&imap, "[Gmail]/All Mail"), [1]);
}

#[test]
fn a_server_is_only_believed_about_new_uids_when_it_is_clear() {
    // Too many, too few, another message's, none at all, nonsense: nothing is known, and the
    // message comes down to be matched by what it is.
    for lie in ["31 1 2,3", "31 1,2 2", "31 2 1", "0 1 2", "31 1:* 1:*", "junk", "31 1 99999999999", "31 1 0", ""] {
        let imap = two_way(true, true);
        imap.world().copyuid = Some(lie.into());
        let folders = without(synced_folders(&imap), "INBOX", &[1]);
        let result = sync(&pushing(&imap, &folders, vec![moving(901, "INBOX", 1, "Projects", PROJECTS)]));
        assert_eq!(result.errors, Vec::<String>::new(), "{lie:?}");
        assert_eq!(result.moved, [moved(901, "INBOX", 1, PROJECTS, 0, 0)], "{lie:?}");
        assert_eq!(uids(&delta(&result, "Projects").new), [2], "{lie:?}");
    }
    // A lie that is clear reads like the truth. The UID it names is not there, which the
    // download says; the message that is there comes down as new. Nothing is lost to it.
    let imap = two_way(true, true);
    imap.world().copyuid = Some("31 1 77".into());
    let folders = without(synced_folders(&imap), "INBOX", &[1]);
    let result = sync(&pushing(&imap, &folders, vec![moving(901, "INBOX", 1, "Projects", PROJECTS)]));
    assert_eq!(result.moved, [moved(901, "INBOX", 1, PROJECTS, 77, 31)]);
    let projects = delta(&result, "Projects");
    assert_eq!((projects.removed.as_slice(), uids(&projects.new)), (&[77][..], vec![2]));
    // One that speaks of another life of the mailbox is not taken for knowledge of this one.
    let imap = two_way(true, true);
    imap.world().copyuid = Some("32 1 2".into());
    let folders = without(synced_folders(&imap), "INBOX", &[1]);
    let result = sync(&pushing(&imap, &folders, vec![moving(901, "INBOX", 1, "Projects", PROJECTS)]));
    assert_eq!(result.moved, [moved(901, "INBOX", 1, PROJECTS, 2, 32)]);
    assert_eq!(uids(&delta(&result, "Projects").new), [2]);
}

#[test]
fn many_changes_go_up_in_few_commands() {
    let imap = two_way(true, true);
    {
        let mut world = imap.world();
        let inbox = inbox(&mut world);
        inbox.messages = (1..=1_200).map(|uid| message(uid, "", &format!("Message {uid}"))).collect();
    }
    let mut first = request(account(Some(&imap), None), local_folders());
    first.max_per_folder = 1;
    let mut folders = applied(&local_folders(), &sync(&first));
    // Every third message was read here, and every seventh moved away.
    folders.iter_mut().find(|f| f.remote == "INBOX").unwrap().known_uids = (1..=1_200).filter(|uid| uid % 7 != 0).collect();
    let read_here = (1..=1_200u32).filter(|uid| uid % 3 == 0 && uid % 7 != 0);
    let mut ops: Vec<RemoteOp> = read_here.map(|uid| flags("INBOX", uid, Some(true), None, None)).collect();
    ops.extend((1..=1_200u32).filter(|uid| uid % 7 == 0).map(|uid| moving(Id::from(uid), "INBOX", uid, "Projects", PROJECTS)));
    let result = sync(&pushing(&imap, &folders, ops));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!((result.flagged.len(), result.moved.len()), (343, 171));
    assert!(result.moved.iter().all(|m| m.new_uid != 0 && m.uid_validity == 31));
    let (stores, moves) = (sent(&imap, "UID STORE"), sent(&imap, "UID MOVE"));
    assert!(stores <= 10 && moves <= 5, "{stores} {moves}");
    let read = on_server(&imap, "INBOX").iter().filter(|(_, flags)| flags == "\\Seen").count();
    assert_eq!((read, server_uids(&imap, "INBOX").len(), server_uids(&imap, "Projects").len()), (343, 1_029, 172));
    // Each moved message is under the UID the server paired it with.
    let world = imap.world();
    let body_of =
        |uid: u32| world.mailboxes.iter().find(|m| m.raw == "Projects").unwrap().messages.iter().find(|m| m.uid == uid).map(|m| m.body.clone());
    for done in &result.moved {
        let body = String::from_utf8(body_of(done.new_uid).unwrap()).unwrap();
        assert!(body.contains(&format!("Subject: Message {}\r\n", done.uid)), "{done:?}");
    }
}

#[test]
fn a_copy_of_what_was_sent_is_kept_on_the_server() {
    let imap = two_way(true, true);
    let smtp = Fake::smtp(smtp_world());
    let folders = synced_folders(&imap);
    let to = |addresses: &[&str]| addresses.iter().map(|a| Address::new("", *a)).collect::<Vec<_>>();
    let lunch = Message {
        id: 501,
        message_id: "lunch@example.org".into(),
        from: Address::new("Ada Lovelace", "ada@example.org"),
        to: to(&["bob@example.org"]),
        bcc: to(&["erin@example.org"]),
        subject: "Lunch".into(),
        date: SENT_AT,
        body: mailcraft_model::Body { text: "See you at noon.".into(), html: None },
        ..Default::default()
    };
    let undated =
        Message { id: 503, message_id: "later@example.org".into(), to: to(&["bob@example.org"]), subject: "Later".into(), ..Default::default() };
    let outbox = vec![lunch, Message { id: 502, subject: "To nobody".into(), ..Default::default() }, undated];
    let result = sync(&SyncRequest { outbox, ..request(account(Some(&imap), Some(&smtp)), folders.clone()) });
    assert_eq!(result.errors, ["Couldn't send \"To nobody\": the message has no recipients"]);
    // What went out is filed, marked as read; what didn't, isn't.
    assert_eq!((result.sent.as_slice(), result.filed.as_slice()), (&[501, 503][..], &[501, 503][..]));
    assert_eq!(on_server(&imap, "Sent Items"), [(3, "\\Seen".into()), (4, "\\Seen".into()), (5, "\\Seen".into())]);
    assert_eq!(changes(&imap).iter().map(|c| c.split('{').next().unwrap().to_string()).collect::<Vec<_>>(), ["APPEND \"Sent Items\" (\\Seen) "; 2]);
    let kept: Vec<String> =
        imap.world().mailbox("Sent Items").messages.iter().skip(1).map(|m| String::from_utf8_lossy(&m.body).into_owned()).collect();
    let went: Vec<String> = smtp.world().delivered.iter().map(|d| d.data.clone()).collect();
    // The sender's copy says who got a blind copy; what went out does not.
    assert!(kept[0].contains("Bcc: erin@example.org\r\n") && !went[0].to_lowercase().contains("bcc"), "{}", kept[0]);
    assert_eq!(kept[0].replace("Bcc: erin@example.org\r\n", "").trim_end(), went[0].trim_end());
    // A message dated when it went keeps that date in its copy.
    assert_eq!(kept[1].trim_end(), went[1].trim_end());
    assert!(kept[1].contains("Date: ") && !kept[1].contains("1970"), "{}", kept[1]);
    // The copies come down in the same run, each with the Message-ID of the message it is a
    // copy of, for the mailbox to match them instead of showing them twice.
    let sent_items = delta(&result, "Sent Items");
    assert_eq!(
        sent_items.new.iter().map(|m| (m.uid, m.message_id.as_str(), m.read, m.bcc.len())).collect::<Vec<_>>(),
        [(4, "lunch@example.org", true, 1), (5, "later@example.org", true, 0)]
    );
    let again = sync(&pushing(&imap, &applied(&folders, &result), Vec::new()));
    assert_eq!((again.errors.len(), again.new_count(), again.filed.len()), (0, 0, 0));

    // A server that keeps what is sent through it gets no second copy: the mailbox is told that
    // one is on its way, to be matched when it shows.
    let files_it_itself = SyncRequest { outbox: vec![hello()], ..request(account(Some(&imap), Some(&smtp)), folders.clone()) };
    let result = whole(&files_it_itself, Limits::in_pieces(), Host { files_sent: true, microsoft: false });
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!((result.sent.as_slice(), result.filed.as_slice()), (&[9][..], &[9][..]));
    assert_eq!((sent(&imap, "APPEND"), server_uids(&imap, "Sent Items").len()), (2, 3));
    // Also when its incoming side can't be reached just now.
    imap.world().password = "changed".into();
    let result = whole(&files_it_itself, Limits::in_pieces(), Host { files_sent: true, microsoft: false });
    assert_eq!((result.errors.len(), result.sent.as_slice(), result.filed.as_slice()), (1, &[9][..], &[9][..]));
    // A server that doesn't, and can't be reached: the mailbox was told to expect the copy when
    // the message went, and is told that it didn't get there.
    let all = pieces(&files_it_itself, Limits::in_pieces());
    assert_eq!(
        all.iter().map(|piece| (piece.sent.len(), piece.filed.len(), piece.unfiled.len())).collect::<Vec<_>>(),
        [(0, 0, 0), (1, 1, 0), (0, 0, 0), (0, 0, 1)]
    );
    let result = sync(&files_it_itself);
    assert_eq!((result.errors.len(), result.sent.as_slice(), result.filed.len(), result.unfiled.as_slice()), (1, &[9][..], 0, &[9][..]));
    imap.world().password = PASSWORD.into();

    // A copy the server won't take is said, and the rest of the run goes on.
    imap.world().refuse = vec!["APPEND".into()];
    let result = sync(&files_it_itself);
    assert_eq!(result.errors, ["Couldn't keep a copy of \"Hello\" on the server: the server said: [SERVERBUG] that can't be done"]);
    assert_eq!((result.sent.as_slice(), result.filed.len(), result.unfiled.as_slice(), result.folders.len()), (&[9][..], 0, &[9][..], 5));
    imap.world().refuse.clear();
    // A connection lost while the copy was on its way: it may have arrived all the same. Should
    // it show later, it is that message's copy, so the mailbox is told to expect it.
    imap.world().hang_up_on = Some("APPEND".into());
    let result = sync(&files_it_itself);
    assert_eq!(result.errors, ["Couldn't keep a copy of \"Hello\" on the server: connection problem: the server closed the connection"]);
    assert_eq!((result.sent.as_slice(), result.filed.as_slice(), result.unfiled.len(), result.folders.len()), (&[9][..], &[9][..], 0, 0));
    imap.world().hang_up_on = None;
    // A server without a mailbox for sent mail has nowhere to put one: the copy here is the one there is.
    imap.world().mailboxes.retain(|m| m.raw != "Sent Items");
    let result = sync(&files_it_itself);
    assert_eq!((result.errors.len(), result.sent.as_slice(), result.filed.len(), result.unfiled.as_slice()), (0, &[9][..], 0, &[9][..]));
    assert_eq!(sent(&imap, "APPEND"), 4);
}

#[test]
fn mailboxes_are_spoken_of_as_the_server_writes_them() {
    // An Inbox in the server's own spelling, and names that aren't plain letters.
    let world = ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        can_move: true,
        uidplus: true,
        mailboxes: vec![
            mailbox("Inbox", "\\HasNoChildren", 11, vec![message(1, "", "One"), message(2, "", "Two")]),
            mailbox("Entw&APw-rfe", "\\Drafts", 41, vec![message(1, "", "Draft")]),
            mailbox("Archiv/&AMQ-lteres", "\\HasNoChildren", 51, Vec::new()),
        ],
        ..Default::default()
    };
    let imap = Fake::imap(world);
    let folders = without(synced_folders_of(&imap), "INBOX", &[1]);
    let older = folders.iter().find(|f| f.remote == "Archiv/Älteres").unwrap().id;
    // Here they go by their names as people read them, and the Inbox by its one name.
    let ops = vec![
        moving(901, "INBOX", 1, "Archiv/Älteres", older),
        flags("INBOX", 2, Some(true), None, None),
        flags("Entwürfe", 1, None, Some(true), None),
    ];
    let result = sync(&pushing(&imap, &folders, ops));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.moved, [moved(901, "INBOX", 1, older, 1, 51)]);
    assert_eq!(result.flagged.iter().map(|f| (f.folder.as_str(), f.uid)).collect::<Vec<_>>(), [("INBOX", 2), ("Entwürfe", 1)]);
    assert_eq!(changes(&imap), ["UID STORE 2 +FLAGS.SILENT (\\Seen)", "UID MOVE 1 \"Archiv/&AMQ-lteres\"", "UID STORE 1 +FLAGS.SILENT (\\Flagged)"]);
    assert_eq!((sent(&imap, "SELECT \"Inbox\""), sent(&imap, "SELECT \"Entw&APw-rfe\"")), (1, 1));
    assert_eq!((on_server(&imap, "Inbox"), server_uids(&imap, "Archiv/&AMQ-lteres")), (vec![(2, "\\Seen".to_string())], vec![1]));
}

// ---- an answer in pieces ----

/// A run's answer piece by piece, as a host that shows mail while it arrives is handed it.
fn pieces(request: &SyncRequest, limits: Limits) -> Vec<SyncResult> {
    let mut all = Vec::new();
    sync_as(request, limits, Host::default(), &AtomicBool::new(false), &mut |piece| all.push(piece));
    all
}

/// A run's answer in one, from a server of which its name tells this much.
fn whole(request: &SyncRequest, limits: Limits, host: Host) -> SyncResult {
    let mut whole = SyncResult::default();
    sync_as(request, limits, host, &AtomicBool::new(false), &mut |piece| whole.absorb(piece));
    whole
}

/// Limits under which a mailbox's new messages come so many to a piece, however long they take.
fn batches_of(messages: usize) -> Limits {
    Limits { batch_messages: messages, batch_bytes: usize::MAX, batch_wait: Duration::MAX, ..Limits::in_pieces() }
}

/// Limits for a server that may not answer: it is not waited for long.
fn impatient(io: Duration, command: Duration) -> Limits {
    Limits { patience: Patience { io, command, ..Patience::default() }, ..Limits::kept_whole() }
}

/// A mailbox worth watching arrive: a few hundred messages over several folders.
fn large_world() -> ImapWorld {
    let many =
        |name: &str, count: u32| (1..=count).map(|uid| message(uid, if uid % 3 == 0 { "\\Seen" } else { "" }, &format!("{name} {uid}"))).collect();
    ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        mailboxes: vec![
            mailbox("Projects", "\\HasNoChildren", 31, many("Project", 25)),
            mailbox("Archive", "\\HasNoChildren \\Archive", 71, many("Filed", 40)),
            mailbox("INBOX", "\\HasNoChildren", 11, many("Mail", 230)),
            mailbox("Sent Items", "\\HasNoChildren \\Sent", 21, many("Sent", 60)),
            mailbox("Drafts", "\\HasNoChildren \\Drafts", 41, Vec::new()),
        ],
        ..Default::default()
    }
}

/// A mailbox as the engine keeps one, as far as a run can tell: its folders, and its mail by
/// server mailbox and UID.
#[derive(Clone)]
struct Kept {
    folders: Vec<FolderState>,
    mail: BTreeMap<String, BTreeMap<u32, String>>,
}

impl Kept {
    fn new() -> Kept {
        Kept { folders: local_folders(), mail: BTreeMap::new() }
    }

    /// Take a piece in. Nothing comes that is here already.
    fn take(&mut self, piece: &SyncResult) {
        for delta in &piece.folders {
            let here = self.mail.entry(delta.remote.clone()).or_default();
            here.retain(|uid, _| !delta.removed.contains(uid));
            for m in &delta.new {
                assert!(here.insert(m.uid, m.subject.clone()).is_none(), "{} came into {} twice", m.uid, delta.remote);
            }
        }
        self.folders = applied(&self.folders, piece);
    }

    /// The mail, without the mailboxes that have none.
    fn mail(&self) -> BTreeMap<String, BTreeMap<u32, String>> {
        self.mail.iter().filter(|(_, mail)| !mail.is_empty()).map(|(remote, mail)| (remote.clone(), mail.clone())).collect()
    }

    fn count(&self) -> usize {
        self.mail.values().map(BTreeMap::len).sum()
    }
}

/// What the server has for a mail program to show in the mailboxes named (as they are written
/// there, and as they are known here): UID and subject.
fn served(imap: &FakeImap, mailboxes: &[(&str, &str)]) -> BTreeMap<String, BTreeMap<u32, String>> {
    let mut world = imap.world();
    let mut all = BTreeMap::new();
    for (raw, remote) in mailboxes {
        let subject = |m: &FakeMessage| String::from_utf8_lossy(&m.body).lines().find_map(|line| line.strip_prefix("Subject: ").map(str::to_string));
        let mail: BTreeMap<u32, String> =
            world.mailbox(raw).messages.iter().filter(|m| !m.has("\\Deleted")).map(|m| (m.uid, subject(m).unwrap())).collect();
        if !mail.is_empty() {
            all.insert(remote.to_string(), mail);
        }
    }
    all
}

const LARGE: [(&str, &str); 5] =
    [("INBOX", "INBOX"), ("Sent Items", "Sent Items"), ("Drafts", "Drafts"), ("Archive", "Archive"), ("Projects", "Projects")];

#[test]
fn a_mailbox_arrives_in_pieces_the_inbox_first() {
    let imap = Fake::imap(large_world());
    let asked = request(account(Some(&imap), None), local_folders());
    let all = pieces(&asked, batches_of(50));
    // Every piece but the last says that more is to come, and how far the run has got.
    let (last, before) = all.split_last().unwrap();
    assert!(!last.partial && last.progress.is_empty() && !last.stopped && last.step.is_none());
    assert!(before.iter().all(|piece| piece.partial && !piece.stopped));
    assert!(all.iter().all(|piece| piece.errors.is_empty() && piece.account == 7 && piece.email == "ada@example.org"));
    // The line is made of the numbers that come with it (#120).
    assert!(before.iter().all(|piece| piece.step.as_ref().is_some_and(|step| step.line() == piece.progress)));
    let reading = before.iter().filter_map(|piece| piece.step.as_ref()).find(|step| step.stage == SyncStage::Reading).unwrap();
    assert_eq!((reading.folder.as_str(), reading.done, reading.total, reading.stage.phase()), ("Inbox", 50, 230, SyncPhase::NewMail));
    assert_eq!(
        before.iter().map(|piece| piece.progress.as_str()).collect::<Vec<_>>(),
        [
            "Connecting to the mail server",
            "Found 5 folders",
            "Checking Inbox (1 of 5)",
            "Inbox: 50 of 230",
            "Inbox: 100 of 230",
            "Inbox: 150 of 230",
            "Inbox: 200 of 230",
            "Checking Sent Items (2 of 5)",
            "Sent Items: 50 of 60",
            "Checking Drafts (3 of 5)",
            "Checking Archive (4 of 5)",
            "Checking Projects (5 of 5)",
        ]
    );
    // The Inbox first, then sent mail, drafts and what was filed, then the rest; one mailbox is
    // through before the next begins.
    let mut order: Vec<&str> = all.iter().flat_map(|piece| piece.folders.iter().map(|f| f.remote.as_str())).collect();
    order.dedup();
    assert_eq!(order, ["INBOX", "Sent Items", "Drafts", "Archive", "Projects"]);
    // The Inbox comes newest first, fifty at a time, and the first of it with the third piece.
    let batches: Vec<_> = all.iter().flat_map(|piece| &piece.folders).filter(|f| f.remote == "INBOX").collect();
    assert_eq!(batches.iter().map(|f| f.new.len()).collect::<Vec<_>>(), [50, 50, 50, 50, 30]);
    assert_eq!((uids(&batches[0].new), uids(&batches[4].new)), ((181..=230).collect::<Vec<_>>(), (1..=30).collect::<Vec<_>>()));
    assert_eq!((all[3].new_count(), all[3].folders[0].new[49].subject.as_str()), (50, "Mail 230"));
    // It is only said to be read once the last of it is here.
    assert_eq!(batches.iter().map(|f| f.uid_seen).collect::<Vec<_>>(), [0, 0, 0, 0, 230]);
    assert!(batches.iter().all(|f| f.id == Some(100)
        && f.uid_validity == 11
        && f.kind == FolderKind::Inbox
        && f.flags.is_empty()
        && f.removed.is_empty()));
    // Put together, the pieces are what a run whose answer is kept whole brings.
    let mut together = SyncResult::default();
    all.iter().cloned().for_each(|piece| together.absorb(piece));
    together.folders.iter_mut().for_each(|f| f.new.sort_by_key(|m| m.uid));
    assert_eq!((together.new_count(), downloads(&imap)), (355, 355));
    // (How long each took is of each run alone.)
    let mut whole = sync(&asked);
    (together.timings, whole.timings) = Default::default();
    assert_eq!(together, whole);
    assert_eq!(imap.world().connections, 2);

    // The next run says what changed with the first piece of a mailbox, and brings what is new.
    let mut here = Kept::new();
    all.iter().for_each(|piece| here.take(piece));
    assert_eq!(here.mail(), served(&imap, &LARGE));
    {
        let mut world = imap.world();
        let there = inbox(&mut world);
        there.messages.retain(|m| m.uid != 7);
        there.messages[0].flags = "\\Flagged".into();
        there.messages.extend((231..=300).map(|uid| message(uid, "", &format!("Mail {uid}"))));
    }
    let next = pieces(&request(account(Some(&imap), None), here.folders.clone()), batches_of(50));
    let batches: Vec<_> = next.iter().flat_map(|piece| &piece.folders).filter(|f| f.remote == "INBOX").collect();
    assert_eq!(
        batches.iter().map(|f| (f.new.len(), f.uid_seen, f.flags.len(), f.removed.clone())).collect::<Vec<_>>(),
        [(50, 230, 229, vec![7]), (20, 300, 0, vec![])]
    );
    assert!(batches[0].flags[0].flagged);
    next.iter().for_each(|piece| here.take(piece));
    assert_eq!(here.mail(), served(&imap, &LARGE));
}

#[test]
fn whatever_part_of_a_run_is_taken_in_the_next_run_brings_the_rest() {
    let imap = Fake::imap(large_world());
    let ada = account(Some(&imap), None);
    // With every message wanted, and with the hundred newest of a mailbox.
    for limit in [0, 100] {
        let asking = |folders: Vec<FolderState>| SyncRequest { max_per_folder: limit, ..request(ada.clone(), folders) };
        let all = pieces(&asking(local_folders()), batches_of(40));
        // What a run leaves that is taken in to the end.
        let mut through = Kept::new();
        all.iter().for_each(|piece| through.take(piece));
        let mut expected = served(&imap, &LARGE);
        if limit != 0 {
            expected.get_mut("INBOX").unwrap().retain(|uid, _| *uid > 130);
        }
        assert_eq!(through.mail(), expected);
        for taken in 0..=all.len() {
            // The first pieces are taken in, and then nobody is there for the others.
            let mut here = Kept::new();
            all[..taken].iter().for_each(|piece| here.take(piece));
            let (had, asked_before) = (here.count(), downloads(&imap));
            // The next run asks for what is missing and for nothing else, and nothing comes twice.
            let rest = pieces(&asking(here.folders.clone()), batches_of(40));
            assert!(rest.iter().all(|piece| piece.errors.is_empty()), "after {taken} pieces");
            rest.iter().for_each(|piece| here.take(piece));
            assert_eq!(here.mail(), expected, "after {taken} of {} pieces, limit {limit}", all.len());
            assert_eq!(downloads(&imap) - asked_before, through.count() - had, "after {taken} pieces, limit {limit}");
            // Then all is as a run that went through left it: another one finds nothing to do.
            assert_eq!(
                here.folders.iter().map(|f| (&f.remote, f.uid_seen)).collect::<Vec<_>>(),
                through.folders.iter().map(|f| (&f.remote, f.uid_seen)).collect::<Vec<_>>()
            );
            let quiet = sync(&asking(here.folders.clone()));
            assert_eq!(
                (quiet.errors.len(), quiet.new_count(), quiet.folders.iter().map(|f| f.removed.len()).sum::<usize>()),
                (0, 0, 0),
                "after {taken} pieces"
            );
        }
    }
}

#[test]
fn a_mailbox_that_was_made_anew_arrives_in_pieces_like_any_other() {
    let imap = Fake::imap(imap_world());
    let ada = account(Some(&imap), None);
    let mut before = Kept::new();
    before.take(&sync(&request(ada.clone(), local_folders())));
    {
        let mut world = imap.world();
        let there = inbox(&mut world);
        there.uid_validity = 12;
        there.messages = (1..=7).map(|uid| message(uid, "", &format!("Anew {uid}"))).collect();
    }
    let expected = served(&imap, &[("INBOX", "INBOX"), ("Sent Items", "Sent Items"), ("Projects", "Projects")]);
    let all = pieces(&request(ada.clone(), before.folders.clone()), batches_of(2));
    // The first piece that speaks of the Inbox says that what was there is gone, and starts the
    // count of how far the new one is read from below its first batch.
    let first = all.iter().flat_map(|piece| &piece.folders).find(|f| f.remote == "INBOX").unwrap();
    assert_eq!((first.uid_validity, first.removed.as_slice(), uids(&first.new), first.uid_seen), (12, &[1, 2, 5][..], vec![6, 7], 0));
    for taken in 0..=all.len() {
        let mut here = before.clone();
        all[..taken].iter().for_each(|piece| here.take(piece));
        let rest = sync(&request(ada.clone(), here.folders.clone()));
        assert_eq!(rest.errors, Vec::<String>::new());
        here.take(&rest);
        assert_eq!(here.mail(), expected, "after {taken} of {} pieces", all.len());
        assert_eq!(here.folders.iter().find(|f| f.remote == "INBOX").map(|f| (f.uid_validity, f.uid_seen)), Some((12, 7)), "after {taken} pieces");
    }
}

#[test]
fn a_run_called_off_anywhere_is_finished_by_the_next() {
    let imap = Fake::imap(large_world());
    let ada = account(Some(&imap), None);
    let expected = served(&imap, &LARGE);
    let count = pieces(&request(ada.clone(), local_folders()), batches_of(40)).len();
    for after in 0..count {
        // Called off by whoever takes the pieces in, once so many have come.
        let cancel = AtomicBool::new(false);
        let (mut here, mut came, mut stopped) = (Kept::new(), 0, false);
        sync_as(&request(ada.clone(), local_folders()), batches_of(40), Host::default(), &cancel, &mut |piece| {
            here.take(&piece);
            came += 1;
            stopped = piece.stopped;
            assert!(piece.errors.is_empty(), "{:?}", piece.errors);
            cancel.store(came > after, Ordering::Relaxed);
        });
        // It stopped soon: with the piece under way at most, and the one that ends the run.
        assert!(came <= after + 3 && (stopped || after + 2 >= count), "{came} pieces when called off after {after} of {count}");
        let asked_before = downloads(&imap);
        let had = here.count();
        let rest = sync(&request(ada.clone(), here.folders.clone()));
        assert_eq!(rest.errors, Vec::<String>::new());
        here.take(&rest);
        assert_eq!(here.mail(), expected, "called off after {after} pieces");
        assert_eq!(downloads(&imap) - asked_before, 355 - had, "called off after {after} pieces");
    }
}

#[test]
fn a_run_that_is_called_off_stops_before_the_next_command_and_says_goodbye() {
    let imap = Fake::imap(large_world());
    let smtp = Fake::smtp(smtp_world());
    let ada = account(Some(&imap), Some(&smtp));
    let cancel = AtomicBool::new(false);
    let mut all: Vec<SyncResult> = Vec::new();
    sync_as(&request(ada.clone(), local_folders()), batches_of(50), Host::default(), &cancel, &mut |piece| {
        // Once the second batch of the Inbox is here.
        cancel.store(piece.progress == "Inbox: 100 of 230", Ordering::Relaxed);
        all.push(piece);
    });
    let last = all.last().unwrap();
    assert!(last.stopped && !last.partial && last.progress.is_empty());
    // Being called off is no problem to tell.
    assert!(all.iter().all(|piece| piece.errors.is_empty()));
    // Not one more message was asked for, and no other mailbox was opened.
    assert_eq!((downloads(&imap), sent(&imap, "EXAMINE")), (100, 1));
    // The Inbox is not said to be read beyond what didn't come.
    assert_eq!(last.folders.iter().map(|f| (f.remote.as_str(), f.new.len(), f.uid_seen)).collect::<Vec<_>>(), [("INBOX", 0, 0)]);
    {
        let world = imap.world();
        assert!(world.log.last().unwrap().ends_with("LOGOUT"), "{:?}", world.log.last());
        assert_eq!(world.connections, 1);
    }
    // What came stays, and the next run brings the rest.
    let mut here = Kept::new();
    all.iter().for_each(|piece| here.take(piece));
    assert_eq!(here.count(), 100);
    here.take(&sync(&request(ada.clone(), here.folders.clone())));
    assert_eq!((here.mail(), downloads(&imap)), (served(&imap, &LARGE), 355));

    // A run that is called off before it begins sends nothing and connects to nobody.
    let cancel = AtomicBool::new(true);
    let mut all: Vec<SyncResult> = Vec::new();
    let mut with_mail = request(ada, here.folders.clone());
    with_mail.outbox = vec![hello()];
    with_mail.ops = vec![flags("INBOX", 1, Some(true), None, None)];
    sync_as(&with_mail, batches_of(50), Host::default(), &cancel, &mut |piece| all.push(piece));
    assert_eq!(all, [SyncResult { account: 7, email: "ada@example.org".into(), stopped: true, ..Default::default() }]);
    assert_eq!((imap.world().connections, smtp.world().connections), (2, 0));

    // Called off while mail goes out: the next message waits, and nothing else is begun. But a
    // message that went is seen through: it is said, and its copy is kept on the server.
    let cancel = AtomicBool::new(false);
    let mut all: Vec<SyncResult> = Vec::new();
    with_mail.outbox.push(Message { id: 10, subject: "Second".into(), ..hello() });
    sync_as(&with_mail, batches_of(50), Host::default(), &cancel, &mut |piece| {
        cancel.store(piece.progress == "Sent 1 of 2", Ordering::Relaxed);
        all.push(piece);
    });
    let mut together = SyncResult::default();
    all.iter().cloned().for_each(|piece| together.absorb(piece));
    assert_eq!((together.sent.as_slice(), together.filed.as_slice(), together.errors.len(), together.stopped), (&[9][..], &[9][..], 0, true));
    {
        let smtp = smtp.world();
        assert_eq!((smtp.delivered.len(), smtp.log.last().map(String::as_str)), (1, Some("QUIT")));
    }
    assert!(together.flagged.is_empty() && together.folders.is_empty());
    assert_eq!(changes(&imap).iter().map(|c| c.split('{').next().unwrap().to_string()).collect::<Vec<_>>(), ["APPEND \"Sent Items\" (\\Seen) "]);
    let imap = imap.world();
    assert!(imap.log.last().unwrap().ends_with("LOGOUT"));
    assert_eq!((imap.connections, imap.sent("EXAMINE")), (3, 6));
}

#[test]
fn a_run_called_off_in_the_middle_of_a_download_does_not_wait_for_it() {
    let imap = Fake::imap(imap_world());
    // A server that takes six seconds or so over a message.
    imap.world().drip_on = Some(("BODY.PEEK[]".into(), Duration::from_millis(20)));
    let ada = account(Some(&imap), None);
    let asked = request(ada.clone(), local_folders());
    let cancel = AtomicBool::new(false);
    let mut all: Vec<SyncResult> = Vec::new();
    let started = Instant::now();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(400));
            cancel.store(true, Ordering::Relaxed);
        });
        sync_with(&asked, &cancel, &mut |piece| all.push(piece));
    });
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    let last = all.last().unwrap();
    assert!(last.stopped && !last.partial, "{last:?}");
    // No problem to tell, and nothing of the message that was cut off.
    assert!(all.iter().all(|piece| piece.errors.is_empty() && piece.new_count() == 0), "{all:?}");
    assert_eq!(last.folders.iter().map(|f| (f.remote.as_str(), f.uid_seen)).collect::<Vec<_>>(), [("INBOX", 0)]);
    // The next run has all of it.
    imap.world().drip_on = None;
    let mut here = Kept::new();
    all.iter().for_each(|piece| here.take(piece));
    let again = sync(&request(ada, here.folders.clone()));
    assert_eq!((again.errors.len(), again.new_count()), (0, 5));
}

#[test]
fn what_was_sent_and_changed_here_is_said_before_anything_comes_down() {
    let imap = two_way(true, true);
    let smtp = Fake::smtp(smtp_world());
    let folders = without(synced_folders(&imap), "INBOX", &[1]);
    let ops =
        vec![flags("INBOX", 5, Some(true), None, None), moving(901, "INBOX", 1, "Projects", PROJECTS), flags("Projects", 1, None, Some(true), None)];
    let outbox = vec![hello(), Message { id: 10, subject: "Second".into(), ..hello() }];
    let all = pieces(&SyncRequest { ops, outbox, ..request(account(Some(&imap), Some(&smtp)), folders) }, batches_of(50));
    assert!(all.iter().all(|piece| piece.errors.is_empty()));
    assert_eq!(
        all.iter().map(|piece| piece.progress.as_str()).collect::<Vec<_>>(),
        [
            "Sending 2 messages",
            "Sent 1 of 2",
            "Sent 2 of 2",
            "Connecting to the mail server",
            "Found 5 folders",
            "Keeping a copy of what was sent",
            "Updated Inbox on the server",
            "Updated Projects on the server",
            "Checking Inbox (1 of 5)",
            "Checking Sent Items (2 of 5)",
            "Checking Entwürfe (3 of 5)",
            "Checking Trash (4 of 5)",
            "Checking Projects (5 of 5)",
            "",
        ]
    );
    // Each message as soon as it went: one that the mailbox took for unsent would go out again.
    // With it, that a copy is kept on the server: the mailbox knows the copy when it shows,
    // also when it hears no more of this run.
    assert_eq!((all[1].sent.as_slice(), all[2].sent.as_slice()), (&[9][..], &[10][..]));
    assert_eq!((all[1].filed.as_slice(), all[2].filed.as_slice()), (&[9][..], &[10][..]));
    assert!(all.iter().all(|piece| piece.unfiled.is_empty()) && sent(&imap, "APPEND") == 2);
    // Each mailbox's changes as soon as they are made.
    assert_eq!((all[6].flagged.len(), all[6].moved.as_slice()), (1, &[moved(901, "INBOX", 1, PROJECTS, 2, 31)][..]));
    assert_eq!(all[7].flagged.iter().map(|f| (f.folder.as_str(), f.uid)).collect::<Vec<_>>(), [("Projects", 1)]);
    // Only then is anything read, and it shows what was done: nothing is said of a mailbox
    // before the piece that follows its changes.
    assert_eq!(all.iter().position(|piece| !piece.folders.is_empty()), Some(9));
    // The folders themselves are said as soon as they are known: all of them, in their tree.
    assert_eq!((all[4].listed.len(), all[4].folders.len()), (5, 0));
    assert!(all.iter().take(9).all(|piece| piece.new_count() == 0));
    assert!(all.iter().skip(8).all(|piece| piece.sent.is_empty() && piece.filed.is_empty() && piece.pushed_count() == 0));
    let mut together = SyncResult::default();
    all.iter().cloned().for_each(|piece| together.absorb(piece));
    assert_eq!(delta(&together, "INBOX").flags.iter().map(|f| (f.uid, f.read)).collect::<Vec<_>>(), [(2, false), (5, true)]);
    assert_eq!(delta(&together, "Projects").flags.iter().map(|f| (f.uid, f.flagged)).collect::<Vec<_>>(), [(1, true), (2, false)]);
    assert_eq!(delta(&together, "Sent Items").new.len(), 2);
}

/// What Exchange lists for a mailbox, in English, when it says nothing about what a folder is for.
fn exchange_world() -> ImapWorld {
    let names = [
        "Archive",
        "Calendar",
        "Calendar/Birthdays",
        "Clutter",
        "Contacts",
        "Contacts/Recipient Cache",
        "Conversation History",
        "Deleted Items",
        "Drafts",
        "INBOX",
        "Journal",
        "Junk Email",
        "Notes",
        "Outbox",
        "Projets &AOk-t&AOk-",
        "Sent Items",
        "Sync Issues",
        "Sync Issues/Conflicts",
        "Sync Issues/Local Failures",
        "Sync Issues/Server Failures",
        "Tasks",
    ];
    let mut mailboxes: Vec<FakeMailbox> =
        names.iter().zip(1..).map(|(raw, validity)| mailbox(raw, "\\HasNoChildren", validity, vec![message(1, "", &format!("In {raw}"))])).collect();
    mailboxes.push(mailbox("Public Folders", "\\Noselect \\HasChildren", 0, Vec::new()));
    mailboxes.push(mailbox("Gone", "\\NonExistent", 0, Vec::new()));
    mailboxes.iter_mut().find(|m| m.raw == "Clutter").unwrap().refuse_open = true;
    ImapWorld { user: "ada".into(), password: PASSWORD.into(), can_move: true, uidplus: true, mailboxes, ..Default::default() }
}

#[test]
fn an_exchange_mailbox_shows_its_mail_folders_and_no_others() {
    let imap = Fake::imap(exchange_world());
    let ada = account(Some(&imap), None);
    let result = sync(&request(ada.clone(), local_folders()));
    // Its standard folders are the ones here, by their names: none shows twice. The Inbox
    // first, then sent mail and drafts, then where mail is kept and where it is thrown away.
    assert_eq!(
        result.folders.iter().map(|f| (f.remote.as_str(), f.id, f.kind)).collect::<Vec<_>>(),
        [
            ("INBOX", Some(100), FolderKind::Inbox),
            ("Sent Items", Some(102), FolderKind::Sent),
            ("Drafts", Some(101), FolderKind::Drafts),
            ("Archive", Some(104), FolderKind::Archive),
            ("Deleted Items", Some(103), FolderKind::Deleted),
            ("Junk Email", Some(105), FolderKind::Junk),
            ("Conversation History", None, FolderKind::Custom),
            ("Projets été", None, FolderKind::Custom),
        ]
    );
    assert_eq!(result.new_count(), 8);
    // A folder that can't be opened is passed over, and said once.
    assert_eq!(result.errors, ["Couldn't open \"Clutter\": the server said: [NONEXISTENT] no such mailbox"]);
    // Appointments, people, tasks, notes, the journal, mail on its way out and the reports on
    // synchronising are no folders of mail: they are not even opened. Nor is what can't be.
    let opened: Vec<String> =
        imap.world().log.iter().filter_map(|line| line.split_once("EXAMINE ").map(|(_, name)| name.trim_matches('"').to_string())).collect();
    assert_eq!(
        opened,
        ["INBOX", "Sent Items", "Drafts", "Archive", "Deleted Items", "Junk Email", "Clutter", "Conversation History", "Projets &AOk-t&AOk-"]
    );

    // The next run finds every folder where the first one put it, and nothing new.
    let folders = applied(&local_folders(), &result);
    let again = sync(&request(ada.clone(), folders.clone()));
    assert_eq!((again.errors.len(), again.new_count(), again.folders.len()), (1, 0, 8));
    let mut found: Vec<Id> = again.folders.iter().filter_map(|f| f.id).collect();
    found.sort_unstable();
    assert_eq!(found, [100, 101, 102, 103, 104, 105, 1_007, 1_008]);
    // What is changed here reaches its folders by the same names.
    let trash = folders.iter().find(|f| f.remote == "Deleted Items").unwrap().id;
    let french = folders.iter().find(|f| f.remote == "Projets été").unwrap().id;
    let ops = vec![moving(901, "INBOX", 1, "Deleted Items", trash), moving(902, "Sent Items", 1, "Projets été", french)];
    let pushed = sync(&pushing(&imap, &without(without(folders.clone(), "INBOX", &[1]), "Sent Items", &[1]), ops));
    assert_eq!(pushed.moved, [moved(901, "INBOX", 1, trash, 2, 8), moved(902, "Sent Items", 1, french, 2, 15)]);
    assert_eq!(changes(&imap), ["UID MOVE 1 \"Deleted Items\"", "UID MOVE 1 \"Projets &AOk-t&AOk-\""]);

    // A folder here that follows one of the others already (from before they were told apart)
    // goes on being looked after.
    let mut with_notes = folders.clone();
    with_notes.push(FolderState { id: 700, remote: "Notes".into(), uid_validity: 13, ..Default::default() });
    let result = sync(&request(ada, with_notes));
    assert_eq!(delta(&result, "Notes").new.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["In Notes"]);
    assert!(result.folders.iter().all(|f| f.remote != "Tasks" && f.remote != "Calendar"));
}

#[test]
fn folders_with_such_names_on_another_kind_of_server_are_folders_of_mail() {
    // Somebody's own "Notes", and an "Outbox" another program made.
    let mut world = imap_world();
    for (raw, validity) in [("Notes", 81), ("Outbox", 82), ("Deleted Items", 83), ("Junk Email", 84)] {
        world.mailboxes.push(mailbox(raw, "\\HasNoChildren", validity, vec![message(1, "", &format!("In {raw}"))]));
    }
    let imap = Fake::imap(world);
    let asked = request(account(Some(&imap), None), local_folders());
    let result = sync(&asked);
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(remotes(&result), ["INBOX", "Sent Items", "Entwürfe", "Deleted Items", "Junk Email", "Projects", "Notes", "Outbox"]);
    assert_eq!(result.new_count(), 9);
    // From a server of Microsoft's, the same list is that of a mailbox at Outlook.com which was
    // never opened in Outlook: its "Notes" and "Outbox" are Exchange's own, and no mail to keep.
    let result = whole(&asked, Limits::in_pieces(), Host { files_sent: true, microsoft: true });
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(remotes(&result), ["INBOX", "Sent Items", "Entwürfe", "Deleted Items", "Junk Email", "Projects"]);
    assert_eq!((sent(&imap, "EXAMINE \"Notes\""), sent(&imap, "EXAMINE \"Outbox\"")), (1, 1));
}

#[test]
fn a_server_that_stops_answering_ends_the_run_with_an_error() {
    let imap = Fake::imap(imap_world());
    imap.world().stall_on = Some("UID FETCH 2 (UID FLAGS INTERNALDATE BODY.PEEK[])".into());
    let ada = account(Some(&imap), None);
    let patient_for = |io: u64| impatient(Duration::from_millis(io), Duration::from_secs(60));
    let started = Instant::now();
    let result = sync_within(&request(ada.clone(), local_folders()), patient_for(300));
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(result.errors, ["Couldn't download a message from \"INBOX\": connection problem: the server took too long to answer"]);
    // What had arrived is kept; the rest, and the other folders, wait for the next run.
    assert_eq!(remotes(&result), ["INBOX"]);
    assert_eq!((uids(&result.folders[0].new), result.folders[0].uid_seen), (vec![5], 0));
    // Silence at any other point is the same: before the sign-in is answered, in the list of
    // folders, when a mailbox is opened or looked into.
    for (silent_at, said) in [
        ("LOGIN", "Couldn't check for new mail"),
        ("LIST", "Couldn't read the list of folders"),
        ("EXAMINE \"INBOX\"", "Couldn't open \"INBOX\""),
        ("UID FETCH 1:* (UID FLAGS)", "Couldn't read \"INBOX\""),
        ("RFC822.SIZE", "Couldn't read \"INBOX\""),
    ] {
        imap.world().stall_on = Some(silent_at.into());
        let started = Instant::now();
        let result = sync_within(&request(ada.clone(), local_folders()), patient_for(200));
        assert!(started.elapsed() < Duration::from_secs(5), "{silent_at}: {:?}", started.elapsed());
        assert_eq!(result.errors, [format!("{said}: connection problem: the server took too long to answer")], "{silent_at}");
        assert_eq!(result.new_count(), 0, "{silent_at}");
    }
    // While changes go up: what was done is said, the rest waits.
    let imap = two_way(true, true);
    let folders = without(synced_folders(&imap), "INBOX", &[1]);
    imap.world().stall_on = Some("UID MOVE".into());
    let ops = vec![flags("INBOX", 5, Some(true), None, None), moving(901, "INBOX", 1, "Projects", PROJECTS)];
    let result = sync_within(&pushing(&imap, &folders, ops), patient_for(200));
    assert_eq!(result.errors, ["Couldn't move messages from \"INBOX\" to \"Projects\": connection problem: the server took too long to answer"]);
    assert_eq!((result.flagged.len(), result.moved.len(), result.folders.len()), (1, 0, 0));
    // When the server answers again, the next run has everything.
    imap.world().stall_on = None;
    let result = sync(&pushing(&imap, &folders, vec![moving(901, "INBOX", 1, "Projects", PROJECTS)]));
    assert_eq!((result.errors.len(), result.moved.len()), (0, 1));
}

#[test]
fn a_server_that_lets_a_byte_through_now_and_then_is_not_waited_for_for_ever() {
    let imap = Fake::imap(imap_world());
    // Every read gets its byte in time; the answer as a whole would take five seconds.
    imap.world().drip_on = Some(("UID FETCH 1:* (UID FLAGS)".into(), Duration::from_millis(30)));
    let started = Instant::now();
    let result = sync_within(&request(account(Some(&imap), None), local_folders()), impatient(Duration::from_secs(3), Duration::from_millis(400)));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(result.errors, ["Couldn't read \"INBOX\": connection problem: the server took too long to answer"]);
    assert!(result.folders.is_empty());
}

#[test]
fn a_connection_that_drops_in_the_middle_of_a_message_keeps_what_arrived() {
    let imap = Fake::imap(imap_world());
    imap.world().cut_at_uid = Some(2);
    let ada = account(Some(&imap), None);
    let all = pieces(&request(ada.clone(), local_folders()), batches_of(1));
    let mut result = SyncResult::default();
    all.iter().cloned().for_each(|piece| result.absorb(piece));
    assert_eq!(result.errors, ["Couldn't download a message from \"INBOX\": connection problem: the server closed the connection"]);
    // The message that came whole was handed over before the connection went; half a message is none.
    assert_eq!(
        all.iter().map(|piece| piece.progress.as_str()).collect::<Vec<_>>(),
        ["Connecting to the mail server", "Found 4 folders", "Checking Inbox (1 of 4)", "Inbox: 1 of 3", ""]
    );
    assert_eq!((uids(&all[3].folders[0].new), all[3].folders[0].uid_seen), (vec![5], 0));
    assert_eq!(remotes(&result), ["INBOX"]);
    assert_eq!((uids(&result.folders[0].new), result.folders[0].uid_seen), (vec![5], 0));

    imap.world().cut_at_uid = None;
    let again = sync(&request(ada, applied(&local_folders(), &result)));
    assert_eq!(again.errors, Vec::<String>::new());
    assert_eq!((uids(&again.folders[0].new), again.folders[0].uid_seen, again.new_count()), (vec![1, 2], 6, 4));
}

#[test]
fn a_message_the_server_keeps_back_is_asked_for_again() {
    let imap = Fake::imap(imap_world());
    // Asked for message 2, the server says it is done and hands over nothing.
    let silent = ("UID FETCH 2 (UID FLAGS INTERNALDATE BODY.PEEK[])".to_string(), "{tag} OK done\r\n".to_string());
    imap.world().answers = vec![silent.clone()];
    let ada = account(Some(&imap), None);
    let result = sync(&request(ada.clone(), local_folders()));
    // It is still there, so it is not passed over: the mailbox is read up to below it.
    assert_eq!(result.errors, ["The server didn't hand over 1 message of \"INBOX\""]);
    assert_eq!((uids(&result.folders[0].new), result.folders[0].uid_seen, result.folders.len()), (vec![1, 5], 1, 4));
    assert_eq!(sent(&imap, "UID FETCH 2 (UID FLAGS)"), 1);
    imap.world().answers.clear();
    let again = sync(&request(ada, applied(&local_folders(), &result)));
    assert_eq!((again.errors.len(), uids(&again.folders[0].new), again.folders[0].uid_seen), (0, vec![2], 6));

    // One that went between the listing and the asking is gone, and nothing is said.
    let imap = Fake::imap(imap_world());
    imap.world().answers = vec![silent, ("UID FETCH 2 (UID FLAGS)".into(), "{tag} OK done\r\n".into())];
    let result = sync(&request(account(Some(&imap), None), local_folders()));
    assert_eq!((result.errors.len(), uids(&result.folders[0].new), result.folders[0].uid_seen), (0, vec![1, 5], 6));
}

#[test]
fn a_server_that_answers_nonsense_breaks_nothing() {
    let honest: [(&str, &str); 4] = [("INBOX", "INBOX"), ("Sent Items", "Sent Items"), ("Entw&APw-rfe", "Entwürfe"), ("Projects", "Projects")];
    let cases: [(&str, &str, &[&str]); 12] = [
        // A mailbox's listing with no message in it that could be one.
        (
            "UID FETCH 1:* (UID FLAGS)",
            "* 1 FETCH (UID 0 FLAGS ())\r\n* x FETCH (UID)\r\n* 2 FETCH (FLAGS (\\Seen))\r\n* 3 FETCH (UID 99999999999 FLAGS ())\r\n* 4 FETCH\r\n{tag} OK done\r\n",
            &["Couldn't read \"INBOX\": the server listed none of its 4 messages"],
        ),
        // A message announced as larger than any message is.
        (
            "BODY.PEEK[]",
            "* 1 FETCH (UID 5 BODY[] {99999999999}\r\nxxxx",
            &["Couldn't download a message from \"INBOX\": unexpected server response: the server sent 99999999999 bytes"],
        ),
        // Another message than the one asked for, every time.
        ("BODY.PEEK[]", "* 1 FETCH (UID 99 BODY[] {2}\r\nhi)\r\n{tag} OK done\r\n", &["The server didn't hand over 3 messages of \"INBOX\""]),
        // The message, and then more of the same without end.
        (
            "BODY.PEEK[]",
            "* 1 FETCH (UID 5 BODY[] {2}\r\nhi (((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((\r\n{tag} OK done\r\n",
            &["Couldn't download a message from \"INBOX\": unexpected server response: the server's response is nested too deeply"],
        ),
        // A list of folders that are none, with names no folder has.
        (
            "LIST",
            "* LIST (\\HasNoChildren) \"/\" \"\"\r\n* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\HasNoChildren) \"/\" \"inbox\"\r\n* LIST (\\Noselect) NIL \"No\\\"where\"\r\n* LIST junk\r\n* LIST (\\HasNoChildren) \"/\" {5}\r\nGhost\r\n* LIST (\\HasNoChildren) \"/\" \"a\u{7}\tb\"\r\n{tag} OK done\r\n",
            &["Couldn't open \"INBOX\": the server said: [NONEXISTENT] no such mailbox", "Couldn't open \"Ghost\"", "Couldn't open \"ab\""],
        ),
        (
            "LIST",
            "* LIST (\\HasNoChildren) \"/\" {99999999999}\r\n",
            &["Couldn't read the list of folders: unexpected server response: the server sent 99999999999 bytes"],
        ),
        // A mailbox that says impossible things about itself.
        (
            "EXAMINE",
            "* 4294967295 EXISTS\r\n* OK [UIDVALIDITY 0] none\r\n* OK [UIDNEXT 99999999999] next\r\n{tag} OK [READ-ONLY] done\r\n",
            &["Couldn't read \"INBOX\": the server listed none of its 4294967295 messages"],
        ),
        // An answer to a command that wasn't sent, a goodbye in the middle, text that is no answer.
        (
            "UID FETCH 1:* (UID FLAGS)",
            "{tag}9 OK done\r\n",
            &["Couldn't read \"INBOX\": unexpected server response: the server answered a command that wasn't sent"],
        ),
        ("RFC822.SIZE", "* BYE going down\r\n", &["Couldn't read \"INBOX\": connection problem: the server ended the session: going down"]),
        (
            "EXAMINE",
            "220 mail.example.org ESMTP\r\n",
            &["Couldn't open \"INBOX\": unexpected server response: \"220 mail.example.org ESMTP\" is not an IMAP response"],
        ),
        // Sizes that are none: the messages come all the same, a header for the "large" ones.
        (
            "RFC822.SIZE",
            "* 1 FETCH (UID 1 RFC822.SIZE 99999999999)\r\n* 2 FETCH (UID 2 RFC822.SIZE -5)\r\n* 3 FETCH (UID 5 RFC822.SIZE 18446744073709551615)\r\n* 4 FETCH (UID 77 RFC822.SIZE 1)\r\n{tag} OK done\r\n",
            &[],
        ),
        // A password that is thrown back at the client.
        (
            "EXAMINE \"Sent Items\"",
            "{tag} NO no mailbox for the one with hunter2\r\n",
            &["Couldn't open \"Sent Items\": the server said: no mailbox for the one with ***"],
        ),
    ];
    for (at, answer, said) in cases {
        let imap = Fake::imap(imap_world());
        imap.world().answers = vec![(at.to_string(), answer.to_string())];
        let ada = account(Some(&imap), None);
        let started = Instant::now();
        let quick = Limits { patience: Patience { io: Duration::from_secs(5), ..Patience::default() }, ..batches_of(2) };
        let all = pieces(&request(ada.clone(), local_folders()), quick);
        assert!(started.elapsed() < Duration::from_secs(4), "{at}: {:?}", started.elapsed());
        // What there is to tell is told in a line a person can read, without the password.
        let errors: Vec<&String> = all.iter().flat_map(|piece| &piece.errors).collect();
        assert!(errors.iter().all(|line| line.len() < 600 && !line.chars().any(char::is_control) && !line.contains(PASSWORD)), "{at}: {errors:?}");
        for (line, start) in errors.iter().zip(said) {
            assert!(line.starts_with(start), "{at}: {errors:?}");
        }
        assert!(errors.len() >= said.len() && (said.is_empty() == errors.is_empty()), "{at}: {errors:?}");
        // Whatever of it is taken in, an honest answer afterwards puts everything right:
        // nothing is missing, nothing is there twice.
        for taken in [all.len(), all.len() / 2] {
            let mut here = Kept::new();
            all[..taken].iter().for_each(|piece| here.take(piece));
            imap.world().answers.clear();
            let after = sync(&request(ada.clone(), here.folders.clone()));
            assert_eq!(after.errors, Vec::<String>::new(), "{at}");
            here.take(&after);
            assert_eq!(here.mail(), served(&imap, &honest), "{at}");
            imap.world().answers = vec![(at.to_string(), answer.to_string())];
        }
    }
}

/// A run that is only to tell the server what changed here does that and reads nothing: no
/// folder is opened for its mail, so a read mark doesn't cost a pass over every folder.
#[test]
fn a_run_that_only_tells_brings_nothing_down() {
    let imap = Fake::imap(imap_world());
    let ada = account(Some(&imap), None);
    // First an ordinary run, so that the mailbox knows the Inbox.
    let first = sync(&request(ada.clone(), local_folders()));
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let inbox = first.folders.iter().find(|f| f.remote == "INBOX").unwrap();
    let uid = inbox.new.first().map(|m| m.uid).unwrap();
    let mut folders = local_folders();
    for state in folders.iter_mut().filter(|f| f.kind == FolderKind::Inbox) {
        state.remote = "INBOX".into();
        state.uid_validity = inbox.uid_validity;
        state.uid_seen = inbox.uid_seen;
        state.known_uids = inbox.new.iter().map(|m| m.uid).collect();
    }
    let fetched = |imap: &FakeImap| imap.world().log.iter().filter(|line| line.contains("FETCH") && line.contains("BODY")).count();
    let before = fetched(&imap);
    let mut only = request(ada, folders);
    only.push_only = true;
    only.ops = vec![RemoteOp::Flags { folder: "INBOX".into(), uid, read: Some(true), flagged: None, answered: None }];
    let result = sync(&only);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.flagged.len(), 1, "the change went up");
    assert!(result.folders.is_empty(), "and nothing came down");
    assert_eq!(fetched(&imap), before, "no message was fetched");
}

/// #276: a run says how long its phases took and how many commands it sent, by phase, with
/// nothing in it that names a mailbox; taken in whole, the pieces add up.
#[test]
fn a_mail_server_run_carries_its_time_and_commands_by_phase() {
    use mailcraft_model::sync::SyncPhase;
    let imap = Fake::imap(imap_world());
    let mut pieces = Vec::new();
    sync_with(&request(account(Some(&imap), None), local_folders()), &AtomicBool::new(false), &mut |piece| pieces.push(piece));
    let in_phase = |phase| -> u32 { pieces.iter().flat_map(|p| &p.timings.phases).filter(|p| p.phase == phase).map(|p| p.requests).sum() };
    // Signing in, listing and the folders' state come before the first folder; each folder is a
    // command or more of new mail.
    assert!(in_phase(SyncPhase::Folders) >= 2, "{pieces:?}");
    assert!(in_phase(SyncPhase::NewMail) >= 4, "four folders are looked at");
    assert_eq!(in_phase(SyncPhase::Sending), 0);
    assert_eq!(in_phase(SyncPhase::CalendarAndContacts), 0);
    // No more than the server was sent (it also logs the goodbye, which comes after the last piece).
    let total: u32 = pieces.iter().map(|p| p.timings.requests()).sum();
    let logged = u32::try_from(imap.world().log.len()).unwrap();
    assert!(total <= logged, "{total} counted, {logged} logged");
    // The same run taken in at once adds up.
    let whole = sync(&request(account(Some(&imap), None), local_folders()));
    assert!(whole.timings.requests() > 0 && whole.timings.skipped == 0);
}

/// #276: mail sent is a request of the sending phase.
#[test]
fn mail_sent_is_counted_in_the_sending_phase() {
    use mailcraft_model::sync::SyncPhase;
    let smtp = Fake::smtp(smtp_world());
    let mut asked = request(account(None, Some(&smtp)), Vec::new());
    asked.outbox = vec![hello()];
    let result = sync(&asked);
    assert_eq!(result.sent.len(), 1, "{:?}", result.errors);
    let sending = result.timings.phases.iter().find(|p| p.phase == SyncPhase::Sending).map_or(0, |p| p.requests);
    assert_eq!(sending, 1);
}

#[test]
fn a_server_that_floods_one_answer_is_given_up_on() {
    let pad = "x".repeat(60 * 1024);
    // About 38 MB of FETCH lines at invented UIDs, and about as much of LIST lines: more than one
    // command may bring, and nothing of it is kept.
    let fetch: String = (0..640).map(|n| format!("* {n} FETCH (UID {} FLAGS ({pad}))\r\n", 1_000 + n)).collect::<String>() + "{tag} OK done\r\n";
    let list: String = (0..640).map(|n| format!("* LIST () \"/\" \"{n}{pad}\"\r\n")).collect::<String>() + "{tag} OK done\r\n";
    for (at, answer) in [("UID FETCH 1:* (UID FLAGS)", fetch), ("LIST", list)] {
        let imap = Fake::imap(imap_world());
        imap.world().answers = vec![(at.to_string(), answer)];
        let ada = account(Some(&imap), None);
        let started = Instant::now();
        let result = sync(&request(ada, local_folders()));
        assert!(started.elapsed() < Duration::from_secs(60), "{at}: {:?}", started.elapsed());
        assert!(!result.errors.is_empty(), "{at}");
        assert!(result.errors.iter().all(|line| line.len() < 600 && !line.chars().any(char::is_control)), "{at}: {:?}", result.errors);
        // Nothing of the flood is taken in.
        assert!(result.folders.iter().all(|folder| folder.new.is_empty()), "{at}");
    }
}
