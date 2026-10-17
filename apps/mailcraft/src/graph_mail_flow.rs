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

    // In the background: all the rest (a delta says no sizes, so none is held back as large).
    app.session.sync_progress.full_paused = false;
    wait(&mut app, &|app| app.session.mailbox.messages.values().all(|m| !m.unfetched));
    assert!(app.session.mailbox.message(by(&app, &ids[0])).unwrap().body.text.contains("The words of message 1."));
}
