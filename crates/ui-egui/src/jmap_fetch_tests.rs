//! A JMAP account's messages fetched whole in the window: a host that answers by hand, the message the
//! owner opens (at once), a reply that waits for its message, the Inbox's newest in the background, what the
//! server no longer has, and the host of each kind of account being its own. Invented people at `example.org`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::jmap_mail::apply_jmap_mail;
use mailcraft_model::sync::{
    FolderListing, GraphFetchResult, GraphFetched, GraphFolder, GraphMessage, JmapFetchRequest, JmapMailResult, MessagePage, ServerMessage, Tokens,
};
use mailcraft_model::{Address, FolderKind, Id, Mailbox};
use serde_json::json;

use super::JmapFetchJob;
use crate::graph_fetch::GraphFetchJob;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const ME: &str = "ada@example.org";
/// The session address carries the account's own, as a server's may.
const SESSION: &str = "https://mail.example.org/jmap/session?account=ada@example.org";
const PASSWORD: &str = "hunter2-hunter2";

/// One fetch the host was asked for, and the way to answer it.
struct Fetch {
    request: JmapFetchRequest,
    secret: Option<String>,
    cancel: Arc<AtomicBool>,
    /// What the job prints.
    shown: String,
    to: Sender<GraphFetchResult>,
}

/// The fetches the host was asked for, in order.
type Asked = Arc<Mutex<Vec<Fetch>>>;

fn mail(id: &str, received: i64) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), size: 2_000, ..Default::default() },
        message_id: format!("<{id}@mail.example.org>"),
        subject: format!("Subject of {id}"),
        from: Address::new("Grace", "grace@example.org"),
        sent: received,
        received,
        preview: format!("Preview of {id}"),
        ..Default::default()
    }
}

/// A window with a JMAP account whose Inbox has `n` messages, `E1` (oldest) to `E<n>`, none of them fetched, and a
/// host that keeps every fetch for the test to answer. `host_holds_credentials`: the host holds the credential
/// itself; otherwise the password has been typed this session. The background is held back, as after Stop, so that
/// the test sees only what it makes happen.
fn window(host_holds_credentials: bool, n: i64) -> (MailApp, Id, Asked) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let asked: Asked = Default::default();
    let log = asked.clone();
    app.services.jmap_fetch = Some(Box::new(move |job: JmapFetchJob| {
        let (tx, rx) = mpsc::channel();
        let shown = format!("{job:?} {job:#?}");
        log.lock().unwrap().push(Fetch { request: job.request, secret: job.secret, cancel: job.cancel, shown, to: tx });
        rx
    }));
    app.services.jmap_host_credentials = host_holds_credentials;
    let added = app.execute("account.add", json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION, "bearer": false}}));
    let account = added.unwrap()["id"].as_u64().unwrap();
    if !host_holds_credentials {
        app.adopt_password(account, PASSWORD.into());
    }
    let folders = vec![
        GraphFolder { id: "Min".into(), name: "INBOX".into(), kind: FolderKind::Inbox, ..Default::default() },
        GraphFolder { id: "Mpr".into(), name: "Projects".into(), kind: FolderKind::Custom, ..Default::default() },
    ];
    let listing = JmapMailResult { account, email: ME.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
    apply_jmap_mail(&mut app.session, listing).unwrap();
    let messages = (1..=n).map(|i| mail(&format!("E{i}"), 1_790_000_000 + i)).collect();
    let page = MessagePage { folder: "Min".into(), messages, afresh: true, ..Default::default() };
    apply_jmap_mail(&mut app.session, JmapMailResult { account, email: ME.into(), pages: vec![page], whole: true, ..Default::default() }).unwrap();
    app.session.sync_progress.full_paused = true;
    (app, account, asked)
}

fn id_of(app: &MailApp, server_id: &str) -> Id {
    app.session.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|s| s.id == server_id)).map(|m| m.id).unwrap()
}

fn mime(id: &str) -> Vec<u8> {
    format!(
        "From: Grace <grace@example.org>\r\nTo: {ME}\r\nSubject: Whole {id}\r\nMessage-ID: <{id}@mail.example.org>\r\nDate: Wed, 14 Oct 2026 09:00:00 +0000\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe words of {id}.\r\n"
    )
    .into_bytes()
}

/// What a host brings when it has fetched `ids`.
fn fetched(account: Id, ids: &[String]) -> GraphFetchResult {
    let fetched = ids.iter().map(|id| GraphFetched { id: id.clone(), mime: mime(id) }).collect();
    GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }
}

/// Hand the host's `run`-th fetch an answer.
fn answer(asked: &Asked, run: usize, result: GraphFetchResult) {
    asked.lock().unwrap()[run].to.send(result).unwrap();
}

fn jobs(asked: &Asked) -> usize {
    asked.lock().unwrap().len()
}

fn ids_of(asked: &Asked, run: usize) -> Vec<String> {
    asked.lock().unwrap()[run].request.ids.clone()
}

fn whole(app: &MailApp, server_id: &str) -> bool {
    !app.session.mailbox.message(id_of(app, server_id)).unwrap().unfetched
}

#[test]
fn a_message_the_owner_opens_is_fetched_at_once_and_shows_whole() {
    let (mut app, account, asked) = window(true, 3);
    let opened = id_of(&app, "E2");
    app.session.sel.messages = vec![opened];
    app.drive_fetch();
    {
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        let job = &asked[0];
        let request = &job.request;
        assert_eq!((request.account, request.email.as_str(), request.session.as_str()), (account, ME, SESSION));
        assert_eq!((request.ids.clone(), request.bearer, request.background), (vec!["E2".to_string()], false, false));
        // The host holds the credential: the job carries none.
        assert_eq!(job.secret, None);
        assert!(!job.cancel.load(Ordering::Relaxed));
    }
    assert!(app.fetching.fetching(opened) && app.fetching.busy());
    // Under way: it is not asked for again.
    app.drive_fetch();
    assert_eq!(jobs(&asked), 1);
    assert!(!whole(&app, "E2"));
    // The answer comes: the message is whole, in its folder, with the marks it had.
    answer(&asked, 0, fetched(account, &["E2".into()]));
    app.drive_fetch();
    let m = app.session.mailbox.message(opened).unwrap();
    assert!(!m.unfetched && m.body.text.contains("The words of E2."), "{:?}", m.body.text);
    assert_eq!((m.subject.as_str(), m.read), ("Whole E2", false));
    assert!(!app.fetching.fetching(opened) && !app.fetching.busy() && !app.fetching.offline);
    // The others are as they were: the background is held back.
    assert!(whole(&app, "E2") && !whole(&app, "E1") && !whole(&app, "E3"));
    assert_eq!(jobs(&asked), 1);
    // Another message is asked for, and once it has come, showing the first again asks for nothing.
    app.session.sel.messages = vec![id_of(&app, "E1")];
    app.drive_fetch();
    assert_eq!(ids_of(&asked, 1), ["E1"]);
    answer(&asked, 1, fetched(account, &["E1".into()]));
    app.drive_fetch();
    assert!(whole(&app, "E1") && !app.fetching.busy());
    app.session.sel.messages = vec![opened];
    app.drive_fetch();
    assert_eq!(jobs(&asked), 2, "E2 was whole already");
}

#[test]
fn the_secret_of_the_job_is_the_accounts_unless_the_host_holds_the_credential() {
    // A password typed this session goes along with the job, and is in nothing the job prints.
    let (mut app, account, asked) = window(false, 3);
    app.session.sel.messages = vec![id_of(&app, "E1")];
    app.drive_fetch();
    assert_eq!(asked.lock().unwrap()[0].secret.as_deref(), Some(PASSWORD));
    for secret in [PASSWORD, "ada@", "example.org", "mail.", "jmap/session", "E1"] {
        assert!(!asked.lock().unwrap()[0].shown.contains(secret), "{secret} in {}", asked.lock().unwrap()[0].shown);
    }
    assert!(asked.lock().unwrap()[0].shown.contains("secret: true"));
    // Nothing under way any more, and the next message is asked for with the secret again...
    answer(&asked, 0, fetched(account, &["E1".into()]));
    app.drive_fetch();
    app.session.sel.messages = vec![id_of(&app, "E2")];
    app.drive_fetch();
    assert_eq!(asked.lock().unwrap().get(1).map(|fetch| fetch.secret.clone()), Some(Some(PASSWORD.to_string())));
    answer(&asked, 1, fetched(account, &["E2".into()]));
    app.drive_fetch();
    // Nobody has signed in this session: nothing is asked for, no password is asked of the owner, and the pane says so.
    let (mut app2, account2, asked2) = window(false, 2);
    app2.passwords.remove(&account2);
    app2.session.sel.messages = vec![id_of(&app2, "E1")];
    app2.drive_fetch();
    assert_eq!(jobs(&asked2), 0);
    assert!(app2.fetching.offline && app2.dialog.is_none());
    // ... until the server turns it down: it is not asked again with it, whoever holds it.
    app.jmap_refused.insert(account);
    app.session.sel.messages = vec![id_of(&app, "E3")];
    app.drive_fetch();
    assert_eq!(jobs(&asked), 2);
    assert!(app.fetching.offline);
    let (mut host_holds, account3, asked3) = window(true, 2);
    host_holds.jmap_refused.insert(account3);
    host_holds.session.sel.messages = vec![id_of(&host_holds, "E1")];
    host_holds.drive_fetch();
    assert_eq!(jobs(&asked3), 0);
}

#[test]
fn a_reply_waits_for_the_message_and_then_opens_with_its_text() {
    let (mut app, account, asked) = window(true, 3);
    let id = id_of(&app, "E3");
    let pending = app.run("mail.reply", json!({"id": id})).unwrap();
    assert_eq!(pending, json!({"pending": "download", "message": id}));
    assert_eq!(app.status_msg.as_ref().map(|(said, _)| said.as_str()), Some("Downloading the message first…"));
    assert_eq!(ids_of(&asked, 0), ["E3"]);
    assert!(app.session.compose.is_none());
    // Not yet: nothing happens.
    app.drive_fetch();
    assert!(app.session.compose.is_none());
    answer(&asked, 0, fetched(account, &["E3".into()]));
    app.drive_fetch();
    let compose = app.session.compose.as_ref().unwrap();
    assert!(compose.subject.contains("Whole E3"), "{}", compose.subject);
    assert!(whole(&app, "E3"));
    // Forward, Save Message As and the rest wait the same way (the same gate).
    let two = id_of(&app, "E2");
    app.session.compose = None;
    assert_eq!(app.run("mail.forward", json!({"id": two})).unwrap()["pending"], "download");
    assert_eq!(ids_of(&asked, 1), ["E2"]);
}

#[test]
fn a_reply_to_a_message_that_cant_be_asked_for_says_so() {
    // The host has no task for JMAP.
    let (mut app, _, asked) = window(true, 2);
    app.services.jmap_fetch = None;
    let said = app.run("mail.reply", json!({"id": id_of(&app, "E2")})).unwrap_err();
    assert_eq!(said, "This message isn't on this computer yet. It will download when you're online.");
    assert!(app.fetching.offline && jobs(&asked) == 0);
    // A message the account hasn't an id for (a moved one, waiting to be found again).
    let (mut app, _, asked) = window(true, 2);
    let id = id_of(&app, "E1");
    app.session.mailbox.message_mut(id).unwrap().server.as_mut().unwrap().id = String::new();
    assert!(app.run("mail.reply", json!({"id": id})).is_err());
    assert_eq!(jobs(&asked), 0);
    // And a message that is whole is replied to at once.
    let (mut app, account, asked) = window(true, 2);
    app.session.sel.messages = vec![id_of(&app, "E1")];
    app.drive_fetch();
    answer(&asked, 0, fetched(account, &["E1".into()]));
    app.drive_fetch();
    let answered = app.run("mail.reply", json!({"id": id_of(&app, "E1")})).unwrap();
    assert!(answered.get("pending").is_none(), "{answered}");
}

#[test]
fn a_message_the_server_no_longer_has_stays_as_it_is_and_is_asked_for_again_when_opened_again() {
    let (mut app, account, asked) = window(true, 3);
    let opened = id_of(&app, "E2");
    app.session.sel.messages = vec![opened];
    app.drive_fetch();
    answer(&asked, 0, GraphFetchResult { account, email: ME.into(), missing: vec!["E2".into()], ..Default::default() });
    app.drive_fetch();
    // Still its headers and preview, no longer downloading, and nothing says the server could not be reached.
    assert!(!whole(&app, "E2") && !app.fetching.fetching(opened) && !app.fetching.offline);
    assert_eq!(jobs(&asked), 1);
    // The background leaves it alone from now on...
    app.session.sync_progress.full_paused = false;
    app.drive_fetch();
    assert_eq!(jobs(&asked), 2);
    assert_eq!(ids_of(&asked, 1), ["E3", "E1"]);
    // ... but opening it again asks again (the message may be back under that name).
    app.session.sync_progress.full_paused = true;
    answer(&asked, 1, fetched(account, &["E3".into(), "E1".into()]));
    app.drive_fetch();
    app.session.sel.messages = vec![id_of(&app, "E1")];
    app.drive_fetch();
    app.session.sel.messages = vec![opened];
    app.drive_fetch();
    assert_eq!(jobs(&asked), 3);
    assert_eq!(ids_of(&asked, 2), ["E2"]);
}

#[test]
fn the_background_fetches_the_inboxs_newest_a_few_at_a_time_and_says_how_far_it_is() {
    let (mut app, account, asked) = window(true, 60);
    app.session.sync_progress.full_paused = false;
    app.drive_fetch();
    assert_eq!(jobs(&asked), 1);
    let first = ids_of(&asked, 0);
    assert_eq!(first, (51..=60).rev().map(|i| format!("E{i}")).collect::<Vec<_>>());
    assert!(asked.lock().unwrap()[0].request.background);
    // One batch at a time.
    app.drive_fetch();
    assert_eq!(jobs(&asked), 1);
    // The forty are what counts, not the sixty.
    answer(&asked, 0, fetched(account, &first));
    app.drive_fetch();
    assert_eq!(app.fetching.progress, "Downloading 10 of 40");
    assert_eq!(jobs(&asked), 2);
    assert_eq!(ids_of(&asked, 1), (41..=50).rev().map(|i| format!("E{i}")).collect::<Vec<_>>());
    // Stop: the batch under way is told, and nothing new starts until a Send/Receive.
    app.pause_fetch();
    assert!(asked.lock().unwrap()[1].cancel.load(Ordering::Relaxed));
    let second = ids_of(&asked, 1);
    answer(&asked, 1, fetched(account, &second[..3]));
    app.drive_fetch();
    app.drive_fetch();
    assert_eq!(jobs(&asked), 2);
    app.resume_fetch();
    app.drive_fetch();
    assert_eq!(jobs(&asked), 3);
    assert!(!asked.lock().unwrap()[2].cancel.load(Ordering::Relaxed), "a fresh batch is not stopped");
    // The three the host brought are not asked for again; the seven it didn't are.
    assert_eq!(ids_of(&asked, 2).len(), 10);
    assert!(ids_of(&asked, 2).iter().all(|id| !second[..3].contains(id)));
    // Everything the forty need, and then nothing: the other twenty wait until they are opened.
    let mut run = 2;
    loop {
        let ids = ids_of(&asked, run);
        answer(&asked, run, fetched(account, &ids));
        app.drive_fetch();
        if jobs(&asked) == run + 1 {
            break;
        }
        run += 1;
    }
    assert!(whole(&app, "E21") && !whole(&app, "E20") && !whole(&app, "E1"));
    assert_eq!(app.fetching.progress, "");
    assert!(!app.fetching.busy());
}

#[test]
fn mail_that_comes_down_is_looked_at_for_fetching_at_once() {
    // Nothing to fetch yet: the background waits a while before it looks again.
    let (mut app, account, asked) = window(true, 0);
    app.session.sync_progress.full_paused = false;
    app.drive_fetch();
    assert_eq!(jobs(&asked), 0);
    // A run brings the Inbox's window: the first batch does not wait for that while to be over.
    let (tx, rx) = mpsc::channel();
    app.jmap_rx = Some((account, false, rx));
    let messages = (1..=3).map(|i| mail(&format!("E{i}"), 1_790_000_000 + i)).collect();
    let page = MessagePage { folder: "Min".into(), messages, afresh: true, ..Default::default() };
    tx.send(JmapMailResult { email: ME.into(), pages: vec![page], whole: true, ..Default::default() }).unwrap();
    app.poll_jmap();
    app.drive_fetch();
    assert_eq!(jobs(&asked), 1);
    assert_eq!(ids_of(&asked, 0), ["E3", "E2", "E1"]);
}

#[test]
fn a_host_that_cant_reach_the_server_is_left_alone_for_a_while() {
    let (mut app, account, asked) = window(true, 30);
    app.session.sync_progress.full_paused = false;
    app.drive_fetch();
    assert_eq!(jobs(&asked), 1);
    // (Nothing else in the answer says to wait: it is the network's alone.)
    let offline = GraphFetchResult { account, email: ME.into(), offline: true, ..Default::default() };
    answer(&asked, 0, offline);
    app.drive_fetch();
    assert!(app.fetching.offline);
    // It tries again in a while, not at once, and nothing else is asked meanwhile.
    for _ in 0..5 {
        app.drive_fetch();
    }
    assert_eq!(jobs(&asked), 1);
    // A Send/Receive starts the waits afresh.
    app.resume_fetch();
    app.drive_fetch();
    assert_eq!(jobs(&asked), 2);
    // A sender that is dropped before an answer is a host that could not be reached.
    let dropped = asked.lock().unwrap().pop().map(|fetch| fetch.to);
    drop(dropped);
    app.drive_fetch();
    assert!(app.fetching.offline && !app.fetching.busy());
}

#[test]
fn a_busy_server_is_given_the_time_it_asked_for() {
    let (mut app, account, asked) = window(true, 30);
    app.session.sync_progress.full_paused = false;
    app.drive_fetch();
    // (Nothing else in the answer says to wait: it is the server's word alone.)
    let busy = GraphFetchResult { account, email: ME.into(), busy_for: Some(120), ..Default::default() };
    answer(&asked, 0, busy);
    app.drive_fetch();
    for _ in 0..5 {
        app.drive_fetch();
    }
    assert_eq!(jobs(&asked), 1, "it asked to be left alone");
}

/// The fetches a Microsoft account's host was asked for, and the way to answer each.
type GraphJobs = Arc<Mutex<Vec<(GraphFetchJob, Sender<GraphFetchResult>)>>>;

/// A window with a JMAP account (an Inbox of `E1` and `E2`, not fetched) and a Microsoft account beside it (one message,
/// not fetched, and a token of this session), and a host for each kind of account that keeps every fetch for the test to
/// answer, unless it is left out: the window, the Microsoft account's message, and what each host was asked for.
fn mixed_window(graph_host: bool, jmap_host: bool) -> (MailApp, Id, Asked, GraphJobs) {
    let (mut app, _, asked) = window(true, 2);
    if !jmap_host {
        app.services.jmap_fetch = None;
    }
    let microsoft = app.execute("account.add", json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = app.session.mailbox.folder_of_kind(microsoft, FolderKind::Inbox).unwrap();
    let server = ServerMessage { id: "AAkA1".into(), folder: "inbox".into(), ..Default::default() };
    let message = mailcraft_model::Message {
        account: microsoft,
        folder: inbox,
        received: NOW - 60,
        unfetched: true,
        server: Some(server),
        ..Default::default()
    };
    let graph_message = app.session.mailbox.add_message(message).unwrap();
    let tokens = Tokens { access: "stand-in".into(), refresh: "r".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    app.seed_graph_token(microsoft, tokens);
    let graph_jobs: GraphJobs = Default::default();
    if graph_host {
        let kept = graph_jobs.clone();
        app.services.graph_fetch = Some(Box::new(move |job: GraphFetchJob| {
            let (tx, rx) = mpsc::channel();
            kept.lock().unwrap().push((job, tx));
            rx
        }));
    }
    (app, graph_message, asked, graph_jobs)
}

fn graph_jobs(jobs: &GraphJobs) -> usize {
    jobs.lock().unwrap().len()
}

#[test]
fn each_kind_of_account_is_fetched_for_by_its_own_host() {
    let (mut app, graph_message, asked, graph) = mixed_window(true, true);
    // The Microsoft message goes to the Graph host, with the token...
    app.session.sel.messages = vec![graph_message];
    app.drive_fetch();
    assert_eq!((graph_jobs(&graph), jobs(&asked)), (1, 0));
    assert_eq!(graph.lock().unwrap()[0].0.access, "stand-in");
    let microsoft = graph.lock().unwrap()[0].0.request.account;
    graph.lock().unwrap()[0].1.send(GraphFetchResult { account: microsoft, ..Default::default() }).unwrap();
    app.drive_fetch();
    // ... and the JMAP message to the JMAP host, with no token.
    app.session.sel.messages = vec![id_of(&app, "E1")];
    app.drive_fetch();
    assert_eq!((graph_jobs(&graph), jobs(&asked)), (1, 1));
    assert_eq!(ids_of(&asked, 0), ["E1"]);
    assert_eq!(asked.lock().unwrap()[0].secret, None);
}

#[test]
fn a_host_with_a_task_for_one_kind_only_is_asked_for_that_kind_only() {
    // Only Graph's: the JMAP message is asked of nobody, and the Microsoft one still is.
    let (mut app, graph_message, asked, graph) = mixed_window(true, false);
    app.session.sel.messages = vec![id_of(&app, "E1")];
    app.drive_fetch();
    assert_eq!((graph_jobs(&graph), jobs(&asked)), (0, 0));
    assert!(app.fetching.offline);
    app.session.sel.messages = vec![graph_message];
    app.drive_fetch();
    assert_eq!((graph_jobs(&graph), jobs(&asked)), (1, 0));
    // Only JMAP's: the other way round.
    let (mut app, graph_message, asked, graph) = mixed_window(false, true);
    app.session.sel.messages = vec![graph_message];
    app.drive_fetch();
    assert_eq!((graph_jobs(&graph), jobs(&asked)), (0, 0));
    assert!(app.fetching.offline);
    app.session.sel.messages = vec![id_of(&app, "E1")];
    app.drive_fetch();
    assert_eq!((graph_jobs(&graph), jobs(&asked)), (0, 1));
    // Neither: nothing is asked of anybody, and nothing breaks.
    let (mut app, graph_message, asked, graph) = mixed_window(false, false);
    for message in [graph_message, id_of(&app, "E1")] {
        app.session.sel.messages = vec![message];
        app.drive_fetch();
    }
    assert_eq!((graph_jobs(&graph), jobs(&asked)), (0, 0));
}

#[test]
fn what_a_fetch_brought_for_another_address_is_not_taken_in() {
    let (mut app, account, asked) = window(true, 2);
    app.session.sel.messages = vec![id_of(&app, "E1")];
    app.drive_fetch();
    // An answer that names another account's address (the mailbox was replaced while it was under way).
    let mut stranger = fetched(account, &["E1".into()]);
    stranger.email = "eve@example.org".into();
    answer(&asked, 0, stranger);
    app.drive_fetch();
    assert!(!whole(&app, "E1"));
    assert!(!app.fetching.busy());
}
