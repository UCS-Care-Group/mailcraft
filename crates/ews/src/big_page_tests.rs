// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A server that answers a page of a folder with more than any answer this program reads
//! (#964, measured on Kerio Connect, which sends whole messages when only the list was asked
//! for): the run asks for fewer changes at a time, and then for the light list alone, so heavy
//! folders still read instead of never being read — and where nothing can fit, the folder costs
//! one problem and the run goes on, messages and all (#1784).

use std::sync::atomic::AtomicBool;

use mailcraft_model::FlagStatus;
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

fn sync(standin: &StandIn, pieces: &mut Vec<EwsMailResult>) {
    let cancel = AtomicBool::new(false);
    block_on(sync_mail(&client(), standin, &request(), &cancel, &mut |_| true, &mut |piece| pieces.push(piece))).unwrap();
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
    sync(&standin, &mut pieces);
    let subjects: Vec<String> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| &page.messages).map(|m| m.subject.clone()).collect();
    let mut sorted = subjects.clone();
    sorted.sort();
    assert_eq!(subjects.len(), ITEMS, "every heavy message came down, and none twice");
    sorted.dedup();
    assert_eq!(sorted.len(), ITEMS, "and every one of them a different message");
    for piece in &pieces {
        assert!(piece.errors.is_empty(), "the heavy pages were asked smaller, not failed: {:?}", piece.errors);
    }
    assert!(pieces.last().is_some_and(|p| p.whole), "the folder's delta reached its end");
    // (Before the shrinking: one ask, no message, and "The server's answer couldn't be used".)
    let state = standin.state();
    let syncs: Vec<&crate::standin::Asked> = state.requests.iter().filter(|r| r.operation == "SyncFolderItems").collect();
    let asks = syncs.len();
    assert!(asks >= 4, "the page shrinks and the folder reads in pages of a hundred: {asks} asks");
    // (The page is the folder's own (#1784, the review): every folder but the heavy one starts
    // at the full page again, so a heavy folder leaves the light ones three changes short of
    // nothing. Every first ask, folder by folder — not just some later twenty hundred.)
    let bodies: Vec<&str> = syncs.iter().map(|r| r.body.as_str()).collect();
    let folder_of = |b: &str| -> String { b.split("FolderId Id=\"").nth(1).unwrap_or_default().split('"').next().unwrap_or_default().to_string() };
    let heavy: String =
        bodies.iter().find(|b| b.contains("<m:MaxChangesReturned>100</m:MaxChangesReturned>")).map(|b| folder_of(b)).unwrap_or_default();
    assert!(!heavy.is_empty(), "the heavy folder was really asked smaller");
    let mut firsts: Vec<(String, bool)> = Vec::new();
    for b in &bodies {
        let f = folder_of(b);
        if !firsts.iter().any(|(x, _)| *x == f) {
            firsts.push((f, b.contains("<m:MaxChangesReturned>200</m:MaxChangesReturned>")));
        }
    }
    assert!(firsts.len() > 2, "more than the heavy folder was read: {} folders", firsts.len());
    for (f, full_page) in &firsts {
        if *f != heavy {
            assert!(full_page, "folder {f} is asked at the full page to begin with");
        }
    }
}

#[test]
fn a_transport_that_refuses_heavy_answers_is_asked_smaller_too() {
    // (Over a socket the transport, not the reader, is the one that refuses an answer heavier
    // than the limit: `TransportError::BodyTooLarge`, which used to end the run as
    // "Couldn't reach the server" — the stall of #1783 as its owner met it.)
    let standin = heavy_inbox();
    standin.state().transport_caps_bodies = true;
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    sync(&standin, &mut pieces);
    // (The precondition the test names, now checked: it was the stand-in's *transport* refusing
    // the heavy answers, once and exactly once — and the recovery is the test's name, not an
    // assumption: the folder was genuinely asked with a smaller page after the refusal.)
    let state = standin.state();
    let refused = state.refused_bodies;
    assert_eq!(refused, 1, "the transport door refused the one over-heavy page, no more: {refused}");
    assert!(
        state.requests.iter().any(|r| r.operation == "SyncFolderItems" && r.body.contains("<m:MaxChangesReturned>100</m:MaxChangesReturned>")),
        "and the folder was asked smaller after it"
    );
    drop(state);
    let subjects: Vec<String> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| &page.messages).map(|m| m.subject.clone()).collect();
    let mut sorted = subjects.clone();
    sorted.sort();
    assert_eq!(subjects.len(), ITEMS, "every heavy message came down, and none twice");
    sorted.dedup();
    assert_eq!(sorted.len(), ITEMS, "and every one of them a different message");
    for piece in &pieces {
        assert!(piece.errors.is_empty(), "refused pages are asked smaller, never a problem: {:?}", piece.errors);
    }
    assert!(pieces.last().is_some_and(|p| p.whole), "the folder's delta reached its end");
}

/// One message whose asked-for properties outweigh any answer read — a 20 MB preview. The pages
/// that ask for the preview all outweigh the limit; the list asked with the properties alone
/// doesn't ask for the preview, and the folder reads after two refusals (#1784).
#[test]
fn a_message_too_heavy_to_list_is_read_once_the_list_drops_the_weight() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        let id = s.deliver("inbox", "Mountain", false);
        if let Some(item) = s.items.iter_mut().find(|i| i.id == id) {
            item.preview = "x".repeat(20_000_000);
        }
    }
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    sync(&standin, &mut pieces);
    let subjects: Vec<String> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| &page.messages).map(|m| m.subject.clone()).collect();
    assert_eq!(subjects, ["Mountain"], "the heavy message came down with the light list");
    for piece in &pieces {
        assert!(piece.errors.is_empty(), "no problem: the weight left the list, it was not failed: {:?}", piece.errors);
    }
    assert!(pieces.last().is_some_and(|p| p.whole), "the folder's delta reached its end");
    let asks = standin.state().requests.iter().filter(|r| r.operation == "SyncFolderItems").count();
    // (Refused at two hundred, refused at a hundred, then the light shape from a fresh page;
    // the folders beside this one are read with it too — nothing like the crawl to a page of one.)
    assert!(asks <= 10, "two refusals teach the shape, the folders read: {asks} asks");
}

/// Kerio Connect as measured (#964): `SyncFolderItems` hangs the whole message on every change
/// whatever else is asked — and measures to answer the `IdOnly` base shape with the properties
/// asked by name alone. The folder reads whole, heavy messages and all, and no answer over the
/// limit is ever taken in (#1784).
#[test]
fn a_server_that_sends_whole_messages_with_the_changes_is_asked_as_ids() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.sends_mime_with_changes = 20_000_000;
        for n in 0..3 {
            s.deliver("inbox", &format!("Heavy {n}"), false);
        }
    }
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    sync(&standin, &mut pieces);
    let messages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| page.messages.iter()).collect();
    assert_eq!(messages.len(), 3, "three messages, once each, mime-laden ones as plain headers");
    assert!(messages.iter().all(|m| !m.subject.is_empty()), "the named subject came with the ids shape");
    assert!(messages.iter().all(|m| m.received != mailcraft_time::Timestamp::default()), "the named date came with the ids shape, not just the id");
    // (The `Ids` list asks no recipients, and the folder doesn't turn `Bare` to learn them: the
    // same fallback that asks for the ones a server leaves out (#1151) fills these too.)
    assert!(messages.iter().all(|m| !m.to.is_empty()), "the recipients came too, asked message by message as ever");
    for piece in &pieces {
        assert!(piece.errors.is_empty(), "the folder was read, not failed: {:?}", piece.errors);
    }
    assert!(pieces.last().is_some_and(|p| p.whole), "the folder's delta reached its end");
    let state = standin.state();
    let asked: Vec<&crate::standin::Asked> = state.requests.iter().filter(|r| r.operation == "SyncFolderItems").collect();
    // (Which folder was asked `IdOnly` — the lesson belongs to the heavy inbox and no one else
    // — and, as an explicit precondition rather than by folder-order luck, that a light folder
    // is read after the heavy one and asked in the schema shape at the full page.)
    let folder_of = |b: &str| -> String { b.split("FolderId Id=\"").nth(1).unwrap_or_default().split('"').next().unwrap_or_default().to_string() };
    let inbox = state.folders.iter().find(|f| f.distinguished == "inbox").map(|f| f.id.clone()).unwrap_or_default();
    let id_only: Vec<(usize, String)> = asked
        .iter()
        .enumerate()
        .filter(|(_, a)| a.body.contains("<t:BaseShape>IdOnly</t:BaseShape>"))
        .map(|(i, a)| (i, folder_of(&a.body)))
        .collect();
    assert!(!id_only.is_empty(), "a page was asked as ids");
    assert!(id_only.iter().all(|(_, f)| *f == inbox), "only the heavy folder was asked as ids: {id_only:?}");
    let after: Vec<&&crate::standin::Asked> = asked[id_only.last().unwrap().0 + 1..].iter().filter(|a| folder_of(&a.body) != inbox).collect();
    assert!(!after.is_empty(), "a light folder is read after the heavy one — the precondition of what follows");
    for a in &after {
        assert!(a.body.contains("<t:BaseShape>Default</t:BaseShape>"), "the light folder keeps the schema shape");
        assert!(a.body.contains("<m:MaxChangesReturned>200</m:MaxChangesReturned>"), "and the light folder starts at the full page");
    }
    assert!(asked.len() <= 8, "two refusals teach the shape, and every folder after reads light: {} asks", asked.len());
}

/// A server that both hangs whole messages on the changes and won't take the ids asking at all,
/// where the items themselves outweigh any answer: every door is shut for that folder, and it
/// must cost one problem and nothing else — the halving goes on to the floor, the run's schema
/// shape stays what it was (the refusal was not a lesson about it), no folder wastes a second
/// ids probe, and the folders after are asked as ever, preview on.
#[test]
fn a_server_that_refuses_both_the_weight_and_the_ids_asking_costs_one_problem_and_nothing_else() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.sends_mime_with_changes = 20_000_000;
        s.refuses_ids = true;
        for n in 0..3 {
            s.deliver("inbox", &format!("Heavy {n}"), false);
        }
    }
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    sync(&standin, &mut pieces);
    let errors: Vec<&String> = pieces.iter().flat_map(|p| &p.errors).collect();
    assert_eq!(errors.len(), 1, "the unreadable folder is one problem, and the only one: {errors:?}");
    // (The run goes on: the folders after the unreadable one are read regardless.)
    let state = standin.state();
    let asked: Vec<&crate::standin::Asked> = state.requests.iter().filter(|r| r.operation == "SyncFolderItems").collect();
    let folder_of = |b: &str| -> String { b.split("FolderId Id=\"").nth(1).unwrap_or_default().split('"').next().unwrap_or_default().to_string() };
    let mut read: Vec<String> = asked.iter().map(|a| folder_of(&a.body)).collect();
    read.sort();
    read.dedup();
    assert!(read.len() > 2, "the run went on past the folder it couldn't read: {} folders asked", read.len());
    let ids = asked.iter().filter(|a| a.body.contains("<t:BaseShape>IdOnly</t:BaseShape>")).count();
    assert_eq!(ids, 2, "the ids asking is tried twice — with the flag and without — and given up for the run when both are refused: {ids}");
    // (The phantom damage the re-review named: the schema step-downs must not fire on a page
    // that never asked their fields. A later folder still asked with the preview proves it.)
    let first_ids = asked.iter().position(|a| a.body.contains("<t:BaseShape>IdOnly</t:BaseShape>")).unwrap_or_default();
    assert!(
        asked[first_ids + 1..].iter().any(|a| a.body.contains("item:Preview")),
        "the shape of the run was untouched: later folders are still asked with the preview"
    );
    assert!(asked.len() <= 16, "the halving to the floor is bounded: {} asks", asked.len());
}

/// Refusals must be CONSECUTIVE to teach the ids lesson: a folder whose first page is too heavy
/// but whose next page reads, and whose page after that is heavy again (an uneven folder, not a
/// Kerio one), halves once more instead of jumping to the light shape — the counter that knows
/// is the one reset by every page that read. (#1784; the hardening the Cursor consult asked for.)
#[test]
fn lone_refusals_on_either_side_of_a_read_page_only_halve_never_switch_the_shape() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        // (Page one at two hundred carries the first two hundred: a light half and a nine-
        // megabyte half, twenty over the limit. At a hundred the first page reads; the second
        // is nineteen megabytes on its own — refused, once again alone, and halves to fifty.)
        for n in 0..100 {
            let id = s.deliver("inbox", &format!("Lighter {n:03}"), false);
            if let Some(item) = s.items.iter_mut().find(|i| i.id == id) {
                item.preview = "x".repeat(90_000);
            }
        }
        for n in 0..100 {
            let id = s.deliver("inbox", &format!("Heavier {n:03}"), false);
            if let Some(item) = s.items.iter_mut().find(|i| i.id == id) {
                item.preview = "x".repeat(190_000);
            }
        }
        for n in 0..50 {
            s.deliver("inbox", &format!("Tail {n:03}"), false);
        }
    }
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    sync(&standin, &mut pieces);
    let subjects: Vec<String> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| &page.messages).map(|m| m.subject.clone()).collect();
    assert_eq!(subjects.len(), 250, "all of it came down, both refusals notwithstanding");
    for piece in &pieces {
        assert!(piece.errors.is_empty(), "an uneven folder costs halvings, not a shape switch or a problem: {:?}", piece.errors);
    }
    let state = standin.state();
    let asked: Vec<&crate::standin::Asked> = state.requests.iter().filter(|r| r.operation == "SyncFolderItems").collect();
    assert!(
        !asked.iter().any(|a| a.body.contains("<t:BaseShape>IdOnly</t:BaseShape>")),
        "no page of this folder was ever asked as ids: the refusals were never consecutive"
    );
    assert!(
        asked.iter().any(|a| a.body.contains("<m:MaxChangesReturned>50</m:MaxChangesReturned>")),
        "the second lone refusal halved once more, to fifty"
    );
}

/// The same refusing server where the items are big rather than unsurvivable (~1 MB each):
/// the pages that don't fit halve, and when the ids asking is refused the halving goes on in
/// the schema shape — pages of a dozen, a megabyte each, fit and read. The folder lives; the
/// ids lesson was refused, but the size lesson the folder learned on its own still saves it
/// (#1784, the re-review's readable world).
#[test]
fn a_refused_ids_asking_falls_back_to_halving_that_eventually_fits() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.sends_mime_with_changes = 1_000_000;
        s.refuses_ids = true;
        for n in 0..150 {
            s.deliver("inbox", &format!("Big {n:03}"), false);
        }
    }
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    sync(&standin, &mut pieces);
    let subjects: Vec<String> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| &page.messages).map(|m| m.subject.clone()).collect();
    assert_eq!(subjects.len(), 150, "every one of them came down, in pages small enough to read");
    for piece in &pieces {
        assert!(piece.errors.is_empty(), "no problem: the halving found the fit: {:?}", piece.errors);
    }
    assert!(pieces.last().is_some_and(|p| p.whole), "the folder's delta reached its end");
    let state = standin.state();
    let asked: Vec<&crate::standin::Asked> = state.requests.iter().filter(|r| r.operation == "SyncFolderItems").collect();
    let ids = asked.iter().filter(|a| a.body.contains("<t:BaseShape>IdOnly</t:BaseShape>")).count();
    assert_eq!(ids, 2, "the refused asking is probed twice for the whole run — once with the flag, once without — then remembered: {ids}");
    assert!(asked.len() <= 24, "the halving is bounded and the reading is steady: {} asks", asked.len());
}

/// The light list must carry the read state (#1784, benandez's repro): a folder of read and
/// unread messages on a server that hangs the weight on the list switches to the ids asking —
/// and every message must land with the read state the server has. An `IdOnly` answer that
/// omits it parses as unread, and that is then applied over the mailbox: read mail visibly
/// turning unread, and mail read in webmail landing unread.
#[test]
fn read_messages_listed_with_the_light_shape_arrive_read() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.sends_mime_with_changes = 200_000;
        for n in 0..200 {
            s.deliver("inbox", &format!("Heavy {n:03}"), n % 2 == 0);
        }
    }
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    sync(&standin, &mut pieces);
    let messages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| page.messages.iter()).collect();
    assert_eq!(messages.len(), 200, "every one came down");
    assert!(messages.iter().any(|m| m.preview.is_empty()), "and they came as the light list, without the weight");
    // (The delivered read state was the even subjects: the last digit of the number says it.)
    let wrong: Vec<&str> = messages
        .iter()
        .filter(|m| {
            let even = matches!(m.subject.chars().last(), Some('0' | '2' | '4' | '6' | '8'));
            m.server.read != even
        })
        .map(|m| m.subject.as_str())
        .collect();
    assert!(wrong.is_empty(), "read stayed read and unread stayed unread under the light list: {wrong:?}");
    let state = standin.state();
    assert!(
        state.requests.iter().any(|r| r.operation == "SyncFolderItems" && r.body.contains("<t:BaseShape>IdOnly</t:BaseShape>")),
        "the folder did switch to the light asking, which is where the fault lived"
    );
}

/// The light list asks the flag when the run's shape is the full one: a flagged message listed
/// under ids must arrive flagged — an answer that says nothing about the flag parses as
/// unflagged, and that is the same fault as the read state, one floor down (#1784, benandez's
/// optional note, taken).
#[test]
fn a_flagged_message_listed_with_the_light_shape_arrives_flagged() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.sends_mime_with_changes = 200_000;
        for n in 0..200 {
            s.deliver("inbox", &format!("Heavy {n:03}"), false);
        }
        let ids: Vec<String> = s.items.iter().take(7).map(|i| i.id.clone()).collect();
        for item in s.items.iter_mut().filter(|i| ids.contains(&i.id)) {
            item.flagged = true;
            item.flag = "Flagged".into();
        }
    }
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    sync(&standin, &mut pieces);
    let messages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| page.messages.iter()).collect();
    assert_eq!(messages.len(), 200, "every one came down");
    let flagged = messages.iter().filter(|m| m.server.flag == FlagStatus::Flagged).count();
    assert_eq!(flagged, 7, "the seven flagged messages stayed flagged under the light list");
    let state = standin.state();
    assert!(
        state
            .requests
            .iter()
            .any(|r| r.operation == "SyncFolderItems" && r.body.contains("<t:BaseShape>IdOnly</t:BaseShape>") && r.body.contains("item:Flag")),
        "it was the light asking that named the flag, as the full run's shape does"
    );
}

/// The other half of the choice, pinned: a run whose schema had to step down (a server without
/// the preview and the flag) must not name the flag on its light pages — a full run's server
/// knows it, a stepped-down one's by definition doesn't, and asking it there would only be
/// refused. Without this test someone can "simplify" the flag ask back to always-on and every
/// other test still passes (#1784).
#[test]
fn a_stepped_down_run_asks_the_light_list_without_the_flag() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.lacks_preview = true;
        s.sends_mime_with_changes = 200_000;
        for n in 0..200 {
            s.deliver("inbox", &format!("Heavy {n:03}"), false);
        }
    }
    let mut pieces: Vec<EwsMailResult> = Vec::new();
    sync(&standin, &mut pieces);
    let subjects: Vec<String> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| &page.messages).map(|m| m.subject.clone()).collect();
    assert_eq!(subjects.len(), 200, "the folder read through the light asking, flag unknown as ever for this server");
    for piece in &pieces {
        assert!(piece.errors.is_empty(), "no problem: the light asking stayed inside what this schema has: {:?}", piece.errors);
    }
    let state = standin.state();
    let ids_asks: Vec<&crate::standin::Asked> =
        state.requests.iter().filter(|r| r.operation == "SyncFolderItems" && r.body.contains("<t:BaseShape>IdOnly</t:BaseShape>")).collect();
    assert!(!ids_asks.is_empty(), "the folder did take up the light asking");
    assert!(ids_asks.iter().all(|a| !a.body.contains("item:Flag")), "no light page of a stepped-down run named the flag");
}
