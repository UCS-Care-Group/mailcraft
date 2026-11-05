// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A rule that forwards or redirects mail (#566): only a Microsoft account's server does it, so
//! such a rule is that server's or nothing, does nothing here until the server has it, and is
//! left out of Run Rules Now; an agent asks for it as for sending. The server's side is written
//! out by hand, as `mailcraft-net` brings it.

use mailcraft_model::remote_rules::{RemoteRule, RuleOutcome, RuleResult, RulesDownload, RulesUploaded};
use mailcraft_model::{Action, Condition, FolderKind, Id, Keep, Message, Rule, ServerRule};
use serde_json::{Value, json};

use super::rules_remote::{apply_rule_changes, mark_sent, merge_rules, rule_upload};
use crate::Session;
use crate::agent::{Access, Class, classify, describe};

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

/// The rule "To the bookkeeper": invoices forwarded to the bookkeeper and marked read, for
/// `account`'s mail.
fn bookkeeper(account: Id) -> Value {
    json!({"name": "To the bookkeeper", "account": account, "conditions": [{"type": "subjectContains", "value": "invoice"}],
           "actions": [{"type": "forwardTo", "value": ["books@example.org", "Ann@Example.org"]}, {"type": "markRead"}]})
}

/// An invoice arrived in `account`'s Inbox: its id.
fn invoice(s: &mut Session, account: Id) -> Id {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.add_message(Message { folder, account, subject: "Invoice 42".into(), ..Default::default() }).unwrap()
}

/// The server has rule `id` now, by the id `there`.
fn taken(s: &mut Session, id: Id, there: &str) {
    let r = s.mailbox.rules.iter_mut().find(|r| r.id == id).unwrap();
    r.server = Some(ServerRule { id: there.into(), synced_enabled: true, ..Default::default() });
}

#[test]
fn a_rule_that_forwards_is_kept_on_a_microsoft_accounts_server_and_refused_elsewhere() {
    let (mut s, account, local) = session();
    let made = run(&mut s, "rules.add", bookkeeper(account));
    assert_eq!(made["keep"], "server");
    let id = made["id"].as_u64().unwrap();
    assert_eq!(rule(&s, id).actions[0], Action::ForwardTo(vec!["books@example.org".into(), "Ann@Example.org".into()]));
    assert!(rule(&s, id).forwards() && rule(&s, id).waiting());
    // Never on this computer: not asked for, not on an account whose rules stay here.
    let mut here = bookkeeper(account);
    here["server"] = json!(false);
    let refused = [
        here,
        bookkeeper(local),
        // More people than a rule forwards to, all its forwarding together; not an address.
        json!({"name": "Many", "account": account, "conditions": [{"type": "toMe"}],
               "actions": [{"type": "forwardTo", "value": (1..=6).map(|i| format!("a{i}@example.org")).collect::<Vec<_>>()},
                           {"type": "redirectTo", "value": (1..=5).map(|i| format!("b{i}@example.org")).collect::<Vec<_>>()}]}),
        json!({"name": "Odd", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "forwardTo", "value": "Ann Smith"}]}),
        // What the server can't keep with it: one way twice, a flag.
        json!({"name": "Twice", "account": account, "conditions": [{"type": "toMe"}],
               "actions": [{"type": "forwardTo", "value": "a@example.org"}, {"type": "forwardTo", "value": "b@example.org"}]}),
        json!({"name": "Flagged", "account": account, "conditions": [{"type": "toMe"}],
               "actions": [{"type": "redirectTo", "value": "a@example.org"}, {"type": "flag"}]}),
    ];
    for params in refused {
        assert!(s.run("rules.add", &params).is_err(), "{params}");
    }
    assert_eq!(s.mailbox.rules.len(), 1);
    // A long address is read whole, never cut into somebody else's; one longer than an address
    // can be is refused.
    let long = format!("a@{}.{}.{}.{}.example", "b".repeat(60), "c".repeat(60), "d".repeat(60), "e".repeat(30));
    assert!(long.len() > 200 && long.len() <= 254);
    let made = json!({"name": "Long", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "forwardTo", "value": long}]});
    let long_id = run(&mut s, "rules.add", made)["id"].as_u64().unwrap();
    assert_eq!(rule(&s, long_id).actions, [Action::ForwardTo(vec![long.clone()])]);
    run(&mut s, "rules.remove", json!({"id": long_id}));
    let longer = format!("a@{}.{}", "f".repeat(40), long.trim_start_matches("a@"));
    assert!(longer.len() > 254);
    let refused =
        json!({"name": "Longer", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "forwardTo", "value": longer}]});
    assert!(s.run("rules.add", &refused).is_err());
    assert_eq!(s.mailbox.rules.len(), 1);
    // A rule kept here isn't changed into one that forwards: it is kept on the server first.
    let flags =
        run(&mut s, "rules.add", json!({"name": "Flags", "account": account, "conditions": [{"type": "toMe"}], "actions": [{"type": "flag"}]}));
    let flags = flags["id"].as_u64().unwrap();
    assert_eq!(rule(&s, flags).keep, Keep::Here);
    let forwarding = json!({"id": flags, "name": "Flags", "conditions": [{"type": "toMe"}],
                            "actions": [{"type": "forwardAsAttachmentTo", "value": "a@example.org"}]});
    assert!(s.run("rules.edit", &forwarding).is_err());
    assert_eq!(rule(&s, flags).actions, [Action::Flag]);
    // One kept on the server is.
    let changed = json!({"id": id, "name": "To the bookkeeper", "conditions": [{"type": "subjectContains", "value": "invoice"}],
                         "actions": [{"type": "redirectTo", "value": "books@example.org"}]});
    run(&mut s, "rules.edit", changed);
    assert_eq!(rule(&s, id).actions, [Action::RedirectTo(vec!["books@example.org".into()])]);
}

#[test]
fn it_does_nothing_here_until_the_server_has_it_and_run_rules_now_leaves_it_out() {
    let (mut s, account, _) = session();
    let id = run(&mut s, "rules.add", bookkeeper(account))["id"].as_u64().unwrap();
    let clock = s.rule_clock();
    // Waiting for the server: it doesn't run, so it marks nothing read here.
    let first = invoice(&mut s, account);
    assert!(s.mailbox.apply_rules(first, &clock).is_empty());
    assert!(!s.mailbox.message(first).unwrap().read);
    // The server has it: it ran there, before the mail came.
    taken(&mut s, id, "r1");
    let second = invoice(&mut s, account);
    assert!(s.mailbox.apply_rules(second, &clock).is_empty());
    assert!(!s.mailbox.message(second).unwrap().read);
    // Run Rules Now leaves it out, the rule alone or with the rest: nothing is forwarded from here.
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert_eq!(run(&mut s, "rules.run", json!({"folder": inbox})), json!({"matched": 0}));
    assert_eq!(run(&mut s, "rules.run", json!({"folder": inbox, "id": id})), json!({"matched": 0}));
    assert!(!s.mailbox.message(first).unwrap().read && !s.mailbox.message(second).unwrap().read);
    // Neither does one the server didn't take, kept here meanwhile.
    let r = s.mailbox.rules.iter_mut().find(|r| r.id == id).unwrap();
    (r.server, r.keep, r.refused) = (None, Keep::Here, "400".into());
    let third = invoice(&mut s, account);
    assert!(s.mailbox.apply_rules(third, &clock).is_empty());
    assert!(!s.mailbox.message(third).unwrap().read);
}

#[test]
fn forwarding_goes_up_whole_and_comes_back_as_the_same_rule() {
    let (mut s, account, _) = session();
    let id = run(&mut s, "rules.add", bookkeeper(account))["id"].as_u64().unwrap();
    let upload = rule_upload(&s.mailbox, account).unwrap();
    let made = upload.ops.first().and_then(|op| op.body.clone()).unwrap();
    assert!(made.actions.contains(&Action::ForwardTo(vec!["books@example.org".into(), "Ann@Example.org".into()])), "{:?}", made.actions);
    mark_sent(&mut s, &upload);
    let outcomes = upload
        .ops
        .iter()
        .map(|op| RuleOutcome { kind: op.kind, rule: op.rule, id: "r1".into(), result: RuleResult::Done, status: 201, ..Default::default() })
        .collect();
    apply_rule_changes(&mut s, &RulesUploaded { account, email: EMAIL.into(), outcomes, token_refused: false }).unwrap();
    assert!(rule(&s, id).on_server());
    // The server gives its people back in its own order and case: the same rule.
    let there = RemoteRule {
        id: "r1".into(),
        name: "To the bookkeeper".into(),
        sequence: 1,
        enabled: true,
        conditions: vec![Condition::SubjectContains(vec!["INVOICE".into()])],
        actions: vec![Action::MarkRead, Action::ForwardTo(vec!["ann@example.org".into(), "BOOKS@example.org".into()])],
        ..Default::default()
    };
    merge_rules(&mut s.mailbox, &RulesDownload { account, email: EMAIL.into(), complete: true, rules: vec![there] }).unwrap();
    assert_eq!(s.mailbox.rules.len(), 1);
    assert!(rule(&s, id).forwards() && rule(&s, id).on_server());
}

#[test]
fn an_agent_asks_as_for_sending_before_a_rule_forwards_mail() {
    let (mut s, account, local) = session();
    let params = bookkeeper(account);
    assert_eq!(classify(&s, "rules.add", &params), Access::Owner(Class::Send));
    let said = describe(&s, "rules.add", &params);
    assert!(said.contains("forward it to books@example.org, Ann@Example.org"), "{said}");
    let id = run(&mut s, "rules.add", params)["id"].as_u64().unwrap();
    taken(&mut s, id, "r1");
    // Beside it, a change to the server's rules of its account asks so too, and names it.
    let other =
        json!({"name": "News", "account": account, "conditions": [{"type": "fromContains", "value": "news"}], "actions": [{"type": "markRead"}]});
    assert_eq!(classify(&s, "rules.add", &other), Access::Owner(Class::Send));
    let said = describe(&s, "rules.add", &other);
    assert!(said.contains("\u{201c}To the bookkeeper\u{201d}, switched on, forwards mail"), "{said}");
    // One of this computer, or another account's, doesn't reach the server's rules.
    let mut here = other.clone();
    here["server"] = json!(false);
    assert_eq!(classify(&s, "rules.add", &here), Access::Owner(Class::Settings));
    let elsewhere = json!({"name": "Mine", "account": local, "conditions": [{"type": "toMe"}], "actions": [{"type": "markRead"}]});
    assert_eq!(classify(&s, "rules.add", &elsewhere), Access::Owner(Class::Settings));
    // Switching it off forwards less; switching it on again forwards.
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id, "value": false})), Access::Owner(Class::Settings));
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id, "value": true})), Access::Owner(Class::Send));
    // Run Rules Now leaves it out: nothing is sent.
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert_ne!(classify(&s, "rules.run", &json!({"folder": inbox})), Access::Owner(Class::Send));
    // An undo that brings it back switched on asks so too.
    run(&mut s, "rules.enable", json!({"id": id, "value": true}));
    run(&mut s, "rules.remove", json!({"id": id}));
    assert_eq!(s.undo_reverts(false), Some(Class::Send));
}
