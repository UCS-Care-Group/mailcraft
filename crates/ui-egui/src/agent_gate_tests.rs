// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The buttons of an agent's question (#566).

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::FolderKind;
use serde_json::json;

use crate::agent_gate::{answer, verbs};
use crate::control::{ControlRequest, Outcome};
use crate::{MailApp, Services};

#[test]
fn a_rule_made_or_switched_on_isnt_answered_with_delete() {
    for id in ["rules.add", "rules.keepOnServer", "rules.enable", "rules.move"] {
        assert_eq!(verbs(Class::Delete, id, false), (tl!("Allow"), tl!("Don't Allow")), "{id}");
    }
    for id in ["rules.remove", "mail.delete"] {
        assert_eq!(verbs(Class::Delete, id, false), (tl!("Delete"), tl!("Don't Delete")), "{id}");
    }
    // A rule made from a message runs at once on the mail from that sender: a yes deletes that
    // mail only when the rule does.
    assert_eq!(verbs(Class::Delete, "rules.fromMessage", true), (tl!("Delete"), tl!("Don't Delete")));
    assert_eq!(verbs(Class::Delete, "rules.fromMessage", false), (tl!("Allow"), tl!("Don't Allow")));
}

/// An undo that only brings back a rule that deletes mail permanently deletes nothing itself: its
/// buttons say Allow, as for making that rule. One that takes something away says Delete.
#[test]
fn an_undo_that_brings_back_a_deleting_rule_isnt_answered_with_delete() {
    let mut a = MailApp::new(Session::empty(), Services::default());
    a.autosave = false;
    let account = a.execute("account.add", json!({"email": "me@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let purge = json!({"name": "Purge", "account": account, "server": false,
        "conditions": [{"type": "subjectContains", "value": "news"}], "actions": [{"type": "deleteForGood"}]});
    let id = a.execute("rules.add", purge).unwrap()["id"].as_u64().unwrap();
    a.execute("rules.remove", json!({"id": id})).unwrap();
    let (req, _reply) = ControlRequest::new("engine.execute", json!({"command": "edit.undo", "params": {}}));
    let out = crate::control::handle(&mut a, &egui::Context::default(), &req.from_agent(Vec::new()));
    assert!(matches!(out, Outcome::Pending));
    let asked = a.asking.front().unwrap();
    assert_eq!(asked.class, Class::Delete, "{}", asked.what);
    assert_eq!(verbs(asked.class, &asked.id, asked.deletes), (tl!("Allow"), tl!("Don't Allow")));
    assert_eq!(verbs(Class::Delete, "edit.undo", true), (tl!("Delete"), tl!("Don't Delete")));
}

#[test]
fn a_rule_that_copies_onto_another_server_and_deletes_for_good_is_asked_about_once_as_a_delete() {
    // Answered Send, the rule was made without the owner's yes to deleting mail for good.
    let mut a = MailApp::new(Session::empty(), Services::default());
    a.autosave = false;
    let mut add = |email: &str| a.execute("account.add", json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let (account, other) = (add("me@example.org"), add("other@example.org"));
    let there = a.session.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    let params = json!({
        "name": "Both", "account": account, "conditions": [{"type": "toMe"}],
        "actions": [{"type": "copyTo", "value": there}, {"type": "deleteForGood"}],
    });
    let (req, reply) = ControlRequest::new("engine.execute", json!({"command": "rules.add", "params": params}));
    let out = crate::control::handle(&mut a, &egui::Context::default(), &req.from_agent(Vec::new()));
    assert!(matches!(out, Outcome::Pending));
    let asked = a.asking.front().unwrap();
    assert_eq!(asked.class, Class::Delete);
    assert!(asked.what.contains("copy it onto the server of other@example.org") && asked.what.contains("delete it permanently"), "{}", asked.what);
    assert_eq!(verbs(asked.class, &asked.id, false), (tl!("Allow"), tl!("Don't Allow")));
    answer(&mut a, true);
    assert!(a.asking.is_empty(), "one question");
    assert!(a.session.mailbox.rules.iter().any(|r| r.name == "Both"));
    assert_eq!(reply.try_recv().unwrap()["ok"], json!(true));
}
