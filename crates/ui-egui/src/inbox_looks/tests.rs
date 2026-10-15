//! The other Inboxes looked at while a run is busy: when, which, with what sign-in, and what
//! comes of it. The host is a stand-in; nothing leaves this machine.

use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::sync::{FolderDelta, SyncRequest, SyncResult, Tokens};
use mailcraft_model::{FolderKind, Message};
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// What the host was asked: the account, whether it was a look at an Inbox, and the token.
type Asked = (Id, bool, String);

struct Rig {
    app: MailApp,
    owner: Id,
    shared: Id,
    asked: Arc<Mutex<Vec<Asked>>>,
    /// The runs that are still under way (their answers never come).
    _open: Arc<Mutex<Vec<mpsc::SyncSender<SyncResult>>>>,
}

fn rig() -> Rig {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let owner = app
        .execute(
            "account.add",
            json!({"email": "avery@harbor.example", "incoming": {"host": "outlook.office365.com"}, "outgoing": {"host": "smtp.office365.com", "security": "starttls"}, "auth": "oauth2", "directory": "organisation"}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let shared = app.execute("account.addShared", json!({"account": owner, "address": "support@harbor.example"})).unwrap()["id"].as_u64().unwrap();
    app.tokens
        .insert(owner, Tokens { access: "tok-1".into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() });
    let asked: Arc<Mutex<Vec<Asked>>> = Default::default();
    let open: Arc<Mutex<Vec<mpsc::SyncSender<SyncResult>>>> = Default::default();
    let (keep, hold) = (asked.clone(), open.clone());
    app.services.sync = Some(Box::new(move |request: SyncRequest| {
        keep.lock().unwrap().push((request.account.id, request.inbox_only, request.access_token.clone()));
        let (tx, rx) = mpsc::sync_channel(8);
        if request.inbox_only {
            // A look: the Inbox has one new message.
            let inbox = request.folders.iter().find(|f| f.kind == FolderKind::Inbox).map(|f| f.id);
            let delta = FolderDelta {
                id: inbox,
                remote: "INBOX".into(),
                kind: FolderKind::Inbox,
                uid_validity: 11,
                uid_seen: 1,
                new: vec![Message { uid: 1, subject: "Printer on fire".into(), ..Default::default() }],
                ..Default::default()
            };
            let _ =
                tx.send(SyncResult { account: request.account.id, email: request.account.email.clone(), folders: vec![delta], ..Default::default() });
        } else {
            // The long run: it goes on and on.
            hold.lock().unwrap().push(tx);
        }
        rx
    }));
    Rig { app, owner, shared, asked, _open: open }
}

#[test]
fn a_shared_inbox_is_looked_at_while_its_accounts_run_is_busy_for_long() {
    let mut r = rig();
    r.app.execute("ui.sync", json!({"account": r.owner})).unwrap();
    assert_eq!(r.asked.lock().unwrap().len(), 1);
    let t0 = 1_000_000.0;
    // Not yet: the run hasn't been at it for long.
    r.app.poll_inbox_looks(t0);
    r.app.poll_inbox_looks(t0 + AFTER_MS - 1.0);
    assert_eq!(r.asked.lock().unwrap().len(), 1);
    // Now the shared mailbox's Inbox is looked at, with the owner's sign-in, beside the run.
    r.app.poll_inbox_looks(t0 + AFTER_MS);
    assert_eq!(r.asked.lock().unwrap().last().cloned(), Some((r.shared, true, "tok-1".to_string())));
    r.app.poll_inbox_looks(t0 + AFTER_MS + 1.0);
    assert!(r.app.inbox_looks.looking().is_none());
    let arrived: Vec<&Message> = r.app.session.mailbox.messages.values().map(|m| m.as_ref()).filter(|m| m.subject == "Printer on fire").collect();
    assert_eq!(arrived.len(), 1);
    assert_eq!(arrived[0].account, r.shared);
    // Not again for a while; then again.
    r.app.poll_inbox_looks(t0 + AFTER_MS + 2.0);
    assert_eq!(r.asked.lock().unwrap().len(), 2);
    r.app.poll_inbox_looks(t0 + AFTER_MS + EVERY_MS);
    assert_eq!(r.asked.lock().unwrap().len(), 3);
    // The run's own account is never looked at beside it.
    assert!(r.asked.lock().unwrap().iter().skip(1).all(|(id, inbox_only, _)| *id != r.owner && *inbox_only));
}

#[test]
fn nothing_is_looked_at_without_a_sign_in_at_hand_or_without_a_long_run() {
    let mut r = rig();
    // No run: nothing.
    r.app.poll_inbox_looks(0.0);
    r.app.poll_inbox_looks(AFTER_MS * 3.0);
    assert!(r.asked.lock().unwrap().is_empty());
    // A long run, but the owner's sign-in has run out: nobody is asked to sign in for a look.
    r.app.execute("ui.sync", json!({"account": r.owner})).unwrap();
    r.app.tokens.clear();
    r.app.poll_inbox_looks(1_000_000.0);
    r.app.poll_inbox_looks(1_000_000.0 + AFTER_MS);
    assert_eq!(r.asked.lock().unwrap().len(), 1);
    assert!(!r.app.signing_in());
}
