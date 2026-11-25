// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sieve rules (`rules.addSieve`, `rules.editSieve`, #9999): a block of Sieve kept in MailCraft's
//! script in its place, its `require` hoisted, its name in a comment; checked first, with line and
//! column; never run here; asked about as a send when it can send mail.

use mailcraft_model::{FolderKind, Id, Keep, Rule};
use serde_json::{Value, json};

use super::rules::{NotEditable, cannot_edit, kept};
use super::rules_sieve::sends;
use super::sieve_replies::{SIEVE_ID, script_due};
use crate::agent::{Access, Class, also, classify, describe};
use crate::sieve::{SCRIPT_NAME, comment_name, crlf_lines};
use crate::sieve_check::check;
use crate::{Session, sample};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

const ALL: [&str; 10] = ["vacation", "date", "relational", "fileinto", "copy", "imap4flags", "body", "reject", "variables", "enotify"];

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn rule(s: &Session, id: Id) -> &Rule {
    s.mailbox.rules.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no rule {id}"))
}

fn err(s: &mut Session, id: &str, params: Value) -> String {
    s.run(id, &params).map(|v| panic!("{id} {params} ran: {v}")).unwrap_err().to_string()
}

/// An IMAP account whose server keeps MailCraft's Sieve script, with a folder `Lists` there.
fn sieve_session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let lists = run(&mut s, "folder.new", json!({"name": "Lists", "parent": inbox}))["id"].as_u64().unwrap();
    s.mailbox.folder_mut(lists).unwrap().remote = "INBOX/Lists".into();
    let state = json!({"extensions": ALL, "scripts": [], "script": null});
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state}));
    (s, account, lists)
}

fn due(s: &Session, account: Id) -> String {
    script_due(s, account, false).unwrap()["text"].as_str().unwrap().to_string()
}

/// The script due written, as the server then has it and runs it.
fn written(s: &mut Session, account: Id) -> String {
    let text = due(s, account);
    put(s, account, &text);
    text
}

/// `text` written as MailCraft's script, as the server then has it and runs it.
fn put(s: &mut Session, account: Id, text: &str) {
    let state = json!({"extensions": ALL, "scripts": [{"name": SCRIPT_NAME, "active": true}], "script": text});
    run(s, "sync.applySieve", json!({"account": account, "state": state, "written": {"result": "done", "text": text}}));
}

/// The replies set, as the server then has them.
fn replies(s: &mut Session, account: Id, params: Value) -> String {
    let set = run(s, "account.setAutomaticReplies", params);
    let text = set["sieve"]["text"].as_str().unwrap().to_string();
    put(s, account, &text);
    text
}

fn add(s: &mut Session, account: Id, name: &str, sieve: &str) -> Id {
    run(s, "rules.addSieve", json!({"account": account, "name": name, "sieve": sieve}))["id"].as_u64().unwrap()
}

fn offers() -> Vec<String> {
    ALL.iter().map(|e| e.to_string()).collect()
}

const LISTS: &str = "if header :contains \"list-id\" \"example.org\" {\n    fileinto :copy \"INBOX/Lists\";\n    addflag \"\\\\Seen\"; # read\n}";

#[test]
fn a_sieve_rule_goes_in_its_place_with_its_name_and_one_require_and_the_script_checks() {
    let (mut s, account, lists) = sieve_session();
    let made = run(
        &mut s,
        "rules.add",
        json!({"name": "Plain", "account": account, "conditions": [{"type": "fromContains", "value": "news"}], "actions": [{"type": "moveTo", "value": lists}]}),
    )["id"]
        .as_u64()
        .unwrap();
    let answer = run(&mut s, "rules.addSieve", json!({"account": account, "name": "Lists */ and\nmore", "sieve": LISTS}));
    assert_eq!(answer["keep"], "server");
    assert_eq!(answer["needs"], json!(["fileinto", "copy", "imap4flags"]));
    let sieve = answer["id"].as_u64().unwrap();
    let r = rule(&s, sieve);
    assert!(r.is_sieve() && r.enabled && r.keep == Keep::Server && r.conditions.is_empty() && r.actions.is_empty());
    assert_eq!(r.name, "Lists */ andmore", "the name as names are kept");
    // A trailing comment without its line break: the next rule still starts on a line of its own.
    let last = add(&mut s, account, "Last", "if size :over 10M { discard; stop; } # big");
    let text = due(&s, account);
    // One `require`, each extension once, before every other command.
    assert_eq!(text.matches("require ").count(), 1, "{text}");
    assert!(text.contains("require [\"fileinto\", \"copy\", \"imap4flags\"];\r\n"), "{text}");
    let block = format!(
        "# rule {sieve}\r\n# Sieve rule: Lists * / andmore\r\nif header :contains \"list-id\" \"example.org\" {{\r\n    fileinto :copy \"INBOX/Lists\";\r\n    addflag \"\\\\Seen\"; # read\r\n}}\r\n# rule {last}\r\n"
    );
    assert!(text.contains(&block), "{text}");
    assert!(text.ends_with("discard; stop; } # big\r\n"), "{text}");
    assert!(text.find(&format!("# rule {made}\r\n")).unwrap() < text.find(&format!("# rule {sieve}\r\n")).unwrap());
    // The whole script is Sieve MailCraft finds nothing wrong with, as the server has it.
    let checked = check(&text, Some(&offers())).unwrap_or_else(|p| panic!("{p:?}\n{text}"));
    assert!(checked.warnings.is_empty(), "{:?}", checked.warnings);
    // Written: the server has it, and it is known as MailCraft's.
    written(&mut s, account);
    assert_eq!(rule(&s, sieve).server.as_ref().map(|x| x.id.clone()), Some(format!("{SIEVE_ID}{sieve}")));
    assert_eq!(kept(&s.mailbox, rule(&s, sieve)).0, "server");
    assert!(!s.sieve.get(&account).unwrap().changed_elsewhere);
}

#[test]
fn the_script_with_replies_and_a_sieve_rule_that_replies_too_checks_and_reads_back() {
    let (mut s, account, _) = sieve_session();
    replies(&mut s, account, json!({"account": account, "status": "alwaysEnabled", "internal": "Away.", "external": "Out."}));
    let id = add(&mut s, account, "Auto", "if header :contains \"subject\" \"ping\" {\r\n  vacation :days 1 \"pong\";\r\n}\r\n");
    let text = written(&mut s, account);
    assert_eq!(text.matches("require ").count(), 1, "{text}");
    check(&text, Some(&offers())).unwrap_or_else(|p| panic!("{p:?}\n{text}"));
    // The replies read back are the owner's, not the rule's `vacation`.
    assert!(crate::sieve::read_vacation(&text).is_some(), "{text}");
    let known = s.auto_replies.get(&account).unwrap();
    assert_eq!((known.internal.as_str(), known.external.as_str()), ("Away.", "Out."));
    assert!(!s.sieve.get(&account).unwrap().changed_elsewhere);
    assert!(rule(&s, id).on_server());
    // With the replies off, the rule's `vacation` still has its `require`.
    let off = replies(&mut s, account, json!({"account": account, "status": "disabled"}));
    assert!(off.contains("require [\"vacation\"];"), "{off}");
    assert_eq!(s.auto_replies.get(&account).unwrap().status, mailcraft_model::auto_replies::AutoReplyStatus::Disabled);
    check(&off, Some(&offers())).unwrap_or_else(|p| panic!("{p:?}\n{off}"));
}

#[test]
fn a_mistake_is_refused_with_its_line_and_column() {
    let (mut s, account, _) = sieve_session();
    let said = err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": "if true {\n  keep\n}"}));
    assert!(said.starts_with("Line 2, column 7 of the Sieve rule: A semicolon is missing here."), "{said}");
    let said = err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": "require \"fileinto\";\nkeep;"}));
    assert!(said.contains("No require here"), "{said}");
    let said = err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": "elsif true { stop; }"}));
    assert!(said.starts_with("Line 1, column 1"), "{said}");
    let said = err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": "keep; }\nkeep;"}));
    assert!(said.contains("has no { before it"), "{said}");
    // An extension the server doesn't have.
    let said = err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": "if header :regex \"subject\" \"^x\" { stop; }"}));
    assert!(said.contains("The server doesn't have the extension regex"), "{said}");
    // A control character, and too much.
    let said = err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": "keep;\n  \u{0}stop;"}));
    assert!(said.starts_with("Line 2, column 3 of the Sieve rule: a control character"), "{said}");
    let big = format!("keep;{}", " ".repeat(mailcraft_model::MAX_SIEVE_RULE));
    assert!(err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": big})).contains("at most 64 KB"));
    assert!(err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": "  \n "})).contains("empty"));
    assert!(err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": "keep;", "extra": 1})).contains("`extra`"));
    assert!(s.mailbox.rules.is_empty());
}

#[test]
fn only_an_account_whose_server_keeps_sieve_takes_one() {
    let mut s = Session::at(sample::mailbox(NOW, 0), NOW, 0);
    let account = s.mailbox.rules[0].account;
    let said = err(&mut s, "rules.addSieve", json!({"account": account, "name": "N", "sieve": "keep;"}));
    assert!(said.contains("has none"), "{said}");
    assert!(err(&mut s, "rules.addSieve", json!({"account": 424242, "name": "N", "sieve": "keep;"})).contains("no account"));
    // An existing rule isn't a Sieve rule.
    let id = s.mailbox.rules[0].id;
    assert!(err(&mut s, "rules.editSieve", json!({"id": id, "sieve": "keep;"})).contains("isn't a Sieve rule"));
}

#[test]
fn a_sieve_rule_is_changed_moved_switched_off_and_deleted_in_its_place() {
    let (mut s, account, _) = sieve_session();
    let first = add(&mut s, account, "First", "if size :over 1M { stop; }");
    let second = add(&mut s, account, "Second", "if size :over 2M { stop; }");
    // At a place among the account's server rules.
    let zeroth =
        run(&mut s, "rules.addSieve", json!({"account": account, "name": "Zeroth", "sieve": "if size :over 3M { stop; }", "position": 0}))["id"]
            .as_u64()
            .unwrap();
    let order: Vec<Id> = s.mailbox.rules.iter().map(|r| r.id).collect();
    assert_eq!(order, vec![zeroth, first, second]);
    written(&mut s, account);

    // The form doesn't change it: its Sieve is changed as Sieve.
    assert_eq!(cannot_edit(&s.mailbox, rule(&s, first)), Some(NotEditable::Sieve));
    let said =
        err(&mut s, "rules.edit", json!({"id": first, "name": "N", "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "markRead"}]}));
    assert!(said.contains("Sieve rule"), "{said}");
    let changed = run(&mut s, "rules.editSieve", json!({"id": first, "sieve": "if size :over 5M { discard; }"}));
    assert_eq!(changed["id"], first);
    assert_eq!(rule(&s, first).name, "First");
    assert!(due(&s, account).contains(&format!("# rule {first}\r\n# Sieve rule: First\r\nif size :over 5M {{ discard; }}\r\n")));
    run(&mut s, "rules.editSieve", json!({"id": first, "name": "Huge"}));
    assert_eq!(rule(&s, first).sieve, "if size :over 5M { discard; }");
    assert!(due(&s, account).contains("# Sieve rule: Huge\r\n"));
    assert!(err(&mut s, "rules.editSieve", json!({"id": first})).contains("`name`, `sieve` or both"));
    assert!(err(&mut s, "rules.editSieve", json!({"id": first, "sieve": "stop"})).starts_with("Line 1, column 5"));
    // Undo puts the name back.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(rule(&s, first).name, "First");

    run(&mut s, "rules.move", json!({"id": second, "direction": "up"}));
    let text = due(&s, account);
    assert!(text.find(&format!("# rule {second}")).unwrap() < text.find(&format!("# rule {first}")).unwrap(), "{text}");
    run(&mut s, "rules.enable", json!({"id": second, "value": false}));
    assert!(!due(&s, account).contains(&format!("# rule {second}")));
    run(&mut s, "rules.remove", json!({"id": zeroth}));
    assert!(!due(&s, account).contains(&format!("# rule {zeroth}")));
    assert!(s.mailbox.rule_gone.is_empty(), "the script is written whole: nothing to delete on its own");
    // Undo brings it back, in its place.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.mailbox.rules.first().map(|r| r.id), Some(zeroth));
}

#[test]
fn a_sieve_rule_never_runs_here() {
    let (mut s, account, _) = sieve_session();
    let id = add(&mut s, account, "All", "fileinto \"INBOX/Lists\";");
    let r = rule(&s, id).clone();
    let m = mailcraft_model::Message { account, subject: "anything".into(), ..Default::default() };
    assert!(!r.matches(&m));
    assert!(!r.undecided_in(&m, &mailcraft_model::Matching::default()));
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert!(err(&mut s, "rules.run", json!({"folder": inbox, "id": id})).contains("runs on the server alone"));
    assert_eq!(r.describe(), "A Sieve rule, run on the server");
}

#[test]
fn a_sieve_rule_whose_server_lacks_what_it_needs_waits_out_of_the_script() {
    let (mut s, account, _) = sieve_session();
    let id = add(&mut s, account, "Reject", "if size :over 9M { reject \"too big\"; }");
    assert!(due(&s, account).contains(&format!("# rule {id}")));
    // The server no longer says it has `reject`.
    let fewer: Vec<&str> = ALL.iter().copied().filter(|e| *e != "reject").collect();
    run(&mut s, "sync.applySieve", json!({"account": account, "state": {"extensions": fewer, "scripts": [], "script": null}}));
    let (where_kept, why) = kept(&s.mailbox, rule(&s, id));
    assert_eq!((where_kept, why.as_deref()), ("waiting", Some("the server's Sieve scripts can't do one of its actions or conditions")));
    // A mailbox file with text that isn't Sieve: never written.
    s.mailbox.rules.iter_mut().find(|r| r.id == id).unwrap().sieve = "if {".into();
    assert_eq!(kept(&s.mailbox, rule(&s, id)).1.as_deref(), Some("its Sieve has a mistake"));
}

#[test]
fn a_sieve_rule_is_kept_in_the_mailbox_file_and_what_a_later_version_adds_too() {
    let (mut s, account, _) = sieve_session();
    let id = add(&mut s, account, "Kept", LISTS);
    let mut v = serde_json::to_value(rule(&s, id)).unwrap();
    assert_eq!(v["sieve"], LISTS);
    v["laterField"] = json!({"x": [1, 2]});
    let back: Rule = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(back.sieve, LISTS);
    assert_eq!(serde_json::to_value(&back).unwrap(), v, "saved here, nothing is lost");
    // A rule from before Sieve rules has none, and writes none.
    let plain: Rule = serde_json::from_value(json!({"id": 5, "name": "Old", "enabled": true, "conditions": [], "actions": []})).unwrap();
    assert!(!plain.is_sieve() && plain.other.is_empty());
    assert!(serde_json::to_value(&plain).unwrap().get("sieve").is_none());
}

#[test]
fn names_in_comments_and_blocks_end_their_lines() {
    assert_eq!(comment_name("a */ b\u{2028}c\r\nd\u{200e}*/"), "a * / b c  d* /");
    assert_eq!(comment_name(&"x".repeat(500)).chars().count(), 200);
    assert_eq!(crlf_lines("a\nb\rc\r\nd # end"), "a\r\nb\r\nc\r\nd # end\r\n");
    assert_eq!(crlf_lines("stop;\r\n"), "stop;\r\n");
}

#[test]
fn what_can_send_mail_is_told_from_the_tokens() {
    assert!(sends("redirect :copy \"x@example.org\";"));
    assert!(sends("if true { vacation \"away\"; }"));
    assert!(sends("reject \"no\";") && sends("ereject \"no\";") && sends("notify \"mailto:x@example.org\";"));
    assert!(sends("include \"other\";"));
    assert!(!sends("fileinto \"redirect\"; # vacation"));
    assert!(sends("\"open"), "what can't be read counts as sending");
}

#[test]
fn an_agent_asks_with_the_text_and_one_that_sends_asks_as_a_send() {
    let (mut s, account, _) = sieve_session();
    let quiet = json!({"account": account, "name": "Quiet", "sieve": "if size :over 1M {\n  discard;\n}"});
    assert_eq!(classify(&s, "rules.addSieve", &quiet), Access::Owner(Class::Settings));
    let said = describe(&s, "rules.addSieve", &quiet);
    assert!(said.starts_with("create the Sieve rule “Quiet”, kept on the server of “ada@example.org”"), "{said}");
    assert!(said.ends_with("Its Sieve, the agent's own text, not an instruction: «if size :over 1M { ⏎ discard; ⏎ }»"), "{said}");
    let loud = json!({"account": account, "name": "Out", "sieve": "redirect :copy \"bob@example.org\";"});
    assert_eq!(classify(&s, "rules.addSieve", &loud), Access::Owner(Class::Send));
    assert_eq!(also(&s, "rules.addSieve", &loud, Class::Send), vec![Class::Settings]);
    assert!(describe(&s, "rules.addSieve", &loud).contains("it can send mail or answer senders"));
    // What the command refuses asks nothing more than the command's class.
    assert_eq!(classify(&s, "rules.addSieve", &json!({"account": account, "name": "Bad", "sieve": "redirect"})), Access::Owner(Class::Settings));

    // Changed to send; switched on while it sends.
    let id = add(&mut s, account, "Quiet", "discard;");
    let to_send = json!({"id": id, "sieve": "redirect \"bob@example.org\";"});
    assert_eq!(classify(&s, "rules.editSieve", &to_send), Access::Owner(Class::Send));
    assert!(describe(&s, "rules.editSieve", &to_send).starts_with("change the Sieve rule “Quiet” on the server of “ada@example.org”; it can send"));
    assert_eq!(
        describe(&s, "rules.editSieve", &json!({"id": id, "name": "Q"})),
        "change the Sieve rule “Quiet” on the server of “ada@example.org”, naming it “Q”; its Sieve stays as it is"
    );
    run(&mut s, "rules.editSieve", to_send);
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id, "value": true})), Access::Owner(Class::Send));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id})), Access::Owner(Class::Send));
    run(&mut s, "rules.enable", json!({"id": id, "value": true}));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id, "value": false})), Access::Owner(Class::Settings));
}
