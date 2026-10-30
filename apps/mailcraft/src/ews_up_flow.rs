// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account (#252, E6) taking changes up, saving drafts and sending, through the
//! host's own runner and `mailcraft_net::https`, to a loopback server that answers with the strict
//! stand-in for Exchange Web Services, behind a sign-in that is Basic, NTLM (the three legs tied
//! to one connection, checked against the password with the server's side of MS-NLMP) or Kerberos
//! alone. Nothing here leaves the machine; every name, address and id is invented.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_ews::standin::{StandIn, block_on};
use mailcraft_ews::standin_ntlm::{challenge_token, verifies};
use mailcraft_ews::{HttpExchange, HttpRequest, Secret};
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::{Value, json};

const ME: &str = "ada@example.org";
pub(crate) const USER: &str = "CORP\\ada";
pub(crate) const PASSWORD: &str = "an invented password";
/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Basic,
    Ntlm,
    /// Offers Negotiate and nothing else.
    Kerberos,
}

/// What the server does to a connection, to see what the program does about it.
#[derive(Default)]
struct Misbehaviour {
    /// A connection is closed (the answer says so) after this many requests of the real run.
    close_after: Option<usize>,
    /// The first request of this operation is done, and its answer is never sent: the connection
    /// is dropped.
    drop_answer_of: Option<&'static str>,
}

pub(crate) struct Server {
    pub(crate) url: String,
    pub(crate) standin: Arc<StandIn>,
    stop: Arc<AtomicBool>,
    /// How many connections were accepted, and the schemes of the `Authorization` fields seen.
    connections: Arc<AtomicUsize>,
    pub(crate) schemes: Arc<Mutex<Vec<String>>>,
    misbehaves: Arc<Mutex<Misbehaviour>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.url.trim_start_matches("http://").split('/').next().unwrap_or_default());
    }
}

pub(crate) fn serve(mode: Mode) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/EWS/Exchange.asmx", listener.local_addr().unwrap());
    let standin = Arc::new(StandIn::new());
    standin.state().authorization = Secret::basic(USER, PASSWORD).unwrap().expose().to_owned();
    let server = Server {
        url: url.clone(),
        standin: standin.clone(),
        stop: Arc::new(AtomicBool::new(false)),
        connections: Arc::new(AtomicUsize::new(0)),
        schemes: Arc::new(Mutex::new(Vec::new())),
        misbehaves: Arc::new(Mutex::new(Misbehaviour::default())),
    };
    let (halt, connections, schemes, misbehaves) =
        (server.stop.clone(), server.connections.clone(), server.schemes.clone(), server.misbehaves.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if halt.load(Ordering::Relaxed) {
                return;
            }
            let Ok(stream) = stream else { continue };
            connections.fetch_add(1, Ordering::Relaxed);
            let (standin, url, schemes, misbehaves) = (standin.clone(), url.clone(), schemes.clone(), misbehaves.clone());
            std::thread::spawn(move || {
                let _ = connection(stream, mode, &standin, &url, &schemes, &misbehaves);
            });
        }
    });
    server
}

/// One request, read as HTTP: its head's fields and its body; none when the client hung up.
fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<(Vec<(String, String)>, Vec<u8>)>> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            return Ok(None);
        }
        head.extend_from_slice(&byte);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let fields: Vec<(String, String)> =
        head.lines().filter_map(|line| line.split_once(':').map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))).collect();
    let length: usize = fields.iter().find(|(n, _)| n == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body)?;
    Ok(Some((fields, body)))
}

fn write_answer(stream: &mut TcpStream, status: u16, challenges: &[String], body: &[u8], keep: bool) -> std::io::Result<()> {
    let mut out = format!("HTTP/1.1 {status} Answer\r\nContent-Length: {}\r\nContent-Type: text/xml\r\n", body.len());
    for challenge in challenges {
        out.push_str(&format!("WWW-Authenticate: {challenge}\r\n"));
    }
    out.push_str(if keep { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" });
    stream.write_all(out.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn connection(
    mut stream: TcpStream,
    mode: Mode,
    standin: &StandIn,
    url: &str,
    schemes: &Mutex<Vec<String>>,
    misbehaves: &Mutex<Misbehaviour>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    // NTLM ties the sign-in to the connection: where it is.
    let (mut negotiate, mut challenge, mut signed_in, mut served) = (String::new(), Vec::new(), false, 0usize);
    loop {
        let Some((fields, body)) = read_request(&mut stream)? else { return Ok(()) };
        let authorization = fields.iter().find(|(n, _)| n == "authorization").map(|(_, v)| v.clone()).unwrap_or_default();
        let scheme = authorization.split(' ').next().unwrap_or_default().to_ascii_lowercase();
        // (A request with no credential must carry no body: a message is not uploaded to be turned away.)
        schemes.lock().unwrap().push(if scheme.is_empty() && !body.is_empty() && !signed_in { "anonymous-with-body".into() } else { scheme });
        let offered: Vec<String> = match mode {
            Mode::Basic => vec!["Basic realm=\"exchange\"".into()],
            Mode::Ntlm => vec!["Negotiate".into(), "NTLM".into()],
            Mode::Kerberos => vec!["Negotiate".into()],
        };
        let mut proceed = mode == Mode::Basic;
        match mode {
            Mode::Basic => {}
            Mode::Kerberos => {
                write_answer(&mut stream, 401, &offered, b"", true)?;
                continue;
            }
            Mode::Ntlm => {
                if let Some(token) = authorization.strip_prefix("NTLM ").filter(|_| !signed_in) {
                    if challenge.is_empty() {
                        // The first leg: the challenge.
                        negotiate = token.to_owned();
                        let (text, raw) = challenge_token();
                        challenge = raw;
                        write_answer(&mut stream, 401, &[format!("NTLM {text}")], b"", true)?;
                        continue;
                    }
                    if verifies(PASSWORD, "ada", "CORP", &negotiate, &challenge, token) {
                        signed_in = true;
                    } else {
                        write_answer(&mut stream, 401, &offered, b"", true)?;
                        continue;
                    }
                }
                if !signed_in {
                    write_answer(&mut stream, 401, &offered, b"", true)?;
                    continue;
                }
                proceed = true;
            }
        }
        // (A leg with no body is part of the sign-in, not a request of the service.)
        if proceed && body.is_empty() && mode == Mode::Ntlm {
            write_answer(&mut stream, 401, &offered, b"", true)?;
            continue;
        }
        let secret = if mode == Mode::Basic { Secret::new(&authorization) } else { Secret::new(&standin.state().authorization) };
        let request = HttpRequest::soap(url, &secret, String::from_utf8_lossy(&body).into_owned()).unwrap();
        let operation =
            String::from_utf8_lossy(&body).split("<m:").nth(1).map(|rest| rest.split([' ', '>', '/']).next().unwrap_or_default().to_owned());
        let response = block_on(standin.exchange(&request)).unwrap().unwrap();
        served += 1;
        if response.status() != 401 {
            let mut m = misbehaves.lock().unwrap();
            if m.drop_answer_of.is_some() && m.drop_answer_of == operation.as_deref() {
                m.drop_answer_of = None;
                // Done, and never answered.
                return Ok(());
            }
            let close = m.close_after.is_some_and(|n| served >= n);
            drop(m);
            if close {
                return write_answer(&mut stream, response.status(), &[], response.body(), false);
            }
        }
        let challenges = if response.status() == 401 { vec!["Basic realm=\"exchange\"".to_owned()] } else { Vec::new() };
        let keep = mode != Mode::Basic;
        write_answer(&mut stream, response.status(), &challenges, response.body(), keep)?;
        if !keep {
            return Ok(());
        }
    }
}

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

fn here(app: &MailApp, account: Id, subject: &str) -> Option<Id> {
    app.session.mailbox.messages.values().find(|m| m.account == account && m.subject == subject).map(|m| m.id)
}

fn first_run(app: &mut MailApp, account: Id) {
    send_receive(app, json!({"account": account, "password": PASSWORD}));
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);
}

fn write_and_send(app: &mut MailApp, account: Id, subject: &str) {
    app.run("mail.new", json!({"account": account, "to": "zoe@example.org", "subject": subject})).unwrap();
    app.run("compose.send", json!({})).unwrap();
}

#[test]
fn an_ntlm_server_is_signed_in_to_on_one_connection_and_never_given_the_password_in_basic() {
    let server = serve(Mode::Ntlm);
    server.standin.state().deliver("inbox", "Kickoff", false);
    let (mut app, account) = window(&server);
    first_run(&mut app, account);
    assert!(here(&app, account, "Kickoff").is_some());
    let schemes = server.schemes.lock().unwrap().clone();
    assert!(schemes.iter().all(|s| s.is_empty() || s == "ntlm"), "{schemes:?}");
    // The first request had no credential; then the three legs; then every request on that connection.
    assert!(schemes.first().is_some_and(String::is_empty));
    assert_eq!(server.connections.load(Ordering::Relaxed), 2, "one to learn the offer, one held for the sign-in and the run");
    assert!(server.standin.state().requests.len() > 4);
}

#[test]
fn a_wrong_password_over_ntlm_is_forgotten_and_asked_for_again() {
    let server = serve(Mode::Ntlm);
    server.standin.state().deliver("inbox", "Kickoff", false);
    let (mut app, account) = window(&server);
    send_receive(&mut app, json!({"account": account, "password": "not the password"}));
    assert!(app.session.sync_status.starts_with("The server turned down the sign-in for"), "{}", app.session.sync_status);
    assert!(here(&app, account, "Kickoff").is_none());
    assert_eq!(send_receive(&mut app, json!({"account": account}))["pending"], "password");
}

#[test]
fn a_server_that_signs_in_with_kerberos_alone_is_told_in_words_and_asked_nothing_more() {
    let server = serve(Mode::Kerberos);
    let (mut app, account) = window(&server);
    send_receive(&mut app, json!({"account": account, "password": PASSWORD}));
    assert!(app.session.sync_status.contains("Kerberos"), "{}", app.session.sync_status);
    assert!(!app.session.sync_status.contains("sign in again"));
    assert!(app.dialog.is_none());
    // No credential of any kind was sent.
    assert!(server.schemes.lock().unwrap().iter().all(String::is_empty));
}

#[test]
fn a_connection_the_server_closes_mid_run_is_opened_again_and_signed_in_again() {
    let server = serve(Mode::Ntlm);
    for subject in ["Kickoff", "Budget", "Agenda"] {
        server.standin.state().deliver("inbox", subject, false);
    }
    server.misbehaves.lock().unwrap().close_after = Some(2);
    let (mut app, account) = window(&server);
    first_run(&mut app, account);
    for subject in ["Kickoff", "Budget", "Agenda"] {
        assert!(here(&app, account, subject).is_some(), "{subject}");
    }
    assert!(server.connections.load(Ordering::Relaxed) > 3, "{}", server.connections.load(Ordering::Relaxed));
}

#[test]
fn changes_made_here_go_up_and_undo_goes_up_as_another_change() {
    for mode in [Mode::Basic, Mode::Ntlm] {
        let server = serve(mode);
        for subject in ["Kickoff", "Budget"] {
            server.standin.state().deliver("inbox", subject, false);
        }
        let (mut app, account) = window(&server);
        first_run(&mut app, account);
        let (kickoff, budget) = (here(&app, account, "Kickoff").unwrap(), here(&app, account, "Budget").unwrap());
        let archive = server.standin.state().add_folder("Archive");
        // The folder comes down with the next run.
        send_receive(&mut app, json!({"account": account}));
        let archive_here = app.session.mailbox.folder_by_server(account, &archive).unwrap();
        app.run("mail.markRead", json!({"ids": [kickoff]})).unwrap();
        app.run("mail.move", json!({"ids": [budget], "folder": archive_here})).unwrap();
        send_receive(&mut app, json!({"account": account}));
        {
            let state = server.standin.state();
            assert!(state.items.iter().find(|i| i.subject == "Kickoff").unwrap().read);
            assert_eq!(state.items.iter().find(|i| i.subject == "Budget").unwrap().folder, archive);
        }
        // The move gave the message an id of its own there; the next run knows it by that one.
        send_receive(&mut app, json!({"account": account}));
        let moved = app.session.mailbox.message(here(&app, account, "Budget").unwrap()).unwrap();
        assert_eq!(moved.folder, archive_here);
        assert_eq!(app.session.mailbox.messages.values().filter(|m| m.subject == "Budget").count(), 1);
        // Undo is a reverse change.
        app.run("edit.undo", json!({})).unwrap();
        app.run("edit.undo", json!({})).unwrap();
        send_receive(&mut app, json!({"account": account}));
        let state = server.standin.state();
        let inbox = state.folder_id("inbox").unwrap();
        assert!(!state.items.iter().find(|i| i.subject == "Kickoff").unwrap().read);
        assert_eq!(state.items.iter().find(|i| i.subject == "Budget").unwrap().folder, inbox);
    }
}

#[test]
fn a_message_is_sent_once_and_filed_in_sent_items_over_either_sign_in() {
    for mode in [Mode::Basic, Mode::Ntlm] {
        let server = serve(mode);
        let (mut app, account) = window(&server);
        first_run(&mut app, account);
        write_and_send(&mut app, account, "Quarterly");
        let outbox = app.session.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
        assert_eq!(app.session.mailbox.messages_in(outbox).count(), 1);
        send_receive(&mut app, json!({"account": account}));
        assert_eq!(app.session.mailbox.messages_in(outbox).count(), 0);
        let state = server.standin.state();
        assert_eq!(state.outgoing.len(), 1);
        let sent = state.folder_id("sentitems").unwrap();
        assert!(state.items.iter().any(|i| i.folder == sent && i.subject == "Quarterly"));
        drop(state);
        // The copy comes down; nothing is sent again.
        send_receive(&mut app, json!({"account": account}));
        assert_eq!(server.standin.state().outgoing.len(), 1);
        assert!(here(&app, account, "Quarterly").is_some());
        // No request without a credential carried a body, the message's included.
        assert!(!server.schemes.lock().unwrap().iter().any(|s| s == "anonymous-with-body"));
    }
}

#[test]
fn a_send_whose_answer_was_lost_is_held_and_never_sent_twice() {
    for mode in [Mode::Basic, Mode::Ntlm] {
        let server = serve(mode);
        let (mut app, account) = window(&server);
        first_run(&mut app, account);
        server.misbehaves.lock().unwrap().drop_answer_of = Some("CreateItem");
        write_and_send(&mut app, account, "Quarterly");
        send_receive(&mut app, json!({"account": account}));
        let outbox = app.session.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
        let held = app.session.mailbox.messages_in(outbox).map(|m| m.held.clone()).collect::<Vec<_>>();
        assert_eq!(held.len(), 1);
        assert!(held[0].contains("may have been sent") && held[0].contains("from the server"), "{held:?}");
        assert_eq!(server.standin.state().outgoing.len(), 1, "it went once");
        // More runs: it is held, and is not sent again.
        send_receive(&mut app, json!({"account": account}));
        send_receive(&mut app, json!({"account": account}));
        assert_eq!(server.standin.state().outgoing.len(), 1);
    }
}

#[test]
fn a_draft_is_kept_on_the_server_edited_in_place_and_sent_as_that_draft() {
    let server = serve(Mode::Ntlm);
    let (mut app, account) = window(&server);
    first_run(&mut app, account);
    app.run("mail.new", json!({"account": account, "to": "zoe@example.org", "subject": "Notes"})).unwrap();
    let draft = app.run("compose.saveDraft", json!({})).unwrap()["draft"].as_u64().unwrap();
    send_receive(&mut app, json!({"account": account}));
    {
        let state = server.standin.state();
        let drafts = state.folder_id("drafts").unwrap();
        assert_eq!(state.items.iter().filter(|i| i.folder == drafts && i.subject == "Notes").count(), 1);
    }
    assert!(app.session.mailbox.message(draft).unwrap().server.is_some());
    app.run("compose.set", json!({"subject": "Notes, edited"})).unwrap();
    app.run("compose.saveDraft", json!({})).unwrap();
    send_receive(&mut app, json!({"account": account}));
    assert_eq!(server.standin.state().items.iter().filter(|i| i.draft).count(), 1, "changed in place, not made again");
    assert!(server.standin.state().items.iter().any(|i| i.draft && i.subject == "Notes, edited"));
    // Sent: as the draft the server has.
    app.run("compose.send", json!({})).unwrap();
    send_receive(&mut app, json!({"account": account}));
    let state = server.standin.state();
    assert_eq!(state.outgoing.len(), 1);
    assert!(state.items.iter().all(|i| !i.draft));
}

#[test]
fn what_a_server_without_a_flag_lacks_is_left_as_it_is() {
    let server = serve(Mode::Basic);
    server.standin.state().lacks_flag = true;
    server.standin.state().deliver("inbox", "Kickoff", false);
    let (mut app, account) = window(&server);
    first_run(&mut app, account);
    let kickoff = here(&app, account, "Kickoff").unwrap();
    app.run("mail.flag", json!({"id": kickoff})).unwrap();
    send_receive(&mut app, json!({"account": account}));
    // Still flagged here, nothing complained of, and not asked again every run.
    assert!(app.session.mailbox.message(kickoff).unwrap().flag != mailcraft_model::FlagStatus::None);
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);
    let asked = server.standin.state().requests.iter().filter(|r| r.operation == "UpdateItem").count();
    send_receive(&mut app, json!({"account": account}));
    assert_eq!(server.standin.state().requests.iter().filter(|r| r.operation == "UpdateItem").count(), asked);
}
