// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Inbox pass in the window (#276): which accounts have one, that it comes before the other
//! phases of the round, what Stop and Skip do to it, and that the run after it starts from the
//! Inbox's link the pass moved on. The host is a fake that notes every job; every address is
//! invented.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::apply_graph_mail;
use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, GraphMessage, MessagePage, ServerMessage, SyncPhase, Timings, Tokens};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::{GraphRequest, GraphResult, MailApp, Services};

const OWNER: &str = "noor@example.org";
const SHARED: &str = "support@example.org";
const FIRST_LINK: &str = "https://graph.microsoft.com/v1.0/me/mailFolders/in/messages/delta?$deltatoken=one";
const NEXT_LINK: &str = "https://graph.microsoft.com/v1.0/me/mailFolders/in/messages/delta?$deltatoken=two";

/// What a Graph job was asked: for whom, whether it was an Inbox pass, and the link it was given
/// for the Inbox.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Job {
    account: Id,
    pass: bool,
    inbox_link: Option<String>,
}

/// The fake host: notes the jobs, answers a pass with one new message and a link moved on, and a
/// run with its end. With `hold`, it keeps the first job's pieces and answer until told.
#[derive(Default)]
struct Host {
    jobs: Vec<Job>,
    hold: bool,
    held: Option<(mpsc::SyncSender<GraphMailResult>, mpsc::Sender<GraphResult>)>,
}

type Shared = Arc<Mutex<Host>>;

fn tokens() -> Tokens {
    Tokens { access: "own-access".into(), refresh: "ref-1".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

fn message(id: &str) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: "in".into(), ..Default::default() },
        subject: "Hello".into(),
        received: 1_790_000_000,
        sent: 1_790_000_000,
        ..Default::default()
    }
}

/// Give `account` an Inbox known to the server (`in`), with a delta link when `link`.
fn seed_inbox(app: &mut MailApp, account: Id, email: &str, link: bool) {
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let pages =
        if link { vec![MessagePage { folder: "in".into(), link: FIRST_LINK.into(), caught_up: true, ..Default::default() }] } else { Vec::new() };
    let seed =
        GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), pages, ..Default::default() };
    apply_graph_mail(&mut app.session, seed).unwrap();
}

/// A window with the owner's account, signed in this session, and a shared mailbox under it;
/// each Inbox has a delta link when `links` says so (owner, shared).
fn window(links: (bool, bool)) -> (MailApp, Id, Id, Shared) {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    // (No round of its own: the tests start them.)
    app.auto_sync = false;
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.tokens.insert(owner, tokens());
    let shared = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED})).unwrap()["id"].as_u64().unwrap();
    seed_inbox(&mut app, owner, OWNER, links.0);
    seed_inbox(&mut app, shared, SHARED, links.1);
    let host: Shared = Default::default();
    let note = host.clone();
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        let mut host = note.lock().unwrap();
        let mail = req.mail.clone().unwrap_or_default();
        let (account, email) = (req.account.id, req.account.email.clone());
        let pass = mail.inbox_pass.is_some();
        host.jobs.push(Job { account, pass, inbox_link: mail.links.iter().find(|l| l.folder == "in").map(|l| l.link.clone()) });
        let (tx, rx) = mpsc::channel();
        let pieces = req.mail_pieces.clone().unwrap();
        if host.hold && host.held.is_none() {
            host.held = Some((pieces, tx));
            return rx;
        }
        let mut timings = Timings::default();
        timings.add(if pass { SyncPhase::Inbox } else { SyncPhase::NewMail }, 120, 1);
        let piece = if pass {
            let page = MessagePage {
                folder: "in".into(),
                messages: vec![message(&format!("m-{account}"))],
                link: NEXT_LINK.into(),
                caught_up: true,
                ..Default::default()
            };
            GraphMailResult { account, email, pages: vec![page], partial: true, inbox_pass: true, timings, ..Default::default() }
        } else {
            GraphMailResult { account, email, timings, ..Default::default() }
        };
        pieces.send(piece).unwrap();
        drop(pieces);
        tx.send(GraphResult { account, ..Default::default() }).unwrap();
        rx
    }));
    // The desktop host can open mail connections; nothing here is kept in step by them.
    app.services.sync = Some(Box::new(|_req: mailcraft_model::sync::SyncRequest| mpsc::channel().1));
    (app, owner, shared, host)
}

/// Poll and start what is next, until the round is over (or `most` frames have gone).
fn drive(app: &mut MailApp, most: usize) {
    for _ in 0..most {
        app.poll_graph();
        app.auto_sync();
        let over = app.graph_rx.is_none() && app.sync_queue.is_empty() && app.inbox_queue.is_empty();
        if over && !app.session.sync_progress.running() {
            break;
        }
    }
}

fn row(app: &MailApp, account: Id) -> serde_json::Value {
    let all = app.session.sync_progress.json(&app.session.mailbox);
    all["accounts"].as_array().unwrap().iter().find(|r| r["id"] == json!(account)).cloned().unwrap()
}

fn jobs(host: &Shared) -> Vec<Job> {
    host.lock().unwrap().jobs.clone()
}

fn count_hello(app: &MailApp, account: Id) -> usize {
    app.session.mailbox.messages.values().filter(|m| m.account == account && m.subject == "Hello").count()
}

#[test]
fn every_graph_accounts_inbox_pass_comes_before_the_other_phases_of_the_round() {
    let (mut app, owner, shared, host) = window((true, true));
    app.execute("ui.sync", json!({})).unwrap();
    drive(&mut app, 200);
    let jobs = jobs(&host);
    let pass = |account| Job { account, pass: true, inbox_link: Some(FIRST_LINK.into()) };
    // The owner's and the shared mailbox's passes first, then each account's own run, which goes
    // on from the link the pass moved on: the Inbox is not read again.
    let run = |account| Job { account, pass: false, inbox_link: Some(NEXT_LINK.into()) };
    assert_eq!(jobs, [pass(owner), pass(shared), run(owner), run(shared)]);
    // What the passes brought is in, once each.
    assert_eq!((count_hello(&app, owner), count_hello(&app, shared)), (1, 1));
    // The round is over, every account done, and the Inbox's time is its own phase in each row's
    // timings, first (the run's own time is New mail's).
    assert!(!app.session.sync_progress.running());
    for account in [owner, shared] {
        let r = row(&app, account);
        assert_eq!(r["state"], json!("done"), "{r}");
        assert_eq!(r["timings"]["phases"][0]["phase"], json!("inbox"), "{r}");
        assert_eq!(r["timings"]["phases"][0]["requests"], json!(1), "{r}");
        assert_eq!(r["new"], json!(1), "what the pass brought is counted");
    }
}

#[test]
fn an_account_with_no_stored_inbox_link_has_no_pass() {
    // The owner was never in step; the shared mailbox was.
    let (mut app, owner, shared, host) = window((false, true));
    app.execute("ui.sync", json!({})).unwrap();
    drive(&mut app, 200);
    let jobs = jobs(&host);
    assert_eq!(jobs.iter().filter(|j| j.pass).map(|j| j.account).collect::<Vec<_>>(), [shared]);
    assert_eq!(jobs.iter().filter(|j| !j.pass).map(|j| j.account).collect::<Vec<_>>(), [owner, shared]);
    assert_eq!(jobs[0], Job { account: shared, pass: true, inbox_link: Some(FIRST_LINK.into()) });
    assert!(mailcraft_engine::cmd::graph_mail::graph_inbox_request(&app.session, owner).is_none());
    // Nor a pass for a round under way, whose Inbox link is the next page of it.
    let owner_inbox = app.session.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap();
    app.session.mailbox.folder_mut(owner_inbox).unwrap().server.as_mut().unwrap().delta = FIRST_LINK.into();
    assert!(mailcraft_engine::cmd::graph_mail::graph_inbox_request(&app.session, owner).is_none(), "not caught up");
}

#[test]
fn stop_during_the_pass_ends_the_run() {
    let (mut app, owner, shared, host) = window((true, true));
    host.lock().unwrap().hold = true;
    app.execute("ui.sync", json!({})).unwrap();
    // The owner's pass is under way, held by the host; the others wait their turn.
    assert_eq!(jobs(&host).len(), 1);
    assert!(jobs(&host)[0].pass);
    assert_eq!(row(&app, owner)["phase"], json!("inbox"));
    app.execute("ui.cancelSync", json!({})).unwrap();
    assert!(app.sync_queue.is_empty() && app.inbox_queue.is_empty(), "nothing is left in line");
    // The host stops the pass after its page in hand, and answers.
    let (pieces, answer) = host.lock().unwrap().held.take().unwrap();
    drop(pieces);
    answer.send(GraphResult { account: owner, ..Default::default() }).unwrap();
    drive(&mut app, 50);
    // Not another job: no second pass, no run.
    assert_eq!(jobs(&host).len(), 1, "{:?}", jobs(&host));
    assert!(!app.session.sync_progress.running());
    assert_eq!(row(&app, owner)["state"], json!("stopped"));
    assert_eq!(row(&app, shared)["state"], json!("stopped"));
}

#[test]
fn skip_leaves_out_an_accounts_other_phases_but_never_its_pass() {
    let (mut app, owner, shared, host) = window((true, true));
    host.lock().unwrap().hold = true;
    app.execute("ui.sync", json!({})).unwrap();
    assert_eq!(row(&app, shared)["state"], json!("waiting"));
    // The shared mailbox is skipped while the owner's pass is under way.
    assert_eq!(app.execute("ui.skipSync", json!({"account": shared})).unwrap(), json!({"skipped": shared}));
    let (pieces, answer) = host.lock().unwrap().held.take().unwrap();
    pieces
        .send(GraphMailResult {
            account: owner,
            email: OWNER.into(),
            partial: true,
            inbox_pass: true,
            pages: vec![MessagePage { folder: "in".into(), link: NEXT_LINK.into(), caught_up: true, ..Default::default() }],
            ..Default::default()
        })
        .unwrap();
    drop(pieces);
    answer.send(GraphResult { account: owner, ..Default::default() }).unwrap();
    host.lock().unwrap().hold = false;
    drive(&mut app, 200);
    let jobs = jobs(&host);
    // Its pass ran; its run did not.
    assert!(jobs.iter().any(|j| j.account == shared && j.pass), "{jobs:?}");
    assert!(!jobs.iter().any(|j| j.account == shared && !j.pass), "{jobs:?}");
    assert!(jobs.iter().any(|j| j.account == owner && !j.pass));
    assert_eq!(count_hello(&app, shared), 1, "what its pass brought is in");
    let r = row(&app, shared);
    assert_eq!(r["state"], json!("skipped"), "{r}");
    assert_eq!(r["timings"]["phases"][0]["phase"], json!("inbox"), "{r}");
}

#[test]
fn a_round_that_started_by_itself_asks_nobody_for_a_pass() {
    // The owner's sign-in is not at hand (nothing kept, nothing remembered): no pass for either,
    // and nothing is asked.
    let (mut app, owner, shared, host) = window((true, true));
    app.tokens.remove(&owner);
    app.queue_inbox_passes(&[owner, shared], true);
    assert!(app.start_next_inbox_pass().is_none());
    assert!(host.lock().unwrap().jobs.is_empty() && app.signing_in.is_none() && app.inbox_queue.is_empty());
}
