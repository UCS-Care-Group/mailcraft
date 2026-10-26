// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A forward of several messages in the window (#360): those of which only the headers are here
//! are fetched first, an account's at a time, and the new message opens once they are all here,
//! with the messages chosen however the selection moved meanwhile; one that doesn't come stops it,
//! named, and isn't asked for again; what is over a limit is refused before anything is fetched.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::{GraphFetchResult, GraphFetched, ServerMessage, Tokens};
use mailcraft_model::{Body, FolderKind, Message};

use super::*;
use crate::Services;

type Jobs = Arc<Mutex<Vec<(GraphFetchJob, mpsc::Sender<GraphFetchResult>)>>>;

const NOW: i64 = 1_791_993_600;

/// A window with two Microsoft accounts and a host that keeps every job for the test to answer.
/// Each account has three messages, newest first: the first and the last of them only headers.
/// The Inbox of the first account is in view, and nothing is fetched in the background.
fn window() -> (MailApp, [[Id; 3]; 2], Jobs) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::default(), NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let mut ids = [[0; 3]; 2];
    for (k, address) in ["one@example.org", "two@example.org"].iter().enumerate() {
        let account = app.execute("account.add", json!({"email": address, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
        for (n, id) in ids[k].iter_mut().enumerate() {
            let server = ServerMessage { id: format!("AA{k}M{n}"), folder: "inbox".into(), size: 2_000, ..Default::default() };
            let unfetched = n != 1;
            let when = NOW - 3_600 * (n as i64 + 1);
            let message = Message {
                account,
                folder: inbox,
                subject: format!("Account {k} message {n}"),
                date: when,
                received: when,
                unfetched,
                body: Body { text: if unfetched { "A preview".into() } else { format!("All of message {n}.") }, html: None },
                server: Some(server),
                ..Default::default()
            };
            *id = app.session.mailbox.add_message(message).unwrap();
        }
        let tokens = Tokens { access: "stand-in".into(), refresh: "r".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
        app.seed_graph_token(account, tokens);
        if k == 0 {
            app.session.view.folder = inbox;
        }
    }
    // Nothing in the background: only what the owner waits for is asked for.
    app.session.sync_progress.full_paused = true;
    let jobs: Jobs = Default::default();
    let kept = jobs.clone();
    app.services.graph_fetch = Some(Box::new(move |job: GraphFetchJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    (app, ids, jobs)
}

/// The jobs handed over since the last look: whose, and which of its messages.
fn asked(jobs: &Jobs) -> Vec<(Id, Vec<String>)> {
    jobs.lock().unwrap().iter().map(|(job, _)| (job.request.account, job.request.ids.clone())).collect()
}

/// Answer every job: each message whole, but those in `missing` (gone from the server) and in
/// `failed` (refused), then let the window take it in.
fn answer(app: &mut MailApp, jobs: &Jobs, missing: &[&str], failed: &[&str]) {
    for (job, tx) in std::mem::take(&mut *jobs.lock().unwrap()) {
        let mut result = GraphFetchResult { account: job.request.account, email: job.request.email.clone(), ..Default::default() };
        for id in &job.request.ids {
            if missing.contains(&id.as_str()) {
                result.missing.push(id.clone());
            } else if failed.contains(&id.as_str()) {
                result.failed.push(id.clone());
            } else {
                let mime = format!("From: someone@example.org\r\nSubject: Whole {id}\r\nMessage-ID: <{id}@example.org>\r\n\r\nAll of {id}.\r\n");
                result.fetched.push(GraphFetched { id: id.clone(), mime: mime.into_bytes() });
            }
        }
        tx.send(result).unwrap();
    }
    app.drive_fetch();
}

/// The subjects of the messages attached, by their names.
fn subjects(app: &MailApp) -> Vec<String> {
    let attachments = &app.session.compose.as_ref().unwrap().attachments;
    assert!(attachments.iter().all(|a| a.mime == "message/rfc822"));
    attachments.iter().map(|a| a.name.strip_suffix(".eml").unwrap().to_string()).collect()
}

#[test]
fn several_are_fetched_first_and_open_as_they_were_chosen() {
    for command in ["mail.forwardAsAttachment", "mail.forward"] {
        let (mut app, [[a, b, c], _], jobs) = window();
        // Chosen by clicks, the focus on the middle one; the other two only headers.
        app.session.sel.messages = vec![c, a, b];
        let r = app.run(command, json!({})).unwrap();
        assert_eq!(r["pending"], "download", "{command}");
        assert!(app.session.compose.is_none(), "{command}: not before they are here");
        // Both in one job of their account.
        let one = app.session.mailbox.message(a).unwrap().account;
        assert_eq!(asked(&jobs), [(one, vec!["AA0M2".to_string(), "AA0M0".to_string()])], "{command}");
        assert!(app.fetching.fetching(a) && app.fetching.fetching(c) && !app.fetching.fetching(b));
        // The selection moves meanwhile: what was chosen is what goes.
        app.session.sel.messages = vec![b];
        answer(&mut app, &jobs, &[], &[]);
        assert!(asked(&jobs).is_empty(), "{command}: nothing more to fetch");
        let compose = app.session.compose.as_ref().unwrap_or_else(|| panic!("{command}: {:?}", app.status_msg));
        assert_eq!((compose.source, compose.also.as_slice()), (Some(b), &[a, c][..]), "{command}");
        assert_eq!(subjects(&app), ["Whole AA0M0", "Account 0 message 1", "Whole AA0M2"], "{command}: whole, in the list's order");
    }
}

#[test]
fn several_of_two_accounts_come_an_account_at_a_time() {
    let (mut app, [[a, b, _], [x, y, z]], jobs) = window();
    let (one, two) = (app.session.mailbox.message(a).unwrap().account, app.session.mailbox.message(x).unwrap().account);
    app.session.sel.messages = vec![z, a, b, y, x];
    assert_eq!(app.run("mail.forwardAsAttachment", json!({})).unwrap()["pending"], "download");
    assert_eq!(asked(&jobs), [(two, vec!["AA1M2".to_string(), "AA1M0".to_string()])], "the first one's account first");
    answer(&mut app, &jobs, &[], &[]);
    assert!(app.session.compose.is_none(), "the other account's still to come");
    assert_eq!(asked(&jobs), [(one, vec!["AA0M0".to_string()])]);
    answer(&mut app, &jobs, &[], &[]);
    let compose = app.session.compose.as_ref().unwrap();
    // From the account of the one in focus; the list shows the first account's, then the rest.
    assert_eq!((compose.source, compose.account), (Some(x), two));
    assert_eq!(subjects(&app), ["Whole AA0M0", "Account 0 message 1", "Whole AA1M2", "Account 1 message 1", "Whole AA1M0"]);
}

#[test]
fn one_that_doesnt_come_stops_them_and_is_named_once() {
    // Gone from its server (a Send/Receive takes it away here), or refused by it.
    let says = "“Account 0 message 2” isn't downloaded yet";
    for (missing, failed) in [(&["AA0M2"][..], &[][..]), (&[], &["AA0M2"][..])] {
        let (mut app, [[a, b, c], _], jobs) = window();
        app.session.sel.messages = vec![a, b, c];
        app.by_hand = true;
        app.run("mail.forward", json!({})).unwrap();
        app.by_hand = false;
        answer(&mut app, &jobs, missing, failed);
        assert!(app.session.compose.is_none(), "{says}");
        assert!(asked(&jobs).is_empty(), "{says}: not asked for again");
        let status = app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
        assert!(status.contains(says), "{status}");
        // The one that came is here whole, for the next try.
        assert!(!app.session.mailbox.message(a).unwrap().unfetched);
    }
}

#[test]
fn over_a_limit_is_refused_before_anything_is_fetched() {
    let (mut app, [[a, b, c], _], jobs) = window();
    // Larger than 64 MB together, as their server said.
    for id in [a, c] {
        let m = app.session.mailbox.message_mut(id).unwrap();
        m.server.as_mut().unwrap().size = 40 << 20;
    }
    app.session.sel.messages = vec![a, b, c];
    app.by_hand = true;
    let e = app.run("mail.forwardAsAttachment", json!({})).unwrap_err();
    assert!(e.contains("larger than 64 MB together"), "{e}");
    assert!(app.refused.as_deref().is_some_and(|said| said.contains("64 MB")), "said in a box too");
    // One of them whose server's copy is gone.
    app.session.mailbox.message_mut(c).unwrap().server.as_mut().unwrap().size = 2_000;
    app.session.mailbox.message_mut(a).unwrap().incomplete = true;
    let e = app.run("mail.forward", json!({})).unwrap_err();
    assert!(e.contains("only the headers of “Account 0 message 0”"), "{e}");
    // More than one message carries.
    app.session.mailbox.message_mut(a).unwrap().incomplete = false;
    let e = app.run("mail.forward", json!({"ids": (1..=201).collect::<Vec<u64>>()})).unwrap_err();
    assert!(e.contains("at most 200 messages"), "{e}");
    assert!(asked(&jobs).is_empty() && app.session.compose.is_none());
}

#[test]
fn offline_the_window_says_so() {
    let (mut app, [[a, b, c], _], jobs) = window();
    app.graph_tokens.clear();
    app.session.sel.messages = vec![a, b, c];
    let e = app.run("mail.forwardAsAttachment", json!({})).unwrap_err();
    assert!(e.contains("It will download when you're online"), "{e}");
    let status = app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert!(status.contains("It will download when you're online"), "{status}");
    // As for one message (a reply, here), which said nothing before.
    app.status_msg = None;
    let e = app.run("mail.reply", json!({"id": a})).unwrap_err();
    assert!(app.status_msg.as_ref().is_some_and(|(text, _)| *text == e), "{e}");
    assert!(asked(&jobs).is_empty());
}
