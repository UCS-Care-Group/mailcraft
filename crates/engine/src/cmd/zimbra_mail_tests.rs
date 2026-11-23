// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Zimbra account (Z3), as the engine works out what a host should read and takes in what it
//! brings back, with results made by hand. (Against `mailcraft-zimbra`'s stand-in: the desktop
//! app's runner.)

use mailcraft_model::sync::{FolderListing, GraphFetched, GraphFolder, GraphMessage, ServerMessage, ZimbraMessagePage, ZimbraResume};
use mailcraft_model::{Address, FlagStatus};
use serde_json::json;

use super::*;

const ME: &str = "ada@example.org";
const URL: &str = "https://zimbra.example.org";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    s.zimbra = true;
    let added = s.run("account.add", &json!({"email": ME, "kind": "zimbra", "zimbra": {"url": URL}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn folder(id: &str, parent: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), parent: parent.into(), name: name.into(), kind, ..Default::default() }
}

fn listing() -> FolderListing {
    FolderListing {
        folders: vec![
            folder("2", "", "Inbox", FolderKind::Inbox),
            folder("5", "", "Sent", FolderKind::Sent),
            folder("3", "", "Trash", FolderKind::Deleted),
            folder("300", "2", "Projects", FolderKind::Custom),
        ],
        complete: true,
    }
}

fn mail(id: &str, folder: &str, subject: &str, read: bool) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: folder.into(), read, size: 1_200, ..Default::default() },
        message_id: format!("{id}@example.org"),
        subject: subject.into(),
        from: Address::new("Bob", "bob@example.org"),
        to: vec![Address::new("", ME)],
        received: 1_700_000_000,
        sent: 1_700_000_000,
        preview: format!("Preview of {id}"),
        ..Default::default()
    }
}

fn result(account: Id, from: &str) -> ZimbraMailResult {
    ZimbraMailResult { account, email: ME.into(), url: URL.into(), username: ME.into(), from: from.into(), ..Default::default() }
}

fn apply(s: &mut Session, r: ZimbraMailResult) -> Value {
    s.run("sync.applyZimbra", &json!({"result": r})).unwrap()
}

#[test]
fn a_login_change_refuses_pieces_from_the_previous_login_before_listing() {
    let (mut s, account) = session();
    s.run("sync.zimbraRequest", &json!({"account": account})).unwrap();
    s.run("account.edit", &json!({"id": account, "zimbra": {"username": "another"}})).unwrap();
    let before = s.mailbox.clone();
    let mut old = result(account, "");
    old.listing = Some(listing());
    old.partial = true;
    assert!(s.run("sync.applyZimbra", &json!({"result": old})).is_err());
    assert_eq!(s.mailbox, before);
}

#[test]
fn sync_pieces_without_login_identity_are_refused_without_mailbox_changes() {
    let (mut s, account) = session();
    let before = s.mailbox.clone();
    let mut piece = result(account, "");
    piece.username.clear();
    piece.listing = Some(listing());
    piece.partial = true;
    assert!(s.run("sync.applyZimbra", &json!({"result": piece})).is_err());
    assert_eq!(s.mailbox, before);
}

#[test]
fn an_unusable_saved_token_can_be_replaced_by_a_fresh_result() {
    for bad in ["has spaces".to_string(), "\u{0}".into(), "x".repeat(257)] {
        let (mut s, account) = session();
        s.mailbox.account_mut(account).unwrap().zimbra.as_mut().unwrap().token = bad;
        let request: ZimbraMailRequest = serde_json::from_value(s.run("sync.zimbraRequest", &json!({"account": account})).unwrap()).unwrap();
        assert!(request.token.is_empty());
        let mut fresh = result(account, "");
        fresh.listing = Some(listing());
        (fresh.token, fresh.whole, fresh.resume) = ("4711".into(), true, Some(Vec::new()));
        apply(&mut s, fresh);
        assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4711");
    }
}

#[test]
fn missing_folders_keep_their_mail_until_reconciliation_finishes() {
    for finish in [true, false] {
        let (mut s, account) = session();
        first_read(&mut s, account);
        let id = s.mailbox.messages.values().find(|m| m.subject == "Filed").unwrap().id;
        let mut listed = result(account, "4711");
        let mut folders = listing();
        folders.folders.retain(|f| f.id != "300");
        folders.folders.push(folder("301", "", "Archive", FolderKind::Custom));
        listed.listing = Some(folders.clone());
        listed.partial = true;
        apply(&mut s, listed);
        assert!(s.mailbox.folder_by_server(account, "300").is_some());
        assert!(s.mailbox.messages.contains_key(&id));
        let mut end = result(account, "4711");
        if finish {
            let mut moved = result(account, "4711");
            moved.partial = true;
            moved.pages.push(ZimbraMessagePage { messages: vec![mail("259", "301", "Filed", true)], ..Default::default() });
            apply(&mut s, moved);
            end.listing = Some(folders);
            (end.token, end.whole, end.resume) = ("4712".into(), true, Some(Vec::new()));
        } else {
            end.listing = Some(folders);
            end.errors.push("Read failed".into());
        }
        apply(&mut s, end);
        assert_eq!(s.mailbox.folder_by_server(account, "300").is_none(), finish);
        let message = s.mailbox.messages.get(&id).unwrap();
        assert_eq!(message.server.as_ref().unwrap().folder, if finish { "301" } else { "300" });
    }
}

#[test]
fn an_incomplete_success_keeps_its_checkpoint_without_sweeping() {
    for resume in [Vec::new(), vec![ZimbraResume { folder: "2".into(), cursor: "257".into(), sort: 1 }]] {
        let (mut s, account) = session();
        first_read(&mut s, account);
        let mut removal = result(account, "4711");
        removal.partial = true;
        removal.pages.push(ZimbraMessagePage { removed: vec!["257".into()], ..Default::default() });
        apply(&mut s, removal);
        let mut checkpoint = result(account, "4711");
        checkpoint.token = "4712".into();
        checkpoint.resume = Some(resume.clone());
        let answer = apply(&mut s, checkpoint);
        assert_eq!(answer["summary"], "Stopped; carries on from here next time");
        let server = s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap();
        assert_eq!(server.token, "4712");
        assert_eq!(server.resume, resume);
        assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
    }
}

#[test]
fn a_completed_zimbra_round_sorts_later_inbox_arrivals() {
    for delta in [false, true] {
        let (mut s, account) = session();
        s.mailbox.junk_lists_mut(account).blocked.push("bob@example.org".into());
        first_read(&mut s, account);
        assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
        let inbox = s.mailbox.folder_by_server(account, "2").unwrap();
        let server = s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap();
        assert!(server.caught_up && server.round_ended);
        assert!(server.delta.is_empty());
        let mut listed = result(account, "4711");
        let mut folders = listing();
        folders.folders.push(folder("4", "", "Junk", FolderKind::Junk));
        listed.listing = Some(folders);
        listed.partial = true;
        apply(&mut s, listed);
        let mut next = result(account, "4711");
        next.partial = true;
        next.pages.push(ZimbraMessagePage {
            folder: if delta { String::new() } else { "2".into() },
            messages: vec![mail("260", "2", "Blocked arrival", false)],
            last: true,
            ..Default::default()
        });
        apply(&mut s, next);
        assert_eq!(subjects(&s, account, "Junk"), ["Blocked arrival"]);
        assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
    }
}

#[test]
fn a_delta_cannot_complete_a_pending_fresh_folder_read() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let mut fresh = result(account, "4711");
    fresh.partial = true;
    fresh.pages.push(ZimbraMessagePage { folder: "2".into(), messages: vec![mail("257", "2", "Hello", false)], afresh: true, ..Default::default() });
    apply(&mut s, fresh);
    let mut delta = result(account, "4711");
    delta.partial = true;
    delta.pages.push(ZimbraMessagePage { messages: vec![mail("258", "2", "Later", true)], last: true, ..Default::default() });
    apply(&mut s, delta);
    let inbox = s.mailbox.folder_by_server(account, "2").unwrap();
    let server = s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap();
    assert!(!server.caught_up && !server.round_ended);
    let mut tail = result(account, "4711");
    tail.partial = true;
    tail.pages.push(ZimbraMessagePage { folder: "2".into(), last: true, ..Default::default() });
    apply(&mut s, tail);
    let server = s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap();
    assert!(server.caught_up && server.round_ended);
}

#[test]
fn a_page_for_a_missing_folder_blocks_the_run_token_and_sweep() {
    for delta in [false, true] {
        for split in [false, true] {
            let (mut s, account) = session();
            first_read(&mut s, account);
            let mut removal = result(account, "4711");
            removal.partial = true;
            removal.pages.push(ZimbraMessagePage { removed: vec!["257".into()], ..Default::default() });
            apply(&mut s, removal);
            let mut missing = result(account, "4711");
            missing.partial = split;
            missing.whole = !split;
            missing.pages.push(ZimbraMessagePage {
                folder: if delta { String::new() } else { "999".into() },
                messages: vec![mail("900", "999", "Not taken", false)],
                last: true,
                ..Default::default()
            });
            if !split {
                missing.token = "4712".into();
                missing.resume = Some(Vec::new());
            }
            let mut answer = apply(&mut s, missing);
            if split {
                let mut end = result(account, "4711");
                (end.token, end.whole, end.resume) = ("4712".into(), true, Some(Vec::new()));
                answer = apply(&mut s, end);
            }
            assert_eq!(answer["summary"], "Stopped; carries on from here next time");
            assert!(s.sync_status.starts_with("Stopped; carries on from here next time"));
            assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4711");
            assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
        }
    }
}

#[test]
fn reported_errors_survive_display_cleaning_and_block_a_later_finish() {
    for error in ["", "\u{0}", "\n \t", "Server error"] {
        for split in [false, true] {
            let (mut s, account) = session();
            first_read(&mut s, account);
            let mut removal = result(account, "4711");
            removal.partial = true;
            removal.pages.push(ZimbraMessagePage { removed: vec!["257".into()], ..Default::default() });
            apply(&mut s, removal);
            let mut failed = result(account, "4711");
            failed.errors.push(error.into());
            failed.partial = split;
            failed.whole = !split;
            if !split {
                failed.token = "4712".into();
                failed.resume = Some(Vec::new());
            }
            apply(&mut s, failed);
            if split {
                let mut end = result(account, "4711");
                (end.token, end.whole, end.resume) = ("4712".into(), true, Some(Vec::new()));
                apply(&mut s, end);
            }
            assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4711", "{error:?}, split={split}");
            assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
        }
    }
}

#[test]
fn an_incomplete_listing_prevents_a_later_piece_advancing_the_token() {
    let (mut s, account) = session();
    let mut partial = result(account, "");
    partial.listing = Some(FolderListing { folders: listing().folders, complete: false });
    partial.partial = true;
    apply(&mut s, partial);
    let mut end = result(account, "");
    end.whole = true;
    end.token = "5".into();
    end.resume = Some(Vec::new());
    apply(&mut s, end);
    assert!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token.is_empty());
}

#[test]
fn a_piece_for_what_is_no_zimbra_account_cuts_no_other_run() {
    // The set of cut runs is shared with Exchange server accounts: a piece sent to the wrong
    // account, or to none, is refused and leaves that account's own run as it was.
    let (mut s, zimbra) = session();
    let other = s.run("account.add", &json!({"email":"bob@example.org","incoming":{"host":"imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    for id in [other, 4242] {
        assert!(s.run("sync.applyZimbra", &json!({"result": result(id, "")})).is_err());
        assert!(!s.ews_run_cut.contains(&id), "{id}");
    }
    // The Zimbra account's own refused piece still cuts its run.
    assert!(s.run("sync.applyZimbra", &json!({"result": result(zimbra, "not-its-token")})).is_err());
    assert!(s.ews_run_cut.contains(&zimbra));
}

#[test]
fn work_offline_asks_the_zimbra_server_for_nothing() {
    let (mut s, account) = session();
    s.run("sync.offline", &json!({"value": true})).unwrap();
    for id in ["sync.zimbraRequest", "sync.zimbraFetchRequest"] {
        match s.run(id, &json!({"account": account})) {
            Err(CmdError::Disabled(why)) => assert_eq!(why, crate::cmd::send_receive::OFFLINE, "{id}"),
            other => panic!("{id} wasn't refused: {other:?}"),
        }
    }
}

#[test]
fn a_folder_whose_saved_cursor_is_no_cursor_is_read_from_its_start() {
    let (mut s, id) = session();
    s.mailbox.account_mut(id).unwrap().zimbra.as_mut().unwrap().resume =
        vec![ZimbraResume { folder: "2".into(), cursor: "25\u{0}7".into(), sort: 9 }];
    let request = s.run("sync.zimbraRequest", &json!({"account": id})).unwrap();
    assert_eq!(request["resume"], json!([{"folder": "2", "sort": 0}]));
}

#[test]
fn a_piece_that_cant_be_read_cuts_its_accounts_run() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let mut removal = result(account, "4711");
    removal.partial = true;
    removal.pages.push(ZimbraMessagePage { removed: vec!["257".into()], ..Default::default() });
    apply(&mut s, removal);
    let mut odd = serde_json::to_value(result(account, "4711")).unwrap();
    odd["pages"] = json!(7);
    assert!(s.run("sync.applyZimbra", &json!({"result": odd})).is_err());
    assert!(s.ews_run_cut.contains(&account));
    let mut end = result(account, "4711");
    (end.token, end.whole, end.resume) = ("4712".into(), true, Some(Vec::new()));
    apply(&mut s, end);
    assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4711");
    assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
}

#[test]
fn a_saved_half_read_folder_holds_off_removal_until_its_read_ends() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    s.mailbox.account_mut(account).unwrap().zimbra.as_mut().unwrap().resume =
        vec![ZimbraResume { folder: "2".into(), cursor: "257".into(), sort: 1 }];
    let mut removal = result(account, "4711");
    removal.partial = true;
    removal.pages.push(ZimbraMessagePage { removed: vec!["257".into()], ..Default::default() });
    apply(&mut s, removal);
    // The run's end says it went well but brings no token, so no list of half-read folders either.
    let mut end = result(account, "4711");
    end.whole = true;
    apply(&mut s, end);
    assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
}

#[test]
fn the_request_goes_on_from_no_more_folders_than_a_listing_holds() {
    let (mut s, id) = session();
    let most = mailcraft_model::sync::MAX_LISTED_FOLDERS;
    s.mailbox.account_mut(id).unwrap().zimbra.as_mut().unwrap().resume =
        (0..most + 50).map(|i| ZimbraResume { folder: format!("{}", i + 10), ..Default::default() }).collect();
    let request = s.run("sync.zimbraRequest", &json!({"account": id})).unwrap();
    assert_eq!(request["resume"].as_array().map(Vec::len), Some(most));
}

fn subjects(s: &Session, account: Id, kind_or_name: &str) -> Vec<String> {
    let folder = s.mailbox.folders.iter().find(|f| f.account == account && f.name == kind_or_name).map(|f| f.id).unwrap();
    let mut out: Vec<String> = s.mailbox.messages.values().filter(|m| m.folder == folder).map(|m| m.subject.clone()).collect();
    out.sort();
    out
}

/// A first read: the folders, then each folder afresh, then the token in the last piece.
fn first_read(s: &mut Session, account: Id) {
    let mut listed = result(account, "");
    (listed.listing, listed.well_known, listed.partial) = (Some(listing()), vec!["2".into(), "3".into(), "5".into()], true);
    apply(s, listed);
    let mut inbox = result(account, "");
    inbox.pages = vec![ZimbraMessagePage {
        folder: "2".into(),
        messages: vec![mail("257", "2", "Hello", false), mail("258", "2", "Later", true)],
        afresh: true,
        last: true,
        ..Default::default()
    }];
    inbox.partial = true;
    apply(s, inbox);
    let mut projects = result(account, "");
    projects.pages = vec![ZimbraMessagePage {
        folder: "300".into(),
        messages: vec![mail("259", "300", "Filed", true)],
        afresh: true,
        last: true,
        ..Default::default()
    }];
    projects.partial = true;
    apply(s, projects);
    let mut last = result(account, "");
    (last.token, last.whole, last.resume) = ("4711".into(), true, Some(Vec::new()));
    apply(s, last);
}

#[test]
fn the_request_says_where_the_server_is_and_where_its_changes_go_on_from() {
    let (mut s, account) = session();
    let request = s.run("sync.zimbraRequest", &json!({"account": account})).unwrap();
    assert_eq!((request["url"].as_str(), request["email"].as_str(), request.get("token")), (Some(URL), Some(ME), None));
    assert!(request.get("password").is_none());
    s.mailbox.account_mut(account).unwrap().zimbra.as_mut().unwrap().token = "4711".into();
    assert_eq!(s.run("sync.zimbraRequest", &json!({"account": account})).unwrap()["token"], "4711");
    // A token that is no token is not gone on from.
    s.mailbox.account_mut(account).unwrap().zimbra.as_mut().unwrap().token = "47 11".into();
    assert!(s.run("sync.zimbraRequest", &json!({"account": account})).unwrap().get("token").is_none());
    s.zimbra = false;
    assert!(s.run("sync.zimbraRequest", &json!({"account": account})).is_err());
}

#[test]
fn a_first_read_brings_the_folders_and_their_mail_and_keeps_the_token() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
    assert_eq!(subjects(&s, account, "Projects"), ["Filed"]);
    let projects = s.mailbox.folders.iter().find(|f| f.account == account && f.name == "Projects").unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert_eq!(projects.parent, Some(inbox));
    assert!(s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap().well_known);
    assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4711");
    let hello = s.mailbox.messages.values().find(|m| m.subject == "Hello").unwrap();
    assert!(!hello.read && hello.server.as_ref().is_some_and(|sv| sv.id == "257"));
}

#[test]
fn a_delta_moves_marks_and_removes_by_server_id() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let mut delta = result(account, "4711");
    delta.pages = vec![ZimbraMessagePage {
        messages: vec![mail("257", "300", "Hello", true), mail("260", "2", "Brand new", false)],
        removed: vec!["258".into()],
        last: true,
        ..Default::default()
    }];
    (delta.token, delta.whole, delta.resume) = ("4712".into(), true, Some(Vec::new()));
    let said = apply(&mut s, delta);
    assert_eq!(said["new"], 1);
    assert_eq!(subjects(&s, account, "Inbox"), ["Brand new"]);
    assert_eq!(subjects(&s, account, "Projects"), ["Filed", "Hello"]);
    let hello = s.mailbox.messages.values().find(|m| m.subject == "Hello").unwrap();
    assert!(hello.read);
    assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4712");
}

#[test]
fn a_piece_from_another_token_or_server_is_refused_and_a_cut_run_keeps_no_token() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    // Went on from a token the account isn't at.
    let mut stale = result(account, "1");
    stale.pages = vec![ZimbraMessagePage { removed: vec!["257".into()], last: true, ..Default::default() }];
    assert!(s.run("sync.applyZimbra", &json!({"result": stale})).is_err());
    let mut elsewhere = result(account, "4711");
    elsewhere.url = "https://other.example.org".into();
    assert!(s.run("sync.applyZimbra", &json!({"result": elsewhere})).is_err());
    assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
    // A run that went wrong keeps the mailbox at its token, and removes nothing.
    let mut removal = result(account, "4711");
    removal.pages = vec![ZimbraMessagePage { removed: vec!["257".into()], last: true, ..Default::default() }];
    removal.partial = true;
    apply(&mut s, removal);
    let mut failed = result(account, "4711");
    (failed.token, failed.resume, failed.errors) = ("4712".into(), Some(Vec::new()), vec!["Couldn't send and receive".into()]);
    apply(&mut s, failed);
    assert_eq!(s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap().token, "4711");
    assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
}

#[test]
fn a_piece_names_the_account_and_server_it_is_for_whatever_their_case() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let before = s.mailbox.clone();
    let mut no_email = result(account, "4711");
    no_email.email.clear();
    no_email.pages = vec![ZimbraMessagePage { removed: vec!["257".into()], last: true, ..Default::default() }];
    assert!(s.run("sync.applyZimbra", &json!({"result": no_email})).is_err());
    let mut no_server = result(account, "4711");
    no_server.url.clear();
    no_server.pages = vec![ZimbraMessagePage { removed: vec!["257".into()], last: true, ..Default::default() }];
    assert!(s.run("sync.applyZimbra", &json!({"result": no_server})).is_err());
    assert_eq!(s.mailbox, before);
    // The same server written in other capitals, and with a slash at its end, is the same server.
    let mut same = result(account, "4711");
    same.url = "HTTPS://Zimbra.Example.org/".into();
    same.email = ME.to_uppercase();
    assert!(s.run("sync.applyZimbra", &json!({"result": same})).is_ok());
}

#[test]
fn hostile_pieces_are_held_to_their_sizes() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let mut odd = result(account, "4711");
    let mut bad = mail("bad\u{0}id", "2", "x", true);
    bad.server.folder = "2".into();
    odd.pages = vec![ZimbraMessagePage { messages: vec![bad, mail("261", "2", "Line\r\nbreak", false)], last: true, ..Default::default() }];
    (odd.token, odd.whole, odd.resume) = ("4712".into(), true, Some(Vec::new()));
    assert!(s.run("sync.applyZimbra", &json!({"result": odd.clone()})).is_err());
    // Past the model's check, a piece that had to be cut keeps no token.
    odd.pages[0].messages.remove(0);
    odd.pages[0].messages[0].server.categories = vec!["\u{7}".into()];
    apply(&mut s, odd);
    assert!(subjects(&s, account, "Inbox").contains(&"Line break".to_string()));
}

#[test]
fn a_folder_left_half_read_is_gone_on_from_and_nothing_goes_before_its_end() {
    let (mut s, account) = session();
    // A first read stopped inside the Inbox, at message 258.
    let mut listed = result(account, "");
    (listed.listing, listed.well_known, listed.partial) = (Some(listing()), vec!["2".into(), "3".into(), "5".into()], true);
    apply(&mut s, listed);
    let mut inbox = result(account, "");
    inbox.pages = vec![ZimbraMessagePage {
        folder: "2".into(),
        messages: vec![mail("257", "2", "Hello", false), mail("258", "2", "Later", true)],
        afresh: true,
        ..Default::default()
    }];
    inbox.partial = true;
    apply(&mut s, inbox);
    let mut last = result(account, "");
    (last.token, last.whole, last.resume) = ("4711".into(), true, Some(vec![ZimbraResume { folder: "2".into(), cursor: "258".into(), sort: 5 }]));
    apply(&mut s, last);
    // The next request goes on from where the read stopped, with the token.
    let request = s.run("sync.zimbraRequest", &json!({"account": account})).unwrap();
    assert_eq!(request["resume"], json!([{"folder": "2", "cursor": "258", "sort": 5}]), "{request}");
    // A delta that still leaves a folder half-read removes nothing, though the run went well.
    let mut delta = result(account, "4711");
    delta.pages = vec![ZimbraMessagePage { removed: vec!["257".into()], last: true, ..Default::default() }];
    (delta.token, delta.whole, delta.resume) = ("4712".into(), true, Some(vec![ZimbraResume { folder: "2".into(), cursor: "258".into(), sort: 5 }]));
    apply(&mut s, delta);
    assert_eq!(subjects(&s, account, "Inbox"), ["Hello", "Later"]);
    // The run that finishes the read ends it: "Hello" is gone, "Later" is all the read brought.
    let mut tail = result(account, "4712");
    tail.pages = vec![ZimbraMessagePage { folder: "2".into(), messages: vec![mail("258", "2", "Later", true)], last: true, ..Default::default() }];
    (tail.token, tail.whole, tail.resume) = ("4713".into(), true, Some(Vec::new()));
    apply(&mut s, tail);
    assert_eq!(subjects(&s, account, "Inbox"), ["Later"]);
    let server = s.mailbox.account(account).unwrap().zimbra.as_ref().unwrap();
    assert_eq!((server.token.as_str(), server.resume.is_empty()), ("4713", true));
}

#[test]
fn messages_are_fetched_whole_by_server_id() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let hello = s.mailbox.messages.values().find(|m| m.subject == "Hello").map(|m| m.id).unwrap();
    let asked = s.run("sync.zimbraFetchRequest", &json!({"account": account, "message": hello})).unwrap();
    assert_eq!(asked["ids"], json!(["257"]));
    assert_eq!(asked["url"], URL);
    let mime =
        b"From: Bob <bob@example.org>\r\nTo: ada@example.org\r\nSubject: Hello\r\nMessage-ID: <257@example.org>\r\n\r\nThe whole text.\r\n".to_vec();
    let fetched = GraphFetchResult { account, email: ME.into(), fetched: vec![GraphFetched { id: "257".into(), mime }], ..Default::default() };
    s.run("sync.applyZimbraFetch", &json!({"result": fetched})).unwrap();
    let hello = s.mailbox.message(hello).unwrap();
    assert!(hello.body.text.contains("The whole text"));
    assert_eq!(hello.flag, FlagStatus::None);
}
