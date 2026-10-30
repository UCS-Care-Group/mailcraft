// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules kept on a Microsoft account's server (#566): which rules go there, what is told to the
//! server, what it made of it, and its rules merged. The server's side is written out by hand, as
//! `mailcraft-net` brings it.

use mailcraft_model::remote_rules::{RemoteRule, RuleOpKind, RuleOutcome, RuleResult, RulesDownload, RulesUploaded};
use mailcraft_model::{Action, Address, Condition, FolderKind, Id, Keep, Message, Rule, ServerFolder};
use serde_json::{Value, json};

use super::rules_remote::{apply_rule_changes, apply_rules, mark_sent, merge_rules, rule_upload};
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
        conditions: vec![Condition::SubjectContains("INVOICE".into())],
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
    assert_eq!(s.mailbox.apply_rules(first), ["News"]);
    assert_eq!(s.mailbox.message(first).unwrap().folder, news);
    all_taken(&mut s, account);
    assert!(rule(&s, id).on_server());
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    // Now the server runs it: mail that comes in is left as the server left it.
    let second = arriving(&mut s, account, "digest@example.org", "November");
    assert!(s.mailbox.apply_rules(second).is_empty());
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
    assert!(s.mailbox.apply_rules(m).is_empty());
    assert_eq!(s.mailbox.message(m).unwrap().flag, mailcraft_model::FlagStatus::None);
    let other = arriving(&mut s, account, "billing@example.org", "Receipt");
    assert!(s.mailbox.apply_rules(other).is_empty());
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
    shouted.conditions = vec![Condition::SubjectContains("RECEIPT".into())];
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Invoices", 1), shouted])).unwrap();
    assert_eq!(rule(&s, mine).conditions, [Condition::SubjectContains("Receipt".into())]);
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
    made.conditions = vec![Condition::SubjectContains("receipt".into())];
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
fn an_agent_asks_the_owner_before_it_deletes_a_rule_the_server_has_and_says_where_a_new_one_goes() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, account, _, news) = session();
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r1", "Server's", 1)])).unwrap();
    let there = s.mailbox.rules[0].id;
    let here = add(&mut s, account, "Here", json!([{"type": "hasAttachment"}]), json!([{"type": "flag"}]));
    assert_eq!(classify(&s, "rules.remove", &json!({"id": there})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "rules.remove", &json!({"id": here})), Access::Owner(Class::Settings));
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
    // Deleted on the web: gone here too, unless it was changed here since.
    merge_rules(&mut s.mailbox, &download(account, vec![remote("r2", "Two", 1), remote("r3", "Three", 2)])).unwrap();
    let three = s.mailbox.rules.iter().find(|r| r.name == "Three").unwrap().id;
    run(&mut s, "rules.enable", json!({"id": three, "value": false}));
    let merged = merge_rules(&mut s.mailbox, &download(account, vec![])).unwrap();
    assert_eq!(merged.removed, 1);
    assert!(s.mailbox.rules.iter().all(|r| r.name != "Two"));
    assert!(rule(&s, three).waiting() && !rule(&s, three).enabled);
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
    assert_eq!(s.mailbox.apply_rules(m), ["News"]);
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
        conditions: vec![Condition::FromContains("Weekly".into())],
        actions: vec![Action::MarkRead],
        ..Default::default()
    });
    // By the sender's name too, on any account's mail.
    for a in [account, local] {
        let m = arriving(&mut s, a, "news@example.org", "x");
        assert_eq!(s.mailbox.apply_rules(m), ["Old"], "account {a}");
    }
    assert_eq!(run(&mut s, "rules.list", json!({"account": local}))["rules"][0]["kept"], "everyAccount");
    run(&mut s, "rules.keepOnServer", json!({"id": 900}));
    assert_eq!((rule(&s, 900).account, rule(&s, 900).keep), (account, Keep::Server));
    // Now the Microsoft account's alone, and by the address, as its server reads it.
    let there = arriving(&mut s, local, "news@example.org", "x");
    assert!(s.mailbox.apply_rules(there).is_empty());
    let by_name = arriving(&mut s, account, "news@example.org", "x");
    assert!(s.mailbox.apply_rules(by_name).is_empty());
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
    there.conditions = vec![Condition::FromContains("DIGEST".into())];
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
    long.conditions = vec![Condition::SubjectContains("w".repeat(10_000)), Condition::BodyContains(String::new())];
    long.actions = vec![Action::MoveTo(1), Action::Categorize("\u{0}".into())];
    long.unknown = vec!["x".repeat(500); 100];
    hostile.push(long);
    let merged = merge_rules(&mut s.mailbox, &download(account, hostile)).unwrap();
    assert_eq!((merged.added, merged.skipped), (2, 3));
    let long = s.mailbox.rules.iter().find(|r| r.name.starts_with('n')).unwrap();
    assert!(long.name.chars().count() <= mailcraft_model::MAX_NAME);
    assert_eq!(long.conditions.len(), 1);
    assert!(matches!(&long.conditions[0], Condition::SubjectContains(t) if t.chars().count() <= 200));
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
    run(&mut s, "rules.add", json!({"name": "Everyone", "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "markRead"}]}));
    run(&mut s, "account.remove", json!({"id": account}));
    let left: Vec<(&str, Id)> = s.mailbox.rules.iter().map(|r| (r.name.as_str(), r.account)).collect();
    assert_eq!(left, [("Local", local), ("Everyone", 0)]);
    // So the account's "delete the digests" acts on no other account's mail.
    let m = arriving(&mut s, local, "digest@news.example", "Weekly digest");
    s.mailbox.apply_rules(m);
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
    assert_eq!(s.mailbox.apply_rules(m), ["Digest"]);
    // A rule for every account that looks for a name isn't moved to a server, which would read
    // addresses alone and so never match.
    s.mailbox.rules.push(Rule {
        id: 900,
        name: "Old".into(),
        enabled: true,
        conditions: vec![Condition::FromContains("Weekly Digest".into())],
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
        conditions: vec![Condition::HasAttachment, Condition::FromContains("DIGEST".into())],
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
    // So is a move out of the account (to one on this computer alone), and a server for a rule of every account or of an account
    // whose server keeps none.
    let inbox_here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let out = json!({"name": "Out", "account": account, "server": true, "conditions": digest, "actions": [{"type": "moveTo", "value": inbox_here}]});
    assert!(s.run("rules.add", &out).unwrap_err().to_string().contains("on this computer"));
    assert!(s.run("rules.add", &json!({"name": "All", "server": true, "conditions": digest, "actions": [{"type": "markRead"}]})).is_err());
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
