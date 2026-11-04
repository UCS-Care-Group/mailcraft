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
            edit: op.edit,
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

/// The rules of `account` in their order here, each with its place.
fn places(s: &Session, account: Id) -> Vec<(String, i32)> {
    let order = s.mailbox.rules_in_order(account);
    order.iter().map(|i| &s.mailbox.rules[*i]).map(|r| (r.name.clone(), r.server.as_ref().map_or(0, |s| s.sequence))).collect()
}

/// Move Up or Move Down on the rule called `name`.
fn step(s: &mut Session, name: &str, direction: &str) -> Value {
    let id = s.mailbox.rules.iter().find(|r| r.name == name).unwrap().id;
    run(s, "rules.move", json!({"id": id, "direction": direction}))
}

#[test]
fn a_rule_the_server_locks_moves_and_goes_back_if_the_server_wont_have_it() {
    let pairs = |list: &[(&str, i32)]| list.iter().map(|(n, p)| (n.to_string(), *p)).collect::<Vec<_>>();
    let start = || {
        let (mut s, account, _, _) = session();
        let rules =
            vec![remote("i", "Invoices", 1), RemoteRule { read_only: true, ..remote("p", "Company policy", 2) }, remote("n", "Newsletters", 3)];
        merge_rules(&mut s.mailbox, &download(account, rules)).unwrap();
        (s, account)
    };
    // Down on the locked rule, or Up on the one under it: the two change places, as any two do.
    for (name, direction) in [("Company policy", "down"), ("Newsletters", "up")] {
        let (mut s, account) = start();
        assert_eq!(step(&mut s, name, direction)["moved"], true);
        assert_eq!(places(&s, account), pairs(&[("Invoices", 1), ("Newsletters", 2), ("Company policy", 3)]), "{name}");
        // The locked rule's place goes first, said to be one the server may not have; the other's after.
        let ops = rule_upload(&s.mailbox, account).unwrap().ops;
        let told: Vec<(&str, Option<i32>, Option<bool>, bool)> = ops.iter().map(|op| (op.id.as_str(), op.sequence, op.enabled, op.locked)).collect();
        assert_eq!(told, [("p", Some(3), None, true), ("n", Some(2), None, false)], "{name}");
    }

    // The server has it: both stand where they were put, and stay there when it is read again.
    let (mut s, account) = start();
    step(&mut s, "Company policy", "down");
    all_taken(&mut s, account);
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    let back = vec![remote("i", "Invoices", 1), remote("n", "Newsletters", 2), RemoteRule { read_only: true, ..remote("p", "Company policy", 3) }];
    merge_rules(&mut s.mailbox, &download(account, back)).unwrap();
    assert_eq!(places(&s, account), pairs(&[("Invoices", 1), ("Newsletters", 2), ("Company policy", 3)]));

    // It doesn't: the host stops there, and the rule goes back to where the server has it, the one
    // it changed places with too (its place wasn't sent); it is said, and nothing is sent again.
    let (mut s, account) = start();
    step(&mut s, "Company policy", "down");
    let refused = RuleOutcome {
        kind: RuleOpKind::Update,
        rule: rule_id(&s, "Company policy"),
        id: "p".into(),
        result: RuleResult::Refused,
        sequence: Some(3),
        status: 403,
        error: "the server doesn't let this rule be moved".into(),
        ..Default::default()
    };
    let merged = apply_rule_changes(&mut s, &answered(account, vec![refused])).unwrap();
    assert_eq!(merged.messages.len(), 1);
    assert!(merged.messages[0].contains("didn't let the rule \"Company policy\" be moved"), "{:?}", merged.messages);
    assert_eq!(places(&s, account), pairs(&[("Invoices", 1), ("Company policy", 2), ("Newsletters", 3)]));
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    // It is still never switched on or off here.
    let id = rule_id(&s, "Company policy");
    assert!(s.run("rules.enable", &json!({"id": id, "value": false})).is_err());

    // A refusal said of it when it wasn't moved (no host sends one) puts nobody else's place back.
    let (mut s, account, _, _) = session();
    let rules = vec![remote("i", "Invoices", 1), remote("n", "Newsletters", 2), RemoteRule { read_only: true, ..remote("p", "Company policy", 3) }];
    merge_rules(&mut s.mailbox, &download(account, rules)).unwrap();
    step(&mut s, "Newsletters", "up");
    let odd = RuleOutcome {
        kind: RuleOpKind::Update,
        rule: rule_id(&s, "Company policy"),
        id: "p".into(),
        result: RuleResult::Refused,
        ..Default::default()
    };
    let merged = apply_rule_changes(&mut s, &answered(account, vec![odd])).unwrap();
    assert!(merged.messages.iter().all(|m| !m.contains("be moved")), "{:?}", merged.messages);
    assert_eq!(places(&s, account), pairs(&[("Newsletters", 1), ("Invoices", 2), ("Company policy", 3)]));
}

fn rule_id(s: &Session, name: &str) -> Id {
    s.mailbox.rules.iter().find(|r| r.name == name).unwrap().id
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
    // The window's question then answers with Delete: the rule runs at once on that mail.
    assert!(crate::agent::rule_from_message_deletes(&s, &json!({"folder": deleted, "id": m})));
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
    for (n, action) in
        [Action::DeleteForGood, Action::FlagFor(mailcraft_model::FollowUp::Today), Action::ClearFlag, Action::ClearCategories].into_iter().enumerate()
    {
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

/// A rule for every account's mail, as one from before rules had accounts is read from its file:
/// `rules.add` makes none (#566).
fn every_account_rule(s: &mut Session, name: &str) -> Id {
    every_account_rule_doing(s, name, vec![Action::MarkRead])
}

fn every_account_rule_doing(s: &mut Session, name: &str, actions: Vec<Action>) -> Id {
    let id = s.mailbox.alloc_id();
    s.mailbox.rules.push(Rule { id, name: name.into(), enabled: true, conditions: vec![Condition::HasAttachment], actions, ..Default::default() });
    id
}

#[test]
fn removing_an_account_takes_its_rules_with_it() {
    let (mut s, account, local, news) = session();
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Server's", 1)])).unwrap();
    add(&mut s, account, "Mine", json!([{"type": "subjectContains", "value": "digest"}]), json!([{"type": "delete"}]));
    news_rule(&mut s, account, news);
    add(&mut s, local, "Local", json!([{"type": "hasAttachment"}]), json!([{"type": "markRead"}]));
    every_account_rule(&mut s, "Everyone");
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
    // So is a move out of the account (to one on this computer alone), and a server for a rule of
    // an account whose server keeps none.
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
        (Rule { actions: vec![Action::ClearFlag], ..base.clone() }, NotOnServer::Flag),
        (Rule { actions: vec![Action::DeleteForGood], ..base.clone() }, NotOnServer::DeleteForGood),
        (
            Rule { exceptions: vec![Condition::Importance(Importance::Low), Condition::Importance(Importance::High)], ..base.clone() },
            NotOnServer::TwoExceptionsOfAKind,
        ),
    ] {
        assert_eq!(cannot(rule.clone()), Some(why), "{:?}", (rule.conditions, rule.exceptions, rule.actions, rule.alert));
    }
    // The same through the command: such a rule is made here, and says why.
    let made = run(
        &mut s,
        "rules.add",
        json!({"name": "Gone", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "deleteForGood"}]}),
    );
    assert_eq!(made["keep"], "here");
    let refused = s.run(
        "rules.add",
        &json!({"name": "Gone", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "deleteForGood"}], "server": true}),
    );
    assert!(refused.is_err_and(|e| e.to_string().contains("for good")));
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

#[test]
fn mail_a_rule_couldnt_copy_isnt_deleted() {
    let (mut s, account, local, _) = session();
    let elsewhere = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    for gone in ["delete", "deleteForGood"] {
        let name = format!("Keep and {gone}");
        let actions = json!([{"type": "copyTo", "value": elsewhere}, {"type": gone}]);
        let id = run(
            &mut s,
            "rules.add",
            json!({"name": name, "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "x"}], "actions": actions}),
        )["id"]
            .as_u64()
            .unwrap();
        // Not downloaded yet, so it can't be copied to another mailbox.
        let m = arriving(&mut s, account, "a@example.org", "x");
        s.mailbox.message_mut(m).unwrap().unfetched = true;
        let before = s.mailbox.message(m).unwrap().folder;
        s.mailbox.apply_rules(m, &s.rule_clock());
        assert_eq!(s.mailbox.message(m).map(|m| m.folder), Some(before), "{gone}");
        // Whole, it is copied and the rule goes on.
        let m = arriving(&mut s, account, "a@example.org", "x");
        s.mailbox.apply_rules(m, &s.rule_clock());
        assert_ne!(s.mailbox.message(m).map(|m| m.folder), Some(before), "{gone}");
        assert!(s.mailbox.messages.values().any(|c| c.folder == elsewhere), "{gone}");
        run(&mut s, "rules.remove", json!({"id": id}));
    }
}

#[test]
fn mail_of_which_only_the_headers_are_here_isnt_deleted_after_a_copy() {
    let (mut s, _, local, _) = session();
    let inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let kept = s.mailbox.add_folder(local, None, "Kept").unwrap();
    let actions = json!([{"type": "copyTo", "value": kept}, {"type": "deleteForGood"}]);
    run(
        &mut s,
        "rules.add",
        json!({"name": "Keep", "account": local, "conditions": [{"type": "subjectContains", "value": "x"}], "actions": actions}),
    );
    let m = arriving(&mut s, local, "a@example.org", "x");
    s.mailbox.message_mut(m).unwrap().not_downloaded = 30_000_000;
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert_eq!(s.mailbox.message(m).map(|m| m.folder), Some(inbox));
    assert!(s.mailbox.messages.values().any(|c| c.folder == kept));
}

#[test]
fn a_rule_that_cant_tell_yet_doesnt_act_and_the_rules_after_it_delete_nothing_for_good() {
    let (mut s, account, _, _) = session();
    let reports = s.mailbox.add_folder(account, None, "Reports").unwrap();
    let report = json!([{"type": "subjectContains", "value": "report"}]);
    // Stops the others: unless the text says "keep"; when it says "draft"; when it is small.
    let firsts = [
        json!({"conditions": report, "exceptions": [{"type": "bodyContains", "value": "keep"}]}),
        json!({"conditions": [{"type": "bodyContains", "value": "draft"}]}),
        json!({"conditions": [{"type": "sizeBetween", "value": {"min": 0, "max": 1024}}]}),
    ];
    for parts in firsts {
        let mut first = json!({"name": "First", "account": account, "server": false, "stop": true, "actions": [{"type": "flag"}]});
        first.as_object_mut().unwrap().extend(parts.as_object().unwrap().clone());
        let first = run(&mut s, "rules.add", first)["id"].as_u64().unwrap();
        let actions = json!([{"type": "moveTo", "value": reports}, {"type": "deleteForGood"}]);
        let then = json!({"name": "Then", "account": account, "server": false, "conditions": report, "actions": actions});
        let then = run(&mut s, "rules.add", then)["id"].as_u64().unwrap();
        // Listed by its server, with a preview that says none of it.
        let m = arriving(&mut s, account, "a@example.org", "Report");
        s.mailbox.message_mut(m).unwrap().unfetched = true;
        assert_eq!(s.mailbox.apply_rules(m, &s.rule_clock()), ["Then"], "{parts}");
        let left = s.mailbox.message(m).expect("the rule after it didn't delete the mail");
        assert_eq!((left.folder, left.flag), (reports, mailcraft_model::FlagStatus::None), "{parts}");
        // An exception that holds leaves that rule out as ever: the rules after it go on as ever.
        let sender = json!({"type": "from", "value": "a@example.org"});
        s.mailbox.rules.iter_mut().find(|r| r.id == first).unwrap().exceptions.push(serde_json::from_value(sender).unwrap());
        let m = arriving(&mut s, account, "a@example.org", "Report");
        s.mailbox.message_mut(m).unwrap().unfetched = true;
        s.mailbox.apply_rules(m, &s.rule_clock());
        assert!(s.mailbox.message(m).is_none(), "{parts}");
        for id in [first, then] {
            run(&mut s, "rules.remove", json!({"id": id}));
        }
    }
}

#[test]
fn an_agent_needs_both_grants_for_a_rule_that_copies_onto_another_server_and_deletes_for_good() {
    use crate::agent::{Class, Verdict, check};
    let (mut s, account, _, _) = session();
    let other = run(&mut s, "account.add", json!({"email": "other@example.org", "kind": "microsoft"}))["id"].as_u64().unwrap();
    let there = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    let params = json!({
        "name": "Both", "account": account, "conditions": [{"type": "toMe"}],
        "actions": [{"type": "copyTo", "value": there}, {"type": "deleteForGood"}],
    });
    let class = |allow: &[Class]| match check(&s, "rules.add", &params, allow) {
        Verdict::Ask(class, _) => Some(class),
        Verdict::Run => None,
        Verdict::Refuse(why) => panic!("{why}"),
    };
    // Asked as a delete unless that is allowed: the question says both, and a yes to a question
    // that names only the server never makes a rule that deletes for good.
    assert_eq!(class(&[]), Some(Class::Delete));
    assert_eq!(class(&[Class::Send]), Some(Class::Delete));
    assert_eq!(class(&[Class::Delete]), Some(Class::Send));
    assert_eq!(class(&[Class::Send, Class::Delete]), None);
    let yes = |allow: &[Class], yes: Class| matches!(crate::agent::answered(&s, "rules.add", &params, allow, yes), Verdict::Run);
    assert!(yes(&[], Class::Delete) && yes(&[Class::Send], Class::Delete) && yes(&[Class::Delete], Class::Send));
    assert!(!yes(&[], Class::Send), "a yes to sending alone");
    let asked = crate::agent::describe(&s, "rules.add", &params);
    assert!(asked.contains("copy it onto the server of other@example.org") && asked.contains("delete it permanently"), "{asked}");
    // Run over the Inbox, the question says both, and that the rule copies.
    run(&mut s, "rules.add", params.clone());
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let running = json!({"folder": inbox});
    let asked = crate::agent::describe(&s, "rules.run", &running);
    assert_eq!(check(&s, "rules.run", &running, &[]), Verdict::Ask(Class::Delete, asked.clone()));
    assert!(
        asked.contains(", which copy mail onto the server of other@example.org, and the rule \u{201c}Both\u{201d} deletes mail permanently")
            && !asked.contains("move"),
        "{asked}"
    );
    // Rules that move mail there are said apart from those that copy it, each account once.
    let archive = s.mailbox.folder_of_kind(other, FolderKind::Archive).unwrap();
    add(&mut s, account, "Moves", json!([{"type": "toMe"}]), json!([{"type": "moveTo", "value": archive}]));
    add(&mut s, account, "Moves too", json!([{"type": "toMe"}]), json!([{"type": "moveTo", "value": archive}]));
    add(&mut s, account, "Copies too", json!([{"type": "toMe"}]), json!([{"type": "copyTo", "value": archive}]));
    let asked = crate::agent::describe(&s, "rules.run", &running);
    assert!(
        asked.contains(", which move mail onto the server of other@example.org and copy mail onto the server of other@example.org, and the rule"),
        "{asked}"
    );
}

#[test]
fn a_rule_filing_within_its_own_account_isnt_said_to_go_onto_another_server() {
    // With another account whose mail could be filed there, the question said a rule of the
    // account's own moved its mail "onto the server of" that same account.
    let (mut s, account, _, _) = session();
    run(&mut s, "account.add", json!({"email": "other@example.org", "kind": "microsoft"}));
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let mut rule = json!({"name": "File", "account": account, "server": false, "conditions": [{"type": "toMe"}],
        "actions": [{"type": "moveTo", "value": archive}, {"type": "copyTo", "value": archive}]});
    let asked = crate::agent::describe(&s, "rules.add", &rule);
    assert!(asked.contains("move it to ") && asked.contains("copy it to ") && !asked.contains("onto the server"), "{asked}");
    // Named no account, the rule is the one in view's, as the Rules window's is (#566): the
    // question says so, and the rule made is.
    rule.as_object_mut().unwrap().remove("account");
    let asked = crate::agent::describe(&s, "rules.add", &rule);
    assert!(asked.contains("move it to ") && !asked.contains("onto the server"), "{asked}");
    assert!(asked.contains(&format!("for the mail of \u{201c}{EMAIL}\u{201d}")), "{asked}");
    let made = run(&mut s, "rules.add", rule)["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.rules.iter().find(|r| r.id == made).map(|r| r.account), Some(account));
    // One for every account's mail, from before rules had accounts, does put the other's there:
    // switched on or run over the other's Inbox, it is asked as a send, and says so.
    use crate::agent::{Access, Class, classify};
    let old = every_account_rule_doing(&mut s, "Old", vec![Action::MoveTo(archive)]);
    s.mailbox.rules.iter_mut().find(|r| r.id == old).unwrap().enabled = false;
    let on = json!({"id": old, "value": true});
    assert_eq!(classify(&s, "rules.enable", &on), Access::Owner(Class::Send));
    run(&mut s, "rules.enable", on);
    let other = s.mailbox.accounts.iter().find(|a| a.email == "other@example.org").unwrap().id;
    let theirs = json!({"folder": s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap()});
    assert_eq!(classify(&s, "rules.run", &theirs), Access::Owner(Class::Send));
    let asked = crate::agent::describe(&s, "rules.run", &theirs);
    assert!(asked.contains("onto the server of "), "{asked}");
}

#[test]
fn a_rule_made_without_naming_an_account_is_for_the_one_in_view() {
    // As the Rules window makes it: a rule belongs to one account (#566), and an agent or a script
    // that names none gets the account in view, which its question names.
    let (mut s, account, local, _) = session();
    let rule = json!({"name": "Read", "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]});
    // Kept where a rule of that account is: on a Microsoft account's server when it can do all of
    // it, as when the account is named.
    for (shown, mine, kept) in [(local, "me@example.org", "here"), (account, EMAIL, "server")] {
        s.view.folder = s.mailbox.folder_of_kind(shown, FolderKind::Inbox).unwrap();
        let asked = crate::agent::describe(&s, "rules.add", &rule);
        assert!(asked.contains(mine), "{asked}");
        let made = run(&mut s, "rules.add", rule.clone());
        assert_eq!(made["keep"], kept);
        let made = made["id"].as_u64().unwrap();
        assert_eq!(s.mailbox.rules.iter().find(|r| r.id == made).map(|r| r.account), Some(shown));
    }
    // Asked for the server, it is refused where the account in view keeps none.
    let mut on_server = rule.clone();
    on_server["server"] = json!(true);
    s.view.folder = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    assert!(s.run("rules.add", &on_server).is_err());
    // A shared mailbox's folder in view: its owner's (not the first account's), as a rule is never
    // a shared mailbox's.
    let boss = run(&mut s, "account.add", json!({"email": "boss@example.org", "kind": "microsoft"}))["id"].as_u64().unwrap();
    let shared = s.mailbox.add_shared_mailbox(boss, "team@example.org").unwrap();
    s.view.folder = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    let made = run(&mut s, "rules.add", rule.clone())["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.rules.iter().find(|r| r.id == made).map(|r| r.account), Some(boss));
    // Nothing in view: the first account. Never every account's mail.
    s.view.folder = 0;
    let made = run(&mut s, "rules.add", rule)["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.rules.iter().find(|r| r.id == made).map(|r| r.account), Some(account));
    assert!(s.run("rules.add", &json!({"name": "R", "account": 0, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]})).is_err());
}

#[test]
fn a_rule_made_before_the_first_account_goes_with_it() {
    // A first start has only a placeholder, which the first account added takes the place of:
    // a rule made for it is that account's then, filing into its folder of the same kind.
    let mut s = Session::empty();
    let placeholder = s.mailbox.placeholder().unwrap();
    let archive = s.mailbox.folder_of_kind(placeholder, FolderKind::Archive).unwrap();
    let rule = json!({"name": "Keep", "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}, {"type": "moveTo", "value": archive}]});
    let made = run(&mut s, "rules.add", rule)["id"].as_u64().unwrap();
    let a = run(&mut s, "account.add", json!({"email": "first@example.org"}))["id"].as_u64().unwrap();
    let r = s.mailbox.rules.iter().find(|r| r.id == made).unwrap();
    let theirs = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    assert_eq!((r.account, &r.actions), (a, &vec![Action::MarkRead, Action::MoveTo(theirs)]));
    // The next account takes nothing of the first's.
    run(&mut s, "account.add", json!({"email": "second@example.org"}));
    assert_eq!(s.mailbox.rules.iter().find(|r| r.id == made).map(|r| r.account), Some(a));
}

#[test]
fn an_agent_asks_the_owner_before_a_rule_that_deletes_for_good() {
    use crate::agent::{Access, Class, classify};
    let (mut s, account, _, _) = session();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let rule = |actions: Value| json!({"name": "Gone", "account": account, "conditions": [{"type": "toMe"}], "actions": actions});
    assert_eq!(classify(&s, "rules.add", &rule(json!([{"type": "deleteForGood"}]))), Access::Owner(Class::Delete));
    // To Deleted Items on this computer, undo reaches it.
    let here = json!({"server": false, "name": "Gone", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "delete"}]});
    assert_eq!(classify(&s, "rules.add", &here), Access::Owner(Class::Settings));
    let id = run(&mut s, "rules.add", rule(json!([{"type": "deleteForGood"}])))["id"].as_u64().unwrap();
    // Switching it on, or running it, deletes for good too; switching it off doesn't.
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id, "value": true})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "rules.run", &json!({"folder": inbox})), Access::Owner(Class::Settings));
    // A rule from a file that has no conditions acts on nothing, so it deletes nothing.
    let empty = s.mailbox.alloc_id();
    s.mailbox.rules.push(Rule {
        id: empty,
        name: "Empty".into(),
        account,
        enabled: true,
        actions: vec![Action::DeleteForGood],
        ..Default::default()
    });
    assert_eq!(classify(&s, "rules.run", &json!({"folder": inbox})), Access::Owner(Class::Settings));
    run(&mut s, "rules.enable", json!({"id": id, "value": true}));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id, "value": false})), Access::Owner(Class::Settings));
    assert_eq!(classify(&s, "rules.run", &json!({"folder": inbox})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "rules.run", &json!({"folder": inbox, "id": id})), Access::Owner(Class::Delete));
    // The question says so, by the rule's name.
    let asked = crate::agent::describe(&s, "rules.run", &json!({"folder": inbox}));
    assert!(asked.contains("and the rule \u{201c}Gone\u{201d} deletes mail permanently") && !asked.contains("Empty"), "{asked}");
}

#[test]
fn an_agent_asks_the_owner_before_changing_the_rules_beside_one_that_deletes_for_good() {
    use crate::agent::{Access, Class, classify};
    let (mut s, account, local, news) = session();
    // Purge deletes for good on this computer; Keep, here too, stops the rules after it.
    let purge = add(&mut s, account, "Purge", json!([{"type": "category", "value": "Bin"}]), json!([{"type": "deleteForGood"}]));
    assert_eq!(rule(&s, purge).keep, Keep::Here);
    let made = json!({"server": false, "name": "Keep", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}], "stop": true});
    let keep = run(&mut s, "rules.add", made)["id"].as_u64().unwrap();
    let other = add(&mut s, local, "Other", json!([{"type": "toMe"}]), json!([{"type": "markRead"}]));
    let m = arriving(&mut s, account, "news@example.org", "Weekly");
    let ask = |s: &Session, id: &str, params: Value| classify(s, id, &params);
    let (delete, settings) = (Access::Owner(Class::Delete), Access::Owner(Class::Settings));
    // A rule kept on the server runs before Purge: one that sets the category it looks for, say.
    let tagger = |account: Id| json!({"name": "Tag", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "categorize", "value": "Bin"}]});
    let changes = |keep: Id| {
        [
            ("rules.add", tagger(account)),
            ("rules.fromMessage", json!({"folder": news, "id": m})),
            ("rules.keepOnServer", json!({"id": keep})),
            ("rules.move", json!({"id": keep, "direction": "down"})),
            ("rules.enable", json!({"id": keep, "value": false})),
            ("rules.enable", json!({"id": keep})),
            ("rules.remove", json!({"id": keep})),
        ]
    };
    // Each question says why it is asked as a delete: Purge, by name.
    let beside = "; the rule \u{201c}Purge\u{201d}, switched on here, deletes mail permanently, and a change to the rules may let more mail reach it";
    for (id, params) in changes(keep) {
        assert_eq!(ask(&s, id, params.clone()), delete, "{id} {params}");
        let said = crate::agent::describe(&s, id, &params);
        assert!(said.contains(beside), "{id} {params}: {said}");
    }
    // A yes to the rule from a message deletes nothing yet: the window's question says Allow.
    assert!(!crate::agent::rule_from_message_deletes(&s, &json!({"folder": news, "id": m})));
    // A rule made without naming an account is the one in view's, whose mail Purge files.
    assert_eq!(ask(&s, "rules.add", json!({"name": "All", "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]})), delete);
    assert_eq!(ask(&s, "rules.move", json!({"id": purge, "direction": "up"})), delete);
    // Purge itself going off or going only deletes less.
    assert_eq!(ask(&s, "rules.enable", json!({"id": purge, "value": false})), settings);
    assert_eq!(ask(&s, "rules.remove", json!({"id": purge})), settings);
    // Another account's mail never reaches Purge.
    assert_eq!(ask(&s, "rules.add", tagger(local)), settings);
    for (id, params) in [
        ("rules.move", json!({"id": other, "direction": "up"})),
        ("rules.enable", json!({"id": other, "value": false})),
        ("rules.remove", json!({"id": other})),
        ("rules.enable", json!({"id": purge, "value": false})),
        ("rules.remove", json!({"id": purge})),
        ("rules.add", tagger(local)),
    ] {
        assert_eq!(ask(&s, id, params.clone()), settings, "{id} {params}");
        let said = crate::agent::describe(&s, id, &params);
        assert!(!said.contains("switched on here"), "{id} {params}: {said}");
    }
    // Nor is a switch the command refuses said to let more mail reach Purge.
    let said = crate::agent::describe(&s, "rules.enable", &json!({"id": keep, "value": "yes"}));
    assert!(!said.contains("Purge"), "{said}");
    // Switched off again, Keep switched on is asked too.
    run(&mut s, "rules.enable", json!({"id": keep, "value": false}));
    assert_eq!(ask(&s, "rules.enable", json!({"id": keep, "value": true})), delete);
    run(&mut s, "rules.enable", json!({"id": keep, "value": true}));
    // Purge off deletes nothing.
    run(&mut s, "rules.enable", json!({"id": purge, "value": false}));
    for (id, params) in changes(keep) {
        assert_eq!(ask(&s, id, params.clone()), settings, "{id} {params}");
    }
}

#[test]
fn an_agents_undo_or_redo_that_leaves_a_rule_on_that_deletes_for_good_asks_as_a_delete() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, account, _, _) = session();
    let undo = |s: &Session, redo: bool| classify(s, if redo { "edit.redo" } else { "edit.undo" }, &json!({}));
    let (delete, settings) = (Access::Owner(Class::Delete), Access::Owner(Class::Settings));
    let purge = add(&mut s, account, "Purge", json!([{"type": "category", "value": "Bin"}]), json!([{"type": "deleteForGood"}]));
    // Undoing its making only deletes less; redoing it brings it back.
    assert_eq!(undo(&s, false), settings);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(undo(&s, true), delete);
    run(&mut s, "edit.redo", json!({}));
    // Switched off, it is switched on again by an undo.
    run(&mut s, "rules.enable", json!({"id": purge, "value": false}));
    assert_eq!(undo(&s, false), delete);
    run(&mut s, "edit.undo", json!({}));
    // While it is on, undoing a rule made beside it changes what reaches it.
    add(&mut s, account, "Keep", json!([{"type": "toMe"}]), json!([{"type": "markRead"}]));
    assert_eq!(undo(&s, false), delete);
    // Deleted, it comes back by an undo, and the question says what it does.
    run(&mut s, "rules.remove", json!({"id": purge}));
    assert_eq!(undo(&s, false), delete);
    assert!(describe(&s, "edit.undo", &json!({})).contains("the rules, with one switched on that deletes mail permanently, back"));
}
