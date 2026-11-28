// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules of a Sieve script MailCraft didn't write (#2057): read where they map, the rest
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

/// A reply sent from a block within a block is a reply all the same: the script sends replies
/// (#2058).
#[test]
fn a_reply_in_a_nested_block_is_seen() {
    let (s, account, _) = session();
    let nested = "require [\"vacation\"];\r\nif header :contains \"subject\" \"x\" {\r\n  if true {\r\n    vacation \"Away.\";\r\n  }\r\n}\r\n";
    assert!(read(&s.mailbox, account, "mine", nested, false).replies);
    assert!(!read(&s.mailbox, account, "mine", "if true { keep; }\r\n", false).replies);
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

/// A webmail of the RainLoop kind's script, of the shape it writes (synthetic).
fn rainloop() -> String {
    let spam = json!({"ID": "0000017000590660000", "Enabled": true, "Name": "Spam",
        "Conditions": [{"Field": "Header", "Type": "EqualTo", "Value": "YES", "ValueSecond": "X-Spam-Flag"},
            {"Field": "Header", "Type": "EqualTo", "Value": "Yes", "ValueSecond": "X-Spam"}],
        "ConditionsType": "Any", "ActionType": "MoveTo", "ActionValue": "Junk", "ActionValueSecond": "", "ActionValueThird": "",
        "ActionValueFourth": "", "Keep": true, "Stop": true, "MarkAsRead": false});
    let list = json!({"ID": "abc123", "Enabled": true, "Name": "ML-list",
        "Conditions": [{"Field": "Recipient", "Type": "Contains", "Value": "list@lists.example.org", "ValueSecond": ""}],
        "ConditionsType": "Any", "ActionType": "MoveTo", "ActionValue": "Mailing-Lists/list", "ActionValueSecond": "", "ActionValueThird": "",
        "ActionValueFourth": "", "Keep": true, "Stop": true, "MarkAsRead": false});
    format!(
        "require [\"fileinto\",\"body\"];\r\n\r\n# This is SnappyMail sieve script.\r\n# Please don't change anything here.\r\n# RAINLOOP:SIEVE\r\n\r\n\
{}\r\n\r\nif anyof(\r\n    header :is [\"X-Spam-Flag\"] \"YES\",\r\n    header :is [\"X-Spam\"] \"Yes\"\r\n)\r\n{{\r\n    fileinto \"Junk\";\r\n    stop;\r\n}}\r\n\r\n\
/* END:FILTER */\r\n\r\n{}\r\n\r\nif header :contains [\"To\", \"CC\"] \"list@lists.example.org\"\r\n{{\r\n    fileinto \"Mailing-Lists/list\";\r\n    stop;\r\n}}\r\n\r\n\
/* END:FILTER */\r\n",
        crate::sieve_rainloop_tests::header("0000017000590660000", &spam.to_string()),
        crate::sieve_rainloop_tests::header("abc123", &list.to_string()),
    )
}

#[test]
fn a_rainloop_script_shows_its_rules_by_name_with_nothing_unknown() {
    let (mut s, account, _) = session();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let junk = s.mailbox.folder_of_kind(account, FolderKind::Junk).unwrap();
    s.mailbox.folder_mut(junk).unwrap().remote = "Junk".into();
    let lists = s.run("folder.new", &json!({"name": "list", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.folder_mut(lists).unwrap().remote = "Mailing-Lists/list".into();
    let e = read(&s.mailbox, account, "snappymail", &rainloop(), false);
    assert!(!e.unreadable && !e.replies);
    let names: Vec<&str> = e.rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Spam", "ML-list"]);
    // (Each filter is one this version knows, whose Sieve is what its settings give: it can be
    // changed here, in the webmail's own shape, `rules_webmail`.)
    for r in &e.rules {
        assert!(r.server.as_ref().unwrap().unknown.is_empty() && r.known() && r.enabled && !r.read_only() && r.stop, "{r:?}");
    }
    assert!(e.blocks.iter().all(|b| b.settings.is_ok()));
    // `:is` stays exact; the two lines, whose words are the same but for case, are one condition.
    assert_eq!(
        e.rules[0].conditions,
        vec![Condition::Header { names: vec!["X-Spam-Flag".into(), "X-Spam".into()], words: vec!["YES".into()], exact: true }]
    );
    assert_eq!(e.rules[0].actions, vec![Action::MoveTo(junk)]);
    assert_eq!(e.rules[1].conditions, vec![Condition::ToContains(vec!["list@lists.example.org".into()])]);
    assert_eq!(e.rules[1].actions, vec![Action::MoveTo(lists)]);
    // The same through rules.list, in words.
    s.run("sync.applySieve", &json!({"account": account, "state": state("snappymail", &rainloop())})).unwrap();
    let listed = s.run("rules.list", &json!({"account": account})).unwrap();
    let text = listed["elsewhere"][0]["rules"][0]["text"].as_str().unwrap_or_default().to_string();
    assert!(text.contains("message header \"X-Spam-Flag\" or \"X-Spam\" is exactly \"YES\""), "{text}");
}

#[test]
fn header_tests_map_as_they_mean_and_no_wider() {
    let (s, account, _) = session();
    let conditions = |test: &str| {
        let e = read(&s.mailbox, account, "x", &format!("if {test} {{ stop; }}"), false);
        let r = &e.rules[0];
        (r.conditions.clone(), r.server.as_ref().unwrap().unknown.clone())
    };
    let header = |names: &[&str], words: &[&str], exact| Condition::Header {
        names: names.iter().map(|n| n.to_string()).collect(),
        words: words.iter().map(|n| n.to_string()).collect(),
        exact,
    };
    let none: Vec<String> = Vec::new();
    assert_eq!(conditions("header :contains \"From\" \"ann\""), (vec![Condition::FromContains(vec!["ann".into()])], none.clone()));
    assert_eq!(conditions("header :contains [\"cc\", \"TO\"] \"ann\""), (vec![Condition::ToContains(vec!["ann".into()])], none.clone()));
    // To alone is not To or Cc; `:is` on From is the whole line, not an address.
    assert_eq!(conditions("header :contains \"To\" \"ann\""), (vec![header(&["To"], &["ann"], false)], none.clone()));
    assert_eq!(conditions("header :is \"From\" \"ann@example.org\""), (vec![header(&["From"], &["ann@example.org"], true)], none.clone()));
    assert_eq!(conditions("header :is \"subject\" \"Hi\""), (vec![header(&["subject"], &["Hi"], true)], none.clone()));
    assert_eq!(conditions("header :contains [\"X-A\", \"x-a\"] [\"1\", \"2\"]"), (vec![header(&["X-A"], &["1", "2"], false)], none.clone()));
    // Several tests, all of which must hold: each is shown.
    assert_eq!(
        conditions("allof (header :is \"X-A\" \"1\", header :contains \"From\" \"ann\")"),
        (vec![header(&["X-A"], &["1"], true), Condition::FromContains(vec!["ann".into()])], none.clone())
    );
    // Any of several: one condition where one says it, else a part not known here.
    assert_eq!(
        conditions("anyof (header :contains \"X-A\" \"1\", header :contains \"x-a\" \"2\")"),
        (vec![header(&["X-A"], &["1", "2"], false)], none.clone())
    );
    assert_eq!(
        conditions("anyof (address :contains \"from\" \"ann\", address :contains \"from\" \"bob\")"),
        (vec![Condition::FromContains(vec!["ann".into(), "bob".into()])], none.clone())
    );
    for unknown in [
        "anyof (header :is \"X-A\" \"1\", header :is \"X-B\" \"2\")",
        "anyof (header :is \"X-A\" \"1\", header :contains \"X-A\" \"2\")",
        "anyof (header :contains \"From\" \"ann\", header :contains \"subject\" \"bob\")",
        "anyof (header :contains \"X-A\" \"1\", exists \"X-B\")",
    ] {
        assert_eq!(conditions(unknown), (Vec::new(), vec!["anyof".to_string()]), "{unknown}");
    }
    for unknown in
        ["header :matches \"X-A\" \"*\"", "header :is :comparator \"i;octet\" \"X-A\" \"1\"", "header :is \"X-A\" \"\"", "header :is \"\" \"1\""]
    {
        assert_eq!(conditions(unknown), (Vec::new(), vec!["header".to_string()]), "{unknown}");
    }
}

#[test]
fn rules_are_named_by_the_comment_before_them_and_switched_off_where_the_script_says() {
    let (s, account, _) = session();
    let script = "require \"fileinto\";\r\n# rule:[Lists]\r\nif header :contains \"subject\" \"a\" { stop; }\r\n\
# rule:[\u{202e}evil\u{0}\u{1b}]\r\n/* note */\r\nif header :contains \"subject\" \"b\" { stop; }\r\n\
if header :contains \"subject\" \"c\" { stop; }\r\n# rule:[Off]\r\nif false # anyof (header :contains \"subject\" \"d\")\r\n{ stop; }\r\n";
    let e = read(&s.mailbox, account, "roundcube", script, false);
    let names: Vec<&str> = e.rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Lists", "evil", "roundcube 3", "Off"]);
    assert!(e.rules[..3].iter().all(|r| r.enabled));
    assert!(!e.rules[3].enabled && e.rules[3].server.as_ref().unwrap().unknown == ["false"]);
    // The `# RAINLOOP:SIEVE` line not there: a filter block's name isn't read.
    let plain = rainloop().replace("# RAINLOOP:SIEVE", "# Something else");
    let e = read(&s.mailbox, account, "x", &plain, false);
    assert_eq!(e.rules.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["x 1", "x 2"]);
    // A block the webmail says is off, with no `if`: a rule switched off, in its place.
    let off = json!({"ID": "z", "Enabled": false, "Name": "Away"}).to_string();
    let script = format!(
        "# RAINLOOP:SIEVE\r\nif true {{ stop; }}\r\n{}\r\n/* END:FILTER */\r\nif header :contains \"subject\" \"x\" {{ stop; }}\r\n",
        crate::sieve_rainloop_tests::header("z", &off)
    );
    let e = read(&s.mailbox, account, "x", &script, false);
    let shown: Vec<(&str, bool)> = e.rules.iter().map(|r| (r.name.as_str(), r.enabled)).collect();
    assert_eq!(shown, [("x 1", true), ("Away", false), ("x 3", true)]);
    assert!(!e.rules[1].known() && e.rules[1].read_only());
    // One that says it is off but has its `if`: the server runs it, so it is on.
    let script = format!("# RAINLOOP:SIEVE\r\n{}\r\nif true {{ stop; }}\r\n/* END:FILTER */\r\n", crate::sieve_rainloop_tests::header("z", &off));
    let e = read(&s.mailbox, account, "x", &script, false);
    assert_eq!(e.rules.iter().map(|r| (r.name.as_str(), r.enabled)).collect::<Vec<_>>(), [("Away", true)]);
    // A comment in a comment: not Sieve.
    assert!(read(&s.mailbox, account, "x", "/* a /* b */ c */ if true { stop; }", false).unreadable);
}

proptest! {
    #[test]
    fn arbitrary_names_and_blocks_never_panic(name in ".{0,300}", bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        let (s, account, _) = session();
        let filter = json!({"Name": name, "Enabled": false}).to_string();
        let b64 = crate::cmd::base64_encode(&bytes);
        for script in [
            format!("# rule:[{name}]\nif true {{ stop; }}"),
            format!("# RAINLOOP:SIEVE\n{}\nif true {{ stop; }}\n/* END:FILTER */", crate::sieve_rainloop_tests::header("1", &filter)),
            format!("# RAINLOOP:SIEVE\n{}\n/* END:FILTER */", crate::sieve_rainloop_tests::header("1", &filter)),
            format!("# RAINLOOP:SIEVE\n/*\nBEGIN:FILTER:{name}\nBEGIN:HEADER\n{b64}\nEND:HEADER\n*/\nif true {{ stop; }}"),
        ] {
            let e = read(&s.mailbox, account, "x", &script, false);
            for r in &e.rules {
                prop_assert!(r.read_only() && !r.name.is_empty());
                prop_assert!(r.name.chars().count() <= super::sieve_elsewhere::MAX_NAME + 8);
                prop_assert!(!r.name.chars().any(|c| c.is_control() || mailcraft_names::is_format(c)));
            }
        }
    }
}
