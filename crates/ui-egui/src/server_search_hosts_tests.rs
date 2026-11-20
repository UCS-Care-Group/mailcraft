// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! More results on the server for an Exchange server and a JMAP account (#1725), in the window:
//! nothing is asked before the click, each account goes to its own host with its own password,
//! one without a password is not asked, and a search that is replaced is called off.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::ews_mail::apply_ews_mail;
use mailcraft_engine::cmd::jmap_mail::apply_jmap_mail;
use mailcraft_model::sync::{
    EwsFolderDeltas, EwsMailResult, FolderListing, GraphFetchResult, GraphFolder, GraphMessage, GraphSearchResult, JmapMailResult, ServerMessage,
};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use super::*;
use crate::Services;
use crate::ews_mail::EwsFetchJob;

type Kept<Job> = Arc<Mutex<Vec<(Job, mpsc::Sender<GraphSearchResult>)>>>;

const EWS_ME: &str = "ada@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";
const JMAP_ME: &str = "bo@example.org";
const JMAP_SESSION: &str = "https://jmap.example.org/jmap/session";
const NOW: i64 = 1_790_000_000;

fn folders() -> Vec<GraphFolder> {
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    vec![folder("in", "Inbox", FolderKind::Inbox), folder("ar", "Archive", FolderKind::Archive), folder("de", "Deleted Items", FolderKind::Deleted)]
}

/// A window with an Exchange server account and a JMAP account, their folders listed, and hosts
/// that keep every search for the test to answer; the accounts' ids and what the hosts were asked.
fn window() -> (MailApp, Id, Id, Kept<EwsSearchJob>, Kept<JmapSearchJob>) {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    let ews = app.execute("account.add", json!({"email": EWS_ME, "kind": "exchange", "ews": {"url": URL}})).unwrap()["id"].as_u64().unwrap();
    let deltas = EwsFolderDeltas { changed: folders(), afresh: true, ..Default::default() };
    let listed = EwsMailResult { account: ews, email: EWS_ME.into(), url: URL.into(), partial: true, folders: Some(deltas), ..Default::default() };
    apply_ews_mail(&mut app.session, EwsMailResult { hierarchy: "H1".into(), ..listed }).unwrap();
    let jmap =
        app.execute("account.add", json!({"email": JMAP_ME, "kind": "jmap", "jmap": {"session": JMAP_SESSION}})).unwrap()["id"].as_u64().unwrap();
    let listing = Some(FolderListing { folders: folders(), complete: true });
    apply_jmap_mail(&mut app.session, JmapMailResult { account: jmap, email: JMAP_ME.into(), partial: true, listing, ..Default::default() }).unwrap();
    let (ews_jobs, jmap_jobs): (Kept<EwsSearchJob>, Kept<JmapSearchJob>) = Default::default();
    let kept = ews_jobs.clone();
    app.services.ews_search = Some(Box::new(move |job: EwsSearchJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    let kept = jmap_jobs.clone();
    app.services.jmap_search = Some(Box::new(move |job: JmapSearchJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    (app, ews, jmap, ews_jobs, jmap_jobs)
}

fn found(id: &str, folder: &str, subject: &str, received: i64) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: folder.into(), ..Default::default() },
        subject: subject.into(),
        received,
        sent: received,
        ..Default::default()
    }
}

fn answer(account: Id, email: &str, ticket: u64, messages: Vec<GraphMessage>, next: &str) -> GraphSearchResult {
    GraphSearchResult { account, email: email.into(), ticket, messages, next: next.into(), ..Default::default() }
}

#[test]
fn one_click_asks_each_account_through_its_own_host_with_its_own_password() {
    let (mut app, ews, jmap, ews_jobs, jmap_jobs) = window();
    app.passwords.insert(ews, "made-up-exchange-word".into());
    app.passwords.insert(jmap, "made-up-jmap-word".into());
    app.run("view.search", json!({"query": "budget", "scope": "everywhere"})).unwrap();
    for _ in 0..3 {
        app.drive_server_search();
    }
    assert_eq!(app.server_link(), Link::Ask);
    assert!(ews_jobs.lock().unwrap().is_empty() && jmap_jobs.lock().unwrap().is_empty(), "nothing is asked by itself");

    app.run("ui.moreResultsOnServer", json!({})).unwrap();
    assert_eq!(app.server_link(), Link::Searching);
    let (ews_job, ews_tx) = ews_jobs.lock().unwrap().pop().unwrap();
    let (jmap_job, jmap_tx) = jmap_jobs.lock().unwrap().pop().unwrap();
    assert_eq!((ews_job.request.account, ews_job.secret.as_str(), ews_job.request.url.as_str()), (ews, "made-up-exchange-word", URL));
    assert_eq!((ews_job.request.folders.as_slice(), ews_job.request.query.as_str()), (["in".to_string(), "ar".to_string()].as_slice(), "budget"));
    assert_eq!((jmap_job.request.account, jmap_job.secret.as_str(), jmap_job.request.session.as_str()), (jmap, "made-up-jmap-word", JMAP_SESSION));
    assert_eq!((jmap_job.request.outside.as_slice(), jmap_job.request.places.len()), (["de".to_string()].as_slice(), 3));
    // Neither job shows its password or the search.
    let shown = format!("{ews_job:?} {jmap_job:?}");
    assert!(!shown.contains("made-up") && !shown.contains("budget") && !shown.contains("example.org"), "{shown}");

    // What they find joins one list, newest first; a page that says where it ended is followed.
    ews_tx.send(answer(ews, EWS_ME, ews_job.request.ticket, vec![found("E-1", "ar", "Budget of last year", NOW - 100)], "50 q ar")).unwrap();
    jmap_tx.send(answer(jmap, JMAP_ME, jmap_job.request.ticket, vec![found("J-1", "in", "Budget for the spring", NOW)], "")).unwrap();
    app.drive_server_search();
    let subjects: Vec<&str> = app.session.server_search.results.iter().map(|r| r.found.subject.as_str()).collect();
    assert_eq!(subjects, ["Budget for the spring", "Budget of last year"]);
    let (next_job, next_tx) = ews_jobs.lock().unwrap().pop().unwrap();
    assert_eq!(next_job.request.next, "50 q ar");
    assert!(jmap_jobs.lock().unwrap().is_empty(), "the JMAP server had no more");
    next_tx.send(answer(ews, EWS_ME, next_job.request.ticket, Vec::new(), "")).unwrap();
    app.drive_server_search();
    assert_eq!(app.server_link(), Link::End);
}

#[test]
fn a_result_opened_is_put_in_its_folder_and_asked_for_whole() {
    let (mut app, ews, _, ews_jobs, _) = window();
    app.passwords.insert(ews, "made-up-exchange-word".into());
    let fetches: Arc<Mutex<Vec<EwsFetchJob>>> = Default::default();
    let kept = fetches.clone();
    app.services.ews_fetch = Some(Box::new(move |job: EwsFetchJob| {
        kept.lock().unwrap().push(job);
        mpsc::channel::<GraphFetchResult>().1
    }));
    app.run("view.search", json!({"query": "budget", "scope": "everywhere"})).unwrap();
    app.run("ui.moreResultsOnServer", json!({})).unwrap();
    let (job, tx) = ews_jobs.lock().unwrap().pop().unwrap();
    tx.send(answer(ews, EWS_ME, job.request.ticket, vec![found("E-1", "ar", "Budget of last year", NOW)], "")).unwrap();
    app.drive_server_search();
    app.open_server_result(0);
    let id = app.session.sel.messages.first().copied().unwrap();
    let m = app.session.mailbox.message(id).unwrap();
    assert!(m.unfetched && m.subject == "Budget of last year" && Some(m.folder) == app.session.mailbox.folder_by_server(ews, "ar"));
    app.drive_fetch();
    let asked: Vec<Vec<String>> = fetches.lock().unwrap().iter().map(|job| job.request.ids.clone()).collect();
    assert!(asked.iter().any(|ids| ids == &["E-1".to_string()]), "{asked:?}");
}

#[test]
fn an_account_without_a_password_or_with_one_turned_down_is_not_asked() {
    let (mut app, ews, jmap, ews_jobs, jmap_jobs) = window();
    app.run("view.search", json!({"query": "budget", "scope": "everywhere"})).unwrap();
    // No password for either: there is no way to ask.
    assert_eq!(app.server_link(), Link::NotConnected { retry: false });
    assert!(app.run("ui.moreResultsOnServer", json!({})).is_err());
    // One for the JMAP account alone: it is asked, and the Exchange server is answered as not reached.
    app.passwords.insert(jmap, "made-up-jmap-word".into());
    assert_eq!(app.server_link(), Link::Ask);
    app.run("ui.moreResultsOnServer", json!({})).unwrap();
    assert!(ews_jobs.lock().unwrap().is_empty());
    let (job, tx) = jmap_jobs.lock().unwrap().pop().unwrap();
    tx.send(answer(jmap, JMAP_ME, job.request.ticket, Vec::new(), "")).unwrap();
    app.drive_server_search();
    assert!(app.session.server_search.offline, "the Exchange server wasn't reached");
    // A password its server turned down is not sent again by a search.
    app.passwords.insert(ews, "made-up-exchange-word".into());
    app.jmap_refused.insert(ews);
    app.run("view.search", json!({"query": "lunch", "scope": "everywhere"})).unwrap();
    app.run("ui.moreResultsOnServer", json!({})).unwrap();
    assert!(ews_jobs.lock().unwrap().is_empty());
    assert_eq!(jmap_jobs.lock().unwrap().len(), 1);
}

#[test]
fn a_search_that_is_replaced_or_whose_mailbox_goes_is_called_off() {
    let (mut app, ews, jmap, ews_jobs, jmap_jobs) = window();
    app.passwords.insert(ews, "made-up-exchange-word".into());
    app.passwords.insert(jmap, "made-up-jmap-word".into());
    app.run("view.search", json!({"query": "budget", "scope": "everywhere"})).unwrap();
    app.run("ui.moreResultsOnServer", json!({})).unwrap();
    let (first_ews, late) = ews_jobs.lock().unwrap().pop().unwrap();
    let (first_jmap, _) = jmap_jobs.lock().unwrap().pop().unwrap();
    assert!(!first_ews.cancel.load(Ordering::Relaxed) && !first_jmap.cancel.load(Ordering::Relaxed));
    // Another search: the first stops before its next request, and what it still brings isn't taken in.
    app.run("view.search", json!({"query": "lunch", "scope": "everywhere"})).unwrap();
    app.server_searching.clear();
    assert!(first_ews.cancel.load(Ordering::Relaxed) && first_jmap.cancel.load(Ordering::Relaxed));
    let _ = late.send(answer(ews, EWS_ME, first_ews.request.ticket, vec![found("E-1", "ar", "Budget of last year", NOW)], ""));
    app.run("ui.moreResultsOnServer", json!({})).unwrap();
    app.drive_server_search();
    assert!(app.session.server_search.results.is_empty());
    let (second, _) = ews_jobs.lock().unwrap().pop().unwrap();
    assert!(!second.cancel.load(Ordering::Relaxed) && second.request.query == "lunch");
    // Once it has ended, nothing of it is kept to be called off.
    drop(ews_jobs.lock().unwrap().drain(..).collect::<Vec<_>>());
    drop(jmap_jobs.lock().unwrap().drain(..).collect::<Vec<_>>());
    app.drive_server_search();
    assert!(!app.server_searching.busy());
    app.server_searching.clear();
    assert!(!second.cancel.load(Ordering::Relaxed));
}
