// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Work started for one mailbox stays with it (a sign-in that comes back late, a command waiting
//! for its message to download), and a link from outside does not destroy a message being written.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::sync::Tokens;
use serde_json::json;

use super::*;
use crate::{SecretStore, Services};

const NOW: i64 = 1_791_993_600;

#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<String, String>>>);

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

type Answer = mpsc::Sender<Result<Tokens, String>>;

/// A window with a sign-in under way for its first account; the sender is where the answer goes.
fn signing_in() -> (MailApp, Store, Answer, Id) {
    let store = Store::default();
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.ui.remember_sign_in = true;
    a.services.secrets = Some(std::sync::Arc::new(store.clone()));
    let (tx, rx) = mpsc::channel();
    let rx = Mutex::new(Some(rx));
    a.services.sign_in = Some(Box::new(move |_| rx.lock().unwrap().take().unwrap_or_else(|| mpsc::channel().1)));
    let account = a.session.mailbox.accounts[0].id;
    let email = a.session.mailbox.accounts[0].email.clone();
    a.begin_sign_in(account, Default::default(), &email, String::new(), false, false).unwrap();
    assert!(a.signing_in());
    (a, store, tx, account)
}

fn tokens() -> Tokens {
    Tokens { access: "invented-access".into(), refresh: "invented-refresh".into(), expires_at: NOW + 3600, ..Default::default() }
}

fn nothing_taken_in(a: &MailApp, store: &Store) -> bool {
    a.tokens.is_empty() && a.graph_tokens.is_empty() && store.0.lock().unwrap().is_empty() && !a.signing_in()
}

#[test]
fn a_sign_in_answer_after_the_mailbox_was_replaced_is_thrown_away() {
    let (mut a, store, tx, _) = signing_in();
    a.session.replace_mailbox(mailcraft_engine::sample::mailbox(NOW, 0));
    tx.send(Ok(tokens())).unwrap();
    a.poll_sign_in();
    assert!(nothing_taken_in(&a, &store));
}

#[test]
fn replacing_the_mailbox_calls_off_the_sign_in() {
    let (mut a, store, tx, _) = signing_in();
    a.session.replace_mailbox(mailcraft_engine::sample::mailbox(NOW, 0));
    a.drop_secrets_if_replaced();
    assert!(!a.signing_in());
    tx.send(Ok(tokens())).ok();
    a.poll_sign_in();
    assert!(nothing_taken_in(&a, &store));
}

#[test]
fn a_sign_in_for_the_open_mailbox_still_counts() {
    let (mut a, _, tx, account) = signing_in();
    tx.send(Ok(tokens())).unwrap();
    a.poll_sign_in();
    assert!(!a.signing_in());
    assert!(a.tokens.contains_key(&account));
}

#[test]
fn a_link_from_outside_saves_the_message_being_written_first() {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.execute("mail.new", json!({"to": "kit@example.net", "subject": "Half written"})).unwrap();
    a.execute("compose.set", json!({"body": "not sent yet"})).unwrap();
    assert!(a.session.compose.as_ref().is_some_and(|c| c.dirty));
    crate::files::dropped_path(&mut a, "mailto:lin@example.org?subject=Hello");
    let kept = a.session.mailbox.messages.iter().any(|(_, m)| m.subject == "Half written");
    assert!(kept, "the draft is in the mailbox");
    assert!(a.session.compose.as_ref().is_some_and(|c| c.subject == "Hello"));
    let said = a.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("saved to Drafts"), "{said}");
}

#[test]
fn a_link_with_nothing_being_written_says_nothing_extra() {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    crate::files::dropped_path(&mut a, "mailto:lin@example.org");
    assert!(a.session.compose.is_some());
    assert!(a.status_msg.as_ref().is_none_or(|(s, _)| !s.contains("saved to Drafts")));
}

/// A command waiting for its message to download was asked of the mailbox open then: once another
/// has taken its place, it doesn't run, not even on the new mailbox's message with the same id.
#[test]
fn a_command_waiting_for_a_download_stays_with_its_mailbox() {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let (tx, rx) = mpsc::channel();
    let rx = Mutex::new(Some(rx));
    a.services.graph_fetch = Some(Box::new(move |_| rx.lock().unwrap().take().unwrap_or_else(|| mpsc::channel().1)));
    let account = a.execute("account.add", json!({"email": "avery@mail.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let fresh = Tokens { expires_at: mailcraft_time::now() + 3600, ..tokens() };
    a.seed_graph_token(account, fresh);
    let id = a.session.mailbox.messages.values().find(|m| !m.subject.is_empty()).unwrap().id;
    let m = a.session.mailbox.message_mut(id).unwrap();
    m.account = account;
    m.server = Some(mailcraft_model::sync::ServerMessage { id: "AAMk1".into(), ..Default::default() });
    m.unfetched = true;
    a.session.sel.messages = vec![id];
    let pending = a.run("mail.forwardAsAttachment", json!({})).unwrap();
    assert_eq!(pending["pending"], "download");

    // Another mailbox takes its place, with a message of that id, whole; then the answer comes.
    a.session.replace_mailbox(mailcraft_engine::sample::mailbox(NOW, 0));
    assert!(a.session.mailbox.message(id).is_some_and(|m| !m.unfetched));
    tx.send(mailcraft_model::sync::GraphFetchResult { offline: true, ..Default::default() }).unwrap();
    a.drive_fetch();
    assert!(a.session.compose.is_none(), "nothing ran on the new mailbox's message {id}");
}
