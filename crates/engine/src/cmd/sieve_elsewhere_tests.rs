// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules of a Sieve script MailCraft didn't write (#9999): read where they map, the rest
//! named and never interpreted, read-only, and a hostile script never panics.

use mailcraft_model::{Action, Condition, FolderKind, Id, Keep, Rule};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::sieve_elsewhere::{ELSEWHERE_BASE, MAX_RULES, read};
use super::sieve_rules::block;
use crate::Session;
use crate::sieve::SCRIPT_NAME;

const ALL: [&str; 7] = ["vacation", "date", "relational", "fileinto", "copy", "imap4flags", "body"];

fn session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let account =
        s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.account_mut(account).unwrap().sieve_offers = ALL.map(String::from).to_vec();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let lists = s.run("folder.new", &json!({"name": "Lists", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.folder_mut(lists).unwrap().remote = "INBOX/Lists".into();
    (s, account, lists)
}

const WEBMAIL: &str = "# rule:[Lists]\r\nrequire [\"fileinto\", \"imap4flags\", \"vacation\", \"regex\"];\r\n\
if allof (address :contains \"from\" \"lists.example.org\", not header :contains \"subject\" \"urgent\") {\r\n  addflag \"\\\\Seen\";\r\n  fileinto \"INBOX/Lists\";\r\n  stop;\r\n}\r\n\
if header :regex \"subject\" \"^\\\\[SPAM\\\\]\" {\r\n  discard;\r\n}\r\n\
elsif address :is [\"to\", \"cc\"] \"ada@example.org\" {\r\n  redirect \"elsewhere@example.net\";\r\n}\r\n\
if size :over 2M { fileinto :copy \"Big\"; }\r\n\
if true { vacation :days 3 \"I am away.\"; }\r\n";

#[test]
fn a_webmail_script_is_read_where_it_maps_and_named_where_it_doesnt() {
    let (s, account, lists) = session();
    let e = read(&s.mailbox, account, "roundcube", WEBMAIL, false);
    assert!(!e.unreadable && e.replies && !e.ours && e.script == "roundcube");
    assert_eq!(e.rules.len(), 4, "{:?}", e.rules);
    let unknown = |r: &Rule| r.server.as_ref().unwrap().unknown.clone();
    let first = &e.rules[0];
    assert_eq!(first.conditions, vec![Condition::FromContains(vec!["lists.example.org".into()])]);
    assert_eq!(first.exceptions, vec![Condition::SubjectContains(vec!["urgent".into()])]);
    assert_eq!(first.actions, vec![Action::MarkRead, Action::MoveTo(lists)]);
    assert!(first.stop && unknown(first).is_empty());
    // Only the command's or test's name of what isn't known here; nothing of its strings.
    assert_eq!(unknown(&e.rules[1]), vec!["header", "discard"]);
    assert_eq!(unknown(&e.rules[2]), vec!["elsif", "redirect"]);
    assert_eq!(e.rules[2].conditions, vec![Condition::ToOrCcMe]);
    assert_eq!(unknown(&e.rules[3]), vec!["fileinto"]);
    for r in &e.rules {
        let said = format!("{:?}", r.server);
        assert!(!said.contains("elsewhere@example.net") && !said.contains("SPAM"), "{said}");
        assert!(r.read_only() && !r.known() || r.read_only(), "{r:?}");
        assert!(r.id >= ELSEWHERE_BASE);
        assert_eq!(r.keep, Keep::Server);
    }
}

#[test]
fn mailcrafts_own_blocks_read_back_as_the_rules_they_were() {
    let (s, account, lists) = session();
    let rule = Rule {
        id: 9,
        enabled: true,
        account,
        keep: Keep::Server,
        conditions: vec![
            Condition::FromContains(vec!["news".into()]),
            Condition::SentTo(vec!["team@example.org".into()]),
            Condition::BodyContains(vec!["sale".into()]),
            Condition::SubjectOrBodyContains(vec!["offer".into()]),
            Condition::ToMe,
        ],
        exceptions: vec![Condition::SubjectContains(vec!["urgent".into()])],
        actions: vec![Action::MarkRead, Action::Flag, Action::CopyTo(lists), Action::MoveTo(lists)],
        stop: true,
        ..Default::default()
    };
    let text = format!("require [\"fileinto\", \"copy\", \"imap4flags\", \"body\"];\r\n{}", block(&s.mailbox, &rule, account).unwrap().text);
    let e = read(&s.mailbox, account, SCRIPT_NAME, &text, true);
    let back = &e.rules[0];
    assert_eq!(back.conditions, rule.conditions);
    assert_eq!(back.exceptions, rule.exceptions);
    assert_eq!(back.actions, rule.actions);
    assert!(back.stop && e.ours);
    assert!(back.server.as_ref().unwrap().unknown.is_empty());
}

#[test]
fn a_script_that_isnt_sieve_or_is_too_deep_or_too_long_is_said_unreadable() {
    let (s, account, _) = session();
    let deep = format!("if {}true{} {{ stop; }}", "not ".repeat(40), "");
    let nested = format!("{}stop;{}", "if true {".repeat(40), "}".repeat(40));
    for text in ["if (", "{", "if true { stop;", "stop", "\"x\";", nested.as_str(), deep.as_str()] {
        let e = read(&s.mailbox, account, "x", text, false);
        assert!(e.unreadable || e.rules.iter().all(|r| r.read_only()), "{text:?}");
    }
    assert!(read(&s.mailbox, account, "x", &nested, false).unreadable);
    // Many rules: only so many are taken.
    let many = "if true { stop; }\r\n".repeat(MAX_RULES + 50);
    assert_eq!(read(&s.mailbox, account, "x", &many, false).rules.len(), MAX_RULES);
}

proptest! {
    #[test]
    fn arbitrary_scripts_never_panic(text in ".{0,300}", bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        let (s, account, _) = session();
        let _ = read(&s.mailbox, account, "x", &text, false);
        let _ = read(&s.mailbox, account, "x", &String::from_utf8_lossy(&bytes), false);
        let shaped = format!("if allof ({text}) {{ fileinto \"{text}\"; }}");
        let _ = read(&s.mailbox, account, "x", &shaped, false);
    }
}

fn state(active: &str, text: &str) -> Value {
    json!({"extensions": ALL, "scripts": [{"name": active, "active": true}], "script": null, "activeScript": {"name": active, "text": text}})
}

#[test]
fn the_owners_rules_are_listed_and_no_command_changes_or_runs_one() {
    let (mut s, account, _) = session();
    s.run("sync.applySieve", &json!({"account": account, "state": state("roundcube", WEBMAIL)})).unwrap();
    let listed = s.run("rules.list", &json!({"account": account})).unwrap();
    let elsewhere = &listed["elsewhere"][0];
    assert_eq!((elsewhere["script"].as_str(), elsewhere["replies"].as_bool()), (Some("roundcube"), Some(true)));
    assert_eq!(elsewhere["rules"].as_array().unwrap().len(), 4);
    assert_eq!(elsewhere["rules"][0]["readOnly"], true);
    let id = elsewhere["rules"][0]["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    for (command, params) in [
        ("rules.enable", json!({"id": id, "value": false})),
        ("rules.remove", json!({"id": id})),
        ("rules.move", json!({"id": id, "direction": "down"})),
        ("rules.keepOnServer", json!({"id": id})),
        ("rules.run", json!({"id": id, "folder": inbox})),
    ] {
        let said = s.run(command, &params).unwrap_err().to_string();
        assert!(said.contains("\"roundcube\"") && said.contains("never changes it"), "{command}: {said}");
    }
    // None of them is in the mailbox, nor runs here.
    assert!(s.mailbox.rules.iter().all(|r| r.id < ELSEWHERE_BASE));
    // MailCraft's own, changed elsewhere, is shown the same way.
    let (mut s, account, _) = session();
    s.mailbox.account_mut(account).unwrap().sieve_written = crate::sieve::digest("keep;");
    let theirs = "require \"fileinto\";\r\nif address :contains \"from\" \"x\" { fileinto \"INBOX/Lists\"; }\r\n";
    s.run(
        "sync.applySieve",
        &json!({"account": account, "state": {"extensions": ALL, "scripts": [{"name": SCRIPT_NAME, "active": true}], "script": theirs}}),
    )
    .unwrap();
    let listed = s.run("rules.list", &json!({"account": account})).unwrap();
    assert_eq!(listed["elsewhere"][0]["mailcraftChanged"], true);
    assert_eq!(listed["elsewhere"][0]["rules"][0]["text"].as_str().map(|t| t.contains("sender's address contains")), Some(true));
}
