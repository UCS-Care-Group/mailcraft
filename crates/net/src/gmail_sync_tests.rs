// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's inbox categories read with Gmail's own search (#911), against the fake server.

use mailcraft_model::GmailCategory;
use mailcraft_model::sync::{GmailMove, GmailSorted, MAX_GMAIL_MOVES};

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
    // The Inbox's listing asks for Gmail's ids and labels with the flags, every other folder's for
    // its labels alone (#1153), each in the one command.
    let log = imap.world().log.clone();
    let listings: Vec<&String> = log.iter().filter(|line| line.contains("UID FETCH 1:*")).collect();
    assert_eq!(listings.iter().filter(|line| line.contains("(UID FLAGS X-GM-MSGID X-GM-LABELS)")).count(), 1, "{listings:?}");
    assert!(listings.len() > 1 && listings.iter().all(|line| line.contains("X-GM-LABELS)")), "{listings:?}");

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

/// The ids each batch the stand-in was sent named, in hex, in order.
fn batches(api: &crate::gmail::standin::StandIn) -> Vec<Vec<String>> {
    let ids = |body: &str| -> Option<Vec<String>> {
        serde_json::from_str::<serde_json::Value>(body).ok()?["ids"].as_array()?.iter().map(|v| v.as_str().map(str::to_string)).collect()
    };
    api.requests().iter().map(|a| ids(&a.body).unwrap_or_default()).collect()
}

fn hex(uid: u32) -> String {
    format!("{:x}", GMAIL_IDS + u64::from(uid))
}

#[test]
fn moves_between_categories_go_to_gmails_api_before_the_inbox_is_read() {
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    api.state().add(GMAIL_IDS + 1, &["INBOX", "CATEGORY_SOCIAL"]);
    api.state().add(GMAIL_IDS + 2, &["INBOX", "CATEGORY_SOCIAL"]);
    api.state().add(GMAIL_IDS + 5, &["INBOX", "CATEGORY_PROMOTIONS"]);
    let _routed = api.route();
    let moves = vec![moving(32, 5, GmailCategory::Updates), moving(31, 2, GmailCategory::Primary), moving(30, 1, GmailCategory::Primary)];
    let result = whole(&signed_in(&imap, moves.clone()), Limits::in_pieces(), gmail());
    assert_eq!(result.errors, Vec::<String>::new());
    // One batch for each category moved to.
    assert_eq!(result.gmail_moved, [moves[2], moves[1], moves[0]]);
    assert!(api.requests().iter().all(|a| a.path == "/gmail/v1/users/me/messages/batchModify"));
    assert_eq!(batches(&api), [vec![hex(1), hex(2)], vec![hex(5)]]);
    assert_eq!(api.state().categories(GMAIL_IDS + 1), ["CATEGORY_PERSONAL"]);
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
fn switched_off_a_request_with_moves_sends_nothing_to_gmails_api() {
    // The build's switch, which this crate holds to whatever a request asks.
    const { assert!(!mailcraft_model::gmail::GMAIL_CATEGORY_MOVES, "ships switched off until Google's review") };
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    api.state().add(GMAIL_IDS + 2, &["INBOX", "CATEGORY_SOCIAL"]);
    let _routed = api.route_switched_off();
    assert!(!crate::gmail::switched_on());
    let result = whole(&signed_in(&imap, vec![moving(31, 2, GmailCategory::Primary)]), Limits::in_pieces(), gmail());
    assert_eq!(result.errors, Vec::<String>::new());
    assert!(result.gmail_moved.is_empty());
    assert!(api.requests().is_empty(), "{:?}", api.requests());
    // Nor does the client itself, asked directly.
    let asked = crate::gmail::batch_modify(crate::gmail::standin::TOKEN, &[GMAIL_IDS + 2], GmailCategory::Primary);
    assert!(matches!(asked, Err(NetError::Config(_))), "{asked:?}");
    assert!(api.requests().is_empty());
    assert_eq!(api.state().categories(GMAIL_IDS + 2), ["CATEGORY_SOCIAL"]);
}

#[test]
fn two_moves_of_one_message_send_its_id_once() {
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    api.state().add(GMAIL_IDS + 2, &["INBOX", "CATEGORY_SOCIAL"]);
    let _routed = api.route();
    // Two copies here of Gmail's one message.
    let moves = vec![moving(31, 2, GmailCategory::Primary), moving(41, 2, GmailCategory::Primary)];
    let result = whole(&signed_in(&imap, moves.clone()), Limits::in_pieces(), gmail());
    assert_eq!(result.gmail_moved, moves);
    assert_eq!(batches(&api), [vec![hex(2)]]);
}

/// `uids`, each a message of Gmail's in Primary, moved here (as messages 100 + uid) to `category`.
fn all_to(api: &crate::gmail::standin::StandIn, uids: std::ops::RangeInclusive<u32>, category: GmailCategory) -> Vec<GmailMove> {
    uids.map(|uid| {
        api.state().add(GMAIL_IDS + u64::from(uid), &["INBOX", "CATEGORY_PERSONAL"]);
        moving(Id::from(uid) + 100, uid, category)
    })
    .collect()
}

fn sorted_by_message(mut moves: Vec<GmailMove>) -> Vec<GmailMove> {
    moves.sort_by_key(|m| m.message);
    moves
}

#[test]
fn a_stale_id_costs_a_few_requests_and_the_rest_go() {
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    let moves = all_to(&api, 1..=8, GmailCategory::Social);
    api.state().refused.push(GMAIL_IDS + 6);
    let _routed = api.route();
    let result = whole(&signed_in(&imap, moves.clone()), Limits::in_pieces(), gmail());
    // Halved: 1-8 refused, 1-4 taken, 5-8 refused, 5-6 refused, 7-8 taken, 5 taken, 6 refused alone.
    let six = moves[5];
    assert_eq!(sorted_by_message(result.gmail_moved.clone()), moves.iter().copied().filter(|m| *m != six).collect::<Vec<_>>());
    assert_eq!(result.gmail_refused, [six]);
    assert_eq!((result.gmail_unsent, api.requests().len()), (0, 7));
    // Said by the engine, which counts the refusals: nothing here.
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(api.state().categories(GMAIL_IDS + 6), ["CATEGORY_PERSONAL"]);
    assert_eq!(api.state().categories(GMAIL_IDS + 7), ["CATEGORY_SOCIAL"]);
}

#[test]
fn halving_waits_for_every_categorys_first_batch_and_stops_at_the_cap() {
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    // Primary's batch refused through and through: its halves alone would take every request.
    let mut moves = all_to(&api, 1..=100, GmailCategory::Primary);
    api.state().refused = moves.iter().map(|m| m.id).collect();
    let social = all_to(&api, 101..=101, GmailCategory::Social);
    let updates = all_to(&api, 102..=102, GmailCategory::Updates);
    moves.extend(social.iter().chain(&updates).copied());
    let _routed = api.route();
    let result = whole(&signed_in(&imap, moves), Limits::in_pieces(), gmail());
    assert_eq!(result.gmail_moved, [social[0], updates[0]]);
    assert_eq!(batches(&api).get(1..3), Some(&[vec![hex(101)], vec![hex(102)]][..]));
    assert_eq!(api.requests().len(), 32);
    // The rest of Primary's waits; none of it was refused on its own yet.
    assert!(result.gmail_refused.is_empty());
    assert_eq!(result.gmail_unsent, 100);
    assert_eq!(result.errors, Vec::<String>::new());
    // All of them refused, and nothing else: the cap all the same.
    let before = api.requests().len();
    let all: Vec<GmailMove> = (1..=MAX_GMAIL_MOVES as u32).map(|uid| moving(Id::from(uid) + 1_000, uid, GmailCategory::Forums)).collect();
    api.state().refused = all.iter().map(|m| m.id).collect();
    let result = whole(&signed_in(&imap, all), Limits::in_pieces(), gmail());
    assert!(result.gmail_moved.is_empty());
    assert_eq!((api.requests().len() - before, result.gmail_unsent), (32, MAX_GMAIL_MOVES));
}

#[test]
fn the_moves_of_one_message_stay_in_one_half() {
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    let mut moves = all_to(&api, 1..=3, GmailCategory::Updates);
    // A second copy here of message 2, and message 3 stale: 1, 2, 2 | 3, not 1, 2 | 2, 3.
    moves.push(moving(202, 2, GmailCategory::Updates));
    api.state().refused.push(GMAIL_IDS + 3);
    let _routed = api.route();
    let result = whole(&signed_in(&imap, moves.clone()), Limits::in_pieces(), gmail());
    assert_eq!(batches(&api), [vec![hex(1), hex(2), hex(3)], vec![hex(1), hex(2)], vec![hex(3)]]);
    assert_eq!(sorted_by_message(result.gmail_moved), sorted_by_message(vec![moves[0], moves[1], moves[3]]));
    assert_eq!(result.gmail_refused, [moves[2]]);
}

#[test]
fn a_stop_partway_through_halving_keeps_what_went_and_the_rest_waits() {
    for status in [401u16, 403, 429, 500, 503] {
        let imap = Fake::imap(google_world());
        let api = crate::gmail::standin::StandIn::start().unwrap();
        let mut moves = all_to(&api, 1..=4, GmailCategory::Primary);
        let social = all_to(&api, 5..=5, GmailCategory::Social);
        moves.extend(&social);
        let _routed = api.route();
        api.state().echo = true;
        // Primary's 1-4 refused, Social's 5 taken, Primary's 1-2 taken, Primary's 3-4: `status`.
        api.state().faults.extend([400, 0, 0, status]);
        let result = whole(&signed_in(&imap, moves.clone()), Limits::in_pieces(), gmail());
        assert_eq!(sorted_by_message(result.gmail_moved.clone()), [moves[0], moves[1], social[0]], "{status}");
        assert!(result.gmail_refused.is_empty() && result.gmail_unsent == 0, "{status}");
        assert_eq!(api.requests().len(), 4, "{status}");
        assert_eq!(result.errors.len(), 1, "{status}: {:?}", result.errors);
        let said = &result.errors[0];
        assert!(said.contains("2 messages") && !said.contains(crate::gmail::standin::TOKEN), "{status}: {said}");
        assert_eq!(api.state().categories(GMAIL_IDS + 3), ["CATEGORY_PERSONAL"], "{status}");
        // And the mail is read all the same.
        assert!(!delta(&result, "INBOX").gmail.is_empty(), "{status}");
    }
}

#[test]
fn a_batch_gmail_refuses_waits_and_a_busy_gmail_holds_the_rest() {
    let imap = Fake::imap(google_world());
    let api = crate::gmail::standin::StandIn::start().unwrap();
    for uid in [1u32, 2, 5] {
        api.state().add(GMAIL_IDS + u64::from(uid), &["INBOX", "CATEGORY_PERSONAL"]);
    }
    let _routed = api.route();
    let moves = vec![moving(30, 1, GmailCategory::Social), moving(31, 2, GmailCategory::Forums), moving(32, 5, GmailCategory::Updates)];
    // One refused (Social's, the first, on its own): the others go.
    api.state().faults.push_back(400);
    let result = whole(&signed_in(&imap, moves.clone()), Limits::in_pieces(), gmail());
    assert_eq!(result.gmail_moved, [moves[2], moves[1]]);
    assert_eq!(result.gmail_refused, [moves[0]]);
    assert_eq!(api.state().categories(GMAIL_IDS + 1), ["CATEGORY_PERSONAL"]);
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
