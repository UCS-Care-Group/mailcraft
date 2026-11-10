// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A folder deleted for good here is deleted on the Exchange server only when the server holds
//! nothing in it or under it that the owner wasn't shown (AU3-H7), against the strict stand-in.
//! Every name and id is invented.

use std::future::Future;
use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{ChangeOutcome, EwsMailRequest, EwsMailResult, FolderChange, Shown};

use crate::standin::{AUTHORIZATION, StandIn, block_on};
use crate::*;

const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";

fn client() -> Client {
    Client::new(URL, Secret::new(AUTHORIZATION)).unwrap()
}

fn request(changes: Vec<FolderChange>) -> EwsMailRequest {
    EwsMailRequest { email: "ada@example.org".into(), url: URL.into(), inbox_pass: false, changes, ..Default::default() }
}

fn run_with<E: HttpExchange>(exchange: &E, changes: Vec<FolderChange>) -> EwsMailResult {
    let mut all = EwsMailResult::default();
    block_on(sync_mail(&client(), exchange, &request(changes), &AtomicBool::new(false), &mut |_| true, &mut |piece| all.absorb(piece))).unwrap();
    all
}

/// A stand-in with "Old projects" in Deleted Items, a subfolder "2025" in it, and two messages in
/// each; the ids of the two folders and of the four messages.
fn standin() -> (StandIn, String, String, Vec<String>) {
    let standin = StandIn::new();
    let mut s = standin.state();
    let deleted = s.folder_id("deleteditems").unwrap();
    let top = s.add_folder("Old projects");
    let sub = s.add_folder("2025");
    for (id, parent) in [(&top, &deleted), (&sub, &top)] {
        if let Some(f) = s.folders.iter_mut().find(|f| f.id == *id) {
            f.parent = parent.clone();
        }
    }
    let messages = (0..4).map(|n| s.deliver(if n < 2 { &top } else { &sub }, &format!("Plan {n}"), true)).collect();
    drop(s);
    (standin, top, sub, messages)
}

fn delete(top: &str, folders: &[&String], messages: &[String]) -> FolderChange {
    let shown = Shown::of("Old projects", folders.iter().map(|f| f.as_str()), messages.iter().map(String::as_str));
    FolderChange::Delete { id: top.to_owned(), shown }
}

fn still_there(standin: &StandIn, id: &str) -> bool {
    standin.state().folders.iter().any(|f| f.id == id)
}

#[test]
fn a_folder_as_it_was_shown_is_deleted() {
    let (standin, top, sub, messages) = standin();
    let result = run_with(&standin, vec![delete(&top, &[&top, &sub], &messages)]);
    assert!(matches!(result.changes[0].outcome, ChangeOutcome::Done { .. }), "{:?}", result.changes);
    assert_eq!(standin.state().deletes, ["SoftDelete"]);
    assert!(!still_there(&standin, &top));
}

#[test]
fn mail_that_arrived_since_keeps_the_folder_and_says_how_much() {
    let (standin, top, sub, messages) = standin();
    standin.state().deliver(&top, "Arrived since", false);
    standin.state().deliver(&sub, "Arrived since too", false);
    let result = run_with(&standin, vec![delete(&top, &[&top, &sub], &messages)]);
    assert_eq!(result.changes[0].outcome, ChangeOutcome::NotShown { items: 2 }, "{:?}", result.changes);
    assert!(standin.state().deletes.is_empty(), "nothing was deleted");
    assert!(still_there(&standin, &top) && still_there(&standin, &sub));
    // The folder list is read afresh, so that the folder comes back.
    assert!(result.folders.as_ref().is_some_and(|f| f.afresh && f.changed.iter().any(|f| f.id == top)));
}

#[test]
fn a_folder_made_elsewhere_under_it_keeps_it() {
    let (standin, top, sub, messages) = standin();
    {
        let mut s = standin.state();
        let made = s.add_folder("Made on the phone");
        if let Some(f) = s.folders.iter_mut().find(|f| f.id == made) {
            f.parent = sub.clone();
        }
    }
    let result = run_with(&standin, vec![delete(&top, &[&top, &sub], &messages)]);
    assert_eq!(result.changes[0].outcome, ChangeOutcome::NotShown { items: 1 }, "{:?}", result.changes);
    assert!(standin.state().deletes.is_empty());
    assert!(still_there(&standin, &top));
}

#[test]
fn a_folder_under_it_the_list_cant_read_keeps_it() {
    let (standin, top, sub, messages) = standin();
    {
        // A folder of mail with no name to read, made elsewhere under the subfolder.
        let mut s = standin.state();
        let made = s.add_folder("   ");
        if let Some(f) = s.folders.iter_mut().find(|f| f.id == made) {
            f.parent = sub.clone();
        }
    }
    let result = run_with(&standin, vec![delete(&top, &[&top, &sub], &messages)]);
    assert!(matches!(&result.changes[0].outcome, ChangeOutcome::Refused { why } if why.contains("check")), "{:?}", result.changes);
    assert!(standin.state().deletes.is_empty());
    assert!(still_there(&standin, &top));
}

#[test]
fn a_deletion_from_before_that_says_nothing_shown_goes_only_for_an_empty_folder() {
    let (standin, top, _, _) = standin();
    let result = run_with(&standin, vec![FolderChange::Delete { id: top.clone(), shown: Shown::default() }]);
    assert_eq!(result.changes[0].outcome, ChangeOutcome::NotShown { items: 5 }, "{:?}", result.changes);
    assert!(standin.state().deletes.is_empty());
}

/// Answers every `SyncFolderItems` with a fault: the check can't be made.
struct Failing<'a>(&'a StandIn);

impl HttpExchange for Failing<'_> {
    fn exchange(&self, request: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        let body = String::from_utf8_lossy(request.body()).into_owned();
        let done = block_on(self.0.exchange(request)).unwrap();
        let fail = body.contains("<m:SyncFolderItems");
        std::future::ready(if fail { Ok(HttpResponse::new(500, b"<oops/>".to_vec())) } else { done })
    }
}

#[test]
fn a_check_that_fails_deletes_nothing() {
    let (standin, top, sub, messages) = standin();
    let result = run_with(&Failing(&standin), vec![delete(&top, &[&top, &sub], &messages)]);
    assert!(matches!(&result.changes[0].outcome, ChangeOutcome::Refused { why } if why.contains("check")), "{:?}", result.changes);
    assert!(standin.state().deletes.is_empty());
    assert!(still_there(&standin, &top));
}

/// A server that can't be reached.
struct Away;

impl HttpExchange for Away {
    fn exchange(&self, _: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        std::future::ready(Err(TransportError::Unreachable))
    }
}

#[test]
fn a_server_that_cant_be_reached_takes_nothing_for_done() {
    let (_, top, sub, messages) = standin();
    let result = run_with(&Away, vec![delete(&top, &[&top, &sub], &messages)]);
    assert!(result.changes.is_empty(), "the deletion waits: {:?}", result.changes);
}

#[test]
fn a_folder_already_gone_on_the_server_is_gone() {
    let (standin, top, sub, messages) = standin();
    standin.state().delete_folder(&top);
    standin.state().deletes.clear();
    let result = run_with(&standin, vec![delete(&top, &[&top, &sub], &messages)]);
    assert_eq!(result.changes[0].outcome, ChangeOutcome::Gone, "{:?}", result.changes);
    assert!(standin.state().deletes.is_empty());
}
