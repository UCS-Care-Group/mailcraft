//! Shared mailboxes in the window: a Send/Receive of one goes with its owner's sign-in, and the
//! pages and dialogs that show one lay out without trouble.

use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// What a run was asked: whose it was, the token it carried, the names the servers are signed in
/// to with, and whether it went with a password.
type Asked = (u64, String, String, String, bool);

/// The sample mailbox with a Microsoft 365 account of work and a shared mailbox it opens.
fn app() -> (MailApp, u64, u64) {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    // No rounds by the clock: only what is asked for, and what that leaves waiting.
    a.auto_sync = false;
    let owner = a
        .execute(
            "account.add",
            json!({"email": "avery@harbor.example", "incoming": {"host": "outlook.office365.com"}, "outgoing": {"host": "smtp.office365.com", "security": "starttls"}, "auth": "oauth2", "directory": "organisation"}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let shared = a.execute("account.addShared", json!({"account": owner, "address": "support@harbor.example"})).unwrap()["id"].as_u64().unwrap();
    (a, owner, shared)
}

fn fresh(access: &str) -> Tokens {
    Tokens { access: access.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() }
}

#[test]
fn a_shared_mailbox_syncs_with_its_owners_sign_in() {
    let (mut a, owner, shared) = app();
    let asked: Arc<Mutex<Vec<Asked>>> = Default::default();
    let asked2 = asked.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        let login = req.account.login().to_string();
        let sending = req.sending_account().login().to_string();
        asked2.lock().unwrap().push((req.account.id, req.access_token.clone(), login, sending, !req.password.is_empty()));
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() }).unwrap();
        rx
    }));
    let sign_ins: Arc<Mutex<Vec<(u64, String, bool)>>> = Default::default();
    let sign_ins2 = sign_ins.clone();
    a.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
        sign_ins2.lock().unwrap().push((req.account, req.email.clone(), req.quiet));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(fresh("tok-2"))).unwrap();
        rx
    }));
    let settle = |a: &mut MailApp| {
        for _ in 0..20 {
            a.poll_sign_in();
            a.poll_sync();
            a.auto_sync();
        }
    };
    // Nothing is at hand: the shared mailbox can't go quietly, nor can its owner.
    assert!(!a.can_sync_quietly(shared));
    // Asked for, the owner signs in (on the owner's page, for the owner's address), the owner's
    // account runs, and the shared mailbox after it with the same token.
    let started = a.execute("ui.sync", json!({"account": shared})).unwrap();
    assert_eq!(started["pending"], "signIn");
    assert_eq!(started["account"], owner);
    settle(&mut a);
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, "avery@harbor.example".to_string(), false)]);
    let runs = asked.lock().unwrap().clone();
    assert_eq!(
        runs,
        [
            (owner, "tok-2".to_string(), "avery@harbor.example".to_string(), "avery@harbor.example".to_string(), false),
            (shared, "tok-2".to_string(), "support@harbor.example".to_string(), "avery@harbor.example".to_string(), false),
        ]
    );
    // No token is kept for the shared mailbox itself; its owner's does for both.
    assert!(!a.tokens.contains_key(&shared) && a.tokens.contains_key(&owner));
    assert!(a.can_sync_quietly(shared));
    // With the owner's token at hand, it runs at once, without a sign-in.
    asked.lock().unwrap().clear();
    a.tokens.insert(owner, fresh("tok-3"));
    assert_eq!(a.execute("ui.sync", json!({"account": shared})).unwrap()["started"], true);
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().first().map(|r| (r.0, r.1.clone())), Some((shared, "tok-3".to_string())));
    assert_eq!(sign_ins.lock().unwrap().len(), 1);
    // A refusal for the shared mailbox costs its owner nothing.
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        let errors = vec!["Couldn't open the shared mailbox support@harbor.example: the server didn't let your account in. Opening it takes Full Access to it, which an administrator of your organisation gives (sign-in failed: AUTHENTICATE failed)".to_string()];
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), errors, ..Default::default() }).unwrap();
        rx
    }));
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert_eq!(a.tokens.get(&owner).map(|t| t.access.as_str()), Some("tok-3"));
}

#[test]
fn the_dialog_adds_it_and_starts_a_send_receive() {
    let (mut a, owner, _) = app();
    a.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner, "address": "sales@harbor.example"})).unwrap();
    assert!(
        matches!(&a.dialog, Some(crate::dialogs::Dialog::SharedMailbox { account, address, .. }) if *account == owner && address == "sales@harbor.example")
    );
    // Not under an account that can't open one.
    let local = a.session.mailbox.accounts[0].id;
    a.dialog = None;
    let _ = a.execute("ui.dialog", json!({"name": "sharedMailbox", "account": local}));
    assert!(a.dialog.is_none());
}

#[test]
fn the_pages_that_show_one_lay_out_in_light_and_dark() {
    for theme in ["light", "dark"] {
        for size in [(1280.0, 800.0), (320.0, 200.0)] {
            let steps: [(&str, serde_json::Value); 1] = [("ui.backstage", json!({"value": true, "page": "info"}))];
            let (mut a, owner, shared) = app();
            a.execute("ui.theme", json!({"value": theme})).unwrap();
            for (id, params) in &steps {
                a.execute(id, params.clone()).unwrap();
            }
            let _ = crate::tests::frames(a, size, 4);
            // The folder pane, and a message from it being written.
            let (mut a, _, _) = app();
            a.execute("ui.theme", json!({"value": theme})).unwrap();
            a.execute("mail.new", json!({"account": shared})).unwrap();
            let _ = crate::tests::frames(a, size, 4);
            // The dialog.
            let (mut a, _, _) = app();
            a.execute("ui.theme", json!({"value": theme})).unwrap();
            a.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
            let _ = crate::tests::frames(a, size, 4);
        }
    }
}

#[test]
fn file_info_offers_a_shared_mailbox_under_a_work_account() {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.auto_sync = false;
    a.execute(
        "account.add",
        json!({"email": "avery@harbor.example", "incoming": {"host": "outlook.office365.com"}, "outgoing": {"host": "smtp.office365.com", "security": "starttls"}, "auth": "oauth2", "provider": "microsoft", "directory": "organisation"}),
    )
    .unwrap();
    a.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    harness.run_steps(4);
    // The link is there, and nothing is drawn over it: clicking it opens its own dialog, not Add
    // Account's (the buttons put inside the card once left the next tile on top of it).
    use egui_kittest::kittest::Queryable;
    let pos = harness.get_by_label_contains("Add shared mailbox").rect().center();
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
    assert!(
        matches!(&harness.state().dialog, Some(crate::dialogs::Dialog::SharedMailbox { .. })),
        "clicking Add shared mailbox… didn't open its dialog"
    );
}
