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
