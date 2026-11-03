// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent's Delete or Move through the window takes the messages it names (#620), and the owner
//! is asked only about messages that are there (#503).

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::FolderKind;
use serde_json::{Value, json};

use crate::agent_gate::answer;
use crate::control::{ControlRequest, Outcome};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

/// `command` sent by an agent the owner allowed `allow` in advance: its answer, when it has one
/// already.
fn agent(a: &mut MailApp, command: &str, params: Value, allow: Vec<Class>) -> Option<Value> {
    let (req, _) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let out = crate::control::handle(a, &egui::Context::default(), &req.from_agent(allow));
    if let Outcome::Done(v) = out { Some(v) } else { None }
}

#[test]
fn an_agents_delete_or_move_naming_no_messages_is_refused_whatever_the_owner_selected() {
    // The owner's closed conversation's row stands for its messages (#503); an agent allowed
    // everything still names what it acts on (#620), and the window fills in none of them.
    let mut a = app();
    a.execute("folder.select", json!({"kind": "inbox"})).unwrap();
    a.execute("view.conversations", json!({"value": true})).unwrap();
    let row = a.session.rows().iter().find(|r| r.members.len() == 2).cloned().unwrap();
    a.execute("mail.select", json!({"id": row.id})).unwrap();
    let (inbox, account) = (a.session.view.folder, a.session.mailbox.accounts[0].id);
    let archive = a.session.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    for (command, params) in [("mail.delete", json!({})), ("mail.deletePermanently", json!({})), ("mail.move", json!({"folder": archive}))] {
        let r = agent(&mut a, command, params, Class::ALL.to_vec());
        assert!(r.as_ref().and_then(|r| r["error"].as_str()).is_some_and(|e| e.contains("needs `ids`")), "{command}: {r:?}");
        assert!(a.asking.is_empty(), "{command}: the owner isn't asked");
        assert!(row.members.iter().all(|m| a.session.mailbox.message(*m).is_some_and(|m| m.folder == inbox)), "{command}");
    }
}

#[test]
fn an_agents_delete_or_move_of_a_message_that_isnt_there_is_refused_not_asked_about() {
    // A message the owner deleted for good, which an undo puts back while the question is up: a
    // yes took it, though the question showed no such message.
    for command in ["mail.deletePermanently", "mail.move"] {
        let mut a = app();
        let account = a.session.mailbox.accounts[0].id;
        let outbox = a.session.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
        let id = a.session.mailbox.messages.values().find(|m| m.account == account && !m.draft).map(|m| m.id).unwrap();
        a.execute("mail.deletePermanently", json!({"ids": [id]})).unwrap();
        assert!(a.session.mailbox.message(id).is_none());
        let r = agent(&mut a, command, json!({"ids": [id], "folder": outbox}), Vec::new());
        a.execute("edit.undo", json!({})).unwrap();
        answer(&mut a, true);
        assert!(a.session.mailbox.message(id).is_some_and(|m| m.folder != outbox), "{command}: the message the undo put back stays");
        assert!(r.as_ref().and_then(|r| r["error"].as_str()).is_some_and(|e| e.contains("isn't in the mailbox")), "{command}: {r:?}");
    }
}
