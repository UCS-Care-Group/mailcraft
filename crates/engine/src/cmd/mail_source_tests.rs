//! Mail read through Microsoft Graph: `account.mailSource`, what a synchronisation of such an
//! account is asked to do, what the engine keeps of Graph's answer, and what a switch does to the
//! mail that is here.

use mailcraft_model::sync::{FolderDelta, RemoteFlags, RemoteId, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, MailSource, Mailbox, Message};
use serde_json::{Value, json};

use super::account::{SEALED_BY_SWITCH, apply_sync, sync_request};
use crate::Session;

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn refused(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Ok(v) => panic!("{id} {params} went through: {v}"),
        Err(e) => e.to_string(),
    }
}

/// A mailbox with the owner's local account and a Microsoft 365 account.
fn setup() -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("Avery Quinn", "avery@lumen.example"), 1_791_993_600, 0);
    let added = run(
        &mut s,
        "account.add",
        json!({
            "email": "avery@harbor.example", "displayName": "Avery Quinn",
            "incoming": {"host": "outlook.office365.com", "port": 993},
            "outgoing": {"host": "smtp.office365.com", "port": 587, "security": "starttls"},
            "auth": "oauth2", "provider": "microsoft", "directory": "organisation",
        }),
    );
    (s, added["id"].as_u64().unwrap())
}

fn inbox(s: &Session, account: Id) -> Id {
    s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap()
}

fn message(uid: u32, remote_id: &str, message_id: &str, subject: &str) -> Message {
    Message {
        uid,
        remote_id: remote_id.into(),
        message_id: message_id.into(),
        subject: subject.into(),
        from: Address::new("Bob", "bob@example.org"),
        ..Default::default()
    }
}

/// An answer for the Inbox of `account`.
fn inbox_news(s: &Session, account: Id, validity: u32, seen: u32, new: Vec<Message>, delta: &str) -> SyncResult {
    SyncResult {
        account,
        email: "avery@harbor.example".into(),
        folders: vec![FolderDelta {
            id: Some(inbox(s, account)),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: validity,
            uid_seen: seen,
            new,
            delta: delta.into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Take in [`inbox_news`].
fn take(s: &mut Session, account: Id, validity: u32, seen: u32, new: Vec<Message>, delta: &str) {
    let news = inbox_news(s, account, validity, seen, new, delta);
    apply_sync(s, news).unwrap();
}

fn by_subject<'a>(s: &'a Session, subject: &str) -> Vec<&'a Message> {
    s.mailbox.messages.values().map(|m| m.as_ref()).filter(|m| m.subject == subject).collect()
}

const DELTA: &str = "https://graph.microsoft.com/v1.0/me/mailFolders/inbox-id/messages/delta?$deltatoken=one";

#[test]
fn an_account_is_imap_until_its_owner_chooses_graph_and_the_file_says_so_only_then() {
    let (mut s, account) = setup();
    let said = run(&mut s, "account.mailSource", json!({"account": account}));
    assert_eq!((said["source"].as_str(), said["changed"].as_bool()), (Some("imap"), Some(false)));
    assert!(!serde_json::to_string(&s.mailbox).unwrap().contains("mailSource"));
    let said = run(&mut s, "account.mailSource", json!({"account": account, "source": "graph"}));
    assert_eq!((said["source"].as_str(), said["changed"].as_bool()), (Some("graph"), Some(true)));
    assert_eq!(s.mailbox.account(account).unwrap().mail_source, MailSource::Graph);
    assert!(serde_json::to_string(&s.mailbox).unwrap().contains(r#""mailSource":"graph""#));
    let listed = run(&mut s, "account.list", json!({}));
    assert!(listed.to_string().contains(r#""mailSource":"graph""#), "{listed}");
    // A file that says something else in its place reads as IMAP.
    let mut a = serde_json::to_value(s.mailbox.account(account).unwrap()).unwrap();
    a["mailSource"] = json!("pigeon");
    let back: mailcraft_model::Account = serde_json::from_value(a).unwrap();
    assert_eq!(back.mail_source, MailSource::Imap);
}

#[test]
fn only_an_account_of_its_own_that_signs_in_with_microsoft_can_have_it() {
    let (mut s, account) = setup();
    let local = s.mailbox.accounts[0].id;
    assert!(refused(&mut s, "account.mailSource", json!({"account": local, "source": "graph"})).contains("Microsoft"));
    let password = run(
        &mut s,
        "account.add",
        json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}}),
    )["id"]
        .as_u64()
        .unwrap();
    assert!(refused(&mut s, "account.mailSource", json!({"account": password, "source": "graph"})).contains("Microsoft"));
    let support = run(&mut s, "account.addShared", json!({"account": account, "address": "support@harbor.example"}))["id"].as_u64().unwrap();
    assert!(refused(&mut s, "account.mailSource", json!({"account": support, "source": "graph"})).contains("shared mailbox"));
    assert!(refused(&mut s, "account.mailSource", json!({"account": account, "source": "jmap"})).contains("imap or graph"));
}

#[test]
fn what_graph_brings_is_kept_with_its_ids_and_asked_about_by_them_next_time() {
    let (mut s, account) = setup();
    run(&mut s, "account.mailSource", json!({"account": account, "source": "graph"}));
    let new = vec![message(1, "AAMkAD-one", "one@example.org", "Quarterly numbers"), message(2, "AAMkAD-two", "two@example.org", "Lunch")];
    take(&mut s, account, 1, 2, new, DELTA);
    let arrived = by_subject(&s, "Quarterly numbers");
    assert_eq!((arrived.len(), arrived[0].uid, arrived[0].remote_id.as_str()), (1, 1, "AAMkAD-one"));
    let id = arrived[0].id;
    let folder = s.mailbox.folder(inbox(&s, account)).unwrap();
    assert_eq!((folder.uid_validity, folder.uid_seen, folder.delta.as_str()), (1, 2, DELTA));

    let request = sync_request(&s, account).unwrap();
    let state = request.folders.iter().find(|f| f.remote == "INBOX").unwrap();
    let mut ids = state.remote_ids.clone();
    ids.sort_by_key(|r| r.uid);
    assert_eq!(ids, vec![RemoteId { uid: 1, id: "AAMkAD-one".into() }, RemoteId { uid: 2, id: "AAMkAD-two".into() }]);
    assert_eq!(state.delta, DELTA);

    // A piece that read the folder without getting through it leaves the link where it was.
    take(&mut s, account, 1, 2, Vec::new(), "");
    assert_eq!(s.mailbox.folder(inbox(&s, account)).unwrap().delta, DELTA);

    // A copy made here is not the server's message.
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap_or(inbox(&s, account));
    let copy = s.mailbox.copy_message(id, archive).unwrap();
    assert!(s.mailbox.message(copy).unwrap().remote_id.is_empty());
}

#[test]
fn graph_says_nothing_of_answered_so_that_stays_as_it_is_here() {
    let (mut s, account) = setup();
    run(&mut s, "account.mailSource", json!({"account": account, "source": "graph"}));
    let mut answered = message(1, "AAMkAD-one", "one@example.org", "Replied to");
    answered.answered = true;
    take(&mut s, account, 1, 1, vec![answered], DELTA);
    let mut news = inbox_news(&s, account, 1, 1, Vec::new(), "");
    news.folders[0].flags = vec![RemoteFlags { uid: 1, read: true, ..Default::default() }];
    apply_sync(&mut s, news).unwrap();
    let m = by_subject(&s, "Replied to")[0];
    assert!(m.read && m.answered);
}

#[test]
fn a_switch_keeps_every_message_and_matches_it_to_the_servers_copy_by_its_message_id() {
    let (mut s, account) = setup();
    let mut kept = message(41, "", "kept@example.org", "Kept across the switch");
    kept.read = true;
    take(&mut s, account, 7, 41, vec![kept], "");
    run(&mut s, "account.mailSource", json!({"account": account, "source": "graph"}));
    let m = by_subject(&s, "Kept across the switch")[0];
    assert_eq!((m.uid, m.synced.map(|f| f.uid)), (0, Some(0)));
    let folder = s.mailbox.folder(inbox(&s, account)).unwrap();
    assert_eq!((folder.remote.as_str(), folder.uid_validity, folder.uid_seen), ("INBOX", 0, 0));
    // Undo can't go back past it.
    assert!(refused(&mut s, "edit.undo", json!({})).contains(SEALED_BY_SWITCH));

    // Graph's copy comes down: it is the message that is here, now with its id there.
    let mut theirs = message(1, "AAMkAD-kept", "kept@example.org", "Kept across the switch");
    theirs.read = true;
    take(&mut s, account, 1, 1, vec![theirs], DELTA);
    let all = by_subject(&s, "Kept across the switch");
    assert_eq!((all.len(), all[0].uid, all[0].remote_id.as_str()), (1, 1, "AAMkAD-kept"));

    // And back again: the same, the other way.
    run(&mut s, "account.mailSource", json!({"account": account, "source": "imap"}));
    let m = by_subject(&s, "Kept across the switch")[0];
    assert_eq!((m.uid, m.remote_id.as_str()), (0, ""));
    assert!(s.mailbox.folder(inbox(&s, account)).unwrap().delta.is_empty());
}

#[test]
fn a_folder_under_the_inbox_is_named_the_same_way_both_ways() {
    let (mut s, account) = setup();
    let parent = inbox(&s, account);
    let projects = run(&mut s, "folder.new", json!({"name": "Projects", "parent": parent, "account": account}))["id"].as_u64().unwrap();
    s.mailbox.folder_mut(projects).unwrap().remote = "Inbox/Projects".into();
    run(&mut s, "account.mailSource", json!({"account": account, "source": "graph"}));
    assert_eq!(s.mailbox.folder(projects).unwrap().remote, "INBOX/Projects");
}

#[test]
fn changes_waiting_for_the_server_hold_a_switch_until_the_owner_lets_them_go() {
    let (mut s, account) = setup();
    take(&mut s, account, 7, 41, vec![message(41, "", "w@example.org", "Waiting")], "");
    let id = by_subject(&s, "Waiting")[0].id;
    run(&mut s, "mail.markRead", json!({"ids": [id]}));
    let said = refused(&mut s, "account.mailSource", json!({"account": account, "source": "graph"}));
    assert!(said.contains("haven't reached the server"), "{said}");
    assert_eq!(s.mailbox.account(account).unwrap().mail_source, MailSource::Imap);
    let said = run(&mut s, "account.mailSource", json!({"account": account, "source": "graph", "discard": true}));
    assert_eq!(said["discarded"].as_u64(), Some(1));
    assert!(by_subject(&s, "Waiting")[0].read);
}

#[test]
fn through_graph_only_drafts_go_up_and_none_is_copied() {
    let (mut s, account) = setup();
    run(&mut s, "account.mailSource", json!({"account": account, "source": "graph"}));
    let drafts = s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    s.mailbox.folder_mut(drafts).unwrap().remote = "Drafts".into();
    s.mailbox.folder_mut(inbox(&s, account)).unwrap().remote = "INBOX".into();
    let draft = Message { account, folder: drafts, subject: "Written here".into(), draft: true, ..Default::default() };
    let imported = Message { account, folder: inbox(&s, account), subject: "Brought in from a file".into(), ..Default::default() };
    s.mailbox.add_message(draft).unwrap();
    s.mailbox.add_message(imported).unwrap();
    let request = sync_request(&s, account).unwrap();
    let going: Vec<&str> = request.uploads.iter().map(|u| u.message.subject.as_str()).collect();
    assert_eq!(going, ["Written here"]);
    assert!(request.uploads.iter().all(|u| u.copy_from.is_empty()));
}

#[test]
fn a_change_still_to_be_pushed_tells_graph_which_message_it_is_about() {
    let (mut s, account) = setup();
    run(&mut s, "account.mailSource", json!({"account": account, "source": "graph"}));
    take(&mut s, account, 1, 2, vec![message(1, "AAMk-moved", "m@example.org", "Moved"), message(2, "AAMk-gone", "g@example.org", "Gone")], DELTA);
    let parent = inbox(&s, account);
    let archive = run(&mut s, "folder.new", json!({"name": "Kept", "parent": parent}))["id"].as_u64().unwrap();
    s.mailbox.folder_mut(archive).unwrap().remote = "INBOX/Kept".into();
    assert_eq!(s.mailbox.folder(archive).unwrap().account, account);
    let moved = by_subject(&s, "Moved")[0].id;
    let gone = by_subject(&s, "Gone")[0].id;
    run(&mut s, "mail.move", json!({"ids": [moved], "folder": archive}));
    s.mailbox.remove_message(gone).unwrap();
    let request = sync_request(&s, account).unwrap();
    let inbox = request.folders.iter().find(|f| f.remote == "INBOX").unwrap();
    let mut ids = inbox.remote_ids.clone();
    ids.sort_by_key(|r| r.uid);
    assert_eq!(ids, vec![RemoteId { uid: 1, id: "AAMk-moved".into() }, RemoteId { uid: 2, id: "AAMk-gone".into() }]);
    assert_eq!(request.ops.len(), 2);
}

/// The shared mailbox `support@` opened through `account`, with one message of the server's in
/// its Inbox (over IMAP, number `uid`).
fn shared_with_mail(s: &mut Session, account: Id, uid: u32) -> Id {
    let support = run(s, "account.addShared", json!({"account": account, "address": "support@harbor.example"}))["id"].as_u64().unwrap();
    let news = SyncResult {
        account: support,
        email: "support@harbor.example".into(),
        folders: vec![FolderDelta {
            id: Some(inbox(s, support)),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 3,
            uid_seen: uid,
            new: vec![message(uid, "", "theirs@example.org", "Theirs")],
            ..Default::default()
        }],
        ..Default::default()
    };
    apply_sync(s, news).unwrap();
    support
}

#[test]
fn a_shared_mailbox_follows_its_owner_and_has_no_choice_of_its_own() {
    let (mut s, account) = setup();
    let support = shared_with_mail(&mut s, account, 9);
    let theirs = by_subject(&s, "Theirs")[0].id;
    run(&mut s, "mail.markRead", json!({"ids": [theirs]}));
    // Refused on the shared mailbox, saying whose it follows.
    let said = refused(&mut s, "account.mailSource", json!({"account": support, "source": "graph"}));
    assert!(said.contains("shared mailbox") && said.contains("avery@harbor.example"), "{said}");
    assert_eq!(s.mailbox.account(support).unwrap().mail_source, MailSource::Imap);
    // What waits in the shared mailbox holds the owner's switch as the owner's own would.
    let said = refused(&mut s, "account.mailSource", json!({"account": account, "source": "graph"}));
    assert!(said.contains("1 change(s)"), "{said}");
    assert_eq!(s.mailbox.account(account).unwrap().mail_source, MailSource::Imap);
    // Let go, the owner's switch is the shared mailbox's switch too: untied the same way.
    let said = run(&mut s, "account.mailSource", json!({"account": account, "source": "graph", "discard": true}));
    assert_eq!(said["discarded"].as_u64(), Some(1));
    assert_eq!(s.mailbox.account(support).unwrap().mail_source, MailSource::Graph);
    assert_eq!(s.mailbox.mail_source_of(support), MailSource::Graph);
    let m = by_subject(&s, "Theirs")[0];
    assert_eq!((m.uid, m.synced.map(|f| f.uid), m.read), (0, Some(0), true));
    let folder = s.mailbox.folder(inbox(&s, support)).unwrap();
    assert_eq!((folder.uid_validity, folder.uid_seen), (0, 0));
    assert!(s.mailbox.pending.iter().all(|p| p.account() != support));
    // A run of the shared mailbox is a Graph run, signed in as the owner.
    let request = sync_request(&s, support).unwrap();
    assert_eq!((request.account.mail_source, request.account.shared_of), (MailSource::Graph, Some(account)));
    assert_eq!(request.account.username, "support@harbor.example");
    // Graph's copy comes down: matched, not doubled.
    let mut graphs = message(1, "AAMk-theirs", "theirs@example.org", "Theirs");
    graphs.read = true;
    let news = SyncResult { email: "support@harbor.example".into(), ..inbox_news(&s, support, 1, 1, vec![graphs], DELTA) };
    apply_sync(&mut s, news).unwrap();
    let all = by_subject(&s, "Theirs");
    assert_eq!((all.len(), all[0].remote_id.as_str()), (1, "AAMk-theirs"));
    // And back, both together.
    run(&mut s, "account.mailSource", json!({"account": account, "source": "imap"}));
    assert_eq!(s.mailbox.account(support).unwrap().mail_source, MailSource::Imap);
    assert_eq!(by_subject(&s, "Theirs")[0].uid, 0);
}

#[test]
fn a_file_that_left_a_shared_mailbox_on_imap_under_a_graph_owner_is_made_to_follow_when_opened() {
    let (mut s, account) = setup();
    let support = shared_with_mail(&mut s, account, 5);
    // As a program from before wrote it: the owner's account switched alone.
    s.mailbox.account_mut(account).unwrap().mail_source = MailSource::Graph;
    // Signing in as it reads the owner's way even so, whatever the file says.
    let request = sync_request(&s, support).unwrap();
    assert_eq!(request.account.mail_source, MailSource::Graph);
    assert!(super::shared::through_graph(&s, support));
    // Opened, the file is put right: the shared mailbox follows, untied as a switch unties.
    let mut opened = s.mailbox.clone();
    let fixed = opened.repair();
    assert!(fixed.iter().any(|line| line.contains("shared mailbox")), "{fixed:?}");
    assert_eq!(opened.account(support).unwrap().mail_source, MailSource::Graph);
    let theirs = opened.messages.values().find(|m| m.subject == "Theirs").unwrap();
    assert_eq!((theirs.uid, theirs.synced.map(|f| f.uid)), (0, Some(0)));
    assert!(opened.repair().is_empty());
}
