// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account (#252, E6) read by the window the whole way: Send/Receive through the
//! host's own runner (`ews_runner`, the one the desktop app uses) and `mailcraft_net::https`, to a
//! loopback server that answers with the strict stand-in for Exchange Web Services, each piece taken
//! in as it comes; a message opened, and one replied to, fetched whole; a delta; a password the
//! server turns down. Nothing here leaves the machine; every name, address and id is invented.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_ews::standin::{StandIn, block_on};
use mailcraft_ews::{HttpExchange, HttpRequest, Secret};
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::{Value, json};

const ME: &str = "ada@example.org";
const USER: &str = "CORP\\ada";
const PASSWORD: &str = "an invented password";
/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// Serves the stand-in over HTTP on this machine until the test is over: one request per
/// connection. Its address.
struct Server {
    url: String,
    standin: Arc<StandIn>,
    stop: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // (Wakes the accept that waits.)
        let _ = TcpStream::connect(self.url.trim_start_matches("http://").split('/').next().unwrap_or_default());
    }
}

fn serve() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/EWS/Exchange.asmx", listener.local_addr().unwrap());
    let standin = Arc::new(StandIn::new());
    standin.state().authorization = Secret::basic(USER, PASSWORD).unwrap().expose().to_owned();
    let (stop, there, at) = (Arc::new(AtomicBool::new(false)), standin.clone(), url.clone());
    let halt = stop.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if halt.load(Ordering::Relaxed) {
                return;
            }
            let Ok(stream) = stream else { continue };
            let (there, at) = (there.clone(), at.clone());
            std::thread::spawn(move || {
                let _ = answer(stream, &there, &at);
            });
        }
    });
    Server { url, standin, stop }
}

/// One request, read as HTTP, answered by the stand-in.
fn answer(mut stream: TcpStream, standin: &StandIn, url: &str) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            return Ok(());
        }
        head.extend_from_slice(&byte);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let field = |name: &str| {
        head.lines().find_map(|line| line.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_owned()))
    };
    let length: usize = field("content-length").and_then(|n| n.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body)?;
    let authorization = field("authorization").unwrap_or_default();
    let request = HttpRequest::soap(url, &Secret::new(&authorization), String::from_utf8_lossy(&body).into_owned()).unwrap();
    let response = block_on(standin.exchange(&request)).unwrap().unwrap();
    let mut out = format!(
        "HTTP/1.1 {} Answer\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/xml\r\n",
        response.status(),
        response.body().len()
    );
    if response.status() == 401 {
        out.push_str("WWW-Authenticate: Basic realm=\"exchange\"\r\n");
    }
    out.push_str("\r\n");
    stream.write_all(out.as_bytes())?;
    stream.write_all(response.body())?;
    stream.flush()
}

/// A window with one Exchange server account on `server`, and the host's own runners.
fn window(server: &Server) -> (MailApp, Id) {
    let mut session = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    session.ews = true;
    let mut app = MailApp::new(session, Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let ctx = egui::Context::default();
    let (run_ctx, fetch_ctx) = (ctx.clone(), ctx);
    app.services.ews = Some(Box::new(move |job| crate::ews_runner::run_mail(job, &run_ctx)));
    app.services.ews_fetch = Some(Box::new(move |job| crate::ews_runner::run_fetch(job, &fetch_ctx)));
    let added = app.execute("account.add", json!({"email": ME, "kind": "exchange", "ews": {"url": server.url, "username": USER}})).unwrap();
    (app, added["id"].as_u64().unwrap())
}

/// Send/Receive as the owner asks for it, to its end; the background download is then held back
/// (as after Stop), so that what is opened next isn't here whole yet. What `ui.sync` answered.
fn send_receive(app: &mut MailApp, params: Value) -> Value {
    let started = app.execute("ui.sync", params).unwrap();
    for _ in 0..6_000 {
        app.poll_sync();
        if !app.syncing() {
            app.session.sync_progress.full_paused = true;
            return started;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the run didn't end");
}

fn until(app: &mut MailApp, done: &dyn Fn(&MailApp) -> bool) {
    for _ in 0..6_000 {
        app.poll_sync();
        app.drive_fetch();
        if done(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("it didn't happen");
}

fn here(app: &MailApp, account: Id, subject: &str) -> Option<(Id, Id, bool)> {
    app.session.mailbox.messages.values().find(|m| m.account == account && m.subject == subject).map(|m| (m.id, m.folder, m.unfetched))
}

#[test]
fn the_window_reads_an_exchange_account_and_fetches_what_is_opened_or_answered_whole() {
    let server = serve();
    {
        let mut state = server.standin.state();
        for subject in ["Kickoff", "Budget", "Agenda"] {
            state.deliver("inbox", subject, false);
        }
        state.deliver("sentitems", "Old minutes", true);
    }
    let (mut app, account) = window(&server);

    // The first Send/Receive asks for the password, and goes once the owner has typed it.
    assert_eq!(send_receive(&mut app, json!({}))["pending"], "password");
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("password"));
    app.dialog = None;
    assert_eq!(send_receive(&mut app, json!({"account": account, "password": PASSWORD}))["ews"], true);
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);
    // The server's folders, with what a list shows of each message, none of them whole yet.
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    for subject in ["Kickoff", "Budget", "Agenda"] {
        assert_eq!(here(&app, account, subject).map(|(_, folder, unfetched)| (folder, unfetched)), Some((inbox, true)), "{subject}");
    }
    let sent = app.session.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    assert_eq!(here(&app, account, "Old minutes").map(|(_, folder, _)| folder), Some(sent));
    let (kickoff, ..) = here(&app, account, "Kickoff").unwrap();
    assert_eq!(app.session.mailbox.message(kickoff).unwrap().body.text, "Hello about Kickoff.");
    // Send/Receive Progress: the account's row, done, with what came.
    let row = app.session.sync_progress.accounts.iter().find(|r| r.account == account).unwrap();
    assert_eq!((row.state, row.new), (mailcraft_engine::cmd::sync_progress::RunState::Done, 4));
    // Nothing but reading went to the server.
    assert!(server.standin.state().writes.is_empty());
    // (The first request of a run goes with no credential and is told how to sign in: that 401 is all that isn't 200.)
    assert!(server.standin.state().requests.iter().filter(|r| r.status != 200).all(|r| r.status == 401));

    // Opened: fetched whole at once.
    app.session.sel.messages = vec![kickoff];
    until(&mut app, &|app| !app.session.mailbox.message(kickoff).unwrap().unfetched);
    assert!(app.session.mailbox.message(kickoff).unwrap().body.text.contains("Hello about Kickoff."));

    // A delta: new mail, an edit, a read mark, a deletion.
    let (budget_id, agenda_id) = {
        let state = server.standin.state();
        let ids: Vec<String> = state.items.iter().map(|i| i.id.clone()).collect();
        (ids[1].clone(), ids[2].clone())
    };
    {
        let mut state = server.standin.state();
        state.deliver("inbox", "Late", false);
        state.set_read(&budget_id, true);
        state.remove(&agenda_id);
    }
    assert_eq!(send_receive(&mut app, json!({"account": account}))["started"], true);
    let (late, _, unfetched) = here(&app, account, "Late").unwrap();
    assert!(unfetched);
    assert!(here(&app, account, "Budget").is_some() && here(&app, account, "Agenda").is_none(), "what the server dropped is gone here");
    let (budget, ..) = here(&app, account, "Budget").unwrap();
    assert!(app.session.mailbox.message(budget).unwrap().read);
    // (It asked for the changes since the state it had, not for the folder again.)
    assert!(server.standin.state().requests.iter().filter(|r| r.operation == "SyncFolderItems").count() > 7);

    // A reply to one that isn't here whole waits for it, and then opens; the password typed before
    // still goes, nothing is asked again.
    assert_eq!(app.run("mail.reply", json!({"id": late})).unwrap()["pending"], "download");
    until(&mut app, &|app| app.session.compose.is_some());
    assert!(app.session.compose.as_ref().unwrap().subject.contains("Late"));
    assert!(!app.session.mailbox.message(late).unwrap().unfetched);

    // In the background, the rest of the Inbox.
    app.session.sync_progress.full_paused = false;
    until(&mut app, &|app| app.session.mailbox.messages.values().filter(|m| m.folder == inbox).all(|m| !m.unfetched));
}

#[test]
fn a_server_that_forgets_its_states_is_read_afresh_and_nothing_is_lost() {
    let server = serve();
    for subject in ["Kickoff", "Budget"] {
        server.standin.state().deliver("inbox", subject, false);
    }
    let (mut app, account) = window(&server);
    send_receive(&mut app, json!({"account": account, "password": PASSWORD}));
    let (kickoff, ..) = here(&app, account, "Kickoff").unwrap();
    app.session.sel.messages = vec![kickoff];
    until(&mut app, &|app| !app.session.mailbox.message(kickoff).unwrap().unfetched);
    server.standin.state().forget_sync_states();
    let budget = server.standin.state().items[1].id.clone();
    server.standin.state().remove(&budget);
    send_receive(&mut app, json!({"account": account}));
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);
    assert!(here(&app, account, "Budget").is_none(), "what the fresh read lacks is gone");
    let (still, _, unfetched) = here(&app, account, "Kickoff").unwrap();
    assert_eq!((still, unfetched), (kickoff, false), "matched by item id: the same message, still whole");
}

#[test]
fn a_busy_server_is_waited_for_and_the_run_goes_on() {
    let server = serve();
    server.standin.state().deliver("inbox", "Kickoff", false);
    // (The shortest wait a busy server is given is a second.)
    server.standin.state().busy = (1, 1000);
    let (mut app, account) = window(&server);
    send_receive(&mut app, json!({"account": account, "password": PASSWORD}));
    assert!(here(&app, account, "Kickoff").is_some());
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);
}

#[test]
fn a_password_the_server_turns_down_is_forgotten_and_asked_for_again() {
    let server = serve();
    server.standin.state().deliver("inbox", "Kickoff", false);
    let (mut app, account) = window(&server);
    send_receive(&mut app, json!({"account": account, "password": "not-the-password"}));
    assert!(app.session.sync_status.starts_with("The server turned down the sign-in for"), "{}", app.session.sync_status);
    assert!(here(&app, account, "Kickoff").is_none());
    // The next Send/Receive asks again, rather than send the same one once more.
    let asked = server.standin.state().requests.len();
    assert_eq!(send_receive(&mut app, json!({"account": account}))["pending"], "password");
    assert_eq!(server.standin.state().requests.len(), asked);
    app.dialog = None;
    send_receive(&mut app, json!({"account": account, "password": PASSWORD}));
    assert!(here(&app, account, "Kickoff").is_some());
}

#[test]
fn nothing_is_read_while_the_switch_is_off() {
    let server = serve();
    let (mut app, account) = window(&server);
    app.session.ews = false;
    let said = app.execute("ui.sync", json!({"account": account, "password": PASSWORD})).unwrap_err();
    assert!(said.contains("MAILCRAFT_EWS=1"), "{said}");
    assert!(server.standin.state().requests.is_empty());
}

#[test]
fn an_account_whose_server_cannot_be_reached_says_so_and_asks_nothing_of_the_owner() {
    // A server that isn't there: a port nobody listens on.
    let gone = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/EWS/Exchange.asmx", listener.local_addr().unwrap())
    };
    let server = Server { url: gone, standin: Arc::new(StandIn::new()), stop: Arc::new(AtomicBool::new(true)) };
    let (mut app, account) = window(&server);
    send_receive(&mut app, json!({"account": account, "password": PASSWORD}));
    assert!(app.session.sync_status.contains("Couldn't reach the server"), "{}", app.session.sync_status);
    assert!(app.dialog.is_none());
}
