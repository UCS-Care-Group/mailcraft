// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the host says went wrong with a mail server is shown in the interface's language where
//! the words are MailCraft's, and as the server said it where they are the server's (#872). The
//! errors here are the host's own: a real check and a real Send/Receive (`mailcraft-net`)
//! against a small IMAP server on this machine.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_i18n::{Lang, Text};
use mailcraft_model::sync::{SyncRequest, SyncResult};
use mailcraft_model::{Account, Security, Server};
use serde_json::json;

use crate::i18n::{RestoreLanguage, set_current};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;
/// What a server that takes no password on a connection is told by the host, in English.
const NO_PASSWORD_LINE: &str = "Couldn't check for new mail: sign-in failed: the server doesn't take a password on this connection";
/// The same in German, as the catalogue has it.
const NO_PASSWORD_DE: &str =
    "Neue E-Mails konnten nicht abgerufen werden: Anmeldung fehlgeschlagen: Der Server nimmt über diese Verbindung kein Passwort an";

type Pipe = Arc<Mutex<Option<mpsc::Sender<SyncResult>>>>;

/// An IMAP server on this machine that says it can do `capabilities` and refuses every sign-in,
/// saying `refusal`. Its port.
fn imap_server(capabilities: &'static str, refusal: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let Ok(stream) = stream else { return };
            std::thread::spawn(move || serve(stream, capabilities, refusal));
        }
    });
    port
}

fn serve(stream: TcpStream, capabilities: &str, refusal: &str) {
    let mut out = stream.try_clone().unwrap();
    if out.write_all(format!("* OK [CAPABILITY {capabilities}] ready\r\n").as_bytes()).is_err() {
        return;
    }
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let (tag, rest) = line.split_once(' ').unwrap_or((&line, ""));
        let verb = rest.split(' ').next().unwrap_or_default().to_ascii_uppercase();
        let reply = match verb.as_str() {
            "CAPABILITY" => format!("* CAPABILITY {capabilities}\r\n{tag} OK done\r\n"),
            "LOGOUT" => format!("* BYE bye\r\n{tag} OK done\r\n"),
            "LOGIN" => format!("{tag} NO {refusal}\r\n"),
            _ => format!("{tag} BAD what\r\n"),
        };
        if out.write_all(reply.as_bytes()).is_err() || verb == "LOGOUT" {
            return;
        }
    }
}

/// An account whose incoming server is the one on this port.
fn account_at(port: u16) -> Account {
    Account {
        email: "noor@lumen.example".into(),
        username: "noor".into(),
        incoming: Some(Server { host: "127.0.0.1".into(), port, security: Security::None }),
        ..Default::default()
    }
}

/// What the host's check of an account says of a server that announces `LOGINDISABLED`.
pub(crate) fn no_password_here() -> Text {
    let port = imap_server("IMAP4rev1 LOGINDISABLED", "never asked");
    mailcraft_net::test_account(&account_at(port), "invented-password").unwrap_err().said()
}

/// What the host's check of an account says of a server that refuses the sign-in with `reason`.
pub(crate) fn refused_with(reason: &'static str) -> Text {
    let port = imap_server("IMAP4rev1", reason);
    mailcraft_net::test_account(&account_at(port), "invented-password").unwrap_err().said()
}

/// What a Send/Receive of the account on this port comes back with.
fn synced(port: u16) -> SyncResult {
    mailcraft_net::sync(&SyncRequest { account: account_at(port), password: "invented-password".into(), ..Default::default() })
}

/// A window with an IMAP account, and a Send/Receive of it under way: what it is handed comes
/// down the pipe.
fn imap_run() -> (MailApp, u64, Pipe) {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let pipe = Pipe::default();
    let feed = pipe.clone();
    a.services.sync = Some(Box::new(move |_: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        *feed.lock().unwrap() = Some(tx);
        rx
    }));
    let added = a.execute(
        "account.add",
        json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example", "port": 993}, "outgoing": {"host": "smtp.mail.example", "port": 465}}),
    );
    let id = added.unwrap()["id"].as_u64().unwrap();
    assert_eq!(a.execute("ui.sync", json!({"account": id, "password": "invented-password"})).unwrap()["started"], true);
    (a, id, pipe)
}

/// The run's answer, handed to the window as its last piece.
fn finish(a: &mut MailApp, id: u64, pipe: &Pipe, result: SyncResult) {
    let last = SyncResult { account: id, email: String::new(), partial: false, ..result };
    pipe.lock().unwrap().as_ref().unwrap().send(last).unwrap();
    a.poll_sync();
}

#[test]
fn send_receive_problems_are_shown_in_german_for_a_german_interface() {
    let _language = RestoreLanguage::hold();
    let result = synced(imap_server("IMAP4rev1 LOGINDISABLED", "never asked"));
    // The English is as it was: what the CLI, MCP and the log read.
    assert_eq!(result.errors, [NO_PASSWORD_LINE]);
    set_current(Lang::German);
    let (mut a, id, pipe) = imap_run();
    finish(&mut a, id, &pipe, result);
    let lines = a.sync_problem_lines();
    assert!(lines.iter().any(|l| l.trim() == NO_PASSWORD_DE), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("doesn't take a password")), "{lines:?}");
    // The status bar says it the same way.
    let status = a.status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default();
    assert_eq!(status, NO_PASSWORD_DE);
    // And in English, as before, once the interface is English again.
    set_current(Lang::English);
    assert!(a.sync_problem_lines().iter().any(|l| l.trim() == NO_PASSWORD_LINE), "{:?}", a.sync_problem_lines());
}

#[test]
fn a_server_s_own_reason_is_kept_as_it_said_it() {
    let _language = RestoreLanguage::hold();
    let result = synced(imap_server("IMAP4rev1", "Basic authentication is disabled."));
    assert_eq!(result.errors, ["Couldn't check for new mail: sign-in failed: Basic authentication is disabled."]);
    set_current(Lang::German);
    let (mut a, id, pipe) = imap_run();
    finish(&mut a, id, &pipe, result);
    let lines = a.sync_problem_lines();
    let german = "Neue E-Mails konnten nicht abgerufen werden: Anmeldung fehlgeschlagen: Basic authentication is disabled.";
    assert!(lines.iter().any(|l| l.trim() == german), "{lines:?}");
}

#[test]
fn a_line_the_host_said_only_in_english_is_shown_as_it_is() {
    let _language = RestoreLanguage::hold();
    set_current(Lang::German);
    let (mut a, id, pipe) = imap_run();
    let line = "Couldn't read \"Archive\": the server said no";
    finish(&mut a, id, &pipe, SyncResult { errors: vec![line.into()], ..Default::default() });
    assert!(a.sync_problem_lines().iter().any(|l| l.trim() == line), "{:?}", a.sync_problem_lines());
}

#[test]
fn the_check_of_an_account_says_why_in_german() {
    let refusal = refused_with("Basic authentication is disabled.");
    assert_eq!(refusal.to_string(), "sign-in failed: Basic authentication is disabled.");
    assert_eq!(refusal.in_lang(Lang::German), "Anmeldung fehlgeschlagen: Basic authentication is disabled.");
    let here = no_password_here();
    assert_eq!(here.to_string(), "sign-in failed: the server doesn't take a password on this connection");
    assert_eq!(here.in_lang(Lang::German), "Anmeldung fehlgeschlagen: Der Server nimmt über diese Verbindung kein Passwort an");
}

#[test]
fn a_long_line_is_kept_cut_short() {
    let (mut a, id, pipe) = imap_run();
    let mut result = SyncResult::default();
    result.problem(mailcraft_i18n::said!("Couldn't open \"{name}\"", name = "x".repeat(70_000)));
    finish(&mut a, id, &pipe, result);
    let lines = a.account_problem_lines(id);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].chars().count(), 600);
    // What is kept is the name cut short, not the whole line.
    let kept = &a.sync_problems[0].3[0];
    assert!(!kept.is_verbatim());
    assert!(kept.to_string().chars().count() < 700, "{}", kept.to_string().len());
}

#[test]
fn a_long_line_is_still_said_in_the_interface_s_language() {
    let _language = RestoreLanguage::hold();
    set_current(Lang::German);
    let (mut a, id, pipe) = imap_run();
    let mut result = SyncResult::default();
    result.problem(mailcraft_i18n::said!("Couldn't open \"{name}\"", name = "x".repeat(70_000)));
    finish(&mut a, id, &pipe, result);
    let lines = a.account_problem_lines(id);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].chars().count(), 600);
    // German puts the name first: „xxx… konnte nicht geöffnet werden.
    assert!(lines[0].starts_with("„x"), "{}", lines[0].chars().take(40).collect::<String>());
    let english = |l: &str| l.starts_with("Couldn't open");
    assert!(!english(&lines[0]));
    set_current(Lang::English);
    assert!(english(&a.account_problem_lines(id)[0]));
}
