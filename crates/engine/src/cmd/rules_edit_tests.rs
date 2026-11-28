// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Change Rule (`rules.edit`, #2057): a rule changed in place, keeping its id, its place and
//! whether it is on; MailCraft's Sieve script written again; what can't be changed refused.

use mailcraft_model::{Action, Condition, FolderKind, Id, Keep, Rule, ServerFolder, ServerRule};
use serde_json::{Value, json};

use super::rules::{NotEditable, cannot_edit, kept};
use super::sieve_replies::{SIEVE_ID, script_due};
use crate::agent::{Access, Class, access, classify, describe};
use crate::sieve::SCRIPT_NAME;
use crate::{Session, sample};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

const ALL: [&str; 7] = ["vacation", "date", "relational", "fileinto", "copy", "imap4flags", "body"];

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn rule(s: &Session, id: Id) -> &Rule {
    s.mailbox.rules.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no rule {id}"))
}

fn place_of(s: &Session, id: Id) -> usize {
    s.mailbox.rules.iter().position(|r| r.id == id).unwrap()
}

/// An IMAP account whose server keeps MailCraft's Sieve script, with a folder `Lists` there; the
/// account and the folder.
fn sieve_session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let added = run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}}));
    let account = added["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let lists = run(&mut s, "folder.new", json!({"name": "Lists", "parent": inbox}))["id"].as_u64().unwrap();
    s.mailbox.folder_mut(lists).unwrap().remote = "INBOX/Lists".into();
    let state = json!({"extensions": ALL, "scripts": [], "script": null});
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state}));
    assert!(s.mailbox.keeps_sieve_rules(account));
    (s, account, lists)
}

/// The script due written, as the server then has it and runs it.
fn written(s: &mut Session, account: Id) -> String {
    let text = script_due(s, account, false).unwrap()["text"].as_str().unwrap().to_string();
    let state = json!({"extensions": ALL, "scripts": [{"name": SCRIPT_NAME, "active": true}], "script": text});
    run(s, "sync.applySieve", json!({"account": account, "state": state, "written": {"result": "done", "text": text}}));
    text
}

#[test]
fn a_rule_of_this_computer_is_changed_in_place_and_undone() {
    let mut s = Session::at(sample::mailbox(NOW, 0), NOW, 0);
    let first = s.mailbox.rules[0].id;
    let account = s.mailbox.rules[0].account;
    let second = run(
        &mut s,
        "rules.add",
        json!({"name": "Second", "account": account, "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "flag"}]}),
    )["id"]
        .as_u64()
        .unwrap();
    run(&mut s, "rules.enable", json!({"id": first, "value": false}));
    let before = rule(&s, first).clone();
    let changed = run(
        &mut s,
        "rules.edit",
        json!({"id": first, "name": "Books", "conditions": [{"type": "subjectContains", "value": ["order", "receipt"]}],
            "exceptions": [{"type": "importance", "value": "low"}], "actions": [{"type": "markRead"}], "stop": true, "alert": true}),
    );
    assert_eq!(changed, json!({"id": first, "keep": "here"}));
    let r = rule(&s, first);
    assert_eq!((r.name.as_str(), r.stop, r.alert, r.enabled), ("Books", true, true, false), "switched off as it was");
    assert_eq!(r.conditions, vec![Condition::SubjectContains(vec!["order".into(), "receipt".into()])]);
    assert_eq!(r.actions, vec![Action::MarkRead]);
    assert_eq!(r.exceptions.len(), 1);
    // In its place, before the rule made after it.
    assert!(place_of(&s, first) < place_of(&s, second));
    assert_eq!(s.mailbox.rules.len(), 2);
    // Undone, it is as it was; done again, as changed.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(rule(&s, first), &before);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(rule(&s, first).name, "Books");
}

#[test]
fn what_a_change_asks_is_checked_as_a_new_rule_and_nothing_changes_when_refused() {
    let mut s = Session::at(sample::mailbox(NOW, 0), NOW, 0);
    let id = s.mailbox.rules[0].id;
    let account = s.mailbox.rules[0].account;
    let before = s.mailbox.rules.clone();
    let ok = json!({"type": "markRead"});
    for params in [
        json!({"id": id, "name": "", "conditions": [{"type": "hasAttachment"}], "actions": [ok]}),
        json!({"id": id, "name": "X", "conditions": [], "actions": [ok]}),
        json!({"id": id, "name": "X", "conditions": [{"type": "hasAttachment"}], "actions": []}),
        json!({"id": id, "name": "X", "conditions": [{"type": "nope"}], "actions": [ok]}),
        json!({"id": id, "name": "X", "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "moveTo", "value": 424242}]}),
        json!({"id": id, "name": "X", "conditions": [{"type": "hasAttachment"}], "actions": [ok], "exceptoins": []}),
        // Another account's, or a rule that isn't there.
        json!({"id": id, "name": "X", "conditions": [{"type": "hasAttachment"}], "actions": [ok], "account": account + 1}),
        json!({"id": 424242, "name": "X", "conditions": [{"type": "hasAttachment"}], "actions": [ok]}),
        json!({"name": "X", "conditions": [{"type": "hasAttachment"}], "actions": [ok]}),
    ] {
        assert!(s.run("rules.edit", &params).is_err(), "{params}");
    }
    assert_eq!(s.mailbox.rules, before);
    // Its own account may be named.
    run(&mut s, "rules.edit", json!({"id": id, "name": "X", "conditions": [{"type": "hasAttachment"}], "actions": [ok], "account": account}));
    // A rule for every account stays one, kept here.
    s.mailbox.rules[0].account = 0;
    run(&mut s, "rules.edit", json!({"id": id, "name": "Y", "conditions": [{"type": "hasAttachment"}], "actions": [ok]}));
    assert_eq!((rule(&s, id).account, rule(&s, id).keep), (0, Keep::Here));
    let server = json!({"id": id, "name": "Y", "conditions": [{"type": "hasAttachment"}], "actions": [ok], "server": true});
    assert!(s.run("rules.edit", &server).is_err());
}

#[test]
fn a_sieve_rule_changed_is_written_again_under_its_id_and_moves_here_when_the_server_cant_keep_it() {
    let (mut s, account, lists) = sieve_session();
    let mv = json!([{"type": "moveTo", "value": lists}]);
    let add = |s: &mut Session, name: &str, word: &str| {
        let made = run(
            s,
            "rules.add",
            json!({"name": name, "account": account, "conditions": [{"type": "fromContains", "value": word}], "actions": mv.clone()}),
        );
        assert_eq!(made["keep"], "server");
        made["id"].as_u64().unwrap()
    };
    let a = add(&mut s, "A", "alpha");
    let b = add(&mut s, "B", "beta");
    let text = written(&mut s, account);
    assert!(text.find(&format!("# rule {a}")).unwrap() < text.find(&format!("# rule {b}")).unwrap());
    assert!(rule(&s, a).on_server());

    // Changed: the script is to be written again, the rule under its id, still before B.
    let changed = run(
        &mut s,
        "rules.edit",
        json!({"id": a, "name": "A2", "account": account, "conditions": [{"type": "subjectContains", "value": "gamma"}], "actions": mv.clone()}),
    );
    assert_eq!(changed, json!({"id": a, "keep": "server"}));
    let due = script_due(&s, account, false).unwrap()["text"].as_str().unwrap().to_string();
    assert!(due.contains(&format!("# rule {a}\r\nif header :contains \"subject\" [\"gamma\"]")), "{due}");
    assert!(!due.contains("alpha"), "{due}");
    assert!(due.find(&format!("# rule {a}")).unwrap() < due.find(&format!("# rule {b}")).unwrap());
    written(&mut s, account);
    assert_eq!(rule(&s, a).server.as_ref().map(|s| s.id.clone()), Some(format!("{SIEVE_ID}{a}")));

    // A part Sieve can't say: kept here, with why, and out of the script.
    let attach = json!({"id": a, "name": "A2", "conditions": [{"type": "hasAttachment"}], "actions": mv.clone()});
    let here = run(&mut s, "rules.edit", attach);
    assert_eq!(here["keep"], "here");
    assert_eq!(here["why"], "the server's Sieve scripts can't look at attachments");
    assert!(rule(&s, a).server.is_none() && rule(&s, a).keep == Keep::Here);
    assert!(!script_due(&s, account, false).unwrap()["text"].as_str().unwrap().contains(&format!("# rule {a}")));
    written(&mut s, account);
    assert_eq!(kept(&s.mailbox, rule(&s, a)).0, "here");
    // Back to what the server can do: it goes there again, as it was kept here only for that.
    let back = json!({"id": a, "name": "A2", "conditions": [{"type": "fromContains", "value": "alpha"}], "actions": mv.clone()});
    assert_eq!(run(&mut s, "rules.edit", back.clone())["keep"], "server");
    // Kept here by the owner: it stays here when changed, unless asked.
    let mut here = back.clone();
    here["server"] = json!(false);
    assert_eq!(run(&mut s, "rules.edit", here)["keep"], "here");
    assert_eq!(run(&mut s, "rules.edit", back.clone())["keep"], "here");
    let mut there = back;
    there["server"] = json!(true);
    assert_eq!(run(&mut s, "rules.edit", there)["keep"], "server");
}

#[test]
fn a_rule_a_microsoft_server_keeps_the_owners_own_script_and_read_only_rules_are_refused() {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": "owner@example.org", "kind": "microsoft"}))["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), name: "Inbox".into(), ..Default::default() });
    let params =
        |id: Value| json!({"id": id, "name": "N", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "markRead"}]});
    let there = run(
        &mut s,
        "rules.add",
        json!({"name": "There", "account": account, "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "markRead"}]}),
    );
    assert_eq!(there["keep"], "server");
    let there = there["id"].as_u64().unwrap();
    let said = s.run("rules.edit", &params(json!(there))).unwrap_err().to_string();
    assert!(said.contains("Microsoft account's server"), "{said}");
    assert_eq!(cannot_edit(&s.mailbox, rule(&s, there)), Some(NotEditable::Microsoft));
    // One of the account's kept on this computer can be changed.
    let here = run(
        &mut s,
        "rules.add",
        json!({"name": "Here", "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "markRead"}]}),
    )["id"]
        .as_u64()
        .unwrap();
    assert_eq!(cannot_edit(&s.mailbox, rule(&s, here)), None);
    assert_eq!(run(&mut s, "rules.edit", params(json!(here)))["keep"], "here");
    // The server's own: read-only, or with parts not known here.
    let mut fixed = rule(&s, here).clone();
    fixed.id = s.mailbox.alloc_id();
    fixed.server = Some(ServerRule { id: "r1".into(), read_only: true, ..Default::default() });
    let mut odd = fixed.clone();
    odd.id = s.mailbox.alloc_id();
    odd.server = Some(ServerRule { id: "r2".into(), unknown: vec!["forwardTo".into()], ..Default::default() });
    let (fixed, odd) = (fixed, odd);
    s.mailbox.rules.push(fixed.clone());
    s.mailbox.rules.push(odd.clone());
    assert_eq!(cannot_edit(&s.mailbox, &fixed), Some(NotEditable::ReadOnly));
    assert_eq!(cannot_edit(&s.mailbox, &odd), Some(NotEditable::Unknown));
    for id in [fixed.id, odd.id] {
        assert!(s.run("rules.edit", &params(json!(id))).is_err());
    }
}

#[test]
fn the_owners_own_sieve_scripts_rules_change_only_into_what_sieve_says() {
    let (mut s, account, _) = sieve_session();
    let theirs = "require \"fileinto\";\r\nif header :contains \"subject\" \"x\" { fileinto \"INBOX\"; }\r\n";
    let state = json!({"extensions": ALL, "scripts": [{"name": "roundcube", "active": true}], "script": null,
        "activeScript": {"name": "roundcube", "text": theirs}});
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state}));
    let id = s.sieve.get(&account).and_then(|k| k.elsewhere.as_ref()).map(|e| e.rules[0].id).unwrap();
    let said = s
        .run("rules.edit", &json!({"id": id, "name": "N", "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "markRead"}]}))
        .unwrap_err()
        .to_string();
    // (The builder may change it, #2057; an attachment isn't something its Sieve can look for.)
    assert!(said.contains("can't be written in your Sieve script"), "{said}");
    let elsewhere = s.sieve.get(&account).and_then(|k| k.elsewhere.as_ref()).map(|e| e.rules[0].clone()).unwrap();
    assert_eq!(cannot_edit(&s.mailbox, &elsewhere), Some(NotEditable::Elsewhere));
}

#[test]
fn an_agent_asks_the_owner_with_the_rule_as_it_was_and_as_it_will_be() {
    let s = Session::at(sample::mailbox(NOW, 0), NOW, 0);
    let id = s.mailbox.rules[0].id;
    assert_eq!(access("rules.edit"), Some(Access::Owner(Class::Settings)));
    let params = json!({"id": id, "name": "Books", "conditions": [{"type": "subjectContains", "value": "order"}], "actions": [{"type": "markRead"}]});
    assert_eq!(classify(&s, "rules.edit", &params), Access::Owner(Class::Settings));
    let said = describe(&s, "rules.edit", &params);
    assert!(said.starts_with("change the rule “Bookshop newsletters”"), "{said}");
    assert!(said.contains("naming it “Books”, from when from contains \"harborbooks\""), "{said}");
    assert!(said.contains(" to when subject contains \"order\": mark it as read"), "{said}");
    assert_eq!(describe(&s, "rules.edit", &json!({"id": id})), "change a rule that can't be changed as asked");
}
