// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Passwords and sign-ins in memory don't outlive the mailbox they were given for (C1 PR-a).

use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::Services;

const NOW: i64 = 1_791_993_600;

fn app_with_secrets() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let account = a.session.mailbox.accounts[0].id;
    a.passwords.insert(account, "invented".into());
    a.tokens.insert(account, Default::default());
    a.graph_tokens.insert(account, Default::default());
    a
}

fn nothing_kept(a: &MailApp) -> bool {
    a.passwords.is_empty() && a.tokens.is_empty() && a.graph_tokens.is_empty()
}

#[test]
fn file_new_drops_secrets_in_memory() {
    let mut a = app_with_secrets();
    a.execute("file.new", json!({})).unwrap();
    assert!(nothing_kept(&a));
}

#[test]
fn file_open_drops_secrets_in_memory() {
    let mut a = app_with_secrets();
    let dir = std::env::temp_dir().join(format!("mailcraft-c1a-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("other.mailcraft");
    a.session.run("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();
    a.passwords.insert(1, "invented".into());
    a.tokens.insert(1, Default::default());
    a.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(nothing_kept(&a));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_replacement_the_window_did_not_run_is_noticed_in_the_next_frame() {
    let mut a = app_with_secrets();
    a.session.replace_mailbox(mailcraft_model::Mailbox::with_local_account("", ""));
    a.drop_secrets_if_replaced();
    assert!(nothing_kept(&a));
}

#[test]
fn secrets_stay_while_the_mailbox_does() {
    let mut a = app_with_secrets();
    a.drop_secrets_if_replaced();
    assert!(!nothing_kept(&a));
    a.execute("mail.markAllRead", json!({})).ok();
    assert!(!a.passwords.is_empty());
}
