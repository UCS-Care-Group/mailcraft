// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a rule's changes and deletes wait for, and what an agent is asked before one (#566): a
//! delete of a rule on its server beside a rule filing onto another server is still a delete; a
//! delete for good of an original whose copy is kept on this computer waits for the save, on a
//! Microsoft account too; a delete from Deleted Items after a rule that can't tell yet waits.

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

// ---- A Microsoft account's delete for good after a copy kept on this computer only ----

use mailcraft_model::sync::{GraphFetchResult, GraphFetched, GraphMailResult, GraphMessage, MessageChange, MessagePage, ServerMessage};
use mailcraft_model::{Address, Message};

const ME: &str = "ada@example.org";

fn listed(id: &str, folder: &str, subject: &str) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: folder.into(), size: 1_200, change_key: "CK1".into(), ..Default::default() },
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

fn whole(id: &str, subject: &str, text: &str) -> GraphFetched {
    let mime = format!(
        "From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: {subject}\r\nMessage-ID: <{id}@example.org>\r\nContent-Type: text/plain\r\n\r\n{text}\r\n"
    );
    GraphFetched { id: id.into(), mime: mime.into_bytes() }
}

/// A Microsoft account whose Inbox ("F-in") has had its first round, and a folder of an account on
/// this computer: the session, the account and that folder.
fn microsoft_and_here() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": ME, "kind": "microsoft"}))["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let local = run(&mut s, "account.add", json!({"email": "me@example.org"}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.account(local).unwrap().kind, mailcraft_model::AccountKind::Local);
    let kept = s.mailbox.add_folder(local, None, "Kept").unwrap();
    (s, account, kept)
}

fn graph_deletes(s: &Session, account: Id) -> Vec<MessageChange> {
    super::graph_mail::graph_mail_request(s, account).unwrap().messages.into_iter().filter(|c| matches!(c, MessageChange::Delete { .. })).collect()
}

fn copies_here(s: &Session, kept: Id, subject: &str) -> usize {
    s.mailbox.messages.values().filter(|m: &&std::sync::Arc<Message>| m.folder == kept && m.subject == subject).count()
}

/// New mail of a Microsoft account: a rule that can't tell yet (it reads the text) comes first, so
/// the rule that copies onto this computer and deletes for good waits for the download (#566), and
/// then copies and deletes. The copy is on this computer only, in no file yet; the server's
/// original must wait for the save, as an IMAP account's does (cf0d93d74), or a quit or a crash
/// within the autosave's 3 s loses the message everywhere.
#[test]
fn a_microsoft_rule_that_copies_onto_this_computer_and_deletes_for_good_waits_for_the_save() {
    let (mut s, account, kept) = microsoft_and_here();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Read it", "account": account, "server": false, "conditions": [{"type": "bodyContains", "value": "budget"}],
            "actions": [{"type": "markRead"}]}),
    );
    run(
        &mut s,
        "rules.add",
        json!({"name": "Keep", "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
            "actions": [{"type": "copyTo", "value": kept}, {"type": "deleteForGood"}]}),
    );
    let page = MessagePage { folder: "F-in".into(), messages: vec![listed("N1", "F-in", "Plan")], ..Default::default() };
    super::graph_mail::apply_graph_mail(
        &mut s,
        GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() },
    )
    .unwrap();
    assert_eq!(copies_here(&s, kept, "Plan"), 0, "nothing copied before the download");
    assert!(s.mailbox.messages.values().any(|m| m.subject == "Plan" && m.rules_waiting.is_some()), "the rules wait for the download");
    super::graph_mail::apply_graph_fetch(
        &mut s,
        GraphFetchResult { account, email: ME.into(), fetched: vec![whole("N1", "Plan", "The budget.")], ..Default::default() },
    )
    .unwrap();
    assert_eq!(copies_here(&s, kept, "Plan"), 1, "the rule copied it onto this computer");
    assert!(!s.file_is_current());
    assert_eq!(graph_deletes(&s, account), [], "the server keeps the original while the copy is in no file");
    // Saved: it goes.
    (s.path, s.dirty) = (Some(std::path::PathBuf::from("mailbox.mailcraft")), false);
    assert_eq!(graph_deletes(&s, account).len(), 1);
}

/// The same by Run Rules Now over the Microsoft account's Inbox, its mail downloaded.
#[test]
fn run_rules_now_copying_a_microsoft_message_onto_this_computer_and_deleting_it_for_good_waits_for_the_save() {
    let (mut s, account, kept) = microsoft_and_here();
    let page = MessagePage { folder: "F-in".into(), messages: vec![listed("N1", "F-in", "Plan")], ..Default::default() };
    super::graph_mail::apply_graph_mail(
        &mut s,
        GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() },
    )
    .unwrap();
    super::graph_mail::apply_graph_fetch(
        &mut s,
        GraphFetchResult { account, email: ME.into(), fetched: vec![whole("N1", "Plan", "The plan.")], ..Default::default() },
    )
    .unwrap();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Keep", "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
            "actions": [{"type": "copyTo", "value": kept}, {"type": "deleteForGood"}]}),
    );
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let matched = run(&mut s, "rules.run", json!({"folder": inbox}))["matched"].as_u64().unwrap();
    assert_eq!((matched, copies_here(&s, kept, "Plan")), (1, 1));
    assert!(!s.file_is_current());
    assert_eq!(graph_deletes(&s, account), [], "the server keeps the original while the copy is in no file");
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

/// Run Rules Now over a Microsoft account's Deleted Items, whose mail isn't downloaded: the first
/// rule copies what holds "invoice" in its text, which can't be told yet; the second deletes what is
/// about "Plan", which in Deleted Items is for good. The rules after one that can't tell "delete
/// nothing for good" (#566, `rules_on`), but a Delete there goes for good all the same, before the
/// copy the first rule may have made.
#[test]
fn a_delete_in_deleted_items_after_a_rule_that_cant_tell_yet_is_held_as_a_delete_for_good() {
    let (mut s, account, _) = microsoft_and_here();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.folder_mut(deleted).unwrap().server = Some(ServerFolder { id: "F-del".into(), caught_up: true, ..Default::default() });
    let kept = s.mailbox.add_folder(account, None, "Invoices").unwrap();
    s.mailbox.folder_mut(kept).unwrap().server = Some(ServerFolder { id: "F-inv".into(), ..Default::default() });
    let page = MessagePage { folder: "F-del".into(), messages: vec![listed("D1", "F-del", "Plan")], ..Default::default() };
    super::graph_mail::apply_graph_mail(
        &mut s,
        GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() },
    )
    .unwrap();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Invoices", "account": account, "server": false, "conditions": [{"type": "bodyContains", "value": "invoice"}],
            "actions": [{"type": "copyTo", "value": kept}]}),
    );
    run(
        &mut s,
        "rules.add",
        json!({"name": "Bin", "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
            "actions": [{"type": "delete"}]}),
    );
    run(&mut s, "rules.run", json!({"folder": deleted}));
    (s.path, s.dirty) = (Some(std::path::PathBuf::from("mailbox.mailcraft")), false);
    assert_eq!(graph_deletes(&s, account), [], "deleted for good while the rule before it couldn't tell");
}
