// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A run that only takes up what was done here (`push_only`) goes beside a long read of another
//! account's mail: it makes its changes on the server and reads nothing after them, against the
//! strict stand-in. Every name and id is invented.

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{ChangeOutcome, EwsMailRequest, EwsMailResult, FolderChange, Shown};

use crate::standin::{AUTHORIZATION, StandIn, block_on};
use crate::*;

const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";

fn run(standin: &StandIn, request: &EwsMailRequest) -> EwsMailResult {
    let client = Client::new(URL, Secret::new(AUTHORIZATION)).unwrap();
    let mut all = EwsMailResult::default();
    block_on(sync_mail(&client, standin, request, &AtomicBool::new(false), &mut |_| true, &mut |piece| all.absorb(piece))).unwrap();
    all
}

fn request(push_only: bool, changes: Vec<FolderChange>) -> EwsMailRequest {
    EwsMailRequest { email: "ada@example.org".into(), url: URL.into(), push_only, changes, ..Default::default() }
}

/// A stand-in with an empty folder "Old projects" in Deleted Items, and mail in the Inbox.
fn standin() -> (StandIn, String) {
    let standin = StandIn::new();
    let mut s = standin.state();
    let deleted = s.folder_id("deleteditems").unwrap();
    let inbox = s.folder_id("inbox").unwrap();
    let top = s.add_folder("Old projects");
    if let Some(f) = s.folders.iter_mut().find(|f| f.id == top) {
        f.parent = deleted;
    }
    s.deliver(&inbox, "Plan 0", true);
    s.deliver(&inbox, "Plan 1", true);
    drop(s);
    (standin, top)
}

fn delete(top: &str) -> FolderChange {
    FolderChange::Delete { id: top.to_owned(), shown: Shown::of("Old projects", [top], std::iter::empty::<&str>()) }
}

#[test]
fn a_push_only_run_makes_its_changes_and_reads_nothing() {
    let (standin, top) = standin();
    let result = run(&standin, &request(true, vec![delete(&top)]));
    assert!(matches!(result.changes[0].outcome, ChangeOutcome::Done { .. }), "{:?}", result.changes);
    assert!(!standin.state().folders.iter().any(|f| f.id == top), "the folder is deleted on the server");
    assert!(result.pages.is_empty() && result.folders.is_none(), "it read the server's mail or folders");
    assert!(!result.whole && result.errors.is_empty(), "{:?}", result.errors);
}

#[test]
fn the_same_run_that_reads_brings_the_mail_of_the_inbox() {
    let (standin, top) = standin();
    let result = run(&standin, &request(false, vec![delete(&top)]));
    assert!(matches!(result.changes[0].outcome, ChangeOutcome::Done { .. }), "{:?}", result.changes);
    assert!(result.pages.iter().any(|page| !page.messages.is_empty()), "the Inbox was read");
}
