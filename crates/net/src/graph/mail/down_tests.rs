// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's mail down through the strict Graph stand-in (R3.2). Nothing here leaves
//! this machine; every message is invented.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mailcraft_model::sync::{GraphLink, GraphMailRequest, GraphMailResult};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

fn request(links: Vec<GraphLink>) -> GraphMailRequest {
    GraphMailRequest { account: 3, email: "owner@example.org".into(), links, ..Default::default() }
}

fn run(server: &StandIn, links: Vec<GraphLink>, cancel: &AtomicBool) -> Vec<GraphMailResult> {
    whole_run(&server.url(), TOKEN, &request(links), Arc::new(Gate::new()), QUICK, cancel)
}

/// The links a client keeps after `pieces`, as the mailbox file would: each folder's latest.
fn links_after(pieces: &[GraphMailResult], mut links: Vec<GraphLink>) -> Vec<GraphLink> {
    for page in pieces.iter().flat_map(|p| &p.pages) {
        links.retain(|l| l.folder != page.folder);
        links.push(GraphLink { folder: page.folder.clone(), link: page.link.clone(), caught_up: page.caught_up, counts: page.counts });
    }
    links
}

/// The pages' folders in the order they came.
fn folders_in_order(pieces: &[GraphMailResult]) -> Vec<String> {
    pieces.iter().flat_map(|p| &p.pages).map(|p| p.folder.clone()).collect()
}

#[test]
fn the_folders_come_first_then_the_newest_page_of_every_folder_then_the_rest() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let archive = server.state().well_known("archive");
    let projects = server.state().add_folder("Projects", None);
    {
        let mut state = server.state();
        state.mailbox.generate(&inbox, 250, NOW);
        state.mailbox.generate(&archive, 150, NOW);
        state.mailbox.generate(&projects, 5, NOW);
    }
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let first = pieces.first().unwrap();
    assert!(first.listing.is_some() && first.partial && first.pages.is_empty());
    let last = pieces.last().unwrap();
    assert!(!last.partial && last.whole && last.errors.is_empty(), "{:?}", last.errors);
    let order = folders_in_order(&pieces);
    // Inbox, Archive and Projects once each (and the empty special folders), then the rest of the
    // Inbox and the Archive.
    let at = |id: &str| order.iter().position(|f| f == id).unwrap();
    assert!(at(&inbox) < at(&archive) && at(&archive) < at(&projects));
    let second_inbox = order.iter().enumerate().filter(|(_, f)| **f == inbox).nth(1).map(|(n, _)| n).unwrap();
    assert!(second_inbox > at(&projects), "{order:?}");
    // Every message once, newest first in its folder; the Inbox's counts for progress.
    let inbox_messages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).flat_map(|p| &p.messages).collect();
    assert_eq!(inbox_messages.len(), 250);
    assert!(inbox_messages.windows(2).all(|w| w[0].received >= w[1].received));
    assert_eq!(inbox_messages[0].subject, "Message 0");
    assert_eq!(inbox_messages[0].server.folder, inbox);
    let progress = pieces.iter().filter_map(|p| p.progress.as_ref()).find(|p| p.folder == inbox).unwrap();
    assert_eq!(progress.total, 250);
    // Every folder's last page ends its round with a delta link.
    let links = links_after(&pieces, Vec::new());
    assert!(links.iter().all(|l| l.caught_up && l.link.contains("$deltatoken=")));

    // Run again from the links: nothing but what changed.
    let gone = server.state().mailbox.messages.values().find(|m| m.folder == inbox).unwrap().id.clone();
    server.state().mailbox.delete(&gone);
    let again = run(&server, links, &AtomicBool::new(false));
    let pages: Vec<_> = again.iter().flat_map(|p| &p.pages).collect();
    assert_eq!(pages.iter().map(|p| p.messages.len()).sum::<usize>(), 0);
    assert_eq!(pages.iter().flat_map(|p| &p.removed).collect::<Vec<_>>(), [&gone]);
    assert!(again.last().unwrap().whole);
}

#[test]
fn cancel_stops_after_the_page_in_hand_and_the_next_run_goes_on_from_there() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 500, NOW);
    let cancel = AtomicBool::new(false);
    let mut pieces = Vec::new();
    super::sync_mail_with(&server.url(), TOKEN, &request(Vec::new()), Arc::new(Gate::new()), QUICK, &cancel, &mut |piece| {
        // Stop when the second page of the Inbox is in hand.
        if piece.pages.iter().filter(|p| p.folder == inbox).count() > 0
            && pieces.iter().flat_map(|p: &GraphMailResult| &p.pages).filter(|p| p.folder == inbox).count() == 1
        {
            cancel.store(true, Ordering::Relaxed);
        }
        pieces.push(piece);
    });
    let last = pieces.last().unwrap();
    assert!(!last.whole && !last.partial);
    assert!(last.stopped && last.errors.is_empty());
    let got: usize = pieces.iter().flat_map(|p| &p.pages).map(|p| p.messages.len()).sum();
    assert_eq!(got, 200);
    let links = links_after(&pieces, Vec::new());
    let rest = run(&server, links, &AtomicBool::new(false));
    let more: Vec<String> = rest.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).map(|m| m.server.id.clone()).collect();
    assert_eq!(more.len(), 300, "nothing twice, nothing missed");
    assert!(rest.last().unwrap().whole);
}

#[test]
fn a_link_no_longer_taken_or_not_graphs_own_starts_the_folder_afresh() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 30, NOW);
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let links = links_after(&pieces, Vec::new());

    server.state().mailbox.forget_links();
    let again = run(&server, links.clone(), &AtomicBool::new(false));
    let first_inbox = again.iter().flat_map(|p| &p.pages).find(|p| p.folder == inbox).unwrap();
    assert!(first_inbox.afresh && first_inbox.messages.len() == 30);
    assert!(again.last().unwrap().whole);

    server.state().mailbox.resets = 1;
    let links = links_after(&again, links);
    let reset = run(&server, links.clone(), &AtomicBool::new(false));
    assert!(reset.iter().flat_map(|p| &p.pages).any(|p| p.folder == inbox && p.afresh));

    // A link the mailbox file says goes elsewhere is never followed.
    let astray: Vec<GraphLink> = links.into_iter().map(|l| GraphLink { link: l.link.replace("127.0.0.1", "graph.example.org"), ..l }).collect();
    let before = server.state().requests.len();
    let fresh = run(&server, astray, &AtomicBool::new(false));
    assert!(fresh.iter().flat_map(|p| &p.pages).any(|p| p.folder == inbox && p.afresh));
    assert!(server.state().requests[before..].iter().all(|r| !r.target.contains("graph.example.org")));
}

#[test]
fn a_busy_server_ends_the_run_with_how_long_to_wait() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 30, NOW);
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let links = links_after(&pieces, Vec::new());
    {
        let mut state = server.state();
        state.retry_after = Some(600);
        for _ in 0..10 {
            state.fail_next.push((format!("GET /v1.0/me/mailFolders/{inbox}/messages/delta"), 429, "TooManyRequests".into()));
        }
    }
    let busy = run(&server, links, &AtomicBool::new(false));
    let last = busy.last().unwrap();
    assert!(!last.whole);
    assert!(last.busy_for.is_some_and(|seconds| seconds >= 590), "{last:?}");
}

#[test]
fn a_round_begun_in_an_earlier_run_is_followed_by_one_that_brings_what_changed_since() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 300, NOW);
    let cancel = AtomicBool::new(false);
    let mut pieces = Vec::new();
    super::sync_mail_with(&server.url(), TOKEN, &request(Vec::new()), Arc::new(Gate::new()), QUICK, &cancel, &mut |piece| {
        if piece.pages.iter().any(|p| p.folder == inbox) {
            cancel.store(true, Ordering::Relaxed);
        }
        pieces.push(piece);
    });
    // The newest came; then it is deleted there.
    assert!(pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).any(|m| m.server.id == ids[0]));
    server.state().mailbox.delete(&ids[0]);
    let links = links_after(&pieces, Vec::new());
    let rest = run(&server, links, &AtomicBool::new(false));
    assert!(rest.last().unwrap().whole);
    assert!(rest.iter().flat_map(|p| &p.pages).any(|p| p.removed.contains(&ids[0])), "the deletion comes in the round after");
}

#[test]
fn messages_come_without_their_size_and_are_fetched_whole_by_their_immutable_id() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 3, NOW);
    server.state().mailbox.update(&ids[1], |m| m.size = 30 << 20);
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let sizes: Vec<u64> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).map(|m| m.server.size).collect();
    // A delta can't be asked for the sizes: none is said.
    assert_eq!(sizes, [0, 0, 0]);

    server.state().mailbox.delete(&ids[2]);
    let request = mailcraft_model::sync::GraphFetchRequest {
        account: 3,
        email: "owner@example.org".into(),
        ids: ids.clone(),
        background: true,
        ..Default::default()
    };
    let got = super::fetch::fetch_messages_with(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    assert_eq!(got.fetched.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [ids[0].as_str(), ids[1].as_str()]);
    assert!(String::from_utf8_lossy(&got.fetched[0].mime).contains("Subject: Message 0\r\n"));
    assert_eq!(got.missing, [ids[2].clone()]);
    assert!(got.errors.is_empty() && !got.offline);
    // Every one asked for by its immutable id.
    assert!(server.state().requests.iter().filter(|r| r.target.contains("$value")).all(|r| r.header("prefer").contains("ImmutableId")));

    // Cancel: nothing more is asked.
    let cancelled = super::fetch::fetch_messages_with(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(true));
    assert!(cancelled.fetched.is_empty());
}

#[test]
fn a_delta_is_never_asked_to_expand_the_sizes_which_exchange_online_refuses() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 3, NOW);
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let last = pieces.last().unwrap();
    assert!(!last.partial && last.whole && last.errors.is_empty(), "{:?}", last.errors);
    let messages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).collect();
    assert_eq!(messages.len(), 3);
    assert!(messages.iter().all(|m| m.server.size == 0), "a size not said is 0");
    let deltas = server.state().requests.iter().filter(|r| r.target.contains("/messages/delta")).count();
    assert!(deltas > 1 && server.state().requests.iter().all(|r| !(r.target.contains("/messages/delta") && r.target.contains("expand"))));
}

#[test]
fn folders_the_server_refuses_to_list_are_said_with_graphs_code() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    {
        let mut state = server.state();
        state.mailbox.generate(&inbox, 3, NOW);
        state.mailbox.refuse_delta = true;
    }
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let last = pieces.last().unwrap();
    assert!(!last.partial && !last.whole);
    assert_eq!(last.errors.len(), 1, "{:?}", last.errors);
    let said = &last.errors[0];
    assert!(said.starts_with("Microsoft Graph refused to list the mail in ") && said.contains("folders (ErrorInvalidRequest)"), "{said}");
    assert!(!said.contains(&inbox), "no folder's id: {said}");
    assert!(pieces.iter().all(|p| p.pages.is_empty()));
}

fn fetch_request(ids: &[String]) -> mailcraft_model::sync::GraphFetchRequest {
    mailcraft_model::sync::GraphFetchRequest {
        account: 3,
        email: "owner@example.org".into(),
        ids: ids.to_vec(),
        background: true,
        ..Default::default()
    }
}

#[test]
fn a_message_the_server_refuses_is_passed_over_and_the_rest_still_come() {
    use crate::graph::standin::Fault;
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 3, NOW);
    server.state().mailbox.faults.insert(ids[0].clone(), Fault::Invalid);
    let got = super::fetch::fetch_messages_with(&server.url(), TOKEN, &fetch_request(&ids), Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    // The first one held up the whole batch before: now it is named, and the others come.
    assert_eq!(got.failed, [ids[0].clone()]);
    assert_eq!(got.fetched.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [ids[1].as_str(), ids[2].as_str()]);
    assert!(got.errors.is_empty() && got.missing.is_empty() && !got.offline && got.busy_for.is_none(), "{:?}", got.errors);
}

#[test]
fn a_message_over_16_mb_is_fetched_whole() {
    // Exchange Online gives messages up to 150 MB; 16 MB was the most any answer could be (#175).
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 2, NOW);
    let attachment = "QUJD".repeat(5 * 1024 * 1024);
    let mime = format!(
        "From: avery@example.org\r\nTo: owner@example.org\r\nSubject: Large\r\nMIME-Version: 1.0\r\nContent-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\n\r\n{attachment}\r\n"
    );
    assert!(mime.len() > 20 * 1024 * 1024);
    server.state().mailbox.update(&ids[0], |m| m.mime = Some(mime.clone()));
    let got = super::fetch::fetch_messages_with(&server.url(), TOKEN, &fetch_request(&ids), Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    assert!(got.errors.is_empty() && got.failed.is_empty(), "{:?}", got.errors);
    assert_eq!(got.fetched.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [ids[0].as_str(), ids[1].as_str()]);
    assert_eq!(got.fetched[0].mime.len(), mime.len());
    assert!(got.fetched[0].mime == mime.as_bytes());
}

#[test]
fn three_messages_refused_one_after_another_stop_the_batch() {
    use crate::graph::standin::Fault;
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 5, NOW);
    for id in &ids[..4] {
        server.state().mailbox.faults.insert(id.clone(), Fault::Invalid);
    }
    let got = super::fetch::fetch_messages_with(&server.url(), TOKEN, &fetch_request(&ids), Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    assert_eq!(got.failed, ids[..3].to_vec());
    assert!(got.fetched.is_empty());
    // Said by its status and Graph's code, and nothing of the message.
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert!(got.errors[0].contains("status 400 (ErrorInvalidRequest)"), "{}", got.errors[0]);
    assert!(ids.iter().all(|id| !got.errors[0].contains(id.as_str())));
    let asked = server.state().requests.iter().filter(|r| r.target.contains("$value")).count();
    assert_eq!(asked, 3, "the rest wait");
}

/// #276: every piece says how long its part took and how many requests it made, by phase: the
/// folders in the first piece, the mail in each page's, and nothing in them names a folder.
#[test]
fn every_piece_of_a_run_carries_its_time_and_requests() {
    use mailcraft_model::sync::SyncPhase;
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 250, NOW);
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let phase = |piece: &GraphMailResult, phase: SyncPhase| piece.timings.phases.iter().find(|p| p.phase == phase).map_or(0, |p| p.requests);
    // The folders' piece: the special folders and the listing are requests; nothing was sent.
    let first = pieces.first().unwrap();
    assert!(phase(first, SyncPhase::Folders) > 0, "{:?}", first.timings);
    assert_eq!(phase(first, SyncPhase::Sending), 0);
    assert_eq!(first.timings.skipped, 0);
    // The mail: every delta request is counted against the new mail of some piece.
    let counted: u32 = pieces.iter().map(|p| phase(p, SyncPhase::NewMail)).sum();
    let asked = u32::try_from(server.state().requests.iter().filter(|r| r.target.contains("/messages/delta")).count()).unwrap();
    assert!(asked >= 3, "250 messages in pages of 100, and the other folders");
    assert_eq!(counted, asked);
    // Taken in at once, as a host that waits for the whole run does, the numbers add up.
    let mut whole = GraphMailResult::default();
    for piece in pieces.iter().cloned() {
        whole.absorb(piece);
    }
    assert_eq!(whole.timings.requests(), pieces.iter().map(|p| p.timings.requests()).sum::<u32>());
    // Pages come with the page's own piece, the last with what was left over.
    assert!(pieces.iter().filter(|p| !p.pages.is_empty()).all(|p| phase(p, SyncPhase::NewMail) >= 1));
}

/// #276: what is sent is timed apart from the listing of the folders.
#[test]
fn changes_made_here_are_timed_as_sending() {
    use mailcraft_model::sync::{FolderChange, SyncPhase};
    let server = StandIn::start().unwrap();
    let mut asked = request(Vec::new());
    asked.changes = vec![FolderChange::Create { folder: 9, name: "Projects".into(), parent: String::new(), parent_folder: None }];
    let pieces = whole_run(&server.url(), TOKEN, &asked, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    let first = pieces.first().unwrap();
    let sending = first.timings.phases.iter().find(|p| p.phase == SyncPhase::Sending).map_or(0, |p| p.requests);
    assert!(sending >= 1, "{:?} {:?}", first.timings, first.errors);
    assert!(first.timings.phases.iter().any(|p| p.phase == SyncPhase::Folders && p.requests > 0));
}

/// The Inbox pass (#276): the Inbox's delta from its stored link, and nothing else.
mod inbox_pass {
    use super::*;
    use mailcraft_model::FolderKind;
    use mailcraft_model::sync::{GraphFolder, SyncPhase};

    fn pass(server: &StandIn, inbox: &str, links: Vec<GraphLink>, cancel: &AtomicBool) -> Vec<GraphMailResult> {
        let folder = GraphFolder { id: inbox.into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
        let inbox_links = links.into_iter().filter(|l| l.folder == inbox).collect();
        let request = GraphMailRequest { inbox_pass: Some(folder), ..request(inbox_links) };
        whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, cancel)
    }

    #[test]
    fn it_reads_the_inboxs_delta_alone_and_the_run_after_it_reads_nothing_of_the_inbox_twice() {
        let server = StandIn::start().unwrap();
        let inbox = server.state().well_known("inbox");
        let archive = server.state().well_known("archive");
        {
            let mut state = server.state();
            state.mailbox.generate(&inbox, 30, NOW);
            state.mailbox.generate(&archive, 10, NOW);
        }
        let first = run(&server, Vec::new(), &AtomicBool::new(false));
        let links = links_after(&first, Vec::new());
        // New mail in both folders since.
        let (new_inbox, new_archive) = {
            let mut state = server.state();
            (state.mailbox.add(&inbox, "Fresh in the Inbox", NOW + 60), state.mailbox.add(&archive, "Fresh in the Archive", NOW + 60))
        };

        let before = server.state().requests.len();
        let pieces = pass(&server, &inbox, links.clone(), &AtomicBool::new(false));
        // Only the Inbox's delta was asked: no listing, no other folder, nothing else.
        let asked: Vec<String> = server.state().requests[before..].iter().map(|r| format!("{} {}", r.method, r.target)).collect();
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert!(asked[0].starts_with("GET ") && asked[0].contains("messages/delta"), "{asked:?}");
        // Every piece is of the pass, partial, and none says the mailbox was read whole.
        assert!(pieces.iter().all(|p| p.inbox_pass && p.partial && !p.whole && p.listing.is_none()), "{pieces:?}");
        let got: Vec<&str> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).map(|m| m.server.id.as_str()).collect();
        assert_eq!(got, [new_inbox.as_str()]);
        assert!(pieces.iter().flat_map(|p| &p.pages).all(|p| p.folder == inbox && p.caught_up));
        // Its time is the Inbox phase's, with its request.
        let mut timings = mailcraft_model::sync::Timings::default();
        pieces.iter().for_each(|p| timings.absorb(&p.timings));
        assert_eq!(timings.phases.len(), 1);
        assert_eq!((timings.phases[0].phase, timings.phases[0].requests), (SyncPhase::Inbox, 1));

        // The run after it, from the links the pass moved on: the Inbox has nothing new.
        let links = links_after(&pieces, links);
        let rest = run(&server, links, &AtomicBool::new(false));
        let inbox_news: usize = rest.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).map(|p| p.messages.len()).sum();
        let archive_news: Vec<&str> =
            rest.iter().flat_map(|p| &p.pages).filter(|p| p.folder == archive).flat_map(|p| &p.messages).map(|m| m.server.id.as_str()).collect();
        assert_eq!(inbox_news, 0, "the Inbox was read by the pass");
        assert_eq!(archive_news, [new_archive.as_str()]);
        assert!(rest.last().unwrap().whole && rest.iter().all(|p| !p.inbox_pass));
    }

    #[test]
    fn stop_ends_the_pass_after_the_page_in_hand_and_the_next_one_goes_on_from_there() {
        let server = StandIn::start().unwrap();
        let inbox = server.state().well_known("inbox");
        server.state().mailbox.generate(&inbox, 20, NOW);
        let links = links_after(&run(&server, Vec::new(), &AtomicBool::new(false)), Vec::new());
        server.state().mailbox.generate(&inbox, 250, NOW + 3_600);
        let cancel = AtomicBool::new(false);
        let folder = GraphFolder { id: inbox.clone(), kind: FolderKind::Inbox, ..Default::default() };
        let asked = GraphMailRequest { inbox_pass: Some(folder), ..request(links.clone()) };
        let mut pieces = Vec::new();
        super::super::sync_mail_with(&server.url(), TOKEN, &asked, Arc::new(Gate::new()), QUICK, &cancel, &mut |piece| {
            cancel.store(true, Ordering::Relaxed);
            pieces.push(piece);
        });
        let last = pieces.last().unwrap();
        assert!(last.stopped && last.inbox_pass && !last.whole);
        let got: usize = pieces.iter().flat_map(|p| &p.pages).map(|p| p.messages.len()).sum();
        assert!(got > 0 && got < 250, "{got}");
        // The next pass goes on from there: nothing twice, nothing missed.
        let links = links_after(&pieces, links);
        let more = pass(&server, &inbox, links, &AtomicBool::new(false));
        let rest: usize = more.iter().flat_map(|p| &p.pages).map(|p| p.messages.len()).sum();
        assert_eq!(got + rest, 250);
    }

    #[test]
    fn a_server_that_wont_have_it_ends_the_pass_with_the_reason_and_nothing_else() {
        let server = StandIn::start().unwrap();
        let inbox = server.state().well_known("inbox");
        let folder = GraphFolder { id: inbox.clone(), kind: FolderKind::Inbox, ..Default::default() };
        let asked = GraphMailRequest { inbox_pass: Some(folder), ..request(Vec::new()) };
        let pieces = whole_run(&server.url(), "not the token", &asked, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
        let last = pieces.last().unwrap();
        assert!(last.partial && last.inbox_pass && !last.errors.is_empty() && last.pages.is_empty());
    }
}
