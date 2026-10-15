//! Mail read through Microsoft Graph in the window: the switch, the permission it takes, and the
//! token a Send/Receive of such an account goes with. The host is a stand-in that answers what a
//! test tells it to; nobody is signed in and nothing leaves this machine.

use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};
use mailcraft_model::{Id, MailSource};
use serde_json::json;

use super::*;
use crate::{GraphRequest, GraphResult, Services, SignInRequest};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The host's stand-in: what it was asked, and how it answers.
#[derive(Default)]
struct Host {
    /// Graph's jobs: whether each brought a token along.
    jobs: Vec<bool>,
    /// The sign-ins: (for Graph, for the mail through Graph, for the Favorites).
    sign_ins: Vec<(bool, bool, bool)>,
    /// The runs: the token each went with, and how the account's mail is read.
    runs: Vec<(String, MailSource)>,
    /// The sign-in doesn't cover the mail through Graph.
    unconsented: bool,
    /// What a sign-in for the mail through Graph comes to, when not tokens.
    sign_in_error: Option<String>,
}

struct Rig {
    app: MailApp,
    host: Arc<Mutex<Host>>,
    account: Id,
}

fn tokens(access: &str) -> Tokens {
    Tokens { access: access.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() }
}

impl Rig {
    fn new() -> Rig {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        let host: Arc<Mutex<Host>> = Default::default();
        let shared = host.clone();
        app.services.graph = Some(Box::new(move |request: GraphRequest| {
            let mut host = shared.lock().unwrap();
            host.jobs.push(!request.access.is_empty());
            let mut out = GraphResult { account: request.account.id, ..Default::default() };
            if request.access.is_empty() {
                if host.unconsented && request.account.mail_source == MailSource::Graph {
                    out.mail_consent_needed = true;
                } else {
                    out.tokens = Some(tokens("graph-from-job"));
                }
            }
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(out);
            rx
        }));
        let shared = host.clone();
        app.services.sign_in = Some(Box::new(move |request: SignInRequest| {
            let mut host = shared.lock().unwrap();
            host.sign_ins.push((request.graph, request.mail_graph, request.favorites));
            let answer = match &host.sign_in_error {
                Some(error) if request.mail_graph => Err(error.clone()),
                _ => {
                    host.unconsented &= !request.mail_graph;
                    Ok(tokens(if request.graph { "graph-from-page" } else { "mail-from-page" }))
                }
            };
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(answer);
            rx
        }));
        let shared = host.clone();
        app.services.sync = Some(Box::new(move |request: SyncRequest| {
            shared.lock().unwrap().runs.push((request.access_token.clone(), request.account.mail_source));
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(SyncResult { account: request.account.id, email: request.account.email.clone(), ..Default::default() });
            rx
        }));
        let added = app.execute(
            "account.add",
            json!({"email": "avery@harbor.example", "auth": "oauth2", "provider": "microsoft", "directory": "organisation", "incoming": {"host": "outlook.office365.com"}}),
        );
        let account = added.unwrap()["id"].as_u64().unwrap();
        app.tokens.insert(account, tokens("mail-tok"));
        // The calendar and contacts were brought down a moment ago: Graph's job runs for no
        // other reason than the one a test gives it.
        app.graph_checked.insert(account, crate::now_ms());
        Rig { app, host, account }
    }

    /// Take in what the host answered, as frames would.
    fn settle(&mut self) {
        for _ in 0..6 {
            self.app.poll_sign_in();
            self.app.poll_graph();
            self.app.poll_sync();
        }
    }

    fn runs(&self) -> Vec<(String, MailSource)> {
        self.host.lock().unwrap().runs.clone()
    }

    fn state(&mut self) -> serde_json::Value {
        self.app.execute("ui.mailSource", json!({"account": self.account})).unwrap()
    }
}

#[test]
fn an_account_reads_over_imap_with_its_mail_token_until_the_owner_switches() {
    let mut r = Rig::new();
    assert_eq!(r.state()["source"], "imap");
    assert_eq!(r.state()["offered"], true);
    r.app.execute("ui.sync", json!({"account": r.account})).unwrap();
    r.settle();
    assert_eq!(r.runs(), vec![("mail-tok".to_string(), MailSource::Imap)]);
    // The local account and a shared mailbox have no switch.
    let local = r.app.session.mailbox.accounts[0].id;
    assert!(!r.app.graph_mail_offered(local));
}

#[test]
fn switching_on_asks_for_the_permission_and_the_mail_then_goes_with_graphs_token() {
    let mut r = Rig::new();
    let said = r.app.execute("ui.mailSource", json!({"account": r.account, "source": "graph"})).unwrap();
    assert_eq!((said["source"].as_str(), said["pending"].as_str()), (Some("graph"), Some("signIn")));
    r.settle();
    assert_eq!(r.host.lock().unwrap().sign_ins, vec![(true, true, true)]);
    assert_eq!(r.runs(), vec![("graph-from-page".to_string(), MailSource::Graph)]);
    assert_eq!(r.state()["line"], json!(null));
}

#[test]
fn without_a_token_for_graph_its_job_gets_one_before_the_mail() {
    let mut r = Rig::new();
    r.app.execute("account.mailSource", json!({"account": r.account, "source": "graph"})).unwrap();
    r.app.execute("ui.sync", json!({"account": r.account})).unwrap();
    r.settle();
    assert_eq!(r.host.lock().unwrap().jobs, vec![false]);
    assert_eq!(r.runs(), vec![("graph-from-job".to_string(), MailSource::Graph)]);
}

#[test]
fn a_sign_in_that_does_not_cover_the_mail_is_said_under_the_switch_and_nothing_runs() {
    let mut r = Rig::new();
    r.app.execute("account.mailSource", json!({"account": r.account, "source": "graph"})).unwrap();
    r.host.lock().unwrap().unconsented = true;
    r.app.execute("ui.sync", json!({"account": r.account})).unwrap();
    r.settle();
    assert!(r.runs().is_empty());
    assert_eq!(r.state()["line"], "allow");
    // "Allow…" puts Microsoft's page up for it, and the mail comes.
    r.app.execute("ui.allowGraphMail", json!({"account": r.account})).unwrap();
    r.settle();
    assert_eq!(r.host.lock().unwrap().sign_ins, vec![(true, true, true)]);
    assert_eq!(r.runs(), vec![("graph-from-page".to_string(), MailSource::Graph)]);
    assert_eq!(r.state()["line"], json!(null));
}

#[test]
fn a_page_that_says_no_is_said_with_its_words_and_the_choice_stands() {
    let mut r = Rig::new();
    r.host.lock().unwrap().sign_in_error = Some(crate::favorites_sync::NEEDS_APPROVAL.to_string());
    r.app.execute("ui.mailSource", json!({"account": r.account, "source": "graph"})).unwrap();
    r.settle();
    let state = r.state();
    assert_eq!((state["source"].as_str(), state["line"].as_str()), (Some("graph"), Some("refused")));
    assert!(r.runs().is_empty());
    // Back to IMAP: nothing more is said, and the mail goes as before.
    r.app.execute("ui.mailSource", json!({"account": r.account, "source": "imap"})).unwrap();
    r.settle();
    assert_eq!(r.state()["line"], json!(null));
    assert_eq!(r.runs(), vec![("mail-tok".to_string(), MailSource::Imap)]);
}

#[test]
fn changes_waiting_for_the_server_hold_the_switch_until_the_owner_lets_them_go() {
    let mut r = Rig::new();
    let inbox = r.app.session.mailbox.folder_of_kind(r.account, mailcraft_model::FolderKind::Inbox).unwrap();
    let news = SyncResult {
        account: r.account,
        email: "avery@harbor.example".into(),
        folders: vec![mailcraft_model::sync::FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: mailcraft_model::FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 1,
            new: vec![mailcraft_model::Message { uid: 1, subject: "Waiting".into(), ..Default::default() }],
            ..Default::default()
        }],
        ..Default::default()
    };
    mailcraft_engine::cmd::account::apply_sync(&mut r.app.session, news).unwrap();
    let id = r.app.session.mailbox.messages.values().find(|m| m.subject == "Waiting").unwrap().id;
    r.app.execute("mail.markRead", json!({"ids": [id]})).unwrap();
    assert!(r.app.execute("ui.mailSource", json!({"account": r.account, "source": "graph"})).is_err());
    assert_eq!((r.state()["source"].as_str(), r.state()["line"].as_str()), (Some("imap"), Some("waiting")));
    let said = r.app.execute("ui.mailSource", json!({"account": r.account, "source": "graph", "discard": true})).unwrap();
    assert_eq!((said["source"].as_str(), said["discarded"].as_u64()), (Some("graph"), Some(1)));
}

#[test]
fn an_account_whose_mail_is_read_through_graph_has_its_favorites_from_graph_without_a_choice() {
    let mut r = Rig::new();
    assert!(r.app.favorites_choice(r.account));
    r.app.execute("account.mailSource", json!({"account": r.account, "source": "graph"})).unwrap();
    assert!(!r.app.favorites_choice(r.account));
    // Whatever was chosen for them before, they are read through Graph now, when they are due.
    r.app.execute("account.favoritesSource", json!({"account": r.account, "source": "none"})).unwrap();
    assert!(r.app.favorites_ask(r.account, crate::now_ms(), true));
}
