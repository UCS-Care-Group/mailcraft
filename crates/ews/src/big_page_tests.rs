// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A server that answers a page of a folder with more than any answer this program reads
//! (#964, measured on Kerio Connect, which sends whole messages when only the list was asked
//! for): the run asks for fewer changes at a time instead of never reading the folder, and
//! takes every message in.

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{EwsMailRequest, EwsMailResult};

use crate::standin::{AUTHORIZATION, StandIn, block_on};
use crate::*;

const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";

/// The weight of one item's asked-for properties. Three hundred of them at a page of two
/// hundred is 18 MB: over the 16 MiB any answer is read at (`RESPONSE_LIMIT`). At a page of a
/// hundred it is 9 MB: under, and the folder reads.
const PAD: usize = 90_000;
const ITEMS: usize = 300;

fn client() -> Client {
    Client::new(URL, Secret::new(AUTHORIZATION)).unwrap()
}

fn request() -> EwsMailRequest {
    EwsMailRequest { email: "ada@example.org".into(), url: URL.into(), list_folders: true, ..Default::default() }
}

/// The stand-in with a heavy inbox: every message's `Preview` padded, as an item's weight is
/// what its asked-for properties weigh.
fn heavy_inbox() -> StandIn {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        for n in 0..ITEMS {
            let id = s.deliver("inbox", &format!("Heavy {n:03}"), false);
            if let Some(item) = s.items.iter_mut().find(|i| i.id == id) {
                item.preview = "x".repeat(PAD);
            }
        }
    }
    standin
}

#[test]
fn a_folder_whose_pages_are_too_heavy_is_read_in_smaller_pages() {
    let standin = heavy_inbox();
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    let cancel = AtomicBool::new(false);
    block_on(sync_mail(&client(), &standin, &request(), &cancel, &mut |_| true, &mut |piece| pieces.push(piece))).unwrap();
    let subjects: Vec<String> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| &page.messages).map(|m| m.subject.clone()).collect();
    let mut sorted = subjects.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ITEMS, "every heavy message came down, and none twice");
    for piece in &pieces {
        assert!(piece.errors.is_empty(), "the heavy pages were asked smaller, not failed: {:?}", piece.errors);
    }
    assert!(pieces.last().is_some_and(|p| p.whole), "the folder's delta reached its end");
    // (Before the shrinking: one ask, no message, and "The server's answer couldn't be used".)
    let asks = standin.state().requests.iter().filter(|r| r.operation == "SyncFolderItems").count();
    assert!(asks >= 4, "the page shrinks and the folder reads in pages of a hundred: {asks} asks");
}

#[test]
fn a_transport_that_refuses_heavy_answers_is_asked_smaller_too() {
    // (Over a socket the transport, not the reader, is the one that refuses an answer heavier
    // than the limit: `TransportError::BodyTooLarge`, which used to end the run as
    // "Couldn't reach the server" — the stall of #1783 as its owner met it.)
    let standin = heavy_inbox();
    standin.state().transport_caps_bodies = true;
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    let cancel = AtomicBool::new(false);
    block_on(sync_mail(&client(), &standin, &request(), &cancel, &mut |_| true, &mut |piece| pieces.push(piece))).unwrap();
    let subjects: Vec<String> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| &page.messages).map(|m| m.subject.clone()).collect();
    let mut sorted = subjects.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ITEMS, "every heavy message came down, and none twice");
    for piece in &pieces {
        assert!(piece.errors.is_empty(), "refused pages are asked smaller, never a problem: {:?}", piece.errors);
    }
    assert!(pieces.last().is_some_and(|p| p.whole), "the folder's delta reached its end");
}

#[test]
fn a_server_that_always_answers_too_heavy_costs_one_problem_and_no_loop() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        // (One message so heavy that even a page of it alone outweighs any answer read: the
        // run shrinks to the floor, fails once there, and the account says one problem.)
        let id = s.deliver("inbox", "Mountain", false);
        if let Some(item) = s.items.iter_mut().find(|i| i.id == id) {
            item.preview = "x".repeat(20_000_000);
        }
    }
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    let cancel = AtomicBool::new(false);
    block_on(sync_mail(&client(), &standin, &request(), &cancel, &mut |_| true, &mut |piece| pieces.push(piece))).unwrap();
    let asks = standin.state().requests.iter().filter(|r| r.operation == "SyncFolderItems").count();
    // (200, 100, 50, 25, 12, 6, 3, 2, 1: down to the floor; no folder is asked beyond it.)
    assert!((9..=30).contains(&asks), "the page shrinks to the floor and stops there: {asks} asks");
    let problems: Vec<&String> = pieces.iter().flat_map(|p| &p.errors).collect();
    assert_eq!(problems.len(), 1, "one problem said, as ever: {problems:?}");
}
