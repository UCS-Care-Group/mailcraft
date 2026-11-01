// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's mail down through Microsoft Graph (R3.2), the whole way from the stand-in
//! through `mailcraft-net` into the mailbox: 200,000 messages, a run stopped, a run cut short, a
//! crash between a page and its save, a link the server forgot. Nothing twice, nothing missed.
//! Nothing here leaves this machine; every message is invented.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_mail, graph_mail_request};
use mailcraft_model::sync::GraphMailResult;
use mailcraft_model::{FlagStatus, FolderKind, Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::json;

const NOW: i64 = 1_791_993_600;

fn session() -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": "avery@mail.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    (s, account)
}

/// One run into `s`, every piece taken in as it comes; `each` sees the pieces taken so far and
/// may stop the run. The last piece.
fn run(s: &mut Session, account: Id, each: &mut dyn FnMut(&mut Session, usize, &AtomicBool)) -> GraphMailResult {
    let request = graph_mail_request(s, account).unwrap();
    let cancel = AtomicBool::new(false);
    let mut last = GraphMailResult::default();
    let mut n = 0;
    mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| {
        last = GraphMailResult { pages: Vec::new(), listing: None, ..piece.clone() };
        apply_graph_mail(s, piece).unwrap();
        n += 1;
        each(s, n, &cancel);
    });
    last
}

/// The server ids of the account's messages here, and whether any came twice.
fn here(s: &Session, account: Id) -> (HashSet<String>, usize) {
    let ids: Vec<String> =
        s.mailbox.messages.values().filter(|m| m.account == account).filter_map(|m| m.server.as_ref().map(|s| s.id.clone())).collect();
    let count = ids.len();
    (ids.into_iter().collect(), count)
}

fn there(server: &StandIn) -> HashSet<String> {
    server.state().mailbox.messages.keys().cloned().collect()
}

#[test]
fn two_hundred_thousand_messages_interrupted_three_times_arrive_once_each() {
    let server = StandIn::start().unwrap();
    let _route = server.route();
    let inbox_there = server.state().well_known("inbox");
    let archive_there = server.state().well_known("archive");
    let projects_there = server.state().add_folder("Projects", None);
    {
        let mut state = server.state();
        state.mailbox.generate(&inbox_there, 150_000, NOW);
        state.mailbox.generate(&archive_there, 45_000, NOW - 86_400);
        state.mailbox.generate(&projects_there, 5_000, NOW - 3_600);
    }
    let (mut s, account) = session();

    // 1. Stopped (Cancel) part way: what came is here, and the list is usable before the end.
    let first = run(&mut s, account, &mut |s, n, cancel| {
        if n == 437 {
            cancel.store(true, Ordering::Relaxed);
            let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
            assert!(s.mailbox.messages_in(inbox).count() >= 100, "the Inbox shows mail while the run goes on");
            assert!(s.sync_status.starts_with("Receiving "), "{}", s.sync_status);
        }
    });
    assert!(first.stopped && !first.whole);
    assert!(s.sync_status.starts_with("Stopped"), "{}", s.sync_status);
    let (ids, count) = here(&s, account);
    assert_eq!(ids.len(), count, "nothing twice");
    assert!(count > 40_000 && count < 200_000);

    // Meanwhile: a message here is fetched and flagged here; on the server it is marked read and
    // moved to Projects; another is deleted there.
    let (moved, flagged_id) = {
        let m = s.mailbox.messages.values().find(|m| m.account == account && m.server.is_some()).unwrap().clone();
        let mm = s.mailbox.message_mut(m.id).unwrap();
        mm.unfetched = false;
        mm.body.text = "The whole of it, fetched.".into();
        mm.flag = FlagStatus::Flagged;
        (m.server.clone().unwrap().id, m.id)
    };
    let deleted = here(&s, account).0.into_iter().filter(|id| *id != moved).min().unwrap();
    {
        let mut state = server.state();
        state.mailbox.update(&moved, |m| m.read = true);
        state.mailbox.move_to(&moved, &projects_there);
        state.mailbox.delete(&deleted);
    }

    // 2. A page that doesn't come (the connection went): that folder waits, the run goes on.
    let second = run(&mut s, account, &mut |_, n, cancel| {
        if n == 300 {
            let mut state = server.state();
            state.fail_next.push((format!("GET /v1.0/me/mailFolders/{inbox_there}/messages/delta"), 500, "ServiceUnavailable".into()));
        }
        if n == 900 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert!(!second.whole);
    let (ids, count) = here(&s, account);
    assert_eq!(ids.len(), count, "nothing twice");

    // 3. A crash between a page and its save: the file has what was saved before, and the run
    // brings again what came after it.
    let mut saved: Option<Mailbox> = None;
    run(&mut s, account, &mut |s, n, cancel| {
        if n == 200 {
            saved = Some(s.mailbox.clone());
        }
        if n == 260 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    let mut s = Session::at(saved.unwrap(), NOW, 0);
    // To the end.
    let last = run(&mut s, account, &mut |_, _, _| {});
    assert!(last.whole && !last.stopped && last.errors.is_empty(), "{last:?}");
    let (ids, count) = here(&s, account);
    assert_eq!(ids.len(), count, "nothing twice");
    assert_eq!(ids, there(&server), "nothing missed, nothing kept that the server hasn't");
    assert_eq!(count, 199_999);
    // The moved message is in Projects with what was fetched of it and its flag from here; its
    // read mark is the server's (it wasn't changed here).
    let m = s.mailbox.message(flagged_id).unwrap();
    let projects = s.mailbox.folder_by_server(account, &projects_there).unwrap();
    assert_eq!(m.folder, projects);
    assert_eq!((m.body.text.as_str(), m.unfetched, m.flag, m.read), ("The whole of it, fetched.", false, FlagStatus::Flagged, true));
    // Everything else came as headers and a preview.
    assert!(s.mailbox.messages.values().filter(|m| m.account == account && m.id != flagged_id).all(|m| m.unfetched));

    // 4. A link the server forgot: the folder is listed afresh, and only what a complete listing
    // lacks goes.
    let gone_meanwhile = server.state().mailbox.messages.values().find(|m| m.folder == archive_there).unwrap().id.clone();
    {
        let mut state = server.state();
        state.mailbox.forget_links();
        state.mailbox.delete(&gone_meanwhile);
    }
    let relisted = run(&mut s, account, &mut |_, _, _| {});
    assert!(relisted.whole);
    let (ids, count) = here(&s, account);
    assert_eq!((ids.len(), count), (199_998, 199_998));
    assert_eq!(ids, there(&server));
    assert_eq!(s.mailbox.message(flagged_id).map(|m| m.body.text.as_str()), Some("The whole of it, fetched."));
}

/// The window, its Send/Receive and the background fetch, the whole way: messages come down as
/// headers and previews; one not yet here whole is fetched when it is opened; a reply to one not
/// yet fetched waits for it; and the rest come whole in the background.
#[test]
fn messages_download_whole_in_the_background_on_opening_and_before_a_reply() {
    use mailcraft_model::sync::Tokens;
    use mailcraft_net::oauth::GraphTokens;
    use mailcraft_ui_egui::graph_fetch::GraphFetchJob;
    use mailcraft_ui_egui::{GraphRequest, MailApp, Services};

    let server = StandIn::start().unwrap();
    let inbox_there = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox_there, 30, NOW);
    let url = server.url();
    let session = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let mut app = MailApp::new(session, Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let base = url.clone();
    app.services.graph = Some(Box::new(move |request: GraphRequest| {
        let (tx, rx) = std::sync::mpsc::channel();
        let base = base.clone();
        std::thread::spawn(move || {
            let _route = mailcraft_net::graph::standin::route(&base);
            let tokens = Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
            let renewed = GraphTokens { tokens, extras_missing: false, covers_favorites: false };
            let _ = tx.send(super::graph_with(request, "UTC", &move |_, _, _| Ok(renewed.clone())));
        });
        rx
    }));
    let base = url.clone();
    app.services.graph_fetch = Some(Box::new(move |job: GraphFetchJob| {
        let (tx, rx) = std::sync::mpsc::channel();
        let base = base.clone();
        std::thread::spawn(move || {
            let _route = mailcraft_net::graph::standin::route(&base);
            let _ = tx.send(mailcraft_net::graph::fetch_messages(&job.access, &job.request, &job.cancel));
        });
        rx
    }));
    // IMAP is there, and never asked.
    app.services.sync = Some(Box::new(|_| std::sync::mpsc::channel().1));
    let account = app.execute("account.add", json!({"email": "avery@mail.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.seed_graph_token(
        account,
        Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() },
    );
    let wait = |app: &mut MailApp, done: &dyn Fn(&MailApp) -> bool| {
        for _ in 0..4_000 {
            app.poll_graph();
            app.drive_fetch();
            if done(app) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("it didn't happen");
    };
    // The run, with the background held back (as after Stop) so that what is opened isn't here yet.
    let synced = |app: &mut MailApp| {
        app.execute("ui.sync", json!({"account": account})).unwrap();
        for _ in 0..4_000 {
            app.poll_graph();
            if !app.graph_running() {
                app.session.sync_progress.full_paused = true;
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the run didn't end");
    };
    synced(&mut app);
    let by = |app: &MailApp, id: &str| app.session.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|s| s.id == id)).unwrap().id;
    let opened = by(&app, &ids[3]);
    assert!(app.session.mailbox.message(opened).unwrap().unfetched);

    // Opened: at once.
    app.session.sel.messages = vec![opened];
    wait(&mut app, &|app| !app.session.mailbox.message(opened).unwrap().unfetched);

    // A reply to one that isn't here whole waits for it, and then opens with its text.
    let late = server.state().mailbox.add(&inbox_there, "Late", NOW + 60);
    synced(&mut app);
    let late = by(&app, &late);
    assert!(app.session.mailbox.message(late).unwrap().unfetched);
    let pending = app.run("mail.reply", json!({"id": late})).unwrap();
    assert_eq!(pending["pending"], "download");
    wait(&mut app, &|app| app.session.compose.is_some());
    assert!(app.session.compose.as_ref().unwrap().subject.contains("Late"));
    assert!(!app.session.mailbox.message(late).unwrap().unfetched);

    // So does a forward as an attachment, which attaches the message whole (#315).
    app.execute("compose.close", json!({})).unwrap();
    let later = server.state().mailbox.add(&inbox_there, "Later", NOW + 120);
    synced(&mut app);
    let later = by(&app, &later);
    assert!(app.session.mailbox.message(later).unwrap().unfetched);
    // Chosen in the window, it is the selected one; once it is here it is that one still, though
    // another was selected meanwhile.
    app.session.sel.messages = vec![later];
    let pending = app.run("mail.forwardAsAttachment", json!({})).unwrap();
    assert_eq!(pending["pending"], "download");
    app.session.sel.messages = vec![late];
    wait(&mut app, &|app| app.session.compose.is_some());
    assert_eq!(app.session.compose.as_ref().unwrap().source, Some(later));
    assert!(!app.session.mailbox.message(later).unwrap().unfetched);
    let c = app.session.compose.as_ref().unwrap();
    assert_eq!(c.subject, "FW: Later");
    match c.attachments.as_slice() {
        [eml] => assert!(eml.name == "Later.eml" && eml.mime == "message/rfc822" && String::from_utf8_lossy(&eml.data).contains("Subject: Later")),
        other => panic!("{} attachments", other.len()),
    }
    // A reply chosen in the window keeps its message too, as every command that waits does.
    app.execute("compose.close", json!({})).unwrap();
    let again = server.state().mailbox.add(&inbox_there, "Once more", NOW + 150);
    synced(&mut app);
    let again = by(&app, &again);
    app.session.sel.messages = vec![again];
    let pending = app.run("mail.reply", json!({})).unwrap();
    assert_eq!(pending["pending"], "download");
    app.session.sel.messages = vec![late];
    wait(&mut app, &|app| app.session.compose.is_some());
    assert_eq!(app.session.compose.as_ref().unwrap().source, Some(again));
    assert!(app.session.compose.as_ref().unwrap().subject.contains("Once more"));
    // Several of them (#360): those that aren't here whole come in one go, and the new message
    // opens with each of them whole, though the selection moved meanwhile.
    app.execute("compose.close", json!({})).unwrap();
    let first = server.state().mailbox.add(&inbox_there, "First of two", NOW + 160);
    let second = server.state().mailbox.add(&inbox_there, "Second of two", NOW + 170);
    synced(&mut app);
    let (first, second) = (by(&app, &first), by(&app, &second));
    assert!([first, second].iter().all(|id| app.session.mailbox.message(*id).unwrap().unfetched));
    app.session.sel.messages = vec![first, late, second];
    let pending = app.run("mail.forward", json!({})).unwrap();
    assert_eq!(pending["pending"], "download");
    app.session.sel.messages = vec![late];
    wait(&mut app, &|app| app.session.compose.is_some());
    let c = app.session.compose.as_ref().unwrap();
    assert_eq!((c.source, c.as_attachment), (Some(second), true));
    let names: Vec<&str> = c.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Second of two.eml", "First of two.eml", "Late.eml"], "the list's order: newest first");
    assert!(c.attachments.iter().all(|eml| String::from_utf8_lossy(&eml.data).contains("The words of")), "each whole");
    // A call that names its message wrongly is refused at once, with another message selected:
    // nothing is fetched for it, and the selected message is never taken for it (#360).
    app.execute("compose.close", json!({})).unwrap();
    let latest = server.state().mailbox.add(&inbox_there, "Latest", NOW + 180);
    synced(&mut app);
    let latest = by(&app, &latest);
    app.session.sel.messages = vec![latest];
    let e = app.run("mail.forwardAsAttachment", json!({"id": first.to_string()})).unwrap_err();
    assert!(e.contains("`id` is a message id"), "{e}");
    assert!(app.session.compose.is_none(), "a message id as text is refused");
    assert!(app.session.mailbox.message(latest).unwrap().unfetched, "nothing waited for");
    // Nor one whose look-up found nothing, which names no message (#512).
    for command in ["mail.forward", "mail.forwardAsAttachment"] {
        for params in [json!({"id": null}), json!({"ids": null})] {
            let e = app.run(command, params.clone()).unwrap_err();
            assert!(e.contains("is null"), "{command} {params}: {e}");
            assert!(app.session.compose.is_none(), "{command} {params}");
            assert!(app.session.mailbox.message(latest).unwrap().unfetched, "{command} {params}: nothing waited for");
        }
    }

    // In the background: all the rest (a delta says no sizes, so none is held back as large).
    app.session.sync_progress.full_paused = false;
    wait(&mut app, &|app| app.session.mailbox.messages.values().all(|m| !m.unfetched));
    assert!(app.session.mailbox.message(by(&app, &ids[0])).unwrap().body.text.contains("The words of message 1."));
}

/// The background download goes the whole way past what it can't take: a message over 16 MB
/// comes whole, and messages the server refuses, three of them one after another among them, are
/// asked for once and passed over, never holding up the rest (#175).
#[test]
fn a_message_over_16_mb_and_those_refused_dont_hold_the_background_up() {
    use mailcraft_model::sync::Tokens;
    use mailcraft_net::graph::standin::Fault;
    use mailcraft_net::oauth::GraphTokens;
    use mailcraft_ui_egui::graph_fetch::GraphFetchJob;
    use mailcraft_ui_egui::{GraphRequest, MailApp, Services};

    let server = StandIn::start().unwrap();
    let inbox_there = server.state().well_known("inbox");
    // Newest first: the large one, then three refused one after another, and one more later on.
    let ids = server.state().mailbox.generate(&inbox_there, 25, NOW);
    let large = format!(
        "From: avery@mail.example\r\nTo: owner@mail.example\r\nSubject: Large\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe large one.\r\n{}",
        "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+/\r\n".repeat(300_000)
    );
    assert!(large.len() > 17 * 1024 * 1024);
    let refused = [&ids[1], &ids[2], &ids[3], &ids[12]];
    {
        let mut state = server.state();
        state.mailbox.update(&ids[0], |m| m.mime = Some(large.clone()));
        for id in refused {
            state.mailbox.faults.insert(id.clone(), Fault::Invalid);
        }
    }
    let url = server.url();
    let session = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let mut app = MailApp::new(session, Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let base = url.clone();
    app.services.graph = Some(Box::new(move |request: GraphRequest| {
        let (tx, rx) = std::sync::mpsc::channel();
        let base = base.clone();
        std::thread::spawn(move || {
            let _route = mailcraft_net::graph::standin::route(&base);
            let tokens = Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
            let renewed = GraphTokens { tokens, extras_missing: false, covers_favorites: false };
            let _ = tx.send(super::graph_with(request, "UTC", &move |_, _, _| Ok(renewed.clone())));
        });
        rx
    }));
    let base = url.clone();
    app.services.graph_fetch = Some(Box::new(move |job: GraphFetchJob| {
        let (tx, rx) = std::sync::mpsc::channel();
        let base = base.clone();
        std::thread::spawn(move || {
            let _route = mailcraft_net::graph::standin::route(&base);
            let _ = tx.send(mailcraft_net::graph::fetch_messages(&job.access, &job.request, &job.cancel));
        });
        rx
    }));
    app.services.sync = Some(Box::new(|_| std::sync::mpsc::channel().1));
    let account = app.execute("account.add", json!({"email": "avery@mail.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.seed_graph_token(
        account,
        Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() },
    );
    app.execute("ui.sync", json!({"account": account})).unwrap();
    let here = |app: &MailApp, id: &str| app.session.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|s| s.id == id)).cloned();
    let done = |app: &MailApp| !app.graph_running() && ids.iter().all(|id| here(app, id).is_some_and(|m| refused.contains(&id) || !m.unfetched));
    // Well within the 20 s an account that stopped waits: nothing stops.
    let mut finished = false;
    for _ in 0..5_000 {
        app.poll_graph();
        app.drive_fetch();
        if done(&app) {
            finished = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let left: Vec<_> = ids.iter().filter(|id| here(&app, id).is_none_or(|m| m.unfetched)).collect();
    assert!(finished, "still not here whole: {left:?}");
    let big = here(&app, &ids[0]).unwrap();
    assert!(big.body.text.starts_with("The large one.") && big.body.text.len() > 16 * 1024 * 1024);
    // Each refused one was asked for once, and is passed over from then on.
    for id in refused {
        assert!(here(&app, id).unwrap().unfetched);
        let asked = server.state().requests.iter().filter(|r| r.target.contains("$value") && r.target.contains(id.trim_end_matches('='))).count();
        assert_eq!(asked, 1, "{id}");
    }
}

/// What changed on the server after every folder had come to its end (read on a phone, flagged
/// and categorised in Outlook on the web) comes down with the next Send/Receive, across a save
/// and a reopen of the mailbox: Exchange Online says a change of read state alone as the
/// message's id and `isRead`, nothing else (#148).
#[test]
fn what_changed_on_the_server_after_a_whole_run_comes_down_with_the_next() {
    let server = StandIn::start().unwrap();
    let _route = server.route();
    let inbox_there = server.state().well_known("inbox");
    let ids = {
        let mut state = server.state();
        state.mailbox.read_alone = true;
        state.mailbox.generate(&inbox_there, 5, NOW)
    };
    let (mut s, account) = session();
    let first = run(&mut s, account, &mut |_, _, _| {});
    assert!(first.whole, "{first:?}");
    let all_caught_up = s.mailbox.folders.iter().filter(|f| f.account == account).filter_map(|f| f.server.as_ref()).all(|f| f.caught_up);
    assert!(all_caught_up);
    // Saved and opened again, as the window does between runs.
    let dir = std::env::temp_dir().join(format!("mailcraft-changes-down-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("changes.mailcraft");
    mailcraft_engine::store::save(&path, &s.mailbox).unwrap();
    let mut s = Session::at(mailcraft_engine::store::open(&path).unwrap().mailbox, NOW, 0);
    let _ = std::fs::remove_dir_all(&dir);
    {
        let mut state = server.state();
        state.mailbox.update(&ids[0], |m| m.read = true);
        state.mailbox.update(&ids[1], |m| m.flag = "flagged".into());
        state.mailbox.update(&ids[2], |m| m.categories = vec!["Red category".into()]);
    }
    let second = run(&mut s, account, &mut |_, _, _| {});
    assert!(second.whole && second.errors.is_empty(), "{second:?}");
    let here = |s: &Session, id: &str| s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|s| s.id == *id)).unwrap().clone();
    assert!(here(&s, &ids[0]).read, "read on the phone");
    assert_eq!(here(&s, &ids[1]).flag, FlagStatus::Flagged);
    assert_eq!(here(&s, &ids[2]).categories, ["Red category"]);
    assert!(!here(&s, &ids[3]).read && here(&s, &ids[3]).flag == FlagStatus::None);
    // And back: marked unread there, it is unread here.
    server.state().mailbox.update(&ids[0], |m| m.read = false);
    run(&mut s, account, &mut |_, _, _| {});
    assert!(!here(&s, &ids[0]).read, "marked unread on the phone");
}
