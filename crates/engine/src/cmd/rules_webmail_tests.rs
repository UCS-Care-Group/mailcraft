// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The filters of a webmail's Sieve script of the RainLoop kind, changed with the rule commands
//! (#9999): a round trip writes the script back byte for byte, a change touches the bytes of its
//! filter alone, settings and Sieve together in the webmail's shape; add, delete, move and switch
//! on and off; a filter changed by hand, or with settings not known, is read-only; hostile
//! settings never panic. The script is synthetic, in the shape such a webmail was seen to write.

use mailcraft_model::{Action, Condition, FolderKind, Id};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::rules_webmail::{NotWebmail, find, read_only};
use super::sieve_own::OWN_CHANGED;
use crate::Session;
use crate::agent::{Access, Class, classify, describe};
use crate::cmd::base64_encode;
use crate::sieve::digest;
use crate::sieve_check::check;
use crate::sieve_rainloop::{ReadOnly, Settings, with_require};

const ALL: [&str; 4] = ["fileinto", "body", "imap4flags", "vacation"];

/// The settings of the first filter, as the webmail wrote them.
const SPAM: &str = r#"{"ID":"0000017000590660000","Enabled":true,"Name":"Spam","Conditions":[{"Field":"Header","Type":"EqualTo","Value":"YES","ValueSecond":"X-Spam-Flag"},{"Field":"Header","Type":"EqualTo","Value":"Yes","ValueSecond":"X-Spam"}],"ConditionsType":"Any","ActionType":"MoveTo","ActionValue":"Junk","ActionValueSecond":"","ActionValueThird":"","ActionValueFourth":"","Keep":true,"Stop":true,"MarkAsRead":false}"#;
const LIST: &str = r#"{"ID":"6orktqi8w2n5v0c3b7x1z9q4m8k2j6hd","Enabled":true,"Name":"ML-list","Conditions":[{"Field":"Recipient","Type":"Contains","Value":"list@lists.example.org","ValueSecond":""}],"ConditionsType":"Any","ActionType":"MoveTo","ActionValue":"Mailing-Lists/list","ActionValueSecond":"","ActionValueThird":"","ActionValueFourth":"","Keep":true,"Stop":true,"MarkAsRead":false}"#;

const SPAM_SIEVE: &str =
    "if anyof(\n    header :is [\"X-Spam-Flag\"] \"YES\",\n    header :is [\"X-Spam\"] \"Yes\"\n)\n{\n    fileinto \"Junk\";\n    stop;\n}";
const LIST_SIEVE: &str = "if header :contains [\"To\", \"CC\"] \"list@lists.example.org\"\n{\n    fileinto \"Mailing-Lists/list\";\n    stop;\n}";
const SPAM_ID: &str = "0000017000590660000";
const LIST_ID: &str = "6orktqi8w2n5v0c3b7x1z9q4m8k2j6hd";

/// A filter's `BEGIN:FILTER` comment, written here apart from the code under test.
fn header(id: &str, json: &str) -> String {
    let b64 = base64_encode(json.as_bytes());
    let lines: Vec<String> = b64.as_bytes().chunks(74).map(|c| String::from_utf8(c.to_vec()).unwrap()).collect();
    format!("/*\nBEGIN:FILTER:{id}\nBEGIN:HEADER\n{}\nEND:HEADER\n*/", lines.join("\n"))
}

fn block(id: &str, json: &str, sieve: &str) -> String {
    format!("{}\n\n{sieve}\n\n/* END:FILTER */", header(id, json))
}

const HEAD: &str =
    "require [\"fileinto\",\"body\"];\n\n# This is SnappyMail sieve script.\n# Please don't change anything here.\n# RAINLOOP:SIEVE\n\n";

/// The script, as the webmail was seen to write it.
fn script() -> String {
    format!("{HEAD}{}\n\n{}\n", block(SPAM_ID, SPAM, SPAM_SIEVE), block(LIST_ID, LIST, LIST_SIEVE))
}

/// `text` with Spam's settings `json` in place of the ones seen.
fn spam_as(text: &str, json: &str) -> String {
    text.replace(&header(SPAM_ID, SPAM), &header(SPAM_ID, json))
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn err(s: &mut Session, id: &str, params: Value) -> String {
    s.run(id, &params).map(|v| panic!("{id} {params} ran: {v}")).unwrap_err().to_string()
}

/// An IMAP account with the folders Junk and Mailing-Lists/list, whose server runs `text` as the
/// owner's script `snappymail`; the account, Junk and the list's folder.
fn session(text: &str) -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let junk = run(&mut s, "folder.new", json!({"name": "Junk", "parent": inbox}))["id"].as_u64().unwrap();
    s.mailbox.folder_mut(junk).unwrap().remote = "Junk".into();
    let lists = run(&mut s, "folder.new", json!({"name": "list", "parent": inbox}))["id"].as_u64().unwrap();
    s.mailbox.folder_mut(lists).unwrap().remote = "Mailing-Lists/list".into();
    read(&mut s, account, text);
    (s, account, junk, lists)
}

/// What the server has: the owner's script, active, with `text`.
fn read(s: &mut Session, account: Id, text: &str) {
    let state = json!({"extensions": ALL, "scripts": [{"name": "snappymail", "active": true}], "script": null, "activeScript": {"name": "snappymail", "text": text}});
    run(s, "sync.applySieve", json!({"account": account, "state": state}));
}

/// The rules the filters are shown as, in order.
fn rules(s: &Session, account: Id) -> Vec<mailcraft_model::Rule> {
    s.sieve[&account].elsewhere.as_ref().unwrap().rules.clone()
}

/// The text a command says the host is to write, and that it is written so: the owner's own
/// script, under the digest of the read, staying active, a whole script that checks.
fn written(answer: &Value, before: &str) -> String {
    let sieve = &answer["sieve"];
    assert_eq!((sieve["name"].as_str(), sieve["own"].as_bool(), sieve["activate"].as_bool()), (Some("snappymail"), Some(true), Some(false)));
    assert_eq!(sieve["lastWritten"], digest(before));
    let text = sieve["text"].as_str().unwrap().to_string();
    let offers: Vec<String> = ALL.map(String::from).to_vec();
    assert!(check(&text, Some(&offers)).is_ok(), "{text}");
    text
}

/// The rule's params as the builder sends them.
fn params(id: Id, name: &str, condition: Value, folder: Id) -> Value {
    json!({"id": id, "name": name, "conditions": [condition], "actions": [{"type": "moveTo", "value": folder}], "stop": true})
}

/// `new` is `old` with the bytes `old[at]` alone changed: what is before and after is the same.
fn only_changed(old: &str, new: &str, at: std::ops::Range<usize>) {
    assert_eq!(new.get(..at.start), old.get(..at.start));
    let tail = old.len() - at.end;
    assert_eq!(new.get(new.len() - tail..), old.get(at.end..));
}

#[test]
fn the_webmails_shape_is_written_as_it_was_seen() {
    let s = Settings::read(&serde_json::from_str(SPAM).unwrap()).unwrap();
    assert_eq!(s.json(), SPAM, "the keys in the webmail's order, nothing else");
    assert_eq!(s.begin_comment("\n"), header(SPAM_ID, SPAM), "base64 in lines of 74");
    assert_eq!(s.sieve(), SPAM_SIEVE);
    let l = Settings::read(&serde_json::from_str(LIST).unwrap()).unwrap();
    assert_eq!((l.json(), l.sieve()), (LIST.to_string(), LIST_SIEVE.to_string()));
    assert_eq!(l.block("\n"), block(LIST_ID, LIST, LIST_SIEVE));
}

#[test]
fn each_filter_is_a_rule_that_can_be_changed() {
    let text = script();
    let (mut s, account, junk, lists) = session(&text);
    let e = s.sieve[&account].elsewhere.as_ref().unwrap();
    assert_eq!(e.blocks.len(), 2);
    assert!(e.blocks.iter().all(|b| b.settings.is_ok() && b.at.is_some()), "{:?}", e.blocks);
    let r = rules(&s, account);
    assert_eq!(r.len(), 2);
    assert_eq!((r[0].name.as_str(), r[1].name.as_str()), ("Spam", "ML-list"));
    assert_eq!(
        r[0].conditions,
        vec![Condition::Header { names: vec!["X-Spam-Flag".into(), "X-Spam".into()], words: vec!["YES".into()], exact: true }]
    );
    assert_eq!(r[0].actions, vec![Action::MoveTo(junk)]);
    assert_eq!(r[1].conditions, vec![Condition::ToContains(vec!["list@lists.example.org".into()])]);
    assert_eq!(r[1].actions, vec![Action::MoveTo(lists)]);
    for one in &r {
        assert!(!one.read_only() && one.known() && one.enabled && one.stop);
        assert!(find(&s, one.id).is_some());
        assert_eq!(read_only(&s, one.id), None);
        assert_eq!(crate::cmd::rules::cannot_edit(&s.mailbox, one), None);
    }
    // Listed as the webmail's filters.
    let listed = run(&mut s, "rules.list", json!({"account": account}));
    let theirs = &listed["elsewhere"][0];
    assert_eq!((theirs["webmail"].as_bool(), theirs["rules"][1]["webmail"].as_bool()), (Some(true), Some(true)));
    assert_eq!(theirs["rules"][0]["readOnly"], false);
    assert!(theirs["rules"][1].get("readOnlyWhy").is_none());
}

#[test]
fn a_change_of_nothing_writes_the_script_back_byte_for_byte() {
    let text = script();
    let (mut s, account, junk, lists) = session(&text);
    let r = rules(&s, account);
    // Spam's condition as the builder shows it: the conditions the webmail wrote are kept as they are.
    let spam = json!({"type": "header", "value": {"names": ["X-Spam-Flag", "X-Spam"], "words": ["YES"], "exact": true}});
    let got = run(&mut s, "rules.edit", params(r[0].id, "Spam", spam, junk));
    assert_eq!(written(&got, &text), text);
    let got = run(&mut s, "rules.edit", params(r[1].id, "ML-list", json!({"type": "toContains", "value": "list@lists.example.org"}), lists));
    assert_eq!(written(&got, &text), text);
    // Switched on already: nothing to write.
    let got = run(&mut s, "rules.enable", json!({"id": r[0].id, "value": true}));
    assert!(got.get("sieve").is_none());
    // Moved up from the top: nothing either.
    assert_eq!(run(&mut s, "rules.move", json!({"id": r[0].id, "direction": "up"}))["moved"], false);
    // Nothing of the mailbox changed: the server's script is what changes.
    assert!(s.mailbox.rules.is_empty());
}

#[test]
fn a_change_rewrites_its_filter_alone_settings_and_sieve_together() {
    let text = script();
    let (mut s, account, junk, _) = session(&text);
    let r = rules(&s, account);
    let at = s.sieve[&account].elsewhere.as_ref().unwrap().blocks[1].at.clone().unwrap();
    let to = json!({"type": "toContains", "value": ["a@lists.example.org", "b@lists.example.org"]});
    let got = run(&mut s, "rules.edit", params(r[1].id, "Lists", to, junk));
    assert_eq!((got["id"].as_u64(), got["webmail"].as_bool()), (Some(r[1].id), Some(true)));
    let new = written(&got, &text);
    only_changed(&text, &new, at.clone());
    let json = r#"{"ID":"6orktqi8w2n5v0c3b7x1z9q4m8k2j6hd","Enabled":true,"Name":"Lists","Conditions":[{"Field":"Recipient","Type":"Contains","Value":"a@lists.example.org","ValueSecond":""},{"Field":"Recipient","Type":"Contains","Value":"b@lists.example.org","ValueSecond":""}],"ConditionsType":"Any","ActionType":"MoveTo","ActionValue":"Junk","ActionValueSecond":"","ActionValueThird":"","ActionValueFourth":"","Keep":true,"Stop":true,"MarkAsRead":false}"#;
    let sieve = "if anyof(\n    header :contains [\"To\", \"CC\"] \"a@lists.example.org\",\n    header :contains [\"To\", \"CC\"] \"b@lists.example.org\"\n)\n{\n    fileinto \"Junk\";\n    stop;\n}";
    assert_eq!(new.get(at.start..new.len() - (text.len() - at.end)), Some(block(LIST_ID, json, sieve).as_str()));
    // Read back from the server, it is a rule again, as changed.
    read(&mut s, account, &new);
    let after = rules(&s, account);
    assert_eq!(after[1].name, "Lists");
    assert_eq!(after[1].conditions, vec![Condition::ToContains(vec!["a@lists.example.org".into(), "b@lists.example.org".into()])]);
    assert_eq!(after[1].actions, vec![Action::MoveTo(junk)]);
    assert!(!after[1].read_only());

    // A header condition on two names and two words is each pair, any one enough.
    let header = json!({"type": "header", "value": {"names": ["X-Spam", "X-Junk"], "words": ["yes", "probably"]}});
    let changed = s.sieve[&account].elsewhere.as_ref().unwrap().blocks[0].at.clone().unwrap();
    let got = run(&mut s, "rules.edit", params(after[0].id, "Spam", header, junk));
    let newer = written(&got, &new);
    only_changed(&new, &newer, changed);
    let tests = "    header :contains [\"X-Spam\"] \"yes\",\n    header :contains [\"X-Spam\"] \"probably\",\n    header :contains [\"X-Junk\"] \"yes\",\n    header :contains [\"X-Junk\"] \"probably\"\n)";
    assert!(newer.contains(tests), "{newer}");
    read(&mut s, account, &newer);
    let spam = &rules(&s, account)[0];
    assert!(!spam.read_only());
    let both = Condition::Header { names: vec!["X-Spam".into(), "X-Junk".into()], words: vec!["yes".into(), "probably".into()], exact: false };
    assert_eq!(spam.conditions, vec![both]);
}

#[test]
fn what_the_webmails_filters_cant_do_is_refused() {
    let text = script();
    let (mut s, account, junk, _) = session(&text);
    let id = rules(&s, account)[1].id;
    let to = json!({"type": "toContains", "value": "x@example.org"});
    let moving = json!([{"type": "moveTo", "value": junk}]);
    let two = json!({"id": id, "name": "x", "conditions": [to, {"type": "subjectContains", "value": "y"}], "actions": moving});
    assert_eq!(err(&mut s, "rules.edit", two), NotWebmail::Condition.said());
    let subject = params(id, "x", json!({"type": "subjectContains", "value": "y"}), junk);
    assert_eq!(err(&mut s, "rules.edit", subject), NotWebmail::Condition.said());
    let unless = json!({"id": id, "name": "x", "conditions": [to], "exceptions": [to], "actions": moving});
    assert_eq!(err(&mut s, "rules.edit", unless), NotWebmail::Condition.said());
    let flag = json!({"id": id, "name": "x", "conditions": [to], "actions": [{"type": "flag"}]});
    assert_eq!(err(&mut s, "rules.edit", flag), NotWebmail::Action.said());
    let go_on = json!({"id": id, "name": "x", "conditions": [to], "actions": moving, "stop": false});
    assert_eq!(err(&mut s, "rules.edit", go_on), NotWebmail::Action.said());
    let alert = json!({"id": id, "name": "x", "conditions": [to], "actions": moving, "alert": true});
    assert_eq!(err(&mut s, "rules.edit", alert), NotWebmail::Action.said());
    // Another account's folder.
    let bob = run(&mut s, "account.add", json!({"email": "bob@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    let other = s.mailbox.folder_of_kind(bob, FolderKind::Inbox).unwrap();
    assert_eq!(err(&mut s, "rules.edit", params(id, "x", to.clone(), other)), NotWebmail::Action.said());
    let bad_name = json!({"type": "header", "value": {"names": ["X Spam:"], "words": ["yes"]}});
    assert!(err(&mut s, "rules.edit", params(id, "x", bad_name, junk)).contains("header names"));
    assert!(
        err(&mut s, "rules.edit", json!({"id": id, "name": "x", "conditions": [to], "actions": moving, "server": false}))
            .contains("kept in your Sieve script")
    );
    // A stale read: refused, nothing said to write.
    let mut stale = params(id, "x", to.clone(), junk);
    stale["digest"] = json!(digest("something else"));
    assert_eq!(err(&mut s, "rules.edit", stale), OWN_CHANGED);
    // A header condition for a rule of MailCraft's own: refused.
    let header = json!([{"type": "header", "value": {"names": ["X-Spam"], "words": ["yes"]}}]);
    let mine = json!({"name": "x", "account": account, "conditions": header, "actions": [{"type": "flag"}]});
    assert!(err(&mut s, "rules.add", mine).contains("only for a filter of your webmail's Sieve script"));
    assert!(s.mailbox.rules.is_empty());
}

#[test]
fn a_new_filter_goes_after_the_last_with_a_fresh_id() {
    let text = script();
    let (mut s, account, junk, _) = session(&text);
    let kind = json!({"type": "header", "value": {"names": ["X-Kind"], "words": ["invoice"], "exact": true}});
    let new = json!({"webmail": true, "account": account, "name": "Invoices", "conditions": [kind], "actions": [{"type": "moveTo", "value": junk}], "stop": true});
    let got = run(&mut s, "rules.add", new.clone());
    let added = written(&got, &text);
    // Everything before stays; after the last filter, set apart as the filters are, the new one.
    let end = text.rfind("/* END:FILTER */").unwrap() + "/* END:FILTER */".len();
    assert_eq!(added.get(..end), text.get(..end));
    assert!(added.ends_with("/* END:FILTER */\n"));
    let tail = added.get(end..).unwrap();
    assert!(tail.starts_with("\n\n/*\nBEGIN:FILTER:"), "{tail}");
    read(&mut s, account, &added);
    let e = s.sieve[&account].elsewhere.as_ref().unwrap();
    assert_eq!(e.blocks.len(), 3);
    let id = e.blocks[2].id.clone();
    assert!(id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()), "{id}");
    assert!(e.blocks[2].settings.is_ok());
    let r = rules(&s, account);
    assert_eq!(r[2].name, "Invoices");
    assert_eq!(r[2].conditions, vec![Condition::Header { names: vec!["X-Kind".into()], words: vec!["invoice".into()], exact: true }]);
    // Asked again with the same clock, the id isn't one of those there.
    let again = written(&run(&mut s, "rules.add", new.clone()), &added);
    read(&mut s, account, &again);
    let e = s.sieve[&account].elsewhere.as_ref().unwrap();
    assert_ne!(e.blocks[3].id, id);

    // A script with no filter yet, whose require lacks fileinto: it gains it, in its list's shape.
    let bare = "require [\"body\", \"vacation\"];\n\n# RAINLOOP:SIEVE\n";
    let (mut s, account, junk, _) = session(bare);
    let mut first = new.clone();
    first["account"] = json!(account);
    first["actions"] = json!([{"type": "moveTo", "value": junk}]);
    let got = written(&run(&mut s, "rules.add", first), bare);
    assert!(got.starts_with("require [\"body\", \"vacation\", \"fileinto\"];\n\n# RAINLOOP:SIEVE\n\n/*\nBEGIN:FILTER:"), "{got}");
    assert!(got.ends_with("/* END:FILTER */\n"));
    // Not a webmail's script: no filter is added to it.
    let (mut s, account, junk, _) = session("require \"fileinto\";\nkeep;\n");
    let mut other = new;
    other["account"] = json!(account);
    other["actions"] = json!([{"type": "moveTo", "value": junk}]);
    assert!(err(&mut s, "rules.add", other).contains("isn't the one this account's server runs"));
}

#[test]
fn require_gains_an_extension_in_the_shape_of_its_list() {
    assert_eq!(with_require("require [\"body\"];\nkeep;", "fileinto", "\n").unwrap(), "require [\"body\",\"fileinto\"];\nkeep;");
    assert_eq!(with_require("require [\"a\", \"b\"];", "fileinto", "\n").unwrap(), "require [\"a\", \"b\", \"fileinto\"];");
    assert_eq!(with_require("require \"body\";", "fileinto", "\n").unwrap(), "require [\"body\",\"fileinto\"];");
    assert_eq!(with_require("require [];", "fileinto", "\n").unwrap(), "require [\"fileinto\"];");
    assert_eq!(with_require("keep;\r\n", "fileinto", "\r\n").unwrap(), "require [\"fileinto\"];\r\n\r\nkeep;\r\n");
    // Asked for already, in any require: unchanged.
    let both = "require \"body\";\r\nrequire [\"FileInto\"];\r\n";
    assert_eq!(with_require(both, "fileinto", "\r\n").unwrap(), both);
    assert_eq!(with_require("\"unclosed", "fileinto", "\n"), None);
}

#[test]
fn a_filter_is_deleted_and_moved_whole() {
    let text = script();
    let spam = block(SPAM_ID, SPAM, SPAM_SIEVE);
    let list = block(LIST_ID, LIST, LIST_SIEVE);
    let (mut s, account, _, _) = session(&text);
    let r = rules(&s, account);
    // Down, and up: the two change places, byte for byte.
    let down = written(&run(&mut s, "rules.move", json!({"id": r[0].id, "direction": "down"})), &text);
    assert_eq!(down, format!("{HEAD}{list}\n\n{spam}\n"));
    let up = written(&run(&mut s, "rules.move", json!({"id": r[1].id, "direction": "up"})), &text);
    assert_eq!(up, down);
    // The first deleted, with what set it apart from the next; the last, with what set it apart
    // from the one before.
    assert_eq!(written(&run(&mut s, "rules.remove", json!({"id": r[0].id})), &text), format!("{HEAD}{list}\n"));
    assert_eq!(written(&run(&mut s, "rules.remove", json!({"id": r[1].id})), &text), format!("{HEAD}{spam}\n"));
    let one = format!("{HEAD}{spam}\n");
    read(&mut s, account, &one);
    let only = rules(&s, account)[0].id;
    assert_eq!(written(&run(&mut s, "rules.remove", json!({"id": only})), &one), format!("{}\n", HEAD.trim_end()));
    // An agent's delete there is asked as a delete: undo doesn't reach the server.
    assert_eq!(classify(&s, "rules.remove", &json!({"id": only})), Access::Owner(Class::Delete));
    let said = describe(&s, "rules.remove", &json!({"id": only}));
    assert!(said.starts_with("delete the filter “Spam” in your own Sieve script “snappymail” on the server of “ada@example.org”"), "{said}");
}

#[test]
fn a_filter_is_switched_off_and_on_again() {
    let text = script();
    let (mut s, account, _, _) = session(&text);
    let r = rules(&s, account);
    let at = s.sieve[&account].elsewhere.as_ref().unwrap().blocks[0].at.clone().unwrap();
    let off = written(&run(&mut s, "rules.enable", json!({"id": r[0].id, "value": false})), &text);
    only_changed(&text, &off, at);
    // Its settings say so; its Sieve is left out, its comments kept (not seen yet: `off_block`).
    let json = SPAM.replace("\"Enabled\":true", "\"Enabled\":false");
    assert!(off.contains(&format!("{}\n\n/* END:FILTER */", header(SPAM_ID, &json))), "{off}");
    assert!(!off.contains("X-Spam-Flag\"] \"YES\""));
    read(&mut s, account, &off);
    let shown = rules(&s, account);
    assert_eq!(shown.len(), 2);
    assert!(!shown[0].enabled && !shown[0].read_only() && shown[0].name == "Spam");
    assert_eq!(shown[0].conditions, r[0].conditions);
    // On again: its Sieve is written from its settings, and the script is as it was.
    let on = written(&run(&mut s, "rules.enable", json!({"id": shown[0].id})), &off);
    assert_eq!(on, text);
    // An agent is asked as for a change of settings.
    assert_eq!(classify(&s, "rules.enable", &json!({"id": shown[0].id})), Access::Owner(Class::Settings));
    assert!(describe(&s, "rules.enable", &json!({"id": shown[0].id})).starts_with("switch on the filter “Spam”"));
}

#[test]
fn a_filter_changed_by_hand_or_with_settings_not_known_is_read_only() {
    let text = script();
    let recipient = r#"{"Field":"Recipient","Type":"Contains","Value":"Yes","ValueSecond":""}"#;
    let second = r#"{"Field":"Header","Type":"EqualTo","Value":"Yes","ValueSecond":"X-Spam"}"#;
    let cases = [
        // Its Sieve changed by hand.
        (text.replace("fileinto \"Junk\";", "fileinto \"Spam\";"), ReadOnly::Disagrees),
        (text.replacen("    stop;\n", "", 1), ReadOnly::Disagrees),
        // Settings not known: another kind of condition, All, another action, mark as read, a key more.
        (spam_as(&text, &SPAM.replace("\"Header\"", "\"Subject\"")), ReadOnly::Unknown),
        (spam_as(&text, &SPAM.replace("\"Any\"", "\"All\"")), ReadOnly::Unknown),
        (spam_as(&text, &SPAM.replace("\"MoveTo\"", "\"Forward\"")), ReadOnly::Unknown),
        (spam_as(&text, &SPAM.replace("\"MarkAsRead\":false", "\"MarkAsRead\":true")), ReadOnly::Unknown),
        (spam_as(&text, &SPAM.replace("\"Keep\":true", "\"Keep\":false")), ReadOnly::Unknown),
        (spam_as(&text, &SPAM.replace("{\"ID\"", "{\"Later\":1,\"ID\"")), ReadOnly::Unknown),
        // Its settings unreadable.
        (text.replace(&header(SPAM_ID, SPAM), "/*\nBEGIN:FILTER:0000017000590660000\nBEGIN:HEADER\n!!!\nEND:HEADER\n*/"), ReadOnly::Unreadable),
        // A folder MailCraft doesn't have.
        (spam_as(&text, &SPAM.replace("\"Junk\"", "\"Elsewhere\"")).replace("\"Junk\";", "\"Elsewhere\";"), ReadOnly::Folder),
        // Conditions that are no one condition: a recipient or a header.
        (
            spam_as(&text, &SPAM.replace(second, recipient)).replace("header :is [\"X-Spam\"] \"Yes\"", "header :contains [\"To\", \"CC\"] \"Yes\""),
            ReadOnly::Shape,
        ),
    ];
    for (changed, why) in cases {
        let (mut s, account, junk, _) = session(&changed);
        let e = s.sieve[&account].elsewhere.as_ref().unwrap();
        assert_eq!(e.blocks[0].settings.as_ref().err(), Some(&why), "{changed}");
        assert!(e.blocks[1].settings.is_ok());
        let r = rules(&s, account);
        assert!(r[0].read_only(), "{why:?}");
        assert_eq!(read_only(&s, r[0].id), Some(why));
        let refused = err(&mut s, "rules.edit", params(r[0].id, "x", json!({"type": "toContains", "value": "x"}), junk));
        assert!(refused.contains(why.said()), "{refused}");
        assert!(err(&mut s, "rules.remove", json!({"id": r[0].id})).contains(why.said()));
        let listed = s.run("rules.list", &json!({})).unwrap();
        assert_eq!(listed["elsewhere"][0]["rules"][0]["readOnlyWhy"], why.said());
        // The filter beside it is moved past it, both whole.
        let moved = written(&run(&mut s, "rules.move", json!({"id": r[1].id, "direction": "up"})), &changed);
        assert_eq!(moved.len(), changed.len());
        assert!(moved.find("BEGIN:FILTER:6ork") < moved.find("BEGIN:FILTER:0000"));
    }
    // White space at the ends of lines and around the Sieve isn't a change by hand.
    let spaced = text.replacen("    stop;\n", "    stop;   \n", 1).replace("\n\nif anyof", "\n\n\nif anyof");
    let (s, account, _, _) = session(&spaced);
    assert!(s.sieve[&account].elsewhere.as_ref().unwrap().blocks[0].settings.is_ok());
}

#[test]
fn a_rule_of_a_script_not_the_webmails_stays_read_only() {
    let roundcube = "# rule:[Lists]\nrequire \"fileinto\";\nif header :contains \"subject\" \"[list]\" {\n  fileinto \"Junk\";\n}\n";
    let (mut s, account, junk, _) = session(roundcube);
    let r = rules(&s, account);
    assert!(r[0].read_only() && find(&s, r[0].id).is_none());
    let refused = err(&mut s, "rules.edit", params(r[0].id, "x", json!({"type": "toContains", "value": "x"}), junk));
    assert!(refused.contains("MailCraft shows it and never changes it"), "{refused}");
    assert!(err(&mut s, "rules.enable", json!({"id": r[0].id})).contains("never changes it"));
    // No signature: no filter of the webmail's, even with its comments.
    let unsigned = script().replace("# RAINLOOP:SIEVE\n", "");
    let (s, account, _, _) = session(&unsigned);
    assert!(s.sieve[&account].elsewhere.as_ref().unwrap().blocks.is_empty());
    assert!(rules(&s, account).iter().all(|r| r.read_only()));
}

#[test]
fn line_ends_of_crlf_are_kept() {
    let text = script().replace('\n', "\r\n");
    let (mut s, account, junk, _) = session(&text);
    let r = rules(&s, account);
    assert!(r.iter().all(|r| !r.read_only()));
    let spam = json!({"type": "header", "value": {"names": ["X-Spam-Flag", "X-Spam"], "words": ["YES"], "exact": true}});
    assert_eq!(written(&run(&mut s, "rules.edit", params(r[0].id, "Spam", spam, junk)), &text), text);
    let off = written(&run(&mut s, "rules.enable", json!({"id": r[1].id})), &text);
    assert!(!off.replace("\r\n", "").contains('\n'));
}

#[test]
fn an_agent_is_asked_about_a_filter_with_what_it_does() {
    let text = script();
    let (s, account, junk, _) = session(&text);
    let r = rules(&s, account);
    let change = params(r[1].id, "Lists", json!({"type": "toContains", "value": "other@example.org"}), junk);
    assert_eq!(classify(&s, "rules.edit", &change), Access::Owner(Class::Settings));
    let said = describe(&s, "rules.edit", &change);
    assert!(
        said.starts_with(
            "change the filter “ML-list” in your own Sieve script “snappymail” on the server of “ada@example.org”, as your webmail writes it"
        ),
        "{said}"
    );
    assert!(said.contains(", naming it “Lists”, from ") && said.contains("other@example.org"), "{said}");
    let add = json!({"webmail": true, "account": account, "name": "New", "conditions": [{"type": "toContains", "value": "x@example.org"}], "actions": [{"type": "moveTo", "value": junk}]});
    assert_eq!(classify(&s, "rules.add", &add), Access::Owner(Class::Settings));
    assert!(describe(&s, "rules.add", &add).starts_with("create the filter “New” in your own Sieve script"));
    let up = json!({"id": r[1].id, "direction": "up"});
    assert_eq!(classify(&s, "rules.move", &up), Access::Owner(Class::Settings));
    assert!(describe(&s, "rules.move", &up).starts_with("move the filter “ML-list” up"));
}

proptest! {
    #[test]
    fn hostile_settings_never_panic_and_a_known_filter_stays_byte_for_byte(
        name in ".{0,60}",
        value in ".{0,60}",
        second in ".{0,30}",
        id in "[a-z0-9]{0,40}|.{0,20}",
        bytes in proptest::collection::vec(any::<u8>(), 0..200),
        field in prop::sample::select(vec!["Header", "Recipient", "From", ""]),
    ) {
        let json = json!({"ID": id, "Enabled": true, "Name": name, "Conditions": [{"Field": field, "Type": "Contains", "Value": value, "ValueSecond": second}],
            "ConditionsType": "Any", "ActionType": "MoveTo", "ActionValue": "Junk", "ActionValueSecond": "", "ActionValueThird": "", "ActionValueFourth": "",
            "Keep": true, "Stop": true, "MarkAsRead": false});
        let mut texts = vec![format!("# RAINLOOP:SIEVE\n\n{}\n", header(&id, &json.to_string()))];
        if let Ok(settings) = Settings::read(&json) {
            texts.push(format!("require \"fileinto\";\n# RAINLOOP:SIEVE\n\n{}\n", settings.block("\n")));
        }
        texts.push(format!("# RAINLOOP:SIEVE\n/*\nBEGIN:FILTER:{id}\nBEGIN:HEADER\n{}\nEND:HEADER\n*/\nkeep;\n/* END:FILTER */", base64_encode(&bytes)));
        texts.push(format!("# RAINLOOP:SIEVE\n/*\nBEGIN:FILTER:x\nBEGIN:HEADER\n{name}\nEND:HEADER\n*/{value}/* END:FILTER */"));
        for text in texts {
            let (mut s, account, junk, _) = session(&text);
            let Some(e) = s.sieve[&account].elsewhere.clone() else { continue };
            for b in &e.blocks {
                prop_assert!(b.at.clone().is_none_or(|at| text.get(at).is_some()));
                let Some(rule) = e.rules.iter().find(|x| Some(x.id) == b.rule) else { continue };
                // (A name or a word with white space at its ends is shown, and saved, trimmed.)
                let as_shown = rule.name == name && value.trim() == value && second.trim() == second;
                if b.settings.is_ok() && as_shown {
                    // Saved as it is shown, it is written back as it was.
                    let condition = match &rule.conditions[0] {
                        Condition::ToContains(w) => json!({"type": "toContains", "value": w}),
                        Condition::Header { names, words, exact } => json!({"type": "header", "value": {"names": names, "words": words, "exact": exact}}),
                        other => panic!("{other:?}"),
                    };
                    if let Ok(got) = s.run("rules.edit", &params(rule.id, &rule.name, condition, junk)) {
                        prop_assert_eq!(got["sieve"]["text"].as_str(), Some(text.as_str()));
                    }
                }
                let _ = s.run("rules.enable", &json!({"id": rule.id}));
                let _ = s.run("rules.remove", &json!({"id": rule.id}));
                let _ = s.run("rules.move", &json!({"id": rule.id, "direction": "down"}));
            }
            let add = json!({"webmail": true, "account": account, "name": name, "conditions": [{"type": "toContains", "value": value}], "actions": [{"type": "moveTo", "value": junk}]});
            let _ = s.run("rules.add", &add);
        }
    }
}
