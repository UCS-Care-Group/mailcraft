// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an agent is asked before a rule's change, and what a rule that can't tell leaves alone
//! (#566): a delete of a rule on its server beside a rule filing onto another server is still a
//! delete; a rule whose exception a Microsoft account's listing cut short doesn't act.

use mailcraft_model::remote_rules::{RemoteRule, RulesDownload};
use mailcraft_model::{Action, Condition, FolderKind, Id, ServerFolder};
use serde_json::{Value, json};

use super::rules_remote::merge_rules;
use crate::Session;
use crate::agent::{Class, Verdict, check};

const EMAIL: &str = "owner@example.org";

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A Microsoft account (Inbox on the server), a second Microsoft account, and a rule of this
/// computer for the first account's mail that moves it onto the second's server.
fn two_microsoft() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": EMAIL, "kind": "microsoft"}))["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), name: "Inbox".into(), ..Default::default() });
    let other = run(&mut s, "account.add", json!({"email": "other@example.org", "kind": "microsoft"}))["id"].as_u64().unwrap();
    let there = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Away", "account": account, "server": false, "conditions": [{"type": "fromContains", "value": "news"}],
            "actions": [{"type": "moveTo", "value": there}]}),
    );
    (s, account, other)
}

fn asked(s: &Session, id: &str, params: &Value, allow: &[Class]) -> Option<Class> {
    match check(s, id, params, allow) {
        Verdict::Ask(class, _) => Some(class),
        Verdict::Run => None,
        Verdict::Refuse(why) => panic!("{why}"),
    }
}

/// Deleting a rule its server has is a Delete for an agent (undo doesn't reach the server). With a
/// rule beside it that files mail onto another server, the command is classed Send first, and the
/// Delete is lost: an agent allowed only to send deletes the server's rule unasked.
#[test]
fn deleting_a_server_rule_beside_a_rule_filing_onto_another_server_still_needs_delete() {
    let (mut s, account, _) = two_microsoft();
    let remote = RemoteRule {
        id: "r1".into(),
        name: "Server's".into(),
        sequence: 1,
        enabled: true,
        conditions: vec![Condition::SubjectContains(vec!["INVOICE".into()])],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    merge_rules(&mut s.mailbox, &RulesDownload { account, email: EMAIL.into(), complete: true, rules: vec![remote] }).unwrap();
    let there = s.mailbox.rules.iter().find(|r| r.name == "Server's").unwrap().id;
    assert!(s.mailbox.rules.iter().find(|r| r.id == there).unwrap().on_server());
    let params = json!({"id": there});
    // Without the rule beside it: a Delete.
    let alone = {
        let mut s2 = Session::empty();
        s2.mailbox = s.mailbox.clone();
        s2.mailbox.rules.retain(|r| r.name != "Away");
        asked(&s2, "rules.remove", &params, &[Class::Send])
    };
    assert_eq!(alone, Some(Class::Delete), "alone it is asked as a delete");
    // Beside "Away": an agent allowed to send must still be asked, as a delete.
    assert_eq!(
        asked(&s, "rules.remove", &params, &[Class::Send]),
        Some(Class::Delete),
        "an agent with --allow send deletes the server's rule unasked"
    );
}

/// The same for a new rule kept on the server that deletes mail (moves it to Deleted Items there).
#[test]
fn a_new_server_rule_that_deletes_beside_a_rule_filing_onto_another_server_still_needs_delete() {
    let (s, account, _) = two_microsoft();
    let params = json!({"name": "Bin", "account": account, "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "delete"}]});
    assert_eq!(asked(&s, "rules.add", &params, &[Class::Send]), Some(Class::Delete), "a yes to send alone puts a deleting rule on the server");
}

/// The siblings of the two above, by the same cause: switching on a server rule with parts not
/// known here, moving a server rule that deletes, and keeping on the server a rule that deletes,
/// each beside "Away", are let through on `--allow send` alone.
#[test]
fn the_other_server_rule_deletes_beside_a_rule_filing_onto_another_server_still_needs_delete() {
    let (mut s, account, _) = two_microsoft();
    let mut forward = RemoteRule {
        id: "r1".into(),
        name: "Forward to Bo".into(),
        sequence: 1,
        enabled: false,
        conditions: vec![Condition::SubjectContains(vec!["INVOICE".into()])],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    forward.unknown = vec!["forwardTo".into()];
    let bin = RemoteRule { id: "r2".into(), name: "Bin".into(), sequence: 2, actions: vec![Action::Delete], enabled: true, ..forward.clone() };
    let bin = RemoteRule { unknown: Vec::new(), ..bin };
    merge_rules(&mut s.mailbox, &RulesDownload { account, email: EMAIL.into(), complete: true, rules: vec![forward, bin] }).unwrap();
    let id = |s: &Session, name: &str| s.mailbox.rules.iter().find(|r| r.name == name).unwrap().id;
    let (forward, bin) = (id(&s, "Forward to Bo"), id(&s, "Bin"));
    let here = run(
        &mut s,
        "rules.add",
        json!({"name": "Purge", "account": account, "server": false, "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "delete"}]}),
    )["id"]
        .as_u64()
        .unwrap();
    let mut let_through = Vec::new();
    for (command, params) in [
        ("rules.enable", json!({"id": forward, "value": true})),
        ("rules.move", json!({"id": bin, "direction": "down"})),
        ("rules.keepOnServer", json!({"id": here})),
    ] {
        let mut alone = Session::empty();
        alone.mailbox = s.mailbox.clone();
        alone.mailbox.rules.retain(|r| r.name != "Away");
        assert_eq!(asked(&alone, command, &params, &[Class::Send]), Some(Class::Delete), "alone: {command} {params}");
        if asked(&s, command, &params, &[Class::Send]) != Some(Class::Delete) {
            let_through.push(format!("{command} {params}: {:?}", asked(&s, command, &params, &[Class::Send])));
        }
    }
    assert!(let_through.is_empty(), "{let_through:#?}");
}

// ---- A rule that can't tell from a Microsoft account's listing ----

use mailcraft_model::Address;
use mailcraft_model::sync::{GraphMailResult, GraphMessage, MessagePage, ServerMessage};

const ME: &str = "ada@example.org";

/// A new message as a Microsoft account's list gives it: no text, and no size.
fn listed(id: &str, folder: &str, subject: &str) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: folder.into(), change_key: "CK1".into(), ..Default::default() },
        message_id: format!("{id}@example.org"),
        subject: subject.into(),
        from: Address::new("Zoe Park", "zoe@example.org"),
        to: vec![Address::new("Ada", ME)],
        sent: 1_000,
        received: 1_060,
        preview: format!("About {subject}"),
        ..Default::default()
    }
}

/// A Microsoft account whose Inbox ("F-in") has had its first round: the session and the account.
fn microsoft() -> (Session, Id) {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": ME, "kind": "microsoft"}))["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    (s, account)
}

/// Whether the message `subject` lies in Deleted Items.
fn binned(s: &Session, account: Id, subject: &str) -> bool {
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.messages.values().any(|m| m.subject == subject && m.folder == deleted)
}

/// A Microsoft account's listing keeps at most 500 recipients of To and Cc and 25 categories, and
/// says when it left some out (`GraphMessage::cut`). A message to 600 people, the owner past the
/// first 500: "except if my address is in To or Cc" can't tell from the listing, so the rule
/// that deletes doesn't act. The same message with the owner among those kept is left alone by
/// the exception.
#[test]
fn a_listing_that_cut_its_recipients_isnt_deleted_past_an_exception_about_me() {
    let (mut s, account) = microsoft();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Purge news", "account": account, "server": false, "conditions": [{"type": "fromContains", "value": "zoe"}],
            "exceptions": [{"type": "toOrCcMe"}], "actions": [{"type": "delete"}]}),
    );
    let mut m = listed("N1", "F-in", "To all 600 of us");
    m.to = (0..500).map(|i| Address::new("", format!("p{i}@example.org"))).collect();
    m.cut = true;
    let mut kept = listed("N2", "F-in", "To 500 of us");
    kept.to = m.to.clone();
    kept.to[10] = Address::new("Ada", ME);
    let page = MessagePage { folder: "F-in".into(), messages: vec![m, kept], ..Default::default() };
    super::graph_mail::apply_graph_mail(
        &mut s,
        GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() },
    )
    .unwrap();
    assert!(s.mailbox.messages.values().any(|m| m.subject == "To 500 of us"), "the exception leaves it alone");
    let cut = s.mailbox.messages.values().find(|m| m.subject == "To all 600 of us").map(|m| m.headers_cut);
    assert!(!binned(&s, account, "To all 600 of us"), "deleted past an exception it can't tell");
    assert!(!binned(&s, account, "To 500 of us"), "the exception leaves it alone");
    assert_eq!(cut, Some(true), "the message past the 500 kept is here, and said to be cut");
}

/// A message already listed whose next listing cuts its categories (over 25 on a Microsoft
/// account) is said to be cut as well, as a new one is: Run Rules Now with "delete it, except if
/// it has the category Keep" leaves it alone, since Keep may be among those left out.
#[test]
fn a_later_listing_that_cut_the_categories_keeps_a_rule_from_deleting_past_a_category_exception() {
    let (mut s, account) = microsoft();
    let apply = |s: &mut Session, m: GraphMessage| {
        let page = MessagePage { folder: "F-in".into(), messages: vec![m], ..Default::default() };
        super::graph_mail::apply_graph_mail(s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() })
            .unwrap();
    };
    apply(&mut s, listed("N1", "F-in", "Contract"));
    let id = s.mailbox.messages.values().find(|m| m.subject == "Contract").map(|m| m.id).unwrap();
    assert!(!s.mailbox.message(id).unwrap().categories_cut);
    // On the web it gets more categories than a listing gives.
    let mut again = listed("N1", "F-in", "Contract");
    again.server.change_key = "CK2".into();
    again.server.categories = (0..25).map(|i| format!("Tag {i}")).collect();
    again.categories_cut = true;
    apply(&mut s, again);
    assert!(s.mailbox.message(id).unwrap().categories_cut, "the second listing cut its categories");
    run(
        &mut s,
        "rules.add",
        json!({"name": "Purge", "account": account, "server": false, "conditions": [{"type": "fromContains", "value": "zoe"}],
            "exceptions": [{"type": "category", "value": "Keep"}], "actions": [{"type": "delete"}]}),
    );
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    run(&mut s, "rules.run", json!({"folder": inbox}));
    assert!(!binned(&s, account, "Contract"), "deleted, though Keep may be among the categories left out");
}
