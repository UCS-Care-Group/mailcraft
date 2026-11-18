// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Runs against the stand-in: a first read, a delta, a token the server lost, a refused sign-in,
//! and messages fetched whole; and what goes over the wire.

use std::sync::atomic::AtomicBool;

use mailcraft_model::FolderKind;
use mailcraft_model::sync::{ZimbraFetchRequest, ZimbraMailRequest, ZimbraMailResult};

use crate::http::Secret;
use crate::standin::{PASSWORD, StandIn, TOKEN, USER, block_on};
use crate::{fetch_messages, sync_mail};

const URL: &str = "http://127.0.0.1:7070";

fn request(token: &str) -> ZimbraMailRequest {
    ZimbraMailRequest { account: 3, email: USER.into(), url: URL.into(), token: token.into(), ..Default::default() }
}

fn run(server: &StandIn, request: &ZimbraMailRequest, password: &str) -> Vec<ZimbraMailResult> {
    let mut pieces = Vec::new();
    block_on(sync_mail(server, request, &Secret::new(password), &AtomicBool::new(false), &mut |piece| pieces.push(piece))).unwrap();
    pieces
}

fn whole(pieces: &[ZimbraMailResult]) -> ZimbraMailResult {
    let mut all = ZimbraMailResult::default();
    for piece in pieces {
        assert_eq!(piece.check(), Ok(()), "{piece:?}");
        all.absorb(piece.clone());
    }
    all
}

#[test]
fn a_first_read_lists_the_folders_of_mail_and_reads_each_from_its_start() {
    let server = StandIn::new();
    let projects = server.add_folder(2, "Projects");
    let first = server.deliver(2, "First", "u");
    let second = server.deliver(2, "Second", "f");
    server.deliver(projects, "Filed", "");
    server.deliver(14, "A chat", "");
    let pieces = run(&server, &request(""), PASSWORD);
    let all = whole(&pieces);
    assert_eq!(all.errors, Vec::<String>::new());
    assert!(all.whole && !all.partial && !all.token.is_empty());
    assert!(pieces.iter().take(pieces.len() - 1).all(|p| p.partial) && !pieces.last().unwrap().partial);
    let listing = all.listing.clone().unwrap();
    let names: Vec<(&str, &str, FolderKind)> = listing.folders.iter().map(|f| (f.id.as_str(), f.name.as_str(), f.kind)).collect();
    assert!(listing.complete);
    assert!(names.contains(&("2", "Inbox", FolderKind::Inbox)) && names.contains(&("3", "Trash", FolderKind::Deleted)));
    assert!(names.iter().all(|(id, _, _)| !["7", "10", "14", "1"].contains(id)), "{names:?}");
    let sub = listing.folders.iter().find(|f| f.id == projects.to_string()).unwrap();
    assert_eq!((sub.parent.as_str(), sub.well_known), ("2", false));
    assert!(all.well_known.contains(&"2".to_string()) && !all.well_known.contains(&projects.to_string()));
    // The Inbox first, newest first; a fresh read of each folder, begun and ended.
    let inbox: Vec<_> = all.pages.iter().filter(|p| p.folder == "2").collect();
    assert_eq!(all.pages.first().map(|p| p.folder.as_str()), Some("2"));
    assert!(inbox.first().unwrap().afresh && inbox.last().unwrap().last);
    let ids: Vec<&str> = inbox.iter().flat_map(|p| p.messages.iter().map(|m| m.server.id.as_str())).collect();
    assert_eq!(ids, [second.to_string(), first.to_string()]);
    let m = &inbox[0].messages[0];
    assert_eq!((m.subject.as_str(), m.from.email.as_str(), m.server.folder.as_str()), ("Second", "bob@example.org", "2"));
    assert_eq!(
        (m.server.read, m.server.flag, m.message_id.as_str()),
        (true, mailcraft_model::FlagStatus::Flagged, format!("{second}@example.org").as_str())
    );
    assert!(!inbox.iter().flat_map(|p| &p.messages).any(|m| m.subject == "A chat"));
    // Every page went on from the token the run asked from, and the password went once.
    assert!(all.pages.iter().all(|p| !p.folder.is_empty()));
    let log = server.state.borrow().log.clone();
    assert_eq!(log.iter().filter(|l| *l == "AuthRequest").count(), 1);
    // The token is asked for before the folders are listed, so that a change made while the
    // folders are read is part of the delta that goes on from the run.
    assert_eq!(log.get(1).map(String::as_str), Some("SyncRequest"));
    assert_eq!(log.get(2).map(String::as_str), Some("GetFolderRequest"));
}

#[test]
fn a_delta_brings_what_changed_since_the_token_and_what_was_deleted() {
    let server = StandIn::new();
    let kept = server.deliver(2, "Kept", "u");
    let gone = server.deliver(2, "Gone", "");
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    server.change(kept, |m| m.flags.clear());
    server.delete(gone);
    let new = server.deliver(5, "Sent elsewhere", "");
    let chat = server.deliver(2, "Moved to the chats", "");
    server.change(chat, |m| m.folder = 14);
    let all = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    assert!(all.whole && all.token != token && !all.token.is_empty());
    assert_eq!(all.from, token);
    let changes: Vec<_> = all.pages.iter().filter(|p| p.folder.is_empty()).collect();
    assert_eq!(changes.len(), 1);
    let page = changes[0];
    assert!(page.last && !page.afresh);
    let read: Vec<(&str, bool)> = page.messages.iter().map(|m| (m.server.id.as_str(), m.server.read)).collect();
    assert!(read.contains(&(kept.to_string().as_str(), true)) && read.iter().any(|(id, _)| *id == new.to_string()));
    assert!(page.removed.contains(&gone.to_string()) && page.removed.contains(&chat.to_string()));
    // Nothing changed: the next delta is empty and the token stays where it is.
    let again = whole(&run(&server, &request(&all.token), PASSWORD));
    assert!(again.pages.iter().all(|p| p.messages.is_empty() && p.removed.is_empty()));
    assert_eq!(again.token, all.token);
}

#[test]
fn a_long_delta_comes_a_page_at_a_time() {
    let server = StandIn::new();
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    for n in 0..1200 {
        server.deliver(2, &format!("Message {n}"), "u");
    }
    let all = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    assert_eq!(all.pages.len(), 3);
    assert!(all.pages.last().unwrap().last && !all.pages.first().unwrap().last);
    assert_eq!(all.pages.iter().map(|p| p.messages.len()).sum::<usize>(), 1200);
}

#[test]
fn a_message_in_trash_or_junk_the_delta_names_is_not_dropped() {
    let server = StandIn::new();
    let trashed = server.deliver(3, "Trashed", "");
    let junked = server.deliver(4, "Junked", "");
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    server.change(trashed, |m| m.flags = "u".into());
    server.change(junked, |m| m.flags = "u".into());
    let all = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    let page = all.pages.iter().find(|p| p.folder.is_empty()).unwrap();
    let ids: Vec<&str> = page.messages.iter().map(|m| m.server.id.as_str()).collect();
    assert!(ids.contains(&trashed.to_string().as_str()) && ids.contains(&junked.to_string().as_str()), "{ids:?}");
    assert!(page.removed.is_empty());
}

#[test]
fn a_folder_made_while_the_run_goes_on_keeps_its_message() {
    let server = StandIn::new();
    let moving = server.deliver(2, "Moving", "");
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    let made = server.add_folder_later(2, "Made mid-run");
    server.change(moving, |m| m.folder = made);
    let all = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    // The folders were listed again inside the run (the first run's listing makes three), the
    // listing has the new one, and the message came back in it rather than as removed.
    let log = server.state.borrow().log.clone();
    assert_eq!(log.iter().filter(|l| *l == "GetFolderRequest").count(), 3, "{log:?}");
    assert!(all.listing.clone().unwrap().folders.iter().any(|f| f.id == made.to_string()));
    let page = all.pages.iter().find(|p| p.folder.is_empty()).unwrap();
    assert!(page.messages.iter().any(|m| m.server.id == moving.to_string() && m.server.folder == made.to_string()));
    assert!(page.removed.is_empty());
}

#[test]
fn an_id_the_search_cant_return_fails_the_run_without_its_token() {
    let server = StandIn::new();
    let gone = server.deliver(2, "Vanishing", "");
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    server.change(gone, |m| m.flags = "u".into());
    server.hide(gone);
    let all = whole(&run(&server, &request(&token), PASSWORD));
    // The delta names it and no message came back: whether it is really gone can't be told — an
    // index that hasn't caught up looks the same. The run fails instead of removing it here, and
    // no token moves on: the next run asks for the same delta, where a message that really was
    // deleted turns up in `deleted` — so nothing is lost by waiting.
    assert!(!all.errors.is_empty());
    assert!(!all.whole && all.token.is_empty());
    assert!(!all.pages.iter().any(|p| p.removed.contains(&gone.to_string())));
}

#[test]
fn an_id_the_server_answers_gone_counts_as_removed() {
    let server = StandIn::new();
    let gone = server.deliver(2, "Vanishing", "");
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    // Changed in the delta, deleted before the read — too late for the delta's `deleted` list.
    // The `GetMsgRequest` probe faults `mail.NO_SUCH_MSG`: the server's own word that it is gone.
    // The run counts it removed and moves its token on; failing here would ask the same page for ever.
    server.change(gone, |m| m.flags = "u".into());
    server.vanish(gone);
    let all = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    assert!(all.whole && !all.token.is_empty());
    assert!(all.pages.iter().any(|p| p.removed.contains(&gone.to_string())));
}

#[test]
fn a_message_moved_to_trash_is_not_the_deleted_one() {
    let server = StandIn::new();
    let trashed = server.deliver(2, "Moved to Trash", "");
    let deleted = server.deliver(2, "Really deleted", "");
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    // One delta page names both: one moved to Trash in webmail (a search with no scope would
    // leave it out), one deleted before the read. The batch faults on the deletion, the probe
    // names each by itself: the Trash message comes back, `mail.NO_SUCH_MSG` is the deleted one's
    // own word. Only the deleted one is removed — the moved one must not vanish here.
    server.change(trashed, |m| m.folder = 3);
    server.change(deleted, |m| m.flags = "u".into());
    server.vanish(deleted);
    let all = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    assert!(all.whole && !all.token.is_empty());
    let removed: Vec<&str> = all.pages.iter().flat_map(|p| p.removed.iter().map(String::as_str)).collect();
    assert_eq!(removed, [deleted.to_string().as_str()]);
    assert!(all.pages.iter().any(|p| p.messages.iter().any(|m| m.server.id == trashed.to_string() && m.server.folder == "3")));
}

#[test]
fn folders_made_between_delta_pages_are_each_listed_and_kept() {
    let server = StandIn::new();
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    // Two folders made in webmail while the delta is being read: the first lands on this round's
    // page, the second on the next.
    let first_folder = server.add_folder_later(2, "Mid-run one");
    let second_folder = server.add_folder_later(2, "Mid-run two");
    let in_first = server.deliver(first_folder, "Into one", "u");
    // Enough changes for a second round (the delta page is 500).
    for n in 0..510 {
        server.deliver(2, &format!("Filler {n}"), "");
    }
    let in_second = server.deliver(second_folder, "Into two", "");
    let all = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    assert!(all.whole && !all.token.is_empty());
    let ids: Vec<&str> = all.pages.iter().flat_map(|p| p.messages.iter().map(|m| m.server.id.as_str())).collect();
    assert!(ids.contains(&in_first.to_string().as_str()) && ids.contains(&in_second.to_string().as_str()));
    assert!(!all.pages.iter().any(|p| p.removed.contains(&in_first.to_string()) || p.removed.contains(&in_second.to_string())));
}

#[test]
fn folders_made_faster_than_relists_keep_the_last_whole_pages_token() {
    let server = StandIn::new();
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    // Three folders made in webmail while the delta is being read, a message moved into each —
    // one on every page. A run asks for the listing again at most MAX_RELISTS times (2 in tests):
    // past it the run keeps the token of the last whole page instead of failing from the start,
    // and the run after finishes the delta.
    let one = server.add_folder_later(2, "Mid-run 1");
    let two = server.add_folder_later(2, "Mid-run 2");
    let three = server.add_folder_later(2, "Mid-run 3");
    let in_one = server.deliver(one, "Into 1", "");
    for n in 0..505 {
        server.deliver(2, &format!("Filler {n}"), "");
    }
    let in_two = server.deliver(two, "Into 2", "");
    for n in 0..505 {
        server.deliver(2, &format!("Filler b {n}"), "");
    }
    let in_three = server.deliver(three, "Into 3", "");
    let first = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(first.errors, Vec::<String>::new());
    assert!(first.whole && !first.token.is_empty() && first.token != token);
    let ids: Vec<String> = first.pages.iter().flat_map(|p| p.messages.iter().map(|m| m.server.id.clone())).collect();
    assert!(ids.contains(&in_one.to_string()) && ids.contains(&in_two.to_string()));
    assert!(!ids.contains(&in_three.to_string()));
    let second = whole(&run(&server, &request(&first.token), PASSWORD));
    assert_eq!(second.errors, Vec::<String>::new());
    assert!(second.whole && second.token != first.token);
    let ids: Vec<String> = second.pages.iter().flat_map(|p| p.messages.iter().map(|m| m.server.id.clone())).collect();
    assert!(ids.contains(&in_three.to_string()));
}

#[test]
fn a_folder_too_large_for_one_run_leaves_where_it_stopped() {
    let server = StandIn::new();
    // More than MAX_FOLDER_PAGES pages (4 pages of 250 in tests): the read stops with a cursor to
    // go on from, and the run after it finishes the folder rather than failing from its start.
    for n in 0..1_100 {
        server.deliver(2, &format!("Message {n}"), "");
    }
    let first = whole(&run(&server, &request(""), PASSWORD));
    assert_eq!(first.errors, Vec::<String>::new());
    let resume = first.resume.clone().unwrap_or_default();
    // The folder's own entry, and one for every folder the run never reached.
    let stopped = resume.iter().filter(|e| !e.cursor.is_empty()).collect::<Vec<_>>();
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].folder, "2");
    assert_eq!(first.pages.iter().filter(|p| p.folder == "2").count(), 4);
    // The run after goes on from the cursor: the folder's end comes without another fresh read.
    let mut again = request(&first.token);
    again.resume = resume;
    let second = whole(&run(&server, &again, PASSWORD));
    assert_eq!(second.errors, Vec::<String>::new());
    assert_eq!(second.resume, Some(Vec::new()));
    let pages: Vec<_> = second.pages.iter().filter(|p| p.folder == "2").collect();
    assert!(pages.last().unwrap().last);
    let read: std::collections::HashSet<&str> = first
        .pages
        .iter()
        .chain(second.pages.iter())
        .filter(|p| p.folder == "2")
        .flat_map(|p| p.messages.iter().map(|m| m.server.id.as_str()))
        .collect();
    assert_eq!(read.len(), 1_100);
}

#[test]
fn a_delta_longer_than_a_run_goes_on_from_the_last_whole_page() {
    let server = StandIn::new();
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    // More than MAX_ROUNDS delta pages (3 of 500 in tests): the run keeps the token of the last
    // whole page instead of ending nowhere.
    for n in 0..1_600 {
        server.deliver(2, &format!("Message {n}"), "");
    }
    let first = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(first.errors, Vec::<String>::new());
    assert!(first.whole && !first.token.is_empty() && first.token != token);
    let seen = first.pages.iter().map(|p| p.messages.len()).sum::<usize>();
    assert_eq!(seen, 1_500);
    let second = whole(&run(&server, &request(&first.token), PASSWORD));
    assert_eq!(second.errors, Vec::<String>::new());
    assert_eq!(second.pages.iter().map(|p| p.messages.len()).sum::<usize>(), 100);
}

#[test]
fn a_folder_left_half_read_starts_over_without_a_token() {
    let server = StandIn::new();
    let older = server.deliver(2, "Older", "");
    let newer = server.deliver(2, "Newer", "");
    let sort = server.state.borrow().items.get(&newer).unwrap().date;
    // A cursor made under a token the request can't name answers for nothing: what changed
    // before it is no longer covered by a delta, so the folder is read from its start again.
    let mut req = request("");
    req.resume = vec![mailcraft_model::sync::ZimbraResume { folder: "2".into(), cursor: newer.to_string(), sort }];
    let all = whole(&run(&server, &req, PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    let folder = all.pages.iter().filter(|p| p.folder == "2").collect::<Vec<_>>();
    assert_eq!(folder.len(), 1);
    assert!(folder[0].afresh && folder[0].last);
    assert_eq!(folder[0].messages.iter().map(|m| m.server.id.as_str()).collect::<Vec<_>>(), [newer.to_string().as_str(), older.to_string().as_str()]);
    assert_eq!(all.resume, Some(Vec::new()));
}

#[test]
fn a_run_on_a_token_finishes_a_folder_left_half_read() {
    let server = StandIn::new();
    let older = server.deliver(2, "Older", "");
    let newer = server.deliver(2, "Newer", "");
    let sort = server.state.borrow().items.get(&newer).unwrap().date;
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    server.deliver(2, "Fresh", "");
    let mut req = request(&token);
    req.resume = vec![mailcraft_model::sync::ZimbraResume { folder: "2".into(), cursor: newer.to_string(), sort }];
    let all = whole(&run(&server, &req, PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    let folder = all.pages.iter().filter(|p| p.folder == "2").collect::<Vec<_>>();
    assert_eq!(folder.len(), 1);
    assert!(!folder[0].afresh && folder[0].last);
    assert_eq!(folder[0].messages.iter().map(|m| m.server.id.as_str()).collect::<Vec<_>>(), [older.to_string().as_str()]);
    assert_eq!(all.resume, Some(Vec::new()));
}

#[test]
fn a_fetch_stops_taking_bodies_once_the_total_is_full() {
    let server = StandIn::new();
    let mut ids = Vec::new();
    for subject in ["One", "Two", "Three", "Four", "Five"] {
        let id = server.deliver(2, subject, "");
        server.change(id, |m| m.mime = "x".repeat(16 * 1024 * 1024));
        ids.push(id.to_string());
    }
    let fetch = ZimbraFetchRequest { account: 3, email: USER.into(), url: URL.into(), ids, ..Default::default() };
    let got = block_on(fetch_messages(&server, &fetch, &Secret::new(PASSWORD), &AtomicBool::new(false))).unwrap();
    assert_eq!(got.errors, Vec::<String>::new());
    // 16 MiB at a time: the fifth would take the run past 64 MiB and is left for the next fetch.
    assert_eq!(got.fetched.len(), 4);
}

#[test]
fn a_token_the_server_lost_reads_the_mailbox_afresh() {
    let server = StandIn::new();
    server.deliver(2, "Here", "");
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    server.deliver(2, "Later", "");
    server.state.borrow_mut().oldest = 1_000;
    let all = whole(&run(&server, &request(&token), PASSWORD));
    assert_eq!(all.errors, Vec::<String>::new());
    assert!(all.whole && all.pages.iter().any(|p| p.folder == "2" && p.afresh));
    assert_eq!(all.from, token);
}

#[test]
fn a_refused_sign_in_is_said_and_nothing_else_is_asked() {
    let server = StandIn::new();
    let pieces = run(&server, &request(""), "wrong");
    assert_eq!(pieces.len(), 1);
    let last = &pieces[0];
    assert!(last.auth_refused && !last.whole && last.token.is_empty() && last.listing.is_none());
    assert!(last.errors[0].contains("turned the sign-in down"));
    // What the server wrote (its fault's reason echoes the password here) never reaches a line.
    assert!(!format!("{pieces:?}{}", last.errors.join(" ")).contains(PASSWORD));
    assert_eq!(server.state.borrow().log, ["AuthRequest"]);
}

#[test]
fn a_run_called_off_says_so_and_keeps_no_token() {
    let server = StandIn::new();
    let mut pieces = Vec::new();
    block_on(sync_mail(&server, &request(""), &Secret::new(PASSWORD), &AtomicBool::new(true), &mut |p| pieces.push(p))).unwrap();
    assert_eq!(pieces.len(), 1);
    assert!(pieces[0].stopped && pieces[0].token.is_empty() && pieces[0].errors.is_empty());
}

#[test]
fn messages_are_fetched_whole_with_the_token_in_a_cookie() {
    let server = StandIn::new();
    let one = server.deliver(2, "Whole", "");
    let fetch = ZimbraFetchRequest {
        account: 3,
        email: USER.into(),
        url: URL.into(),
        ids: vec![one.to_string(), "999".into(), "x1".into()],
        ..Default::default()
    };
    let got = block_on(fetch_messages(&server, &fetch, &Secret::new(PASSWORD), &AtomicBool::new(false))).unwrap();
    assert_eq!(got.errors, Vec::<String>::new());
    assert_eq!(got.fetched.len(), 1);
    assert!(String::from_utf8_lossy(&got.fetched[0].mime).contains("Subject: Whole"));
    assert_eq!(got.missing, ["999", "x1"]);
}

#[test]
fn the_token_goes_in_the_header_or_a_cookie_and_never_in_an_address() {
    let token = Secret::new(TOKEN);
    for request in [
        crate::folders::request(URL, &token).unwrap(),
        crate::delta::request(URL, &token, "12").unwrap(),
        crate::items::by_id(URL, &token, &["1".into()]).unwrap(),
        crate::fetch::request(URL, &token, "257").unwrap(),
    ] {
        assert!(!request.url().contains(TOKEN), "{}", request.url());
        let in_body = String::from_utf8_lossy(request.body()).contains(TOKEN);
        let in_cookie = request.field("Cookie").is_some_and(|c| c.contains(TOKEN));
        assert!(in_body != in_cookie, "one place only: {request:?}");
        assert!(!format!("{request:?}").contains(TOKEN));
    }
    let auth = crate::auth::request(URL, "ada\r\nX: y", &Secret::new(PASSWORD)).unwrap();
    assert!(!String::from_utf8_lossy(auth.body()).contains('\r'));
    assert!(crate::auth::request("http://zimbra.example.org", "ada", &Secret::new(PASSWORD)).is_err());
    assert!(crate::fetch::request(URL, &token, "1&zauthtoken=x").is_err());
}

#[test]
fn a_server_that_answers_the_same_page_for_ever_is_cut_off() {
    // `more` every time but the cursor it was asked from back again: no page goes on from there,
    // so the run fails the read rather than asking the same page for ever.
    let server = StandIn::new();
    server.deliver(2, "One", "");
    server.stuck.set(true);
    let all = whole(&run(&server, &request(""), PASSWORD));
    assert!(!all.whole && !all.errors.is_empty() && all.token.is_empty());
}

/// A server whose answer of one operation is replaced before it is read: `operation` names the
/// SOAP answer (`SearchResponse`, `SyncResponse`, `GetFolderResponse`); for the folder listing the
/// first folder's children take the replacement, for the others the `m` list.
struct Altered {
    server: StandIn,
    operation: &'static str,
    replacement: serde_json::Value,
}

impl crate::http::HttpExchange for Altered {
    async fn exchange(&self, request: &crate::http::HttpRequest) -> Result<crate::http::HttpResponse, crate::http::TransportError> {
        let response = self.server.exchange(request).await?;
        let mut body: serde_json::Value = serde_json::from_slice(response.body()).unwrap_or_default();
        if let Some(answer) = body.pointer_mut(&format!("/Body/{}", self.operation)) {
            if self.operation == "GetFolderResponse" {
                answer["folder"][0]["folder"] = self.replacement.clone();
            } else {
                answer["m"] = self.replacement.clone();
            }
        }
        Ok(crate::http::HttpResponse::new(response.status(), body.to_string().into_bytes()))
    }
}

fn collect(server: &impl crate::http::HttpExchange, token: &str) -> Vec<ZimbraMailResult> {
    let mut pieces = Vec::new();
    block_on(sync_mail(server, &request(token), &Secret::new(PASSWORD), &AtomicBool::new(false), &mut |p| pieces.push(p))).unwrap();
    pieces
}

#[test]
fn a_page_of_no_list_shape_completes_no_fresh_sweep() {
    // `m` as a string where the page's messages belong: nothing of the folder may come out read.
    let server = StandIn::new();
    server.deliver(2, "Still on server", "");
    let broken = Altered { server, operation: "SearchResponse", replacement: serde_json::json!("unreadable") };
    let all = whole(&collect(&broken, ""));
    assert!(!all.whole && all.token.is_empty(), "{all:?}");
}

#[test]
fn a_delta_of_no_list_shape_moves_no_token() {
    // `m` as a string where the page's changes belong: nothing may move past them.
    let server = StandIn::new();
    let token = whole(&run(&server, &request(""), PASSWORD)).token;
    server.deliver(2, "Arrived after the token", "");
    let broken = Altered { server, operation: "SyncResponse", replacement: serde_json::json!("unreadable") };
    let all = whole(&collect(&broken, &token));
    assert!(all.token.is_empty(), "{all:?}");
}

#[test]
fn folder_children_of_no_list_shape_make_no_complete_listing() {
    // A folder's `folder` field as a string: the tree is only half-read, and an empty listing it
    // emits may never be the complete one a sweep is trusted with.
    let server = StandIn::new();
    server.deliver(2, "Still on server", "");
    let broken = Altered { server, operation: "GetFolderResponse", replacement: serde_json::json!("unreadable") };
    let pieces = collect(&broken, "");
    assert!(
        !pieces.iter().any(|p| p.listing.as_ref().is_some_and(|l| l.complete && l.folders.is_empty())),
        "an unreadable tree passed for a complete listing"
    );
}

#[test]
fn an_invalid_deleted_id_becomes_no_others_id() {
    // A NUL inside a deleted id: taken out it would name another message — the raw id is checked,
    // and the answer refused.
    let response = crate::http::HttpResponse::new(
        200,
        serde_json::json!({"Body":{"SyncResponse":{"token":"42","deleted":[{"m":[{"ids":"3\u{0000}00"}]}]}}}).to_string().into_bytes(),
    );
    assert!(crate::delta::answer(&response, false).is_err());
}

#[test]
fn a_page_cursor_sends_the_documents_sort_val() {
    let request = crate::items::folder_page(URL, &Secret::new("token"), "2", Some(("300", 1_700_000_000_000))).unwrap();
    let body: serde_json::Value = serde_json::from_slice(request.body()).unwrap_or_default();
    assert_eq!(body.pointer("/Body/SearchRequest/cursor/sortVal"), Some(&serde_json::json!("1700000000000")));
}

#[test]
fn a_page_cursor_reads_the_hits_sort_field() {
    // `sf`, not `d`, is what a `sortVal` continues from: a hit with `sf` and no `d` still carries
    // a cursor.
    let response = crate::http::HttpResponse::new(
        200,
        serde_json::json!({"Body":{"SearchResponse":{"more":true,"m":[{"id":"300","l":"2","sf":"1700000000000"}]}}}).to_string().into_bytes(),
    );
    assert_eq!(crate::items::answer(&response).ok().and_then(|p| p.cursor), Some(("300".into(), 1_700_000_000_000)));
}

#[test]
fn a_resume_survives_the_cursor_items_deletion() {
    let server = StandIn::new();
    // More than the test page cap: the run stops with a cursor to go on from.
    for n in 0..1_100 {
        server.deliver(2, &format!("Message {n}"), "");
    }
    let first = whole(&run(&server, &request(""), PASSWORD));
    assert_eq!(first.errors, Vec::<String>::new());
    let resume = first.resume.clone().unwrap_or_default();
    let stopped = resume.iter().find(|e| !e.cursor.is_empty()).unwrap();
    // The item the cursor names is deleted in webmail between the two runs: Zimbra goes on from
    // the cursor's sort value, and so does the stand-in — the folder's tail is still read.
    server.delete(stopped.cursor.parse::<u32>().unwrap());
    let mut again = request(&first.token);
    again.resume = resume;
    let second = whole(&run(&server, &again, PASSWORD));
    assert_eq!(second.errors, Vec::<String>::new());
    assert_eq!(second.resume, Some(Vec::new()));
    let read: usize = second.pages.iter().filter(|p| p.folder == "2").map(|p| p.messages.len()).sum();
    assert_eq!(read, 100);
}
