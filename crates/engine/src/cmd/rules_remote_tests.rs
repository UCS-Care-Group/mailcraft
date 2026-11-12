// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules kept on a Microsoft account's server (#566): which rules go there, what is told to the
//! server, what it made of it, and its rules merged. The server's side is written out by hand, as
//! `mailcraft-net` brings it.

use mailcraft_model::remote_rules::{RemoteRule, RuleOpKind, RuleOutcome, RuleResult, RulesDownload, RulesUploaded};
use mailcraft_model::{Action, Address, Condition, FolderKind, Id, Keep, Message, Rule, ServerFolder};
use serde_json::{Value, json};

use super::rules_remote::{apply_rule_changes, apply_rules, mark_sent, merge_rules, not_sent, rule_upload};
use crate::Session;

const EMAIL: &str = "owner@example.org";

/// A session with a Microsoft account kept in step through Graph, whose Inbox and a folder of its
/// own are on the server, and an account of this computer; the ids of the two accounts and of
/// the Microsoft account's folder.
fn session() -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": EMAIL, "kind": "microsoft"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let here = s.run("account.add", &json!({"email": "me@example.org"})).unwrap();
    let local = here["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.account(local).unwrap().kind, mailcraft_model::AccountKind::Local);
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), name: "Inbox".into(), ..Default::default() });
    let made = run(&mut s, "folder.new", json!({"name": "Newsletters", "parent": inbox}));
    let news = made["id"].as_u64().unwrap();
    s.mailbox.folder_mut(news).unwrap().server = Some(ServerFolder { id: "news".into(), name: "Newsletters".into(), ..Default::default() });
    (s, account, local, news)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn rule(s: &Session, id: Id) -> &Rule {
    s.mailbox.rules.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no rule {id}"))
}

fn add(s: &mut Session, account: Id, name: &str, conditions: Value, actions: Value) -> Id {
    let made = run(s, "rules.add", json!({"name": name, "account": account, "conditions": conditions, "actions": actions}));
    made["id"].as_u64().unwrap()
}

fn news_rule(s: &mut Session, account: Id, news: Id) -> Id {
    add(s, account, "News", json!([{"type": "fromContains", "value": "digest"}]), json!([{"type": "moveTo", "value": news}]))
}

fn remote(id: &str, name: &str, sequence: i32) -> RemoteRule {
    RemoteRule {
        id: id.into(),
        name: name.into(),
        sequence,
        enabled: true,
        conditions: vec![Condition::SubjectContains(vec!["INVOICE".into()])],
        actions: vec![Action::MarkRead],
        ..Default::default()
    }
}

fn download(account: Id, rules: Vec<RemoteRule>) -> RulesDownload {
    RulesDownload { account, email: EMAIL.into(), complete: true, rules }
}

fn answered(account: Id, outcomes: Vec<RuleOutcome>) -> RulesUploaded {
    RulesUploaded { account, email: EMAIL.into(), outcomes, token_refused: false }
}

/// The server took every change it was told, the rules it made being called `made-<rule>`.
fn all_taken(s: &mut Session, account: Id) -> Value {
    let upload = rule_upload(&s.mailbox, account).unwrap();
    let outcomes = upload
        .ops
        .iter()
        .map(|op| RuleOutcome {
            kind: op.kind,
            rule: op.rule,
            id: if op.kind == RuleOpKind::Create { format!("made-{}", op.rule) } else { op.id.clone() },
            result: RuleResult::Done,
            enabled: op.enabled.or(op.body.as_ref().map(|b| b.enabled)),
            sequence: op.sequence.or(op.body.as_ref().map(|b| b.sequence)),
            status: 200,
            error: String::new(),
        })
        .collect();
    let merged = apply_rule_changes(s, &answered(account, outcomes)).unwrap();
    json!({"messages": merged.messages, "done": merged.added + merged.updated + merged.removed})
}

fn arriving(s: &mut Session, account: Id, from: &str, subject: &str) -> Id {
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let m = Message { folder: inbox, from: Address::new("Weekly Digest", from), subject: subject.into(), ..Default::default() };
    s.mailbox.add_message(m).unwrap()
}

#[test]
fn a_rule_of_a_microsoft_account_is_kept_on_its_server_when_it_can_be() {
    let (mut s, account, local, news) = session();
    let id = news_rule(&mut s, account, news);
    assert_eq!((rule(&s, id).account, rule(&s, id).keep), (account, Keep::Server));
    assert!(rule(&s, id).waiting());
    // Flagging: the server can't, so the whole rule stays here, as an Outlook rule that flags is
    // "client-only".
    let flags = add(&mut s, account, "Flag", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}, {"type": "markRead"}]));
    assert_eq!(rule(&s, flags).keep, Keep::Here);
    // A move to a folder of this computer, two texts of one kind (the server would take either),
    // and an account that isn't a Microsoft one: all here.
    let local_inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let here = add(&mut s, account, "Here", json!([{"type": "hasAttachment"}]), json!([{"type": "moveTo", "value": local_inbox}]));
    let two = add(
        &mut s,
        account,
        "Two",
        json!([{"type": "subjectContains", "value": "a"}, {"type": "subjectContains", "value": "b"}]),
        json!([{"type": "markRead"}]),
    );
    let other = add(&mut s, local, "Elsewhere", json!([{"type": "hasAttachment"}]), json!([{"type": "markRead"}]));
    for id in [here, two, other] {
        assert_eq!(rule(&s, id).keep, Keep::Here, "{}", rule(&s, id).name);
    }
    let listed = run(&mut s, "rules.list", json!({"account": account}));
    let why = |name: &str| listed["rules"].as_array().unwrap().iter().find(|r| r["name"] == name).map(|r| r["why"].clone()).unwrap();
    assert_eq!(why("Flag"), "the server can't flag mail");
    assert_eq!(why("Here"), "the folder it moves mail to is on this computer");
    assert!(why("Two").as_str().unwrap().contains("either of them"));
    // The account's list: its server's first, then this computer's; the other account's rule isn't in it.
    let names: Vec<&str> = listed["rules"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["News", "Flag", "Here", "Two"]);
    assert_eq!(listed["rules"][0]["kept"], "waiting");
    assert!(listed["rules"][0]["text"].as_str().unwrap().starts_with("When sender's address contains"), "{}", listed["rules"][0]["text"]);
}

#[test]
fn a_new_rule_goes_up_whole_after_the_servers_and_stops_running_here_once_there() {
    let (mut s, account, _, news) = session();
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Invoices", 3)])).unwrap();
    let id = news_rule(&mut s, account, news);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    assert_eq!(upload.ops.len(), 1);
    let op = &upload.ops[0];
    let body = op.body.as_ref().unwrap();
    assert_eq!((op.kind, op.rule, body.sequence, body.move_to.as_str(), body.name.as_str()), (RuleOpKind::Create, id, 4, "news", "News"));
    assert!(body.actions.iter().all(|a| !matches!(a, Action::MoveTo(_))));
    // While it waits, it runs here.
    let first = arriving(&mut s, account, "digest@example.org", "October");
    assert_eq!(s.mailbox.apply_rules(first, &s.rule_clock()), ["News"]);
    assert_eq!(s.mailbox.message(first).unwrap().folder, news);
    all_taken(&mut s, account);
    assert!(rule(&s, id).on_server());
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    // Now the server runs it: mail that comes in is left as the server left it.
    let second = arriving(&mut s, account, "digest@example.org", "November");
    assert!(s.mailbox.apply_rules(second, &s.rule_clock()).is_empty());
    assert_ne!(s.mailbox.message(second).unwrap().folder, news);
    // Run Rules Now runs it here too, as Outlook's does.
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert_eq!(run(&mut s, "rules.run", json!({"folder": inbox}))["matched"], 1);
    assert_eq!(s.mailbox.message(second).unwrap().folder, news);
}

#[test]
fn a_rule_of_the_server_that_stops_the_rest_stops_this_computers_too() {
    let (mut s, account, _, _) = session();
    let mut stop = remote("r1", "Invoices", 1);
    stop.stop = true;
    merge_rules(&mut s.mailbox, &download(account, vec![stop])).unwrap();
    let flag = add(&mut s, account, "Flag", json!([{"type": "subjectContains", "value": "invoice"}]), json!([{"type": "flag"}]));
    assert_eq!(rule(&s, flag).keep, Keep::Here);
    let m = arriving(&mut s, account, "billing@example.org", "Invoice 12");
    assert!(s.mailbox.apply_rules(m, &s.rule_clock()).is_empty());
    assert_eq!(s.mailbox.message(m).unwrap().flag, mailcraft_model::FlagStatus::None);
    let other = arriving(&mut s, account, "billing@example.org", "Receipt");
    assert!(s.mailbox.apply_rules(other, &s.rule_clock()).is_empty());
}

#[test]
fn rules_made_elsewhere_are_taken_in_and_one_with_unknown_parts_is_only_switched_moved_and_deleted() {
    let (mut s, account, _, news) = session();
    let mut forward = remote("r2", "Forward to Bo", 2);
    forward.unknown = vec!["forwardTo".into()];
    let mut moves = remote("r3", "To news", 3);
    moves.actions.clear();
    moves.move_to = "news".into();
    let mut locked = remote("r4", "Locked", 4);
    locked.read_only = true;
    let merged = merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Invoices", 1), forward, moves, locked])).unwrap();
    assert_eq!(merged.added, 4);
    let by = |s: &Session, name: &str| s.mailbox.rules.iter().find(|r| r.name == name).cloned().unwrap();
    assert_eq!(by(&s, "To news").actions, [Action::MoveTo(news)]);
    let fwd = by(&s, "Forward to Bo");
    assert!(!fwd.known() && fwd.on_server() && fwd.account == account);
    // It never runs here: what it would do isn't known.
    let m = arriving(&mut s, account, "a@example.org", "invoice");
    assert!(!fwd.matches(s.mailbox.message(m).unwrap()));
    assert_eq!(run(&mut s, "rules.enable", json!({"id": fwd.id, "value": false}))["enabled"], false);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    assert_eq!(upload.ops.len(), 1);
    assert_eq!(
        (upload.ops[0].kind, upload.ops[0].enabled, upload.ops[0].sequence, upload.ops[0].body.is_none()),
        (RuleOpKind::Update, Some(false), None, true)
    );
    // A rule the server keeps read-only is not changed or deleted.
    let locked = by(&s, "Locked");
    assert!(s.run("rules.enable", &json!({"id": locked.id})).is_err());
    assert!(s.run("rules.remove", &json!({"id": locked.id})).is_err());
    // The list says what it can't show.
    let listed = run(&mut s, "rules.list", json!({"account": account}));
    let entry = listed["rules"].as_array().unwrap().iter().find(|r| r["name"] == "Forward to Bo").unwrap().clone();
    assert_eq!((entry["kept"].clone(), entry["unknownParts"].clone()), (json!("server"), json!(["forwardTo"])));
    // A folder the mailbox doesn't have is a part it doesn't know.
    let mut lost = remote("r3", "To news", 3);
    lost.actions.clear();
    lost.move_to = "somewhere".into();
    merge_rules(&mut s.mailbox, &download(account, vec![lost])).unwrap();
    assert_eq!(by(&s, "To news").server.unwrap().unknown, ["moveToFolder"]);
}

#[test]
fn a_change_made_here_survives_a_download_until_the_server_has_it() {
    let (mut s, account, _, _) = session();
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Invoices", 1)])).unwrap();
    let id = s.mailbox.rules.iter().find(|r| r.name == "Invoices").unwrap().id;
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    // The server still says on: that is the old word.
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Invoices", 1)])).unwrap();
    assert!(!rule(&s, id).enabled);
    all_taken(&mut s, account);
    let mut off = remote("r1", "Invoices", 1);
    off.enabled = false;
    let again = merge_rules(&mut s.mailbox, &download(account, vec![off])).unwrap();
    assert_eq!(again.unchanged, 1);
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    // Switched on in Outlook on the web: the server's word, with nothing changed here.
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Invoices", 1)])).unwrap();
    assert!(rule(&s, id).enabled);
    // The server keeps the words in capitals; the owner's spelling stays.
    let mine = add(&mut s, account, "Mine", json!([{"type": "subjectContains", "value": "Receipt"}]), json!([{"type": "markRead"}]));
    all_taken(&mut s, account);
    let mut shouted = remote(&format!("made-{mine}"), "Mine", 2);
    shouted.conditions = vec![Condition::SubjectContains(vec!["RECEIPT".into()])];
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Invoices", 1), shouted])).unwrap();
    assert_eq!(rule(&s, mine).conditions, [Condition::SubjectContains(vec!["Receipt".into()])]);
}

#[test]
fn moving_one_accounts_rule_leaves_anothers_in_its_order() {
    let (mut s, account, local, _) = session();
    // A rule for every account, then one of this computer's account, then one of the Microsoft
    // account's kept here: the Microsoft account's list is Every, Mine; the other's Every, Theirs.
    s.mailbox.rules.push(Rule {
        id: 900,
        name: "Every".into(),
        enabled: true,
        conditions: vec![Condition::HasAttachment],
        actions: vec![Action::Flag],
        ..Default::default()
    });
    add(&mut s, local, "Theirs", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}]));
    let mine = add(&mut s, account, "Mine", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}]));
    let names = |s: &Session, a: Id| s.mailbox.rules_in_order(a).iter().map(|i| s.mailbox.rules[*i].name.clone()).collect::<Vec<_>>();
    assert_eq!(
        (names(&s, account), names(&s, local)),
        (["Every", "Mine"].map(String::from).to_vec(), ["Every", "Theirs"].map(String::from).to_vec())
    );
    assert_eq!(run(&mut s, "rules.move", json!({"id": mine, "direction": "up"}))["moved"], true);
    assert_eq!(names(&s, account), ["Mine", "Every"]);
    assert_eq!(names(&s, local), ["Every", "Theirs"], "the other account's rules run as they did");
    assert_eq!(run(&mut s, "rules.move", json!({"id": mine, "direction": "down"}))["moved"], true);
    assert_eq!(
        (names(&s, account), names(&s, local)),
        (["Every", "Mine"].map(String::from).to_vec(), ["Every", "Theirs"].map(String::from).to_vec())
    );
}

#[test]
fn moving_a_rule_for_every_account_leaves_anothers_in_its_order() {
    let (mut s, account, local, _) = session();
    s.mailbox.rules.push(Rule {
        id: 900,
        name: "Every".into(),
        enabled: true,
        conditions: vec![Condition::HasAttachment],
        actions: vec![Action::Flag],
        ..Default::default()
    });
    add(&mut s, local, "Theirs", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}]));
    add(&mut s, account, "Mine", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}]));
    let names = |s: &Session, a: Id| s.mailbox.rules_in_order(a).iter().map(|i| s.mailbox.rules[*i].name.clone()).collect::<Vec<_>>();
    let both = |s: &Session| (names(s, account), names(s, local));
    let list = |a: [&str; 2]| a.map(String::from).to_vec();
    // Moved down in the Microsoft account's list, below its own rule: the other's stays.
    assert_eq!(run(&mut s, "rules.move", json!({"id": 900, "account": account, "direction": "down"}))["moved"], true);
    assert_eq!(both(&s), (list(["Mine", "Every"]), list(["Every", "Theirs"])));
    // And up again.
    assert_eq!(run(&mut s, "rules.move", json!({"id": 900, "account": account, "direction": "up"}))["moved"], true);
    assert_eq!(both(&s), (list(["Every", "Mine"]), list(["Every", "Theirs"])));
}

#[test]
fn a_rule_deleted_while_the_server_was_making_it_is_deleted_there_too() {
    let deleted_while_sent = || {
        let (mut s, account, _, _) = session();
        let id = add(&mut s, account, "Receipts", json!([{"type": "subjectContains", "value": "receipt"}]), json!([{"type": "markRead"}]));
        let upload = rule_upload(&s.mailbox, account).unwrap();
        mark_sent(&mut s, &upload);
        run(&mut s, "rules.remove", json!({"id": id}));
        // Nothing to tell the server until it is known whether it has it.
        assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
        (s, account)
    };
    // No answer came, and the server made it: a full read finds it, and it is deleted there.
    let (mut s, account) = deleted_while_sent();
    let mut made = remote("made-1", "Receipts", 1);
    made.conditions = vec![Condition::SubjectContains(vec!["receipt".into()])];
    merge_rules(&mut s.mailbox, &download(account, vec![made, remote("r2", "Invoices", 2)])).unwrap();
    let names: Vec<&str> = s.mailbox.rules.iter().filter(|r| r.account == account).map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Invoices"], "the deleted rule isn't brought back");
    let ops = rule_upload(&s.mailbox, account).unwrap().ops;
    assert_eq!(ops.iter().map(|op| (op.kind, op.id.as_str())).collect::<Vec<_>>(), [(RuleOpKind::Delete, "made-1")]);
    // The server didn't make it: a full read without it lets it go.
    let (mut s, account) = deleted_while_sent();
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r2", "Invoices", 2)])).unwrap();
    assert!(s.mailbox.rule_gone.is_empty());
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
}

#[test]
fn an_answer_for_a_rule_deleted_while_it_was_made_leaves_a_look_alike_on_the_server_alone() {
    // The owner adds it, its job starts, and it is deleted before the answer comes. The server
    // has a rule of somebody else's that looks the same, not known here yet.
    let deleted_while_sent = || {
        let (mut s, account, _, _) = session();
        let id = receipts_here(&mut s, account);
        let upload = rule_upload(&s.mailbox, account).unwrap();
        mark_sent(&mut s, &upload);
        run(&mut s, "rules.remove", json!({"id": id}));
        (s, account, id, upload)
    };
    let theirs = || RemoteRule { id: "theirs".into(), ..receipts_there() };
    let left_alone = |s: &Session, account: Id, told: &[&str]| {
        let ops = rule_upload(&s.mailbox, account).unwrap().ops;
        assert_eq!(
            ops.iter().map(|op| (op.kind, op.id.as_str())).collect::<Vec<_>>(),
            told.iter().map(|id| (RuleOpKind::Delete, *id)).collect::<Vec<_>>()
        );
        assert!(s.mailbox.rules.iter().any(|r| r.server.as_ref().is_some_and(|x| x.id == "theirs")), "it is shown here");
        assert!(s.mailbox.rule_gone.iter().all(|g| g.made.is_none()));
    };
    // The server made it, and said what it is called: that one goes, by its id.
    let (mut s, account, id, _) = deleted_while_sent();
    let made = RuleOutcome { kind: RuleOpKind::Create, rule: id, id: "s1".into(), result: RuleResult::Done, ..Default::default() };
    apply_rule_changes(&mut s, &answered(account, vec![made])).unwrap();
    merge_rules(&mut s.mailbox, &download(account, vec![RemoteRule { id: "s1".into(), ..receipts_there() }, theirs()])).unwrap();
    left_alone(&s, account, &["s1"]);
    // The server didn't make it, and said so: there is nothing to look for.
    for result in [RuleResult::Refused, RuleResult::Denied] {
        let (mut s, account, id, _) = deleted_while_sent();
        let told = RuleOutcome { kind: RuleOpKind::Create, rule: id, result, ..Default::default() };
        let merged = apply_rule_changes(&mut s, &answered(account, vec![told])).unwrap();
        assert!(merged.messages.iter().all(|m| !m.contains("kept on this computer")), "{:?}", merged.messages);
        merge_rules(&mut s.mailbox, &download(account, vec![theirs()])).unwrap();
        left_alone(&s, account, &[]);
        // Undone and done again, it isn't looked for again.
        assert!(s.run("edit.undo", &json!({})).is_err());
    }
    // Made, with no word of what it is called: it is looked for still, or it would come back.
    let (mut s, account, id, _) = deleted_while_sent();
    let nameless = RuleOutcome { kind: RuleOpKind::Create, rule: id, result: RuleResult::Refused, status: 201, ..Default::default() };
    apply_rule_changes(&mut s, &answered(account, vec![nameless])).unwrap();
    merge_rules(&mut s.mailbox, &download(account, vec![theirs()])).unwrap();
    assert!(s.mailbox.rules.is_empty());
    assert_eq!(rule_upload(&s.mailbox, account).unwrap().ops.iter().map(|op| op.id.as_str()).collect::<Vec<_>>(), ["theirs"]);
    // The job stopped before it, or nothing went out at all: it isn't looked for, and that is saved.
    let (mut s, account, _, upload) = deleted_while_sent();
    let rev = s.rev();
    not_sent(&mut s, &upload, &[]);
    assert_ne!(s.rev(), rev);
    assert!(s.run("edit.undo", &json!({})).is_err());
    merge_rules(&mut s.mailbox, &download(account, vec![theirs()])).unwrap();
    left_alone(&s, account, &[]);
    // No answer at all: the server may have made it, and it is still looked for.
    let (mut s, account, id, upload) = deleted_while_sent();
    let lost = RuleOutcome { kind: RuleOpKind::Create, rule: id, result: RuleResult::Failed, ..Default::default() };
    not_sent(&mut s, &upload, std::slice::from_ref(&lost));
    apply_rule_changes(&mut s, &answered(account, vec![lost])).unwrap();
    assert_eq!(s.mailbox.rule_gone.iter().filter(|g| g.made.is_some()).count(), 1);
}

#[test]
fn a_rule_the_server_found_for_one_deleted_while_made_is_saved_to_be_deleted_at_once() {
    let (mut s, account, _, _) = session();
    let id = receipts_here(&mut s, account);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    run(&mut s, "rules.remove", json!({"id": id}));
    let rev = s.rev();
    apply_rules(&mut s, &download(account, vec![receipts_there()])).unwrap();
    assert_ne!(s.rev(), rev);
    assert_eq!(rule_upload(&s.mailbox, account).unwrap().ops.iter().map(|op| op.id.as_str()).collect::<Vec<_>>(), ["made-1"]);
}

#[test]
fn a_rule_on_the_server_with_more_to_it_than_one_made_here_isnt_taken_for_it() {
    let theirs = [
        RemoteRule { unknown: vec!["forwardTo".into()], ..receipts_there() },
        RemoteRule { read_only: true, ..receipts_there() },
        // It also moves mail to a folder that isn't known here.
        RemoteRule { move_to: "not-here".into(), ..receipts_there() },
    ];
    for theirs in theirs {
        // One deleted while it was being made, with no answer: that rule isn't deleted for it.
        let (mut s, account, _, _) = session();
        let id = receipts_here(&mut s, account);
        let upload = rule_upload(&s.mailbox, account).unwrap();
        mark_sent(&mut s, &upload);
        run(&mut s, "rules.remove", json!({"id": id}));
        merge_rules(&mut s.mailbox, &download(account, vec![theirs.clone()])).unwrap();
        assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
        assert!(s.mailbox.rules.iter().any(|r| r.on_server()));
        // One whose answer was lost: it isn't taken for it, and goes again.
        let (mut s, account, _, _) = session();
        let id = receipts_here(&mut s, account);
        let upload = rule_upload(&s.mailbox, account).unwrap();
        mark_sent(&mut s, &upload);
        merge_rules(&mut s.mailbox, &download(account, vec![theirs])).unwrap();
        assert!(!rule(&s, id).on_server());
        assert_eq!(rule_upload(&s.mailbox, account).unwrap().ops.iter().map(|op| (op.kind, op.rule)).collect::<Vec<_>>(), [(RuleOpKind::Create, id)]);
    }
}

#[test]
fn a_lost_answer_waits_through_a_read_that_left_a_rule_out() {
    let (mut s, account, _, news) = session();
    let id = news_rule(&mut s, account, news);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    // Its folder couldn't be looked up: the read leaves it out, and isn't whole.
    apply_rules(&mut s, &RulesDownload { complete: false, ..download(account, Vec::new()) }).unwrap();
    assert!(rule(&s, id).unanswered);
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    let mut there = remote("r9", "News", 1);
    there.conditions = vec![Condition::FromContains(vec!["DIGEST".into()])];
    there.actions.clear();
    there.move_to = "news".into();
    apply_rules(&mut s, &download(account, vec![there])).unwrap();
    assert!(rule(&s, id).on_server());
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    assert_eq!(s.mailbox.rules.len(), 1);
}

#[test]
fn a_rule_with_an_action_said_twice_isnt_made_twice_after_a_lost_answer() {
    let (mut s, account, _, _) = session();
    let blue = json!({"type": "categorize", "value": "Blue"});
    let id = add(&mut s, account, "Blue", json!([{"type": "hasAttachment"}]), json!([blue, blue, {"type": "markRead"}, {"type": "markRead"}]));
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    // The server keeps each once.
    let there = RemoteRule {
        conditions: vec![Condition::HasAttachment],
        actions: vec![Action::MarkRead, Action::Categorize("blue".into())],
        ..remote("r9", "Blue", 1)
    };
    let other = RemoteRule { actions: vec![Action::Categorize("Blue".into())], id: "r8".into(), ..there.clone() };
    let merged = merge_rules(&mut s.mailbox, &download(account, vec![other, there])).unwrap();
    // What is left out or put in beside them is still another rule.
    assert_eq!((merged.added, merged.updated), (1, 1));
    assert_eq!(rule(&s, id).server.as_ref().map(|x| x.id.as_str()), Some("r9"));
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
}

/// The server's copy of the rule `add` made as "Receipts", looking for "receipt".
fn receipts_there() -> RemoteRule {
    let mut made = remote("made-1", "Receipts", 1);
    made.conditions = vec![Condition::SubjectContains(vec!["receipt".into()])];
    made
}

fn receipts_here(s: &mut Session, account: Id) -> Id {
    add(s, account, "Receipts", json!([{"type": "subjectContains", "value": "receipt"}]), json!([{"type": "markRead"}]))
}

#[test]
fn undo_after_the_server_took_a_lost_rule_in_doesnt_make_it_twice() {
    let (mut s, account, _, _) = session();
    let id = receipts_here(&mut s, account);
    let m = arriving(&mut s, account, "shop@example.org", "Hello");
    run(&mut s, "mail.flag", json!({"id": m}));
    // The job sends the rule and no answer comes; its read takes the server's copy as this rule.
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    let lost = RuleOutcome { kind: RuleOpKind::Create, rule: id, result: RuleResult::Failed, error: "timed out".into(), ..Default::default() };
    apply_rule_changes(&mut s, &answered(account, vec![lost])).unwrap();
    apply_rules(&mut s, &download(account, vec![receipts_there()])).unwrap();
    assert!(rule(&s, id).on_server());
    // Undoing the flag doesn't make the rule wait to be made again.
    let _ = s.run("edit.undo", &json!({}));
    assert!(rule(&s, id).on_server());
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.iter().all(|op| op.kind != RuleOpKind::Create));
}

/// A deletion that has gone to the server can't be undone here: Undo would bring the rule back
/// here alone, and the next read would take it away again without a word.
#[test]
fn undo_after_a_deletion_went_to_the_server_doesnt_bring_the_rule_back_here_alone() {
    let (mut s, account, _, _) = session();
    apply_rules(&mut s, &download(account, vec![remote("r1", "Invoices", 1)])).unwrap();
    let id = s.mailbox.rules.iter().find(|r| r.name == "Invoices").unwrap().id;
    run(&mut s, "rules.remove", json!({"id": id}));
    // The job sends the deletion, and before its answer the owner presses Undo.
    let upload = rule_upload(&s.mailbox, account).unwrap();
    assert_eq!(upload.ops.iter().map(|op| op.kind).collect::<Vec<_>>(), [RuleOpKind::Delete]);
    mark_sent(&mut s, &upload);
    let _ = s.run("edit.undo", &json!({}));
    assert!(s.mailbox.rules.iter().all(|r| r.id != id), "the server is deleting it");
}

#[test]
fn a_rule_deleted_elsewhere_isnt_made_again() {
    let setup = || {
        let (mut s, account, _, _) = session();
        merge_rules(&mut s.mailbox, &download(account, vec![remote("a", "A", 1), remote("b", "B", 2)])).unwrap();
        let a = s.mailbox.rules.iter().find(|r| r.name == "A").unwrap().id;
        // Moving A below B changes where B stands too.
        run(&mut s, "rules.move", json!({"id": a, "direction": "down"}));
        let b = s.mailbox.rules.iter().find(|r| r.name == "B").unwrap();
        assert!(b.changed_here());
        let b = b.id;
        (s, account, b)
    };
    let creates = |s: &Session, account: Id| rule_upload(&s.mailbox, account).unwrap().ops.iter().filter(|op| op.kind == RuleOpKind::Create).count();
    // B was deleted in Outlook on the web: told of its new place, the server has no B.
    let (mut s, account, b) = setup();
    let upload = rule_upload(&s.mailbox, account).unwrap();
    let outcomes = upload
        .ops
        .iter()
        .map(|op| RuleOutcome {
            kind: op.kind,
            rule: op.rule,
            id: op.id.clone(),
            result: if op.id == "b" { RuleResult::Gone } else { RuleResult::Done },
            enabled: op.enabled,
            sequence: op.sequence,
            ..Default::default()
        })
        .collect();
    let merged = apply_rule_changes(&mut s, &answered(account, outcomes)).unwrap();
    assert!(s.mailbox.rules.iter().all(|r| r.id != b), "deleted here too");
    assert!(merged.messages.iter().any(|m| m.contains("\"B\"") && m.contains("deleted")), "{:?}", merged.messages);
    assert_eq!(creates(&s, account), 0);
    // Or a full read shows it gone before the change is told.
    let (mut s, account, b) = setup();
    let merged = merge_rules(&mut s.mailbox, &download(account, vec![remote("a", "A", 1)])).unwrap();
    assert!(s.mailbox.rules.iter().all(|r| r.id != b), "deleted here too");
    assert!(merged.messages.iter().any(|m| m.contains("\"B\"") && m.contains("deleted")), "{:?}", merged.messages);
    assert_eq!(creates(&s, account), 0);
}

#[test]
fn a_rule_switched_off_while_its_creation_had_no_answer_stays_off() {
    let (mut s, account, _, _) = session();
    let id = receipts_here(&mut s, account);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    // The server made it, switched on, and its read takes that as this rule: the switch goes up.
    merge_rules(&mut s.mailbox, &download(account, vec![receipts_there()])).unwrap();
    assert!(rule(&s, id).on_server() && !rule(&s, id).enabled);
    let ops = rule_upload(&s.mailbox, account).unwrap().ops;
    assert_eq!(ops.iter().map(|op| (op.kind, op.id.as_str(), op.enabled)).collect::<Vec<_>>(), [(RuleOpKind::Update, "made-1", Some(false))]);
}

#[test]
fn moving_a_rule_among_the_servers_sends_where_each_stands() {
    let (mut s, account, _, _) = session();
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r2", "Second", 20), remote("r1", "First", 10)])).unwrap();
    let names = |s: &Session| s.mailbox.rules_in_order(account).iter().map(|i| s.mailbox.rules[*i].name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s), ["First", "Second"]);
    let second = s.mailbox.rules.iter().find(|r| r.name == "Second").unwrap().id;
    assert_eq!(run(&mut s, "rules.move", json!({"id": second, "direction": "up"}))["moved"], true);
    assert_eq!(names(&s), ["Second", "First"]);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    let mut sent: Vec<(String, Option<i32>)> = upload.ops.iter().map(|op| (op.id.clone(), op.sequence)).collect();
    sent.sort();
    assert_eq!(sent, [("r1".to_string(), Some(20)), ("r2".to_string(), Some(10))]);
    // At the top already.
    assert_eq!(run(&mut s, "rules.move", json!({"id": second, "direction": "up"}))["moved"], false);
    all_taken(&mut s, account);
    let moved = merge_rules(&mut s.mailbox, &download(account, vec![remote("r2", "Second", 10), remote("r1", "First", 20)])).unwrap();
    assert_eq!(moved.unchanged, 2);
    // A rule waiting to go up isn't moved past one that is there.
    let waiting = add(&mut s, account, "New", json!([{"type": "hasAttachment"}]), json!([{"type": "markRead"}]));
    assert!(s.run("rules.move", &json!({"id": waiting, "direction": "up"})).is_err());
    // Those of this computer move among themselves.
    let a = add(&mut s, account, "A", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}]));
    let b = add(&mut s, account, "B", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}]));
    run(&mut s, "rules.move", json!({"id": b, "direction": "up"}));
    let order = names(&s);
    assert!(order.iter().position(|n| n == "B") < order.iter().position(|n| n == "A"), "{order:?}");
    assert_eq!(run(&mut s, "rules.move", json!({"id": a, "direction": "down"}))["moved"], false);
    // Places the server gives below 1 are numbered afresh when a rule moves: it would take them
    // as 1, and two rules would stand in one place.
    let (mut s, account, _, _) = session();
    let low = vec![remote("m1", "M1", i32::MIN), remote("m2", "M2", 0), remote("m3", "M3", i32::MAX)];
    merge_rules(&mut s.mailbox, &download(account, low)).unwrap();
    let m3 = s.mailbox.rules.iter().find(|r| r.name == "M3").unwrap().id;
    run(&mut s, "rules.move", json!({"id": m3, "direction": "up"}));
    let mut places: Vec<i32> = s.mailbox.rules.iter().filter_map(|r| r.server.as_ref()).map(|s| s.sequence).collect();
    places.sort_unstable();
    assert!(places.first().is_some_and(|p| *p >= 1) && places.windows(2).all(|w| w[0] < w[1]), "{places:?}");
}

#[test]
fn numbering_rules_afresh_goes_round_one_the_server_wont_let_be_moved() {
    let (mut s, account, _, _) = session();
    let locked = RemoteRule { read_only: true, ..remote("ro", "Locked", 5) };
    let rules = vec![remote("m1", "M1", 1), remote("m2", "M2", 1), locked, remote("m3", "M3", 6)];
    merge_rules(&mut s.mailbox, &download(account, rules)).unwrap();
    let m2 = s.mailbox.rules.iter().find(|r| r.name == "M2").unwrap().id;
    run(&mut s, "rules.move", json!({"id": m2, "direction": "up"}));
    let order = s.mailbox.rules_in_order(account);
    let places: Vec<(&str, i32)> =
        order.iter().map(|i| &s.mailbox.rules[*i]).map(|r| (r.name.as_str(), r.server.as_ref().unwrap().sequence)).collect();
    assert_eq!(places, [("M2", 1), ("M1", 2), ("Locked", 5), ("M3", 6)]);
    assert!(s.mailbox.rules.iter().find(|r| r.name == "Locked").is_some_and(|r| !r.changed_here()));
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.iter().all(|op| op.id != "ro"));
    // Its place isn't given to another. Where it leaves too little room before it for the order
    // asked (two rules with one number, and no number free between them and it), the server would
    // put one of them after it: the move is refused, says why, and changes nothing (#765).
    for (a, locked, b) in [(1, 2, 1), (1, 1, 1)] {
        let (mut s, account, _, _) = session();
        let rules = vec![remote("a", "A", a), remote("b", "B", b), RemoteRule { read_only: true, ..remote("ro", "Locked", locked) }];
        merge_rules(&mut s.mailbox, &download(account, rules)).unwrap();
        let rules_before = s.mailbox.rules.clone();
        let id = s.mailbox.rules.iter().find(|r| r.name == "B").unwrap().id;
        let why = s.run("rules.move", &json!({"id": id, "direction": "up"})).unwrap_err().to_string();
        assert!(why.contains("can't keep the rules in that order"), "{why}");
        assert_eq!(s.mailbox.rules, rules_before, "{a} {locked} {b}");
        assert!(rule_upload(&s.mailbox, account).ok().is_none_or(|u| u.ops.is_empty()));
    }
    // Room enough, and it goes, with every rule in the order asked.
    let (mut s, account, _, _) = session();
    let rules = vec![remote("a", "A", 1), remote("b", "B", 1), RemoteRule { read_only: true, ..remote("ro", "Locked", 3) }];
    merge_rules(&mut s.mailbox, &download(account, rules)).unwrap();
    let id = s.mailbox.rules.iter().find(|r| r.name == "B").unwrap().id;
    run(&mut s, "rules.move", json!({"id": id, "direction": "up"}));
    let order = s.mailbox.rules_in_order(account);
    let places: Vec<(&str, i32)> =
        order.iter().map(|i| &s.mailbox.rules[*i]).map(|r| (r.name.as_str(), r.server.as_ref().unwrap().sequence)).collect();
    assert_eq!(places, [("B", 1), ("A", 2), ("Locked", 3)]);
}

#[test]
fn two_rules_with_one_number_keep_the_lists_order_round_a_locked_one() {
    let (mut s, account, _, _) = session();
    let rules = vec![remote("a", "A", 5), remote("b", "B", 5), RemoteRule { read_only: true, ..remote("ro", "Locked", 5) }];
    merge_rules(&mut s.mailbox, &download(account, rules)).unwrap();
    let b = s.mailbox.rules.iter().find(|r| r.name == "B").unwrap().id;
    run(&mut s, "rules.move", json!({"id": b, "direction": "up"}));
    let place = |s: &Session, name: &str| s.mailbox.rules.iter().find(|r| r.name == name).unwrap().server.as_ref().unwrap().sequence;
    assert_eq!(place(&s, "Locked"), 5);
    assert!(place(&s, "B") < place(&s, "A") && place(&s, "A") < 5, "{} {}", place(&s, "B"), place(&s, "A"));
}

#[test]
fn a_switch_the_server_wasnt_told_of_yet_doesnt_decide_whether_later_rules_run() {
    let (mut s, account, _, _) = session();
    let mut stop = remote("r1", "Invoices", 1);
    stop.stop = true;
    merge_rules(&mut s.mailbox, &download(account, vec![stop])).unwrap();
    let id = s.mailbox.rules[0].id;
    add(&mut s, account, "Flag", json!([{"type": "subjectContains", "value": "invoice"}]), json!([{"type": "flag"}]));
    // Off here, still on there: the server ran it, and the rest stop as they would have.
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    let m = arriving(&mut s, account, "billing@example.org", "Invoice 12");
    assert!(s.mailbox.apply_rules(m, &s.rule_clock()).is_empty());
    assert_eq!(s.mailbox.message(m).unwrap().flag, mailcraft_model::FlagStatus::None);
    // Told, and on here again, still off there: the server didn't, and they run.
    all_taken(&mut s, account);
    run(&mut s, "rules.enable", json!({"id": id, "value": true}));
    let m = arriving(&mut s, account, "billing@example.org", "Invoice 13");
    assert_eq!(s.mailbox.apply_rules(m, &s.rule_clock()), ["Flag"]);
    assert_eq!(s.mailbox.message(m).unwrap().flag, mailcraft_model::FlagStatus::Flagged);
}

#[test]
fn an_agent_asks_the_owner_before_a_rule_that_deletes_mail_goes_to_a_server() {
    use crate::agent::{Access, Class, classify};
    let (mut s, account, local, _) = session();
    let params = |account: Id, more: Value| {
        let mut p = json!({"name": "Bin", "account": account, "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "delete"}]});
        p.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        p
    };
    assert_eq!(classify(&s, "rules.add", &params(account, json!({}))), Access::Owner(Class::Delete));
    // On this computer, undo reaches it.
    assert_eq!(classify(&s, "rules.add", &params(account, json!({"server": false}))), Access::Owner(Class::Settings));
    assert_eq!(classify(&s, "rules.add", &params(local, json!({}))), Access::Owner(Class::Settings));
    // Neither does a rule that doesn't delete need more than any other.
    let keeps = json!({"actions": [{"type": "markRead"}]});
    assert_eq!(classify(&s, "rules.add", &params(account, keeps)), Access::Owner(Class::Settings));
    // Sent to the server afterwards.
    let here = run(&mut s, "rules.add", params(account, json!({"server": false})))["id"].as_u64().unwrap();
    assert_eq!(classify(&s, "rules.keepOnServer", &json!({"id": here})), Access::Owner(Class::Delete));
    // Switched on there: not off, which stops it.
    run(&mut s, "rules.keepOnServer", json!({"id": here}));
    run(&mut s, "rules.enable", json!({"id": here, "value": false}));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": here, "value": true})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": here})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": here, "value": false})), Access::Owner(Class::Settings));
    // Moving one that deletes, among the others there, is asked as well; one that doesn't isn't.
    assert_eq!(classify(&s, "rules.move", &json!({"id": here, "direction": "up"})), Access::Owner(Class::Delete));
    let quiet = run(&mut s, "rules.add", params(account, json!({"actions": [{"type": "markRead"}]})))["id"].as_u64().unwrap();
    assert_eq!(classify(&s, "rules.move", &json!({"id": quiet, "direction": "up"})), Access::Owner(Class::Settings));
    // Asked of a rule already there, keeping it there is nothing new.
    assert_eq!(classify(&s, "rules.keepOnServer", &json!({"id": here})), Access::Owner(Class::Settings));
    // Moving mail into Deleted Items is deleting it, however it is asked for.
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.folder_mut(deleted).unwrap().server = Some(ServerFolder { id: "bin".into(), name: "Deleted Items".into(), ..Default::default() });
    let moves = json!({"actions": [{"type": "moveTo", "value": deleted}]});
    assert_eq!(classify(&s, "rules.add", &params(account, moves)), Access::Owner(Class::Delete));
    let m = arriving(&mut s, account, "digest@news.example", "x");
    assert_eq!(classify(&s, "rules.fromMessage", &json!({"folder": deleted, "id": m})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "rules.fromMessage", &json!({"folder": deleted, "id": m, "server": false})), Access::Owner(Class::Settings));
    // So is moving it into a folder under Deleted Items, at any depth; a folder elsewhere isn't.
    let under = s.mailbox.add_folder(account, Some(deleted), "Old").unwrap();
    let deeper = s.mailbox.add_folder(account, Some(under), "Older").unwrap();
    let elsewhere = s.mailbox.add_folder(account, None, "Kept").unwrap();
    for (folder, class) in [(under, Class::Delete), (deeper, Class::Delete), (elsewhere, Class::Settings)] {
        s.mailbox.folder_mut(folder).unwrap().server = Some(ServerFolder { id: format!("f{folder}"), ..Default::default() });
        let moves = json!({"actions": [{"type": "moveTo", "value": folder}]});
        assert_eq!(classify(&s, "rules.add", &params(account, moves)), Access::Owner(class), "{folder}");
    }
    let m = arriving(&mut s, account, "digest@news.example", "y");
    assert_eq!(classify(&s, "rules.fromMessage", &json!({"folder": deeper, "id": m})), Access::Owner(Class::Delete));
}

#[test]
fn an_agent_asks_the_owner_before_it_switches_on_a_server_rule_with_parts_not_known_here() {
    use crate::agent::{Access, Class, Verdict, check, classify};
    let (mut s, account, _, _) = session();
    let mut forward = remote("r1", "Forward to Bo", 1);
    forward.enabled = false;
    forward.unknown = vec!["forwardTo".into()];
    merge_rules(&mut s.mailbox, &download(account, vec![forward])).unwrap();
    let id = s.mailbox.rules.iter().find(|r| r.name == "Forward to Bo").unwrap().id;
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id, "value": true})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id})), Access::Owner(Class::Delete));
    // Off stops it.
    run(&mut s, "rules.enable", json!({"id": id, "value": true}));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id, "value": false})), Access::Owner(Class::Settings));
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    // The question says there is more to it than it can show.
    let Verdict::Ask(class, text) = check(&s, "rules.enable", &json!({"id": id, "value": true}), &[Class::Settings]) else {
        panic!("not asked");
    };
    assert_eq!(class, Class::Delete);
    assert!(text.contains("forwardTo") && text.contains("switch on the rule"), "{text}");
}

#[test]
fn an_agent_asks_the_owner_before_it_deletes_a_rule_the_server_has_and_says_where_a_new_one_goes() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, account, _, news) = session();
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Server's", 1)])).unwrap();
    let there = s.mailbox.rules[0].id;
    let here = add(&mut s, account, "Here", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}]));
    assert_eq!(classify(&s, "rules.remove", &json!({"id": there})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "rules.remove", &json!({"id": here})), Access::Owner(Class::Settings));
    // One that went to be made there, with no answer, may be there: deleted, it is deleted there.
    let sent = receipts_here(&mut s, account);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    assert!(rule(&s, sent).unanswered);
    assert_eq!(classify(&s, "rules.remove", &json!({"id": sent})), Access::Owner(Class::Delete));
    // The question says the server's rule is deleted or switched there too.
    for (command, rule) in [("rules.remove", there), ("rules.remove", sent), ("rules.enable", there)] {
        let asked = describe(&s, command, &json!({"id": rule}));
        assert!(asked.contains(&format!("on the server of “{EMAIL}” too")), "{asked}");
    }
    assert!(!describe(&s, "rules.remove", &json!({"id": here})).contains("server"));
    // A rule from a message goes to the account's server, and the question says so.
    let m = arriving(&mut s, account, "digest@news.example", "x");
    let asked = describe(&s, "rules.fromMessage", &json!({"folder": news, "id": m}));
    assert!(asked.contains("kept on the server of"), "{asked}");
    assert!(!describe(&s, "rules.fromMessage", &json!({"folder": news, "id": m, "server": false})).contains("server"));
    // Its account's own folder: not mail put on another server.
    assert_eq!(classify(&s, "rules.fromMessage", &json!({"folder": news, "id": m})), Access::Owner(Class::Settings));
}

#[test]
fn a_deleted_rule_is_deleted_on_the_server_and_not_brought_back_meanwhile() {
    let (mut s, account, _, _) = session();
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Invoices", 1)])).unwrap();
    let id = s.mailbox.rules.iter().find(|r| r.name == "Invoices").unwrap().id;
    run(&mut s, "rules.remove", json!({"id": id}));
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Invoices", 1)])).unwrap();
    assert!(s.mailbox.rules.iter().all(|r| r.name != "Invoices"));
    let upload = rule_upload(&s.mailbox, account).unwrap();
    assert_eq!((upload.ops.len(), upload.ops[0].kind, upload.ops[0].id.as_str()), (1, RuleOpKind::Delete, "r1"));
    all_taken(&mut s, account);
    assert!(s.mailbox.rule_gone.is_empty());
    // Deleted on the web: gone here too, even when it was changed here since, which is said.
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r2", "Two", 1), remote("r3", "Three", 2)])).unwrap();
    let three = s.mailbox.rules.iter().find(|r| r.name == "Three").unwrap().id;
    run(&mut s, "rules.enable", json!({"id": three, "value": false}));
    let merged = merge_rules(&mut s.mailbox, &download(account, vec![])).unwrap();
    assert_eq!(merged.removed, 2);
    assert!(s.mailbox.rules.iter().all(|r| r.account != account));
    assert_eq!(merged.messages.len(), 1, "{:?}", merged.messages);
    assert!(merged.messages[0].contains("\"Three\""), "{:?}", merged.messages);
    // A download that doesn't say it is whole removes nothing.
    let mut partial = download(account, vec![]);
    partial.complete = false;
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r4", "Four", 1)])).unwrap();
    assert_eq!(merge_rules(&mut s.mailbox, &partial).unwrap().removed, 0);
}

#[test]
fn a_rule_the_server_refuses_stays_here_and_says_why_until_it_is_sent_again() {
    let (mut s, account, _, news) = session();
    let id = news_rule(&mut s, account, news);
    let refused = RuleOutcome {
        kind: RuleOpKind::Create,
        rule: id,
        result: RuleResult::Refused,
        status: 400,
        error: "the rule is too large".into(),
        ..Default::default()
    };
    let merged = apply_rule_changes(&mut s, &answered(account, vec![refused])).unwrap();
    assert!(merged.messages[0].contains("kept on this computer: the rule is too large"), "{:?}", merged.messages);
    assert_eq!((rule(&s, id).keep, rule(&s, id).refused.as_str()), (Keep::Here, "the rule is too large"));
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    let listed = run(&mut s, "rules.list", json!({"account": account}));
    assert_eq!(listed["rules"][0]["why"], "the server didn't take it: the rule is too large");
    // It still runs here.
    let m = arriving(&mut s, account, "digest@example.org", "x");
    assert_eq!(s.mailbox.apply_rules(m, &s.rule_clock()), ["News"]);
    // Keep on Server sends it again.
    run(&mut s, "rules.keepOnServer", json!({"id": id}));
    assert!(rule(&s, id).waiting() && rule(&s, id).refused.is_empty());
    assert_eq!(rule_upload(&s.mailbox, account).unwrap().ops.len(), 1);
    // A rule the server can't do isn't sent.
    let flag = add(&mut s, account, "Flag", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}]));
    let said = s.run("rules.keepOnServer", &json!({"id": flag})).unwrap_err().to_string();
    assert!(said.contains("can't flag"), "{said}");
}

#[test]
fn a_rule_from_before_rules_had_accounts_runs_for_every_account_until_it_is_moved_to_a_server() {
    let (mut s, account, local, news) = session();
    s.mailbox.rules.push(Rule {
        id: 900,
        name: "Old".into(),
        enabled: true,
        conditions: vec![Condition::FromContains(vec!["Weekly".into()])],
        actions: vec![Action::MarkRead],
        ..Default::default()
    });
    // By the sender's name too, on any account's mail.
    for a in [account, local] {
        let m = arriving(&mut s, a, "news@example.org", "x");
        assert_eq!(s.mailbox.apply_rules(m, &s.rule_clock()), ["Old"], "account {a}");
    }
    assert_eq!(run(&mut s, "rules.list", json!({"account": local}))["rules"][0]["kept"], "everyAccount");
    run(&mut s, "rules.keepOnServer", json!({"id": 900}));
    assert_eq!((rule(&s, 900).account, rule(&s, 900).keep), (account, Keep::Server));
    // Now the Microsoft account's alone, and by the address, as its server reads it.
    let there = arriving(&mut s, local, "news@example.org", "x");
    assert!(s.mailbox.apply_rules(there, &s.rule_clock()).is_empty());
    let by_name = arriving(&mut s, account, "news@example.org", "x");
    assert!(s.mailbox.apply_rules(by_name, &s.rule_clock()).is_empty());
    let mut s2 = session().0;
    let a2 = s2.mailbox.accounts.iter().find(|a| s2.mailbox.keeps_rules(a.id)).unwrap().id;
    s2.run("account.add", &json!({"email": "second@example.org", "kind": "microsoft"})).unwrap();
    s2.mailbox.rules.push(Rule {
        id: 901,
        name: "Old".into(),
        enabled: true,
        conditions: vec![Condition::HasAttachment],
        actions: vec![Action::MarkRead],
        ..Default::default()
    });
    // Two Microsoft accounts: which one is to be said.
    assert!(s2.run("rules.keepOnServer", &json!({"id": 901})).is_err());
    run(&mut s2, "rules.keepOnServer", json!({"id": 901, "account": a2}));
    assert_eq!(s2.mailbox.rules.iter().find(|r| r.id == 901).unwrap().account, a2);
    let _ = news;
}

#[test]
fn a_lost_answer_doesnt_make_a_rule_twice_and_a_rule_deleted_while_made_is_deleted_there() {
    let (mut s, account, _, news) = session();
    let id = news_rule(&mut s, account, news);
    // Its creation goes out (the window marks it as the job starts) and no answer comes: the
    // connection dropped, or MailCraft was closed. It isn't sent again until the server's rules
    // have been read whole, a download that isn't whole changing nothing of that.
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    assert!(rule(&s, id).unanswered);
    // An answer that stopped before it (the sign-in refused part-way) leaves it so too.
    apply_rule_changes(&mut s, &answered(account, Vec::new())).unwrap();
    assert!(rule(&s, id).unanswered && rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    let failed = |rule: Id| RuleOutcome { kind: RuleOpKind::Create, rule, result: RuleResult::Failed, ..Default::default() };
    apply_rule_changes(&mut s, &answered(account, vec![failed(id)])).unwrap();
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    merge_rules(&mut s.mailbox, &RulesDownload { complete: false, ..download(account, vec![]) }).unwrap();
    assert!(rule(&s, id).unanswered && rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    // The server made it: its download shows it, it is taken as this one, and that is saved.
    let mut there = remote("r9", "News", 1);
    there.conditions = vec![Condition::FromContains(vec!["DIGEST".into()])];
    there.actions.clear();
    there.move_to = "news".into();
    let rev = s.rev();
    let merged = apply_rules(&mut s, &download(account, vec![there.clone()])).unwrap();
    assert_eq!((merged.added, merged.updated, s.mailbox.rules.len()), (0, 1, 1));
    assert_ne!(s.rev(), rev);
    assert!(rule(&s, id).on_server() && !rule(&s, id).unanswered);
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    // The server didn't make it: read whole without it, it is sent again.
    let again = add(&mut s, account, "Again", json!([{"type": "hasAttachment"}]), json!([{"type": "markRead"}]));
    apply_rule_changes(&mut s, &answered(account, vec![failed(again)])).unwrap();
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    merge_rules(&mut s.mailbox, &download(account, vec![there])).unwrap();
    let ops: Vec<(RuleOpKind, Id)> = rule_upload(&s.mailbox, account).unwrap().ops.iter().map(|op| (op.kind, op.rule)).collect();
    assert_eq!(ops, [(RuleOpKind::Create, again)]);
    // An answer giving it the id another rule here has is no answer: deleting the one would
    // delete the other there.
    let twin = RuleOutcome { kind: RuleOpKind::Create, rule: again, id: "r9".into(), result: RuleResult::Done, ..Default::default() };
    apply_rule_changes(&mut s, &answered(account, vec![twin])).unwrap();
    assert!(rule(&s, again).waiting() && rule(&s, again).unanswered);
    // Made while it was being deleted here.
    let gone = add(&mut s, account, "Gone", json!([{"type": "hasAttachment"}]), json!([{"type": "markRead"}]));
    run(&mut s, "rules.remove", json!({"id": gone}));
    let made = RuleOutcome { kind: RuleOpKind::Create, rule: gone, id: "r10".into(), result: RuleResult::Done, ..Default::default() };
    apply_rule_changes(&mut s, &answered(account, vec![made])).unwrap();
    assert_eq!(s.mailbox.rule_gone.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(), ["r10"]);
}

#[test]
fn what_is_for_another_account_or_hostile_changes_nothing_it_shouldnt() {
    let (mut s, account, local, _) = session();
    let stranger = RulesDownload { email: "someone@example.org".into(), ..download(account, vec![remote("r1", "x", 1)]) };
    assert!(merge_rules(&mut s.mailbox, &stranger).is_err());
    assert!(merge_rules(&mut s.mailbox, &RulesDownload { account: local, email: String::new(), complete: true, rules: vec![] }).is_err());
    assert!(rule_upload(&s.mailbox, local).is_err());
    let mut hostile = vec![remote("", "No id", 1), remote("r1", "One", 1), remote("r1", "Twice", 2), remote("bad id", "Space", 3)];
    let mut long = remote("r2", &"n".repeat(10_000), 4);
    long.conditions = vec![Condition::SubjectContains(vec!["w".repeat(10_000)]), Condition::BodyContains(vec![String::new()])];
    long.actions = vec![Action::MoveTo(1), Action::Categorize("\u{0}".into())];
    long.unknown = vec!["x".repeat(500); 100];
    hostile.push(long);
    let merged = merge_rules(&mut s.mailbox, &download(account, hostile)).unwrap();
    assert_eq!((merged.added, merged.skipped), (2, 3));
    let long = s.mailbox.rules.iter().find(|r| r.name.starts_with('n')).unwrap();
    assert!(long.name.chars().count() <= mailcraft_model::MAX_NAME);
    assert_eq!(long.conditions.len(), 1);
    assert!(matches!(&long.conditions[0], Condition::SubjectContains(t) if t.iter().all(|w| w.chars().count() <= 200)));
    assert!(long.actions.is_empty());
    // What was cut (words too long, a condition with no words, a category with no name) is
    // named with the parts not known here: the rule is never made again from what is left.
    assert_eq!(long.server.as_ref().unwrap().unknown.len(), 4, "{:?}", long.server);
    assert!(!long.known());
    // So is a part the server doesn't name.
    let mut unnamed = remote("r3", "Unnamed", 5);
    unnamed.unknown = vec![" ".into(), "\u{0}\u{1}".into()];
    merge_rules(&mut s.mailbox, &RulesDownload { complete: false, ..download(account, vec![unnamed]) }).unwrap();
    assert!(!s.mailbox.rules.iter().find(|r| r.name == "Unnamed").unwrap().known());
    // And so is an action no server's rule takes, which would act here at Run Rules Now.
    for (n, action) in [Action::FlagFor(mailcraft_model::FollowUp::Today), Action::ClearFlag, Action::ClearCategories].into_iter().enumerate() {
        let mut odd = remote(&format!("r{}", 10 + n), &format!("Odd {n}"), 10 + n as i32);
        odd.actions = vec![Action::MarkRead, action.clone()];
        merge_rules(&mut s.mailbox, &RulesDownload { complete: false, ..download(account, vec![odd]) }).unwrap();
        let r = s.mailbox.rules.iter().find(|r| r.name == format!("Odd {n}")).unwrap();
        assert!(!r.known() && !r.actions.contains(&action), "{action:?}: {r:?}");
    }
    // Junk params are refused and change nothing (and never panic: crates/engine/src/tests.rs
    // runs every command with junk).
    let before = serde_json::to_value(&s.mailbox).unwrap();
    for params in [json!({}), json!({"id": "x"}), json!({"id": 1, "direction": "sideways"}), json!({"account": -1})] {
        assert!(s.run("rules.move", &params).is_err());
        assert!(s.run("rules.keepOnServer", &params).is_err());
        assert!(s.run("sync.ruleChanges", &params).is_err());
        assert!(s.run("sync.applyRules", &json!({"download": params})).is_err());
        assert!(s.run("sync.applyRuleChanges", &json!({"result": params})).is_err());
    }
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
}

#[test]
fn a_rule_never_sent_isnt_taken_for_one_made_elsewhere_that_looks_the_same() {
    let (mut s, account, _, _) = session();
    let id = add(&mut s, account, "Invoices", json!([{"type": "subjectContains", "value": "invoice"}]), json!([{"type": "markRead"}]));
    // The same rule, made on the web and switched off there.
    let mut web = remote("web-1", "Invoices", 1);
    web.enabled = false;
    merge_rules(&mut s.mailbox, &download(account, vec![web])).unwrap();
    assert!(rule(&s, id).waiting() && rule(&s, id).enabled);
    assert_eq!(s.mailbox.rules.len(), 2);
    // Deleting this one here deletes nothing there.
    run(&mut s, "rules.remove", json!({"id": id}));
    assert!(s.mailbox.rule_gone.is_empty());
}

#[test]
fn removing_an_account_takes_its_rules_with_it() {
    let (mut s, account, local, news) = session();
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Server's", 1)])).unwrap();
    add(&mut s, account, "Mine", json!([{"type": "subjectContains", "value": "digest"}]), json!([{"type": "delete"}]));
    news_rule(&mut s, account, news);
    add(&mut s, local, "Local", json!([{"type": "hasAttachment"}]), json!([{"type": "markRead"}]));
    // A rule for every account's mail, as a mailbox from before rules had accounts holds.
    let everyone = add(&mut s, local, "Everyone", json!([{"type": "hasAttachment"}]), json!([{"type": "markRead"}]));
    s.mailbox.rules.iter_mut().find(|r| r.id == everyone).unwrap().account = 0;
    run(&mut s, "account.remove", json!({"id": account}));
    let left: Vec<(&str, Id)> = s.mailbox.rules.iter().map(|r| (r.name.as_str(), r.account)).collect();
    assert_eq!(left, [("Local", local), ("Everyone", 0)]);
    // So the account's "delete the digests" acts on no other account's mail.
    let m = arriving(&mut s, local, "digest@news.example", "Weekly digest");
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert_eq!(s.mailbox.message(m).map(|m| m.folder), s.mailbox.folder_of_kind(local, FolderKind::Inbox));
}

#[test]
fn a_value_of_the_wrong_kind_is_refused_and_changes_nothing() {
    let (mut s, account, _, news) = session();
    news_rule(&mut s, account, news);
    s.mailbox.rules.push(Rule {
        id: 900,
        name: "Old".into(),
        enabled: true,
        conditions: vec![Condition::HasAttachment],
        actions: vec![Action::MarkRead],
        ..Default::default()
    });
    let m = arriving(&mut s, account, "digest@news.example", "x");
    run(&mut s, "mail.select", json!({"id": m}));
    let new = |extra: Value| {
        let mut v = json!({"name": "S", "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "markRead"}]});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    let before = serde_json::to_value(&s.mailbox).unwrap();
    let junk = [json!("64"), json!(0), json!(null), json!(-1), json!(1.5), json!([1])];
    let mut asked: Vec<(&str, Value)> = Vec::new();
    for j in &junk {
        asked.push(("rules.add", new(json!({"account": j}))));
        asked.push(("rules.add", new(json!({"account": account, "server": j}))));
        asked.push(("rules.add", new(json!({"account": account, "stop": j}))));
        asked.push(("rules.keepOnServer", json!({"id": 900, "account": j})));
        asked.push(("rules.move", json!({"id": 900, "direction": "down", "account": j})));
        asked.push(("rules.fromMessage", json!({"folder": news, "id": m, "server": j})));
        asked.push(("rules.run", json!({"id": j})));
        asked.push(("rules.list", json!({"account": j})));
    }
    for (command, params) in asked {
        let said = s.run(command, &params);
        assert!(matches!(said, Err(crate::CmdError::Params(_))), "{command} {params}: {said:?}");
    }
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
}

#[test]
fn a_rule_that_looks_for_a_name_stays_on_this_computer_and_reads_names() {
    let (mut s, account, _, news) = session();
    let by_name = json!([{"type": "fromContains", "value": "Weekly Digest"}]);
    let made = run(
        &mut s,
        "rules.add",
        json!({"name": "Digest", "account": account, "conditions": by_name, "actions": [{"type": "moveTo", "value": news}]}),
    );
    assert_eq!(made["keep"], "here");
    let listed = run(&mut s, "rules.list", json!({"account": account}));
    assert_eq!(listed["rules"][0]["why"], "it looks for a name, and the server looks at addresses only");
    assert!(
        s.run("rules.add", &json!({"name": "D", "account": account, "server": true, "conditions": by_name, "actions": [{"type": "markRead"}]}))
            .is_err()
    );
    // Kept here, it reads the sender's name, as a rule of this computer does.
    let m = arriving(&mut s, account, "news@example.org", "x");
    assert_eq!(s.mailbox.apply_rules(m, &s.rule_clock()), ["Digest"]);
    // A rule for every account that looks for a name isn't moved to a server, which would read
    // addresses alone and so never match.
    s.mailbox.rules.push(Rule {
        id: 900,
        name: "Old".into(),
        enabled: true,
        conditions: vec![Condition::FromContains(vec!["Weekly Digest".into()])],
        actions: vec![Action::MarkRead],
        ..Default::default()
    });
    let said = s.run("rules.keepOnServer", &json!({"id": 900})).unwrap_err().to_string();
    assert!(said.contains("looks for a name"), "{said}");
    assert_eq!((rule(&s, 900).account, rule(&s, 900).keep), (0, Keep::Here));
}

#[test]
fn always_move_messages_from_a_sender_of_a_shared_mailbox_makes_no_rule() {
    use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult};
    let (mut s, account, _, news) = session();
    let shared = run(&mut s, "account.addSharedMailbox", json!({"address": "team@example.org"}))["id"].as_u64().unwrap();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("t-in", "Inbox", FolderKind::Inbox), folder("t-ar", "Archive", FolderKind::Archive)];
    let listing = GraphMailResult {
        account: shared,
        email: "team@example.org".into(),
        listing: Some(FolderListing { folders, complete: true }),
        ..Default::default()
    };
    crate::cmd::graph_mail::apply_graph_mail(&mut s, listing).unwrap();
    let team_inbox = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    let team_archive = s.mailbox.folder_of_kind(shared, FolderKind::Archive).unwrap();
    let m = s.mailbox.add_message(Message { folder: team_inbox, from: Address::new("Club", "news@club.example"), ..Default::default() }).unwrap();
    for folder in [team_archive, news] {
        let said = s.run("rules.fromMessage", &json!({"folder": folder, "id": m})).unwrap_err().to_string();
        assert!(said.contains("shared mailbox"), "{said}");
    }
    assert!(s.mailbox.rules.is_empty());
    let _ = account;
}

#[test]
fn always_move_messages_from_this_sender_runs_the_new_rule_alone() {
    let (mut s, account, _, news) = session();
    add(&mut s, account, "Read", json!([{"type": "subjectContains", "value": "week"}]), json!([{"type": "markRead"}]));
    let m = arriving(&mut s, account, "digest@news.example", "This week");
    let made = run(&mut s, "rules.fromMessage", json!({"folder": news, "id": m}));
    assert_eq!(made["matched"], 1);
    let after = s.mailbox.message(m).unwrap();
    assert_eq!((after.folder, after.read), (news, false));
}

#[test]
fn a_rule_the_server_gives_back_in_its_own_order_and_capitals_is_the_same_rule() {
    let (mut s, account, _, news) = session();
    let id = add(
        &mut s,
        account,
        "Digest",
        json!([{"type": "fromContains", "value": "digest"}, {"type": "hasAttachment"}]),
        json!([{"type": "categorize", "value": "Blue category"}, {"type": "markRead"}, {"type": "moveTo", "value": news}]),
    );
    all_taken(&mut s, account);
    let made = rule(&s, id).clone();
    // The server keeps the words in capitals, and gives the parts in an order of its own.
    let there = RemoteRule {
        id: format!("made-{id}"),
        name: "Digest".into(),
        sequence: made.server.as_ref().unwrap().sequence,
        enabled: true,
        conditions: vec![Condition::HasAttachment, Condition::FromContains(vec!["DIGEST".into()])],
        actions: vec![Action::MarkRead, Action::Categorize("BLUE CATEGORY".into())],
        move_to: "news".into(),
        ..Default::default()
    };
    let merged = merge_rules(&mut s.mailbox, &download(account, vec![there])).unwrap();
    assert_eq!((merged.updated, merged.added, merged.removed), (0, 0, 0), "{merged:?}");
    assert_eq!(rule(&s, id).conditions, made.conditions);
    assert_eq!(rule(&s, id).actions, made.actions);
}

#[test]
fn always_move_messages_from_a_sender_of_a_microsoft_account_goes_to_its_server() {
    let (mut s, account, _, news) = session();
    let m = arriving(&mut s, account, "digest@news.example", "This week");
    let made = run(&mut s, "rules.fromMessage", json!({"folder": news, "id": m}));
    let id = made["id"].as_u64().unwrap();
    let r = rule(&s, id);
    assert_eq!((r.account, r.keep), (account, Keep::Server));
    assert!(r.waiting());
    // It ran here at once, as it does until the server has it.
    assert_eq!(made["matched"], 1);
    assert_eq!(s.mailbox.message(m).unwrap().folder, news);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    assert_eq!(upload.ops.len(), 1);
    assert_eq!(upload.ops[0].body.as_ref().unwrap().move_to, "news");
    // Kept on this computer when asked, as `rules.add` is.
    let other = arriving(&mut s, account, "shop@news.example", "Sale");
    let here = run(&mut s, "rules.fromMessage", json!({"folder": news, "id": other, "server": false}));
    assert_eq!(rule(&s, here["id"].as_u64().unwrap()).keep, Keep::Here);
}

#[test]
fn the_owner_says_whether_a_new_rule_is_kept_on_the_server_and_is_told_why_it_cant_be() {
    let (mut s, account, local, news) = session();
    let digest = json!([{"type": "fromContains", "value": "digest"}]);
    // Kept on this computer, though the server could keep it: nothing of it is sent.
    let made = run(
        &mut s,
        "rules.add",
        json!({"name": "News", "account": account, "server": false, "conditions": digest, "actions": [{"type": "moveTo", "value": news}]}),
    );
    assert_eq!(made["keep"], "here");
    let id = made["id"].as_u64().unwrap();
    assert_eq!((rule(&s, id).keep, rule(&s, id).waiting()), (Keep::Here, false));
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    let listed = run(&mut s, "rules.list", json!({"account": account}));
    let entry = listed["rules"].as_array().unwrap().iter().find(|r| r["id"] == id).unwrap();
    assert_eq!((entry["kept"].as_str(), entry.get("why")), (Some("here"), None));
    // Asked for the server, a rule it can't keep is refused, saying why; left out, it stays here.
    let flag = json!({"name": "Boss", "account": account, "server": true, "conditions": digest, "actions": [{"type": "flag"}]});
    let refused = s.run("rules.add", &flag).unwrap_err().to_string();
    assert!(refused.contains("can't flag"), "{refused}");
    let mut left_out = flag.clone();
    left_out.as_object_mut().unwrap().remove("server");
    assert_eq!(run(&mut s, "rules.add", left_out)["keep"], "here");
    // So is a move out of the account (to one on this computer alone), and a server for a rule of an account whose server keeps
    // none.
    let inbox_here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let out = json!({"name": "Out", "account": account, "server": true, "conditions": digest, "actions": [{"type": "moveTo", "value": inbox_here}]});
    assert!(s.run("rules.add", &out).unwrap_err().to_string().contains("on this computer"));
    assert!(
        s.run("rules.add", &json!({"name": "Here", "account": local, "server": true, "conditions": digest, "actions": [{"type": "markRead"}]}))
            .is_err()
    );
    assert_eq!(
        run(&mut s, "rules.add", json!({"name": "Here", "account": local, "server": false, "conditions": digest, "actions": [{"type": "markRead"}]}))
            ["keep"],
        "here"
    );
    // What a form says before the rule is made, unfinished parts left out.
    let draft = |actions: Value| {
        super::rules::server_cannot_draft(&s, &json!({"account": account, "conditions": [{"type": "fromContains", "value": ""}], "actions": actions}))
    };
    assert_eq!(draft(json!([{"type": "moveTo", "value": news}])), None);
    assert_eq!(draft(json!([{"type": "moveTo", "value": 0}])), None);
    assert_eq!(draft(json!([{"type": "markRead"}, {"type": "flag"}])), Some(super::rules::NotOnServer::Flag));
    assert_eq!(draft(json!([{"type": "moveTo", "value": inbox_here}])), Some(super::rules::NotOnServer::FolderHere));
    assert_eq!(
        super::rules::server_cannot_draft(&s, &json!({"account": local, "actions": [{"type": "markRead"}]})),
        Some(super::rules::NotOnServer::NoServer)
    );
    assert_eq!(super::rules::server_cannot_draft(&s, &json!({"account": 9_999})), None);
    assert_eq!(super::rules::server_cannot_draft(&s, &json!("junk")), None);
}

#[test]
fn the_server_keeps_the_new_parts_it_can_and_says_why_not_for_the_rest() {
    use super::rules::{NotOnServer, server_cannot};
    use mailcraft_model::{FollowUp, Importance, Sensitivity, time::Date};
    let (mut s, account, local, news) = session();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let unsent = run(&mut s, "folder.new", json!({"name": "Not yet", "parent": inbox}));
    let unsent = unsent["id"].as_u64().unwrap();
    let base = Rule { account, conditions: vec![Condition::SentOnlyToMe], actions: vec![Action::MarkRead], ..Default::default() };
    let cannot = |r: Rule| server_cannot(&s.mailbox, &r, account);
    // All of these the server does.
    let kept = Rule {
        conditions: vec![
            Condition::From(vec!["ada@example.org".into(), "noor@example.org".into()]),
            Condition::SentTo(vec!["team@example.org".into()]),
            Condition::ToOrCcMe,
            Condition::SubjectOrBodyContains(vec!["agenda".into(), "minutes".into()]),
            Condition::HeaderContains(vec!["X-Club".into()]),
            Condition::Sensitivity(Sensitivity::Private),
            Condition::SizeBetween { min: 0, max: 512 },
            Condition::MeetingRequest,
            Condition::AutomaticReply,
            Condition::Category(vec!["Red category".into()]),
            Condition::Flagged,
        ],
        exceptions: vec![Condition::SubjectContains(vec!["unsubscribe".into()]), Condition::Importance(Importance::Low)],
        actions: vec![Action::CopyTo(news), Action::MarkRead],
        ..base.clone()
    };
    assert_eq!(cannot(kept), None);
    let day = Date::parse_iso("2026-10-01");
    for (rule, why) in [
        (Rule { conditions: vec![Condition::ReceivedBetween { from: day, to: None }], ..base.clone() }, NotOnServer::Days),
        (Rule { exceptions: vec![Condition::ReceivedBetween { from: None, to: day }], ..base.clone() }, NotOnServer::Days),
        (Rule { exceptions: vec![Condition::FromContains(vec!["Noor Smith".into()])], ..base.clone() }, NotOnServer::Names),
        (Rule { alert: true, ..base.clone() }, NotOnServer::Alert),
        (Rule { actions: vec![Action::CopyTo(here)], ..base.clone() }, NotOnServer::CopyHere),
        (Rule { actions: vec![Action::CopyTo(unsent)], ..base.clone() }, NotOnServer::CopyNotThere),
        (Rule { actions: vec![Action::CopyTo(999_999)], ..base.clone() }, NotOnServer::CopyNotThere),
        (Rule { actions: vec![Action::CopyTo(news), Action::CopyTo(news)], ..base.clone() }, NotOnServer::TwoCopies),
        (Rule { actions: vec![Action::ClearCategories], ..base.clone() }, NotOnServer::ClearCategories),
        (Rule { actions: vec![Action::FlagFor(FollowUp::Today)], ..base.clone() }, NotOnServer::Flag),
        (Rule { actions: vec![Action::ClearFlag], ..base.clone() }, NotOnServer::ClearFlag),
        (
            Rule { exceptions: vec![Condition::Importance(Importance::Low), Condition::Importance(Importance::High)], ..base.clone() },
            NotOnServer::TwoExceptionsOfAKind,
        ),
    ] {
        assert_eq!(cannot(rule.clone()), Some(why), "{:?}", (rule.conditions, rule.exceptions, rule.actions, rule.alert));
    }
    // The same through the command: such a rule is made here, and says why.
    let made =
        run(&mut s, "rules.add", json!({"name": "Unflag", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "clearFlag"}]}));
    assert_eq!(made["keep"], "here");
    let refused = s.run(
        "rules.add",
        &json!({"name": "Unflag", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "clearFlag"}], "server": true}),
    );
    assert!(refused.is_err_and(|e| e.to_string().contains("clear a flag")));
}

#[test]
fn exceptions_of_a_kind_go_up_as_one_and_a_lost_answer_finds_the_rule_again() {
    let (mut s, account, _, news) = session();
    let made = run(
        &mut s,
        "rules.add",
        json!({
            "name": "Club",
            "account": account,
            "conditions": [{"type": "from", "value": "club@example.org"}],
            "exceptions": [{"type": "subjectContains", "value": "unsubscribe"}, {"type": "subjectContains", "value": "opt out"}, {"type": "hasAttachment"}],
            "actions": [{"type": "copyTo", "value": news}, {"type": "markRead"}],
        }),
    );
    assert_eq!(made["keep"], "server");
    let id = made["id"].as_u64().unwrap();
    assert_eq!(rule(&s, id).exceptions, [Condition::SubjectContains(vec!["unsubscribe".into(), "opt out".into()]), Condition::HasAttachment]);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    let body = upload.ops[0].body.clone().unwrap();
    assert_eq!((body.copy_to.as_str(), body.exceptions.len()), ("news", 2));
    // The answer is lost; the server has it, in its own order and capitals.
    mark_sent(&mut s, &upload);
    let mut there = remote("r7", "Club", 1);
    there.conditions = vec![Condition::From(vec!["CLUB@EXAMPLE.ORG".into()])];
    there.exceptions = vec![Condition::HasAttachment, Condition::SubjectContains(vec!["OPT OUT".into(), "UNSUBSCRIBE".into()])];
    there.copy_to = "news".into();
    let merged = apply_rules(&mut s, &download(account, vec![there])).unwrap();
    assert_eq!((merged.added, merged.updated, s.mailbox.rules.len()), (0, 1, 1));
    assert!(rule(&s, id).on_server() && rule(&s, id).known());
    assert_eq!(rule(&s, id).actions, [Action::CopyTo(news), Action::MarkRead]);
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
}

#[test]
fn a_rule_this_version_couldnt_show_before_is_shown_once_its_parts_are_known() {
    let (mut s, account, _, news) = session();
    // As the version before read it: its exceptions and its copy unknown, so only switched,
    // moved and deleted.
    let mut before = remote("r1", "Club", 1);
    before.unknown = vec!["exceptions".into(), "copyToFolder".into()];
    merge_rules(&mut s.mailbox, &download(account, vec![before])).unwrap();
    let id = s.mailbox.rules[0].id;
    assert!(!rule(&s, id).known());
    // Read whole by this one: all of it is known.
    let mut now = remote("r1", "Club", 1);
    now.exceptions = vec![Condition::Flagged];
    now.copy_to = "news".into();
    merge_rules(&mut s.mailbox, &download(account, vec![now])).unwrap();
    let r = rule(&s, id);
    assert!(r.known(), "{:?}", r.server);
    assert_eq!(
        (r.exceptions.as_slice(), r.actions.as_slice()),
        ([Condition::Flagged].as_slice(), [Action::CopyTo(news), Action::MarkRead].as_slice())
    );
}

/// New mail not downloaded yet, which a rule copies into another account and then deletes: the
/// rule waits for the download and deletes nothing before; once the message is whole, it copies it
/// and goes on.
#[test]
fn mail_a_rule_copies_into_another_account_isnt_deleted_before_it_is_whole() {
    let (mut s, account, local, _) = session();
    let elsewhere = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let actions = json!([{"type": "copyTo", "value": elsewhere}, {"type": "delete"}]);
    run(
        &mut s,
        "rules.add",
        json!({"name": "Keep and delete", "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "x"}], "actions": actions}),
    );
    // Not downloaded yet, so it can't be copied to another mailbox.
    let m = arriving(&mut s, account, "a@example.org", "x");
    s.mailbox.message_mut(m).unwrap().unfetched = true;
    let before = s.mailbox.message(m).unwrap().folder;
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert_eq!(s.mailbox.message(m).map(|m| m.folder), Some(before));
    // Whole, it is copied and the rule goes on.
    let m = arriving(&mut s, account, "a@example.org", "x");
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert_eq!(s.mailbox.message(m).map(|m| m.folder), Some(deleted));
    assert!(s.mailbox.messages.values().any(|c| c.folder == elsewhere));
}

/// A rule that copies mail onto another account's server and deletes it is asked as a send, and
/// a yes to that makes it; the question about running the rules says which servers they move
/// mail onto and which they copy mail onto, each account once.
#[test]
fn an_agent_is_asked_as_a_send_for_a_rule_that_copies_onto_another_server_and_deletes() {
    use crate::agent::{Class, Verdict, check};
    let (mut s, account, _, _) = session();
    let other = run(&mut s, "account.add", json!({"email": "other@example.org", "kind": "microsoft"}))["id"].as_u64().unwrap();
    let there = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    let params = json!({
        "name": "Both", "account": account, "conditions": [{"type": "toMe"}],
        "actions": [{"type": "copyTo", "value": there}, {"type": "delete"}],
    });
    assert!(matches!(check(&s, "rules.add", &params, &[]), Verdict::Ask(Class::Send, _)));
    assert_eq!(check(&s, "rules.add", &params, &[Class::Send]), Verdict::Run);
    let asked = crate::agent::describe(&s, "rules.add", &params);
    assert!(asked.contains("copy it onto the server of other@example.org"), "{asked}");
    // Run over the Inbox, the question says the rule copies.
    run(&mut s, "rules.add", params.clone());
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let running = json!({"folder": inbox});
    let asked = crate::agent::describe(&s, "rules.run", &running);
    assert_eq!(check(&s, "rules.run", &running, &[]), Verdict::Ask(Class::Send, asked.clone()));
    assert!(asked.contains(", which copy mail onto the server of other@example.org") && !asked.contains("move"), "{asked}");
    // Rules that move mail there are said apart from those that copy it, each account once.
    let archive = s.mailbox.folder_of_kind(other, FolderKind::Archive).unwrap();
    add(&mut s, account, "Moves", json!([{"type": "toMe"}]), json!([{"type": "moveTo", "value": archive}]));
    add(&mut s, account, "Moves too", json!([{"type": "toMe"}]), json!([{"type": "moveTo", "value": archive}]));
    add(&mut s, account, "Copies too", json!([{"type": "toMe"}]), json!([{"type": "copyTo", "value": archive}]));
    let asked = crate::agent::describe(&s, "rules.run", &running);
    assert!(asked.contains(", which move mail onto the server of other@example.org and copy mail onto the server of other@example.org"), "{asked}");
}

#[test]
fn a_rule_filing_within_its_own_account_isnt_said_to_go_onto_another_server() {
    // With another account whose mail could be filed there, the question said a rule of the
    // account's own moved its mail "onto the server of" that same account.
    let (mut s, account, _, _) = session();
    let other = run(&mut s, "account.add", json!({"email": "other@example.org", "kind": "microsoft"}))["id"].as_u64().unwrap();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let mut rule = json!({"name": "File", "account": account, "server": false, "conditions": [{"type": "toMe"}],
        "actions": [{"type": "moveTo", "value": archive}, {"type": "copyTo", "value": archive}]});
    let asked = crate::agent::describe(&s, "rules.add", &rule);
    assert!(asked.contains("move it to ") && asked.contains("copy it to ") && !asked.contains("onto the server"), "{asked}");
    // A rule for the other account's mail does put it there.
    rule["account"] = json!(other);
    let asked = crate::agent::describe(&s, "rules.add", &rule);
    assert!(asked.contains("move it onto the server of ") && asked.contains("copy it onto the server of "), "{asked}");
}

/// A rule that copies or moves this computer's mail onto the Microsoft account's server is a
/// send for an agent to make; deleted by the owner, an agent's Undo brings it back, and it files
/// the next mail there. That undo is asked as a send too, as `rules.add` of the same rule is.
#[test]
fn an_agents_undo_that_brings_back_a_rule_filing_onto_another_server_is_asked_as_a_send() {
    use crate::agent::{Access, Class, classify, describe};
    let mut found = Vec::new();
    for kind in ["copyTo", "moveTo"] {
        let (mut s, account, local, news) = session();
        let params = json!({"name": "Up", "account": local, "conditions": [{"type": "fromContains", "value": "news@example.org"}], "actions": [{"type": kind, "value": news}]});
        assert_eq!(classify(&s, "rules.add", &params), Access::Owner(Class::Send), "{kind}");
        let id = run(&mut s, "rules.add", params)["id"].as_u64().unwrap();
        run(&mut s, "rules.remove", json!({"id": id}));
        let undo = classify(&s, "edit.undo", &json!({}));
        let said = describe(&s, "edit.undo", &json!({}));
        run(&mut s, "edit.undo", json!({}));
        // The rule is back, and the next mail of this computer goes onto the Microsoft server.
        let m = arriving(&mut s, local, "news@example.org", "Weekly");
        s.mailbox.apply_rules(m, &s.rule_clock());
        let up = s.mailbox.queued.iter().filter(|q| matches!(q, mailcraft_model::Queued::PlaceMessage { account: a, .. } if *a == account)).count();
        assert_eq!(up, 1, "{kind}: on its way onto the server");
        found.push(format!("{kind}: {undo:?}, said: {said}"));
    }
    assert!(found.iter().all(|f| f.contains("Owner(Send)")), "an agent's undo that brings the rule back is asked as a send: {found:#?}");
    assert!(found.iter().all(|f| f.contains("so that new mail is filed onto the server of")), "{found:#?}");
}

/// A rule that copies this computer's mail onto the Microsoft account's server: another change to
/// the rules for that mail may let more of it reach the rule, and an agent's change is asked as a
/// send (#566).
#[test]
fn an_agent_asks_the_owner_before_changing_the_rules_beside_one_that_puts_mail_on_another_server() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, account, local, news) = session();
    let up = add(&mut s, local, "Up", json!([{"type": "fromContains", "value": "news@example.org"}]), json!([{"type": "copyTo", "value": news}]));
    let other = add(&mut s, local, "Other", json!([{"type": "toMe"}]), json!([{"type": "markRead"}]));
    let theirs = add(&mut s, account, "Theirs", json!([{"type": "toMe"}]), json!([{"type": "markRead"}]));
    let (send, settings) = (Access::Owner(Class::Send), Access::Owner(Class::Settings));
    for (id, params) in [
        ("rules.move", json!({"id": other, "direction": "up"})),
        ("rules.enable", json!({"id": other, "value": false})),
        ("rules.remove", json!({"id": other})),
        ("rules.add", json!({"name": "More", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]})),
    ] {
        assert_eq!(classify(&s, id, &params), send, "{id} {params}");
        let said = describe(&s, id, &params);
        assert!(said.contains(&format!("; the rule \u{201c}Up\u{201d}, switched on, puts mail onto the server of {EMAIL}")), "{id}: {said}");
    }
    // Up itself going off or going only puts less there.
    assert_eq!(classify(&s, "rules.enable", &json!({"id": up, "value": false})), settings);
    assert_eq!(classify(&s, "rules.remove", &json!({"id": up})), settings);
    // The Microsoft account's own mail never reaches it.
    assert_eq!(classify(&s, "rules.enable", &json!({"id": theirs, "value": false})), settings);
}

/// A new rule is always for one account's mail (#566): `rules.add` without `account` makes it for
/// the account of the folder in view, as `rules.move` reads it, and is refused when that folder
/// is no account's. A script or an agent can't make a rule for every account any more.
#[test]
fn a_rule_made_without_an_account_is_for_the_account_in_view() {
    let (mut s, account, local, news) = session();
    let params = |name: &str| json!({"name": name, "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "markRead"}]});
    s.view.folder = news;
    let made = run(&mut s, "rules.add", params("Microsoft's"));
    let id = made["id"].as_u64().unwrap();
    assert_eq!((rule(&s, id).account, made["keep"].as_str()), (account, Some("server")));
    s.view.folder = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let id = run(&mut s, "rules.add", params("Local"))["id"].as_u64().unwrap();
    assert_eq!(rule(&s, id).account, local);
    // The agent's question says what the command then makes.
    s.view.folder = news;
    let asked = crate::agent::describe(&s, "rules.add", &params("Asked"));
    assert!(asked.contains(EMAIL), "{asked}");
    // A folder that is no account's: refused, the rules as they were.
    let before = s.mailbox.rules.clone();
    s.view.folder = 0;
    let refused = s.run("rules.add", &params("Nobody's")).unwrap_err().to_string();
    assert!(refused.contains("`account`"), "{refused}");
    assert_eq!(s.mailbox.rules, before);
}

/// An undo of one account's rules reaches only that account's mail: a rule for every account that
/// files another account's mail onto this one's server isn't one the undo lets more mail reach,
/// so it is asked as a change of the rules, not as a send (#566).
#[test]
fn an_undo_of_one_accounts_rules_isnt_a_send_for_a_rule_of_every_account_filing_into_it() {
    use crate::agent::{Access, Class, classify};
    let (mut s, account, _, _) = session();
    run(&mut s, "account.add", json!({"email": "other@example.org", "kind": "microsoft"}));
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    s.mailbox.folder_mut(archive).unwrap().server = Some(ServerFolder { id: "arch".into(), name: "Archive".into(), ..Default::default() });
    // A rule for every account's mail, as a mailbox from before rules had accounts holds.
    let every = add(&mut s, account, "Everyone", json!([{"type": "hasAttachment"}]), json!([{"type": "copyTo", "value": archive}]));
    let r = s.mailbox.rules.iter_mut().find(|r| r.id == every).unwrap();
    (r.account, r.keep) = (0, Keep::Here);
    let mine = add(&mut s, account, "Mine", json!([{"type": "subjectContains", "value": "digest"}]), json!([{"type": "markRead"}]));
    run(&mut s, "rules.enable", json!({"id": mine, "value": false}));
    assert_eq!(classify(&s, "edit.undo", &json!({})), Access::Owner(Class::Settings));
}

/// The question about a change to the rules names every server the rules beside it put mail
/// onto: of two rules, and of one rule that copies onto one and moves onto another (#566).
#[test]
fn a_question_beside_rules_onto_two_servers_names_both_servers() {
    use crate::agent::{Access, Class, classify, describe};
    for one in [false, true] {
        let (mut s, _account, local, news) = session();
        let other = run(&mut s, "account.add", json!({"email": "other@example.org", "kind": "microsoft"}))["id"].as_u64().unwrap();
        let there = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
        if one {
            add(
                &mut s,
                local,
                "Both",
                json!([{"type": "fromContains", "value": "news@example.org"}]),
                json!([{"type": "copyTo", "value": news}, {"type": "moveTo", "value": there}]),
            );
        } else {
            add(&mut s, local, "Up", json!([{"type": "fromContains", "value": "news@example.org"}]), json!([{"type": "copyTo", "value": news}]));
            add(&mut s, local, "Away", json!([{"type": "fromContains", "value": "shop@example.org"}]), json!([{"type": "moveTo", "value": there}]));
        }
        let params = json!({"name": "More", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]});
        assert_eq!(classify(&s, "rules.add", &params), Access::Owner(Class::Send));
        let said = describe(&s, "rules.add", &params);
        assert!(said.contains(&format!("onto the server of {EMAIL}, other@example.org")), "{said}");
        // And so does the undo of a change beside them.
        let keep = add(&mut s, local, "Keep", json!([{"type": "toMe"}]), json!([{"type": "markRead"}]));
        run(&mut s, "rules.remove", json!({"id": keep}));
        assert_eq!(classify(&s, "edit.undo", &json!({})), Access::Owner(Class::Send));
        let said = describe(&s, "edit.undo", &json!({}));
        assert!(said.contains(&format!("onto the server of {EMAIL}, other@example.org")), "{said}");
    }
}

/// Run Rules Now over the Inbox of an account with no address names that account by its name,
/// as the Rules dialog does: every account has an Inbox (#566).
#[test]
fn the_run_rules_question_names_an_account_with_no_address_by_its_name() {
    use crate::agent::describe;
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": EMAIL, "kind": "microsoft"})).unwrap();
    let local = s.mailbox.local_account();
    assert!(s.mailbox.account(local).unwrap().email.is_empty());
    add(&mut s, local, "Purge", json!([{"type": "subjectContains", "value": "x"}]), json!([{"type": "delete"}]));
    let inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let said = describe(&s, "rules.run", &json!({"folder": inbox}));
    assert!(said.contains("“Inbox” of “On this computer”"), "{said}");
}

/// A rule's Delete moves new mail to Deleted Items, as Outlook's "delete it" does, and no
/// further: not when the same rule, or one before it, moved it there first; nor over Deleted
/// Items; and in an account with none it leaves the mail where it is (#566).
#[test]
fn a_rules_delete_of_new_mail_moved_to_deleted_items_leaves_it_there() {
    use crate::agent::{Access, Class, classify};
    let rule = |account: Id, name: &str, actions: Value| json!({"name": name, "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "x"}], "actions": actions});
    // One rule that does both, and two rules one after the other.
    for two in [false, true] {
        let (mut s, account, _, _) = session();
        let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
        let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let (bin, delete) = (json!({"type": "moveTo", "value": deleted}), json!({"type": "delete"}));
        let rules = if two {
            vec![rule(account, "Bin", json!([bin])), rule(account, "Gone", json!([delete]))]
        } else {
            vec![rule(account, "Both", json!([bin, delete]))]
        };
        for r in rules {
            assert_eq!(classify(&s, "rules.add", &r), Access::Owner(Class::Settings));
            run(&mut s, "rules.add", r);
        }
        let m = arriving(&mut s, account, "a@example.org", "x");
        s.mailbox.apply_rules(m, &s.rule_clock());
        assert_eq!(s.mailbox.message(m).map(|m| m.folder), Some(deleted), "two: {two}");
        // Run Rules Now over the Inbox: the same.
        let again = arriving(&mut s, account, "a@example.org", "x");
        run(&mut s, "rules.run", json!({"folder": inbox}));
        assert_eq!(s.mailbox.message(again).map(|m| m.folder), Some(deleted), "two: {two}");
    }
    // Over Deleted Items, the owner's run leaves it there, and an agent's is a change of the rules.
    let (mut s, account, _, _) = session();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    run(&mut s, "rules.add", rule(account, "Gone", json!([{"type": "delete"}])));
    let m = arriving(&mut s, account, "a@example.org", "x");
    s.mailbox.message_mut(m).unwrap().folder = deleted;
    assert_eq!(classify(&s, "rules.run", &json!({"folder": deleted})), Access::Owner(Class::Settings));
    run(&mut s, "rules.run", json!({"folder": deleted}));
    assert_eq!(s.mailbox.message(m).map(|m| m.folder), Some(deleted), "deleted for good from Deleted Items");
    // An account with no Deleted Items: the mail stays where it is, and an agent isn't asked about
    // a delete.
    s.mailbox.remove_message(m).unwrap();
    s.mailbox.folders.retain(|f| !(f.account == account && f.kind == FolderKind::Deleted));
    assert_eq!(classify(&s, "rules.add", &rule(account, "Gone", json!([{"type": "delete"}]))), Access::Owner(Class::Settings));
    let m = arriving(&mut s, account, "a@example.org", "x");
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert_eq!(s.mailbox.message(m).map(|m| m.folder), Some(inbox), "deleted for good with no Deleted Items");
}

/// An agent's question about a rule kept on this computer names whose mail it is for, an account
/// with no address by its name (#566).
#[test]
fn the_question_about_a_rule_kept_here_names_its_account() {
    use crate::agent::describe;
    let (s, account, _, _) = session();
    let rule = |account: Id| json!({"name": "Here", "account": account, "server": false, "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "flag"}]});
    let asked = describe(&s, "rules.add", &rule(account));
    assert!(asked.contains(&format!("the rule “Here” for the mail of “{EMAIL}”:")), "{asked}");
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": EMAIL, "kind": "microsoft"})).unwrap();
    let local = s.mailbox.local_account();
    assert!(s.mailbox.account(local).unwrap().email.is_empty());
    let asked = describe(&s, "rules.add", &rule(local));
    assert!(asked.contains("the rule “Here” for the mail of “On this computer”:"), "{asked}");
}

/// Words that differ only in the case of a letter outside A–Z ("Büro", "BÜRO") are one word, as
/// a rule matches them: kept once, so a server that keeps them in capitals is found to hold the
/// same rule after a lost answer, and it isn't made twice (#566).
#[test]
fn words_differing_in_a_non_ascii_letters_case_are_one_and_find_the_rule_again() {
    let (mut s, account, _, news) = session();
    let made = run(
        &mut s,
        "rules.add",
        json!({
            "name": "Office",
            "account": account,
            "conditions": [{"type": "from", "value": "club@example.org"}, {"type": "category", "value": ["Été", "ÉTÉ"]}],
            "exceptions": [{"type": "subjectContains", "value": "Büro"}, {"type": "subjectContains", "value": "BÜRO"}],
            "actions": [{"type": "copyTo", "value": news}],
        }),
    );
    assert_eq!(made["keep"], "server");
    let id = made["id"].as_u64().unwrap();
    assert_eq!(rule(&s, id).exceptions, [Condition::SubjectContains(vec!["Büro".into()])]);
    assert_eq!(rule(&s, id).conditions[1], Condition::Category(vec!["Été".into()]));
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    // The answer is lost; the server has it, in its own capitals.
    let mut there = remote("r8", "Office", 1);
    there.conditions = vec![Condition::From(vec!["CLUB@EXAMPLE.ORG".into()]), Condition::Category(vec!["ÉTÉ".into()])];
    there.exceptions = vec![Condition::SubjectContains(vec!["BÜRO".into(), "BÜRO".into()])];
    (there.actions, there.copy_to) = (Vec::new(), "news".into());
    // (Read as a download is.)
    there.tidy();
    let merged = apply_rules(&mut s, &download(account, vec![there])).unwrap();
    assert_eq!((merged.added, s.mailbox.rules.len()), (0, 1), "exceptions here: {:?}", rule(&s, id).exceptions);
}

/// An undo of a move of one account's rule past another account's rule only gives that rule
/// another place in the list: it is asked about as the move was, as settings, though the other
/// rule puts mail onto a server (#566).
#[test]
fn an_undo_of_a_move_past_another_accounts_rule_asks_as_the_move_did() {
    use crate::agent::{Access, Class, classify};
    let (mut s, account, local, news) = session();
    let x = add(&mut s, account, "X", json!([{"type": "subjectContains", "value": "x"}]), json!([{"type": "markRead"}]));
    add(&mut s, local, "Away", json!([{"type": "fromContains", "value": "shop@example.org"}]), json!([{"type": "moveTo", "value": news}]));
    let w = add(&mut s, account, "W", json!([{"type": "subjectContains", "value": "w"}]), json!([{"type": "markRead"}]));
    for r in [x, w] {
        s.mailbox.rules.iter_mut().find(|y| y.id == r).unwrap().keep = Keep::Here;
    }
    let down = json!({"id": x, "direction": "down", "account": account});
    assert_eq!(classify(&s, "rules.move", &down), Access::Owner(Class::Settings));
    run(&mut s, "rules.move", down);
    assert_eq!(s.mailbox.rules.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Away", "W", "X"]);
    assert_eq!(classify(&s, "edit.undo", &json!({})), Access::Owner(Class::Settings), "past a rule that moves mail onto a server");
}

/// An undo asked as a send because it brings back a meeting change that writes to people still
/// says so when it also brings back a rule beside one that files mail onto another server (#566).
#[test]
fn an_undos_send_question_names_the_meeting_change_beside_a_rule_onto_another_server() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, _, local, news) = session();
    add(&mut s, local, "Up", json!([{"type": "fromContains", "value": "news@example.org"}]), json!([{"type": "copyTo", "value": news}]));
    let keep = add(&mut s, local, "Keep", json!([{"type": "toMe"}]), json!([{"type": "markRead"}]));
    run(&mut s, "rules.remove", json!({"id": keep}));
    // The step was taken while a meeting update waited to go; it has gone since.
    let change = mailcraft_model::remote::EventChange { account: 1, event: 4_242, send: true, token: "t".into(), ..Default::default() };
    s.history.last_mut().unwrap().mailbox.event_changes.push(change);
    assert_eq!(classify(&s, "edit.undo", &json!({})), Access::Owner(Class::Send));
    let said = describe(&s, "edit.undo", &json!({}));
    assert!(said.contains("a change to a meeting") && said.contains("filed onto the server of"), "{said}");
}

/// The owner's yes to an agent's Run Rules Now reaches the mail that was in the folder when asked:
/// a message moved there while the question is up (by the agent itself, say) is never put onto
/// another server by it, nothing runs and the agent is to ask again; mail moved out meanwhile
/// doesn't stop it (#566).
#[test]
fn run_rules_nows_yes_reaches_only_the_mail_there_when_asked() {
    use crate::agent::bind::{Reach, Shown};
    use crate::agent::{Class, Verdict, check};
    let (mut s, _, local, news) = session();
    let archive = s.mailbox.folder_of_kind(local, FolderKind::Archive).unwrap();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Away", "account": local,
        "conditions": [{"type": "subjectContains", "value": "invoice"}], "actions": [{"type": "moveTo", "value": news}]}),
    );
    let put = |s: &mut Session, subject: &str| {
        s.mailbox
            .add_message(Message { folder: archive, from: Address::new("Shop", "shop@example.org"), subject: subject.into(), ..Default::default() })
            .unwrap()
    };
    let (one, two) = (put(&mut s, "invoice 1"), put(&mut s, "invoice 2"));
    let params = json!({"folder": archive});
    let Verdict::Ask(Class::Send, what) = check(&s, "rules.run", &params, &[]) else { panic!("asked as a send") };
    let shown = Shown::new(&s, "rules.run", &params, &what).unwrap();
    // One moved out meanwhile: the rest are still those asked about.
    s.mailbox.remove_message(two).unwrap();
    assert_eq!(shown.check(&s, "rules.run", &params), Ok(Reach::All));
    // One moved in meanwhile: nothing runs.
    let came = put(&mut s, "invoice 3");
    assert!(shown.check(&s, "rules.run", &params).is_err_and(|why| why.contains("came into it")));
    assert!(s.mailbox.message(one).is_some_and(|m| m.folder == archive) && s.mailbox.message(came).is_some_and(|m| m.folder == archive));
    // Another rule that moves mail onto a server, switched on meanwhile, changes what the yes would
    // do: nothing runs either.
    s.mailbox.remove_message(came).unwrap();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Also", "account": local,
        "conditions": [{"type": "subjectContains", "value": "receipt"}], "actions": [{"type": "moveTo", "value": news}]}),
    );
    assert!(shown.check(&s, "rules.run", &params).is_err(), "the question named only “Away”");
}

/// A rule for every account, kept on one account's server, files only that account's mail from
/// then on: moving it into that account's own folder is no move onto another server, and isn't
/// asked as one.
#[test]
fn keeping_a_rule_for_every_account_on_its_server_isnt_asked_as_a_move_onto_another_server() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, account, _, news) = session();
    let every = news_rule(&mut s, account, news);
    let r = s.mailbox.rules.iter_mut().find(|r| r.id == every).unwrap();
    (r.account, r.keep) = (0, Keep::Here);
    let params = json!({"id": every, "account": account});
    assert!(super::rules::keeping_on_server(&s, &params).is_ok());
    let said = describe(&s, "rules.keepOnServer", &params);
    assert!(!said.contains("onto the server of"), "{said}");
    assert_eq!(classify(&s, "rules.keepOnServer", &params), Access::Owner(Class::Settings));
}

/// Keeping on its server a rule its server keeps already changes nothing, and isn't asked as a
/// send for the rule beside it that moves mail onto another account's server.
#[test]
fn keeping_a_rule_on_the_server_that_keeps_it_already_isnt_asked_as_a_send() {
    use crate::agent::{Access, Class, classify};
    let (mut s, account, _, news) = session();
    let there = news_rule(&mut s, account, news);
    assert_eq!(rule(&s, there).keep, Keep::Server);
    let elsewhere = elsewhere(&mut s);
    let away = add(&mut s, account, "Away", json!([{"type": "subjectContains", "value": "spam"}]), json!([{"type": "moveTo", "value": elsewhere}]));
    assert_eq!(rule(&s, away).keep, Keep::Here);
    let params = json!({"id": there});
    assert_eq!(classify(&s, "rules.keepOnServer", &params), Access::Owner(Class::Settings));
}

/// A rule given "copy it to Newsletters" twice copies the message there once.
#[test]
fn a_rule_given_one_copy_twice_copies_once() {
    let (mut s, account, _, news) = session();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.add_message(Message { folder: inbox, account, subject: "Weekly digest".into(), ..Default::default() }).unwrap();
    let twice = json!({"name": "Digest", "account": account, "server": false,
        "conditions": [{"type": "subjectContains", "value": "digest"}],
        "actions": [{"type": "copyTo", "value": news}, {"type": "copyTo", "value": news}]});
    let id = run(&mut s, "rules.add", twice)["id"].as_u64().unwrap();
    assert_eq!(rule(&s, id).actions, vec![Action::CopyTo(news)]);
    run(&mut s, "rules.run", json!({"folder": inbox}));
    assert_eq!(s.mailbox.messages_in(news).count(), 1);
}

/// Run Rules Now asked as settings (a rule kept here that moves mail to Deleted Items) names the
/// folder, its account and the rule, as it does when asked as a send.
#[test]
fn a_run_rules_now_question_asked_as_settings_names_the_folder_its_account_and_the_rule() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, account, _, _) = session();
    let made = json!({"server": false, "name": "Bin it", "account": account,
        "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "delete"}]});
    let id = run(&mut s, "rules.add", made)["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let params = json!({"folder": inbox, "id": id});
    assert_eq!(classify(&s, "rules.run", &params), Access::Owner(Class::Settings));
    assert_eq!(describe(&s, "rules.run", &params), format!("run the rule “Bin it” on “Inbox” of {EMAIL}"));
}

/// A rule for every account's mail that stops the rules after it keeps another account's mail
/// from that account's rule after it, which moves that mail onto the Microsoft account's server.
/// Once Old is kept on that server, it is that account's alone, so the other account's mail that
/// stopped at Old goes onto that server: an agent's `rules.keepOnServer` of it is a change to the
/// rules for that mail, and asks as a send, as a change beside such a rule does.
#[test]
fn keeping_an_every_account_rule_on_one_server_lets_anothers_mail_reach_its_move_onto_a_server() {
    use crate::agent::{Access, Class, classify};
    let (mut s, account, local, news) = session();
    s.mailbox.rules.push(Rule {
        id: 900,
        name: "Old".into(),
        enabled: true,
        conditions: vec![Condition::SubjectContains(vec!["invoice".into()])],
        actions: vec![Action::MarkRead],
        stop: true,
        ..Default::default()
    });
    // The other account's rule, after it, moves its invoices into the Microsoft account's folder.
    let mover = json!({"name": "Mover", "account": local, "conditions": [{"type": "subjectContains", "value": "invoice"}], "actions": [{"type": "moveTo", "value": news}]});
    run(&mut s, "rules.add", mover);
    assert!(s.mailbox.placed_into(local, account));
    let params = json!({"id": 900});
    let asked = classify(&s, "rules.keepOnServer", &params);
    run(&mut s, "rules.keepOnServer", params);
    let m = arriving(&mut s, local, "billing@example.org", "invoice 2");
    assert_eq!(s.mailbox.apply_rules(m, &s.rule_clock()), ["Mover"]);
    assert_eq!(s.mailbox.message(m).map(|m| m.account), Some(account), "onto the Microsoft account's server");
    assert_eq!(asked, Access::Owner(Class::Send), "asked as {asked:?}");
}

/// A rule for every account's mail comes from a mailbox saved before rules had accounts (#668).
/// One that moves mail onto another account's server is beside a change to any account's rules;
/// a change to one is beside every account's rules that do.
#[test]
fn a_rule_for_every_account_is_beside_a_change_to_any_accounts_rules() {
    use crate::agent::{Access, Class, classify};
    let (mut s, _, local, news) = session();
    let ask = |s: &Session, id: &str, params: Value| classify(s, id, &params);
    let (send, settings) = (Access::Owner(Class::Send), Access::Owner(Class::Settings));
    let every = |id: Id, name: &str, action: Action| Rule {
        id,
        name: name.into(),
        enabled: true,
        conditions: vec![Condition::SubjectContains(vec!["invoice".into()])],
        actions: vec![action],
        ..Default::default()
    };
    let read = json!({"name": "Read", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]});
    let off = json!({"id": 900, "value": false});
    s.mailbox.rules.push(every(900, "Old", Action::MarkRead));
    assert_eq!((ask(&s, "rules.add", read.clone()), ask(&s, "rules.enable", off.clone())), (settings, settings));
    // One account's rule beside a change to the rule for every account.
    let away = add(&mut s, local, "Away", json!([{"type": "subjectContains", "value": "invoice"}]), json!([{"type": "moveTo", "value": news}]));
    assert_eq!(ask(&s, "rules.enable", off.clone()), send);
    run(&mut s, "rules.remove", json!({"id": away}));
    // The rule for every account beside a change to one account's rules.
    s.mailbox.rules.push(every(902, "Onto", Action::MoveTo(news)));
    assert_eq!(ask(&s, "rules.add", read), send);
}

/// Two accounts each have a rule kept here called "Purge" that deletes mail. The
/// question about switching one on, deleting it or moving it says whose mail it is for, as the
/// question about making one does: by the name alone the owner can't tell which it is (#566).
#[test]
fn the_question_about_switching_on_deleting_or_moving_a_rule_kept_here_names_its_account() {
    use crate::agent::describe;
    let (mut s, account, local, _) = session();
    let purge = |s: &mut Session, account: Id| {
        let made = run(
            s,
            "rules.add",
            json!({"name": "Purge", "account": account, "conditions": [{"type": "subjectContains", "value": "spam"}], "actions": [{"type": "delete"}], "server": false}),
        );
        let id = made["id"].as_u64().unwrap();
        run(s, "rules.enable", json!({"id": id, "value": false}));
        id
    };
    let theirs = purge(&mut s, local);
    let mine = purge(&mut s, account);
    for (id, params) in [
        ("rules.enable", json!({"id": mine, "value": true})),
        ("rules.remove", json!({"id": mine})),
        ("rules.move", json!({"id": mine, "direction": "up"})),
    ] {
        let asked = describe(&s, id, &params);
        assert!(asked.contains(EMAIL), "{id}: {asked}");
    }
    assert!(!describe(&s, "rules.enable", &json!({"id": theirs, "value": true})).contains(EMAIL));
}

/// An agent's undo of the owner's blocking a sender takes them off the Blocked Senders again,
/// beside a rule that moves their mail onto another account's server: asked as a send, as
/// `junk.remove` is; the undo of taking one off lets less through, and isn't (#566).
#[test]
fn an_undo_that_unblocks_a_sender_beside_a_rule_onto_another_server_is_asked_as_a_send() {
    use crate::agent::{Access, Class, classify};
    let (mut s, account, _, _) = session();
    let elsewhere = elsewhere(&mut s);
    add(&mut s, account, "Away", json!([{"type": "fromContains", "value": "zoe"}]), json!([{"type": "moveTo", "value": elsewhere}]));
    let zoe = json!({"list": "blocked", "entry": "zoe@example.org", "account": account});
    run(&mut s, "junk.add", zoe.clone());
    assert_eq!(classify(&s, "edit.undo", &json!({})), Access::Owner(Class::Send));
    let said = crate::agent::describe(&s, "edit.undo", &json!({}));
    assert!(said.contains("elsewhere@example.org"), "{said}");
    run(&mut s, "junk.remove", zoe);
    assert_ne!(classify(&s, "edit.undo", &json!({})), Access::Owner(Class::Send));
}

/// An agent asks Run Rules Now over a folder where the rule that moves mail onto another server
/// matches nothing, then gives the messages there the category it looks for, which it may do
/// unasked. The owner's yes runs nothing: what the rules would do was asked about the messages as
/// they were. With their categories, flag and importance as they were (another folder's mail
/// flagged, a read mark, what a server last said), the yes runs.
#[test]
fn run_rules_nows_yes_isnt_steered_by_a_safe_edit_meanwhile() {
    use crate::agent::bind::Shown;
    use crate::agent::{Class, Verdict, check};
    let (mut s, _, local, news) = session();
    let archive = s.mailbox.folder_of_kind(local, FolderKind::Archive).unwrap();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Away", "account": local,
        "conditions": [{"type": "category", "value": "Bin"}], "actions": [{"type": "moveTo", "value": news}]}),
    );
    let put = |s: &mut Session, subject: &str| {
        s.mailbox
            .add_message(Message { folder: archive, from: Address::new("Ada", "ada@example.org"), subject: subject.into(), ..Default::default() })
            .unwrap()
    };
    let (one, two) = (put(&mut s, "Contract"), put(&mut s, "Tax return"));
    let params = json!({"folder": archive});
    let Verdict::Ask(Class::Send, what) = check(&s, "rules.run", &params, &[]) else { panic!("asked as a send") };
    let shown = Shown::new(&s, "rules.run", &params, &what).unwrap();
    let inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let elsewhere = s.mailbox.add_message(Message { folder: inbox, subject: "Elsewhere".into(), ..Default::default() }).unwrap();
    run(&mut s, "mail.flag", json!({"id": elsewhere}));
    // What only a server changes (what it last said of a message) isn't what the rules read.
    s.mailbox.message_mut(one).unwrap().synced = Some(Default::default());
    s.mailbox.message_mut(two).unwrap().read = true;
    assert!(shown.check(&s, "rules.run", &params).is_ok(), "nothing the rules read changed in the folder");
    // While the question is up, the agent tags both messages: no question for that.
    let tag = json!({"category": "Bin", "value": true, "ids": [one, two]});
    assert_eq!(check(&s, "mail.categorize", &tag, &[]), Verdict::Run, "categorize is safe");
    run(&mut s, "mail.categorize", tag);
    let reach = shown.check(&s, "rules.run", &params);
    // What the yes would do: as the window runs it, with the class the question was about.
    s.agent = Some(vec![Class::Send]);
    let ran = s.run("rules.run", &params);
    s.agent = None;
    let moved = [one, two].iter().filter(|m| s.mailbox.message(**m).is_some_and(|m| m.folder == news)).count();
    assert!(ran.is_ok());
    assert!(
        reach.is_err(),
        "the yes still reaches the folder ({reach:?}), and running it moves {moved} of 2 messages onto another server that the rule matched none of when the owner was asked"
    );
}

/// A rule kept here moves a sender's mail onto another account's server "except if my address is
/// in To". An agent's change of the account's address lets that rule reach mail its exception
/// left alone, so it is asked as a send and names the rule, as a change to the Safe or Blocked
/// Senders that lets more through is; and so is an undo that changes it back. A change that
/// leaves the address, or one beside no rule that reads it, is asked as before.
#[test]
fn changing_the_address_a_me_exception_reads_is_asked_as_the_rule_change_it_is() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, account, _, _) = session();
    let edit = json!({"id": account, "email": "other@example.org"});
    let before = classify(&s, "account.edit", &edit);
    assert_ne!(before, Access::Owner(Class::Send), "no rule reads the address yet");
    add_with_exception(&mut s, account);
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let to_me = |s: &mut Session| {
        let m = Message {
            folder: inbox,
            from: Address::new("Shop", "news@shop.example"),
            to: vec![Address::new("Me", EMAIL)],
            subject: "Your order".into(),
            ..Default::default()
        };
        let id = s.mailbox.add_message(m).unwrap();
        let clock = s.rule_clock();
        s.mailbox.apply_rules(id, &clock);
        id
    };
    // As it is, mail addressed to me is left alone.
    let kept = to_me(&mut s);
    assert_eq!(s.mailbox.message(kept).map(|m| m.folder), Some(inbox), "the exception holds");
    assert_eq!(classify(&s, "account.edit", &edit), Access::Owner(Class::Send));
    let what = describe(&s, "account.edit", &edit);
    assert!(what.contains("“Away”") && what.contains("address"), "{what}");
    // It is still a change to the account: a grant to send alone doesn't make it.
    use crate::agent::{Verdict, check};
    assert!(matches!(check(&s, "account.edit", &edit, &[Class::Send]), Verdict::Ask(Class::Accounts, _)));
    assert!(matches!(check(&s, "account.edit", &edit, &[Class::Accounts]), Verdict::Ask(Class::Send, _)));
    assert_eq!(check(&s, "account.edit", &edit, &[Class::Accounts, Class::Send]), Verdict::Run);
    // Its name alone leaves the address as it is.
    let named = json!({"id": account, "name": "Owner"});
    assert_eq!(classify(&s, "account.edit", &named), before);
    assert_eq!(classify(&s, "account.edit", &json!({"id": account, "email": EMAIL.to_uppercase()})), before);
    // Changed by the owner, the rule then reaches mail to the old address; an agent's undo, which
    // changes it back, is asked as a send too.
    run(&mut s, "account.edit", edit);
    let gone = to_me(&mut s);
    assert_ne!(s.mailbox.message(gone).map(|m| m.folder), Some(inbox), "what the question warns of");
    // (An undo that puts the accounts back is asked as one, and as a send beside it.)
    assert_eq!(classify(&s, "edit.undo", &json!({})), Access::Owner(Class::Accounts));
    assert!(crate::agent::also(&s, "edit.undo", &json!({}), Class::Accounts).contains(&Class::Send));
    let undo = describe(&s, "edit.undo", &json!({}));
    assert!(undo.contains("elsewhere@example.org"), "{undo}");
}

/// A rule kept here that moves mail from shop.example onto another account's server, except if
/// the account's address is in To.
fn add_with_exception(s: &mut Session, account: Id) {
    let elsewhere = elsewhere(s);
    run(
        s,
        "rules.add",
        json!({"name": "Away", "account": account,
        "conditions": [{"type": "fromContains", "value": "shop.example"}],
        "exceptions": [{"type": "toMe"}], "actions": [{"type": "moveTo", "value": elsewhere}]}),
    );
}

/// The Inbox, on its server, of another Microsoft account, `elsewhere@example.org`, added for it.
fn elsewhere(s: &mut Session) -> Id {
    let other = run(s, "account.add", json!({"email": "elsewhere@example.org", "kind": "microsoft"}))["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "elsewhere-in".into(), name: "Inbox".into(), ..Default::default() });
    inbox
}

/// The owner renames a category on a Microsoft account's list. A rule on its server that puts that
/// category on mail keeps the old name (the server's list isn't written), and the rule on this
/// computer whose exception names the category names it by both, so the next mail the server
/// tagged is still excepted, not deleted.
#[test]
fn a_renamed_category_still_excepts_mail_a_server_rule_tagged() {
    let (mut s, account, _, _) = session();
    // The account's own list, as at the first read of its server's categories.
    s.mailbox.start_account_categories(account, &["Keep".to_string()]);
    run(&mut s, "category.set", json!({"name": "Keep", "color": "#2E8B57", "account": account}));
    // On the server: mail from the boss is tagged "Keep" (made in Outlook on the web, or here and
    // kept there, as a Categorize rule of a Microsoft account is by default).
    let mut tag = remote("r1", "Boss", 1);
    tag.conditions = vec![Condition::From(vec!["boss@example.org".into()])];
    tag.actions = vec![Action::Categorize("Keep".into())];
    merge_rules(&mut s.mailbox, &download(account, vec![tag])).unwrap();
    // On this computer: everything from example.org deleted, except what has "Keep".
    let purge = run(
        &mut s,
        "rules.add",
        json!({"name": "Purge", "account": account, "server": false,
            "conditions": [{"type": "fromContains", "value": "example.org"}],
            "exceptions": [{"type": "category", "value": "Keep"}], "actions": [{"type": "delete"}]}),
    )["id"]
        .as_u64()
        .unwrap();
    assert_eq!(rule(&s, purge).keep, Keep::Here);
    // The owner renames it.
    run(&mut s, "category.rename", json!({"from": "Keep", "to": "Important", "account": account}));
    let server_rule = s.mailbox.rules.iter().find(|r| r.on_server()).unwrap();
    assert_eq!(server_rule.actions, [Action::Categorize("Keep".into())], "the server's rule keeps its name");
    assert_eq!(rule(&s, purge).exceptions, [Condition::Category(vec!["Keep".into(), "Important".into()])], "the rule here follows the rename");
    // The boss's next mail comes in, tagged "Keep" by the server's rule.
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let m = Message {
        folder: inbox,
        from: Address::new("Boss", "boss@example.org"),
        subject: "Contract".into(),
        categories: vec!["Keep".into()],
        ..Default::default()
    };
    let m = s.mailbox.add_message(m).unwrap();
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert_eq!(s.mailbox.message(m).map(|m| m.folder), Some(inbox), "the boss's mail, which the server tagged as the exception names, was deleted");
}

/// An agent's rename of a category that rewrites the rules for the same mail (an exception of a
/// rule here that moves mail onto another account's server) is a change to the rules beside that
/// rule: it asks as a send.
#[test]
fn an_agents_rename_that_rewrites_the_exception_of_a_rule_onto_another_server_asks() {
    let (mut s, account, _, _) = session();
    run(&mut s, "category.set", json!({"name": "Keep", "color": "#2E8B57", "account": account}));
    let elsewhere = elsewhere(&mut s);
    run(
        &mut s,
        "rules.add",
        json!({"name": "Away", "account": account,
            "conditions": [{"type": "fromContains", "value": "example.org"}],
            "exceptions": [{"type": "category", "value": "Keep"}], "actions": [{"type": "moveTo", "value": elsewhere}]}),
    );
    let rename = json!({"from": "Keep", "to": "Important", "account": account});
    let verdict = crate::agent::check(&s, "category.rename", &rename, &[]);
    assert!(matches!(verdict, crate::agent::Verdict::Ask(crate::agent::Class::Send, _)), "{verdict:?}");
}

/// A rule sent to the server whose answer was lost is the server's or may be: a rename of a
/// category it names waits for the answer, so that when the server's rules are read it is known
/// for the one the server made, and doesn't go up a second time; the rename then goes, and the
/// server's rule keeps its name.
#[test]
fn a_rename_while_an_answer_is_lost_doesnt_make_the_rule_twice() {
    let (mut s, account, _, _) = session();
    run(&mut s, "category.set", json!({"name": "Keep", "color": "#2E8B57", "account": account}));
    let id = add(&mut s, account, "Boss", json!([{"type": "fromContains", "value": "boss"}]), json!([{"type": "categorize", "value": "Keep"}]));
    assert_eq!(rule(&s, id).keep, Keep::Server);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    assert!(rule(&s, id).unanswered);
    let rename = json!({"from": "Keep", "to": "Important", "account": account});
    let waits = s.run("category.rename", &rename);
    assert!(waits.as_ref().is_err_and(|e| e.to_string().contains("no answer yet")), "{waits:?}");
    // The server made it as it was sent.
    let mut there = remote("r9", "Boss", 1);
    there.conditions = vec![Condition::FromContains(vec!["boss".into()])];
    there.actions = vec![Action::Categorize("Keep".into())];
    let merged = merge_rules(&mut s.mailbox, &download(account, vec![there])).unwrap();
    let creates = rule_upload(&s.mailbox, account).unwrap().ops.iter().filter(|op| op.kind == RuleOpKind::Create).count();
    assert_eq!((merged.added, s.mailbox.rules.len(), creates), (0, 1, 0), "the rule the server made is taken in once, not made again");
    run(&mut s, "category.rename", rename.clone());
    assert_eq!(rule(&s, id).actions, [Action::Categorize("Keep".into())], "the server's rule keeps the name its server has");
}

/// The zone the calendar is shown in decides which day "received on or after" reads a message as.
/// An agent's change of it beside a rule here that reads those days and moves mail onto another
/// server asks as a send, naming the rule; and the owner's yes to an agent's Run Rules Now, bound
/// to the mail as it was asked about, is refused when the zone changed meanwhile (the owner
/// changed it in the window, say), rather than moving onto another server a message the rule
/// didn't take when asked.
#[test]
fn run_rules_nows_yes_doesnt_follow_a_time_zone_changed_meanwhile() {
    use crate::agent::bind::Shown;
    use crate::agent::{Class, Verdict, answered, check};
    let (mut s, account, _, _) = session();
    s.clock.zone = Some(mailcraft_time::Zone::named("Europe/London").unwrap());
    run(&mut s, "view.timeZone", json!({"zone": "Europe/London"}));
    let elsewhere = elsewhere(&mut s);
    run(
        &mut s,
        "rules.add",
        json!({"name": "Move new reports", "account": account, "server": false,
        "conditions": [{"type": "subjectContains", "value": "report"}, {"type": "receivedBetween", "value": {"from": "2026-11-11"}}],
        "actions": [{"type": "moveTo", "value": elsewhere}]}),
    );
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    // 23:30 UTC on 10 November: the 10th in London, the 11th in Tokyo.
    let at = 1_794_353_400;
    let m = s
        .mailbox
        .add_message(Message {
            folder: inbox,
            account,
            from: Address::new("Shop", "shop@example.org"),
            subject: "Monthly report".into(),
            received: at,
            date: at,
            ..Default::default()
        })
        .unwrap();
    let params = json!({"folder": inbox});
    let Verdict::Ask(Class::Send, what) = check(&s, "rules.run", &params, &[]) else { panic!("asked as a send") };
    let shown = Shown::new(&s, "rules.run", &params, &what).unwrap();
    // As asked, the rule leaves it alone: the 10th is before the 11th.
    let mut as_asked = s.mailbox.clone();
    as_asked.run_rules(m, true, &s.rule_clock());
    assert_eq!(as_asked.message(m).map(|m| m.folder), Some(inbox), "as asked, the rule doesn't take it");
    // An agent's change of the zone is asked about as a send, naming the rule.
    let tokyo = json!({"zone": "Asia/Tokyo"});
    let v = check(&s, "view.timeZone", &tokyo, &[]);
    let Verdict::Ask(Class::Send, why) = v else { panic!("asked as a send: {v:?}") };
    assert!(why.starts_with("show the calendar in the time zone \u{201c}Asia/Tokyo\u{201d}") && why.contains("Move new reports"), "{why}");
    assert_eq!(check(&s, "view.timeZone", &json!({"zone": "Europe/London"}), &[]), Verdict::Run, "the zone it is already");
    // While the question about the rules is up, the zone changes all the same.
    s.run("view.timeZone", &tokyo).unwrap();
    // The owner says yes.
    let reach = shown.check(&s, "rules.run", &params);
    assert!(reach.as_ref().is_err_and(|e| e.contains("time zone")), "{reach:?}");
    let verdict = answered(&s, "rules.run", &params, &[], Class::Send);
    if reach.is_ok() && verdict == Verdict::Run {
        s.agent = Some(vec![Class::Send]);
        s.run("rules.run", &params).unwrap();
        s.agent = None;
    }
    assert_eq!(
        s.mailbox.message(m).map(|m| m.folder),
        Some(inbox),
        "the yes moved onto another server a message the rule didn't take when the owner was asked (reach {reach:?}, verdict {verdict:?}; asked: {what})"
    );
}

/// Keeping on the server a rule the command refuses (one that moves mail into another account)
/// is asked as a change to the rules alone, which the command then refuses: the question doesn't
/// say it lets more mail reach the rule, since it changes nothing.
#[test]
fn keeping_a_refused_rule_on_the_server_is_asked_as_nothing_more_than_a_change_to_the_rules() {
    use crate::agent::{Verdict, check};
    let (mut s, account, _, _) = session();
    let elsewhere = elsewhere(&mut s);
    let away = run(
        &mut s,
        "rules.add",
        json!({"name": "Away", "account": account, "server": false,
        "conditions": [{"type": "subjectContains", "value": "spam"}], "actions": [{"type": "moveTo", "value": elsewhere}]}),
    )["id"]
        .as_u64()
        .unwrap();
    let params = json!({"id": away});
    assert!(super::rules::keeping_on_server(&s, &params).is_err(), "the command refuses it");
    let verdict = check(&s, "rules.keepOnServer", &params, &[]);
    let Verdict::Ask(crate::agent::Class::Settings, what) = &verdict else { panic!("asked as a change to the rules: {verdict:?}") };
    assert!(!what.contains("may let more mail reach"), "{what}");
}

/// An undo of a change of an account's address in the case of a letter outside A to Z ("Ädä" to
/// "ädä") is asked as the change was: a rule reads "me" by A to Z's case alone, so to it the
/// address changed.
#[test]
fn undoing_an_address_change_in_a_non_ascii_letters_case_is_asked_as_the_change_was() {
    use crate::agent::{Access, Class, also, classify};
    let (mut s, account, _, _) = session();
    run(&mut s, "account.edit", json!({"id": account, "email": "Ädä@example.org"}));
    add_with_exception(&mut s, account);
    let edit = json!({"id": account, "email": "ädä@example.org"});
    assert_eq!(classify(&s, "account.edit", &edit), Access::Owner(Class::Send));
    run(&mut s, "account.edit", edit);
    assert_eq!(classify(&s, "edit.undo", &json!({})), Access::Owner(Class::Accounts));
    assert!(also(&s, "edit.undo", &json!({}), Class::Accounts).contains(&Class::Send));
}

/// The owner's yes to Run Rules Now is held to the mail as the rules read it when they were asked
/// about, not to its marks alone (#566). A draft's subject an agent changes unasked
/// (`mail.editDraft`, `compose.set`, `compose.saveDraft`, all safe) while the question is up keeps
/// its id and its marks; the yes runs nothing, so the rule that moves mail onto another server
/// doesn't take the draft it didn't take when asked.
#[test]
fn a_drafts_subject_changed_while_run_rules_now_is_asked_doesnt_ride_the_yes() {
    use crate::agent::bind::Shown;
    use crate::agent::{Class, Verdict, check, classify};
    let (mut s, _, local, news) = session();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Away", "account": local, "conditions": [{"type": "subjectContains", "value": "purge me"}],
            "actions": [{"type": "moveTo", "value": news}]}),
    );
    // The owner's draft, in the local account's Drafts.
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"account": local, "to": "boss@example.org", "subject": "Contract", "body": "Owner's work"}));
    let draft = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    run(&mut s, "compose.close", json!({}));
    let drafts = s.mailbox.folder_of_kind(local, FolderKind::Drafts).unwrap();
    assert_eq!(s.mailbox.message(draft).unwrap().folder, drafts);
    // The agent asks to run the rules over Drafts: the owner is asked, as a send.
    let params = json!({"folder": drafts});
    let Verdict::Ask(Class::Send, what) = check(&s, "rules.run", &params, &[]) else { panic!("asked as a send") };
    let shown = Shown::new(&s, "rules.run", &params, &what).unwrap();
    assert!(shown.check(&s, "rules.run", &params).is_ok(), "nothing changed yet");
    // While the question is up, the agent changes the draft's subject, unasked: each call passes
    // the gate with nothing allowed.
    for (id, params) in [
        ("mail.editDraft", json!({"id": draft})),
        ("compose.set", json!({"subject": "purge me"})),
        ("compose.saveDraft", json!({})),
        ("compose.close", json!({})),
    ] {
        assert_eq!(check(&s, id, &params, &[]), Verdict::Run, "{id} runs unasked");
        assert_eq!(classify(&s, id, &params), crate::agent::Access::Safe, "{id}");
        s.agent = Some(Vec::new());
        run(&mut s, id, params);
        s.agent = None;
    }
    assert_eq!(s.mailbox.message(draft).map(|m| m.subject.clone()).as_deref(), Some("purge me"), "saved in place, same id");
    let reach = shown.check(&s, "rules.run", &params);
    assert!(reach.as_ref().is_err_and(|e| e.contains("what the rules read")), "the yes runs: {reach:?}");
}

/// An agent's change of the account's address is asked as a send when a rule before the one that
/// moves mail onto another server reads the address and puts on a category the moving rule's
/// exception looks for (or stops the rules after it): that changes what reaches the moving rule
/// as switching the rule off would, which asks as a send, and so does the undo of the change
/// (#566).
#[test]
fn an_address_change_read_by_a_rule_before_one_onto_another_server_is_asked_as_a_send() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, account, _, _) = session();
    let elsewhere = elsewhere(&mut s);
    run(
        &mut s,
        "rules.add",
        json!({"name": "Mine", "account": account, "server": false,
        "conditions": [{"type": "toMe"}], "actions": [{"type": "categorize", "value": "Keep"}]}),
    );
    run(
        &mut s,
        "rules.add",
        json!({"name": "Away", "account": account, "server": false,
        "conditions": [{"type": "fromContains", "value": "shop.example"}],
        "exceptions": [{"type": "category", "value": "Keep"}], "actions": [{"type": "moveTo", "value": elsewhere}]}),
    );
    // Switching "Mine" off would let more mail reach "Away": asked as a send.
    let mine = s.mailbox.rules.iter().find(|r| r.name == "Mine").unwrap().id;
    assert_eq!(classify(&s, "rules.enable", &json!({"id": mine, "value": false})), Access::Owner(Class::Send));
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let to_me = |s: &mut Session| {
        let m = Message {
            folder: inbox,
            from: Address::new("Shop", "news@shop.example"),
            to: vec![Address::new("Me", EMAIL)],
            subject: "Your order".into(),
            ..Default::default()
        };
        let id = s.mailbox.add_message(m).unwrap();
        let clock = s.rule_clock();
        s.mailbox.apply_rules(id, &clock);
        id
    };
    let kept = to_me(&mut s);
    assert_eq!(s.mailbox.message(kept).map(|m| m.folder), Some(inbox), "Mine puts Keep on it, so Away leaves it alone");
    let edit = json!({"id": account, "email": "other@example.org"});
    let class = classify(&s, "account.edit", &edit);
    let what = describe(&s, "account.edit", &edit);
    // The owner's own change (or an agent's on `--allow accounts`), then the undo's class.
    run(&mut s, "account.edit", edit);
    let gone = to_me(&mut s);
    let undo_asks_send = crate::agent::also(&s, "edit.undo", &json!({}), Class::Accounts).contains(&Class::Send);
    assert_ne!(s.mailbox.message(gone).map(|m| m.folder), Some(inbox), "what the change lets through");
    assert!(undo_asks_send, "the undo asks as a send");
    assert_eq!(class, Access::Owner(Class::Send), "the change itself isn't asked as one: {what}");
}

/// The same for the time zone, which needs no grant otherwise: a rule before the one that moves
/// mail onto another server reads the days ("received on or after") and puts on the category the
/// moving rule's exception looks for, so `view.timeZone` is asked about (#566).
#[test]
fn a_zone_change_read_by_a_rule_before_one_onto_another_server_is_asked_as_a_send() {
    use crate::agent::{Access, Class, Verdict, check, classify};
    let (mut s, account, _, _) = session();
    s.clock.zone = Some(mailcraft_time::Zone::named("Europe/London").unwrap());
    run(&mut s, "view.timeZone", json!({"zone": "Europe/London"}));
    let elsewhere = elsewhere(&mut s);
    run(
        &mut s,
        "rules.add",
        json!({"name": "Recent", "account": account, "server": false,
        "conditions": [{"type": "receivedBetween", "value": {"from": "2026-11-11"}}], "actions": [{"type": "categorize", "value": "Keep"}]}),
    );
    run(
        &mut s,
        "rules.add",
        json!({"name": "Away", "account": account, "server": false,
        "conditions": [{"type": "subjectContains", "value": "report"}],
        "exceptions": [{"type": "category", "value": "Keep"}], "actions": [{"type": "moveTo", "value": elsewhere}]}),
    );
    let recent = s.mailbox.rules.iter().find(|r| r.name == "Recent").unwrap().id;
    assert_eq!(classify(&s, "rules.enable", &json!({"id": recent, "value": false})), Access::Owner(Class::Send));
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    // 23:30 UTC on 10 November: the 10th in London, the 11th in Tokyo.
    let at = 1_794_353_400;
    let report = |s: &mut Session| {
        let m = Message { folder: inbox, account, subject: "Monthly report".into(), received: at, date: at, ..Default::default() };
        let id = s.mailbox.add_message(m).unwrap();
        let clock = s.rule_clock();
        s.mailbox.apply_rules(id, &clock);
        id
    };
    let tokyo = json!({"zone": "Asia/Tokyo"});
    let verdict = check(&s, "view.timeZone", &tokyo, &[]);
    // In Tokyo it is the 11th, so Recent puts Keep on it; the agent moves the zone back to London.
    run(&mut s, "view.timeZone", tokyo);
    let kept = report(&mut s);
    assert_eq!(s.mailbox.message(kept).map(|m| m.folder), Some(inbox), "Recent puts Keep on it in Tokyo");
    let london = json!({"zone": "Europe/London"});
    let back = check(&s, "view.timeZone", &london, &[]);
    run(&mut s, "view.timeZone", london);
    let gone = report(&mut s);
    assert_ne!(s.mailbox.message(gone).map(|m| m.folder), Some(inbox), "what the change lets through");
    assert_ne!(back, Verdict::Run, "the agent's zone change that lets more mail reach Away runs unasked");
    assert_ne!(verdict, Verdict::Run, "so does the one to Tokyo");
}

/// The same in any folder: an agent's undo (asked as nothing) puts an attachment back on a
/// message while the owner is asked about Run Rules Now. It keeps its id and its marks, but the
/// rules read it otherwise, and the yes runs nothing (#566).
#[test]
fn an_undo_while_run_rules_now_is_asked_doesnt_ride_the_yes() {
    use crate::agent::bind::Shown;
    use crate::agent::{Access, Class, Verdict, check, classify};
    let (mut s, _, local, news) = session();
    let inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    run(
        &mut s,
        "rules.add",
        json!({"name": "Big files", "account": local, "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "moveTo", "value": news}]}),
    );
    let file = mailcraft_model::Attachment {
        name: "a.pdf".into(),
        mime: "application/pdf".into(),
        data: std::sync::Arc::new(vec![1, 2, 3]),
        ..Default::default()
    };
    let m = s
        .mailbox
        .add_message(Message { folder: inbox, account: local, subject: "Plans".into(), attachments: vec![file], ..Default::default() })
        .unwrap();
    // The owner took its file off before (and so the rule leaves it alone).
    run(&mut s, "mail.removeAttachment", json!({"id": m, "index": 0}));
    assert!(!s.mailbox.message(m).unwrap().has_attachments());
    let params = json!({"folder": inbox});
    let Verdict::Ask(Class::Send, what) = check(&s, "rules.run", &params, &[]) else { panic!("asked as a send") };
    let shown = Shown::new(&s, "rules.run", &params, &what).unwrap();
    // While the question is up, the agent undoes, unasked.
    let class = classify(&s, "edit.undo", &json!({}));
    let verdict = check(&s, "edit.undo", &json!({}), &[]);
    s.agent = Some(Vec::new());
    let undone = s.run("edit.undo", &json!({}));
    s.agent = None;
    assert_eq!((class, verdict), (Access::Safe, Verdict::Run), "{undone:?}");
    assert!(s.mailbox.message(m).is_some_and(|m| m.has_attachments()), "the file is back: {undone:?}");
    assert!(shown.check(&s, "rules.run", &params).is_err(), "the yes runs: {what}");
}

/// An agent's Run Rules Now over the Archive is asked as a send, naming the rule "Tidy" ("has the
/// category Bin": move it onto another account's server), which leaves "Tax return" (tagged
/// "Trash" by another program) alone. While the question is up the owner renames Bin to Trash in
/// the window, which rewrites Tidy to take "Bin or Trash": the yes then runs nothing (#566).
#[test]
fn a_rename_while_run_rules_now_is_asked_doesnt_ride_the_yes() {
    use crate::agent::bind::Shown;
    use crate::agent::{Class, Verdict, check};
    let (mut s, account, _, _) = session();
    run(&mut s, "category.set", json!({"name": "Bin", "color": "#2E8B57", "account": account}));
    let elsewhere = elsewhere(&mut s);
    run(
        &mut s,
        "rules.add",
        json!({"server": false, "name": "Tidy", "account": account,
            "conditions": [{"type": "category", "value": "Bin"}], "actions": [{"type": "moveTo", "value": elsewhere}]}),
    );
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let tagged = s.mailbox.add_message(Message { folder: archive, subject: "Tax return".into(), ..Default::default() }).unwrap();
    s.mailbox.message_mut(tagged).unwrap().categories = vec!["Trash".into()];
    let params = json!({"folder": archive});
    let Verdict::Ask(Class::Send, what) = check(&s, "rules.run", &params, &[]) else { panic!("asked as a send") };
    let shown = Shown::new(&s, "rules.run", &params, &what).unwrap();
    let rename = json!({"from": "Bin", "to": "Trash", "account": account});
    let renamed = s.run("category.rename", &rename);
    assert!(renamed.is_ok(), "{renamed:?}");
    let reach = shown.check(&s, "rules.run", &params);
    assert!(reach.as_ref().is_err_and(|e| e.contains("the rules `rules.run` was asked about were changed")), "the yes runs: {reach:?}");
}

/// The same with `--allow settings`: a stop rule above "Tidy" keeps a contract out of its reach
/// when the owner is asked; switching it off meanwhile (which an agent is asked about, as a send)
/// leaves the yes running nothing.
#[test]
fn a_stop_rule_switched_off_while_run_rules_now_is_asked_doesnt_ride_the_yes() {
    use crate::agent::bind::Shown;
    use crate::agent::{Class, Verdict, check};
    let (mut s, account, _, _) = session();
    let elsewhere = elsewhere(&mut s);
    let keep = run(
        &mut s,
        "rules.add",
        json!({"server": false, "name": "Contracts", "account": account, "stop": true,
            "conditions": [{"type": "subjectContains", "value": "Contract"}], "actions": [{"type": "markRead"}]}),
    )["id"]
        .as_u64()
        .unwrap();
    run(
        &mut s,
        "rules.add",
        json!({"server": false, "name": "Tidy", "account": account,
            "conditions": [{"type": "fromContains", "value": "example.org"}], "actions": [{"type": "moveTo", "value": elsewhere}]}),
    );
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    s.mailbox
        .add_message(Message { folder: archive, from: Address::new("Ada", "ada@example.org"), subject: "Contract".into(), ..Default::default() })
        .unwrap();
    let allow = [Class::Settings];
    let params = json!({"folder": archive});
    let Verdict::Ask(Class::Send, what) = check(&s, "rules.run", &params, &allow) else { panic!("asked as a send") };
    let shown = Shown::new(&s, "rules.run", &params, &what).unwrap();
    let off = json!({"id": keep, "value": false});
    assert!(matches!(check(&s, "rules.enable", &off, &allow), Verdict::Ask(Class::Send, _)), "lets more mail reach Tidy");
    let switched = s.run("rules.enable", &off);
    assert!(switched.is_ok(), "{switched:?}");
    assert!(shown.check(&s, "rules.run", &params).is_err(), "the yes runs past the stop rule switched off");
}

/// A rule kept on the server by default is on its way there (sent, no answer yet) when the owner
/// renames a category it names. The rename waits for the answer: had it gone, a refusal (or a job
/// stopped before it) would leave the rule this computer's with the old name alone, its exception
/// no longer keeping the mail renamed here, and a stop rule no longer holding a later delete
/// (#566).
#[test]
fn a_rename_waits_while_a_rule_naming_it_is_on_its_way_to_the_server() {
    for (stop, refused) in [(false, true), (false, false), (true, true)] {
        let (mut s, account, _, _) = session();
        run(&mut s, "category.set", json!({"name": "Keep", "color": "#2E8B57", "account": account}));
        let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let kept = Message {
            folder: inbox,
            from: Address::new("Boss", "boss@example.org"),
            subject: "Contract".into(),
            categories: vec!["Keep".into()],
            ..Default::default()
        };
        let kept = s.mailbox.add_message(kept).unwrap();
        let on_its_way = if stop {
            json!({"name": "Keepers", "account": account, "stop": true,
                "conditions": [{"type": "category", "value": "Keep"}], "actions": [{"type": "markRead"}]})
        } else {
            json!({"name": "Tidy", "account": account, "conditions": [{"type": "fromContains", "value": "example.org"}],
                "exceptions": [{"type": "category", "value": "Keep"}], "actions": [{"type": "delete"}]})
        };
        let id = run(&mut s, "rules.add", on_its_way)["id"].as_u64().unwrap();
        assert_eq!(rule(&s, id).keep, Keep::Server);
        if stop {
            let purge = json!({"name": "Purge", "account": account, "server": false,
                "conditions": [{"type": "fromContains", "value": "example.org"}], "actions": [{"type": "delete"}]});
            run(&mut s, "rules.add", purge);
        }
        let upload = rule_upload(&s.mailbox, account).unwrap();
        mark_sent(&mut s, &upload);
        let rename = json!({"from": "Keep", "to": "Important", "account": account});
        assert!(s.run("category.rename", &rename).is_err(), "renamed while the rule was on its way");
        assert_eq!(s.mailbox.message(kept).unwrap().categories, ["Keep".to_string()], "nothing changed");
        if refused {
            let told = RuleOutcome { kind: RuleOpKind::Create, rule: id, result: RuleResult::Refused, status: 400, ..Default::default() };
            apply_rule_changes(&mut s, &answered(account, vec![told])).unwrap();
            assert_eq!(rule(&s, id).keep, Keep::Here, "kept on this computer");
        } else {
            not_sent(&mut s, &upload, &[]);
            assert!(rule(&s, id).waiting() && !rule(&s, id).unanswered, "waits to go again");
        }
        // The answer in, the rename goes, and the rule follows it.
        run(&mut s, "category.rename", rename.clone());
        assert_eq!(s.mailbox.message(kept).unwrap().categories, ["Important".to_string()]);
        run(&mut s, "rules.run", json!({"folder": inbox}));
        let left = s.mailbox.message(kept).map(|m| m.folder);
        assert_eq!(left, Some(inbox), "stop {stop}, refused {refused}: the rule is now {:?}", rule(&s, id));
    }
}

/// A server rule's exception changed on the server only in a letter that the matcher tells apart
/// (an address compares by the A to Z letters' case alone) is taken, so that Run Rules Now leaves
/// alone what the server's rule leaves alone (#566).
#[test]
fn an_exception_changed_on_the_server_in_a_letter_the_matcher_tells_apart_is_taken() {
    let (mut s, account, _, _) = session();
    let mut theirs = remote("r1", "Invoices", 1);
    theirs.actions = vec![Action::Delete];
    theirs.exceptions = vec![Condition::From(vec!["Ünal@example.org".into()])];
    merge_rules(&mut s.mailbox, &download(account, vec![theirs.clone()])).unwrap();
    // Changed in Outlook on the web: to the matcher, another address.
    theirs.exceptions = vec![Condition::From(vec!["ünal@example.org".into()])];
    merge_rules(&mut s.mailbox, &download(account, vec![theirs])).unwrap();
    let m = arriving(&mut s, account, "ünal@example.org", "INVOICE 12");
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    run(&mut s, "rules.run", json!({"folder": inbox}));
    let r = s.mailbox.rules.iter().find(|r| r.server.as_ref().is_some_and(|x| x.id == "r1")).unwrap();
    let found = (s.mailbox.message(m).map(|m| m.folder), r.exceptions.clone());
    assert_eq!(found, (Some(inbox), vec![Condition::From(vec!["ünal@example.org".into()])]), "(the mail's folder, the kept exception)");
}

/// Always Move's question names whose mail the rule kept here is for, and the yes makes it for that
/// account: a move of the message onto this computer meanwhile changes the question (#566).
#[test]
fn always_move_question_names_whose_mail_and_the_yes_keeps_to_it() {
    use crate::agent::bind::{Reach, Shown};
    use crate::agent::{Class, Verdict, check, describe};
    let (mut s, account, local, _) = session();
    // A folder on this computer: the rule stays here, and puts nothing onto another server.
    let kept = s.mailbox.folder_of_kind(local, FolderKind::Archive).unwrap();
    let m = arriving(&mut s, account, "news@example.org", "Weekly");
    // The window shows another folder than the message's.
    run(&mut s, "folder.select", json!({"id": kept}));
    // (Any command since, as in the window, which runs them all the time.)
    run(&mut s, "mail.markRead", json!({"ids": [m]}));
    // (The window lists the folder in view, every frame.)
    let _ = s.rows();
    let params = json!({"id": m, "folder": kept});
    let Verdict::Ask(Class::Settings, what) = check(&s, "rules.fromMessage", &params, &[]) else { panic!("asked as settings") };
    let shown = Shown::new(&s, "rules.fromMessage", &params, &what).unwrap();
    // An agent's unasked move of the message onto this computer, while the question is up.
    let local_inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    assert_eq!(check(&s, "mail.move", &json!({"ids": [m], "folder": local_inbox}), &[]), Verdict::Run);
    let (sel, view) = (s.sel.clone(), s.view.clone());
    run(&mut s, "mail.move", json!({"ids": [m], "folder": local_inbox}));
    assert!(s.sel == sel, "the window's selection is as the question had it: {sel:?} then, {:?} now", s.sel);
    assert!(s.view == view, "the window's view is as the question had it");
    let still = shown.check(&s, "rules.fromMessage", &params);
    let made = run(&mut s, "rules.fromMessage", params.clone())["id"].as_u64().unwrap();
    let whose = rule(&s, made).account;
    assert!(
        still != Ok(Reach::All) || whose == account,
        "the yes made the rule for account {whose}, not {account} the question was about; the text was the same: {}",
        describe(&s, "rules.fromMessage", &params) == what
    );
}

/// Always Move's question says whose mail the rule kept here is for (#566).
#[test]
fn always_move_question_says_whose_mail() {
    use crate::agent::{Class, Verdict, check};
    let (mut s, account, local, _) = session();
    let kept = s.mailbox.folder_of_kind(local, FolderKind::Archive).unwrap();
    let m = arriving(&mut s, account, "news@example.org", "Weekly");
    let Verdict::Ask(Class::Settings, what) = check(&s, "rules.fromMessage", &json!({"id": m, "folder": kept}), &[]) else { panic!("asked") };
    assert!(what.contains("for the mail of"), "the question doesn't say whose mail: {what}");
}

/// With `accounts` allowed already, an undo of a category's rename that puts back the accounts,
/// the rules of a rule that copies onto another server, and takes the new name away, is asked
/// once, and the owner's yes to that question runs it (#566).
#[test]
fn an_undo_needing_send_and_delete_beside_allowed_accounts_runs_on_the_yes() {
    use crate::agent::{Class, Verdict, answered, check};
    let (mut s, _, local, news) = session();
    run(&mut s, "category.set", json!({"name": "Old", "color": "#2E8B57"}));
    add(&mut s, local, "Up", json!([{"type": "category", "value": "Old"}]), json!([{"type": "copyTo", "value": news}]));
    run(&mut s, "category.rename", json!({"from": "Old", "to": "New"}));
    // Since: the accounts changed.
    s.mailbox.accounts[0].name.push_str(" (renamed)");
    let allow = [Class::Accounts];
    let Verdict::Ask(class, what) = check(&s, "edit.undo", &json!({}), &allow) else { panic!("asked") };
    assert!(what.contains("onto the server of") && what.contains("takes away"), "{what}");
    assert_eq!(answered(&s, "edit.undo", &json!({}), &allow, class), Verdict::Run, "asked as {class:?}: {what}");
}
