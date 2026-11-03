// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The page that offers the switch to Microsoft Graph, and what the window does with the answer
//! (R3.7). Nothing here leaves this machine: the hosts are stand-ins, the mail invented.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};
use mailcraft_model::{AccountKind, FolderKind, GraphSwitch, Id, Message};
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

fn says(h: &Harness<'static, MailApp>, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
}

/// What the status bar says now.
fn status(h: &Harness<'static, MailApp>) -> String {
    h.state().status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default()
}

fn click(h: &mut Harness<'static, MailApp>, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run_steps(4);
}

fn page(h: &Harness<'static, MailApp>) -> Option<&'static str> {
    h.state().dialog.as_ref().map(|d| d.name())
}

/// The sample mailbox, kept in a file of the test's own, with a Microsoft account on IMAP that
/// has a token at hand (and no refresh token: nothing of Graph's is asked before the switch).
fn app(name: &str) -> (MailApp, Id, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("mailcraft-r37-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    s.path = Some(dir.join("mailbox.mailcraft"));
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.offer_graph_switch = true;
    let added = app.execute(
        "account.add",
        json!({"email": "owner@contoso.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}, "outgoing": {"host": "smtp.office365.com", "port": 587, "security": "starttls"}}),
    );
    let account = added.unwrap()["id"].as_u64().unwrap();
    let tokens = Tokens { access: "imap-token".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() };
    app.tokens.insert(account, tokens);
    (app, account, dir)
}

#[test]
fn the_page_is_shown_once_and_later_keeps_the_account_on_imap() {
    let (app, account, dir) = app("later");
    let mut h = window(app);
    assert_eq!(page(&h), Some("graphSwitch"));
    for line in super::wording("owner@contoso.example") {
        assert!(says(&h, line.trim_start_matches(['•', ' '])), "{line}");
    }
    assert!(says(&h, "Kept from owner@contoso.example"));
    click(&mut h, "Later");
    assert_eq!(page(&h), None);
    let a = h.state().session.mailbox.account(account).unwrap();
    assert_eq!((a.kind, a.graph_switch), (AccountKind::Imap, GraphSwitch::Later));
    assert!(status(&h).contains("stays on IMAP for now"), "{}", status(&h));
    // Not again: not in this run, nor in the next one (the answer is kept with the account).
    h.run_steps(4);
    assert_eq!(page(&h), None);
    let session = std::mem::replace(&mut h.state_mut().session, Session::empty());
    let mut again = MailApp::new(session, Services::default());
    again.autosave = false;
    again.offer_graph_switch = true;
    let h = window(again);
    assert_eq!(page(&h), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_page_closed_without_an_answer_comes_back_the_next_time() {
    let (app, account, dir) = app("closed");
    let mut h = window(app);
    assert_eq!(page(&h), Some("graphSwitch"));
    h.state_mut().dialog = None;
    h.run_steps(4);
    assert_eq!(page(&h), None, "not again in this run");
    assert_eq!(h.state().session.mailbox.account(account).unwrap().graph_switch, GraphSwitch::NotAsked);
    // And from command search, whenever the owner likes.
    h.state_mut().execute("ui.graphSwitch", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(page(&h), Some("graphSwitch"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switch_without_them_switches_at_once_and_says_what_is_in_the_backup() {
    let (mut app, account, dir) = app("without");
    // A flag changed here that the IMAP server, which can't be reached, hasn't been told.
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let f = app.session.mailbox.folder_mut(inbox).unwrap();
    (f.remote, f.uid_validity) = ("INBOX".into(), 41);
    let message = Message {
        folder: inbox,
        subject: "Figures".into(),
        uid: 7,
        synced: Some(mailcraft_model::sync::RemoteFlags { uid: 7, ..Default::default() }),
        ..Default::default()
    };
    let message = app.session.mailbox.add_message(message).unwrap();
    app.session.mailbox.message_mut(message).unwrap().flag = mailcraft_model::FlagStatus::Flagged;
    app.session.mailbox.account_mut(account).unwrap().graph_switch = GraphSwitch::Switching;
    app.services.sync = Some(Box::new(|req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, errors: vec!["Couldn't connect to outlook.office365.com".into()], ..Default::default() })
            .unwrap();
        rx
    }));
    let mut h = window(app);
    h.state_mut().execute("ui.sync", json!({"account": account})).unwrap();
    h.run_steps(6);
    assert_eq!(page(&h), Some("graphSwitch"));
    assert!(says(&h, "1 change waiting for the IMAP server won't reach it; it stays in the backup copy."));
    click(&mut h, "Switch without Them");
    assert_eq!(page(&h), None);
    assert_eq!(h.state().session.mailbox.account(account).unwrap().kind, AccountKind::Microsoft);
    let backup = dir.join("mailbox (backup 2026-10-14).mailcraft");
    let kept = mailcraft_engine::io::open_path(&backup).unwrap();
    assert_eq!(kept.message(message).map(|m| m.flag), Some(mailcraft_model::FlagStatus::Flagged));
    assert!(status(&h).contains("1 change that didn't reach the IMAP server is in the copy"), "{}", status(&h));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switch_sends_what_waits_first_waits_while_offline_and_then_switches() {
    let (mut app, account, dir) = app("switch");
    // Something waits for the IMAP server: a message in the Outbox.
    let outbox = app.session.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let waiting =
        Message { folder: outbox, subject: "Minutes".into(), to: vec![mailcraft_model::Address::new("", "kit@example.org")], ..Default::default() };
    let waiting = app.session.mailbox.add_message(waiting).unwrap();
    // The IMAP host: offline until told otherwise; online, it sends what it was given.
    let offline = Arc::new(AtomicBool::new(true));
    let runs: Arc<Mutex<usize>> = Default::default();
    let (offline2, runs2) = (offline.clone(), runs.clone());
    app.services.sync = Some(Box::new(move |req: SyncRequest| {
        *runs2.lock().unwrap() += 1;
        let (tx, rx) = mpsc::channel();
        let result = if offline2.load(Ordering::Relaxed) {
            SyncResult { account: req.account.id, errors: vec!["Couldn't connect to outlook.office365.com".into()], ..Default::default() }
        } else {
            SyncResult { account: req.account.id, sent: req.outbox.iter().map(|m| m.id).collect(), ..Default::default() }
        };
        tx.send(result).unwrap();
        rx
    }));
    let mut h = window(app);
    assert_eq!(page(&h), Some("graphSwitch"));
    click(&mut h, "Switch");
    // The Send/Receive went first, and couldn't reach the server: the switch waits.
    h.run_steps(6);
    assert_eq!(*runs.lock().unwrap(), 1);
    let a = h.state().session.mailbox.account(account).unwrap();
    assert_eq!((a.kind, a.graph_switch), (AccountKind::Imap, GraphSwitch::Switching));
    assert!(status(&h).contains("waits until MailCraft can reach the server"), "{}", status(&h));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no backup yet");
    // And it offers to switch without what waits (the Outbox goes through Graph: nothing is left).
    assert_eq!(page(&h), Some("graphSwitch"));
    for line in super::waiting_wording("owner@contoso.example", 0) {
        assert!(says(&h, &line), "{line}");
    }
    click(&mut h, "Keep Waiting");
    assert_eq!(page(&h), None);
    assert_eq!(h.state().session.mailbox.account(account).unwrap().graph_switch, GraphSwitch::Switching);
    // Online again: the next Send/Receive sends it, and the switch goes ahead by itself.
    offline.store(false, Ordering::Relaxed);
    h.state_mut().execute("ui.sync", json!({"account": account})).unwrap();
    h.run_steps(8);
    assert!(h.state().session.mailbox.message(waiting).is_none_or(|m| m.folder != outbox), "sent");
    let a = h.state().session.mailbox.account(account).unwrap();
    assert_eq!((a.kind, a.incoming.is_none()), (AccountKind::Microsoft, true));
    let backup = dir.join("mailbox (backup 2026-10-14).mailcraft");
    assert!(backup.exists());
    assert!(status(&h).contains("now uses Microsoft Graph"), "{}", status(&h));
    let _ = std::fs::remove_dir_all(&dir);
}

/// What the log has said, for the tests that look at it (one logger for the whole test run).
pub(crate) fn logged() -> &'static Mutex<Vec<String>> {
    static LINES: std::sync::OnceLock<Mutex<Vec<String>>> = std::sync::OnceLock::new();
    struct Kept;
    impl log::Log for Kept {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Info
        }
        fn log(&self, r: &log::Record<'_>) {
            if let Some(lines) = LINES.get() {
                lines.lock().unwrap().push(format!("{} {}", r.level(), r.args()));
            }
        }
        fn flush(&self) {}
    }
    static KEPT: Kept = Kept;
    let lines = LINES.get_or_init(|| Mutex::new(Vec::new()));
    if log::set_logger(&KEPT).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    lines
}

/// An account already kept in step through Graph, whose Graph runs all answer `answer`; the
/// sign-ins asked for are kept in `asked`, and answered with tokens.
fn on_graph(name: &str, answer: fn() -> crate::GraphResult) -> (MailApp, Id, Arc<Mutex<Vec<crate::SignInRequest>>>, Arc<Mutex<Vec<bool>>>) {
    let (mut app, account, _) = app(name);
    app.offer_graph_switch = false;
    app.dialog = None;
    // Only the runs the tests start (no round of Send/Receive by itself).
    app.auto_sync = false;
    app.session.mailbox.account_mut(account).unwrap().kind = AccountKind::Microsoft;
    let refresh = Tokens { access: String::new(), refresh: "kept-refresh".into(), expires_at: 0, ..Default::default() };
    app.tokens.insert(account, refresh);
    app.services.sync = Some(Box::new(|_req: SyncRequest| mpsc::channel().1));
    let runs = Arc::new(Mutex::new(Vec::new()));
    let seen = runs.clone();
    app.services.graph = Some(Box::new(move |req: crate::GraphRequest| {
        seen.lock().unwrap().push(req.mail.is_some());
        let (tx, rx) = mpsc::channel();
        // The job's mail pieces end with it.
        drop(req.mail_pieces);
        tx.send(crate::GraphResult { account: req.account.id, ..answer() }).unwrap();
        rx
    }));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let kept = asked.clone();
    // Still in the browser: the sender is kept, so the page waits until the test answers.
    let browsers = Mutex::new(Vec::new());
    app.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
        kept.lock().unwrap().push(req);
        let (tx, rx) = mpsc::channel();
        browsers.lock().unwrap().push(tx);
        rx
    }));
    (app, account, asked, runs)
}

fn consent() -> crate::GraphResult {
    crate::GraphResult { consent_needed: true, ..Default::default() }
}

fn refused() -> crate::GraphResult {
    crate::GraphResult { sign_in_refused: true, ..Default::default() }
}

/// After the switch, a Send/Receive whose token Microsoft won't give for the mail (permission
/// needed, or the sign-in refused) asks the owner to sign in for it, never ends in silence with
/// "Sending and receiving…" left behind, and says so in the log (#124).
#[test]
fn a_graph_run_without_a_token_for_the_mail_asks_the_owner_to_sign_in() {
    let lines = logged();
    for (name, answer) in [("consent", consent as fn() -> crate::GraphResult), ("refused", refused)] {
        let (app, account, asked, runs) = on_graph(&format!("124-{name}"), answer);
        let mut h = window(app);
        h.state_mut().run("ui.sync", json!({})).unwrap();
        h.run_steps(6);
        assert_eq!(runs.lock().unwrap().as_slice(), [true], "{name}: one mail run");
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1, "{name}: a sign-in is asked for");
        assert!(asked[0].graph && asked[0].mail_write && asked[0].account == account, "{name}");
        assert_ne!(h.state().session.sync_status, "Sending and receiving…", "{name}");
        assert!(h.state().signing_in(), "{name}");
    }
    let lines = lines.lock().unwrap();
    assert!(
        lines.iter().any(|l| l.starts_with("WARN graph mail, account") && l.contains("needs permission for the mail; asking the owner")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("the sign-in was refused; asking the owner")), "{lines:?}");
    // Nothing of the owner in it.
    assert!(lines.iter().all(|l| !l.contains("owner@contoso.example") && !l.contains("kept-refresh")), "{lines:?}");
}

/// The same with nobody asking (Send/Receive by itself): no page is opened; the owner is told
/// what to do.
#[test]
fn a_quiet_graph_run_without_a_token_for_the_mail_says_what_to_do() {
    let (app, _, asked, _) = on_graph("124-quiet", consent);
    let mut h = window(app);
    h.state_mut().run("ui.sync", json!({"quiet": true})).unwrap();
    h.run_steps(6);
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(status(&h), "Sign in to Microsoft again to get the mail of owner@contoso.example: press Send/Receive");
    assert_ne!(h.state().session.sync_status, "Sending and receiving…");
    assert!(!h.state().syncing());
}

/// Once the owner has signed in, the mail run goes ahead with the new sign-in; when Microsoft
/// still says no, the owner is told, and not sent to its page again and again.
#[test]
fn after_signing_in_the_mail_run_goes_ahead() {
    let (app, account, asked, runs) = on_graph("124-after", consent);
    let mut h = window(app);
    h.state_mut().run("ui.sync", json!({})).unwrap();
    h.run_steps(6);
    assert_eq!(asked.lock().unwrap().len(), 1);
    // The browser comes back with tokens: the window takes them, and runs the mail again.
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(Tokens {
        access: "graph-token".into(),
        refresh: "new-refresh".into(),
        expires_at: mailcraft_time::now() + 3600,
        ..Default::default()
    }))
    .unwrap();
    h.state_mut().signing_in.as_mut().unwrap().rx = rx;
    h.run_steps(6);
    assert_eq!(runs.lock().unwrap().as_slice(), [true, true]);
    assert!(h.state().graph_tokens.contains_key(&account));
    // (The stand-in says no to every run.)
    assert_eq!(asked.lock().unwrap().len(), 1, "no second trip to Microsoft's page");
    assert!(!h.state().signing_in());
    assert!(status(&h).starts_with("Signed in, but Microsoft still won't let MailCraft read the mail of owner@contoso.example"), "{}", status(&h));
    assert_ne!(h.state().session.sync_status, "Sending and receiving…");
    // The owner's own Send/Receive after that asks once more.
    h.state_mut().run("ui.sync", json!({})).unwrap();
    h.run_steps(6);
    assert_eq!(asked.lock().unwrap().len(), 2);
    assert!(!status(&h).starts_with("Signed in, but"), "what the last run said is over: {}", status(&h));
}

/// A host that writes the copy in the background: the test says when it is done, and how.
type Done = Arc<Mutex<Option<(std::path::PathBuf, mailcraft_model::Mailbox, mpsc::Sender<Result<(), String>>)>>>;

fn copying_host(app: &mut MailApp) -> Done {
    let pending: Done = Arc::default();
    let kept = pending.clone();
    app.services.write_copy = Some(Box::new(move |path, mailbox| {
        let (tx, rx) = mpsc::channel();
        *kept.lock().unwrap() = Some((path, mailbox, tx));
        rx
    }));
    pending
}

/// While the copy of the mailbox is written (off the window's thread), the page says so with a
/// spinner and nothing has changed; the switch is made once the copy is on disk (#118).
#[test]
fn the_page_says_the_copy_is_being_saved_and_the_switch_waits_for_it() {
    let (mut app, account, dir) = app("copying");
    let pending = copying_host(&mut app);
    let mut h = window(app);
    click(&mut h, "Switch");
    assert_eq!(page(&h), Some("graphSwitch"));
    assert!(says(&h, "Saving a copy of your mailbox before the switch…"));
    assert!(h.state().session.mailbox.account(account).unwrap().on_imap_with_microsoft(), "nothing changes before the copy is saved");
    assert!(h.state_mut().switch_to_graph(account, "switch").is_err(), "one copy at a time");
    h.run_steps(4);
    assert!(says(&h, "Saving a copy of your mailbox before the switch…"));
    // The host writes it, and says so.
    let (path, mailbox, tx) = pending.lock().unwrap().take().unwrap();
    assert_eq!(path, dir.join("mailbox (backup 2026-10-14).mailcraft"));
    assert_eq!(mailbox.account(account).map(|a| a.graph_switch), Some(GraphSwitch::NotAsked));
    mailcraft_engine::store::save_single(&path, &mailbox).unwrap();
    tx.send(Ok(())).unwrap();
    h.run_steps(6);
    assert_eq!(page(&h), None);
    assert_eq!(h.state().session.mailbox.account(account).unwrap().kind, AccountKind::Microsoft);
    assert!(status(&h).contains("mailbox (backup 2026-10-14).mailcraft"), "{}", status(&h));
    // The engine took that copy and wrote no other.
    assert!(!dir.join("mailbox (backup 2026-10-14, 2).mailcraft").exists());
    assert!(h.state().session.graph_switch_backup.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A copy that can't be written changes nothing: the account stays on IMAP, and the status bar
/// says why.
#[test]
fn a_copy_that_cant_be_written_leaves_the_account_on_imap() {
    let (mut app, account, dir) = app("copy-fails");
    let pending = copying_host(&mut app);
    let mut h = window(app);
    click(&mut h, "Switch");
    let (_, _, tx) = pending.lock().unwrap().take().unwrap();
    tx.send(Err("No space left on device".into())).unwrap();
    h.run_steps(6);
    assert_eq!(page(&h), None);
    let a = h.state().session.mailbox.account(account).unwrap();
    assert!(a.on_imap_with_microsoft());
    assert_ne!(a.graph_switch, GraphSwitch::Switching);
    assert!(status(&h).contains("couldn't be written, so nothing was changed (No space left on device)"), "{}", status(&h));
    // A host that gave up says as much.
    let pending = copying_host(h.state_mut());
    h.state_mut().switch_to_graph(account, "switch").unwrap();
    drop(pending.lock().unwrap().take());
    h.run_steps(6);
    assert!(h.state().session.mailbox.account(account).unwrap().on_imap_with_microsoft());
    assert!(status(&h).contains("stopped unexpectedly"), "{}", status(&h));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A run whose mail came a piece at a time, as every run of the desktop host's does, ended
/// without a word in the log that the mail didn't come: it did, before the job's own answer.
#[test]
fn a_run_whose_mail_came_in_pieces_doesnt_say_it_ended_without_the_mail() {
    let lines = logged();
    let (app, account, _, runs) = on_graph("streamed", crate::GraphResult::default);
    let mut h = window(app);
    h.state_mut().services.graph = Some(Box::new(move |req: crate::GraphRequest| {
        let (tx, rx) = mpsc::channel();
        if let Some(to) = &req.mail_pieces {
            let piece = mailcraft_model::sync::GraphMailResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() };
            to.send(piece).unwrap();
        }
        drop(req.mail_pieces);
        tx.send(crate::GraphResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    h.state_mut().run("ui.sync", json!({})).unwrap();
    h.run_steps(6);
    assert!(runs.lock().unwrap().is_empty(), "the test's own host answered");
    assert!(!h.state().syncing());
    assert_ne!(h.state().session.sync_status, "Sending and receiving…");
    let said = format!("graph mail, account {account}: the run ended without the mail");
    assert!(lines.lock().unwrap().iter().all(|l| !l.contains(&said)), "{:?}", lines.lock().unwrap());
}

/// The mailbox of `app`, built the same way again: an account of the same id, in the same state.
fn same_mailbox_again() -> mailcraft_model::Mailbox {
    let (other, _, _) = app("again");
    other.session.mailbox.clone()
}

#[test]
fn a_copy_written_for_a_mailbox_that_was_replaced_switches_nobody() {
    let (mut app, account, dir) = app("swap-copy");
    let (tx, rx) = mpsc::channel();
    let rx = Mutex::new(Some(rx));
    app.services.write_copy = Some(Box::new(move |_, _| rx.lock().unwrap().take().unwrap_or_else(|| mpsc::channel().1)));
    app.switch_to_graph(account, "switchWithout").unwrap();
    assert!(app.graph_switch_copy.is_some());
    // Another file is opened, with an account of the same id that is on IMAP with Microsoft.
    app.session.replace_mailbox(same_mailbox_again());
    app.drop_secrets_if_replaced();
    tx.send(Ok(())).ok();
    app.poll_graph_switch_copy();
    assert!(app.graph_switch_copy.is_none() && app.session.graph_switch_backup.is_none());
    assert!(app.session.mailbox.account(account).is_some_and(|a| a.on_imap_with_microsoft()), "switched without being asked");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_copy_that_finishes_after_a_swap_the_window_has_not_noticed_yet_is_dropped() {
    let (mut app, account, dir) = app("swap-late");
    let (tx, rx) = mpsc::channel();
    let rx = Mutex::new(Some(rx));
    app.services.write_copy = Some(Box::new(move |_, _| rx.lock().unwrap().take().unwrap_or_else(|| mpsc::channel().1)));
    app.switch_to_graph(account, "switchWithout").unwrap();
    app.session.replace_mailbox(same_mailbox_again());
    tx.send(Ok(())).unwrap();
    app.poll_graph_switch_copy();
    assert!(app.session.mailbox.account(account).is_some_and(|a| a.on_imap_with_microsoft()), "switched without being asked");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_mailbox_waiting_for_its_servers_to_be_confirmed_starts_no_switch() {
    let (mut app, account, dir) = app("unconfirmed");
    if let Some(a) = app.session.mailbox.accounts.iter_mut().find(|a| a.id == account) {
        a.graph_switch = GraphSwitch::Switching;
    }
    app.unconfirmed = true;
    app.tick_graph_switch();
    assert!(app.graph_switch_tried.is_empty());
    assert!(app.session.mailbox.account(account).is_some_and(|a| a.on_imap_with_microsoft()));
    let _ = std::fs::remove_dir_all(dir);
}
