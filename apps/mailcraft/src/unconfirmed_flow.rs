// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A crafted mailbox file, the whole way (C1 PR-c): it carries the owner's address, accounts at
//! another server, and a message waiting in its Outbox. Opened, nothing of it reaches the IMAP
//! or SMTP stand-ins, nor Microsoft Graph, and no password is sent; once the owner confirms its
//! servers, Send/Receive goes. The stand-ins are listeners on this machine that count what
//! arrives; nothing leaves it and every name is invented.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::{FolderKind, Message};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::json;

const NOW: i64 = 1_791_993_600;

/// A server that says what a mail server says at the start and answers every command, refusing
/// the sign-in; it keeps every line it was sent and counts the connections.
struct Listener {
    port: u16,
    connections: Arc<AtomicUsize>,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Listener {
    fn start(greeting: &'static str) -> Listener {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let (connections, lines) = (Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(Vec::new())));
        let (count, kept) = (connections.clone(), lines.clone());
        std::thread::spawn(move || {
            for stream in socket.incoming().flatten() {
                count.fetch_add(1, Ordering::Relaxed);
                let kept = kept.clone();
                std::thread::spawn(move || {
                    let Ok(mut out) = stream.try_clone() else { return };
                    let _ = out.write_all(greeting.as_bytes());
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        kept.lock().unwrap().push(line.clone());
                        let (tag, command) = line.split_once(' ').unwrap_or((&line, ""));
                        let reply = if command.to_ascii_uppercase().starts_with("CAPABILITY") {
                            format!("* CAPABILITY IMAP4rev1\r\n{tag} OK done\r\n")
                        } else if command.to_ascii_uppercase().starts_with("LOGIN") {
                            format!("{tag} NO refused\r\n")
                        } else {
                            format!("{tag} OK done\r\n")
                        };
                        if out.write_all(reply.as_bytes()).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Listener { port, connections, lines }
    }
}

fn finish(app: &mut MailApp) {
    for _ in 0..2_000 {
        app.poll_sync();
        if !app.syncing() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the Send/Receive didn't end");
}

#[test]
fn a_crafted_mailbox_reaches_no_server_until_the_owner_confirms_its_servers() {
    let (imap, smtp) = (Listener::start("* OK ready\r\n"), Listener::start("220 ready\r\n"));
    // The file: the owner's address, the stand-ins as its servers, and mail waiting to be sent.
    let dir = std::env::temp_dir().join(format!("mailcraft-c1c-flow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("crafted.mailcraft");
    let mut crafted = Session::empty();
    let account = crafted
        .run(
            "account.add",
            &json!({
                "email": "owner@example.org",
                "incoming": {"host": "127.0.0.1", "port": imap.port, "security": "none"},
                "outgoing": {"host": "127.0.0.1", "port": smtp.port, "security": "none"},
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let outbox = crafted.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    crafted.mailbox.add_message(Message { folder: outbox, account, subject: "Invoice".into(), ..Default::default() }).unwrap();
    crafted.run("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();

    // The window: Send/Receive, Graph and sign-in as the desktop app has them, Graph counted.
    let graph_calls = Arc::new(AtomicUsize::new(0));
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.services.sync = Some(Box::new(move |request| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let cancel = AtomicBool::new(false);
            mailcraft_net::sync_with(&request, &cancel, &mut |piece| {
                let _ = tx.send(piece);
            });
        });
        rx
    }));
    let counted = graph_calls.clone();
    app.services.graph = Some(Box::new(move |_| {
        counted.fetch_add(1, Ordering::Relaxed);
        std::sync::mpsc::channel().1
    }));

    app.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(app.is_unconfirmed());
    let id = app.session.mailbox.accounts.iter().find(|a| a.email == "owner@example.org").map(|a| a.id).unwrap();
    // Nobody confirmed: not quietly, not when asked for, not with the password at hand.
    assert!(app.execute("ui.sync", json!({"account": id, "quiet": true})).is_err());
    assert_eq!(app.execute("ui.sync", json!({"account": id, "password": "invented-pw"})).unwrap()["pending"], "confirmServers");
    assert!(app.execute("ui.connectMicrosoft", json!({"account": id})).is_err());
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!((imap.connections.load(Ordering::Relaxed), smtp.connections.load(Ordering::Relaxed)), (0, 0));
    assert_eq!(graph_calls.load(Ordering::Relaxed), 0);
    assert!(imap.lines.lock().unwrap().is_empty() && smtp.lines.lock().unwrap().is_empty());
    let waiting = app.session.mailbox.messages.values().filter(|m| m.subject == "Invoice").count();
    assert_eq!(waiting, 1, "the Outbox stays");

    // The owner has seen the servers and says yes: Send/Receive goes, to the servers the file names.
    app.execute("ui.confirmServers", json!({})).unwrap();
    assert!(!app.is_unconfirmed());
    app.execute("ui.sync", json!({"account": id, "password": "invented-pw"})).unwrap();
    finish(&mut app);
    assert!(imap.connections.load(Ordering::Relaxed) >= 1);
    assert!(imap.lines.lock().unwrap().iter().any(|l| l.contains("LOGIN") && l.contains("invented-pw")), "{:?}", imap.lines.lock().unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}
