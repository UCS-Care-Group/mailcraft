// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's inbox categories read with Gmail's own search (#911), against the fake server.

use mailcraft_model::GmailCategory;
use mailcraft_model::sync::{GmailMove, GmailSorted};

use super::*;
use crate::fake::GMAIL_IDS;

/// A server that behaves as Gmail does, its Inbox's message 2 in Social and 5 in Promotions.
fn gmail_world() -> ImapWorld {
    let categories = [("INBOX", 2, "social"), ("INBOX", 5, "promotions"), ("Projects", 1, "forums")];
    ImapWorld { gmail: true, gmail_categories: categories.iter().map(|(at, uid, c)| (at.to_string(), *uid, c.to_string())).collect(), ..imap_world() }
}

fn gmail() -> Host {
    Host { files_sent: true, gmail: true, ..Default::default() }
}

fn searches(imap: &FakeImap) -> Vec<String> {
    imap.world()
        .log
        .iter()
        .filter(|line| line.contains("X-GM-RAW"))
        .map(|line| line.split_once(' ').map_or("", |(_, rest)| rest).to_string())
        .collect()
}

fn sorted(uid: u32, category: GmailCategory) -> GmailSorted {
    GmailSorted { uid, category, id: GMAIL_IDS + u64::from(uid) }
}

#[test]
fn an_inbox_on_gmail_comes_with_its_categories() {
    let imap = Fake::imap(gmail_world());
    let result = whole(&request(account(Some(&imap), None), local_folders()), Limits::in_pieces(), gmail());
    assert_eq!(result.errors, Vec::<String>::new());
    let inbox = delta(&result, "INBOX");
    // Each new message with its own, and Gmail's id of it; Primary is what no search found.
    let got: Vec<(u32, Option<GmailCategory>, u64)> = inbox.new.iter().map(|m| (m.uid, m.gmail_category, m.gmail_id)).collect();
    assert_eq!(
        got,
        [
            (1, Some(GmailCategory::Primary), GMAIL_IDS + 1),
            (2, Some(GmailCategory::Social), GMAIL_IDS + 2),
            (5, Some(GmailCategory::Promotions), GMAIL_IDS + 5)
        ]
    );
    // And every message of the Inbox's, said once with the piece that ends it.
    assert_eq!(inbox.gmail, [sorted(1, GmailCategory::Primary), sorted(2, GmailCategory::Social), sorted(5, GmailCategory::Promotions)]);
    // Four searches of the Inbox, over the messages it lists (6 is marked as deleted, and not
    // asked about); no other folder is asked.
    assert_eq!(
        searches(&imap),
        [
            "UID SEARCH UID 1:6 UNDELETED X-GM-RAW \"category:social\"",
            "UID SEARCH UID 1:6 UNDELETED X-GM-RAW \"category:promotions\"",
            "UID SEARCH UID 1:6 UNDELETED X-GM-RAW \"category:updates\"",
            "UID SEARCH UID 1:6 UNDELETED X-GM-RAW \"category:forums\""
        ]
    );
    let projects = delta(&result, "Projects");
    assert!(projects.gmail.is_empty() && projects.new.iter().all(|m| m.gmail_category.is_none() && m.gmail_id == 0));
    assert_eq!(imap.world().log.iter().filter(|line| line.contains("X-GM-MSGID")).count(), 1, "the Inbox's listing alone");

    // Gmail sorts a message again: the next run says so, for the messages already here.
    let folders = applied(&local_folders(), &result);
    imap.world().gmail_categories.retain(|(_, uid, _)| *uid != 2);
    imap.world().gmail_categories.push(("INBOX".into(), 2, "updates".into()));
    let again = whole(&request(account(Some(&imap), None), folders), Limits::in_pieces(), gmail());
    let inbox = delta(&again, "INBOX");
    assert!(inbox.new.is_empty());
    assert_eq!(inbox.gmail, [sorted(1, GmailCategory::Primary), sorted(2, GmailCategory::Updates), sorted(5, GmailCategory::Promotions)]);
}

#[test]
fn no_other_server_is_asked() {
    let imap = Fake::imap(gmail_world());
    let result = whole(&request(account(Some(&imap), None), local_folders()), Limits::in_pieces(), Host::default());
    assert!(searches(&imap).is_empty());
    assert!(!imap.world().log.iter().any(|line| line.contains("X-GM-MSGID")));
    let inbox = delta(&result, "INBOX");
    assert!(inbox.gmail.is_empty() && inbox.new.iter().all(|m| m.gmail_category.is_none() && m.gmail_id == 0));
}

#[test]
fn a_search_that_is_refused_leaves_the_categories_out_and_the_mail_in() {
    // Gmail's name, and a server that doesn't know its search: no message is taken for Primary.
    let imap = Fake::imap(ImapWorld { gmail: false, ..gmail_world() });
    let result = whole(&request(account(Some(&imap), None), local_folders()), Limits::in_pieces(), gmail());
    assert_eq!(result.errors, Vec::<String>::new(), "nothing to tell the owner");
    let inbox = delta(&result, "INBOX");
    assert_eq!(uids(&inbox.new), [1, 2, 5]);
    assert!(inbox.gmail.is_empty() && inbox.new.iter().all(|m| m.gmail_category.is_none()));
    assert_eq!(searches(&imap).len(), 1, "given up after the first refusal");
}

#[test]
fn a_server_naming_other_messages_is_believed_for_its_own_alone() {
    let imap = Fake::imap(gmail_world());
    // Messages the Inbox hasn't got, a deleted one inside the set asked about, and one the set
    // didn't name.
    let said = "* SEARCH 1 6 99 4294967295\r\n{tag} OK done\r\n".to_string();
    imap.world().answers.push(("category:social".into(), said));
    let result = whole(&request(account(Some(&imap), None), local_folders()), Limits::in_pieces(), gmail());
    let inbox = delta(&result, "INBOX");
    assert_eq!(inbox.gmail, [sorted(1, GmailCategory::Social), sorted(2, GmailCategory::Primary), sorted(5, GmailCategory::Promotions)]);
}

#[test]
fn a_large_inbox_is_searched_in_sets_the_answer_fits() {
    let mut world = gmail_world();
    let many: Vec<FakeMessage> = (1..=12_001).map(|uid| message(uid, "\\Seen", "Many")).collect();
    inbox(&mut world).messages = many;
    world.gmail_categories = vec![("INBOX".into(), 12_001, "forums".into()), ("INBOX".into(), 3, "social".into())];
    let imap = Fake::imap(world);
    // Only how far down it is read matters here: one message is downloaded.
    let request = SyncRequest { max_per_folder: 1, ..request(account(Some(&imap), None), local_folders()) };
    let result = whole(&request, Limits::in_pieces(), gmail());
    let sets: Vec<String> =
        searches(&imap).iter().filter(|s| s.ends_with("social\"")).map(|s| s.split(' ').nth(3).unwrap_or("").to_string()).collect();
    assert_eq!(sets, ["1:5000", "5001:10000", "10001:12001"]);
    let inbox = delta(&result, "INBOX");
    assert_eq!(inbox.gmail.len(), 12_001);
    assert_eq!(inbox.new.first().map(|m| (m.uid, m.gmail_category)), Some((12_001, Some(GmailCategory::Forums))));
    assert!(inbox.gmail.contains(&sorted(3, GmailCategory::Social)));
}

#[test]
fn deleted_messages_count_towards_the_sets_and_are_never_sorted() {
    let mut world = gmail_world();
    // Every other message marked as deleted by another program: the sets are of what the server
    // lists, so an answer naming all of a set still fits in a line.
    let many: Vec<FakeMessage> = (1..=6_000).map(|uid| message(uid, if uid % 2 == 0 { "\\Deleted" } else { "\\Seen" }, "Many")).collect();
    inbox(&mut world).messages = many;
    world.gmail_categories = vec![("INBOX".into(), 4, "social".into()), ("INBOX".into(), 5_999, "social".into())];
    // And a server whose search finds deleted messages anyway.
    world.gmail_finds_deleted = true;
    let imap = Fake::imap(world);
    let request = SyncRequest { max_per_folder: 1, ..request(account(Some(&imap), None), local_folders()) };
    let result = whole(&request, Limits::in_pieces(), gmail());
    assert_eq!(result.errors, Vec::<String>::new());
    let sets: Vec<String> =
        searches(&imap).iter().filter(|s| s.ends_with("social\"")).map(|s| s.split(' ').nth(3).unwrap_or("").to_string()).collect();
    assert_eq!(sets, ["1:5000", "5001:6000"]);
    let inbox = delta(&result, "INBOX");
    assert_eq!(inbox.gmail.len(), 3_000, "the messages not marked as deleted, and no other");
    assert!(inbox.gmail.iter().all(|g| g.uid % 2 == 1));
    assert!(inbox.gmail.contains(&sorted(5_999, GmailCategory::Social)));
    assert!(inbox.gmail.contains(&sorted(1, GmailCategory::Primary)));
}

/// A Gmail account that signs in with Google, against the fake IMAP server, with moves between
/// categories to tell Gmail's API (the stand-in), whose token is the same as the IMAP server's.
fn signed_in(imap: &FakeImap, moves: Vec<GmailMove>) -> SyncRequest {
    let oauth = OAuth { provider: OAuthProvider::Google, client_id: "client-1".into(), ..Default::default() };
    let account = Account { oauth: Some(oauth), ..oauth_account(Some(imap), None) };
    SyncRequest { access_token: crate::gmail::standin::TOKEN.into(), gmail_moves: moves, ..request(account, local_folders()) }
}

fn google_world() -> ImapWorld {
    ImapWorld { user: "ada@example.org".into(), token: crate::gmail::standin::TOKEN.into(), ..gmail_world() }
}

fn moving(message: Id, uid: u32, category: GmailCategory) -> GmailMove {
    GmailMove { message, id: GMAIL_IDS + u64::from(uid), category }
}

fn modify_path(uid: u32) -> String {
    format!("/gmail/v1/users/me/messages/{:x}/modify", GMAIL_IDS + u64::from(uid))
}

#[test]
fn moves_between_categories_go_to_gmails_api_before_the_inbox_is_read() {
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    api.state().add(GMAIL_IDS + 2, &["INBOX", "CATEGORY_SOCIAL"]);
    api.state().add(GMAIL_IDS + 5, &["INBOX", "CATEGORY_PROMOTIONS"]);
    let _routed = api.route();
    let moves = vec![moving(31, 2, GmailCategory::Primary), moving(32, 5, GmailCategory::Updates)];
    let result = whole(&signed_in(&imap, moves.clone()), Limits::in_pieces(), gmail());
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.gmail_moved, moves);
    let paths: Vec<String> = api.requests().iter().map(|a| a.path.clone()).collect();
    assert_eq!(paths, [modify_path(2), modify_path(5)]);
    assert_eq!(api.state().categories(GMAIL_IDS + 2), ["CATEGORY_PERSONAL"]);
    assert_eq!(api.state().categories(GMAIL_IDS + 5), ["CATEGORY_UPDATES"]);
    // The Inbox was read after them, as ever.
    assert!(!delta(&result, "INBOX").gmail.is_empty());
}

#[test]
fn no_request_goes_to_gmails_api_without_moves_or_a_google_sign_in() {
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    api.state().add(GMAIL_IDS + 2, &["INBOX", "CATEGORY_SOCIAL"]);
    let _routed = api.route();
    // No moves (what a mailbox sends while moves are switched off): nothing at all.
    let result = whole(&signed_in(&imap, Vec::new()), Limits::in_pieces(), gmail());
    assert_eq!(result.errors, Vec::<String>::new());
    assert!(result.gmail_moved.is_empty());
    // Moves, but for an account signed in with a password, or another provider, or a server that
    // isn't Gmail's: none goes either.
    let moves = vec![moving(31, 2, GmailCategory::Primary)];
    let with_password = Fake::imap(gmail_world());
    let request = SyncRequest { gmail_moves: moves.clone(), ..request(account(Some(&with_password), None), local_folders()) };
    assert!(whole(&request, Limits::in_pieces(), gmail()).gmail_moved.is_empty());
    let mut microsoft = signed_in(&imap, moves.clone());
    microsoft.account.oauth = Some(OAuth { provider: OAuthProvider::Microsoft, ..Default::default() });
    assert!(whole(&microsoft, Limits::in_pieces(), gmail()).gmail_moved.is_empty());
    assert!(whole(&signed_in(&imap, moves), Limits::in_pieces(), Host::default()).gmail_moved.is_empty());
    assert!(api.requests().is_empty(), "{:?}", api.requests());
    assert_eq!(api.state().categories(GMAIL_IDS + 2), ["CATEGORY_SOCIAL"]);
}

#[test]
fn a_move_gmail_refuses_waits_and_a_busy_gmail_holds_the_rest() {
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    for uid in [1u32, 2, 5] {
        api.state().add(GMAIL_IDS + u64::from(uid), &["INBOX", "CATEGORY_PERSONAL"]);
    }
    let _routed = api.route();
    let moves = vec![moving(30, 1, GmailCategory::Social), moving(31, 2, GmailCategory::Forums), moving(32, 5, GmailCategory::Updates)];
    // One refused: the others go, and the problem is said once, without the token.
    api.state().echo = true;
    api.state().faults.push_back(400);
    let result = whole(&signed_in(&imap, moves.clone()), Limits::in_pieces(), gmail());
    assert_eq!(result.gmail_moved, moves[1..]);
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors[0].contains("Gmail") && !result.errors[0].contains(crate::gmail::standin::TOKEN), "{:?}", result.errors);
    // Busy: nothing after it is asked this run.
    let before = api.requests().len();
    api.state().faults.push_back(429);
    let result = whole(&signed_in(&imap, moves.clone()), Limits::in_pieces(), gmail());
    assert!(result.gmail_moved.is_empty());
    assert_eq!(api.requests().len(), before + 1);
    assert!(result.errors.iter().any(|e| e.contains("3 messages")), "{:?}", result.errors);
    // A token Gmail doesn't take: likewise, and the mail is read all the same.
    api.state().faults.push_back(401);
    let result = whole(&signed_in(&imap, moves), Limits::in_pieces(), gmail());
    assert!(result.gmail_moved.is_empty());
    assert!(!delta(&result, "INBOX").gmail.is_empty());
}
