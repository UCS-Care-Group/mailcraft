// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A forward of several messages in the window (#360): those of which only the headers are here
//! are fetched first, an account's at a time and ten at a time, and the new message opens once
//! they are all here, with the messages chosen however the selection moved meanwhile. One its
//! server didn't send stops it, named, with the reason; a batch cut short is asked for again, and
//! one that brought nothing stops it. What is over a limit is refused before anything is fetched,
//! or, for sizes the server didn't say, as soon as a batch shows it.

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
    reply(app, jobs, &|id| {
        if missing.contains(&id) {
            Reply::Missing
        } else if failed.contains(&id) {
            Reply::Failed
        } else {
            Reply::Whole(String::new())
        }
    });
}

/// What a test's server says of a message it is asked for.
enum Reply {
    /// Whole, with this much more text.
    Whole(String),
    Missing,
    Failed,
    /// Nothing: it stopped before it.
    None,
}

/// Answer every job as `says` says of each message, then let the window take it in.
fn reply(app: &mut MailApp, jobs: &Jobs, says: &dyn Fn(&str) -> Reply) {
    for (job, tx) in std::mem::take(&mut *jobs.lock().unwrap()) {
        let mut result = GraphFetchResult { account: job.request.account, email: job.request.email.clone(), ..Default::default() };
        for id in &job.request.ids {
            match says(id) {
                Reply::Whole(more) => {
                    let mime =
                        format!("From: someone@example.org\r\nSubject: Whole {id}\r\nMessage-ID: <{id}@example.org>\r\n\r\nAll of {id}.\r\n{more}");
                    result.fetched.push(GraphFetched { id: id.clone(), mime: mime.into_bytes() });
                }
                Reply::Missing => result.missing.push(id.clone()),
                Reply::Failed => result.failed.push(id.clone()),
                Reply::None => {}
            }
        }
        if result.fetched.len() + result.missing.len() + result.failed.len() < job.request.ids.len() {
            result.busy_for = Some(30);
        }
        // (A job the window let go of has nobody to answer.)
        let _ = tx.send(result);
    }
    app.drive_fetch();
}

/// Of the jobs handed over since the last answer, which were called off.
fn called_off(jobs: &Jobs) -> Vec<bool> {
    jobs.lock().unwrap().iter().map(|(job, _)| job.cancel.load(Ordering::Relaxed)).collect()
}

/// What the status bar says.
fn said(app: &MailApp) -> String {
    app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
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
        assert_eq!(app.status_msg.as_ref().map(|(text, _)| text.as_str()), Some("Downloading the messages first…"), "{command}");
        // Both in one job of their account, in the order they go.
        let one = app.session.mailbox.message(a).unwrap().account;
        assert_eq!(asked(&jobs), [(one, vec!["AA0M0".to_string(), "AA0M2".to_string()])], "{command}");
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
    // In the order they go: the list's first, then the other account's, as they were chosen.
    assert_eq!(asked(&jobs), [(one, vec!["AA0M0".to_string()])], "the first one's account first");
    answer(&mut app, &jobs, &[], &[]);
    assert!(app.session.compose.is_none(), "the other account's still to come");
    assert_eq!(asked(&jobs), [(two, vec!["AA1M2".to_string(), "AA1M0".to_string()])]);
    answer(&mut app, &jobs, &[], &[]);
    let compose = app.session.compose.as_ref().unwrap();
    // From the account of the one in focus; the list shows the first account's, then the rest.
    assert_eq!((compose.source, compose.account), (Some(x), two));
    assert_eq!(subjects(&app), ["Whole AA0M0", "Account 0 message 1", "Whole AA1M2", "Account 1 message 1", "Whole AA1M0"]);
}

#[test]
fn one_its_server_didnt_send_stops_them_and_is_named_with_the_reason() {
    for (missing, failed, says) in [
        // Gone from its server (deleted or moved meanwhile).
        (&["AA0M2"][..], &[][..], "“Account 0 message 2” wasn't found on its server, so it can't be forwarded"),
        // Refused by it.
        (&[], &["AA0M2"][..], "the server wouldn't send “Account 0 message 2” whole, so it can't be forwarded"),
        // Two of them: the first in the list is named, whatever the reason of each.
        (&["AA0M2", "AA0M0"][..], &[][..], "“Account 0 message 0” wasn't found on its server"),
        (&["AA0M2"][..], &["AA0M0"][..], "the server wouldn't send “Account 0 message 0” whole"),
    ] {
        let (mut app, [[a, b, c], _], jobs) = window();
        app.session.sel.messages = vec![a, b, c];
        app.by_hand = true;
        app.run("mail.forward", json!({})).unwrap();
        app.by_hand = false;
        answer(&mut app, &jobs, missing, failed);
        assert!(app.session.compose.is_none(), "{says}");
        assert!(asked(&jobs).is_empty(), "{says}: not asked for again");
        assert!(said(&app).contains(says), "{}", said(&app));
        // The one that came is here whole, for the next try.
        assert_eq!(app.session.mailbox.message(a).unwrap().unfetched, missing.contains(&"AA0M0") || failed.contains(&"AA0M0"));
    }
}

#[test]
fn a_batch_cut_short_is_asked_for_again_and_one_that_brings_none_stops() {
    let refused = "Couldn't download the messages from Microsoft: the server refused the sign-in";
    for (offline, why, says) in [
        (false, None, "The server didn't send the messages. Try again in a few minutes."),
        (true, None, "“Account 0 message 2” isn't downloaded yet: open it once you're online"),
        // What the server said, when it said why.
        (false, Some(refused), refused),
    ] {
        let (mut app, [[a, b, c], _], jobs) = window();
        let one = app.session.mailbox.message(a).unwrap().account;
        app.session.sel.messages = vec![a, b, c];
        app.run("mail.forwardAsAttachment", json!({})).unwrap();
        // The server busy after the first: the other is asked for again.
        reply(&mut app, &jobs, &|id| if id == "AA0M0" { Reply::Whole(String::new()) } else { Reply::None });
        assert_eq!(asked(&jobs), [(one, vec!["AA0M2".to_string()])], "offline {offline}");
        assert!(app.session.compose.is_none());
        // A batch that brings none of them stops it, saying why, and asks for nothing more.
        for (job, tx) in std::mem::take(&mut *jobs.lock().unwrap()) {
            let errors = why.iter().map(|why| why.to_string()).collect();
            let result = GraphFetchResult { account: job.request.account, offline, errors, busy_for: (!offline).then_some(30), ..Default::default() };
            tx.send(result).unwrap();
        }
        app.drive_fetch();
        assert!(asked(&jobs).is_empty() && app.session.compose.is_none(), "offline {offline}");
        assert!(said(&app).contains(says), "{}", said(&app));
    }
}

#[test]
fn one_named_by_id_or_ids_is_fetched_first() {
    let (mut app, [[a, b, c], [x, ..]], jobs) = window();
    let (one, two) = (app.session.mailbox.message(a).unwrap().account, app.session.mailbox.message(x).unwrap().account);
    app.session.sel.messages = vec![b];
    for (command, params, message, asked_for) in [
        ("mail.forward", json!({"ids": [a]}), a, (one, "AA0M0")),
        ("mail.forwardAsAttachment", json!({"ids": [c]}), c, (one, "AA0M2")),
        ("mail.forwardAsAttachment", json!({"id": x}), x, (two, "AA1M0")),
    ] {
        app.session.compose = None;
        assert_eq!(app.run(command, params.clone()).unwrap()["pending"], "download", "{command} {params}");
        assert_eq!(app.status_msg.as_ref().map(|(text, _)| text.as_str()), Some("Downloading the message first…"));
        assert_eq!(asked(&jobs), [(asked_for.0, vec![asked_for.1.to_string()])], "{command} {params}");
        answer(&mut app, &jobs, &[], &[]);
        assert_eq!(app.session.compose.as_ref().and_then(|c| c.source), Some(message), "{command} {params}: {}", said(&app));
    }
    // Forward quoted it whole; Forward as Attachment attached it whole.
    assert_eq!(subjects(&app), ["Whole AA1M0"]);
    app.session.compose = None;
    app.run("mail.forward", json!({"ids": [a]})).unwrap();
    assert!(app.session.compose.as_ref().unwrap().body.plain().contains("All of AA0M0."));
}

#[test]
fn without_sizes_over_64_mb_together_is_found_out_after_a_batch() {
    let (mut app, [[a, ..], _], jobs) = window();
    let (account, inbox) = app.session.mailbox.message(a).map(|m| (m.account, m.folder)).unwrap();
    // A Microsoft account's delta says no sizes.
    let mut ids = Vec::new();
    for n in 0..12 {
        let server = ServerMessage { id: format!("BIG{n}"), folder: "inbox".into(), size: 0, ..Default::default() };
        let m = Message { account, folder: inbox, subject: format!("Big {n}"), unfetched: true, server: Some(server), ..Default::default() };
        ids.push(app.session.mailbox.add_message(m).unwrap());
    }
    app.session.sel.messages = ids;
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    assert_eq!(asked(&jobs).iter().map(|(_, ids)| ids.len()).collect::<Vec<_>>(), [10]);
    // Each is 7 MB: the ten that came are over 64 MB together, and the other two aren't asked for.
    let line = format!("{}\r\n", "x".repeat(76));
    let seven = line.repeat((7 << 20) / line.len());
    reply(&mut app, &jobs, &|_| Reply::Whole(seven.clone()));
    assert!(asked(&jobs).is_empty() && app.session.compose.is_none());
    assert!(said(&app).contains("larger than 64 MB together"), "{}", said(&app));
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
    // The engine's refusal is kept, for the window to say in the interface language.
    let e = app.fetch_first("mail.forwardAsAttachment", &json!({})).unwrap().unwrap_err();
    assert_eq!(app.last_error.take().map(|said| said.to_string()), Some(e));
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
    let copy = Message { id: 0, ..app.session.mailbox.message(c).unwrap().clone() };
    let many: Vec<Id> = (0..201).map(|_| app.session.mailbox.add_message(copy.clone()).unwrap()).collect();
    let e = app.run("mail.forward", json!({"ids": many})).unwrap_err();
    assert!(e.contains("at most 200 messages"), "{e}");
    assert!(asked(&jobs).is_empty() && app.session.compose.is_none());
}

#[test]
fn offline_the_window_says_so() {
    let (mut app, [[a, ..], [x, ..]], jobs) = window();
    let two = app.session.mailbox.message(x).unwrap().account;
    // The other account's sign-in is gone: the first's come, and the other's first is named.
    app.graph_tokens.remove(&two);
    app.session.sel.messages = vec![a, x];
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    answer(&mut app, &jobs, &[], &[]);
    assert!(said(&app).contains("“Account 1 message 0” isn't downloaded yet: open it once you're online"), "{}", said(&app));
    assert!(asked(&jobs).is_empty() && app.session.compose.is_none());
    // None of them can come: the first is named.
    let (mut app, [[a, b, c], _], jobs) = window();
    app.graph_tokens.clear();
    app.session.sel.messages = vec![a, b, c];
    let e = app.run("mail.forwardAsAttachment", json!({})).unwrap_err();
    assert!(e.contains("“Account 0 message 0” isn't downloaded yet: open it once you're online"), "{e}");
    assert_eq!(said(&app), e);
    // As for one message (a reply, here), which said nothing before.
    app.status_msg = None;
    let e = app.run("mail.reply", json!({"id": a})).unwrap_err();
    assert!(app.status_msg.as_ref().is_some_and(|(text, _)| *text == e), "{e}");
    assert!(asked(&jobs).is_empty());
}

#[test]
fn more_than_ten_come_ten_at_a_time() {
    let (mut app, [[a, ..], _], jobs) = window();
    let account = app.session.mailbox.message(a).unwrap().account;
    let inbox = app.session.mailbox.message(a).unwrap().folder;
    let mut ids = Vec::new();
    for n in 0..25 {
        let server = ServerMessage { id: format!("MANY{n}"), folder: "inbox".into(), size: 2_000, ..Default::default() };
        let m = Message { account, folder: inbox, subject: format!("Many {n}"), unfetched: true, server: Some(server), ..Default::default() };
        ids.push(app.session.mailbox.add_message(m).unwrap());
    }
    app.session.sel.messages = ids;
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    for batch in [10, 10, 5] {
        assert_eq!(asked(&jobs).iter().map(|(_, ids)| ids.len()).collect::<Vec<_>>(), [batch], "the next, once those before came");
        answer(&mut app, &jobs, &[], &[]);
    }
    assert_eq!(app.session.compose.as_ref().map(|c| c.attachments.len()), Some(25));
}

#[test]
fn one_sent_but_not_kept_isnt_asked_for_again_and_is_said_to_be() {
    // The answer is for another address than the account's now: nothing of it is taken in, and
    // that is why, whatever it said of each. Or it is for the other account: none of them is
    // found there. With one gone from the server too, the first of them in the list is named.
    for (other, missing, says) in [
        (false, &[][..], "these messages were for another account"),
        (false, &["AA0M0"], "these messages were for another account"),
        (true, &[], "“Account 0 message 0” came from its server but couldn't be kept on this computer"),
        (true, &["AA0M2"], "“Account 0 message 0” came from its server but couldn't be kept on this computer"),
        (true, &["AA0M0"], "“Account 0 message 0” wasn't found on its server"),
    ] {
        let (mut app, [[a, b, c], [x, ..]], jobs) = window();
        let two = app.session.mailbox.message(x).unwrap().account;
        app.session.sel.messages = vec![a, b, c];
        app.run("mail.forwardAsAttachment", json!({})).unwrap();
        for (job, tx) in std::mem::take(&mut *jobs.lock().unwrap()) {
            let (gone, sent): (Vec<String>, Vec<String>) = job.request.ids.iter().cloned().partition(|id| missing.contains(&id.as_str()));
            let fetched = sent.into_iter().map(|id| GraphFetched { id, mime: b"Subject: x\r\n\r\nx\r\n".to_vec() }).collect();
            let (account, email) = if other { (two, "two@example.org") } else { (job.request.account, "someone.else@example.org") };
            tx.send(GraphFetchResult { account, email: email.into(), fetched, missing: gone, ..Default::default() }).unwrap();
        }
        app.drive_fetch();
        assert!(asked(&jobs).is_empty() && app.session.compose.is_none(), "{other} {missing:?}");
        assert!(said(&app).contains(says), "{other} {missing:?}: {}", said(&app));
    }
}

#[test]
fn a_reply_waiting_on_a_forwards_batch_opens_once_its_own_message_is_here() {
    let (mut app, [[a, b, c], _], jobs) = window();
    app.session.sel.messages = vec![a, b, c];
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    // A reply to one of the batch on its way: it takes the forward's place, and waits for it.
    assert_eq!(app.run("mail.reply", json!({"id": a})).unwrap()["pending"], "download");
    assert_eq!(asked(&jobs).len(), 1, "already asked for");
    // Its own message comes; the other of the batch doesn't.
    answer(&mut app, &jobs, &["AA0M2"], &[]);
    let compose = app.session.compose.as_ref().unwrap_or_else(|| panic!("{:?}", app.status_msg));
    assert_eq!((compose.source, compose.as_attachment), (Some(a), false));
}

#[test]
fn a_forward_that_joined_anothers_batch_is_judged_by_its_own_messages() {
    let (mut app, [[a, b, c], _], jobs) = window();
    app.session.sel.messages = vec![a, b, c];
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    // Another forward, of two of them, the one still to come of which is on its way already.
    assert_eq!(app.run("mail.forwardAsAttachment", json!({"ids": [b, c]})).unwrap()["pending"], "download");
    assert_eq!(asked(&jobs).len(), 1, "already asked for");
    // The first forward's other one is gone from the server; this one's are all here.
    answer(&mut app, &jobs, &["AA0M0"], &[]);
    assert_eq!(subjects(&app), ["Account 0 message 1", "Whole AA0M2"]);
}

#[test]
fn a_forward_that_joined_anothers_batch_goes_on_as_its_own_would() {
    // Cut short before its own message came: that is asked for again.
    let (mut app, [[a, b, c], _], jobs) = window();
    let one = app.session.mailbox.message(a).unwrap().account;
    app.session.sel.messages = vec![a, b, c];
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    app.run("mail.forwardAsAttachment", json!({"ids": [b, c]})).unwrap();
    reply(&mut app, &jobs, &|id| if id == "AA0M0" { Reply::Whole(String::new()) } else { Reply::None });
    assert_eq!(asked(&jobs), [(one, vec!["AA0M2".to_string()])]);
    answer(&mut app, &jobs, &[], &[]);
    assert_eq!(subjects(&app), ["Account 0 message 1", "Whole AA0M2"]);
    // With another account's message too: that comes in a batch of its own once these are here.
    let (mut app, [[a, b, c], [x, ..]], jobs) = window();
    let two = app.session.mailbox.message(x).unwrap().account;
    app.session.sel.messages = vec![a, b, c];
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    app.run("mail.forwardAsAttachment", json!({"ids": [c, x]})).unwrap();
    assert_eq!(asked(&jobs).len(), 1, "on its way already");
    answer(&mut app, &jobs, &[], &[]);
    assert_eq!(asked(&jobs), [(two, vec!["AA1M0".to_string()])]);
    answer(&mut app, &jobs, &[], &[]);
    assert_eq!(subjects(&app), ["Whole AA0M2", "Whole AA1M0"]);
}

#[test]
fn a_forward_that_joined_anothers_batch_stops_as_its_own_would() {
    // The batch brings none of them, the server busy; what came isn't taken in; offline.
    for (case, says) in [
        ("busy", "The server didn't send the messages. Try again in a few minutes."),
        ("refused", "these messages were for another account"),
        ("offline", "“Account 0 message 2” isn't downloaded yet"),
    ] {
        let (mut app, [[a, b, c], _], jobs) = window();
        app.session.sel.messages = vec![a, b, c];
        app.run("mail.forwardAsAttachment", json!({})).unwrap();
        app.run("mail.forwardAsAttachment", json!({"ids": [b, c]})).unwrap();
        assert_eq!(asked(&jobs).len(), 1, "{case}: on its way already");
        for (job, tx) in std::mem::take(&mut *jobs.lock().unwrap()) {
            let account = job.request.account;
            let result = match case {
                "busy" => GraphFetchResult { account, email: job.request.email.clone(), busy_for: Some(30), ..Default::default() },
                "refused" => {
                    let fetched =
                        job.request.ids.iter().map(|id| GraphFetched { id: id.clone(), mime: b"Subject: x\r\n\r\nx\r\n".to_vec() }).collect();
                    GraphFetchResult { account, email: "someone.else@example.org".into(), fetched, ..Default::default() }
                }
                _ => GraphFetchResult { account, offline: true, ..Default::default() },
            };
            tx.send(result).unwrap();
        }
        app.drive_fetch();
        assert!(asked(&jobs).is_empty() && app.session.compose.is_none(), "{case}");
        assert!(said(&app).contains(says), "{case}: {}", said(&app));
    }
}

#[test]
fn those_come_by_other_means_meanwhile_go_on() {
    for offline in [false, true] {
        let (mut app, [[a, b, c], _], jobs) = window();
        let one = app.session.mailbox.message(a).unwrap().account;
        app.session.sel.messages = vec![a, b, c];
        app.run("mail.forwardAsAttachment", json!({})).unwrap();
        // The background brings them while the batch asked for is on its way, and that one
        // brings none: the server busy, or the network gone.
        let fetched =
            ["AA0M0", "AA0M2"].map(|id| GraphFetched { id: id.into(), mime: format!("Subject: Whole {id}\r\n\r\nAll of {id}.\r\n").into_bytes() });
        take_in(&mut app, GraphFetchResult { account: one, email: "one@example.org".into(), fetched: fetched.to_vec(), ..Default::default() })
            .unwrap();
        for (job, tx) in std::mem::take(&mut *jobs.lock().unwrap()) {
            let result = GraphFetchResult { account: job.request.account, offline, busy_for: (!offline).then_some(30), ..Default::default() };
            tx.send(result).unwrap();
        }
        app.drive_fetch();
        assert_eq!(subjects(&app), ["Whole AA0M0", "Account 0 message 1", "Whole AA0M2"], "offline {offline}: {}", said(&app));
    }
}

#[test]
fn a_message_opened_while_a_batch_is_on_its_way_is_fetched_after_it() {
    let (mut app, [[a, b, c], [x, ..]], jobs) = window();
    let two = app.session.mailbox.message(x).unwrap().account;
    app.session.sel.messages = vec![a, b, c];
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    // Another message of which only the headers are here, opened meanwhile in its own folder: it
    // comes after the batch the forward waits for, and the reading pane says so.
    app.session.view.folder = app.session.mailbox.message(x).unwrap().folder;
    app.session.sel.messages = vec![x];
    app.drive_fetch();
    assert_eq!(asked(&jobs).len(), 1);
    let said = |app: &MailApp| crate::reading::unfetched_said(app, app.session.mailbox.message(x).unwrap());
    assert_eq!(said(&app), "This message downloads once the messages being downloaded now are here.");
    // (Offline, it says that first.)
    app.fetching.offline = true;
    assert_eq!(said(&app), "This message isn't on this computer yet. It will download when you're online.");
    app.fetching.offline = false;
    answer(&mut app, &jobs, &[], &[]);
    assert_eq!(asked(&jobs), [(two, vec!["AA1M0".to_string()])]);
}

#[test]
fn a_message_opened_while_another_downloads_comes_after_it() {
    let (mut app, [[a, _, c], _], jobs) = window();
    let one = app.session.mailbox.message(a).unwrap().account;
    app.session.sel.messages = vec![a];
    app.drive_fetch();
    // The owner goes on to another before the first has come: one at a time.
    app.session.sel.messages = vec![c];
    app.drive_fetch();
    assert_eq!(asked(&jobs), [(one, vec!["AA0M0".to_string()])]);
    let said = crate::reading::unfetched_said(&app, app.session.mailbox.message(c).unwrap());
    assert_eq!(said, "This message downloads once the messages being downloaded now are here.");
    assert_eq!(called_off(&jobs), [false]);
    answer(&mut app, &jobs, &[], &[]);
    assert_eq!(asked(&jobs), [(one, vec!["AA0M2".to_string()])]);
}

#[test]
fn a_message_open_whose_download_a_command_took_the_place_of_is_asked_for_again() {
    let (mut app, [[a, _, c], _], jobs) = window();
    let one = app.session.mailbox.message(a).unwrap().account;
    app.session.sel.messages = vec![a];
    app.drive_fetch();
    // A reply to another message of which only the headers are here, by an agent.
    assert_eq!(app.run("mail.reply", json!({"id": c})).unwrap()["pending"], "download");
    assert_eq!(called_off(&jobs), [true, false]);
    answer(&mut app, &jobs, &[], &[]);
    assert_eq!(app.session.compose.as_ref().unwrap().source, Some(c));
    assert_eq!(asked(&jobs), [(one, vec!["AA0M0".to_string()])], "the one open, again");
}

#[test]
fn those_deleted_meanwhile_are_said_to_be_gone() {
    let (mut app, [[a, b, c], _], jobs) = window();
    app.session.sel.messages = vec![a, b, c];
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    // Deleted for good while they come, and the server stops before them.
    for id in [a, c] {
        app.session.mailbox.remove_message(id).unwrap();
    }
    reply(&mut app, &jobs, &|_| Reply::None);
    assert!(asked(&jobs).is_empty() && app.session.compose.is_none());
    assert!(said(&app).contains(&format!("no message with id {a}")), "{}", said(&app));
}

#[test]
fn a_forward_called_off_by_another_download_is_said_to_be() {
    // A reply, or a forward of another message, of which only the headers are here, or another
    // forward of these: with another in focus, of fewer of them, or letting go of what is being
    // written. Each is asked for before the forward's have come.
    // What another message is fetched for calls the forward's batch off; one of these goes on
    // with it.
    let other = |[a, b, c]: [Id; 3], x: Id| {
        [
            ("mail.reply", json!({"id": x}), x, false, vec![true, false]),
            ("mail.forwardAsAttachment", json!({"id": x}), x, true, vec![true, false]),
            ("mail.forward", json!({"ids": [a, c, b]}), b, true, vec![false]),
            ("mail.forwardAsAttachment", json!({"ids": [b, c]}), c, true, vec![false]),
            ("mail.forwardAsAttachment", json!({"ids": [a, b, c], "discard": true}), c, true, vec![false]),
        ]
    };
    for n in 0..5 {
        let (mut app, [[a, b, c], [x, ..]], jobs) = window();
        let (command, params, source, attached, batches) = other([a, b, c], x)[n].clone();
        app.session.sel.messages = vec![a, b, c];
        app.run("mail.forwardAsAttachment", json!({})).unwrap();
        assert_eq!(app.run(command, params.clone()).unwrap()["pending"], "download", "{command} {params}");
        assert!(said(&app).contains("The forward of several messages that was waiting was called off"), "{command} {params}: {}", said(&app));
        assert_eq!(called_off(&jobs), batches, "{command} {params}");
        // What comes is for it, which opens; the forward that waited doesn't.
        answer(&mut app, &jobs, &[], &[]);
        let compose = app.session.compose.as_ref().unwrap_or_else(|| panic!("{command} {params}: {:?}", app.status_msg));
        assert_eq!((compose.source, compose.as_attachment), (Some(source), attached), "{command} {params}");
        assert!(asked(&jobs).is_empty(), "{command} {params}");
    }
}

#[test]
fn the_same_forward_asked_for_again_goes_on() {
    // Pressed again, as the other forward (with several they do the same), with the list sorted
    // the other way meanwhile, or with `discard` said to be false: nothing is called off, nor
    // asked for twice.
    let again = [
        ("mail.forwardAsAttachment", json!({}), false),
        ("mail.forward", json!({}), false),
        ("mail.forwardAsAttachment", json!({}), true),
        ("mail.forwardAsAttachment", json!({"discard": false}), false),
    ];
    for (command, params, resorted) in again {
        let (mut app, [[a, b, c], _], jobs) = window();
        app.session.sel.messages = vec![a, b, c];
        app.run("mail.forwardAsAttachment", json!({})).unwrap();
        assert_eq!(asked(&jobs).len(), 1);
        if resorted {
            app.session.view.sort_descending = !app.session.view.sort_descending;
        }
        assert_eq!(app.run(command, params.clone()).unwrap()["pending"], "download", "{command} {params}");
        assert_eq!(said(&app), "Downloading the messages first…", "{command} {params} {resorted}");
        assert_eq!(asked(&jobs).len(), 1, "{command} {params}");
        answer(&mut app, &jobs, &[], &[]);
        let mut list = ["Whole AA0M0", "Account 0 message 1", "Whole AA0M2"];
        if resorted {
            list.reverse();
        }
        assert_eq!(subjects(&app), list, "{command} {params}: in the list's order");
    }
}

#[test]
fn a_forward_left_waiting_in_another_mailbox_isnt_said_to_be_called_off() {
    let (mut app, [[a, b, c], [x, ..]], _jobs) = window();
    app.session.sel.messages = vec![a, b, c];
    app.run("mail.forwardAsAttachment", json!({})).unwrap();
    // Another mailbox takes this one's place, and a reply there waits for its message.
    let mailbox = app.session.mailbox.clone();
    app.session.replace_mailbox(mailbox);
    app.drop_secrets_if_replaced();
    app.judge_mailbox();
    app.confirm_servers().unwrap();
    let account = app.session.mailbox.message(x).unwrap().account;
    let tokens = Tokens { access: "stand-in".into(), refresh: "r".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    app.seed_graph_token(account, tokens);
    assert_eq!(app.run("mail.reply", json!({"id": x})).unwrap()["pending"], "download");
    assert_eq!(said(&app), "Downloading the message first…");
}
