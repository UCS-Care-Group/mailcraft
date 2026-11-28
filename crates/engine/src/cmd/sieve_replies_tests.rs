// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Automatic Replies of an IMAP account kept in a Sieve script (#2057): what is to be written, and
//! what a ManageSieve server's answer comes to. The server's side is written out by hand, as
//! `mailcraft-net`'s `sieve` brings it.

use mailcraft_model::Id;
use serde_json::{Value, json};

use super::auto_replies::{NOT_MICROSOFT, NOT_READ};
use super::sieve_replies::{CHANGED_ELSEWHERE, NO_CONTACTS, NO_SCHEDULE, NO_VACATION};
use crate::Session;
use crate::agent::{self, Access, Class};
use crate::sieve::{SCRIPT_NAME, digest, read_vacation};

/// 2026-09-21T13:46:40Z.
const NOW: i64 = 1_790_000_000;
const ALL: [&str; 4] = ["fileinto", "vacation", "date", "relational"];

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn refusal(s: &mut Session, id: &str, params: Value) -> String {
    s.run(id, &params).err().map(|e| e.to_string()).unwrap_or_else(|| panic!("{id} {params} went through"))
}

/// The server's answer to a read: its extensions, its scripts, and MailCraft's script's text.
fn state(extensions: &[&str], active: Option<&str>, script: Option<&str>) -> Value {
    let mut scripts = vec![];
    if let Some(name) = active {
        scripts.push(json!({"name": name, "active": true}));
    }
    json!({"extensions": extensions, "scripts": scripts, "script": script})
}

fn read(s: &mut Session, account: Id, state: Value) -> Value {
    run(s, "sync.applySieve", json!({"account": account, "state": state}))
}

#[test]
fn a_read_with_no_script_yet_knows_the_replies_are_off_and_asks_for_the_sieve_read() {
    let (mut s, account) = session();
    let asked = run(&mut s, "account.automaticReplies", json!({"account": account}));
    assert_eq!(asked["known"], Value::Null);
    assert_eq!(asked["ask"], json!({"account": account, "sieve": SCRIPT_NAME}));
    assert_eq!(asked["sieve"]["read"], false);
    // Not the default account for them: that stays a Microsoft one.
    assert!(refusal(&mut s, "account.automaticReplies", json!({})).contains(NOT_MICROSOFT));

    let answer = read(&mut s, account, state(&ALL, None, None));
    assert_eq!(answer["known"]["status"], "disabled");
    assert_eq!(answer["changedElsewhere"], false);
    let asked = run(&mut s, "account.automaticReplies", json!({"account": account}));
    assert_eq!(asked["sieve"]["replies"], true);
    assert_eq!(asked["sieve"]["schedule"], true);
}

#[test]
fn a_change_says_which_script_to_write_and_its_digest_is_kept_once_written() {
    let (mut s, account) = session();
    read(&mut s, account, state(&ALL, None, None));
    let change = run(
        &mut s,
        "account.setAutomaticReplies",
        json!({"account": account, "status": "alwaysEnabled", "internal": "Back Monday.", "external": "Away\"; discard;", "audience": "all"}),
    );
    assert_eq!(change["pending"], "automaticReplies");
    assert_eq!(change["sieve"]["name"], SCRIPT_NAME);
    assert_eq!(change["sieve"]["lastWritten"], Value::Null);
    assert_eq!(change["sieve"]["activate"], true);
    let text = change["sieve"]["text"].as_str().unwrap().to_string();
    let vacation = read_vacation(&text).unwrap();
    assert_eq!((vacation.internal.as_str(), vacation.external.as_str()), ("Back Monday.", "Away\"; discard;"));
    assert!(text.contains(":addresses [\"ada@example.org\"]") && text.contains("\"example.org\""), "{text}");

    // Written: the digest is kept with the account, and the server's script is known as ours.
    let answer = run(
        &mut s,
        "sync.applySieve",
        json!({"account": account, "state": state(&ALL, Some(SCRIPT_NAME), Some(&text)), "written": {"result": "done", "text": text}}),
    );
    assert_eq!(answer["known"]["status"], "alwaysEnabled");
    assert_eq!(answer["known"]["on"], true);
    assert_eq!(s.mailbox.account(account).unwrap().sieve_written, digest(&text));
    // The next change names it.
    let change = run(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "disabled"}));
    assert_eq!(change["sieve"]["lastWritten"], digest(&text));
    assert_eq!(change["sieve"]["activate"], false);
    // What isn't given stays as the server has it.
    let off = read_vacation(change["sieve"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(off.status, mailcraft_model::auto_replies::AutoReplyStatus::Disabled);
}

#[test]
fn a_script_changed_elsewhere_is_never_written_over() {
    let (mut s, account) = session();
    // Somebody else's script under MailCraft's name, MailCraft never having written one.
    let answer = read(&mut s, account, state(&ALL, Some(SCRIPT_NAME), Some("require \"fileinto\";\r\nfileinto \"Lists\";\r\n")));
    assert_eq!(answer["changedElsewhere"], true);
    assert_eq!(answer["known"], Value::Null);
    assert!(refusal(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled"})).contains(CHANGED_ELSEWHERE));

    // MailCraft's script, changed since: the same.
    let (mut s, account) = session();
    read(&mut s, account, state(&ALL, None, None));
    let text = run(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "internal": "x"}))["sieve"]["text"]
        .as_str()
        .unwrap()
        .to_string();
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state(&ALL, None, Some(&text)), "written": {"result": "done", "text": text}}));
    let edited = text.replace("\"x\"", "\"y\"");
    assert_eq!(read(&mut s, account, state(&ALL, None, Some(&edited)))["changedElsewhere"], true);
    // And removed elsewhere.
    assert_eq!(read(&mut s, account, state(&ALL, None, None))["changedElsewhere"], true);
    assert!(refusal(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "disabled"})).contains(CHANGED_ELSEWHERE));
}

#[test]
fn the_owners_own_active_script_is_said_and_left_alone() {
    let (mut s, account) = session();
    let answer = read(&mut s, account, state(&ALL, Some("roundcube"), None));
    assert_eq!(answer["active"], "roundcube");
    let text = run(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled"}))["sieve"]["text"]
        .as_str()
        .unwrap()
        .to_string();
    // Written beside it, not active: kept as MailCraft's all the same.
    let answer = run(
        &mut s,
        "sync.applySieve",
        json!({"account": account, "state": state(&ALL, Some("roundcube"), Some(&text)), "written": {"result": "anotherActive", "text": text}}),
    );
    assert_eq!((answer["active"].as_str(), answer["changedElsewhere"].as_bool()), (Some("roundcube"), Some(false)));
    assert_eq!(run(&mut s, "account.automaticReplies", json!({"account": account}))["sieve"]["active"], "roundcube");
}

#[test]
fn what_the_server_cant_do_is_refused_before_anything_is_written() {
    let (mut s, account) = session();
    assert!(refusal(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled"})).contains(NOT_READ));
    read(&mut s, account, state(&["fileinto"], None, None));
    assert!(refusal(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled"})).contains(NO_VACATION));
    read(&mut s, account, state(&["vacation"], None, None));
    let scheduled = json!({"account": account, "status": "scheduled", "start": "2026-09-22T08:00", "end": "2026-09-29T08:00"});
    assert!(refusal(&mut s, "account.setAutomaticReplies", scheduled.clone()).contains(NO_SCHEDULE));
    assert!(
        refusal(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "audience": "contactsOnly"}))
            .contains(NO_CONTACTS)
    );
    // With the date tests, a time range is a currentdate test.
    read(&mut s, account, state(&ALL, None, None));
    let text = run(&mut s, "account.setAutomaticReplies", scheduled)["sieve"]["text"].as_str().unwrap().to_string();
    assert!(text.contains("\"2026-09-22T08:00:00\"") && text.contains("\"2026-09-29T08:00:00\""), "{text}");
    // Offline: nothing.
    s.prefs.offline = true;
    assert!(s.run("account.setAutomaticReplies", &json!({"account": account, "status": "disabled"})).is_err());
}

#[test]
fn hostile_answers_are_refused_or_capped() {
    let (mut s, account) = session();
    for bad in [
        json!({"account": account}),
        json!({"account": account, "state": "x"}),
        json!({"account": account, "state": {"script": 5}}),
        json!({"account": account, "state": {"script": "#".repeat(crate::sieve::MAX_SCRIPT + 1)}}),
        json!({"account": account, "state": {}, "written": {"result": "done"}}),
        json!({"account": account, "state": {}, "written": {"result": "whatever", "text": "x"}}),
    ] {
        assert!(s.run("sync.applySieve", &bad).is_err(), "{bad}");
    }
    let many: Vec<String> = (0..10_000).map(|n| format!("ext{n}")).collect();
    run(&mut s, "sync.applySieve", json!({"account": account, "state": {"extensions": many}}));
    assert_eq!(s.sieve[&account].extensions.len(), 256);
    // Not for a Microsoft account, nor the mailbox on this computer.
    let ms = s.run("account.add", &json!({"email": "bo@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    let ms = ms["id"].as_u64().unwrap();
    assert!(refusal(&mut s, "sync.applySieve", json!({"account": ms, "state": {}})).contains(NO_VACATION));
}

#[test]
fn only_the_host_takes_in_what_the_server_said_and_a_change_needs_the_owner() {
    assert_eq!(agent::access("sync.applySieve"), Some(Access::Never));
    assert_eq!(agent::access("account.setAutomaticReplies"), Some(Access::Owner(Class::Send)));
}

#[test]
fn a_script_changed_elsewhere_is_replaced_only_when_asked_and_only_as_it_was_read() {
    let (mut s, account) = session();
    let theirs = "require \"fileinto\";\r\nfileinto \"Lists\";\r\n";
    read(&mut s, account, state(&ALL, Some(SCRIPT_NAME), Some(theirs)));
    let asked = json!({"account": account, "status": "alwaysEnabled", "internal": "Away.", "replace": true});
    let change = run(&mut s, "account.setAutomaticReplies", asked);
    // What is written over is the script as it was read, by its digest: changed again since, it stays.
    assert_eq!(change["sieve"]["lastWritten"], digest(theirs));
    assert_eq!(read_vacation(change["sieve"]["text"].as_str().unwrap()).unwrap().internal, "Away.");
    assert!(s.run("account.setAutomaticReplies", &json!({"account": account, "status": "disabled", "replace": "yes"})).is_err());
    // Removed elsewhere: replaced by writing it anew.
    let (mut s, account) = session();
    s.mailbox.account_mut(account).unwrap().sieve_written = digest("keep;");
    read(&mut s, account, state(&ALL, None, None));
    let change = run(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "disabled", "replace": true}));
    assert_eq!(change["sieve"]["lastWritten"], Value::Null);
}

#[test]
fn the_owner_may_name_another_managesieve_server_and_an_agent_is_asked_about_it() {
    let (mut s, account) = session();
    run(&mut s, "account.edit", json!({"id": account, "sieve": {"host": "sieve.example.org.", "port": 4191}}));
    let a = s.mailbox.account(account).unwrap();
    assert_eq!((a.sieve_host.as_str(), a.sieve_port), ("sieve.example.org", 4191));
    for wrong in [json!({"host": "a b"}), json!({"host": "x/y"}), json!({"port": 70_000}), json!({"host": 5}), json!("x")] {
        assert!(s.run("account.edit", &json!({"id": account, "sieve": wrong})).is_err(), "{wrong}");
    }
    let what = agent::describe(&s, "account.edit", &json!({"id": account, "sieve": {"host": "elsewhere.example.net"}}));
    assert!(what.contains("sieve elsewhere.example.net"), "{what}");
    assert_eq!(agent::access("account.edit"), Some(Access::Owner(Class::Accounts)));
    run(&mut s, "account.edit", json!({"id": account, "sieve": null}));
    let a = s.mailbox.account(account).unwrap();
    assert_eq!((a.sieve_host.as_str(), a.sieve_port), ("", 0));
}

/// Another ManageSieve server named in Account Settings: what was known of the one before (its
/// script, its replies) is no longer the account's, and is read again before anything is written;
/// a change of anything else keeps it (#2058).
#[test]
fn another_sieve_server_forgets_what_the_one_before_had() {
    let (mut s, account) = session();
    read(&mut s, account, state(&ALL, None, None));
    assert!(s.sieve.contains_key(&account) && s.auto_replies.contains_key(&account));
    run(&mut s, "account.edit", json!({"id": account, "name": "Ada"}));
    assert!(s.sieve.contains_key(&account) && s.auto_replies.contains_key(&account));
    run(&mut s, "account.edit", json!({"id": account, "sieve": {"host": "sieve.example.org", "port": 4191}}));
    assert!(!s.sieve.contains_key(&account) && !s.auto_replies.contains_key(&account));
    assert!(refusal(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "disabled"})).contains("read"));
}

/// A write the server took, and a read after it that failed: the write is taken in all the same,
/// so the next read knows the script as MailCraft's, not as one changed elsewhere (#2058).
#[test]
fn a_write_taken_without_a_read_after_it_is_still_known_as_mailcrafts() {
    let (mut s, account) = session();
    read(&mut s, account, state(&ALL, None, None));
    let change =
        run(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "internal": "Away.", "audience": "none"}));
    let text = change["sieve"]["text"].as_str().unwrap().to_string();
    run(&mut s, "sync.applySieve", json!({"account": account, "written": {"result": "done", "text": text}}));
    assert_eq!(s.mailbox.account(account).unwrap().sieve_written, digest(&text));
    let answer = read(&mut s, account, state(&ALL, Some(SCRIPT_NAME), Some(&text)));
    assert_eq!(answer["changedElsewhere"], false);
    // Without a write, a state is still required; a bad write changes nothing.
    assert!(refusal(&mut s, "sync.applySieve", json!({"account": account})).contains("state"));
    assert!(s.run("sync.applySieve", &json!({"account": account, "written": {"result": "odd"}})).is_err());
    assert_eq!(s.mailbox.account(account).unwrap().sieve_written, digest(&text));
}
