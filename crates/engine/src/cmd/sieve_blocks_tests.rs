// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every rule of a Sieve account changed one at a time (#2057): a rule of the owner's own script as
//! its Sieve text (`sieve.ownBlock`, `sieve.putOwnBlock`), its bytes alone replaced, under the
//! digest of the read; in the rule builder only when that loses nothing (`rules.edit`); a filter of
//! a webmail with settings not known as its Sieve alone, its settings left as they are; and one of
//! MailCraft's own rules made a Sieve rule (`rules.toSieve`), with undo. The scripts are synthetic.

use mailcraft_model::{Condition, FolderKind, Id, Keep};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::sieve_blocks::{block_problems, find, for_server_check, form, text_offered};
use super::sieve_elsewhere::{NoForm, same_meaning};
use super::sieve_own::OWN_CHANGED;
use crate::Session;
use crate::agent::{Access, Class, classify, describe};
use crate::cmd::base64_encode;
use crate::sieve::digest;
use crate::sieve_check::check;

const ALL: [&str; 5] = ["fileinto", "imap4flags", "vacation", "body", "copy"];

/// The owner's script: a rule named in a comment, one with a header test on From (which the builder
/// would write as an address test), one with branches, and one the builder writes as it is.
const OWN: &str = "require [\"fileinto\"];\r\n\r\n# rule:[Lists]\r\nif header :contains \"subject\" \"[list]\" {\r\n  fileinto \"INBOX/Lists\";\r\n}\r\n\r\n# from the boss\r\nif header :contains \"from\" \"boss@example.org\" {\r\n  fileinto \"INBOX/Lists\";\r\n}\r\nif address :is \"from\" \"a@example.org\" {\r\n  stop;\r\n} else {\r\n  keep;\r\n}\r\n# rule:[Spam]\r\nif header :is \"X-Spam-Flag\" \"YES\" {\r\n  fileinto \"INBOX/Lists\";\r\n  stop;\r\n}\r\n";

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn err(s: &mut Session, id: &str, params: Value) -> String {
    s.run(id, &params).map(|v| panic!("{id} {params} ran: {v}")).unwrap_err().to_string()
}

/// An IMAP account with the folder INBOX/Lists, whose server runs the owner's script `text`.
fn session(text: &str) -> (Session, Id, Id) {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let lists = run(&mut s, "folder.new", json!({"name": "Lists", "parent": inbox}))["id"].as_u64().unwrap();
    s.mailbox.folder_mut(lists).unwrap().remote = "INBOX/Lists".into();
    read(&mut s, account, text);
    (s, account, lists)
}

fn read(s: &mut Session, account: Id, text: &str) {
    let state =
        json!({"extensions": ALL, "scripts": [{"name": "mine", "active": true}], "script": null, "activeScript": {"name": "mine", "text": text}});
    run(s, "sync.applySieve", json!({"account": account, "state": state}));
}

fn rules(s: &Session, account: Id) -> Vec<mailcraft_model::Rule> {
    s.sieve[&account].elsewhere.as_ref().unwrap().rules.clone()
}

/// The rule named `name`.
fn named(s: &Session, account: Id, name: &str) -> Id {
    rules(s, account).iter().find(|r| r.name == name).unwrap_or_else(|| panic!("no rule {name}")).id
}

/// What the host is told to write: the owner's script, under the digest of the read, whole and
/// checking.
fn written(answer: &Value, before: &str) -> String {
    let sieve = &answer["sieve"];
    assert_eq!((sieve["name"].as_str(), sieve["own"].as_bool(), sieve["activate"].as_bool()), (Some("mine"), Some(true), Some(false)));
    assert_eq!(sieve["lastWritten"], digest(before));
    let text = sieve["text"].as_str().unwrap().to_string();
    assert!(check(&text, None).is_ok(), "{text}");
    text
}

#[test]
fn each_rule_is_its_comment_lines_and_its_chain() {
    let (mut s, account, _) = session(OWN);
    let shown = rules(&s, account);
    // Lists, the boss's, the if of the chain and its else, Spam.
    assert_eq!(shown.len(), 5, "{shown:?}");
    let lists = named(&s, account, "Lists");
    let lists = run(&mut s, "sieve.ownBlock", json!({"id": lists}));
    assert_eq!(lists["text"], "# rule:[Lists]\r\nif header :contains \"subject\" \"[list]\" {\r\n  fileinto \"INBOX/Lists\";\r\n}");
    assert_eq!(lists["digest"], digest(OWN));
    assert_eq!(lists["script"], "mine");
    let boss = run(&mut s, "sieve.ownBlock", json!({"id": shown[1].id}));
    assert_eq!(boss["text"], "# from the boss\r\nif header :contains \"from\" \"boss@example.org\" {\r\n  fileinto \"INBOX/Lists\";\r\n}");
    // The `if` and its `else` are one text: the whole chain.
    let chain = "if address :is \"from\" \"a@example.org\" {\r\n  stop;\r\n} else {\r\n  keep;\r\n}";
    assert_eq!(run(&mut s, "sieve.ownBlock", json!({"id": shown[2].id}))["text"], chain);
    assert_eq!(run(&mut s, "sieve.ownBlock", json!({"id": shown[3].id}))["text"], chain);
    assert!(text_offered(&s, shown[2].id));
    // Not a rule of the owner's script: refused.
    assert!(s.run("sieve.ownBlock", &json!({"id": 3})).is_err());
}

#[test]
fn the_form_only_when_it_loses_nothing() {
    let (mut s, account, lists) = session(OWN);
    let shown = rules(&s, account);
    // Its Sieve is what the builder writes for it.
    assert_eq!(form(&s, named(&s, account, "Lists")), Some(Ok(())));
    assert_eq!(form(&s, named(&s, account, "Spam")), Some(Ok(())));
    // A header test on From is shown as "from" here, which the builder writes as an address test.
    assert_eq!(shown[1].conditions, vec![Condition::FromContains(vec!["boss@example.org".into()])]);
    assert_eq!(form(&s, shown[1].id), Some(Err(NoForm::Differs)));
    // Branches.
    assert_eq!(form(&s, shown[2].id), Some(Err(NoForm::Branches)));
    assert_eq!(form(&s, shown[3].id), Some(Err(NoForm::NotIf)));
    // And `rules.edit` says why.
    let edit = json!({"id": shown[1].id, "name": "x", "conditions": [{"type": "fromContains", "value": "boss@example.org"}], "actions": [{"type": "moveTo", "value": lists}]});
    let refused = err(&mut s, "rules.edit", edit);
    assert!(refused.contains("address test"), "{refused}");
}

#[test]
fn same_meaning_reads_defaults_and_nothing_more() {
    // Defaults written out or left out, one string or a list of one, header names in any case.
    assert!(same_meaning(
        "if header \"Subject\" \"x\" { stop; }",
        "if header :is :comparator \"i;ascii-casemap\" [\"subject\"] [\"x\"] {\n stop;\n}"
    ));
    assert!(same_meaning("if address :contains \"from\" \"a\" { stop; }", "if address :all :contains \"from\" \"a\" { stop; }"));
    assert!(same_meaning("if allof (header :contains \"subject\" \"a\") { stop; }", "if header :contains \"subject\" \"a\" { stop; }"));
    // A header test isn't an address test, a word isn't another, an order of actions isn't another.
    assert!(!same_meaning("if header :contains \"from\" \"a\" { stop; }", "if address :all :contains \"from\" \"a\" { stop; }"));
    assert!(!same_meaning("if header :contains \"subject\" \"a\" { stop; }", "if header :contains \"subject\" \"A\" { stop; }"));
    assert!(!same_meaning("if true { fileinto \"x\"; addflag \"\\\\Seen\"; }", "if true { addflag \"\\\\Seen\"; fileinto \"x\"; }"));
    assert!(!same_meaning("if header :comparator \"i;octet\" \"subject\" \"a\" { stop; }", "if header \"subject\" \"a\" { stop; }"));
    // Nothing cut: a long string is compared whole.
    let long = "y".repeat(5_000);
    assert!(!same_meaning(
        &format!("if header \"subject\" \"{long}\" {{ stop; }}"),
        &format!("if header \"subject\" \"{}\" {{ stop; }}", &long[..1024])
    ));
    assert!(!same_meaning("not sieve {", "not sieve {"));
}

#[test]
fn unchanged_text_writes_the_same_bytes_and_one_rule_changes_its_bytes_alone() {
    let (mut s, account, _) = session(OWN);
    let boss = rules(&s, account)[1].id;
    let got = run(&mut s, "sieve.ownBlock", json!({"id": boss}));
    let text = got["text"].as_str().unwrap().to_string();
    let same = run(&mut s, "sieve.putOwnBlock", json!({"id": boss, "text": text, "digest": digest(OWN)}));
    assert_eq!(written(&same, OWN), OWN);
    assert_eq!(same["id"], boss);
    assert_eq!(same["sends"], false);

    let changed = text.replace("boss@example.org", "chief@example.org");
    let after = written(&run(&mut s, "sieve.putOwnBlock", json!({"id": boss, "text": changed, "digest": digest(OWN)})), OWN);
    let at = OWN.find(&text).unwrap();
    assert_eq!(&after[..at], &OWN[..at]);
    assert_eq!(&after[at + changed.len()..], &OWN[at + text.len()..]);
    assert_eq!(&after[at..at + changed.len()], changed);

    // An extension the new text needs joins the script's require, in the shape of its list.
    let flagged = "if header :contains \"subject\" \"x\" {\r\n  addflag \"\\\\Flagged\";\r\n}";
    let after = written(&run(&mut s, "sieve.putOwnBlock", json!({"id": boss, "text": flagged, "digest": digest(OWN)})), OWN);
    assert!(after.starts_with("require [\"fileinto\",\"imap4flags\"];\r\n"), "{after}");
    assert!(after.contains(flagged));
}

#[test]
fn a_rule_that_reaches_out_of_its_place_is_refused() {
    let (mut s, account, _) = session(OWN);
    let boss = rules(&s, account)[1].id;
    let put = |text: &str| json!({"id": boss, "text": text, "digest": digest(OWN)});
    for text in [
        "if true {",
        "}",
        "elsif true { stop; }",
        "stop; /* open",
        "require \"body\";\r\nstop;",
        "# only a comment",
        "",
        "stop;\u{0}",
        "if true { stop; } }\r\nif true {",
    ] {
        assert!(s.run("sieve.putOwnBlock", &put(text)).is_err(), "{text:?}");
    }
    // A mistake that is a mistake of the whole script is said with the rule's line and column.
    let problems = block_problems(&s, boss, "if header :contains \"subject\" \"x\" {\r\n  fileinto;\r\n}");
    assert_eq!(problems.inside.first().map(|p| p.line), Some(2), "{problems:?}");
    assert!(problems.outside.is_empty());
    // A comment ending the text takes in nothing after it.
    let after = written(&run(&mut s, "sieve.putOwnBlock", put("stop; # the end")), OWN);
    assert!(after.contains("stop; # the end\r\n"));
}

#[test]
fn a_script_changed_on_the_server_since_the_read_is_not_written() {
    let (mut s, account, _) = session(OWN);
    let boss = rules(&s, account)[1].id;
    let refused = err(&mut s, "sieve.putOwnBlock", json!({"id": boss, "text": "stop;", "digest": digest("keep;")}));
    assert_eq!(refused, OWN_CHANGED);
    // Read again: the rules are those of the new text, and the old digest is refused.
    let theirs = OWN.replace("boss@example.org", "other@example.org");
    read(&mut s, account, &theirs);
    let refused = err(&mut s, "sieve.putOwnBlock", json!({"id": boss, "text": "stop;", "digest": digest(OWN)}));
    assert_eq!(refused, OWN_CHANGED);
    let edit = json!({"id": named(&s, account, "Spam"), "name": "Spam", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [], "digest": digest(OWN)});
    assert_eq!(err(&mut s, "rules.edit", edit), OWN_CHANGED);
}

#[test]
fn the_builder_writes_its_rule_alone_in_the_scripts_style() {
    let (mut s, account, lists) = session(OWN);
    let spam = named(&s, account, "Spam");
    let header = json!({"type": "header", "value": {"names": ["X-Spam-Flag"], "words": ["YES", "Maybe"], "exact": true}});
    let edit = json!({"id": spam, "name": "Spam", "conditions": [header], "actions": [{"type": "moveTo", "value": lists}], "stop": true, "digest": digest(OWN)});
    let after = written(&run(&mut s, "rules.edit", edit), OWN);
    let old = "if header :is \"X-Spam-Flag\" \"YES\" {\r\n  fileinto \"INBOX/Lists\";\r\n  stop;\r\n}";
    let new = "if header :is [\"X-Spam-Flag\"] [\"YES\", \"Maybe\"] {\r\n  fileinto \"INBOX/Lists\";\r\n  stop;\r\n}";
    assert_eq!(after, OWN.replace(old, new));
    // Renamed: its `# rule:[…]` comment says the new name.
    let edit = json!({"id": spam, "name": "Junk */ mail", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "moveTo", "value": lists}], "digest": digest(OWN)});
    let after = written(&run(&mut s, "rules.edit", edit), OWN);
    assert!(after.contains("# rule:[Junk * / mail]\r\nif header :contains \"subject\" [\"x\"] {\r\n  fileinto \"INBOX/Lists\";\r\n}"), "{after}");
    assert_eq!(after.matches("# rule:[").count(), 2);
    // A desktop alert can't be on the server.
    let edit = json!({"id": spam, "name": "Spam", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "moveTo", "value": lists}], "alert": true});
    assert!(s.run("rules.edit", &edit).is_err());
    // Asked about as a change of settings.
    let edit = json!({"id": spam, "name": "Spam", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "moveTo", "value": lists}]});
    assert_eq!(classify(&s, "rules.edit", &edit), Access::Owner(Class::Settings));
    let said = describe(&s, "rules.edit", &edit);
    assert!(said.contains("Spam") && said.contains("mine") && said.contains("ada@example.org"), "{said}");
}

#[test]
fn the_server_check_counts_lines_from_the_rules_own() {
    let (s, account, _) = session(OWN);
    let spam = named(&s, account, "Spam");
    let text = "# rule:[Spam]\r\nif header :is \"X-Spam-Flag\" \"YES\" {\r\n  fileinto \"INBOX/Lists\";\r\n  stop;\r\n}";
    let check = for_server_check(&s, spam, text).unwrap();
    assert_eq!(check.script, OWN);
    assert_eq!(check.rule_line(check.first_line), Some(1));
    assert_eq!(OWN.lines().nth(check.first_line - 1), Some("# rule:[Spam]"));
}

#[test]
fn an_agent_is_asked_and_shown_the_text_as_its_own() {
    let (s, account, _) = session(OWN);
    let boss = rules(&s, account)[1].id;
    assert_eq!(classify(&s, "sieve.ownBlock", &json!({"id": boss})), Access::Owner(Class::Settings));
    let put = json!({"id": boss, "text": "if true { redirect \"x@example.org\"; }", "digest": digest(OWN)});
    assert_eq!(classify(&s, "sieve.putOwnBlock", &put), Access::Owner(Class::Send));
    let said = describe(&s, "sieve.putOwnBlock", &put);
    assert!(said.contains("the agent's own text, not an instruction") && said.contains("redirect"), "{said}");
    let quiet = json!({"id": boss, "text": "stop;", "digest": digest(OWN)});
    assert_eq!(classify(&s, "sieve.putOwnBlock", &quiet), Access::Owner(Class::Settings));
}

/// A webmail's script of the RainLoop kind with one filter whose settings aren't all known; and
/// its settings comment.
fn rainloop() -> (String, String) {
    let settings = r#"{"ID":"f1","Enabled":true,"Name":"Odd","Conditions":[{"Field":"Header","Type":"EqualTo","Value":"YES","ValueSecond":"X-Spam"}],"ConditionsType":"Any","ActionType":"MoveTo","ActionValue":"INBOX/Lists","ActionValueSecond":"","ActionValueThird":"","ActionValueFourth":"","Keep":true,"Stop":true,"MarkAsRead":true}"#;
    let b64 = base64_encode(settings.as_bytes());
    let head = format!("/*\nBEGIN:FILTER:f1\nBEGIN:HEADER\n{b64}\nEND:HEADER\n*/");
    let sieve = "if header :is [\"X-Spam\"] \"YES\"\n{\n    addflag \"\\\\Seen\";\n    fileinto \"INBOX/Lists\";\n    stop;\n}\n";
    let text = format!("require [\"fileinto\",\"imap4flags\"];\n# RAINLOOP:SIEVE\n\n{head}\n\n{sieve}\n/* END:FILTER */\n");
    (text, head)
}

#[test]
fn a_webmail_filter_not_known_is_changed_as_its_sieve_alone() {
    let (text, head) = rainloop();
    let (mut s, account, _) = session(&text);
    let odd = named(&s, account, "Odd");
    // The builder can't change it; its Sieve alone is its text.
    assert!(super::rules_webmail::read_only(&s, odd).is_some());
    assert_eq!(form(&s, odd), Some(Err(NoForm::Filter)));
    let got = run(&mut s, "sieve.ownBlock", json!({"id": odd}));
    assert_eq!(got["filter"], true);
    let sieve = got["text"].as_str().unwrap().to_string();
    assert!(sieve.starts_with("if header") && sieve.ends_with("}\n"), "{sieve:?}");
    let changed = sieve.replace("\"YES\"", "\"Yes\"");
    let after = written(&run(&mut s, "sieve.putOwnBlock", json!({"id": odd, "text": changed, "digest": digest(&text)})), &text);
    // Its settings stay as they were, byte for byte; the Sieve changes, nothing else.
    assert!(after.contains(&head));
    assert_eq!(after, text.replace(&sieve, &changed));
    // The webmail's marks can't be put in it.
    let marks = json!({"id": odd, "text": "stop;\n/* END:FILTER */\n", "digest": digest(&text)});
    assert!(s.run("sieve.putOwnBlock", &marks).is_err());
    assert!(find(&s, odd).unwrap().span.filter);
}

/// A filter without its END:FILTER runs on to the end of the script: what follows it (a `keep;` of
/// the owner's) would be taken in with it. It is shown, never changed as Sieve (#2058).
#[test]
fn a_webmail_filter_without_its_end_is_shown_and_not_changed() {
    let (text, _) = rainloop();
    let text = text.replace("/* END:FILTER */\n", "keep;\n");
    let (mut s, account, _) = session(&text);
    let odd = named(&s, account, "Odd");
    assert!(!find(&s, odd).unwrap().span.whole);
    assert!(!text_offered(&s, odd));
    let e = err(&mut s, "sieve.putOwnBlock", json!({"id": odd, "text": "stop;\n", "digest": digest(&text)}));
    assert!(e.contains("isn't clear"), "{e}");
}

/// A chain whose `if` is in a filter and whose `else` follows its END:FILTER: writing either again
/// would take the filter's mark out. Neither is changed (#2058).
#[test]
fn a_chain_across_a_filters_end_is_not_changed() {
    let (text, _) = rainloop();
    let text = text.replace("    stop;\n}\n/* END:FILTER */\n", "    stop;\n}\n/* END:FILTER */\nelse {\n    keep;\n}\n");
    let (mut s, account, _) = session(&text);
    let shown = rules(&s, account);
    assert!(shown.len() >= 2, "{shown:?}");
    for r in &shown {
        if let Some(f) = find(&s, r.id) {
            assert!(!f.span.whole, "{:?}", r.name);
            assert!(!text_offered(&s, r.id));
            assert!(form(&s, r.id).is_some_and(|f| f.is_err()));
            assert!(s.run("sieve.putOwnBlock", &json!({"id": r.id, "text": "keep;\n", "digest": digest(&text)})).is_err());
        }
    }
}

#[test]
fn one_of_mailcrafts_rules_becomes_a_sieve_rule_and_undo_brings_it_back() {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    let state = json!({"extensions": ALL, "scripts": [], "script": null});
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state}));
    // Words of which one has a semicolon: the builder can't show them whole.
    let made = run(
        &mut s,
        "rules.add",
        json!({"account": account, "name": "Odd", "conditions": [{"type": "subjectContains", "value": ["a;b", "c"]}], "actions": [{"type": "flag"}], "server": true}),
    );
    let id = made["id"].as_u64().unwrap();
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    let before = s.mailbox.rules.clone();
    let got = run(&mut s, "rules.toSieve", json!({"id": id}));
    assert_eq!(got["needs"], json!(["imap4flags"]));
    let r = s.mailbox.rules.iter().find(|r| r.id == id).unwrap();
    assert_eq!(r.sieve, "if header :contains \"subject\" [\"a;b\", \"c\"] {\n    addflag \"\\\\Flagged\";\n}\n");
    assert!(r.is_sieve() && r.conditions.is_empty() && r.actions.is_empty());
    assert_eq!((r.name.as_str(), r.enabled, r.keep), ("Odd", false, Keep::Server));
    assert_eq!(s.mailbox.rules.iter().position(|r| r.id == id), before.iter().position(|r| r.id == id));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.mailbox.rules, before);
    // Its text as given, checked; the agent's text that can send is asked about as a send.
    run(&mut s, "rules.enable", json!({"id": id, "value": true}));
    let text = json!({"id": id, "sieve": "redirect \"x@example.org\";"});
    assert_eq!(classify(&s, "rules.toSieve", &text), Access::Owner(Class::Send));
    assert!(s.run("rules.toSieve", &json!({"id": id, "sieve": "if {"})).is_err());
    // Not a rule kept on the server: refused.
    let local = run(
        &mut s,
        "rules.add",
        json!({"account": account, "name": "Here", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "flag"}], "server": false}),
    );
    assert!(s.run("rules.toSieve", &json!({"id": local["id"]})).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the text, a change of one rule never panics, and what is written keeps every byte
    /// after the rule's.
    #[test]
    fn any_text_for_a_rule_never_panics(text in "\\PC{0,200}") {
        let (mut s, account, _) = session(OWN);
        let boss = rules(&s, account)[1].id;
        let old = run(&mut s, "sieve.ownBlock", json!({"id": boss}))["text"].as_str().unwrap().to_string();
        let _ = block_problems(&s, boss, &text);
        let _ = for_server_check(&s, boss, &text);
        if let Ok(v) = s.run("sieve.putOwnBlock", &json!({"id": boss, "text": text, "digest": digest(OWN)})) {
            let after = v["sieve"]["text"].as_str().unwrap().to_string();
            let at = OWN.find(&old).unwrap();
            prop_assert!(after.ends_with(&OWN[at + old.len()..]));
        }
    }
}
