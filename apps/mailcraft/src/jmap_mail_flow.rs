// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A JMAP account (M6) read by the window the whole way: Send/Receive through a host that runs
//! `mailcraft_jmap` on a thread of its own, against the crate's JMAP server in the process
//! (`stand-in`), each piece taken in as it comes; a message opened, and one replied to, fetched
//! whole; the token the owner types, or the host's own. Nothing here leaves the process; every
//! name, address and id is invented.

use std::sync::mpsc;
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::sync_progress::RunState;
use mailcraft_jmap::standin::{SESSION, StandIn, TOKEN};
use mailcraft_jmap::{Connection, Credentials, block_on, failed_run, fetch, sync_mail};
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_ui_egui::jmap_mail::{JmapFetchJob, JmapJob};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::{Value, json};

const ME: &str = "ada@example.org";
/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// An email in `mailbox` of the server, as a mail program writes one, arriving at the `n`th minute
/// of the morning: its id there.
fn deliver(server: &StandIn, mailbox: &str, subject: &str, n: usize) -> String {
    let raw = format!(
        "From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: {subject}\r\nMessage-ID: <{n}@example.org>\r\nDate: Wed, 14 Oct 2026 09:30:00 +0200\r\n\r\nHello about {subject}.\r\n"
    );
    let at = format!("2026-10-14T{:02}:{:02}:00Z", 6 + n / 60, n % 60);
    server.with(|d| d.deliver(mailbox, raw.as_bytes(), &[], &at))
}

/// Signs in with the job's secret, or, when it has none, with the host's own token.
fn credentials(secret: &str) -> Credentials {
    Credentials::bearer(if secret.is_empty() { TOKEN } else { secret }).unwrap()
}

/// A window with one JMAP account on `server`, and a host that runs each job on a thread of its
/// own (as the browser runs it as a task), handing every piece to the window as it comes.
fn window(server: &StandIn) -> (MailApp, Id) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let there = server.clone();
    app.services.jmap = Some(Box::new(move |job: JmapJob| {
        let (tx, rx) = mpsc::channel();
        let server = there.clone();
        std::thread::spawn(move || {
            block_on(async {
                match Connection::connect(server, &job.request.session, credentials(&job.secret)).await {
                    Ok(connection) => {
                        sync_mail(&connection, &job.request, &job.cancel, &mut |piece| {
                            let _ = tx.send(piece);
                        })
                        .await
                    }
                    Err(error) => {
                        let _ = tx.send(failed_run(&job.request, &error));
                    }
                }
            });
        });
        rx
    }));
    let there = server.clone();
    app.services.jmap_fetch = Some(Box::new(move |job: JmapFetchJob| {
        let (tx, rx) = mpsc::channel();
        let server = there.clone();
        std::thread::spawn(move || {
            let fetched = block_on(async {
                match Connection::connect(server, &job.request.session, credentials(&job.secret)).await {
                    Ok(connection) => fetch::fetch_messages(&connection, &job.request, &job.cancel).await,
                    Err(error) => fetch::failed_fetch(&job.request, &error),
                }
            });
            let _ = tx.send(fetched);
        });
        rx
    }));
    let added = app.execute("account.add", json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION, "bearer": true}})).unwrap();
    (app, added["id"].as_u64().unwrap())
}

/// Send/Receive as the owner asks for it, to its end; the background download is then held back
/// (as after Stop), so that what is opened next isn't here whole yet. What `ui.sync` answered.
fn send_receive(app: &mut MailApp, params: Value) -> Value {
    let started = app.execute("ui.sync", params).unwrap();
    for _ in 0..4_000 {
        app.poll_sync();
        if !app.syncing() {
            app.session.sync_progress.full_paused = true;
            return started;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the run didn't end");
}

/// Frames of the window, a few milliseconds apart, until `done`.
fn until(app: &mut MailApp, done: &dyn Fn(&MailApp) -> bool) {
    for _ in 0..4_000 {
        app.poll_sync();
        app.drive_fetch();
        if done(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("it didn't happen");
}

/// The account's message with this subject: its id, its folder, and whether it is still to be
/// fetched whole.
fn here(app: &MailApp, account: Id, subject: &str) -> Option<(Id, Id, bool)> {
    app.session.mailbox.messages.values().find(|m| m.account == account && m.subject == subject).map(|m| (m.id, m.folder, m.unfetched))
}

#[test]
fn the_window_reads_a_jmap_account_and_fetches_what_is_opened_or_answered_whole() {
    let server = StandIn::new();
    let (inbox, archive) = server.with(|d| (d.mailbox("Inbox", None, Some("inbox")), d.mailbox("Archive", None, Some("archive"))));
    for (n, subject) in ["Kickoff", "Budget", "Agenda"].into_iter().enumerate() {
        deliver(&server, &inbox, subject, n);
    }
    deliver(&server, &archive, "Old minutes", 9);
    let (mut app, account) = window(&server);

    // The first Send/Receive asks for the token, and goes once the owner has typed it.
    assert_eq!(send_receive(&mut app, json!({}))["pending"], "password");
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("password"));
    app.dialog = None;
    assert_eq!(send_receive(&mut app, json!({"account": account, "password": TOKEN}))["jmap"], true);
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);
    // The server's folders, with what a list shows of each message, none of them whole yet.
    let inbox_here = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    for subject in ["Kickoff", "Budget", "Agenda"] {
        assert_eq!(here(&app, account, subject).map(|(_, folder, unfetched)| (folder, unfetched)), Some((inbox_here, true)), "{subject}");
    }
    let archive_here = app.session.mailbox.folder_by_server(account, &archive).unwrap();
    assert_eq!(here(&app, account, "Old minutes").map(|(_, folder, _)| folder), Some(archive_here));
    // Send/Receive Progress: the account's row, done, with what came.
    let row = app.session.sync_progress.accounts.iter().find(|r| r.account == account).unwrap();
    assert_eq!((row.state, row.new), (RunState::Done, 4));

    // Opened: fetched whole at once.
    let (kickoff, ..) = here(&app, account, "Kickoff").unwrap();
    app.session.sel.messages = vec![kickoff];
    until(&mut app, &|app| !app.session.mailbox.message(kickoff).unwrap().unfetched);
    assert!(app.session.mailbox.message(kickoff).unwrap().body.text.contains("Hello about Kickoff."));

    // A reply to one that isn't here whole waits for it, and then opens; the token typed before
    // still goes, nothing is asked again.
    deliver(&server, &inbox, "Late", 30);
    assert_eq!(send_receive(&mut app, json!({"account": account}))["started"], true);
    let (late, _, unfetched) = here(&app, account, "Late").unwrap();
    assert!(unfetched);
    assert_eq!(app.run("mail.reply", json!({"id": late})).unwrap()["pending"], "download");
    until(&mut app, &|app| app.session.compose.is_some());
    assert!(app.session.compose.as_ref().unwrap().subject.contains("Late"));
    assert!(!app.session.mailbox.message(late).unwrap().unfetched);

    // In the background, the rest of the Inbox.
    app.session.sync_progress.full_paused = false;
    until(&mut app, &|app| app.session.mailbox.messages.values().filter(|m| m.folder == inbox_here).all(|m| !m.unfetched));
    assert!(app.session.mailbox.message(here(&app, account, "Agenda").unwrap().0).unwrap().body.text.contains("Hello about Agenda."));
}

#[test]
fn a_host_that_holds_the_credential_is_never_asked_for_one() {
    let server = StandIn::new();
    let inbox = server.with(|d| d.mailbox("Inbox", None, Some("inbox")));
    deliver(&server, &inbox, "Kickoff", 1);
    let (mut app, account) = window(&server);
    app.services.jmap_host_credentials = true;
    assert_eq!(send_receive(&mut app, json!({}))["started"], true);
    assert!(app.dialog.is_none());
    let (kickoff, _, unfetched) = here(&app, account, "Kickoff").unwrap();
    assert!(unfetched);
    app.session.sel.messages = vec![kickoff];
    until(&mut app, &|app| !app.session.mailbox.message(kickoff).unwrap().unfetched);
    // Turned down: said, and still nothing is asked of the owner.
    server.with(|d| d.token = None);
    send_receive(&mut app, json!({}));
    assert!(app.session.sync_status.starts_with("The server turned down the sign-in for"), "{}", app.session.sync_status);
    assert!(app.dialog.is_none());
}

#[test]
fn a_token_the_server_turns_down_is_forgotten_and_asked_for_again() {
    let server = StandIn::new();
    let inbox = server.with(|d| d.mailbox("Inbox", None, Some("inbox")));
    deliver(&server, &inbox, "Kickoff", 1);
    let (mut app, account) = window(&server);
    send_receive(&mut app, json!({"account": account, "password": "not-the-token"}));
    assert!(app.session.sync_status.starts_with("The server turned down the sign-in for"), "{}", app.session.sync_status);
    assert!(here(&app, account, "Kickoff").is_none());
    // The next Send/Receive asks again, rather than send the same one once more.
    let asked = server.seen().len();
    assert_eq!(send_receive(&mut app, json!({"account": account}))["pending"], "password");
    assert_eq!(server.seen().len(), asked);
    app.dialog = None;
    send_receive(&mut app, json!({"account": account, "password": TOKEN}));
    assert!(here(&app, account, "Kickoff").is_some());
}
