// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A run over the stand-in and over scripted servers: what is asked of the server and in how many
//! requests, the pieces it comes in, how it ends, and what a hostile server makes of it. The reading of a
//! mailbox list and of an email has tests of its own, beside each (`folders`, `message`).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::sync::JmapKnown;
use mailcraft_model::{Address, FlagStatus};
use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::connection::tests::any_json;
use crate::http::API_RESPONSE_LIMIT;
use crate::standin::{Profile, SESSION, StandIn, TOKEN};
use crate::test_support::made_secret;
use crate::{CAPABILITY_CORE as CORE, CAPABILITY_MAIL as MAIL, Credentials, HttpRequest, HttpResponse, Method, RetryAfter, block_on};

const ME: &str = "ada@example.org";
const ID: u64 = 3;

/// A stand-in with the mailboxes a person has: the Inbox and its fellows by role, and a folder of their own
/// with one inside it. None has mail yet.
struct Ids {
    inbox: String,
    drafts: String,
    sent: String,
    trash: String,
    projects: String,
    apollo: String,
}

fn world() -> (StandIn, Ids) {
    let stand_in = StandIn::new();
    let ids = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        let drafts = d.mailbox("Drafts", None, Some("drafts"));
        let sent = d.mailbox("Sent", None, Some("sent"));
        let trash = d.mailbox("Trash", None, Some("trash"));
        let projects = d.mailbox("Projects", None, None);
        let apollo = d.mailbox("Apollo", Some(&projects), None);
        Ids { inbox, drafts, sent, trash, projects, apollo }
    });
    (stand_in, ids)
}

/// A message as a mail program writes one, sent at 07:30 UTC on 15 October 2026 by a person in the zone two
/// hours ahead.
fn mail(subject: &str) -> Vec<u8> {
    format!(
        "From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: {subject}\r\nMessage-ID: <{subject}@example.org>\r\nDate: Thu, 15 Oct 2026 09:30:00 +0200\r\n\r\nHello about {subject}.\r\n"
    )
    .into_bytes()
}

/// When the email numbered `n` arrives: later for a bigger `n`.
fn at(n: usize) -> String {
    format!("2026-01-01T{:02}:{:02}:{:02}Z", n / 3600, (n % 3600) / 60, n % 60)
}

/// An email in `mailbox` that arrives at the time `n` stands for: its id.
fn deliver(stand_in: &StandIn, mailbox: &str, subject: &str, keywords: &[&str], n: usize) -> String {
    stand_in.with(|d| d.deliver(mailbox, &mail(subject), keywords, &at(n)))
}

fn request(window: u32, scope: JmapScope, list_folders: bool) -> JmapMailRequest {
    JmapMailRequest { account: ID, email: ME.into(), session: SESSION.into(), bearer: true, window, list_folders, scope, ..Default::default() }
}

/// A run that reads every mailbox, the list of them first.
fn everything(window: u32) -> JmapMailRequest {
    request(window, JmapScope::All, true)
}

/// A run that reads the Inbox and the mailboxes named, with no list.
fn light(window: u32, also: &[&str]) -> JmapMailRequest {
    request(window, JmapScope::Light { also: also.iter().map(|id| id.to_string()).collect() }, false)
}

/// What the engine has of `mailbox`: these ids.
fn has(request: &mut JmapMailRequest, mailbox: &str, ids: &[&str]) {
    request.known.push(JmapKnown { mailbox: mailbox.into(), ids: ids.iter().map(|id| id.to_string()).collect() });
}

/// Every piece of a run over `http`, signed in with the stand-in's token.
fn run_over<H: HttpExchange>(http: H, session: &str, request: &JmapMailRequest, cancel: &AtomicBool) -> Vec<JmapMailResult> {
    let client = block_on(Connection::connect(http, session, Credentials::bearer(TOKEN).unwrap())).unwrap();
    let mut pieces = Vec::new();
    block_on(sync_mail(&client, request, cancel, &mut |piece| pieces.push(piece)));
    well_formed(&pieces);
    pieces
}

fn run(stand_in: &StandIn, request: &JmapMailRequest) -> Vec<JmapMailResult> {
    run_over(stand_in.clone(), SESSION, request, &AtomicBool::new(false))
}

/// What every run is like: pieces for the account, all `partial` but the last, which has no page and no
/// listing; the listing, if there is one, first.
fn well_formed(pieces: &[JmapMailResult]) {
    let (last, rest) = pieces.split_last().expect("a run ends with a piece");
    assert!(!last.partial && last.pages.is_empty() && last.listing.is_none() && last.progress.is_none(), "{last:?}");
    assert!(rest.iter().all(|piece| piece.partial && piece.errors.is_empty() && !piece.whole && !piece.stopped), "{rest:?}");
    assert!(pieces.iter().all(|piece| (piece.account, piece.email.as_str(), piece.session.as_str()) == (ID, ME, SESSION)));
    assert!(pieces.iter().skip(1).all(|piece| piece.listing.is_none()), "the listing comes first");
    // A piece is the list, or one window with how far the run has got.
    for piece in rest {
        match (&piece.listing, piece.pages.len()) {
            (Some(_), 0) => assert!(piece.progress.is_none()),
            (None, 1) => assert!(piece.progress.is_some()),
            _ => panic!("a piece that is neither the list nor a window: {piece:?}"),
        }
    }
    assert!(!(last.whole && (last.stopped || last.auth_refused || last.busy_for.is_some() || !last.errors.is_empty())), "{last:?}");
}

/// Whether `text` is an id a server may hand out.
fn is_id(text: &str) -> bool {
    Id::new(text).is_ok()
}

fn last(pieces: &[JmapMailResult]) -> &JmapMailResult {
    pieces.last().expect("a run ends with a piece")
}

fn pages(pieces: &[JmapMailResult]) -> Vec<&MessagePage> {
    pieces.iter().flat_map(|piece| piece.pages.iter()).collect()
}

fn page_of<'a>(pieces: &'a [JmapMailResult], mailbox: &str) -> &'a MessagePage {
    pages(pieces).into_iter().find(|page| page.folder == mailbox).unwrap_or_else(|| panic!("no window of {mailbox}"))
}

fn subjects(page: &MessagePage) -> Vec<&str> {
    page.messages.iter().map(|m| m.subject.as_str()).collect()
}

fn ids_of(page: &MessagePage) -> Vec<&str> {
    page.messages.iter().map(|m| m.server.id.as_str()).collect()
}

fn folders_read(pieces: &[JmapMailResult]) -> Vec<&str> {
    pages(pieces).iter().map(|page| page.folder.as_str()).collect()
}

/// The time an ISO date names.
fn time(iso: &str) -> i64 {
    mailcraft_time::iso_to_timestamp(iso, 0).unwrap()
}

#[test]
fn a_first_run_lists_the_mailboxes_and_reads_each_window_whole() {
    let (stand_in, ids) = world();
    let first = deliver(&stand_in, &ids.inbox, "first", &["$seen"], 1);
    let second = deliver(&stand_in, &ids.inbox, "second", &[], 2);
    let third = deliver(&stand_in, &ids.inbox, "third", &["$flagged", "$answered"], 3);
    let outgoing = deliver(&stand_in, &ids.sent, "outgoing", &["$seen", "$forwarded"], 4);
    let draft = deliver(&stand_in, &ids.drafts, "draft", &["$draft", "$seen"], 5);
    let nested = deliver(&stand_in, &ids.apollo, "nested", &[], 6);
    let pieces = run(&stand_in, &everything(500));

    // The pieces: the list, the windows (the Inbox, the special mailboxes, the rest), the end.
    assert_eq!(pieces.len(), 8);
    assert_eq!(folders_read(&pieces), [&ids.inbox, &ids.drafts, &ids.sent, &ids.trash, &ids.projects, &ids.apollo]);
    let listing = pieces[0].listing.as_ref().unwrap();
    assert!(listing.complete);
    let tree: Vec<(&str, &str, FolderKind, &str)> =
        listing.folders.iter().map(|f| (f.name.as_str(), f.id.as_str(), f.kind, f.parent.as_str())).collect();
    assert_eq!(
        tree,
        [
            ("Inbox", ids.inbox.as_str(), FolderKind::Inbox, ""),
            ("Drafts", ids.drafts.as_str(), FolderKind::Drafts, ""),
            ("Sent", ids.sent.as_str(), FolderKind::Sent, ""),
            ("Trash", ids.trash.as_str(), FolderKind::Deleted, ""),
            ("Projects", ids.projects.as_str(), FolderKind::Custom, ""),
            ("Apollo", ids.apollo.as_str(), FolderKind::Custom, ids.projects.as_str()),
        ]
    );
    assert_eq!((listing.folders[0].total, listing.folders[0].unread), (3, 2));

    // The Inbox: newest first, every message whole, the keywords as marks.
    let inbox = page_of(&pieces, &ids.inbox);
    assert!(inbox.afresh && inbox.removed.is_empty() && inbox.link.is_empty());
    assert_eq!(subjects(inbox), ["third", "second", "first"]);
    assert_eq!(ids_of(inbox), [third.as_str(), second.as_str(), first.as_str()]);
    let m = &inbox.messages[0];
    assert_eq!((m.message_id.as_str(), m.from.name.as_str(), m.from.email.as_str()), ("third@example.org", "Zoe Park", "zoe@example.org"));
    assert_eq!((m.to.len(), m.to[0].name.as_str(), m.to[0].email.as_str()), (1, "Ada", ME));
    assert_eq!((m.sent, m.received), (time("2026-10-15T07:30:00Z"), time("2026-01-01T00:00:03Z")));
    assert_eq!((m.preview.as_str(), m.has_attachments, m.draft), ("Hello about third.", false, false));
    assert_eq!(m.server.size, u64::try_from(mail("third").len()).unwrap());
    assert_eq!(
        (m.server.folder.as_str(), m.server.read, m.server.flag, m.server.answered, m.server.forwarded),
        (ids.inbox.as_str(), false, FlagStatus::Flagged, true, false)
    );
    assert_eq!((inbox.messages[1].server.read, inbox.messages[1].server.flag), (false, FlagStatus::None));
    assert!(inbox.messages[2].server.read);
    // The others: their own marks and their own mailbox.
    let sent = &page_of(&pieces, &ids.sent).messages[0];
    assert_eq!(
        (sent.server.id.as_str(), sent.server.read, sent.server.forwarded, sent.server.folder.as_str()),
        (outgoing.as_str(), true, true, ids.sent.as_str())
    );
    let drafted = &page_of(&pieces, &ids.drafts).messages[0];
    assert_eq!((drafted.server.id.as_str(), drafted.draft, drafted.server.read), (draft.as_str(), true, true));
    assert_eq!(ids_of(page_of(&pieces, &ids.apollo)), [nested.as_str()]);
    // Mailboxes with no mail are complete listings of nothing.
    assert!(page_of(&pieces, &ids.trash).messages.is_empty() && page_of(&pieces, &ids.trash).afresh);

    // How far it got, mailbox by mailbox.
    let progress: Vec<(u32, u32, u32, u32)> =
        pieces.iter().filter_map(|p| p.progress.as_ref()).map(|p| (p.read, p.total, p.number, p.folders)).collect();
    assert_eq!(progress, [(3, 3, 1, 6), (1, 1, 2, 6), (1, 1, 3, 6), (0, 0, 4, 6), (0, 0, 5, 6), (1, 1, 6, 6)]);
    assert_eq!(pieces[1].progress.as_ref().map(|p| p.folder.as_str()), Some(ids.inbox.as_str()));

    // It ended whole.
    let end = last(&pieces);
    assert!(end.whole && !end.stopped && !end.auth_refused && end.busy_for.is_none() && end.errors.is_empty(), "{end:?}");

    // The work: the list; for each mailbox one request of two calls, and one for the emails it had to read whole.
    let window = ["Email/query", "Email/get"];
    let mut calls = vec!["Mailbox/get"];
    for whole in [true, true, true, false, false, true] {
        calls.extend(window);
        if whole {
            calls.push("Email/get");
        }
    }
    assert_eq!(stand_in.calls(), calls);
    assert_eq!(stand_in.api_requests(), 1 + 6 + 4);
}

#[test]
fn the_second_run_reads_the_keywords_of_what_the_engine_has_and_the_headers_of_what_it_lacks() {
    let (stand_in, ids) = world();
    let first = deliver(&stand_in, &ids.inbox, "first", &["$seen"], 1);
    let second = deliver(&stand_in, &ids.inbox, "second", &[], 2);
    let third = deliver(&stand_in, &ids.inbox, "third", &[], 3);
    // Another client reads one, flags one, and deletes one; another message comes.
    stand_in.with(|d| {
        d.set_keyword(&second, "$seen", true);
        d.set_keyword(&third, "$flagged", true);
        d.remove(&first);
    });
    let fourth = deliver(&stand_in, &ids.inbox, "fourth", &[], 4);
    stand_in.with(|d| d.seen.clear());
    let mut again = light(500, &[]);
    has(&mut again, &ids.inbox, &[&third, &second, &first]);
    let pieces = run(&stand_in, &again);

    // Only the Inbox (a run without a list, and none named); the list is asked for all the same.
    assert_eq!(pieces.len(), 2);
    assert!(pieces[0].listing.is_none());
    let inbox = page_of(&pieces, &ids.inbox);
    assert!(inbox.afresh, "three emails are the whole mailbox: the one that went is not in it");
    assert_eq!(ids_of(inbox), [fourth.as_str(), third.as_str(), second.as_str()]);
    // The new one whole; the ones the engine has with their marks alone.
    assert_eq!((inbox.messages[0].subject.as_str(), inbox.messages[0].preview.as_str()), ("fourth", "Hello about fourth."));
    for known in &inbox.messages[1..] {
        let blank = GraphMessage { server: known.server.clone(), ..Default::default() };
        assert_eq!(known, &blank, "nothing but the marks");
        assert_eq!(known.server.folder, ids.inbox);
    }
    assert_eq!((inbox.messages[1].server.read, inbox.messages[1].server.flag), (false, FlagStatus::Flagged));
    assert_eq!((inbox.messages[2].server.read, inbox.messages[2].server.flag), (true, FlagStatus::None));

    // Asked: the list, the window with the keywords only, and the headers of the one email it lacked.
    let asked = stand_in.asked();
    let shape: Vec<(&str, Option<usize>, Vec<&str>)> =
        asked.iter().map(|a| (a.method.as_str(), a.ids, a.properties.iter().map(String::as_str).collect())).collect();
    assert_eq!(
        shape,
        [
            ("Mailbox/get", None, MAILBOX_PROPERTIES.to_vec()),
            ("Email/query", None, vec![]),
            ("Email/get", Some(3), WINDOW_PROPERTIES.to_vec()),
            ("Email/get", Some(1), EMAIL_PROPERTIES.to_vec()),
        ]
    );
    assert_eq!((asked[1].mailbox.as_deref(), asked[1].limit), (Some(ids.inbox.as_str()), Some(500)));
    assert!(EMAIL_PROPERTIES.contains(&"headers") && !WINDOW_PROPERTIES.contains(&"headers"));

    // With everything known the headers are not asked for at all.
    let mut all_known = light(500, &[]);
    has(&mut all_known, &ids.inbox, &[&fourth, &third, &second]);
    stand_in.with(|d| d.seen.clear());
    let pieces = run(&stand_in, &all_known);
    assert_eq!(stand_in.calls(), ["Mailbox/get", "Email/query", "Email/get"]);
    assert_eq!(page_of(&pieces, &ids.inbox).messages.len(), 3);
    // Known in another mailbox is not known in this one.
    let mut elsewhere = light(500, &[]);
    has(&mut elsewhere, &ids.sent, &[&fourth, &third, &second]);
    stand_in.with(|d| d.seen.clear());
    let pieces = run(&stand_in, &elsewhere);
    assert_eq!(stand_in.calls(), ["Mailbox/get", "Email/query", "Email/get", "Email/get"]);
    assert!(page_of(&pieces, &ids.inbox).messages.iter().all(|m| !m.subject.is_empty()));
}

#[test]
fn a_window_is_one_request_of_two_calls_and_what_it_lacks_comes_200_at_a_time() {
    let (stand_in, ids) = world();
    for n in 0..450 {
        deliver(&stand_in, &ids.inbox, &format!("m{n}"), &[], n);
    }
    stand_in.with(|d| d.seen.clear());
    let pieces = run(&stand_in, &light(500, &[]));
    let inbox = page_of(&pieces, &ids.inbox);
    assert_eq!(inbox.messages.len(), 450);
    assert!(inbox.afresh, "450 are the whole mailbox");
    assert_eq!((inbox.messages[0].subject.as_str(), inbox.messages[449].subject.as_str()), ("m449", "m0"));
    // The list, the window (one request, two calls), then 200, 200 and 50 emails whole.
    assert_eq!(stand_in.calls(), ["Mailbox/get", "Email/query", "Email/get", "Email/get", "Email/get", "Email/get"]);
    assert_eq!(stand_in.api_requests(), 5);
    let gets: Vec<Option<usize>> = stand_in.asked().iter().filter(|a| a.method == "Email/get").map(|a| a.ids).collect();
    assert_eq!(gets, [Some(450), Some(200), Some(200), Some(50)]);
    let seen = stand_in.seen();
    // (After the session, which has no calls.)
    assert_eq!(seen.iter().map(|s| s.calls.len()).collect::<Vec<_>>(), [0, 1, 2, 1, 1, 1]);
    assert_eq!(last(&pieces).errors, Vec::<String>::new());
}

#[test]
fn a_bigger_mailbox_than_the_window_is_read_in_part_the_biggest_window_at_500() {
    let (stand_in, ids) = world();
    for n in 0..1200 {
        deliver(&stand_in, &ids.inbox, &format!("m{n}"), &[], n);
    }
    stand_in.with(|d| d.seen.clear());
    // The window a request asks for is held to 500, and 0 asks for the biggest.
    for (asked, applied) in [(0, 500), (10_000, 500), (500, 500), (2, 2)] {
        stand_in.with(|d| d.seen.clear());
        let pieces = run(&stand_in, &light(asked, &[]));
        let inbox = page_of(&pieces, &ids.inbox);
        assert_eq!(inbox.messages.len(), applied, "window {asked}");
        assert_eq!(inbox.messages[0].subject, "m1199");
        assert!(!inbox.afresh, "1200 are more than {applied}: nothing is removed for being missing");
        let query = stand_in.asked().into_iter().find(|a| a.method == "Email/query").unwrap();
        assert_eq!(query.limit, Some(applied));
        let progress = pieces.iter().find_map(|p| p.progress.as_ref()).unwrap();
        assert_eq!((progress.read, progress.total), (u32::try_from(applied).unwrap(), 1200));
        assert!(last(&pieces).whole, "a window read in part is a run that went well");
    }
    // 500 in the window (the first round asks for the keywords of all 500), whole 200 at a time.
    stand_in.with(|d| d.seen.clear());
    run(&stand_in, &light(500, &[]));
    let gets: Vec<Option<usize>> = stand_in.asked().iter().filter(|a| a.method == "Email/get").map(|a| a.ids).collect();
    assert_eq!(gets, [Some(500), Some(200), Some(200), Some(100)]);
}

#[test]
fn what_the_server_takes_in_a_get_holds_the_window_and_the_chunks() {
    let (stand_in, ids) = world();
    for n in 0..250 {
        deliver(&stand_in, &ids.inbox, &format!("m{n}"), &[], n);
    }
    stand_in.with(|d| (d.max_get, d.seen) = (60, Vec::new()));
    let pieces = run(&stand_in, &light(500, &[]));
    let inbox = page_of(&pieces, &ids.inbox);
    assert_eq!((inbox.messages.len(), inbox.afresh), (60, false));
    let gets: Vec<Option<usize>> = stand_in.asked().iter().filter(|a| a.method == "Email/get").map(|a| a.ids).collect();
    assert_eq!(gets, [Some(60), Some(60)]);
    assert_eq!(stand_in.asked().iter().find(|a| a.method == "Email/query").and_then(|a| a.limit), Some(60));
    assert!(last(&pieces).errors.is_empty());
}

#[test]
fn a_server_that_takes_one_call_to_a_request_is_asked_in_two() {
    let (stand_in, ids) = world();
    deliver(&stand_in, &ids.inbox, "only", &["$seen"], 1);
    stand_in.with(|d| (d.max_calls, d.seen) = (1, Vec::new()));
    let pieces = run(&stand_in, &light(500, &[]));
    assert_eq!(stand_in.calls(), ["Mailbox/get", "Email/query", "Email/get", "Email/get"]);
    assert_eq!(stand_in.api_requests(), 4);
    let inbox = page_of(&pieces, &ids.inbox);
    assert_eq!((subjects(inbox), inbox.messages[0].server.read, inbox.afresh), (vec!["only"], true, true));
    assert!(last(&pieces).whole);
}

#[test]
fn a_window_is_complete_when_the_server_had_no_more_to_list() {
    let (stand_in, ids) = world();
    for n in 0..4 {
        deliver(&stand_in, &ids.inbox, &format!("m{n}"), &[], n);
    }
    let complete = |window: u32| page_of(&run(&stand_in, &light(window, &[])), &ids.inbox).afresh;
    // Four emails: a window of five holds them all, a window of four is just them, one of three is not.
    assert!(complete(5) && complete(4) && !complete(3) && !complete(1));
    stand_in.with(|d| d.remove(&d.emails[0].id.clone()));
    assert!(complete(4), "now a window of four holds more than is there");
    assert!(complete(500));
}

#[test]
fn an_email_in_several_mailboxes_is_listed_by_the_window_of_the_smallest_id() {
    let (stand_in, ids) = world();
    let both = deliver(&stand_in, &ids.projects, "shared", &[], 1);
    assert!(stand_in.with(|d| d.add_to_mailbox(&both, &ids.apollo)));
    let alone = deliver(&stand_in, &ids.apollo, "alone", &[], 2);
    // (M5 is smaller than M6.)
    assert!(ids.projects < ids.apollo);
    let pieces = run(&stand_in, &everything(500));
    assert_eq!(ids_of(page_of(&pieces, &ids.projects)), [both.as_str()]);
    assert_eq!(ids_of(page_of(&pieces, &ids.apollo)), [alone.as_str()], "listed once, not in both");
    // Projects lists all it has; Apollo left one out to the other, so it is not the whole of what is in Apollo.
    assert!(page_of(&pieces, &ids.projects).afresh);
    assert!(!page_of(&pieces, &ids.apollo).afresh, "what it leaves out would be removed here, though the server has it");
    // A run that reads Apollo but not Projects lists the shared email nowhere, and does not take it from Projects.
    let pieces = run(&stand_in, &light(500, &[&ids.apollo]));
    assert_eq!(ids_of(page_of(&pieces, &ids.apollo)), [alone.as_str()]);
    assert!(!page_of(&pieces, &ids.apollo).afresh);
    // Ids are compared as text, not as numbers.
    let claimable = |ids: &[&str]| -> HashMap<String, usize> { ids.iter().map(|id| (id.to_string(), 6)).collect() };
    let both_ids = Some(json!({"M10": true, "M2": true}));
    let all = claimable(&["M10", "M2"]);
    assert!(claims("M10", both_ids.as_ref(), Some(&all)) && !claims("M2", both_ids.as_ref(), Some(&all)));
}

#[test]
fn a_message_in_the_inbox_and_a_label_is_the_inboxs_whatever_their_ids() {
    // The Inbox's id sorts after the label's: the Inbox lists the message all the same, and its window is whole,
    // so what it no longer holds is removed here.
    let stand_in = StandIn::new();
    // A mailbox whose id is the one given.
    let named = |name: &str, role: Option<&str>, id: &str| {
        stand_in.with(|d| {
            let given = d.mailbox(name, None, role);
            if let Some(mailbox) = d.mailboxes.iter_mut().find(|m| m.id == given) {
                mailbox.id = id.into();
            }
        });
        id.to_string()
    };
    let (inbox, label) = (named("Inbox", Some("inbox"), "inbox-id-z"), named("Label", None, "label-a"));
    let both = deliver(&stand_in, &inbox, "both", &[], 1);
    let alone = deliver(&stand_in, &inbox, "alone", &[], 2);
    assert!(stand_in.with(|d| d.add_to_mailbox(&both, &label)));
    for mut asked in [everything(500), light(500, &[&label])] {
        // The engine has a message in the Inbox that the server no longer holds.
        has(&mut asked, &inbox, &["Egone"]);
        let pieces = run(&stand_in, &asked);
        let page = page_of(&pieces, &inbox);
        assert_eq!(ids_of(page), [alone.as_str(), both.as_str()]);
        assert!(page.afresh, "the Inbox's window is whole: what it lacks is removed");
        let labelled = page_of(&pieces, &label);
        assert!(ids_of(labelled).is_empty() && !labelled.afresh, "the label leaves the message to the Inbox");
        assert!(last(&pieces).whole, "{:?}", last(&pieces).errors);
    }
    // A special mailbox comes before a folder of the owner's own as well.
    let (sent, mine) = (named("Sent", Some("sent"), "sent-z"), named("Mine", None, "mine-a"));
    let sent_too = deliver(&stand_in, &mine, "sent too", &[], 3);
    assert!(stand_in.with(|d| d.add_to_mailbox(&sent_too, &sent)));
    let pieces = run(&stand_in, &everything(500));
    assert_eq!(ids_of(page_of(&pieces, &sent)), [sent_too.as_str()]);
    assert!(ids_of(page_of(&pieces, &mine)).is_empty());
}

#[test]
fn a_mailbox_that_is_no_place_for_mail_does_not_take_it_from_the_one_it_is_in() {
    // A view over every email ("all"), with an id that is smaller than any other: it is no mailbox the
    // list has, so it claims nothing, and the Inbox lists what is in it.
    let (stand_in, ids) = world();
    let view = stand_in.with(|d| {
        let view = d.mailbox("All mail", None, Some("all"));
        if let Some(mailbox) = d.mailboxes.iter_mut().find(|m| m.id == view) {
            mailbox.id = "A0".into();
        }
        "A0".to_string()
    });
    let first = deliver(&stand_in, &ids.inbox, "first", &[], 1);
    let second = deliver(&stand_in, &ids.inbox, "second", &[], 2);
    assert!(stand_in.with(|d| d.add_to_mailbox(&first, &view) && d.add_to_mailbox(&second, &view)));
    for asked in [everything(500), light(500, &[])] {
        let pieces = run(&stand_in, &asked);
        let inbox = page_of(&pieces, &ids.inbox);
        assert_eq!(ids_of(inbox), [second.as_str(), first.as_str()]);
        assert!(inbox.afresh);
    }
    // Where the list could not be had, every id the email has counts.
    let fake_list_failed = |claimable: Option<&HashMap<String, usize>>| claims("M1", Some(&json!({"A0": true, "M1": true})), claimable);
    assert!(!fake_list_failed(None));
    assert!(fake_list_failed(Some(&[("M1".to_string(), 0)].into())));
}

#[test]
fn a_mailbox_whose_window_left_emails_to_another_is_not_the_whole_mailbox() {
    // The engine has E in Apollo; the server has put it in Projects as well, which has the smaller id.
    let (stand_in, ids) = world();
    let shared = deliver(&stand_in, &ids.apollo, "shared", &[], 1);
    let stays = deliver(&stand_in, &ids.apollo, "stays", &[], 2);
    assert!(stand_in.with(|d| d.add_to_mailbox(&shared, &ids.projects)));
    let mut asked = light(500, &[&ids.apollo]);
    has(&mut asked, &ids.apollo, &[&stays, &shared]);
    let pieces = run(&stand_in, &asked);
    let apollo = page_of(&pieces, &ids.apollo);
    // Apollo's window does not list it; a window that says it is the whole of Apollo would have it removed here.
    assert_eq!(ids_of(apollo), [stays.as_str()]);
    assert!(!apollo.afresh);
    assert!(last(&pieces).whole, "nothing failed");
}

#[test]
fn the_same_messages_are_read_from_a_store_and_from_a_server_that_names_by_place() {
    let build = |profile: Profile| {
        let (stand_in, ids) = world();
        stand_in.with(|d| d.profile = profile);
        deliver(&stand_in, &ids.inbox, "first", &["$seen"], 1);
        deliver(&stand_in, &ids.inbox, "second", &["$flagged"], 2);
        deliver(&stand_in, &ids.sent, "outgoing", &[], 3);
        let pieces = run(&stand_in, &everything(500));
        (pieces, stand_in.asked())
    };
    let (standard, _) = build(Profile::standard());
    let (by_place, asked) = build(Profile::by_place());
    assert_eq!(standard, by_place);
    assert_eq!(subjects(pages(&by_place)[0]), ["second", "first"]);
    // And nothing was asked for that such a server does not have.
    for ask in asked.iter().filter(|a| a.method == "Email/get") {
        assert!(ask.properties.iter().all(|p| EMAIL_PROPERTIES.contains(&p.as_str())), "{ask:?}");
    }
}

#[test]
fn a_run_that_names_mailboxes_reads_the_inbox_and_them_in_that_order() {
    let (stand_in, ids) = world();
    for mailbox in [&ids.apollo, &ids.sent, &ids.inbox] {
        deliver(&stand_in, mailbox, "m", &[], 1);
    }
    // Named twice, named with the Inbox, not an id, not a mailbox of the server's: each read as it should be.
    let named = light(500, &[&ids.apollo, &ids.inbox, "no good", "M999", &ids.apollo, &ids.sent]);
    let pieces = run(&stand_in, &named);
    assert_eq!(folders_read(&pieces), [&ids.inbox, &ids.apollo, &ids.sent]);
    assert!(pieces.iter().all(|p| p.listing.is_none()), "no list was asked for");
    let progress: Vec<(u32, u32)> = pieces.iter().filter_map(|p| p.progress.as_ref()).map(|p| (p.number, p.folders)).collect();
    assert_eq!(progress, [(1, 3), (2, 3), (3, 3)]);
    // The list asked for as well: it comes first, and the windows are the same.
    let mut listed = named.clone();
    listed.list_folders = true;
    let pieces = run(&stand_in, &listed);
    assert!(pieces[0].listing.as_ref().is_some_and(|l| l.folders.len() == 6));
    assert_eq!(folders_read(&pieces), [&ids.inbox, &ids.apollo, &ids.sent]);
    // The special mailboxes come in the order a folder pane lists them, then the rest in the list's.
    assert_eq!(folders_read(&run(&stand_in, &everything(500))), [&ids.inbox, &ids.drafts, &ids.sent, &ids.trash, &ids.projects, &ids.apollo]);
}

#[test]
fn a_mailbox_list_that_has_a_cycle_is_a_tree_when_it_leaves() {
    let (stand_in, ids) = world();
    // Projects inside Apollo, which is inside Projects; Trash inside itself; Drafts inside a mailbox that isn't there.
    stand_in.with(|d| {
        for mailbox in &mut d.mailboxes {
            if mailbox.name == "Projects" {
                mailbox.parent = Some(ids.apollo.clone());
            }
            if mailbox.name == "Trash" {
                mailbox.parent = Some(ids.trash.clone());
            }
            if mailbox.name == "Drafts" {
                mailbox.parent = Some("M999".into());
            }
        }
    });
    let pieces = run(&stand_in, &everything(500));
    let listing = pieces[0].listing.as_ref().unwrap();
    assert!(listing.complete);
    let parents: Vec<(&str, &str)> = listing.folders.iter().map(|f| (f.name.as_str(), f.parent.as_str())).collect();
    // Cut loose where the walk came round: Projects, which is listed first of the two.
    assert_eq!(parents, [("Inbox", ""), ("Drafts", ""), ("Sent", ""), ("Trash", ""), ("Projects", ""), ("Apollo", ids.projects.as_str())]);
    // All six are read, none twice.
    assert_eq!(folders_read(&pieces).len(), 6);
}

/// What a scripted server answers to one request.
type Reply = Result<HttpResponse, TransportError>;

/// An exchange that lets a function see each request before the server it wraps answers it.
struct Hook<H> {
    inner: H,
    see: Rc<dyn Fn(&HttpRequest)>,
}

impl<H: HttpExchange> HttpExchange for Hook<H> {
    async fn exchange(&self, req: &HttpRequest) -> Reply {
        (self.see)(req);
        self.inner.exchange(req).await
    }
}

fn is_query(req: &HttpRequest) -> bool {
    String::from_utf8_lossy(req.body()).contains("Email/query")
}

#[test]
fn a_run_stopped_before_it_begins_asks_nothing() {
    let (stand_in, ids) = world();
    deliver(&stand_in, &ids.inbox, "first", &[], 1);
    let cancel = AtomicBool::new(true);
    let pieces = run_over(stand_in.clone(), SESSION, &everything(500), &cancel);
    assert_eq!(pieces.len(), 1);
    let end = last(&pieces);
    assert!(end.stopped && !end.whole && end.errors.is_empty() && !end.auth_refused);
    assert_eq!(stand_in.api_requests(), 0, "only the session was asked for");
}

#[test]
fn a_run_stopped_between_two_requests_of_a_window_leaves_that_window_out() {
    let (stand_in, ids) = world();
    deliver(&stand_in, &ids.inbox, "first", &[], 1);
    deliver(&stand_in, &ids.sent, "second", &[], 2);
    let cancel = Rc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    // The owner presses Cancel while the Inbox's window is on its way.
    let hook = Hook {
        inner: stand_in.clone(),
        see: Rc::new(move |req| {
            if is_query(req) {
                flag.store(true, Ordering::Relaxed)
            }
        }),
    };
    let pieces = run_over(hook, SESSION, &everything(500), &cancel);
    // The list had been read; the window had not been taken in, and its emails were not asked for.
    assert_eq!(pieces.len(), 2);
    assert!(pieces[0].listing.is_some());
    assert!(last(&pieces).stopped && !last(&pieces).whole);
    assert_eq!(stand_in.calls(), ["Mailbox/get", "Email/query", "Email/get"]);
}

#[test]
fn a_run_stopped_after_a_piece_ends_there() {
    let (stand_in, ids) = world();
    for mailbox in [&ids.inbox, &ids.drafts, &ids.sent] {
        deliver(&stand_in, mailbox, "m", &[], 1);
    }
    let client = block_on(Connection::connect(stand_in.clone(), SESSION, Credentials::bearer(TOKEN).unwrap())).unwrap();
    let cancel = AtomicBool::new(false);
    let mut pieces = Vec::new();
    // The owner presses Cancel when the first window arrives.
    block_on(sync_mail(&client, &everything(500), &cancel, &mut |piece| {
        if !piece.pages.is_empty() {
            cancel.store(true, Ordering::Relaxed);
        }
        pieces.push(piece);
    }));
    well_formed(&pieces);
    assert_eq!(folders_read(&pieces), [&ids.inbox]);
    let end = last(&pieces);
    assert!(end.stopped && !end.whole && end.errors.is_empty());
    // Nothing was asked for the next mailbox.
    assert_eq!(stand_in.calls().iter().filter(|call| *call == "Email/query").count(), 1);
}

#[test]
fn a_server_that_refuses_the_credentials_ends_the_run_with_a_word_for_the_engine() {
    let (stand_in, ids) = world();
    for mailbox in [&ids.inbox, &ids.drafts, &ids.sent] {
        deliver(&stand_in, mailbox, "m", &[], 1);
    }
    // Signed in, and then the token is no longer held.
    let client = block_on(Connection::connect(stand_in.clone(), SESSION, Credentials::bearer(TOKEN).unwrap())).unwrap();
    stand_in.with(|d| d.token = Some("rotated".into()));
    let mut pieces = Vec::new();
    block_on(sync_mail(&client, &everything(500), &AtomicBool::new(false), &mut |piece| pieces.push(piece)));
    well_formed(&pieces);
    assert_eq!(pieces.len(), 1);
    let end = last(&pieces);
    assert!(end.auth_refused && !end.whole && !end.stopped && end.busy_for.is_none() && end.errors.is_empty(), "{end:?}");
    assert_eq!(stand_in.api_requests(), 1, "nothing more was asked");

    // The token goes in the middle of the run: what came before is kept.
    stand_in.with(|d| (d.token, d.seen, d.drop_token_after) = (Some(TOKEN.into()), Vec::new(), None));
    let mut has_inbox = light(500, &[&ids.drafts, &ids.sent]);
    let inbox_ids: Vec<String> = stand_in.with(|d| d.emails.iter().filter(|e| e.mailboxes.contains(&ids.inbox)).map(|e| e.id.clone()).collect());
    has(&mut has_inbox, &ids.inbox, &inbox_ids.iter().map(String::as_str).collect::<Vec<_>>());
    let client = block_on(Connection::connect(stand_in.clone(), SESSION, Credentials::bearer(TOKEN).unwrap())).unwrap();
    // (The list and the Inbox are let through: the Drafts' window is the third request.)
    stand_in.with(|d| d.drop_token_after = Some(2));
    let mut pieces = Vec::new();
    block_on(sync_mail(&client, &has_inbox, &AtomicBool::new(false), &mut |piece| pieces.push(piece)));
    well_formed(&pieces);
    assert_eq!(folders_read(&pieces), [&ids.inbox]);
    assert!(last(&pieces).auth_refused && !last(&pieces).whole);
    assert_eq!(stand_in.api_requests(), 3);
}

#[test]
fn a_server_that_is_busy_ends_the_run_with_the_seconds_it_asked_for() {
    let (stand_in, ids) = world();
    for mailbox in [&ids.inbox, &ids.sent] {
        deliver(&stand_in, mailbox, "m", &[], 1);
    }
    for (refused, waits) in [(Some((429, Some(7))), 7), (Some((503, None)), 30), (Some((429, Some(0))), 1), (Some((503, Some(100_000))), 3600)] {
        stand_in.with(|d| (d.refuse_next, d.seen) = ([refused].into(), Vec::new()));
        let pieces = run(&stand_in, &everything(500));
        assert_eq!(pieces.len(), 1);
        let end = last(&pieces);
        assert_eq!((end.busy_for, end.whole, end.auth_refused, end.stopped), (Some(waits), false, false, false), "{refused:?}");
        assert_eq!(end.errors.len(), 1);
        assert!(end.errors[0].starts_with("Couldn't bring the mail down from the server: the server is busy "), "{}", end.errors[0]);
        assert_eq!(stand_in.api_requests(), 1, "the server is left alone");
    }
    // It is busy after it gave the list and a window: they are kept, and no more is asked.
    let in_inbox: Vec<String> = stand_in.with(|d| d.emails.iter().filter(|e| e.mailboxes.contains(&ids.inbox)).map(|e| e.id.clone()).collect());
    let mut known = everything(500);
    has(&mut known, &ids.inbox, &in_inbox.iter().map(String::as_str).collect::<Vec<_>>());
    stand_in.with(|d| (d.refuse_next, d.seen) = ([None, None, Some((503, Some(12)))].into(), Vec::new()));
    let pieces = run(&stand_in, &known);
    well_formed(&pieces);
    assert!(pieces[0].listing.is_some());
    assert_eq!(folders_read(&pieces), [&ids.inbox], "the Inbox's window was whole by the time the server tired");
    assert_eq!(last(&pieces).busy_for, Some(12));
    assert_eq!(stand_in.api_requests(), 3);
}

#[test]
fn a_method_error_in_one_mailbox_does_not_stop_the_others() {
    let (stand_in, ids) = world();
    for (n, mailbox) in [&ids.inbox, &ids.drafts, &ids.sent, &ids.trash, &ids.apollo].into_iter().enumerate() {
        deliver(&stand_in, mailbox, "m", &[], n);
    }
    stand_in.with(|d| d.broken_mailboxes.insert(ids.sent.clone(), "serverFail"));
    let pieces = run(&stand_in, &everything(500));
    assert_eq!(folders_read(&pieces), [&ids.inbox, &ids.drafts, &ids.trash, &ids.projects, &ids.apollo]);
    let end = last(&pieces);
    assert!(!end.whole && !end.stopped && !end.auth_refused && end.busy_for.is_none());
    // Said once, with the server's reason, and without a name or an id.
    assert_eq!(
        end.errors,
        ["Couldn't read the mail in 1 mailbox: the server refused this call with the error serverFail: \"this mailbox can't be read just now\""]
    );
    // Several are counted, and the first reason given.
    stand_in.with(|d| d.broken_mailboxes.insert(ids.apollo.clone(), "serverFail"));
    let end = last(&run(&stand_in, &everything(500))).errors.clone();
    assert_eq!(
        end,
        ["Couldn't read the mail in 2 mailboxes: the server refused this call with the error serverFail: \"this mailbox can't be read just now\""]
    );
}

#[test]
fn an_error_for_every_mailbox_is_told_and_no_mailbox_is_left_unread_for_the_others_sake() {
    // The server answers an error for each of the mailboxes but the Inbox: it is there, and it says so.
    // None of that is a reason to leave the mailboxes after them for the next run, and the next, and the next.
    let (stand_in, ids) = world();
    deliver(&stand_in, &ids.apollo, "last of all", &[], 1);
    stand_in.with(|d| d.broken_mailboxes.extend([&ids.drafts, &ids.sent, &ids.trash, &ids.projects].map(|id| (id.clone(), "serverFail"))));
    let pieces = run(&stand_in, &everything(500));
    assert_eq!(folders_read(&pieces), [&ids.inbox, &ids.apollo]);
    assert_eq!(subjects(page_of(&pieces, &ids.apollo)), ["last of all"]);
    assert_eq!(last(&pieces).errors.len(), 1, "{:?}", last(&pieces).errors);
}

#[test]
fn a_server_that_says_it_is_unavailable_three_mailboxes_in_a_row_is_left_alone() {
    let (stand_in, ids) = world();
    stand_in.with(|d| d.broken_mailboxes.extend([&ids.drafts, &ids.sent, &ids.trash].map(|id| (id.clone(), "serverUnavailable"))));
    let pieces = run(&stand_in, &everything(500));
    assert_eq!(folders_read(&pieces), [&ids.inbox], "the rest wait for the next run");
    let end = last(&pieces);
    assert_eq!(end.errors.len(), 2);
    assert!(
        end.errors[0].starts_with("Couldn't read the mail in 3 mailboxes: the server refused this call with the error serverUnavailable: "),
        "{:?}",
        end.errors
    );
    assert_eq!(end.errors[1], "The server failed 3 mailboxes in a row; the rest wait for the next run.");
    assert!(!end.whole);
    // Two in a row, and then one that works: the count starts again.
    stand_in.with(|d| d.broken_mailboxes.remove(&ids.trash));
    let pieces = run(&stand_in, &everything(500));
    assert_eq!(folders_read(&pieces), [&ids.inbox, &ids.trash, &ids.projects, &ids.apollo]);
    assert_eq!(last(&pieces).errors.len(), 1);
    // An error of another kind between them: the server answered, so nothing counts.
    stand_in.with(|d| {
        d.broken_mailboxes =
            [(ids.drafts.clone(), "serverUnavailable"), (ids.sent.clone(), "serverFail"), (ids.trash.clone(), "serverUnavailable")].into()
    });
    let pieces = run(&stand_in, &everything(500));
    assert_eq!(folders_read(&pieces), [&ids.inbox, &ids.projects, &ids.apollo]);
    assert!(last(&pieces).errors.iter().all(|line| !line.contains("in a row")), "{:?}", last(&pieces).errors);
}

#[test]
fn requests_that_fail_whole_three_in_a_row_end_the_run_and_a_good_one_in_between_starts_the_count_again() {
    // The list and the Inbox go through; then every request is refused with a 500, except that one in the middle is not.
    let (stand_in, ids) = world();
    for mailbox in [&ids.inbox, &ids.drafts, &ids.sent, &ids.trash, &ids.projects, &ids.apollo] {
        deliver(&stand_in, mailbox, "m", &[], 1);
    }
    let known_in_inbox: Vec<String> = stand_in.with(|d| d.emails.iter().filter(|e| e.mailboxes.contains(&ids.inbox)).map(|e| e.id.clone()).collect());
    let mut asked = everything(500);
    has(&mut asked, &ids.inbox, &known_in_inbox.iter().map(String::as_str).collect::<Vec<_>>());
    // Request 1: the list. 2: the Inbox. 3 to 5: Drafts, Sent and Trash, refused.
    stand_in.with(|d| d.refuse_next = [None, None, Some((500, None)), Some((500, None)), Some((500, None))].into());
    let pieces = run(&stand_in, &asked);
    assert_eq!(folders_read(&pieces), [&ids.inbox]);
    let end = last(&pieces);
    assert_eq!(end.errors.len(), 2, "{:?}", end.errors);
    assert!(
        end.errors[0]
            .starts_with("Couldn't read the mail in 3 mailboxes: the server answered with HTTP status 500, which this program cannot act on"),
        "{:?}",
        end.errors
    );
    assert_eq!(end.errors[1], "The server failed 3 mailboxes in a row; the rest wait for the next run.");
    assert_eq!(stand_in.api_requests(), 5, "and no more was asked of it");
    // Refused twice, then well, then refused twice: the run goes on to the end.
    stand_in.with(|d| d.refuse_next = [None, None, Some((500, None)), Some((500, None)), None, None, Some((500, None)), Some((500, None))].into());
    let pieces = run(&stand_in, &asked);
    assert_eq!(folders_read(&pieces).len(), 2, "the Inbox, and Trash after two refused and one that was not: its details too");
    assert!(!last(&pieces).errors.is_empty());
}

#[test]
fn one_email_that_breaks_a_call_is_found_by_halving_it_and_the_rest_are_read() {
    let (stand_in, ids) = world();
    let all: Vec<String> = (0..450).map(|n| deliver(&stand_in, &ids.inbox, &format!("m{n}"), &[], n)).collect();
    deliver(&stand_in, &ids.sent, "outgoing", &[], 500);
    // The ones the engine has (the oldest ten) come with their keywords; one of the rest breaks every call it is in.
    let mut next = everything(500);
    has(&mut next, &ids.inbox, &all[..10].iter().map(String::as_str).collect::<Vec<_>>());
    stand_in.with(|d| (d.broken_emails.insert(all[100].clone()), d.seen.clear()));
    let pieces = run(&stand_in, &next);
    let inbox = page_of(&pieces, &ids.inbox);
    // Everything but that one: 439 whole and the ten the engine has.
    assert_eq!(inbox.messages.len(), 449);
    assert!(inbox.messages.iter().all(|m| m.subject != "m100"));
    assert_eq!((inbox.messages[0].subject.as_str(), inbox.messages[438].subject.as_str()), ("m449", "m10"));
    assert!(inbox.messages[439..].iter().all(|m| m.subject.is_empty()), "the ten it has");
    // Not the whole mailbox as far as the page goes: the page lacks an email the server has.
    assert!(!inbox.afresh);
    // The other mailboxes were read, and the trouble is said once.
    assert_eq!(subjects(page_of(&pieces, &ids.sent)), ["outgoing"]);
    let end = last(&pieces);
    assert_eq!(
        end.errors,
        ["Couldn't read 1 message in full: the server refused this call with the error serverFail: \"an email can't be read just now\""]
    );
    assert!(!end.whole);
    // Finding it took a few calls, not one for each email: the calls for emails are the three chunks, the halves
    // of the one that failed, and the Sent mailbox's; each level of halving makes one more that fails.
    let gets = stand_in.asked().iter().filter(|a| a.method == "Email/get" && a.properties.iter().any(|p| p == "headers")).count();
    assert!((15..=30).contains(&gets), "{gets} calls for emails");
}

#[test]
fn halving_is_bounded_when_every_call_for_emails_fails() {
    let (stand_in, ids) = world();
    let all: Vec<String> = (0..450).map(|n| deliver(&stand_in, &ids.inbox, &format!("m{n}"), &[], n)).collect();
    deliver(&stand_in, &ids.sent, "outgoing", &[], 500);
    stand_in.with(|d| {
        (
            d.broken_emails.extend(all.iter().cloned()),
            d.broken_emails.insert(d.emails.last().map(|e| e.id.clone()).unwrap_or_default()),
            d.seen.clear(),
        )
    });
    let pieces = run(&stand_in, &everything(500));
    let gets = stand_in.asked().iter().filter(|a| a.method == "Email/get" && a.properties.iter().any(|p| p == "headers")).count();
    // The halvings of the whole run are bounded (each makes two calls from one), and every email that could not
    // be read is told.
    assert!(gets <= 2 * MAX_SPLITS + 6, "{gets} calls for emails");
    let inbox = page_of(&pieces, &ids.inbox);
    assert!(inbox.messages.is_empty() && !inbox.afresh);
    let end = last(&pieces);
    assert!(end.errors[0].starts_with("Couldn't read 451 messages in full: "), "{:?}", end.errors);
    assert!(!end.whole);
}

#[test]
fn a_call_whose_answer_is_too_large_is_halved_too() {
    // Each email is a few kilobytes here, and 200 of them are more than a call for them may be answered.
    let (stand_in, ids) = world();
    for n in 0..300 {
        let raw = format!("From: a@example.org\r\nSubject: m{n}\r\nX-Padding: {}\r\n\r\nbody\r\n", "p".repeat(60_000));
        stand_in.with(|d| d.deliver(&ids.inbox, raw.as_bytes(), &[], &at(n)));
    }
    let pieces = run(&stand_in, &light(300, &[]));
    let inbox = page_of(&pieces, &ids.inbox);
    // (The padding is not among the fields that are read, but every field of the message is in the answer.)
    assert_eq!(inbox.messages.len(), 300);
    assert!(inbox.afresh);
    assert!(last(&pieces).whole, "{:?}", last(&pieces).errors);
}

/// An exchange in front of a server, as a gateway is: it answers 502 to the requests a function picks, and
/// passes the rest on.
struct Gateway<H> {
    inner: H,
    gives_up_on: Rc<dyn Fn(&HttpRequest) -> bool>,
    refused: Rc<Cell<usize>>,
}

impl<H: HttpExchange> HttpExchange for Gateway<H> {
    async fn exchange(&self, req: &HttpRequest) -> Reply {
        if (self.gives_up_on)(req) {
            self.refused.set(self.refused.get() + 1);
            return json_reply(502, &json!({}));
        }
        self.inner.exchange(req).await
    }
}

/// Whether a call asks for the header fields of emails (what a message list needs, as against their keywords).
fn wants_headers(args: &Value) -> bool {
    args.get("properties").and_then(Value::as_array).is_some_and(|names| names.iter().any(|name| name == "headers"))
}

/// How many emails a request asks for whole: none for any other request.
fn details_asked(req: &HttpRequest) -> usize {
    calls_of(req)
        .iter()
        .filter(|(name, args, _)| name == "Email/get" && wants_headers(args))
        .map(|(_, args, _)| args.get("ids").and_then(Value::as_array).map_or(0, Vec::len))
        .sum()
}

#[test]
fn a_call_the_server_cannot_answer_is_halved_a_few_times_and_then_the_mailbox_waits() {
    let (stand_in, ids) = world();
    for n in 0..40 {
        deliver(&stand_in, &ids.inbox, &format!("m{n}"), &[], n);
    }
    deliver(&stand_in, &ids.sent, "outgoing", &[], 100);
    // A gateway gives up on a call for more than ten emails (too much for the server behind it): halved, they go through.
    let refused = Rc::new(Cell::new(0));
    let gateway = Gateway { inner: stand_in.clone(), gives_up_on: Rc::new(|req| details_asked(req) > 10), refused: refused.clone() };
    let pieces = run_over(gateway, SESSION, &everything(500), &AtomicBool::new(false));
    assert_eq!((page_of(&pieces, &ids.inbox).messages.len(), page_of(&pieces, &ids.inbox).afresh), (40, true));
    assert!(last(&pieces).whole && last(&pieces).errors.is_empty(), "{:?}", last(&pieces).errors);
    assert_eq!(refused.get(), 3, "the forty, and the two twenties");
    // A gateway that gives up on every call for emails: a few halvings, and then the mailbox waits (its emails are
    // told as not read) and the run goes on to the next, which is the one email of Sent.
    let refused = Rc::new(Cell::new(0));
    let gateway = Gateway { inner: stand_in.clone(), gives_up_on: Rc::new(|req| details_asked(req) > 0), refused: refused.clone() };
    let pieces = run_over(gateway, SESSION, &everything(500), &AtomicBool::new(false));
    assert!(page_of(&pieces, &ids.inbox).messages.is_empty() && !page_of(&pieces, &ids.inbox).afresh);
    assert_eq!(folders_read(&pieces).len(), 6, "every mailbox was tried");
    let end = last(&pieces);
    assert!(!end.whole && end.errors.len() == 1 && end.errors[0].starts_with("Couldn't read 41 messages in full: "), "{:?}", end.errors);
    assert_eq!(refused.get(), 6, "the Inbox: 40, 20, 10, 5 and 2 emails (four halvings); Sent: its one");
}

#[test]
fn a_stop_between_the_calls_for_emails_of_a_window_leaves_that_window_out() {
    let (stand_in, ids) = world();
    for n in 0..450 {
        deliver(&stand_in, &ids.inbox, &format!("m{n}"), &[], n);
    }
    // Three calls for emails (200, 200 and 50) are needed; the first goes through, and then the run is stopped.
    // The server tires of it:
    stand_in.with(|d| (d.refuse_next, d.seen) = ([None, None, None, Some((429, Some(9)))].into(), Vec::new()));
    let pieces = run(&stand_in, &everything(500));
    assert!(folders_read(&pieces).is_empty(), "the Inbox's window was cut");
    assert!(pieces[0].listing.is_some());
    assert_eq!((last(&pieces).busy_for, last(&pieces).whole), (Some(9), false));
    assert_eq!(stand_in.api_requests(), 4);
    // It no longer holds the token:
    stand_in.with(|d| (d.token, d.drop_token_after, d.seen) = (Some(TOKEN.into()), Some(3), Vec::new()));
    let pieces = run(&stand_in, &everything(500));
    assert!(folders_read(&pieces).is_empty() && last(&pieces).auth_refused && !last(&pieces).whole);
    assert_eq!(stand_in.api_requests(), 4);
    // Its owner presses Cancel:
    stand_in.with(|d| (d.token, d.drop_token_after, d.seen) = (Some(TOKEN.into()), None, Vec::new()));
    let cancel = Rc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let hook = Hook {
        inner: stand_in.clone(),
        see: Rc::new(move |req| {
            if details_asked(req) > 0 {
                flag.store(true, Ordering::Relaxed)
            }
        }),
    };
    let pieces = run_over(hook, SESSION, &everything(500), &cancel);
    assert!(folders_read(&pieces).is_empty() && last(&pieces).stopped && !last(&pieces).whole);
    assert_eq!(stand_in.calls(), ["Mailbox/get", "Email/query", "Email/get", "Email/get"]);
}

/// A session address as a person's own server might hand one out, with their address in its query.
const FAKE_SESSION: &str = "https://jmap.example.org/jmap/session?account=ada%40example.org";

/// A server that answers as a function says, and keeps what it was sent.
#[derive(Clone)]
struct Fake {
    answer: Rc<dyn Fn(&HttpRequest) -> Reply>,
    sent: Rc<RefCell<Vec<HttpRequest>>>,
}

impl Fake {
    fn new(answer: impl Fn(&HttpRequest) -> Reply + 'static) -> Fake {
        Fake { answer: Rc::new(answer), sent: Rc::default() }
    }

    /// The arguments of each call to `method`, in the order they were made.
    fn args_of(&self, method: &str) -> Vec<Value> {
        let sent = self.sent.borrow();
        sent.iter()
            .filter(|req| req.method() == Method::Post)
            .flat_map(calls_of)
            .filter(|(name, ..)| name == method)
            .map(|(_, args, _)| args)
            .collect()
    }

    /// The methods each request to the API called.
    fn posts(&self) -> Vec<Vec<String>> {
        let sent = self.sent.borrow();
        sent.iter().filter(|req| req.method() == Method::Post).map(|req| calls_of(req).into_iter().map(|(name, _, _)| name).collect()).collect()
    }
}

impl HttpExchange for Fake {
    async fn exchange(&self, req: &HttpRequest) -> Reply {
        self.sent.borrow_mut().push(req.clone());
        (self.answer)(req)
    }
}

/// The method calls of a request to the API: name, arguments, call id.
fn calls_of(req: &HttpRequest) -> Vec<(String, Value, String)> {
    let body: Value = serde_json::from_slice(req.body()).unwrap_or(Value::Null);
    let calls = body.get("methodCalls").and_then(Value::as_array).cloned().unwrap_or_default();
    calls
        .iter()
        .map(|call| (call[0].as_str().unwrap_or_default().to_string(), call[1].clone(), call[2].as_str().unwrap_or_default().to_string()))
        .collect()
}

fn json_reply(status: u16, body: &Value) -> Reply {
    Ok(HttpResponse::new(status, serde_json::to_vec(body).unwrap()).with_field("Content-Type", "application/json"))
}

/// An answer of `status` with no body, and a `Retry-After` of so many seconds.
fn status_reply(status: u16, retry_after: Option<u32>) -> Reply {
    let answer = HttpResponse::new(status, Vec::new());
    Ok(match retry_after {
        Some(seconds) => answer.with_field("Retry-After", &seconds.to_string()),
        None => answer,
    })
}

/// What a scripted server says to one method call.
enum Answer {
    Is(Value),
    Error(&'static str, String),
    Missing,
}

/// The session of a server of the standard kind, and for a request to the API each call answered by `f`
/// (given its name and arguments).
fn serve(req: &HttpRequest, f: impl Fn(&str, &Value) -> Answer) -> Reply {
    serve_with(16, req, f)
}

/// [`serve`] for a server that takes `max_calls` calls to a request.
fn serve_with(max_calls: u32, req: &HttpRequest, f: impl Fn(&str, &Value) -> Answer) -> Reply {
    if req.method() == Method::Get {
        let session = json!({
            "capabilities": {CORE: {"maxCallsInRequest": max_calls, "maxObjectsInGet": 500}, MAIL: {}},
            "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {MAIL: {}}}},
            "primaryAccounts": {MAIL: "A1"},
            "apiUrl": "https://jmap.example.org/jmap/api",
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
            "uploadUrl": "/jmap/upload/{accountId}/",
            "state": "1",
        });
        return json_reply(200, &session);
    }
    let mut responses = Vec::new();
    for (name, args, id) in calls_of(req) {
        match f(&name, &args) {
            Answer::Is(result) => responses.push(json!([name, result, id])),
            Answer::Error(kind, why) => responses.push(json!(["error", {"type": kind, "description": why}, id])),
            Answer::Missing => {}
        }
    }
    json_reply(200, &json!({"methodResponses": responses, "sessionState": "1"}))
}

/// A list of mailboxes with the Inbox in it.
fn inbox_only() -> Value {
    json!({"accountId": "A1", "state": "1", "notFound": [], "list": [{"id": "M1", "name": "Inbox", "parentId": null, "role": "inbox", "totalEmails": 5, "unreadEmails": 1}]})
}

/// The emails `ids`, whole, in `M1`.
fn emails(ids: &[&str]) -> Value {
    let list: Vec<Value> = ids
        .iter()
        .map(|id| {
            json!({
                "id": id, "blobId": format!("B{id}"), "keywords": {"$seen": true}, "mailboxIds": {"M1": true}, "size": 100,
                "receivedAt": "2026-10-15T07:30:00Z", "hasAttachment": false, "preview": format!("About {id}"),
                "headers": [{"name": "Subject", "value": format!(" Subject of {id}")}, {"name": "From", "value": " Zoe <zoe@example.org>"}],
            })
        })
        .collect();
    json!({"accountId": "A1", "state": "1", "list": list, "notFound": []})
}

/// A server with the Inbox and `E1` to `E5` in it, that answers a query as `query` says.
fn with_query(query: Value) -> Fake {
    Fake::new(move |req| {
        serve(req, |name, _| match name {
            "Mailbox/get" => Answer::Is(inbox_only()),
            "Email/query" => Answer::Is(query.clone()),
            _ => Answer::Is(emails(&["E1", "E2", "E3", "E4", "E5"])),
        })
    })
}

fn run_fake(fake: &Fake, request: &JmapMailRequest) -> Vec<JmapMailResult> {
    run_over(fake.clone(), FAKE_SESSION, request, &AtomicBool::new(false))
}

#[test]
fn an_email_that_the_call_for_emails_says_is_gone_is_no_loss() {
    // E3 was there when the window was made and gone when it was read whole, and the server says so.
    let server = Fake::new(|req| {
        serve(req, |name, args| match name {
            "Mailbox/get" => Answer::Is(inbox_only()),
            "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": ["E1", "E2", "E3"], "total": 3, "position": 0})),
            _ if wants_headers(args) => {
                let mut got = emails(&["E1", "E2"]);
                got["notFound"] = json!(["E3"]);
                Answer::Is(got)
            }
            _ => Answer::Is(emails(&["E1", "E2", "E3"])),
        })
    });
    let pieces = run_fake(&server, &light(500, &[]));
    let inbox = page_of(&pieces, "M1");
    assert_eq!((ids_of(inbox), inbox.afresh), (vec!["E1", "E2"], true));
    assert!(last(&pieces).whole && last(&pieces).errors.is_empty());
}

#[test]
fn a_server_that_takes_one_call_to_a_request_is_asked_for_the_ids_of_the_window_that_are_ids() {
    // Seven entries in the list, the window is four: the first four are looked at, and two of them are ids.
    let server = Fake::new(|req| {
        serve_with(1, req, |name, args| match name {
            "Mailbox/get" => Answer::Is(inbox_only()),
            "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": ["E1", "bad id", 5, "E2", "E3", "E4", "E5"], "total": 7, "position": 0})),
            _ if wants_headers(args) => Answer::Is(emails(&["E1", "E2", "E3"])),
            _ => Answer::Is(emails(&["E1", "E2", "E3", "E4"])),
        })
    });
    let pieces = run_fake(&server, &light(4, &[]));
    let inbox = page_of(&pieces, "M1");
    assert_eq!((ids_of(inbox), inbox.afresh), (vec!["E1", "E2"], false));
    assert_eq!(server.posts(), [vec!["Mailbox/get"], vec!["Email/query"], vec!["Email/get"], vec!["Email/get"]]);
    // The second asks for what the first named, and for no more than the window.
    assert_eq!(server.args_of("Email/get")[0]["ids"], json!(["E1", "E2"]));
    assert!(last(&pieces).whole && last(&pieces).errors.is_empty());
}

#[test]
fn a_window_is_complete_only_when_the_server_says_it_listed_the_whole_mailbox() {
    let few = json!(["E1", "E2"]);
    let three = json!(["E1", "E2", "E3"]);
    let four = json!(["E1", "E2", "E3", "E4"]);
    // Asked for 3: the ids it listed, the total it gave, where it began, and whether that is the whole mailbox.
    // Only a total that is no more than the emails listed says so: a server that gives none, or a total that
    // is none, may have cut the list anywhere, and what is not listed here is removed from the mailbox here.
    let cases = [
        (&three, json!({"total": 3}), true),
        (&three, json!({"total": 4}), false),
        (&few, json!({"total": 2}), true),
        (&few, json!({"total": 5}), false),
        (&few, json!({"total": 2, "limit": 2}), true),
        (&few, json!({"total": 3, "limit": 2}), false),
        (&three, json!({"total": 3, "position": 0}), true),
        (&three, json!({"total": 3, "position": 1}), false),
        (&three, json!({"total": 3, "position": "0"}), false),
        (&three, json!({"total": 3, "position": -1}), false),
        (&three, json!({"total": 3, "position": null}), true),
        (&json!([]), json!({"total": 4}), false),
        (&four, json!({"total": 4}), false),
        // A total that is not the number of emails listed: more is a list that was cut; less is an answer that
        // contradicts itself.
        (&three, json!({"total": 2}), false),
        (&few, json!({"total": 0}), false),
        // An empty list is no whole mailbox when the list of mailboxes says the Inbox holds five emails (the
        // server of these answers); a mailbox that is empty is told apart in a test of its own.
        (&json!([]), json!({"total": 0}), false),
        // No total, or none that is a number: nothing says the list is all there is.
        (&few, json!({}), false),
        (&three, json!({}), false),
        (&json!([]), json!({}), false),
        (&few, json!({"limit": 100}), false),
        (&few, json!({"limit": 0}), false),
        (&few, json!({"total": "2"}), false),
        (&few, json!({"total": -1}), false),
        (&few, json!({"total": 2.0}), false),
        (&few, json!({"total": null}), false),
        // An id that is none, or one twice, is an email the list did not name.
        (&json!(["E1", "bad id", "E2"]), json!({"total": 3}), false),
        (&json!(["E1", "bad id", "E2"]), json!({"total": 2}), false),
        (&json!(["E1", 5, "E2"]), json!({"total": 3}), false),
        (&json!(["E1", 5]), json!({"total": 1}), false),
        (&json!(["E1", "E1", "E2"]), json!({"total": 3}), false),
        (&json!(["E1", "E1", "E2"]), json!({"total": 2}), true),
    ];
    for (ids, extra, complete) in cases {
        let mut query = json!({"accountId": "A1", "queryState": "1", "canCalculateChanges": false, "position": 0, "ids": ids});
        query.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let pieces = run_fake(&with_query(query.clone()), &light(3, &[]));
        assert_eq!(page_of(&pieces, "M1").afresh, complete, "{query}");
        assert!(last(&pieces).errors.is_empty() && last(&pieces).whole);
    }
    // No position at all is a list from the start, as for a server that does not say.
    let pieces = run_fake(&with_query(json!({"accountId": "A1", "ids": three, "total": 3})), &light(3, &[]));
    assert!(page_of(&pieces, "M1").afresh);
}

#[test]
fn an_empty_window_is_the_whole_of_a_mailbox_only_when_the_list_of_mailboxes_says_it_is_empty() {
    // The mailbox list says what the Inbox holds (or does not say); the window says nothing is in it.
    let server = |holds: Value| {
        Fake::new(move |req| {
            serve(req, |name, _| match name {
                "Mailbox/get" => {
                    let mut list = inbox_only();
                    match &holds {
                        Value::Null => drop(list["list"][0].as_object_mut().map(|inbox| inbox.remove("totalEmails"))),
                        count => list["list"][0]["totalEmails"] = count.clone(),
                    }
                    Answer::Is(list)
                }
                _ => Answer::Is(json!({"accountId": "A1", "ids": [], "total": 0, "position": 0, "list": [], "notFound": []})),
            })
        })
    };
    // Empty on both sides: the mailbox is empty, and a complete window of nothing removes what the engine has.
    let pieces = run_fake(&server(json!(0)), &light(500, &[]));
    assert!(page_of(&pieces, "M1").messages.is_empty() && page_of(&pieces, "M1").afresh);
    // The list says it holds emails, or says nothing of the kind: a server that could not open the mailbox answers
    // just so, and a window that said "empty, complete" would have the engine drop every message it has of it.
    for holds in [json!(1), json!(5), json!(4821), Value::Null, json!("0"), json!(-1), json!(0.5), json!(false), json!([0])] {
        let pieces = run_fake(&server(holds.clone()), &light(500, &[]));
        assert!(page_of(&pieces, "M1").messages.is_empty() && !page_of(&pieces, "M1").afresh, "{holds}");
        assert!(last(&pieces).whole && last(&pieces).errors.is_empty(), "no failure was told: the run goes on");
    }
    // A list of mailboxes that could not be had says nothing at all, and so is no proof either.
    let broken_list = Fake::new(|req| {
        serve(req, |name, _| match name {
            "Mailbox/get" => Answer::Error("serverFail", "try later".into()),
            _ => Answer::Is(json!({"accountId": "A1", "ids": [], "total": 0, "position": 0, "list": [], "notFound": []})),
        })
    });
    let pieces = run_fake(&broken_list, &light(500, &["M1"]));
    assert!(page_of(&pieces, "M1").messages.is_empty() && !page_of(&pieces, "M1").afresh);
    // One that is not empty is judged on what it names, whatever the list says of its count.
    let pieces = run_fake(&with_query(json!({"accountId": "A1", "ids": ["E1", "E2"], "total": 2, "position": 0})), &light(500, &[]));
    assert!(page_of(&pieces, "M1").afresh);
}

#[test]
fn a_call_for_emails_that_the_server_says_are_all_gone_is_not_taken_for_their_going() {
    // A server that could not open the mailbox says so of each id. The window names `named`, and says they are
    // there; the call for them whole finds `gone` of them gone.
    let server = |named: &'static [&'static str], gone: &'static [&'static str]| {
        Fake::new(move |req| {
            serve(req, |name, args| match name {
                "Mailbox/get" => Answer::Is(inbox_only()),
                "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": named, "total": named.len(), "position": 0})),
                _ if wants_headers(args) => {
                    let mut got = emails(&named.iter().copied().filter(|id| !gone.contains(id)).collect::<Vec<_>>());
                    got["notFound"] = json!(gone);
                    Answer::Is(got)
                }
                _ => Answer::Is(emails(named)),
            })
        })
    };
    // All three, or the only one: the page lacks them, and it is not the whole mailbox; the run says so and is not
    // whole.
    for (named, plural) in [(&["E1", "E2", "E3"][..], "3 messages"), (&["E1"][..], "1 message")] {
        let pieces = run_fake(&server(named, named), &light(500, &[]));
        let inbox = page_of(&pieces, "M1");
        assert!(inbox.messages.is_empty() && !inbox.afresh, "{named:?}");
        assert!(!last(&pieces).whole);
        assert_eq!(last(&pieces).errors, [format!("Couldn't read {plural} in full: the server's answer did not have them")]);
    }
    // Some of them: those are gone, and the rest are read; nothing is lost by it.
    let pieces = run_fake(&server(&["E1", "E2", "E3"], &["E2"]), &light(500, &[]));
    let inbox = page_of(&pieces, "M1");
    assert_eq!((ids_of(inbox), inbox.afresh), (vec!["E1", "E3"], true));
    assert!(last(&pieces).whole && last(&pieces).errors.is_empty());
}

#[test]
fn a_list_with_no_mailbox_in_it_is_said_and_the_run_is_not_an_up_to_date() {
    // An account always has an Inbox: a server that lists none has not told the whole of it.
    let server = Fake::new(|req| serve(req, |_, _| Answer::Is(json!({"accountId": "A1", "state": "1", "list": [], "notFound": []}))));
    for asked in [everything(500), light(500, &["M1"]), {
        let mut named = light(500, &["M1"]);
        named.list_folders = true;
        named
    }] {
        let pieces = run_fake(&server, &asked);
        assert!(pages(&pieces).is_empty(), "nothing is read");
        let end = last(&pieces);
        assert!(!end.whole);
        assert_eq!(end.errors, ["Couldn't list the mailboxes on the server: the server's answer cannot be used: the answer lists no mailbox"]);
        // What there is of a listing is not a complete one: no folder is removed on its word.
        for piece in &pieces {
            assert!(piece.listing.as_ref().is_none_or(|listing| listing.folders.is_empty() && !listing.complete));
        }
    }
}

#[test]
fn an_email_the_list_names_and_the_second_answer_lacks_is_gone_only_when_the_server_says_so() {
    // The server names E1 to E3 and answers for E1 and E2 alone.
    let server = |not_found: Value| {
        Fake::new(move |req| {
            serve(req, |name, _| match name {
                "Mailbox/get" => Answer::Is(inbox_only()),
                "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": ["E1", "E2", "E3"], "total": 3})),
                _ => {
                    let mut got = emails(&["E1", "E2"]);
                    got["notFound"] = not_found.clone();
                    Answer::Is(got)
                }
            })
        })
    };
    // It does not say why: the list may be cut, so it is not the whole mailbox, and nothing is removed for the lack.
    let pieces = run_fake(&server(json!([])), &light(500, &[]));
    assert_eq!((ids_of(page_of(&pieces, "M1")), page_of(&pieces, "M1").afresh), (vec!["E1", "E2"], false));
    assert!(last(&pieces).whole, "nothing failed");
    // It says E3 is not found (gone since the list was made): the mailbox is whole without it.
    let pieces = run_fake(&server(json!(["E3"])), &light(500, &[]));
    assert_eq!((ids_of(page_of(&pieces, "M1")), page_of(&pieces, "M1").afresh), (vec!["E1", "E2"], true));
    // It says all of them are not found: more likely it could not open the mailbox than that every email went
    // between the two calls of one request. Nothing is removed on its word.
    let everything_gone = Fake::new(|req| {
        serve(req, |name, _| match name {
            "Mailbox/get" => Answer::Is(inbox_only()),
            "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": ["E1", "E2"], "total": 2})),
            _ => Answer::Is(json!({"accountId": "A1", "list": [], "notFound": ["E1", "E2"]})),
        })
    });
    let pieces = run_fake(&everything_gone, &light(500, &[]));
    assert!(page_of(&pieces, "M1").messages.is_empty() && !page_of(&pieces, "M1").afresh);
    // Not found is for the ids the list named: a stranger's id in it is nothing.
    let pieces = run_fake(&server(json!(["E9", 5, null])), &light(500, &[]));
    assert!(!page_of(&pieces, "M1").afresh);
}

#[test]
fn a_window_lists_each_email_the_server_names_once_and_leaves_out_what_is_no_id() {
    let query = json!({"accountId": "A1", "ids": ["E1", "E1", "bad id", "E2", 5, null, "", "E3", "E9"], "total": 9});
    let pieces = run_fake(&with_query(query), &light(500, &[]));
    let inbox = page_of(&pieces, "M1");
    // (E9 is named, and the second answer has nothing for it: it went meanwhile.)
    assert_eq!(ids_of(inbox), ["E1", "E2", "E3"]);
    assert!(!inbox.afresh);
    assert!(last(&pieces).whole);
    // Only the first of the ids a window may hold.
    let many: Vec<String> = (0..600).map(|n| format!("X{n}")).collect();
    let query = json!({"accountId": "A1", "ids": many, "total": 600});
    let pieces = run_fake(&with_query(query), &light(4, &[]));
    assert!(page_of(&pieces, "M1").messages.is_empty(), "none of those is an email the server has");
    assert!(!page_of(&pieces, "M1").afresh, "it was cut, and it is not the whole mailbox");
}

#[test]
fn what_the_server_sends_that_was_not_asked_for_is_not_taken() {
    // The server lists E1 to E3, then answers a request for whole emails with those, a copy, and a stranger.
    let fake = Fake::new(|req| {
        serve(req, |name, args| match name {
            // (M0 is a mailbox of the list.)
            "Mailbox/get" => Answer::Is(json!({"accountId": "A1", "list": [
                {"id": "M1", "name": "Inbox", "role": "inbox"}, {"id": "M0", "name": "Before"}]})),
            "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": ["E1", "E2", "E3"], "total": 3})),
            _ if args.get("properties").is_some_and(|p| p.as_array().is_some_and(|p| p.iter().any(|n| n == "headers"))) => {
                let mut list = emails(&["E1", "E2", "E9", "E1"]);
                // E2 has gone to the other mailbox meanwhile: that mailbox lists it.
                list["list"][1]["mailboxIds"] = json!({"M0": true});
                Answer::Is(list)
            }
            _ => Answer::Is(emails(&["E1", "E2", "E3"])),
        })
    });
    let pieces = run_fake(&fake, &light(500, &[]));
    let inbox = page_of(&pieces, "M1");
    // E3 has no details (the server did not send them), E2 is the other mailbox's, E9 and the copy are not asked for.
    assert_eq!(ids_of(inbox), ["E1"]);
    assert_eq!(subjects(inbox), ["Subject of E1"]);
    assert!(!inbox.afresh, "the page lacks emails the server has: it is not the whole mailbox");
    // Two emails the page lacks (E2 lies in the other mailbox now, E3 was left out of the answer): the run says so
    // and is not whole, or what was moved here from a mailbox that was read in full would be swept.
    assert!(!last(&pieces).whole);
    assert_eq!(last(&pieces).errors, ["Couldn't read 2 messages in full: the server's answer did not have them"]);
    assert_eq!(fake.posts(), [vec!["Mailbox/get"], vec!["Email/query", "Email/get"], vec!["Email/get"]]);
}

#[test]
fn a_mailbox_list_that_cannot_be_read_is_said_and_a_light_run_goes_on_with_the_mailboxes_it_was_told() {
    let broken = |reply: fn() -> Answer| {
        Fake::new(move |req| {
            serve(req, |name, _| match name {
                "Mailbox/get" => reply(),
                "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": ["E1"], "total": 1})),
                _ => Answer::Is(emails(&["E1"])),
            })
        })
    };
    let cases: [(fn() -> Answer, &str); 3] = [
        (
            || Answer::Error("serverFail", "try later".into()),
            "Couldn't list the mailboxes on the server: the server refused this call with the error serverFail: \"try later\"",
        ),
        (
            || Answer::Is(json!({"accountId": "A1"})),
            "Couldn't list the mailboxes on the server: the server's answer cannot be used: the answer has no list of mailboxes",
        ),
        (
            || Answer::Missing,
            "Couldn't list the mailboxes on the server: the server's answer holds no result for this call; the server may have stopped before reaching it",
        ),
    ];
    for (reply, said) in cases {
        // Asked for the list: there is none to hand on; the mailboxes it was told are read (the Inbox is not known).
        let mut asked = light(500, &["MA", "MB"]);
        asked.list_folders = true;
        let pieces = run_fake(&broken(reply), &asked);
        assert!(pieces.iter().all(|p| p.listing.is_none()));
        assert_eq!(folders_read(&pieces), ["MA", "MB"]);
        let end = last(&pieces);
        assert_eq!((end.errors.clone(), end.whole), (vec![said.to_string()], false));
        // A run that reads them all has no mailbox to read.
        let pieces = run_fake(&broken(reply), &everything(500));
        assert_eq!((pieces.len(), last(&pieces).errors.clone()), (1, vec![said.to_string()]));
    }
}

#[test]
fn a_call_the_server_does_not_answer_fails_its_mailbox_alone() {
    // The second answer of the first window it is asked for is missing: the Inbox's.
    let missed = Rc::new(std::cell::Cell::new(false));
    let fake = Fake::new(move |req| {
        let missed = missed.clone();
        serve(req, move |name, args| match name {
            "Mailbox/get" => Answer::Is(json!({"accountId": "A1", "list": [
                {"id": "M1", "name": "Inbox", "role": "inbox"}, {"id": "M2", "name": "Other"}]})),
            "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": ["E1"], "total": 1})),
            _ if args.get("#ids").is_some() && !missed.replace(true) => Answer::Missing,
            _ => Answer::Is(emails(&["E1"])),
        })
    });
    let pieces = run_fake(&fake, &everything(500));
    assert_eq!(folders_read(&pieces), ["M2"]);
    assert_eq!(
        last(&pieces).errors,
        ["Couldn't read the mail in 1 mailbox: the server's answer holds no result for this call; the server may have stopped before reaching it"]
    );
}

#[test]
fn a_server_that_refuses_the_credentials_or_is_busy_with_a_header_is_told_apart() {
    let at = |status: u16, retry_after: Option<u32>| {
        Fake::new(move |req| if req.method() == Method::Get { serve(req, |_, _| Answer::Missing) } else { status_reply(status, retry_after) })
    };
    let end = last(&run_fake(&at(401, None), &everything(500))).clone();
    assert!(end.auth_refused && end.errors.is_empty() && !end.whole);
    let end = last(&run_fake(&at(401, None), &light(500, &[]))).clone();
    assert!(end.auth_refused);
    // A 403, whether the server says it is the credentials (RFC 9457 §3.1.2: what the origin meant) or not.
    let token_refused = Fake::new(|req| {
        if req.method() == Method::Get { serve(req, |_, _| Answer::Missing) } else { json_reply(403, &json!({"status": 401, "detail": "expired"})) }
    });
    assert!(last(&run_fake(&token_refused, &everything(500))).auth_refused);
    let end = last(&run_fake(&at(403, None), &everything(500))).clone();
    assert!(end.auth_refused && end.errors.is_empty() && !end.whole);
    let end = last(&run_fake(&at(429, Some(45)), &everything(500))).clone();
    assert_eq!((end.busy_for, end.auth_refused), (Some(45), false));
    let end = last(&run_fake(&at(503, None), &everything(500))).clone();
    assert_eq!(end.busy_for, Some(30));
    // Any other status says the server can not be had: at the list, the run ends there.
    let fake = at(500, None);
    let pieces = run_fake(&fake, &light(500, &["MA", "MB", "MC", "MD", "ME"]));
    let end = last(&pieces);
    assert_eq!(pieces.len(), 1);
    assert_eq!(
        end.errors,
        ["Couldn't bring the mail down from the server: the server answered with HTTP status 500, which this program cannot act on"]
    );
    assert_eq!(fake.posts().len(), 1, "and no more was asked of it");
    assert!(!end.whole && end.busy_for.is_none() && !end.auth_refused);
}

#[test]
fn a_network_that_goes_away_ends_the_run_at_the_list_or_after_three_mailboxes() {
    // Gone from the first request: the list is not had, and nothing more is asked.
    let gone = Fake::new(|req| if req.method() == Method::Get { serve(req, |_, _| Answer::Missing) } else { Err(TransportError::Unreachable) });
    let pieces = run_fake(&gone, &light(500, &["MA", "MB", "MC", "MD"]));
    assert_eq!(pieces.len(), 1);
    assert_eq!(
        last(&pieces).errors,
        [
            "Couldn't bring the mail down from the server: the request did not complete: the server could not be reached; check its address and the network connection"
        ]
    );
    assert_eq!(gone.posts().len(), 1);
    // Gone after the list: three mailboxes fail, and the fourth is not tried.
    let six: Vec<Value> = (1..=6)
        .map(|n| json!({"id": format!("M{n}"), "name": format!("Folder {n}"), "role": if n == 1 { json!("inbox") } else { Value::Null }}))
        .collect();
    let later = Rc::new(std::cell::Cell::new(0));
    let fake = Fake::new(move |req| {
        let calls = later.replace(later.get() + 1);
        match (req.method().as_str(), calls) {
            ("GET", _) => serve(req, |_, _| Answer::Missing),
            ("POST", 1) => serve(req, |_, _| Answer::Is(json!({"accountId": "A1", "list": six}))),
            _ => Err(TransportError::Unreachable),
        }
    });
    let pieces = run_fake(&fake, &everything(500));
    assert!(pieces[0].listing.is_some() && pages(&pieces).is_empty());
    let end = last(&pieces);
    assert_eq!(
        end.errors,
        [
            "Couldn't read the mail in 3 mailboxes: the request did not complete: the server could not be reached; check its address and the network connection",
            "The server failed 3 mailboxes in a row; the rest wait for the next run."
        ]
    );
    assert!(!end.whole);
    assert_eq!(fake.posts().len(), 1 + 3);
    // An exchange the program called off because the owner pressed Cancel is the run stopped, and no trouble of the
    // server's; one called off for another reason is a server that could not be had.
    let cancel = Rc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let aborted = Fake::new(move |req| {
        if req.method() == Method::Get {
            return serve(req, |_, _| Answer::Missing);
        }
        flag.store(true, Ordering::Relaxed);
        Err(TransportError::Aborted)
    });
    let pieces = run_over(aborted.clone(), FAKE_SESSION, &everything(500), &cancel);
    let end = last(&pieces);
    assert!(end.stopped && end.errors.is_empty() && !end.whole && !end.auth_refused, "{end:?}");
    assert_eq!(aborted.posts().len(), 1);
    let aborted = Fake::new(|req| if req.method() == Method::Get { serve(req, |_, _| Answer::Missing) } else { Err(TransportError::Aborted) });
    let end = last(&run_fake(&aborted, &everything(500))).clone();
    assert!(!end.stopped && !end.whole);
    assert_eq!(
        end.errors,
        ["Couldn't bring the mail down from the server: the request did not complete: the request was cancelled before it completed"]
    );
}

#[test]
fn no_line_of_a_run_shows_an_address_the_session_the_account_or_a_secret() {
    let account_id = "AYWRhQGV4YW1wbGUub3Jn";
    const LEAKY: &str = "no mailbox for ada@example.org (ada%40example.org) at https://jmap.example.org/jmap/session?account=ada%40example.org for AYWRhQGV4YW1wbGUub3Jn with hunter2 and tk-secret-123; ask bob@example.net or <carol@example.com> or dave%40example.edu";
    // (Each sign-in has its own secret to keep out of what the server echoes.)
    for (credentials, secret) in
        [(Credentials::basic(ME, &made_secret("hunter2")).unwrap(), "hunter2"), (Credentials::bearer("tk-secret-123").unwrap(), "tk-secret-123")]
    {
        let api = |answer: fn() -> Reply| -> Box<dyn Fn(&HttpRequest) -> Reply> {
            Box::new(move |req| if req.method() == Method::Get { serve(req, |_, _| Answer::Missing) } else { answer() })
        };
        let doors: [(&str, Box<dyn Fn(&HttpRequest) -> Reply>); 5] = [
            ("a method error", Box::new(|req| serve(req, |_, _| Answer::Error("serverFail", LEAKY.to_string())))),
            ("an HTTP error", api(|| json_reply(500, &json!({"detail": LEAKY})))),
            ("a conflict", api(|| json_reply(409, &json!({"detail": LEAKY})))),
            ("a request refused", api(|| json_reply(400, &json!({"type": "urn:ietf:params:jmap:error:notRequest", "detail": LEAKY})))),
            ("a limit", api(|| json_reply(400, &json!({"type": "urn:ietf:params:jmap:error:limit", "limit": "maxCallsInRequest", "detail": LEAKY})))),
        ];
        for (door, answer) in doors {
            let fake = Fake::new(move |req| {
                // The account's id is made of the address, as some servers make it.
                if req.method() == Method::Get {
                    let mut session = serde_json::from_slice::<Value>(serve(req, |_, _| Answer::Missing).unwrap().body()).unwrap();
                    session["accounts"] = json!({account_id: session["accounts"]["A1"].clone()});
                    session["primaryAccounts"][MAIL] = json!(account_id);
                    return json_reply(200, &session);
                }
                answer(req)
            });
            let client = block_on(Connection::connect(fake, FAKE_SESSION, credentials.clone())).unwrap();
            let mut asked = light(500, &["MA", "MB", "MC"]);
            asked.list_folders = true;
            let mut pieces = Vec::new();
            block_on(sync_mail(&client, &asked, &AtomicBool::new(false), &mut |piece| pieces.push(piece)));
            let shown = format!("{:?}", last(&pieces).errors);
            assert!(!last(&pieces).errors.is_empty(), "{door}");
            let hidden =
                ["ada", "example.org", "example.net", "example.com", "example.edu", secret, "account=", "AYWRhQ", "@", "%40", "jmap/session"];
            for (n, hidden) in hidden.into_iter().enumerate() {
                assert!(!shown.contains(hidden), "{door}: what is hidden ({n}) shows in a line");
            }
        }
    }
}

#[test]
fn an_answer_is_held_to_the_size_its_request_can_need() {
    // The sizes asked for: the list of mailboxes, a window, the emails of a message list. Each request carries its
    // own, which the exchange reads before the body, and each is less than the most any answer to the API may be.
    let seen = with_query(json!({"accountId": "A1", "ids": ["E1"], "total": 1}));
    run_fake(&seen, &light(500, &[]));
    let posts: Vec<usize> = seen.sent.borrow().iter().filter(|req| req.method() == Method::Post).map(HttpRequest::max_response_bytes).collect();
    assert_eq!(posts, [LIST_BYTES, WINDOW_BYTES, DETAILS_BYTES]);
    const { assert!(LIST_BYTES < API_RESPONSE_LIMIT && WINDOW_BYTES < LIST_BYTES && DETAILS_BYTES < API_RESPONSE_LIMIT) };

    // A list of mailboxes larger than that is no list: said, and the mailboxes the request names are read all the same.
    let padding = "x".repeat(LIST_BYTES + 1);
    let big_list = Fake::new(move |req| {
        serve(req, |name, _| match name {
            "Mailbox/get" => Answer::Is(json!({"accountId": "A1", "list": [], "padding": padding})),
            "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": [], "total": 0})),
            _ => Answer::Is(emails(&[])),
        })
    });
    let mut asked = light(500, &["MA"]);
    asked.list_folders = true;
    let pieces = run_fake(&big_list, &asked);
    assert_eq!(folders_read(&pieces), ["MA"]);
    assert!(
        last(&pieces).errors[0]
            == "Couldn't list the mailboxes on the server: the server's answer cannot be used: it is larger than the size limit set for this request",
        "{:?}",
        last(&pieces).errors
    );

    // So is the answer for a window, and the emails of a message list.
    let padding = "x".repeat(WINDOW_BYTES + 1);
    let big_window = Fake::new(move |req| {
        serve(req, |name, _| match name {
            "Mailbox/get" => Answer::Is(inbox_only()),
            "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": ["E1"], "total": 1, "padding": padding})),
            _ => Answer::Is(emails(&["E1"])),
        })
    });
    let pieces = run_fake(&big_window, &light(500, &[]));
    assert!(pages(&pieces).is_empty());
    assert_eq!(
        last(&pieces).errors,
        ["Couldn't read the mail in 1 mailbox: the server's answer cannot be used: it is larger than the size limit set for this request"]
    );
    // And the emails of a message list.
    let padding = "x".repeat(DETAILS_BYTES + 1);
    let big_details = Fake::new(move |req| {
        serve(req, |name, args| match name {
            "Mailbox/get" => Answer::Is(inbox_only()),
            "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": ["E1"], "total": 1})),
            _ if wants_headers(args) => {
                let mut got = emails(&["E1"]);
                got["padding"] = json!(padding);
                Answer::Is(got)
            }
            _ => Answer::Is(emails(&["E1"])),
        })
    });
    let pieces = run_fake(&big_details, &light(500, &[]));
    assert_eq!(
        last(&pieces).errors,
        ["Couldn't read 1 message in full: the server's answer cannot be used: it is larger than the size limit set for this request"]
    );
}

#[test]
fn a_run_that_could_not_begin_ends_as_a_run_would_have() {
    let asked = JmapMailRequest { account: ID, email: ME.into(), session: FAKE_SESSION.into(), ..Default::default() };
    let refused = failed_run(&asked, &Error::CredentialsRefused);
    assert_eq!((refused.account, refused.email.as_str(), refused.auth_refused, refused.partial, refused.whole), (ID, ME, true, false, false));
    assert_eq!(refused.session, FAKE_SESSION);
    assert!(refused.errors.is_empty() && refused.busy_for.is_none());
    let busy = failed_run(&asked, &Error::Busy { retry_after: RetryAfter::parse("9") });
    assert_eq!((busy.busy_for, busy.auth_refused), (Some(9), false));
    assert_eq!(busy.errors, ["Couldn't reach the server: the server is busy and asks to wait 9 s before the next attempt"]);
    assert_eq!(failed_run(&asked, &Error::Busy { retry_after: None }).busy_for, Some(30));
    // A date can't be waited for here: the default wait.
    assert_eq!(failed_run(&asked, &Error::Busy { retry_after: RetryAfter::parse("Sat, 29 Oct 1994 19:43:31 GMT") }).busy_for, Some(30));
    // Anything else is a line, with no address in it: neither one the connection knows nor another.
    let said = format!("no route to {FAKE_SESSION} for ada@example.org, ask bob@example.net");
    let fake = Fake::new(move |_| json_reply(500, &json!({"detail": said})));
    let error = block_on(Connection::connect(fake, FAKE_SESSION, Credentials::bearer(TOKEN).unwrap())).unwrap_err();
    let down = failed_run(&asked, &error);
    assert_eq!(
        down.errors,
        [
            "Couldn't reach the server: the server answered with HTTP status 500, which this program cannot act on (server said: \"no route to [redacted] for [redacted], ask [redacted]"
        ]
    );
    assert!(!down.auth_refused && down.busy_for.is_none() && !down.whole);
    // And it is what the run of a client that signed in would have said.
    for refused in
        [json_reply(401, &json!({})), json_reply(403, &json!({})), json_reply(403, &json!({"type": "urn:ietf:params:oauth:error:invalid_token"}))]
    {
        let fake = Fake::new(move |_| refused.clone());
        let error = block_on(Connection::connect(fake, FAKE_SESSION, Credentials::bearer(TOKEN).unwrap())).unwrap_err();
        assert!(failed_run(&asked, &error).auth_refused, "{error:?}");
    }
}

#[test]
fn a_run_reads_at_most_a_thousand_mailboxes() {
    let list: Vec<Value> = (0..1200)
        .map(|n| json!({"id": format!("M{n}"), "name": format!("Folder {n}"), "role": if n == 0 { json!("inbox") } else { Value::Null }}))
        .collect();
    let fake = Fake::new(move |req| {
        serve(req, |name, _| match name {
            "Mailbox/get" => Answer::Is(json!({"accountId": "A1", "list": list})),
            "Email/query" => Answer::Is(json!({"accountId": "A1", "ids": [], "total": 0})),
            _ => Answer::Is(emails(&[])),
        })
    });
    let pieces = run_fake(&fake, &everything(500));
    let read = folders_read(&pieces);
    assert_eq!((read.len(), read[0]), (1000, "M0"));
    assert_eq!(pieces.iter().filter_map(|p| p.progress.as_ref()).next_back().map(|p| (p.number, p.folders)), Some((1000, 1000)));
    let end = last(&pieces);
    assert_eq!(end.errors, ["200 mailboxes were not read: a run reads 1000 at most."]);
    assert!(!end.whole);
    // The list that went on is the whole of the 1200.
    assert_eq!(pieces[0].listing.as_ref().map(|l| l.folders.len()), Some(1200));
}

/// A value that is what it should be in most places and anything at all in the others.
fn mostly(valid: Value) -> impl Strategy<Value = Value> {
    prop_oneof![9 => Just(valid), 1 => any_json()]
}

fn one_of(words: &[&'static str]) -> BoxedStrategy<Value> {
    proptest::sample::select(words.to_vec()).prop_map(Value::from).boxed()
}

/// A mailbox with what it takes to be one in some places, and anything at all in the others.
fn mailbox_like() -> impl Strategy<Value = Value> {
    let id = prop_oneof![4 => one_of(&["M1", "M2", "M3", "M4", "M5", "bad id", ""]), 1 => any_json()].boxed();
    let role = prop_oneof![
        4 => one_of(&["inbox", "sent", "drafts", "trash", "junk", "archive", "all", "flagged", "important", "subscribed", "other", "INBOX"]),
        2 => Just(Value::Null),
        1 => any_json(),
    ];
    (id.clone(), mostly(json!("A name")), id, role, mostly(json!(7)), mostly(json!(2)))
        .prop_map(|(id, name, parent, role, total, unread)| json!({"id": id, "name": name, "parentId": parent, "role": role, "totalEmails": total, "unreadEmails": unread}))
}

/// Three mailboxes, `M1` to `M3`, whose other fields are what a hostile server makes of them, and some more
/// that may be anything.
fn mailboxes_like() -> impl Strategy<Value = Value> {
    let more = proptest::collection::vec(prop_oneof![5 => mailbox_like(), 1 => any_json()], 0..5);
    (proptest::collection::vec(mailbox_like(), 3), more).prop_map(|(mut list, more)| {
        for (n, mailbox) in list.iter_mut().enumerate() {
            if let Some(map) = mailbox.as_object_mut() {
                map.insert("id".into(), json!(format!("M{}", n + 1)));
            }
        }
        list.extend(more);
        json!({"accountId": "A1", "list": list})
    })
}

fn query_like() -> impl Strategy<Value = Value> {
    // Mostly some of the emails the other answers have, sometimes ids that are none, and sometimes anything.
    let ids = prop_oneof![
        6 => proptest::sample::subsequence(vec!["E1", "E2", "E3", "E4", "E5", "E6"], 0..=6).prop_map(|ids| json!(ids)),
        1 => proptest::collection::vec(prop_oneof![one_of(&["E1", "E2", "E1", "bad id", ""]), any_json()], 0..9).prop_map(Value::Array),
        1 => any_json(),
    ];
    (ids, mostly(json!(9)), mostly(json!(4)), mostly(json!(0)))
        .prop_map(|(ids, total, limit, position)| json!({"accountId": "A1", "ids": ids, "total": total, "limit": limit, "position": position}))
}

/// The raw header fields of an email, with names that matter, values with folds and encoded words in them, and junk.
fn headers_like() -> impl Strategy<Value = Value> {
    let name = prop_oneof![5 => one_of(&["Subject", "From", "To", "Cc", "Date", "Message-ID", "subject", "TO", "X-Other"]), 1 => any_json()];
    let value = prop_oneof![
        3 => "[ -~]{0,60}".prop_map(Value::from),
        3 => one_of(&[
            " =?UTF-8?B?Q2Fmw6k=?= =?utf-8?q?caf=C3=A9?=", " a\r\n b\r\n\tc", " Zoe <zoe@example.org>, \"Park, B\" <bob@example.org> (Bob)",
            " Thu, 15 Oct 2026 09:30:00 +0200", " <id@example.org>", " x\r\n\r\nSubject: evil", " \u{0}\u{7f}\u{85}",
        ]),
        1 => "(.|\n){0,3000}".prop_map(Value::from),
        1 => any_json(),
    ];
    let entry = prop_oneof![5 => (name, value).prop_map(|(name, value)| json!({"name": name, "value": value})), 1 => any_json()];
    prop_oneof![4 => proptest::collection::vec(entry, 0..14).prop_map(Value::Array), 1 => any_json()]
}

fn email_like() -> impl Strategy<Value = Value> {
    let keywords = prop_oneof![9 => Just(json!({"$seen": true, "$Flagged": true, "$answered": false, "$draft": true})), 1 => any_json()];
    // (Whose it is depends on the mailbox that is read: M1, M2 or M3, or none that is said, or any.)
    let mailbox_ids = prop_oneof![
        3 => Just(json!({"M1": true})),
        2 => Just(json!({"M2": true})),
        2 => Just(Value::Null),
        1 => Just(json!({"M0": true, "M1": true})),
        1 => Just(json!({"M3": true, "M2": true})),
        1 => any_json(),
    ];
    let preview = prop_oneof![9 => "(.|\n){0,400}".prop_map(Value::from), 1 => any_json()];
    let received = prop_oneof![
        7 => one_of(&["2026-10-15T07:30:00Z", "2026-10-15T09:30:00+02:00", "0001-01-01T00:00:00Z", "99999-01-01T00:00:00Z"]),
        // (Text that is not ASCII, cut anywhere, is no date and no crash.)
        2 => "\\PC{0,70}".prop_map(Value::from),
        1 => any_json(),
        1 => Just(Value::Null),
    ];
    ((keywords, mailbox_ids, preview), (mostly(json!(1234)), received, mostly(json!(true)), headers_like())).prop_map(
        |((keywords, mailbox_ids, preview), (size, received, attachment, headers))| {
            json!({"id": "E0", "keywords": keywords, "mailboxIds": mailbox_ids, "preview": preview, "size": size, "receivedAt": received, "hasAttachment": attachment, "headers": headers})
        },
    )
}

/// Six emails, `E1` to `E6`, whose contents are what a hostile server makes of them.
fn emails_like() -> impl Strategy<Value = Value> {
    proptest::collection::vec(prop_oneof![8 => email_like(), 1 => any_json()], 6).prop_map(|mut list| {
        for (n, email) in list.iter_mut().enumerate() {
            if let Some(map) = email.as_object_mut() {
                map.insert("id".into(), json!(format!("E{}", n + 1)));
            }
        }
        json!({"accountId": "A1", "list": list, "notFound": []})
    })
}

/// What must hold of any listing: a name each, parents before what is in them, no level too many.
fn check_listing(listing: &FolderListing) {
    assert!(listing.folders.len() <= mailcraft_model::sync::MAX_LISTED_FOLDERS);
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (n, folder) in listing.folders.iter().enumerate() {
        assert!(is_id(&folder.id) && !folder.name.is_empty() && folder.name.chars().count() <= mailcraft_model::MAX_NAME);
        assert!(!folder.name.chars().any(char::is_control) && folder.name == folder.name.trim());
        assert!(index.insert(folder.id.as_str(), n).is_none(), "an id twice");
        assert!(folder.parent.is_empty() || index.get(folder.parent.as_str()).is_some_and(|parent| *parent < n), "a parent after what is in it");
    }
    for folder in &listing.folders {
        let (mut depth, mut at) = (1, folder);
        while let Some(parent) = index.get(at.parent.as_str()).and_then(|p| listing.folders.get(*p)) {
            (depth, at) = (depth + 1, parent);
        }
        assert!(depth <= folders::MAX_DEPTH, "{depth} levels");
    }
}

/// What must hold of a message, whatever the server said of it.
fn check_message(m: &GraphMessage) {
    assert!(m.subject.chars().count() <= 998 && m.preview.chars().count() <= 256 && m.message_id.chars().count() <= 998);
    assert!(m.to.len() <= 500 && m.cc.len() <= 500);
    for text in [&m.subject, &m.preview, &m.message_id, &m.from.name, &m.from.email] {
        assert!(!text.chars().any(char::is_control), "a control character in {text:?}");
    }
    for person in m.to.iter().chain(&m.cc) {
        assert!(person.name.chars().count() <= 256 && person.email.chars().count() <= 320 && !(person.name.is_empty() && person.email.is_empty()));
        assert!(!person.name.chars().chain(person.email.chars()).any(char::is_control));
    }
}

/// What must hold of a run, whatever the server said.
fn sane(pieces: &[JmapMailResult]) {
    well_formed(pieces);
    if let Some(listing) = pieces.first().and_then(|piece| piece.listing.as_ref()) {
        check_listing(listing);
    }
    for page in pages(pieces) {
        assert!(is_id(&page.folder) && page.messages.len() <= 500);
        let mut seen = HashSet::new();
        for m in &page.messages {
            assert!(is_id(&m.server.id) && seen.insert(m.server.id.as_str()), "an id that is none, or twice");
            assert_eq!(m.server.folder, page.folder);
            check_message(m);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn whatever_the_server_answers_a_run_ends_with_what_can_be_made_of_it(
        mailboxes in prop_oneof![7 => mailboxes_like(), 1 => any_json()],
        query in prop_oneof![7 => query_like(), 1 => any_json()],
        emails in prop_oneof![7 => emails_like(), 1 => any_json()],
        window in 0..700u32,
    ) {
        let fake = Fake::new(move |req| {
            serve(req, |name, _| match name {
                "Mailbox/get" => Answer::Is(mailboxes.clone()),
                "Email/query" => Answer::Is(query.clone()),
                "Email/get" => Answer::Is(emails.clone()),
                _ => Answer::Missing,
            })
        });
        let mut everywhere = everything(window);
        has(&mut everywhere, "M1", &["E1", "E2", "bad id"]);
        let mut somewhere = light(window, &["M2", "M3", "bad id"]);
        somewhere.list_folders = true;
        has(&mut somewhere, "M2", &["E3"]);
        for asked in [everywhere, somewhere] {
            sane(&run_fake(&fake, &asked));
        }
    }

    #[test]
    fn any_email_is_a_message_within_its_sizes(entry in prop_oneof![7 => email_like(), 1 => any_json()], marks in any::<[bool; 5]>()) {
        let marks = Marks { read: marks[0], flagged: marks[1], answered: marks[2], forwarded: marks[3], draft: marks[4] };
        let m = message::whole("E1", "M1", &entry, marks);
        check_message(&m);
        prop_assert_eq!((m.server.id.as_str(), m.server.folder.as_str(), m.server.read, m.draft), ("E1", "M1", marks.read, marks.draft));
        // Whatever the headers say, the marks and the ids are the ones it was given.
        prop_assert_eq!((m.server.answered, m.server.forwarded, m.server.flag == FlagStatus::Flagged), (marks.answered, marks.forwarded, marks.flagged));
    }

    #[test]
    fn any_headers_make_a_message_and_a_header_cannot_end_in_another(headers in headers_like(), tail in "(.|\n){0,50}") {
        // Whatever the fields hold, one field of ours stays one field: a subject that closes the block and
        // begins a new field would be a From the message did not have.
        let planted = json!([{"name": "Subject", "value": format!(" s\r\n\r\nFrom: planted@example.org{tail}")}]);
        let m = message::whole("E1", "M1", &json!({"headers": planted}), Marks::default());
        prop_assert_eq!(&m.from, &Address::default());
        let m = message::whole("E1", "M1", &json!({"headers": headers}), Marks::default());
        check_message(&m);
        prop_assert!(!m.from.email.contains("planted"));
    }

    #[test]
    fn any_mailbox_list_is_a_tree_or_none(list in prop_oneof![7 => mailboxes_like(), 1 => any_json()]) {
        if let Some(listing) = folders::listing(&list) {
            check_listing(&listing);
        }
    }

    #[test]
    fn whatever_a_server_says_of_the_session_and_the_calls_a_run_ends(noise in proptest::collection::vec(any::<u8>(), 0..80), status in prop_oneof![Just(200u16), any::<u16>()]) {
        // Not JSON, or JSON of no shape: the run ends with a line or a word, never with a panic.
        let fake = Fake::new(move |req| {
            if req.method() == Method::Get { return serve(req, |_, _| Answer::Missing); }
            Ok(HttpResponse::new(status, noise.clone()).with_field("Content-Type", "application/json"))
        });
        let pieces = run_fake(&fake, &everything(500));
        let end = last(&pieces);
        prop_assert!(pieces.len() == 1 && !end.whole && (end.auth_refused || end.busy_for.is_some() || !end.errors.is_empty()));
    }
}

/// A model server and what goes wrong with it, for the property that matters most: a page that says it is the
/// whole of a mailbox lists exactly what the server holds there.
#[derive(Debug, Clone)]
struct World {
    /// Each email: the mailboxes it lies in, a bit for each of five (none: the Inbox).
    emails: Vec<u8>,
    /// Whether a view over all mail, with the smallest id of all, holds every email as well.
    view: bool,
    /// Whether the server names an email by where it lies (one mailbox to an email, the first it is put in).
    by_place: bool,
    window: u32,
    /// For each email: whether the engine has it, and whether it breaks every call for it whole.
    has: Vec<bool>,
    poison: Vec<bool>,
    /// The mailboxes (a bit for each) whose query fails, that can not be opened and say nothing, that list their
    /// emails but can not find them again (at all, or when their headers are asked for), and whose counts the list
    /// of mailboxes leaves out.
    broken: u8,
    hollow: u8,
    forgetful: u8,
    forgetful_headers: u8,
    uncounted: u8,
    /// The first requests, each refused (500) or let through.
    refused: Vec<bool>,
    all: bool,
    /// The mailboxes a light run names besides the Inbox (a bit for each).
    also: u8,
    one_call: bool,
    small_get: bool,
}

const BOXES: usize = 5;

/// The mailboxes (by position) a mask names: the Inbox, if it names none.
fn places(mask: u8) -> Vec<usize> {
    let named: Vec<usize> = (0..BOXES).filter(|b| mask & (1 << b) != 0).collect();
    if named.is_empty() { vec![0] } else { named }
}

impl World {
    /// The mailboxes email number `n` lies in: all it was put in, or on a server that names by place the first.
    fn lies(&self, n: usize) -> Vec<usize> {
        let all = places(self.emails[n]);
        if self.by_place { all.into_iter().take(1).collect() } else { all }
    }

    /// Nothing is wrong with the server.
    fn calm(&self) -> bool {
        !self.refused.iter().any(|refused| *refused)
            && (self.broken, self.hollow, self.forgetful, self.forgetful_headers, self.uncounted) == (0, 0, 0, 0, 0)
            && !self.poison.iter().take(self.emails.len()).any(|poison| *poison)
    }
}

fn worlds() -> impl Strategy<Value = World> {
    (
        (proptest::collection::vec(0u8..32, 0..24), any::<bool>(), any::<bool>(), 1..40u32),
        (proptest::collection::vec(any::<bool>(), 24), proptest::collection::vec(prop::bool::weighted(0.1), 24)),
        (
            prop_oneof![3 => Just(0u8), 1 => 0u8..32],
            prop_oneof![3 => Just(0u8), 1 => 0u8..32],
            prop_oneof![3 => Just(0u8), 1 => 0u8..32],
            prop_oneof![3 => Just(0u8), 1 => 0u8..32],
            prop_oneof![3 => Just(0u8), 1 => 0u8..32],
        ),
        proptest::collection::vec(prop::bool::weighted(0.15), 0..8),
        (any::<bool>(), 0u8..32, any::<bool>(), any::<bool>()),
        any::<bool>(),
    )
        .prop_map(
            |(
                (emails, view, by_place, window),
                (has, poison),
                (broken, hollow, forgetful, forgetful_headers, uncounted),
                refused,
                (all, also, one_call, small_get),
                calm,
            )| {
                let mut world = World {
                    emails,
                    view,
                    by_place,
                    window,
                    has,
                    poison,
                    broken,
                    hollow,
                    forgetful,
                    forgetful_headers,
                    uncounted,
                    refused,
                    all,
                    also,
                    one_call,
                    small_get,
                };
                // A mailbox that can not be opened the second time is one whose emails all go together, as they do on a
                // server whose ids say where their email lies; with ids that do not, an email in two mailboxes is
                // found through the other, and one that is found beside one that is "gone" is no lie to see through.
                if !world.by_place {
                    (world.forgetful, world.forgetful_headers) = (0, 0);
                }
                // Half the servers have nothing wrong with them: the property that a page says so whenever it can.
                if calm {
                    (world.broken, world.hollow, world.forgetful, world.forgetful_headers, world.uncounted) = (0, 0, 0, 0, 0);
                    (world.refused, world.poison) = (Vec::new(), vec![false; 24]);
                }
                world
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn a_page_that_says_it_is_the_whole_of_a_mailbox_lists_what_the_server_holds_there(world in worlds()) {
        let stand_in = StandIn::new();
        let (boxes, emails) = stand_in.with(|d| {
            if world.by_place {
                d.profile = Profile::by_place();
            }
            let boxes: Vec<String> = [("Inbox", Some("inbox")), ("Sent", Some("sent")), ("Drafts", Some("drafts")), ("Projects", None), ("Apollo", None)]
                .into_iter()
                .map(|(name, role)| d.mailbox(name, None, role))
                .collect();
            if world.view {
                let view = d.mailbox("All mail", None, Some("all"));
                if let Some(mailbox) = d.mailboxes.iter_mut().find(|m| m.id == view) {
                    mailbox.id = "A0".into();
                }
            }
            let mut emails = Vec::new();
            for n in 0..world.emails.len() {
                let lies = world.lies(n);
                let id = d.deliver(&boxes[lies[0]], &mail(&format!("m{n}")), &[], &at(n));
                for b in &lies[1..] {
                    d.add_to_mailbox(&id, &boxes[*b]);
                }
                if world.view {
                    d.add_to_mailbox(&id, "A0");
                }
                emails.push(id);
            }
            (boxes, emails)
        });
        let get_limit = if world.small_get { 7 } else { 500 };
        stand_in.with(|d| {
            d.max_calls = if world.one_call { 1 } else { 16 };
            d.max_get = get_limit;
            for (n, id) in emails.iter().enumerate() {
                if world.poison[n] {
                    d.broken_emails.insert(id.clone());
                }
            }
            for (b, id) in boxes.iter().enumerate() {
                let bit = 1 << b;
                if world.broken & bit != 0 {
                    d.broken_mailboxes.insert(id.clone(), "serverFail");
                }
                if world.hollow & bit != 0 {
                    d.hollow.insert(id.clone());
                }
                if world.forgetful & bit != 0 {
                    d.forgetful.insert(id.clone());
                }
                if world.forgetful_headers & bit != 0 {
                    d.forgetful_headers.insert(id.clone());
                }
                if world.uncounted & bit != 0 {
                    d.uncounted.insert(id.clone());
                }
            }
            d.refuse_next = world.refused.iter().map(|refused| refused.then_some((500, None))).collect();
        });
        // The mailbox that lists an email is the first of those it lies in: the Inbox, then the special ones in the
        // folder pane's order (Drafts before Sent), then the one with the smallest id.
        let ranks = [FolderKind::Inbox, FolderKind::Sent, FolderKind::Drafts, FolderKind::Custom, FolderKind::Custom].map(rank);
        let owner_of = |n: usize| world.lies(n).into_iter().min_by(|a, b| (ranks[*a], &boxes[*a]).cmp(&(ranks[*b], &boxes[*b]))).unwrap_or(0);
        let mut request = if world.all {
            everything(world.window)
        } else {
            let named: Vec<&str> = (0..BOXES).filter(|b| world.also & (1 << b) != 0).map(|b| boxes[b].as_str()).collect();
            light(world.window, &named)
        };
        for (n, id) in emails.iter().enumerate() {
            if world.has[n] {
                has(&mut request, &boxes[owner_of(n)], &[id.as_str()]);
            }
        }
        has(&mut request, &boxes[0], &["Estale1", "Estale2"]);

        let pieces = run(&stand_in, &request);
        sane(&pieces);
        let effective = (world.window as usize).min(500).min(get_limit);
        let calm = world.calm();
        for page in pages(&pieces) {
            let b = boxes.iter().position(|id| *id == page.folder).unwrap();
            let owned: HashSet<&str> = (0..emails.len()).filter(|n| owner_of(*n) == b).map(|n| emails[n].as_str()).collect();
            let listed: HashSet<&str> = ids_of(page).into_iter().collect();
            // A page lists what its mailbox lists, and what it lists twice is nothing more.
            prop_assert!(listed.is_subset(&owned), "{} lists what it does not own: {listed:?} of {owned:?}", page.folder);
            // And a page that says it is the whole of the mailbox lists all of it.
            if page.afresh {
                prop_assert_eq!(&listed, &owned, "{} says it is the whole mailbox", page.folder);
            }
            // With nothing wrong, it says so whenever it can: the server holds no more there than the window, and
            // none of it is another mailbox's.
            if calm {
                let here = (0..emails.len()).filter(|n| world.lies(*n).contains(&b)).count();
                prop_assert_eq!(page.afresh, here == owned.len() && here <= effective, "{}: {} here, {} its own, a window of {}", page.folder, here, owned.len(), effective);
            }
        }
        if calm {
            // Every mailbox the run meant to read was read, once, and the run says all went well.
            let wanted: HashSet<&str> = if world.all {
                boxes.iter().map(String::as_str).collect()
            } else {
                std::iter::once(0).chain((0..BOXES).filter(|b| world.also & (1 << b) != 0)).map(|b| boxes[b].as_str()).collect()
            };
            let read = folders_read(&pieces);
            prop_assert_eq!(read.iter().copied().collect::<HashSet<&str>>(), wanted);
            prop_assert_eq!(read.len(), read.iter().collect::<HashSet<_>>().len());
            prop_assert!(last(&pieces).whole && last(&pieces).errors.is_empty(), "{:?}", last(&pieces).errors);
        }
    }
}
