// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A shared mailbox in the window (#70): read with its owner's sign-in, its permission asked for
//! by itself when the owner asked for the run, and a mailbox the server won't open left alone by
//! runs that start by themselves. Hosts are fakes; every address is invented.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::{GraphMailRequest, GraphMailResult, SyncRequest, SyncResult, Tokens};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::{GraphRequest, GraphResult, MailApp, Services, SignInRequest};

const OWNER: &str = "noor@example.org";
const SHARED: &str = "support@example.org";

/// Set when anything asked for a mail-protocol run: a shared mailbox is read through Graph alone.
/// Tests run on threads of one process, so each test reads it only through [`no_imap`], which
/// every test here shares: none of them may ever set it.
static IMAP_USED: AtomicBool = AtomicBool::new(false);

fn no_imap() {
    assert!(!IMAP_USED.load(Ordering::Relaxed), "a mail-protocol run was started");
}

type Answers = Arc<Mutex<VecDeque<(GraphResult, Vec<GraphMailResult>)>>>;
/// What each Graph job was asked: the account, the refresh token, the mail request, the Outbox's
/// length and whether the calendar was asked for.
type Jobs = Arc<Mutex<Vec<(Id, String, Option<GraphMailRequest>, usize, bool)>>>;
/// What each sign-in was asked: the account, for Graph, for shared mailboxes, the address.
type SignIns = Arc<Mutex<Vec<(Id, bool, bool, String)>>>;

fn tokens(access: &str, refresh: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

/// A window with a Microsoft account signed in this session, and hosts that note what they are
/// asked and answer what `answers` holds, in turn.
fn window() -> (MailApp, Id, Answers, Jobs, SignIns) {
    let session = Session::empty();
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.tokens.insert(owner, tokens("own-access", "ref-1"));
    let (answers, jobs, sign_ins): (Answers, Jobs, SignIns) = Default::default();
    let (to_answer, log) = (answers.clone(), jobs.clone());
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        log.lock().unwrap().push((req.account.id, req.refresh.clone(), req.mail.clone(), req.outbox.len(), req.directory));
        let (tx, rx) = mpsc::channel();
        let (answer, pieces) = to_answer.lock().unwrap().pop_front().unwrap_or_default();
        if let Some(to) = &req.mail_pieces {
            for mut piece in pieces {
                // (A piece that names no account is for the one asked about.)
                if piece.account == 0 {
                    piece.account = req.account.id;
                }
                to.send(piece).unwrap();
            }
        }
        tx.send(answer).unwrap();
        rx
    }));
    // The desktop host can open mail connections; a shared mailbox never uses them (it has no
    // IMAP leg), which [`no_imap`] checks at the end of every test.
    app.services.sync = Some(Box::new(|req: mailcraft_model::sync::SyncRequest| {
        IMAP_USED.store(true, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        tx.send(mailcraft_model::sync::SyncResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    let log = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.account, req.graph, req.shared, req.email.clone()));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Tokens { email: OWNER.into(), ..tokens("shared-access", "ref-2") })).unwrap();
        rx
    }));
    (app, owner, answers, jobs, sign_ins)
}

#[test]
fn a_shared_mailbox_is_read_with_its_owners_sign_in_and_its_permission_is_asked_for_once() {
    let (mut app, owner, answers, jobs, sign_ins) = window();
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    // Its first run: the owner's sign-in doesn't cover shared mailboxes yet.
    answers.lock().unwrap().push_back((GraphResult { account: shared, consent_needed: true, ..Default::default() }, Vec::new()));
    let listing = mailcraft_model::sync::FolderListing {
        folders: vec![mailcraft_model::sync::GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }],
        complete: true,
    };
    let piece = GraphMailResult { account: shared, email: SHARED.into(), listing: Some(listing), ..Default::default() };
    answers
        .lock()
        .unwrap()
        .push_back((GraphResult { account: shared, tokens: Some(tokens("shared-access-2", "ref-3")), ..Default::default() }, vec![piece]));

    assert_eq!(app.execute("ui.sync", json!({"account": shared})).unwrap()["shared"], true);
    {
        let jobs = jobs.lock().unwrap();
        let (account, refresh, mail, outbox, directory) = jobs[0].clone();
        assert_eq!((account, refresh.as_str(), outbox, directory), (shared, "ref-1", 0, false), "the owner's refresh token, mail alone");
        let mail = mail.unwrap();
        assert_eq!((mail.account, mail.email.as_str(), mail.shared), (shared, SHARED, true));
    }
    app.poll_graph();
    // Asked for on Microsoft's page, by itself, for the owner.
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, true, true, OWNER.to_string())]);
    app.poll_sign_in();
    // Allowed: read at once, and its folders come in under it.
    assert_eq!(jobs.lock().unwrap().len(), 2);
    app.poll_graph();
    assert!(app.session.mailbox.folder_by_server(shared, "in").is_some());
    assert!(app.session.mailbox.folder_by_server(owner, "in").is_none());
    // Its access token is its own, in memory, for fetching; the owner's is as it was.
    assert_eq!(app.graph_tokens.get(&shared).map(|t| t.access.as_str()), Some("shared-access-2"));
    assert_eq!(app.tokens.get(&owner).map(|t| (t.access.as_str(), t.refresh.as_str())), Some(("own-access", "ref-3")));
    assert_eq!(sign_ins.lock().unwrap().len(), 1, "asked once");
    no_imap();
}

#[test]
fn a_shared_mailbox_the_server_wont_open_waits_for_the_owner_to_ask() {
    let (mut app, _, answers, jobs, sign_ins) = window();
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    let refused = GraphMailResult {
        account: shared,
        email: SHARED.into(),
        refused: Some(mailcraft_model::sync::SharedRefusal::NoAccess),
        errors: vec!["Microsoft won't open this shared mailbox for you: you need Full Access to it.".into()],
        ..Default::default()
    };
    answers.lock().unwrap().push_back((GraphResult { account: shared, ..Default::default() }, vec![refused]));
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    app.poll_graph();
    assert!(app.session.sync_status.contains("Full Access"), "{}", app.session.sync_status);
    assert!(app.shared_refused.contains_key(&shared) && !app.can_sync_quietly(shared));
    // A run that starts by itself doesn't ask again; the owner's own does.
    assert!(app.execute("ui.sync", json!({"account": shared, "quiet": true})).is_err());
    assert_eq!(jobs.lock().unwrap().len(), 1);
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    app.poll_graph();
    assert_eq!(jobs.lock().unwrap().len(), 2);
    assert!(!app.shared_refused.contains_key(&shared), "it opened this time");
    assert!(sign_ins.lock().unwrap().is_empty(), "nobody was sent to a sign-in page");
    no_imap();
}

#[test]
fn the_add_shared_mailbox_dialog_collects_the_address_and_runs_the_command() {
    let (mut app, owner, answers, jobs, _) = window();
    // Nothing to open it under: a local account can't have one.
    let local = app.execute("account.add", json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert!(app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": local})).is_err());
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    assert!(matches!(app.dialog, Some(crate::dialogs::Dialog::SharedMailbox { account, .. }) if account == owner));
    // Laid out in both themes, as File › Info is with a shared mailbox under its owner.
    app.execute("account.addSharedMailbox", json!({"account": owner, "address": "sales@example.org"})).unwrap();
    let mut h =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for theme in [crate::theme::Appearance::Light, crate::theme::Appearance::Dark] {
        h.state_mut().ui.theme = theme;
        h.run_steps(3);
        h.state_mut().execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
        h.run_steps(3);
        h.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    }
    // An address that is no address is said in the dialog; a good one adds the mailbox and reads it.
    if let Some(crate::dialogs::Dialog::SharedMailbox { address, .. }) = h.state_mut().dialog.as_mut() {
        *address = "not an address".into();
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(matches!(&h.state().dialog, Some(crate::dialogs::Dialog::SharedMailbox { error, .. }) if error.contains("e-mail address")));
    if let Some(crate::dialogs::Dialog::SharedMailbox { address, .. }) = h.state_mut().dialog.as_mut() {
        *address = SHARED.into();
    }
    // Its Inbox is read, and then it is kept.
    answers.lock().unwrap().push_back((GraphResult::default(), vec![inbox_read(SHARED)]));
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(h.state().dialog.is_none(), "{:?}", h.state().dialog.as_ref().map(|d| d.name()));
    let added = h.state().session.mailbox.accounts.iter().find(|a| a.email == SHARED).map(|a| (a.id, a.shared_of));
    assert!(h.state().shared_checking.is_empty());
    let (id, of) = added.unwrap();
    assert_eq!(of, Some(owner));
    assert!(jobs.lock().unwrap().iter().any(|(account, ..)| *account == id), "read at once");
    no_imap();
}

/// A run of a shared mailbox's that read its Inbox: the folders, by the account's address. (The
/// account's id is the one the run was for: the window's fake host fills it in.)
fn inbox_read(email: &str) -> GraphMailResult {
    let listing = mailcraft_model::sync::FolderListing {
        folders: vec![mailcraft_model::sync::GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }],
        complete: true,
    };
    GraphMailResult { email: email.into(), listing: Some(listing), ..Default::default() }
}

/// Add Shared Mailbox with `address` under `owner`, as the dialog does, the window's hosts
/// answering its first run with `pieces`. The dialog, after.
fn add_in_dialog(app: &mut MailApp, answers: &Answers, owner: Id, address: &str, pieces: Vec<GraphMailResult>) {
    answers.lock().unwrap().push_back((GraphResult::default(), pieces));
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    if let Some(crate::dialogs::Dialog::SharedMailbox { address: typed, .. }) = app.dialog.as_mut() {
        *typed = address.into();
    }
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let mut out = ctx.run_ui(egui::RawInput { events: vec![enter()], ..Default::default() }, |ui| crate::dialogs::show(app, &ui.ctx().clone()));
    out.textures_delta.clear();
}

fn enter() -> egui::Event {
    egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
}

#[test]
fn add_shared_mailbox_keeps_only_a_mailbox_whose_inbox_could_be_read() {
    use mailcraft_model::sync::SharedRefusal;
    let (mut app, owner, answers, jobs, sign_ins) = window();
    for (why, said) in [
        (SharedRefusal::NoAccess, "You don't have access to support@example.org. If access was just given, try again in an hour."),
        (SharedRefusal::NoMailbox, "There's no mailbox at support@example.org."),
        (SharedRefusal::NotThroughGraph, "can't be reached through Microsoft Graph"),
    ] {
        let refused = GraphMailResult { email: SHARED.into(), refused: Some(why), errors: vec![format!("{}.", why.said())], ..Default::default() };
        add_in_dialog(&mut app, &answers, owner, SHARED, vec![refused]);
        // Being checked: the dialog stays, and says so.
        assert!(matches!(&app.dialog, Some(crate::dialogs::Dialog::SharedMailbox { checking: Some(_), .. })));
        app.poll_graph();
        // Nothing is added, and the dialog says why, in the owner's words.
        assert!(app.session.mailbox.accounts.iter().all(|a| a.email != SHARED), "{why:?}");
        assert!(app.shared_checking.is_empty() && app.shared_refused.is_empty());
        match &app.dialog {
            Some(crate::dialogs::Dialog::SharedMailbox { checking: None, error, .. }) => assert!(error.contains(said), "{error}"),
            _ => panic!("the dialog stays open"),
        }
        app.dialog = None;
    }
    // A run that brings nothing back (offline, say) doesn't keep it either.
    add_in_dialog(&mut app, &answers, owner, SHARED, Vec::new());
    app.poll_graph();
    assert!(app.session.mailbox.accounts.iter().all(|a| a.email != SHARED));
    app.dialog = None;
    // Called off while it is checked: not kept.
    add_in_dialog(&mut app, &answers, owner, SHARED, vec![inbox_read(SHARED)]);
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let escape = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
    let mut out = ctx.run_ui(egui::RawInput { events: vec![escape], ..Default::default() }, |ui| crate::dialogs::show(&mut app, &ui.ctx().clone()));
    out.textures_delta.clear();
    assert!(app.dialog.is_none() && app.session.mailbox.accounts.iter().all(|a| a.email != SHARED));
    app.poll_graph();
    assert!(app.session.mailbox.accounts.iter().all(|a| a.email != SHARED), "an answer that comes after adds nothing");
    // Read: kept, and the dialog closes.
    add_in_dialog(&mut app, &answers, owner, SHARED, vec![inbox_read(SHARED)]);
    app.poll_graph();
    let kept = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED).map(|a| a.id).unwrap();
    assert!(app.dialog.is_none() && app.shared_checking.is_empty());
    assert!(app.session.mailbox.folder_by_server(kept, "in").is_some());
    assert!(
        jobs.lock().unwrap().iter().all(|(_, _, mail, ..)| mail.as_ref().is_some_and(|m| m.shared && m.messages.is_empty() && m.drafts.is_empty()))
    );
    assert!(sign_ins.lock().unwrap().is_empty());
    no_imap();
}

#[test]
fn add_shared_mailbox_closes_once_the_inbox_is_read_without_waiting_for_the_whole_mailbox() {
    let (mut app, owner, _, _, _) = window();
    // A big mailbox: the folders come first, and the rest of the run is still under way (the
    // host holds on to the run's pieces and its answer).
    type Run = (mpsc::SyncSender<GraphMailResult>, mpsc::Sender<GraphResult>);
    let running: Arc<Mutex<Option<Run>>> = Default::default();
    let keep = running.clone();
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        let pieces = req.mail_pieces.clone().unwrap();
        pieces.send(GraphMailResult { account: req.account.id, partial: true, ..inbox_read(SHARED) }).unwrap();
        *keep.lock().unwrap() = Some((pieces, tx));
        rx
    }));
    let answers: Answers = Default::default();
    add_in_dialog(&mut app, &answers, owner, SHARED, Vec::new());
    assert!(matches!(&app.dialog, Some(crate::dialogs::Dialog::SharedMailbox { checking: Some(_), .. })));
    app.poll_graph();
    // Kept, and the dialog is gone, while the run goes on bringing the rest down.
    let kept = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED).map(|a| a.id).unwrap();
    assert!(app.dialog.is_none() && app.shared_checking.is_empty());
    assert!(app.session.mailbox.folder_by_server(kept, "in").is_some());
    assert!(app.graph_rx.is_some(), "the run is still under way");
    // Its end changes nothing about that.
    let (pieces, answer) = running.lock().unwrap().take().unwrap();
    drop(pieces);
    answer.send(GraphResult { account: kept, ..Default::default() }).unwrap();
    app.poll_graph();
    app.poll_graph();
    assert!(app.session.mailbox.account(kept).is_some() && app.dialog.is_none());
    no_imap();
}

#[test]
fn add_shared_mailbox_while_a_send_receive_is_under_way_calls_it_off_and_reads_the_new_one_first() {
    let (mut app, owner, _, _, _) = window();
    // The owner's own run is bringing a big mailbox down when Add is pressed: the host holds on
    // to it until it is called off. The shared mailbox's run answers at once.
    type Run = (mpsc::SyncSender<GraphMailResult>, mpsc::Sender<GraphResult>);
    let running: Arc<Mutex<Option<Run>>> = Default::default();
    let asked: Arc<Mutex<Vec<Id>>> = Default::default();
    let (keep, log) = (running.clone(), asked.clone());
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        log.lock().unwrap().push(req.account.id);
        let (tx, rx) = mpsc::channel();
        let pieces = req.mail_pieces.clone().unwrap();
        if req.account.email == SHARED {
            pieces.send(GraphMailResult { account: req.account.id, partial: true, ..inbox_read(SHARED) }).unwrap();
            drop(pieces);
            tx.send(GraphResult { account: req.account.id, ..Default::default() }).unwrap();
        } else {
            *keep.lock().unwrap() = Some((pieces, tx));
        }
        rx
    }));
    app.execute("ui.sync", json!({"account": owner})).unwrap();
    assert!(app.graph_rx.is_some());
    let answers: Answers = Default::default();
    add_in_dialog(&mut app, &answers, owner, SHARED, Vec::new());
    // Not refused as "already running": it waits, and the owner's run is called off.
    assert!(matches!(&app.dialog, Some(crate::dialogs::Dialog::SharedMailbox { checking: Some(_), error, .. }) if error.is_empty()));
    let shared = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED).map(|a| a.id).unwrap();
    assert!(app.services.sync_cancel.load(Ordering::Relaxed));
    assert_eq!(app.sync_queue.front(), Some(&(owner, false)), "the owner's run goes on after, as asked for");
    app.shared_check_tick();
    assert_eq!(*asked.lock().unwrap(), [owner], "nothing starts while the owner's run is stopping");
    // The owner's run stops.
    let (pieces, answer) = running.lock().unwrap().take().unwrap();
    drop(pieces);
    answer.send(GraphResult { account: owner, ..Default::default() }).unwrap();
    app.poll_graph();
    app.poll_graph();
    assert!(app.graph_rx.is_none());
    // The shared mailbox is read first, before anything else in line, and kept: the dialog
    // closes on the folders, which are its run's first piece.
    app.shared_check_tick();
    assert_eq!(*asked.lock().unwrap(), [owner, shared]);
    app.poll_graph();
    assert!(app.dialog.is_none() && app.shared_checking.is_empty());
    assert!(app.session.mailbox.account(shared).is_some() && app.session.mailbox.folder_by_server(shared, "in").is_some());
    // Then the owner's run goes on (the window's frame: a run by itself after the check).
    app.auto_sync();
    assert_eq!(*asked.lock().unwrap(), [owner, shared, owner], "the called-off run goes on after");
    no_imap();
}

#[test]
fn add_shared_mailbox_while_a_mail_protocol_run_is_under_way_calls_it_off_and_reads_the_new_one_first() {
    let (mut app, owner, answers, jobs, _) = window();
    // Another account's run, over the mail protocols, is bringing a big mailbox down when Add is
    // pressed: this host holds on to it until it is called off. (The window's own host would
    // take any run for one of a shared mailbox's: this test's notes whose it was.)
    let running: Arc<Mutex<Option<mpsc::Sender<SyncResult>>>> = Default::default();
    let asked: Arc<Mutex<Vec<Id>>> = Default::default();
    let (keep, log) = (running.clone(), asked.clone());
    app.services.sync = Some(Box::new(move |req: SyncRequest| {
        log.lock().unwrap().push(req.account.id);
        let (tx, rx) = mpsc::channel();
        *keep.lock().unwrap() = Some(tx);
        rx
    }));
    let other = app.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    app.passwords.insert(other, "pw".into());
    app.execute("ui.sync", json!({"account": other})).unwrap();
    assert!(app.sync_rx.is_some());
    add_in_dialog(&mut app, &answers, owner, SHARED, vec![inbox_read(SHARED)]);
    // Not refused as "already running": it waits, and the other account's run is called off.
    assert!(matches!(&app.dialog, Some(crate::dialogs::Dialog::SharedMailbox { checking: Some(_), error, .. }) if error.is_empty()));
    let shared = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED).map(|a| a.id).unwrap();
    assert!(app.services.sync_cancel.load(Ordering::Relaxed));
    assert!(matches!(app.sync_run, Some((whose, _, true, _)) if whose == other), "called off, to go on after");
    app.poll_sync();
    app.shared_check_tick();
    assert!(jobs.lock().unwrap().is_empty(), "nothing starts while the other run is stopping");
    // The host stops: the run's last piece.
    running.lock().unwrap().take().unwrap().send(SyncResult { account: other, stopped: true, ..Default::default() }).unwrap();
    app.poll_sync();
    assert!(app.sync_rx.is_none());
    assert_eq!(app.sync_queue.front(), Some(&(other, false)), "the other account's run goes on after, as asked for");
    // The shared mailbox is read first, through Graph alone, and the dialog closes on its folders.
    app.shared_check_tick();
    assert_eq!(jobs.lock().unwrap().iter().map(|(a, ..)| *a).collect::<Vec<_>>(), [shared]);
    app.poll_graph();
    app.poll_graph();
    assert!(app.dialog.is_none() && app.shared_checking.is_empty());
    assert!(app.session.mailbox.folder_by_server(shared, "in").is_some());
    // Then the other account's run goes on.
    app.auto_sync();
    assert_eq!(*asked.lock().unwrap(), [other, other]);
    assert_eq!(jobs.lock().unwrap().len(), 1, "the shared mailbox was read once, through Graph");
}

#[test]
fn add_shared_mailbox_waits_for_a_graph_job_that_brings_no_mail_down_without_calling_it_off() {
    let (mut app, owner, answers, jobs, _) = window();
    // The owner's run has its mail in and is on its calendar and contacts: short, and not
    // called off; the shared mailbox's run starts once it has ended.
    let (end, held) = mpsc::channel::<GraphResult>();
    app.graph_rx = Some((owner, true, false, held));
    add_in_dialog(&mut app, &answers, owner, SHARED, vec![inbox_read(SHARED)]);
    assert!(matches!(&app.dialog, Some(crate::dialogs::Dialog::SharedMailbox { checking: Some(_), error, .. }) if error.is_empty()));
    let shared = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED).map(|a| a.id).unwrap();
    app.shared_check_tick();
    assert!(!app.services.sync_cancel.load(Ordering::Relaxed) && jobs.lock().unwrap().is_empty());
    assert!(app.sync_queue.is_empty(), "nothing to put back in line: the job wasn't called off");
    end.send(GraphResult { account: owner, ..Default::default() }).unwrap();
    app.poll_graph();
    assert!(app.graph_rx.is_none());
    app.shared_check_tick();
    assert_eq!(jobs.lock().unwrap().iter().map(|(a, ..)| *a).collect::<Vec<_>>(), [shared]);
    app.poll_graph();
    assert!(app.dialog.is_none() && app.shared_checking.is_empty());
    assert!(app.session.mailbox.folder_by_server(shared, "in").is_some());
    no_imap();
}

#[test]
fn add_shared_mailbox_called_off_while_it_waits_starts_nothing() {
    let (mut app, owner, answers, jobs, _) = window();
    // A run that never ends by itself.
    let (_hold, held) = mpsc::channel::<GraphResult>();
    app.graph_rx = Some((owner, true, false, held));
    add_in_dialog(&mut app, &answers, owner, SHARED, Vec::new());
    let shared = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED).map(|a| a.id).unwrap();
    app.drop_shared_check(shared);
    app.graph_rx = None;
    app.shared_check_tick();
    assert!(jobs.lock().unwrap().is_empty() && app.session.mailbox.account(shared).is_none());
    no_imap();
}

#[test]
fn after_add_shared_mailbox_the_called_off_run_goes_on_before_the_rest_of_the_shared_mailbox() {
    let (mut app, owner, _, _, _) = window();
    // Both mailboxes are big: the host holds on to each run until it is called off, and the
    // shared mailbox's sends its folders first (#274).
    type Run = (mpsc::SyncSender<GraphMailResult>, mpsc::Sender<GraphResult>);
    let running: Arc<Mutex<Vec<(Id, Run)>>> = Default::default();
    let asked: Arc<Mutex<Vec<Id>>> = Default::default();
    let (keep, log) = (running.clone(), asked.clone());
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        log.lock().unwrap().push(req.account.id);
        let (tx, rx) = mpsc::channel();
        let pieces = req.mail_pieces.clone().unwrap();
        if req.account.email == SHARED {
            pieces.send(GraphMailResult { account: req.account.id, partial: true, ..inbox_read(SHARED) }).unwrap();
        }
        keep.lock().unwrap().push((req.account.id, (pieces, tx)));
        rx
    }));
    let stop = |app: &mut MailApp, whose: Id| {
        let at = running.lock().unwrap().iter().position(|(a, _)| *a == whose).unwrap();
        let (_, (pieces, answer)) = running.lock().unwrap().remove(at);
        drop(pieces);
        answer.send(GraphResult { account: whose, ..Default::default() }).unwrap();
        app.poll_graph();
        app.poll_graph();
        assert!(app.graph_rx.is_none());
    };
    // The owner pressed Send/Receive, then Add.
    app.execute("ui.sync", json!({"account": owner})).unwrap();
    let answers: Answers = Default::default();
    add_in_dialog(&mut app, &answers, owner, SHARED, Vec::new());
    assert!(app.services.sync_cancel.load(Ordering::Relaxed));
    // Put back in line as the owner asked for it: its errors are still shown.
    assert_eq!(app.sync_queue.iter().copied().collect::<Vec<_>>(), [(owner, false)]);
    stop(&mut app, owner);
    app.shared_check_tick();
    let shared = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED).map(|a| a.id).unwrap();
    // Its folders are in: kept, and the dialog closes.
    app.poll_graph();
    assert!(app.dialog.is_none() && app.shared_checking.is_empty());
    // The rest of the shared mailbox doesn't hold the owner's run up: it is called off in its
    // turn, and comes down after.
    assert!(app.services.sync_cancel.load(Ordering::Relaxed), "the shared mailbox's run is called off");
    assert_eq!(app.sync_queue.iter().copied().collect::<Vec<_>>(), [(owner, false), (shared, false)]);
    stop(&mut app, shared);
    assert!(app.session.mailbox.account(shared).is_some(), "kept");
    app.auto_sync();
    assert_eq!(*asked.lock().unwrap(), [owner, shared, owner], "the owner's run goes on first");
    assert!(!app.services.sync_cancel.load(Ordering::Relaxed));
    stop(&mut app, owner);
    app.auto_sync();
    assert_eq!(*asked.lock().unwrap(), [owner, shared, owner, shared], "then the rest of the shared mailbox");
    assert!(app.sync_queue.is_empty());
    no_imap();
}

#[test]
fn add_shared_mailbox_calls_off_a_sign_in_left_waiting_in_the_browser() {
    let (mut app, owner, answers, jobs, _) = window();
    // Another account's sign-in waits on a page in the browser, which the owner left (#274).
    let held: Arc<Mutex<Vec<(Arc<AtomicBool>, mpsc::Sender<Result<Tokens, String>>)>>> = Default::default();
    let keep = held.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        let (tx, rx) = mpsc::channel();
        keep.lock().unwrap().push((req.cancel.clone(), tx));
        rx
    }));
    let other = app.execute("account.add", json!({"email": "avery@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.execute("ui.sync", json!({"account": other})).unwrap();
    assert!(app.signing_in.is_some());
    add_in_dialog(&mut app, &answers, owner, SHARED, vec![inbox_read(SHARED)]);
    // Called off, and the shared mailbox is read at once rather than when the page times out.
    assert!(held.lock().unwrap()[0].0.load(Ordering::Relaxed), "the sign-in is called off");
    assert!(app.signing_in.is_none());
    // And the owner is told, in Send/Receive's problems, under the account it was for.
    let said = app.sync_problem_lines();
    assert!(said.iter().any(|l| l.contains("The sign-in to avery@example.org waiting in your browser was stopped")), "{said:?}");
    app.shared_check_tick();
    let shared = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED).map(|a| a.id).unwrap();
    assert_eq!(jobs.lock().unwrap().iter().map(|(a, ..)| *a).collect::<Vec<_>>(), [shared]);
    app.poll_graph();
    assert!(app.dialog.is_none() && app.shared_checking.is_empty());
    assert!(app.session.mailbox.folder_by_server(shared, "in").is_some());
    no_imap();
}

#[test]
fn add_shared_mailbox_waits_for_a_sign_in_being_renewed() {
    let (mut app, owner, answers, jobs, _) = window();
    // A sign-in renewed with a kept token: nobody is in the browser, and it is short.
    let (renewed, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.signing_in = Some(crate::SigningIn {
        graph: false,
        favorites: false,
        quiet: true,
        account: owner,
        shared: None,
        shared_send: false,
        shared_calendars: false,
        tasks: false,
        browser: false,
        opening: app.session.openings(),
        rx,
        cancel: cancel.clone(),
    });
    add_in_dialog(&mut app, &answers, owner, SHARED, vec![inbox_read(SHARED)]);
    app.shared_check_tick();
    assert!(!cancel.load(Ordering::Relaxed) && app.signing_in.is_some() && jobs.lock().unwrap().is_empty());
    drop(renewed);
    app.signing_in = None;
    app.shared_check_tick();
    assert_eq!(jobs.lock().unwrap().len(), 1);
    app.poll_graph();
    assert!(app.dialog.is_none() && app.shared_checking.is_empty());
    no_imap();
}

#[test]
fn a_shared_mailbox_microsoft_wont_open_shows_no_access_and_keeps_its_mail() {
    let (mut app, _, answers, _, _) = window();
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    answers.lock().unwrap().push_back((GraphResult::default(), vec![inbox_read(SHARED)]));
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    app.poll_graph();
    let inbox = app.session.mailbox.folder_by_server(shared, "in").unwrap();
    let mut kept = mailcraft_model::Message { folder: inbox, subject: "Kept".into(), ..Default::default() };
    kept.account = shared;
    let id = app.session.mailbox.add_message(kept).unwrap();
    // A change made in it here, which waits.
    app.execute("folder.new", json!({"parent": inbox, "name": "Ours"})).unwrap();
    // Access is taken away.
    let refused = GraphMailResult {
        email: SHARED.into(),
        refused: Some(mailcraft_model::sync::SharedRefusal::NoAccess),
        errors: vec!["Microsoft won't open this shared mailbox for you: you need Full Access to it.".into()],
        ..Default::default()
    };
    answers.lock().unwrap().push_back((GraphResult::default(), vec![refused]));
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    app.poll_graph();
    assert!(app.session.mailbox.account(shared).is_some() && app.session.mailbox.message(id).is_some(), "never removed by itself");
    assert!(app.shared_refused.contains_key(&shared));
    assert!(app.sync_problem_lines().iter().any(|l| l.contains("wait on this computer")), "{:?}", app.sync_problem_lines());
    // The folder pane says No access on its heading, in both themes.
    let mut h =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for theme in [crate::theme::Appearance::Light, crate::theme::Appearance::Dark] {
        h.state_mut().ui.theme = theme;
        h.run_steps(3);
    }
    let a = h.state().session.mailbox.account(shared).cloned().unwrap();
    assert!(crate::nav::heading_name(h.state(), &a).ends_with("No access"));
    no_imap();
}

#[test]
fn shared_mailboxes_are_offered_only_where_they_can_be_and_only_while_switched_on() {
    let (mut app, owner, _, _, _) = window();
    // A personal Microsoft account has none.
    if let Ok(a) = app.session.mailbox.account_mut(owner) {
        a.oauth = Some(mailcraft_model::OAuth { directory: mailcraft_model::Directory::Personal, ..Default::default() });
    }
    assert!(app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).is_err() || app.dialog.is_none());
    if let Ok(a) = app.session.mailbox.account_mut(owner) {
        a.oauth = Some(mailcraft_model::OAuth { directory: mailcraft_model::Directory::Organisation, ..Default::default() });
    }
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    assert!(app.dialog.is_some());
}

#[test]
fn removing_a_shared_mailbox_asks_first_and_names_what_waits() {
    let (mut app, _, _, _, _) = window();
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    let inbox = app.session.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    app.execute("folder.new", json!({"parent": inbox, "name": "Ours"})).unwrap();
    let ask = crate::dialogs::confirmation(&app, "account.removeSharedMailbox", &json!({"id": shared})).unwrap();
    assert!(ask.text.contains(SHARED) && ask.text.contains("hasn't reached the server"), "{}", ask.text);
    // Through the window's own run, it is asked and not removed until the yes.
    let _ = app.run("account.removeSharedMailbox", json!({"id": shared}));
    assert!(matches!(app.dialog, Some(crate::dialogs::Dialog::Confirm { .. })) && app.session.mailbox.account(shared).is_some());
}

#[test]
fn a_refused_sign_in_of_the_owner_is_said_and_the_mailbox_being_added_is_not_kept() {
    let (mut app, owner, answers, _, sign_ins) = window();
    answers.lock().unwrap().push_back((GraphResult { sign_in_refused: true, ..Default::default() }, Vec::new()));
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    if let Some(crate::dialogs::Dialog::SharedMailbox { address, .. }) = app.dialog.as_mut() {
        *address = SHARED.into();
    }
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let mut out = ctx.run_ui(egui::RawInput { events: vec![enter()], ..Default::default() }, |ui| crate::dialogs::show(&mut app, &ui.ctx().clone()));
    out.textures_delta.clear();
    app.poll_graph();
    assert!(app.session.mailbox.accounts.iter().all(|a| a.email != SHARED));
    assert!(matches!(&app.dialog, Some(crate::dialogs::Dialog::SharedMailbox { error, .. }) if error.contains("sign in again")));
    assert!(sign_ins.lock().unwrap().is_empty(), "nobody is sent to a page from here");
}

/// What each Graph job was asked, as JSON-ish facts: the account, whether it asked for Outlook's
/// Favorites or their permissions, the calendar or the directory, and its mail request.
type Asked = Arc<Mutex<Vec<(Id, bool, bool, bool, bool, Option<GraphMailRequest>)>>>;

/// [`window`] with a shared mailbox whose Inbox was read; the window, the owner, the shared
/// mailbox, and from then on what each Graph job is asked (each answered with nothing).
fn with_shared_inbox() -> (MailApp, Id, Id, Asked) {
    let (mut app, owner, answers, _, _) = window();
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    answers.lock().unwrap().push_back((GraphResult { account: shared, ..Default::default() }, vec![inbox_read(SHARED)]));
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    app.poll_graph();
    assert!(app.session.mailbox.folder_by_server(shared, "in").is_some());
    let asked: Asked = Default::default();
    let log = asked.clone();
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        log.lock().unwrap().push((req.account.id, req.favorites, req.favorites_scopes, req.calendar.is_some(), req.directory, req.mail.clone()));
        let (tx, rx) = mpsc::channel();
        tx.send(GraphResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    (app, owner, shared, asked)
}

#[test]
fn a_shared_mailboxs_folder_among_the_favorites_says_whose_it_is() {
    let (mut app, owner, shared, _) = with_shared_inbox();
    let theirs = app.session.mailbox.folder_by_server(shared, "in").unwrap();
    let mine = app.session.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap();
    app.execute("folder.favorite", json!({"id": theirs, "value": true})).unwrap();
    let mb = &app.session.mailbox;
    assert_eq!(crate::nav::folder_label(mb, mb.folder(theirs).unwrap()), format!("Inbox - {SHARED}"));
    assert_eq!(crate::nav::folder_label(mb, mb.folder(mine).unwrap()), "Inbox", "the owner's own are by their names");
    let listed = mailcraft_engine::cmd::folder::favorites(&app.session);
    assert!(listed.contains(&theirs) && listed.contains(&mine), "{listed:?}");
    // The pane draws it.
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        crate::nav::folder_pane(&mut app, ui);
    });
    out.textures_delta.clear();
    no_imap();
}

#[test]
fn favourites_searches_and_counts_in_a_shared_mailbox_ask_no_server_for_anything() {
    let (mut app, owner, shared, asked) = with_shared_inbox();
    let theirs = app.session.mailbox.folder_by_server(shared, "in").unwrap();
    app.execute("folder.favorite", json!({"id": theirs, "value": true})).unwrap();
    app.execute("view.search", json!({"query": "figures", "scope": "everywhere"})).unwrap();
    app.execute("folder.list", json!({})).unwrap();
    // Nothing was asked of any server for those.
    assert!(asked.lock().unwrap().is_empty());
    // The shared mailbox's next run is its mail alone, at its own address: no Favorites, no
    // permission for them, no calendar, no directory, and nothing to change.
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    app.poll_graph();
    // The owner's own run is about his mailbox alone.
    app.execute("ui.sync", json!({"account": owner})).unwrap();
    app.poll_graph();
    let asked = asked.lock().unwrap().clone();
    let theirs_run = asked.iter().find(|a| a.0 == shared).expect("the shared mailbox's run");
    assert_eq!((theirs_run.1, theirs_run.2, theirs_run.3, theirs_run.4), (false, false, false, false));
    let mail = theirs_run.5.clone().unwrap();
    assert_eq!((mail.account, mail.email.as_str(), mail.shared), (shared, SHARED, true));
    assert!(mail.changes.is_empty() && mail.messages.is_empty() && mail.drafts.is_empty() && mail.sends.is_empty(), "{mail:?}");
    let own = asked.iter().find(|a| a.0 == owner).expect("the owner's run");
    if let Some(mail) = &own.5 {
        assert!(!mail.shared && mail.account == owner && mail.email == OWNER);
        assert!(!serde_json::to_string(mail).unwrap().contains(SHARED), "nothing of the shared mailbox in the owner's run: {mail:?}");
    }
    no_imap();
}

/// What each sign-in was asked: for shared mailboxes, and for sending from them.
type SendSignIns = Arc<Mutex<Vec<(bool, bool)>>>;

/// [`window`] with a shared mailbox that has something waiting in its Outbox, and a sign-in that
/// answers `answer`; the shared mailbox, the message, and what sign-ins were asked.
fn sending(answer: Result<Tokens, String>) -> (MailApp, Id, Id, Id, Answers, Jobs, SendSignIns) {
    let (mut app, owner, answers, jobs, _) = window();
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    app.session.view.folder = app.session.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    app.execute("mail.new", json!({"to": "kit@example.net", "subject": "Rota"})).unwrap();
    let id = app.execute("compose.send", json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(app.session.mailbox.message(id).map(|m| (m.account, m.from.email.clone())), Some((shared, SHARED.to_string())));
    let asked: SendSignIns = Default::default();
    let log = asked.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.shared, req.shared_send));
        let (tx, rx) = mpsc::channel();
        tx.send(answer.clone()).unwrap();
        rx
    }));
    (app, owner, shared, id, answers, jobs, asked)
}

/// The Send/Receive of `shared` the owner asked for, answered: sending from shared mailboxes
/// isn't allowed yet.
fn not_allowed_yet(app: &mut MailApp, shared: Id, answers: &Answers, quiet: bool) {
    answers.lock().unwrap().push_back((GraphResult { account: shared, send_consent_needed: true, ..Default::default() }, Vec::new()));
    app.execute("ui.sync", json!({"account": shared, "quiet": quiet})).unwrap();
    app.poll_graph();
}

#[test]
fn the_first_send_from_a_shared_mailbox_asks_the_owner_first_then_microsofts_page() {
    let (mut app, owner, shared, id, answers, jobs, asked) = sending(Ok(Tokens { email: OWNER.into(), ..tokens("send-access", "ref-4") }));
    not_allowed_yet(&mut app, shared, &answers, false);
    // The run took what waits to the host, which found it may not go yet.
    let sends = jobs.lock().unwrap()[0].2.as_ref().map(|m| m.sends.len());
    assert_eq!(sends, Some(1));
    // The window asks first; nothing is held, and no page has come up.
    let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = &app.dialog else { panic!("the owner is asked first") };
    assert_eq!((command.as_str(), params["account"].as_u64()), ("ui.allowSharedSend", Some(shared)));
    assert!(ask.text.contains(SHARED) && ask.text.contains("nothing is sent from your own address"), "{}", ask.text);
    assert!(asked.lock().unwrap().is_empty());
    assert!(app.session.mailbox.message(id).is_some_and(|m| m.held.is_empty()));
    // Go ahead: Microsoft's page, for sending from shared mailboxes, by itself.
    app.dialog = None;
    assert_eq!(app.run("ui.allowSharedSend", json!({"account": shared, "confirmed": true})).unwrap()["account"], owner);
    assert_eq!(*asked.lock().unwrap(), [(true, true)]);
    // Allowed: the shared mailbox runs again at once, with what came back.
    app.poll_sign_in();
    assert_eq!(app.graph_tokens.get(&shared).map(|t| t.access.as_str()), Some("send-access"));
    assert_eq!(jobs.lock().unwrap().len(), 2);
    assert!(app.session.mailbox.message(id).is_some_and(|m| m.held.is_empty()));
    no_imap();
}

#[test]
fn a_no_on_microsofts_page_holds_the_mail_with_why_and_an_administrators_approval_has_its_page() {
    use crate::favorites_sync::NEEDS_APPROVAL;
    // Refused by the owner (or the page left).
    let (mut app, _, shared, id, answers, _, _) = sending(Err("access_denied".into()));
    not_allowed_yet(&mut app, shared, &answers, false);
    app.dialog = None;
    app.run("ui.allowSharedSend", json!({"account": shared, "confirmed": true})).unwrap();
    app.poll_sign_in();
    let held = app.session.mailbox.message(id).map(|m| m.held.clone()).unwrap_or_default();
    assert!(held.starts_with("It wasn't sent") && held.contains("wasn't allowed"), "{held}");
    assert_eq!(app.session.mailbox.message(id).map(|m| m.account), Some(shared), "still in its own Outbox");
    // An administrator has to approve first: held with that, and the page that says what to do.
    let (mut app, owner, shared, id, answers, _, _) = sending(Err(NEEDS_APPROVAL.into()));
    not_allowed_yet(&mut app, shared, &answers, false);
    app.dialog = None;
    app.run("ui.allowSharedSend", json!({"account": shared, "confirmed": true})).unwrap();
    app.poll_sign_in();
    let held = app.session.mailbox.message(id).map(|m| m.held.clone()).unwrap_or_default();
    assert!(held.contains("administrator has to approve"), "{held}");
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("account"), "the approval page");
    let _ = owner;
    no_imap();
}

#[test]
fn a_run_that_started_by_itself_never_asks_and_the_mail_waits() {
    let (mut app, _, shared, id, answers, _, asked) = sending(Ok(tokens("send-access", "ref-4")));
    not_allowed_yet(&mut app, shared, &answers, true);
    assert!(app.dialog.is_none() && asked.lock().unwrap().is_empty());
    assert!(app.session.mailbox.message(id).is_some_and(|m| m.held.is_empty()), "it waits, not held");
    assert!(app.session.sync_status.contains("needs your permission"), "{}", app.session.sync_status);
    // Only a shared mailbox's.
    assert!(app.run("ui.allowSharedSend", json!({"account": 1, "confirmed": true})).is_err());
    assert!(app.run("ui.allowSharedSend", json!({"confirmed": true})).is_err());
    no_imap();
}

/// One look of the window's own Send/Receive, as each frame has it, with the few seconds between
/// looks gone by: what waits to go out may start a run, and a run that is under way is finished.
fn frame(app: &mut MailApp) {
    app.push_checked = 0.0;
    app.auto_sync();
    app.auto_sync();
    app.poll_graph();
}

/// Runs that start by themselves come only every few minutes here: what is left is the window's
/// look for something that waits to go out.
fn by_itself_only_to_push(app: &mut MailApp) {
    app.session.prefs.sync_minutes = 0;
    app.auto_sync = true;
}

#[test]
fn mail_held_back_for_permission_doesnt_start_run_after_run() {
    let (mut app, _, shared, id, answers, jobs, asked) = sending(Ok(tokens("send-access", "ref-4")));
    by_itself_only_to_push(&mut app);
    for _ in 0..6 {
        answers.lock().unwrap().push_back((GraphResult { account: shared, send_consent_needed: true, ..Default::default() }, Vec::new()));
    }
    for _ in 0..6 {
        frame(&mut app);
    }
    // One run took it to the host, which found it may not go yet; nothing started again for it.
    let runs: Vec<usize> = jobs.lock().unwrap().iter().map(|j| j.2.as_ref().map_or(0, |m| m.sends.len())).collect();
    assert_eq!(runs, [1], "one run, then none until something changes");
    assert!(app.dialog.is_none() && asked.lock().unwrap().is_empty(), "nobody asked for that run");
    assert!(app.session.mailbox.message(id).is_some_and(|m| m.held.is_empty()), "it waits, not held");
    // A run that starts by itself later (every few minutes) doesn't ask Microsoft about it again.
    app.execute("ui.sync", json!({"account": shared, "quiet": true})).unwrap();
    app.poll_graph();
    assert_eq!(jobs.lock().unwrap().last().and_then(|j| j.2.as_ref()).map(|m| m.sends.len()), Some(0));
    for _ in 0..3 {
        frame(&mut app);
    }
    assert_eq!(jobs.lock().unwrap().len(), 2);
    no_imap();
}

#[test]
fn send_receive_asks_about_mail_held_back_for_permission() {
    let (mut app, _, shared, _, answers, jobs, asked) = sending(Ok(tokens("send-access", "ref-4")));
    by_itself_only_to_push(&mut app);
    answers.lock().unwrap().push_back((GraphResult { account: shared, send_consent_needed: true, ..Default::default() }, Vec::new()));
    frame(&mut app);
    assert!(app.dialog.is_none());
    // The owner's Send/Receive of every account: the owner's own, then the shared mailbox, whose
    // mail goes to the host again, which still may not send it: the window asks.
    answers.lock().unwrap().push_back((GraphResult::default(), Vec::new()));
    answers.lock().unwrap().push_back((GraphResult { account: shared, send_consent_needed: true, ..Default::default() }, Vec::new()));
    app.run("ui.sync", json!({})).unwrap();
    for _ in 0..4 {
        frame(&mut app);
    }
    let last = jobs.lock().unwrap().last().cloned().unwrap();
    assert_eq!((last.0, last.2.map(|m| m.sends.len())), (shared, Some(1)));
    let Some(crate::dialogs::Dialog::Confirm { command, .. }) = &app.dialog else { panic!("the owner is asked") };
    assert_eq!(command, "ui.allowSharedSend");
    assert!(asked.lock().unwrap().is_empty(), "no page before the owner says go ahead");
    no_imap();
}

#[test]
fn pressing_send_in_a_shared_mailbox_asks_about_sending_from_it() {
    let (mut app, _, shared, _, answers, jobs, _) = sending(Ok(tokens("send-access", "ref-4")));
    by_itself_only_to_push(&mut app);
    // The first send the owner pressed: the run that takes it out finds it may not go yet, and
    // the window asks, though that run started by itself.
    app.session.view.folder = app.session.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    app.execute("mail.new", json!({"to": "kit@example.net", "subject": "Second"})).unwrap();
    app.run("compose.send", json!({})).unwrap();
    answers.lock().unwrap().push_back((GraphResult { account: shared, send_consent_needed: true, ..Default::default() }, Vec::new()));
    frame(&mut app);
    assert_eq!(jobs.lock().unwrap().len(), 1);
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("confirm"));
    // Not now: it waits, and nothing starts again for it.
    app.execute("ui.closeDialog", json!({})).unwrap();
    for _ in 0..3 {
        frame(&mut app);
    }
    assert_eq!(jobs.lock().unwrap().len(), 1);
    // Another one pressed while sending from it is known not to be allowed yet: asked at once,
    // with no run for it.
    app.execute("mail.new", json!({"to": "kit@example.net", "subject": "Third"})).unwrap();
    app.run("compose.send", json!({})).unwrap();
    let Some(crate::dialogs::Dialog::Confirm { command, params, .. }) = &app.dialog else { panic!("asked at once") };
    assert_eq!((command.as_str(), params["account"].as_u64()), ("ui.allowSharedSend", Some(shared)));
    frame(&mut app);
    assert_eq!(jobs.lock().unwrap().len(), 1);
    no_imap();
}

/// A Graph host that holds on to every run until the test ends it: what it was asked for, and
/// each run's pipes, by account.
type HeldRun = (mpsc::SyncSender<GraphMailResult>, mpsc::Sender<GraphResult>);
type Held = Arc<Mutex<Vec<(Id, HeldRun)>>>;

fn holding_graph(app: &mut MailApp) -> (Held, Arc<Mutex<Vec<Id>>>) {
    let running: Held = Default::default();
    let asked: Arc<Mutex<Vec<Id>>> = Default::default();
    let (keep, log) = (running.clone(), asked.clone());
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        log.lock().unwrap().push(req.account.id);
        let (tx, rx) = mpsc::channel();
        keep.lock().unwrap().push((req.account.id, (req.mail_pieces.clone().unwrap(), tx)));
        rx
    }));
    (running, asked)
}

/// A look of the window's, as each frame has it, with the few seconds between looks gone by.
fn look(app: &mut MailApp) {
    app.last_auto_sync = crate::now_ms();
    app.push_checked = 0.0;
    app.poll_graph();
    app.auto_sync();
}

/// What the owner does while a long Graph run is still bringing mail down doesn't wait for the
/// run to end (#274): the run is called off at its next safe place, and goes on at once after,
/// as it was asked for, with the change to tell first. Its own mailbox's and a shared one's.
#[test]
fn a_change_made_during_a_long_graph_download_goes_out_without_waiting_for_it() {
    for (address, quiet) in [(OWNER, false), (SHARED, true)] {
        let (mut app, owner, _, _, _) = window();
        by_itself_only_to_push(&mut app);
        let (running, asked) = holding_graph(&mut app);
        let whose = if address == OWNER {
            owner
        } else {
            app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap()
        };
        app.execute("ui.sync", json!({"account": whose, "quiet": quiet})).unwrap();
        // Its folders are in, which tells the server what there was to tell; mail comes down.
        let pieces = running.lock().unwrap()[0].1.0.clone();
        pieces.send(GraphMailResult { account: whose, partial: true, ..inbox_read(address) }).unwrap();
        drop(pieces);
        look(&mut app);
        assert!(app.syncing() && !app.services.sync_cancel.load(Ordering::Relaxed), "nothing waits: the run goes on");
        // The owner makes a folder: at the next look the run is called off.
        let inbox = app.session.mailbox.folder_by_server(whose, "in").unwrap();
        app.execute("folder.new", json!({"parent": inbox, "name": "Ours"})).unwrap();
        look(&mut app);
        assert!(app.services.sync_cancel.load(Ordering::Relaxed), "{address}: called off");
        assert!(app.syncing(), "until the host says it stopped");
        look(&mut app);
        assert_eq!(asked.lock().unwrap().len(), 1, "one run at a time");
        // The host stops; the run goes on at once, as it was asked for.
        let (_, (pieces, answer)) = running.lock().unwrap().remove(0);
        drop(pieces);
        answer.send(GraphResult { account: whose, ..Default::default() }).unwrap();
        app.poll_graph();
        app.poll_graph();
        assert!(app.graph_rx.is_none());
        assert_eq!(app.sync_queue.front(), Some(&(whose, quiet)), "{address}: put back as it was");
        app.auto_sync();
        assert_eq!(*asked.lock().unwrap(), [whose, whose], "{address}: the next run starts at once");
        assert!(!app.services.sync_cancel.load(Ordering::Relaxed));
        // A run the owner stops is not started again behind their back.
        app.execute("folder.new", json!({"parent": inbox, "name": "Ours too"})).unwrap();
        let pieces = running.lock().unwrap()[0].1.0.clone();
        pieces.send(GraphMailResult { account: whose, partial: true, ..inbox_read(address) }).unwrap();
        drop(pieces);
        look(&mut app);
        // (Called off for the change first, then stopped by the owner.)
        app.execute("folder.new", json!({"parent": inbox, "name": "And another"})).unwrap();
        look(&mut app);
        assert!(app.services.sync_cancel.load(Ordering::Relaxed));
        app.execute("ui.cancelSync", json!({})).unwrap();
        let (_, (pieces, answer)) = running.lock().unwrap().remove(0);
        drop(pieces);
        answer.send(GraphResult { account: whose, ..Default::default() }).unwrap();
        look(&mut app);
        look(&mut app);
        assert!(app.sync_queue.is_empty() && app.graph_rx.is_none(), "{address}: stopped is stopped");
    }
    no_imap();
}
