// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Changing a rule after it is made (#566, Outlook's Change Rule): on this computer, and on a
//! Microsoft account's server, where the change goes whole and survives a read of the server's
//! rules until the server has it. The server's side is written out by hand, as `mailcraft-net`
//! brings it.

use mailcraft_model::remote_rules::{RemoteRule, RuleOpKind, RuleOutcome, RuleResult, RulesDownload, RulesUploaded};
use mailcraft_model::{Action, Condition, Id, Keep, Rule};
use serde_json::{Value, json};

use super::rules_remote::{apply_rule_changes, mark_sent, merge_rules, rule_upload};
use crate::Session;

const EMAIL: &str = "owner@example.org";

/// A session with a Microsoft account kept in step through Graph and one of this computer: their
/// ids.
fn session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": EMAIL, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let local = s.run("account.add", &json!({"email": "me@example.org"})).unwrap()["id"].as_u64().unwrap();
    (s, account, local)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn rule(s: &Session, id: Id) -> &Rule {
    s.mailbox.rules.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no rule {id}"))
}

fn invoices(id: &str) -> RemoteRule {
    RemoteRule {
        id: id.into(),
        name: "Invoices".into(),
        sequence: 1,
        enabled: true,
        conditions: vec![Condition::SubjectContains(vec!["INVOICE".into()])],
        actions: vec![Action::MarkRead],
        ..Default::default()
    }
}

fn download(account: Id, rules: Vec<RemoteRule>) -> RulesDownload {
    RulesDownload { account, email: EMAIL.into(), complete: true, rules }
}

/// The server's rule "Invoices", taken in: its id here.
fn on_server(s: &mut Session, account: Id) -> Id {
    merge_rules(&mut s.mailbox, &download(account, vec![invoices("r1")])).unwrap();
    s.mailbox.rules.iter().find(|r| r.name == "Invoices").unwrap().id
}

/// The change that makes "Invoices" look for receipts and categorise them too.
fn receipts(id: Id) -> Value {
    json!({"id": id, "name": "Receipts", "conditions": [{"type": "subjectContains", "value": "receipt"}],
           "actions": [{"type": "markRead"}, {"type": "categorize", "value": "Finance"}], "stop": true})
}

/// What the server answers to the upload of `account`: `result` for each change.
fn answer(s: &mut Session, account: Id, result: RuleResult) {
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(s, &upload);
    let outcomes = upload
        .ops
        .iter()
        .map(|op| RuleOutcome {
            kind: op.kind,
            rule: op.rule,
            id: op.id.clone(),
            result,
            enabled: op.enabled,
            sequence: op.sequence,
            edit: op.edit,
            status: if result == RuleResult::Done { 200 } else { 400 },
            ..Default::default()
        })
        .collect();
    apply_rule_changes(s, &RulesUploaded { account, email: EMAIL.into(), outcomes, token_refused: false }).unwrap();
}

#[test]
fn a_rule_here_is_changed_whole_and_keeps_its_place_and_switch() {
    let (mut s, _, local) = session();
    let first = run(
        &mut s,
        "rules.add",
        json!({"name": "First", "account": local, "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "flag"}]}),
    );
    let first = first["id"].as_u64().unwrap();
    let second =
        run(&mut s, "rules.add", json!({"name": "Second", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]}));
    let second = second["id"].as_u64().unwrap();
    run(&mut s, "rules.enable", json!({"id": first, "value": false}));
    let changed = run(
        &mut s,
        "rules.edit",
        json!({"id": first, "name": "Big ones", "conditions": [{"type": "sizeBetween", "value": {"min": 1000, "max": 9000}}],
               "exceptions": [{"type": "fromContains", "value": "boss"}], "actions": [{"type": "clearFlag"}], "alert": true}),
    );
    assert_eq!(changed, json!({"id": first, "keep": "here"}));
    let r = rule(&s, first);
    assert_eq!(r.name, "Big ones");
    assert_eq!(r.conditions, [Condition::SizeBetween { min: 1000, max: 9000 }]);
    assert_eq!(r.exceptions, [Condition::FromContains(vec!["boss".into()])]);
    assert_eq!(r.actions, [Action::ClearFlag]);
    assert!(r.alert && !r.stop && !r.enabled, "switched off as it was");
    assert_eq!((r.account, r.keep), (local, Keep::Here));
    let order: Vec<Id> = s.mailbox.rules.iter().map(|r| r.id).collect();
    assert_eq!(order, [first, second]);
    // One undo step puts it back as it was.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(rule(&s, first).name, "First");
    assert_eq!(rule(&s, first).actions, [Action::Flag]);
}

#[test]
fn a_rule_on_the_server_is_changed_there_whole_and_a_read_meanwhile_keeps_the_change() {
    let (mut s, account, _) = session();
    let id = on_server(&mut s, account);
    assert_eq!(run(&mut s, "rules.edit", receipts(id)), json!({"id": id, "keep": "server"}));
    assert!(rule(&s, id).changed_here());
    // The server still has it as it was: that is the old word, and the change stays.
    merge_rules(&mut s.mailbox, &download(account, vec![invoices("r1")])).unwrap();
    assert_eq!(rule(&s, id).name, "Receipts");
    assert_eq!(rule(&s, id).conditions, [Condition::SubjectContains(vec!["receipt".into()])]);
    let upload = rule_upload(&s.mailbox, account).unwrap();
    assert_eq!(upload.ops.len(), 1);
    let op = &upload.ops[0];
    assert_eq!((op.kind, op.id.as_str(), op.edit, op.enabled, op.sequence), (RuleOpKind::Update, "r1", 1, None, None));
    let body = op.body.as_ref().unwrap();
    assert_eq!(body.name, "Receipts");
    assert_eq!(body.actions, [Action::MarkRead, Action::Categorize("Finance".into())]);
    assert!(body.stop);
    answer(&mut s, account, RuleResult::Done);
    assert!(!rule(&s, id).changed_here());
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
    // Read back as the server now has it, in its capitals: the same rule, nothing changed.
    let mut there = invoices("r1");
    there.name = "Receipts".into();
    there.conditions = vec![Condition::SubjectContains(vec!["RECEIPT".into()])];
    there.actions = vec![Action::MarkRead, Action::Categorize("Finance".into())];
    there.stop = true;
    assert_eq!(merge_rules(&mut s.mailbox, &download(account, vec![there])).unwrap().unchanged, 1);
    // A change that changes nothing isn't sent.
    run(&mut s, "rules.edit", receipts(id));
    assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty());
}

#[test]
fn a_change_made_again_while_the_first_was_on_its_way_goes_again() {
    let (mut s, account, _) = session();
    let id = on_server(&mut s, account);
    run(&mut s, "rules.edit", receipts(id));
    let upload = rule_upload(&s.mailbox, account).unwrap();
    let mut again = receipts(id);
    again["name"] = json!("Receipts and bills");
    run(&mut s, "rules.edit", again);
    let outcomes = upload
        .ops
        .iter()
        .map(|op| RuleOutcome {
            kind: op.kind,
            rule: op.rule,
            id: op.id.clone(),
            result: RuleResult::Done,
            edit: op.edit,
            status: 200,
            ..Default::default()
        })
        .collect();
    apply_rule_changes(&mut s, &RulesUploaded { account, email: EMAIL.into(), outcomes, token_refused: false }).unwrap();
    let next = rule_upload(&s.mailbox, account).unwrap();
    assert_eq!(next.ops.len(), 1);
    assert_eq!(next.ops[0].body.as_ref().unwrap().name, "Receipts and bills");
}

#[test]
fn a_change_the_server_refuses_gives_way_to_the_servers_rule_at_the_next_read() {
    let (mut s, account, _) = session();
    let id = on_server(&mut s, account);
    run(&mut s, "rules.edit", receipts(id));
    answer(&mut s, account, RuleResult::Refused);
    assert!(!rule(&s, id).changed_here());
    merge_rules(&mut s.mailbox, &download(account, vec![invoices("r1")])).unwrap();
    assert_eq!(rule(&s, id).name, "Invoices");
    assert_eq!(rule(&s, id).actions, [Action::MarkRead]);
    // Not given leave to change the server's rules: it waits, and goes once it is.
    run(&mut s, "rules.edit", receipts(id));
    answer(&mut s, account, RuleResult::Denied);
    assert!(rule(&s, id).changed_here());
    assert_eq!(rule_upload(&s.mailbox, account).unwrap().ops[0].edit, 1);
}

/// The server refuses a change while the owner has changed the rule again: the newer change is
/// kept, survives the next read and goes up, rather than give way to the server's rule.
#[test]
fn a_change_made_again_while_a_refused_one_was_on_its_way_goes_again() {
    let (mut s, account, _) = session();
    let id = on_server(&mut s, account);
    run(&mut s, "rules.edit", receipts(id));
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    let mut again = receipts(id);
    again["name"] = json!("Receipts and bills");
    run(&mut s, "rules.edit", again);
    let outcomes = upload
        .ops
        .iter()
        .map(|op| RuleOutcome {
            kind: op.kind,
            rule: op.rule,
            id: op.id.clone(),
            result: RuleResult::Refused,
            edit: op.edit,
            status: 400,
            ..Default::default()
        })
        .collect();
    apply_rule_changes(&mut s, &RulesUploaded { account, email: EMAIL.into(), outcomes, token_refused: false }).unwrap();
    assert!(rule(&s, id).changed_here());
    merge_rules(&mut s.mailbox, &download(account, vec![invoices("r1")])).unwrap();
    assert_eq!(rule(&s, id).name, "Receipts and bills");
    let next = rule_upload(&s.mailbox, account).unwrap();
    assert_eq!(next.ops.len(), 1);
    assert_eq!(next.ops[0].body.as_ref().unwrap().name, "Receipts and bills");
}

/// A rule changed here that the server, before the change goes, no longer lets be changed, or
/// gives parts MailCraft can't show: the server's rule stays, the change made here is let go of,
/// and that is said.
#[test]
fn a_change_to_a_rule_the_server_locks_meanwhile_is_let_go_of_and_said() {
    for locked in [true, false] {
        let (mut s, account, _) = session();
        let id = on_server(&mut s, account);
        run(&mut s, "rules.edit", receipts(id));
        let mut there = invoices("r1");
        if locked {
            there.read_only = true;
        } else {
            there.unknown = vec!["forwardTo".into()];
        }
        let merged = merge_rules(&mut s.mailbox, &download(account, vec![there])).unwrap();
        assert!(merged.messages.iter().any(|m| m.contains("let go of")), "{locked}: {:?}", merged.messages);
        assert_eq!((rule(&s, id).name.as_str(), rule(&s, id).actions.clone()), ("Invoices", vec![Action::MarkRead]), "{locked}");
        assert!(!rule(&s, id).changed_here(), "{locked}");
        assert!(rule_upload(&s.mailbox, account).unwrap().ops.is_empty(), "{locked}");
    }
}

#[test]
fn a_rule_waiting_to_go_to_the_server_goes_as_it_was_changed() {
    let (mut s, account, _) = session();
    let made =
        run(&mut s, "rules.add", json!({"name": "New", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]}));
    let id = made["id"].as_u64().unwrap();
    assert!(rule(&s, id).waiting());
    run(&mut s, "rules.edit", receipts(id));
    let upload = rule_upload(&s.mailbox, account).unwrap();
    assert_eq!((upload.ops[0].kind, upload.ops[0].body.as_ref().map(|b| b.name.as_str())), (RuleOpKind::Create, Some("Receipts")));
}

#[test]
fn what_cant_be_changed_is_refused_and_left_as_it_was() {
    let (mut s, account, _) = session();
    let mut locked = invoices("r1");
    locked.read_only = true;
    let mut unknown = invoices("r2");
    unknown.name = "Forward".into();
    unknown.unknown = vec!["forwardTo".into()];
    merge_rules(&mut s.mailbox, &download(account, vec![locked, unknown])).unwrap();
    let ids: Vec<Id> = s.mailbox.rules.iter().map(|r| r.id).collect();
    let waiting =
        run(&mut s, "rules.add", json!({"name": "Sent", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]}));
    let waiting = waiting["id"].as_u64().unwrap();
    let upload = rule_upload(&s.mailbox, account).unwrap();
    mark_sent(&mut s, &upload);
    let before = s.mailbox.rules.clone();
    for (id, said) in [(ids[0], "doesn't let"), (ids[1], "can't show"), (waiting, "still making")] {
        let e = s.run("rules.edit", &receipts(id)).unwrap_err().to_string();
        assert!(e.contains(said), "{id}: {e}");
    }
    // A rule the server keeps can have only what the server can do.
    let ok = on_server_named(&mut s, account, "r3");
    let mut flags = receipts(ok);
    flags["actions"] = json!([{"type": "flag"}]);
    assert!(s.run("rules.edit", &flags).unwrap_err().to_string().contains("flag"));
    let mut alert = receipts(ok);
    alert["alert"] = json!(true);
    assert!(s.run("rules.edit", &alert).unwrap_err().to_string().contains("alert"));
    // Nor is a rule that isn't there, or one without a condition or an action.
    assert!(s.run("rules.edit", &receipts(999)).is_err());
    let mut empty = receipts(ok);
    empty["conditions"] = json!([]);
    assert!(s.run("rules.edit", &empty).is_err());
    assert_eq!(s.mailbox.rules.iter().filter(|r| r.id != ok).cloned().collect::<Vec<_>>(), before);
    assert_eq!(rule(&s, ok).name, "Invoices");
}

/// A second rule of the server, called `remote`, taken in beside those there: its id here.
fn on_server_named(s: &mut Session, account: Id, remote: &str) -> Id {
    let mut all: Vec<RemoteRule> = s
        .mailbox
        .rules
        .iter()
        .filter(|r| r.on_server())
        .map(|r| {
            let server = r.server.clone().unwrap_or_default();
            RemoteRule {
                id: server.id,
                name: r.name.clone(),
                sequence: server.sequence,
                enabled: r.enabled,
                read_only: server.read_only,
                conditions: r.conditions.clone(),
                actions: r.actions.clone(),
                unknown: server.unknown,
                ..Default::default()
            }
        })
        .collect();
    let mut more = invoices(remote);
    more.sequence = 9;
    all.push(more);
    merge_rules(&mut s.mailbox, &RulesDownload { account, email: EMAIL.into(), complete: false, rules: all }).unwrap();
    s.mailbox.rules.iter().find(|r| r.server.as_ref().is_some_and(|x| x.id == remote)).unwrap().id
}

#[test]
fn an_agent_asks_the_owner_as_for_a_new_rule() {
    use crate::agent::{Access, Class, classify};
    let (mut s, account, local) = session();
    let id = on_server(&mut s, account);
    assert_eq!(classify(&s, "rules.edit", &receipts(id)), Access::Owner(Class::Settings));
    // Changed to delete mail, on the server where undo doesn't reach.
    let mut bin = receipts(id);
    bin["actions"] = json!([{"type": "delete"}]);
    assert_eq!(classify(&s, "rules.edit", &bin), Access::Owner(Class::Delete));
    // Switched off, it deletes nothing until it is switched on, which asks then.
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    assert_eq!(classify(&s, "rules.edit", &bin), Access::Owner(Class::Settings));
    // One here changed to delete for good.
    let here =
        run(&mut s, "rules.add", json!({"name": "Here", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]}));
    let here = here["id"].as_u64().unwrap();
    let mut gone = receipts(here);
    gone["actions"] = json!([{"type": "deleteForGood"}]);
    assert_eq!(classify(&s, "rules.edit", &gone), Access::Owner(Class::Delete));
    let said = crate::agent::describe(&s, "rules.edit", &receipts(id));
    assert!(said.contains("\u{201c}Invoices\u{201d}") && said.contains("\u{201c}Receipts\u{201d}") && said.contains("server"), "{said}");
}

#[test]
fn a_change_is_asked_about_for_what_the_rule_becomes() {
    use crate::agent::{Access, Class, Verdict, check, classify};
    let (mut s, account, local) = session();
    let gone = json!({"name": "Gone", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "deleteForGood"}]});
    let id = run(&mut s, "rules.add", gone)["id"].as_u64().unwrap();
    // Changed so that it no longer deletes for good: no delete, though it did.
    let read = json!({"id": id, "name": "Read", "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]});
    assert_eq!(classify(&s, "rules.edit", &read), Access::Owner(Class::Settings));
    // Changed to copy onto the Microsoft account's server and still delete for good: one
    // question, as a delete, that says both, until both are allowed.
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    let both = json!({"id": id, "name": "Both", "conditions": [{"type": "toMe"}],
                      "actions": [{"type": "copyTo", "value": inbox}, {"type": "deleteForGood"}]});
    for allow in [&[][..], &[Class::Send][..]] {
        match check(&s, "rules.edit", &both, allow) {
            Verdict::Ask(Class::Delete, said) => {
                assert!(said.contains(&format!("copy it onto the server of {EMAIL}")) && said.contains("delete it permanently"), "{said}");
            }
            other => panic!("{allow:?}: {other:?}"),
        }
    }
    assert!(matches!(check(&s, "rules.edit", &both, &[Class::Send, Class::Delete]), Verdict::Run));
}

#[test]
fn a_change_asked_about_for_a_rule_beside_it_names_that_rule() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, _, local) = session();
    let plain = json!({"name": "Read", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]});
    let id = run(&mut s, "rules.add", plain)["id"].as_u64().unwrap();
    let change = json!({"id": id, "name": "Read more", "conditions": [{"type": "toOrCcMe"}], "actions": [{"type": "markRead"}]});
    assert_eq!(classify(&s, "rules.edit", &change), Access::Owner(Class::Settings));
    assert!(!describe(&s, "rules.edit", &change).contains("permanently"));
    // A rule here that deletes for good: the change is asked as a delete, and says why.
    let gone =
        json!({"name": "Gone", "account": local, "conditions": [{"type": "fromContains", "value": "spam"}], "actions": [{"type": "deleteForGood"}]});
    run(&mut s, "rules.add", gone);
    assert_eq!(classify(&s, "rules.edit", &change), Access::Owner(Class::Delete));
    let said = describe(&s, "rules.edit", &change);
    assert!(said.contains("the rule \u{201c}Gone\u{201d}, switched on here, deletes mail permanently"), "{said}");
    // One of another account's mail isn't named; one for every account's is, beside it.
    let other = s.run("account.add", &json!({"email": "else@example.org"})).unwrap()["id"].as_u64().unwrap();
    let theirs = json!({"name": "Theirs", "account": other, "conditions": [{"type": "toMe"}], "actions": [{"type": "deleteForGood"}]});
    run(&mut s, "rules.add", theirs);
    // One for every account's mail is one from before rules had accounts, read from its file:
    // `rules.add` makes none (#566).
    let every = s.mailbox.alloc_id();
    let (conditions, actions) = (vec![Condition::FromContains(vec!["ads".into()])], vec![Action::DeleteForGood]);
    s.mailbox.rules.push(Rule { id: every, name: "Every".into(), enabled: true, conditions, actions, ..Default::default() });
    let said = describe(&s, "rules.edit", &change);
    assert!(
        said.contains("the rules \u{201c}Gone\u{201d}, \u{201c}Every\u{201d}, switched on here, delete mail permanently") && !said.contains("Theirs"),
        "{said}"
    );
    // Changed itself, "Gone" is weighed by what it becomes, and isn't named beside itself; "Every"
    // still is.
    let gone_id = s.mailbox.rules.iter().find(|r| r.name == "Gone").unwrap().id;
    let kept =
        json!({"id": gone_id, "name": "Gone", "conditions": [{"type": "fromContains", "value": "junk"}], "actions": [{"type": "deleteForGood"}]});
    let said = describe(&s, "rules.edit", &kept);
    assert!(
        said.contains("delete it permanently; the rule \u{201c}Every\u{201d}, switched on here,") && !said.contains("\u{201c}Gone\u{201d}, "),
        "{said}"
    );
}

/// A rule for every account's mail, as one from before rules had accounts is read from its file
/// (`rules.add` makes none, #566), doing `actions`: its id.
fn for_every_account(s: &mut Session, name: &str, actions: Vec<Action>) -> Id {
    let id = s.mailbox.alloc_id();
    let conditions = vec![Condition::FromContains(vec!["news".into()])];
    s.mailbox.rules.push(Rule { id, name: name.into(), enabled: true, conditions, actions, ..Default::default() });
    id
}

/// Given to one account (#566), a rule for every account is that account's alone, kept on this
/// computer and first of its rules there, where it ran before them; it leaves All accounts and
/// no other account's mail. One of an account already, or given to a shared mailbox, an account
/// that isn't there or one of a kind this version doesn't know, is refused.
#[test]
fn a_rule_for_every_account_is_given_to_one_and_runs_where_it_did() {
    let (mut s, account, local) = session();
    let theirs =
        run(&mut s, "rules.add", json!({"name": "Theirs", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]}));
    let theirs = theirs["id"].as_u64().unwrap();
    let every = for_every_account(&mut s, "Every", vec![Action::Flag]);
    let names = |s: &Session, a: Id| s.mailbox.rules_in_order(a).iter().map(|i| s.mailbox.rules[*i].name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s, local), ["Every", "Theirs"]);
    let said = run(&mut s, "rules.forAccount", json!({"id": every, "account": local}));
    assert_eq!(said, json!({"id": every, "account": local}));
    let r = rule(&s, every);
    assert_eq!((r.account, r.keep, r.enabled), (local, Keep::Here, true));
    assert_eq!(names(&s, local), ["Every", "Theirs"], "it runs where it ran");
    assert_eq!(names(&s, account), Vec::<String>::new(), "and not on the other account's mail");
    let before = s.mailbox.rules.clone();
    for (params, said) in [
        (json!({"id": every, "account": local}), "one account already"),
        (json!({"id": theirs, "account": account}), "one account already"),
        (json!({"id": every, "account": 999}), "no account"),
        (json!({"id": 999, "account": local}), "no rule"),
    ] {
        let e = s.run("rules.forAccount", &params).unwrap_err().to_string();
        assert!(e.contains(said), "{params}: {e}");
    }
    for params in [json!({"id": every}), json!({"id": every, "account": "2"}), json!({"account": local})] {
        assert!(matches!(s.run("rules.forAccount", &params), Err(crate::CmdError::Params(_))), "{params}");
    }
    assert_eq!(s.mailbox.rules, before);
    // An Undo puts it back for every account.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(rule(&s, every).account, 0);
}

/// A rule for every account that moves mail to one account's folder: given to that account, it
/// keeps its folder; given to another, it is refused until the folder is chosen again, which a
/// change given with `account` does (#566). A rule for every account isn't changed without one.
#[test]
fn a_rule_given_to_another_account_than_its_folders_has_the_folder_chosen_again() {
    let (mut s, account, local) = session();
    let archive = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Archive).unwrap();
    let mine = s.mailbox.folder_of_kind(local, mailcraft_model::FolderKind::Archive).unwrap();
    let every = for_every_account(&mut s, "News", vec![Action::MoveTo(archive), Action::MarkRead]);
    let e = s.run("rules.forAccount", &json!({"id": every, "account": local})).unwrap_err().to_string();
    assert!(e.contains("another account's folder") && e.contains("rules.edit"), "{e}");
    assert_eq!(rule(&s, every).account, 0);
    // A change of a rule for every account says which account it is then for.
    let change = |folder: Id| {
        json!({"id": every, "name": "News", "conditions": [{"type": "fromContains", "value": "news"}],
               "actions": [{"type": "moveTo", "value": folder}, {"type": "markRead"}]})
    };
    let e = s.run("rules.edit", &change(mine)).unwrap_err().to_string();
    assert!(e.contains("`account`"), "{e}");
    let mut given = change(mine);
    given["account"] = json!(local);
    run(&mut s, "rules.edit", given);
    let r = rule(&s, every);
    assert_eq!((r.account, r.keep, r.actions.clone()), (local, Keep::Here, vec![Action::MoveTo(mine), Action::MarkRead]));
    // Undone, it is the rule for every account it was; redone, the account's again.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!((rule(&s, every).account, rule(&s, every).actions.clone()), (0, vec![Action::MoveTo(archive), Action::MarkRead]));
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(rule(&s, every).account, local);
    // Changed again, it stays for its account.
    let mut elsewhere = change(mine);
    elsewhere["account"] = json!(account);
    assert!(s.run("rules.edit", &elsewhere).unwrap_err().to_string().contains("stays for its account"));
    // Another such rule, given to the account whose folder it names, keeps it.
    let other = for_every_account(&mut s, "Archived", vec![Action::MoveTo(archive)]);
    run(&mut s, "rules.forAccount", json!({"id": other, "account": account}));
    assert_eq!((rule(&s, other).account, rule(&s, other).actions.clone()), (account, vec![Action::MoveTo(archive)]));
}

/// An agent asks the owner before it gives a rule to one account, as for any change to the
/// rules, and the question says what it does (#566).
#[test]
fn an_agent_asks_before_it_gives_a_rule_to_one_account() {
    use crate::agent::{Access, Class, classify, describe};
    let (mut s, account, local) = session();
    let every = for_every_account(&mut s, "Every", vec![Action::Flag]);
    let params = json!({"id": every, "account": local});
    assert_eq!(classify(&s, "rules.forAccount", &params), Access::Owner(Class::Settings));
    let said = describe(&s, "rules.forAccount", &params);
    assert!(said.contains("\u{201c}Every\u{201d}") && said.contains("every account") && said.contains("me@example.org"), "{said}");
    // Given as it is changed: the question names the account it is then for.
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    let change = json!({"id": every, "account": account, "name": "Every", "conditions": [{"type": "toMe"}],
                        "actions": [{"type": "moveTo", "value": inbox}]});
    let said = describe(&s, "rules.edit", &change);
    assert!(said.contains("every account") && said.contains(&format!("for the mail of \u{201c}{EMAIL}\u{201d} alone")), "{said}");
    // A rule here that deletes for good is named, as for any change to the rules.
    let gone = json!({"name": "Gone", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "deleteForGood"}]});
    run(&mut s, "rules.add", gone);
    assert_eq!(classify(&s, "rules.forAccount", &params), Access::Owner(Class::Delete));
}
