// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exchange server accounts, the skeleton (#252, E1): the switch, what the commands refuse, and a
//! mailbox that holds one opening whether the switch is on or off.

use std::path::PathBuf;

use mailcraft_model::{Account, AccountKind, EwsServer, FolderKind, Id, Mailbox, Message};
use serde_json::{Value, json};

use crate::Session;
use crate::cmd::ews_mail::{NOT_BUILT, SWITCHED_OFF};

const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

/// A folder of its own under the system's temporary one, removed when the test is over.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-ews-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("mailbox.mailcraft")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn exchange(email: &str) -> Value {
    json!({"email": email, "kind": "exchange", "ews": {"url": URL, "username": "CORP\\ada"}})
}

fn session(ews: bool) -> Session {
    let mut s = Session::empty();
    s.ews = ews;
    s
}

fn said(result: Result<Value, crate::CmdError>) -> String {
    let said = result.expect_err("should have been refused").to_string();
    // (A command that is out of reach says so first.)
    said.strip_prefix("not available: ").map_or(said.clone(), str::to_string)
}

/// A mailbox with an Exchange server account whose Inbox has a message, as a later step's first
/// download will leave one.
fn mailbox_with_exchange() -> (Mailbox, Id) {
    let mut s = session(true);
    let id = s.run("account.add", &exchange("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    let mut mb = s.mailbox.clone();
    mb.account_mut(id).unwrap().ews.as_mut().unwrap().trusted = "ab".repeat(32);
    let inbox = mb.folder_of_kind(id, FolderKind::Inbox).unwrap();
    mb.add_message(Message { folder: inbox, subject: "Kept from the server".into(), date: 10, ..Default::default() }).unwrap();
    (mb, id)
}

#[test]
fn switched_off_an_exchange_account_cannot_be_added_and_no_command_runs() {
    let mut s = session(false);
    assert_eq!(said(s.run("account.add", &exchange("ada@example.org"))), SWITCHED_OFF);
    // Any spelling of the kind, and nothing in the params turns the switch on.
    let sneaky = json!({"email": "ada@example.org", "kind": " Exchange ", "ews": {"url": URL}, "switch": true, "ewsEnabled": true});
    assert_eq!(said(s.run("account.add", &sneaky)), SWITCHED_OFF);
    assert!(!s.ews);
    assert!(s.mailbox.accounts.iter().all(|a| a.kind != AccountKind::Exchange));
    for command in ["sync.ewsRequest", "sync.applyEws", "sync.ewsFetchRequest"] {
        for params in [json!({}), json!({"account": 1}), json!({"result": {"account": 1}}), json!({"ews": true})] {
            assert_eq!(said(s.run(command, &params)), SWITCHED_OFF, "{command} {params}");
        }
    }
}

#[test]
fn switched_on_an_exchange_account_is_added_and_its_commands_say_it_is_not_built() {
    let mut s = session(true);
    assert!(s.ews);
    let id = s.run("account.add", &exchange("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    let a = s.mailbox.account(id).unwrap();
    assert_eq!(a.kind, AccountKind::Exchange);
    assert!(a.incoming.is_none() && a.outgoing.is_none() && a.oauth.is_none() && a.jmap.is_none());
    assert_eq!(a.ews, Some(EwsServer { url: URL.into(), username: "CORP\\ada".into(), trusted: String::new(), hierarchy: String::new() }));
    // It has the standard folders a server account has.
    assert!(s.mailbox.folder_of_kind(id, FolderKind::Inbox).is_some());
    // The list says where it connects, never the whole address.
    let listed = s.run("account.list", &json!({})).unwrap();
    let text = listed.to_string();
    assert!(text.contains("https://exchange.example.org") && !text.contains("Exchange.asmx"), "{text}");
    assert_eq!(mailcraft_engine_text(&s, id), "exchange https://exchange.example.org");
    // Its commands check what they are given as the real ones will, then say it is not built.
    assert_eq!(said(s.run("sync.ewsRequest", &json!({}))), NOT_BUILT);
    assert_eq!(said(s.run("sync.ewsRequest", &json!({"account": id, "light": true}))), NOT_BUILT);
    assert_eq!(said(s.run("sync.ewsFetchRequest", &json!({"account": id}))), NOT_BUILT);
    let result = json!({"result": {"account": id, "email": "ada@example.org", "whole": true}});
    assert_eq!(said(s.run("sync.applyEws", &result)), NOT_BUILT);
    // The ordinary request for a server account says the same, not that it isn't connected.
    assert_eq!(said(s.run("sync.request", &json!({"account": id}))), NOT_BUILT);
}

fn mailcraft_engine_text(s: &Session, id: Id) -> String {
    crate::cmd::account::servers_text(s.mailbox.account(id).unwrap())
}

#[test]
fn the_commands_refuse_what_is_not_theirs_and_change_nothing() {
    let mut s = session(true);
    let local = s.run("account.add", &json!({"email": "local@example.org"})).unwrap()["id"].as_u64().unwrap();
    let id = s.run("account.add", &exchange("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    let before = s.mailbox.clone();
    // Not an Exchange server account; one that isn't there.
    assert!(said(s.run("sync.ewsRequest", &json!({"account": local}))).contains("isn't an Exchange server account"));
    assert!(said(s.run("sync.ewsRequest", &json!({"account": 9999}))).contains("no account"));
    // Results that are not results, and results for another account.
    for result in [
        json!(5),
        json!(null),
        json!({"account": "x"}),
        json!({"account": local, "email": "local@example.org"}),
        json!({"account": id, "email": "other@example.org"}),
    ] {
        let refused = said(s.run("sync.applyEws", &json!({"result": result})));
        assert_ne!(refused, NOT_BUILT, "{result}");
    }
    assert_eq!(said(s.run("sync.applyEws", &json!({}))), "`result` is missing");
    assert!(s.mailbox == before);
}

#[test]
fn settings_that_cannot_be_used_are_refused_when_the_account_is_added_or_edited() {
    let mut s = session(true);
    for ews in [
        json!({"url": "http://exchange.example.org/EWS/Exchange.asmx"}),
        json!({"url": "https://user:pw@exchange.example.org/EWS/Exchange.asmx"}),
        json!({"url": "https://exchange.example.org/EWS/Exchange.asmx", "trusted": "nope"}),
        json!({"url": URL, "username": "a\nb"}),
        json!({"url": 5}),
        json!({}),
        json!("https://exchange.example.org"),
    ] {
        let params = json!({"email": "ada@example.org", "kind": "exchange", "ews": ews});
        assert!(s.run("account.add", &params).is_err(), "{ews}");
    }
    assert!(s.run("account.add", &json!({"email": "ada@example.org", "kind": "exchange"})).is_err(), "no settings at all");
    assert!(s.mailbox.accounts.iter().all(|a| a.kind != AccountKind::Exchange));
    let id = s.run("account.add", &exchange("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("account.edit", &json!({"id": id, "ews": {"url": "ftp://x.example.org"}})).is_err());
    assert!(s.run("account.edit", &json!({"id": id, "ews": {"trusted": "ab".repeat(32)}})).is_ok());
    assert_eq!(s.mailbox.account(id).unwrap().ews.as_ref().map(|e| (e.url.as_str(), e.trusted.len())), Some((URL, 64)));
    // An account is an Exchange server account from the start or never.
    let local = s.run("account.add", &json!({"email": "local@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("account.edit", &json!({"id": local, "kind": "exchange", "ews": {"url": URL}})).is_err());
    assert!(s.run("account.edit", &json!({"id": id, "kind": "imap"})).is_err());
}

#[test]
fn switched_off_an_account_already_there_keeps_its_mail_and_its_settings_and_does_not_sync() {
    let (mb, id) = mailbox_with_exchange();
    let mut s = session(false);
    s.mailbox = mb;
    // Its mail is shown.
    let inbox = s.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap();
    s.view.folder = inbox;
    assert_eq!(s.rows().len(), 1);
    // It is not synchronised, and the reason is the switch.
    assert_eq!(said(s.run("sync.request", &json!({"account": id}))), SWITCHED_OFF);
    assert_eq!(said(s.run("sync.ewsRequest", &json!({"account": id}))), SWITCHED_OFF);
    assert_eq!(crate::cmd::account::unpushed(&s, id), 0);
    // Its settings are not changed meanwhile, but its name can be.
    assert_eq!(said(s.run("account.edit", &json!({"id": id, "ews": {"url": URL}}))), SWITCHED_OFF);
    assert!(s.run("account.edit", &json!({"id": id, "displayName": "Ada"})).is_ok());
    assert_eq!(s.mailbox.account(id).unwrap().kind, AccountKind::Exchange);
    assert!(s.mailbox.account(id).unwrap().ews.is_some());
    // Nothing is written for it to send: a message from it stays as it is being written.
    s.run("mail.new", &json!({"account": id, "to": "bob@example.org", "subject": "Hello"})).unwrap();
    let refused = said(s.run("compose.send", &json!({})));
    assert!(refused.contains(SWITCHED_OFF), "{refused}");
    assert!(s.compose.is_some(), "the message stays as it is being written");
}

#[test]
fn a_mailbox_with_an_exchange_account_round_trips_through_save_and_load() {
    let (mb, id) = mailbox_with_exchange();
    let tmp = Temp::new("round-trip");
    let path = tmp.file();
    crate::store::save(&path, &mb).unwrap();
    // The file says what the account is, and the server is its address, name and fingerprint alone.
    let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
    assert!(text.contains(r#""kind":"exchange""#) && text.contains("Exchange.asmx"), "{text}");
    let opened = crate::store::open(&path).unwrap();
    let a = opened.mailbox.account(id).unwrap();
    assert_eq!(a.kind, AccountKind::Exchange);
    assert_eq!(a.ews, mb.account(id).unwrap().ews);
    assert_eq!(a.ews.as_ref().map(|e| (e.username.as_str(), e.trusted.len())), Some(("CORP\\ada", 64)));
    assert_eq!(opened.mailbox.accounts, mb.accounts);
    // Saved again as it is, nothing is written.
    assert!(crate::store::save(&path, &opened.mailbox).unwrap().unchanged);
    // It opens with the switch off and with it on, with its mail either way.
    for on in [false, true] {
        let mut s = session(on);
        s.mailbox = crate::store::open(&path).unwrap().mailbox;
        let inbox = s.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap();
        assert_eq!(s.mailbox.count(inbox), 1);
    }
}

#[test]
fn an_account_of_a_kind_this_version_does_not_know_still_loads_beside_an_exchange_one() {
    // What an older build makes of an Exchange server account is an `Unknown` one: it reads who it
    // is and keeps the rest as the file had it. Here, the same reading of a kind nobody has, in
    // a file that holds an Exchange account too: both load, and both are written back as they were.
    let (mut mb, exchange_id) = mailbox_with_exchange();
    let newer = mb.add_account(Account { email: "ada@newer.example".into(), ..Default::default() });
    let mut file = serde_json::to_value(&mb).unwrap();
    let written = json!({"id": newer, "name": "Newer", "email": "ada@newer.example", "kind": "fromANewerVersion", "settings": {"x": [1, 2]}});
    *file["accounts"].as_array_mut().unwrap().iter_mut().find(|a| a["id"] == newer).unwrap() = written.clone();
    let loaded: Mailbox = serde_json::from_value(file).unwrap();
    assert_eq!(loaded.account(newer).map(|a| a.kind), Some(AccountKind::Unknown));
    assert_eq!(loaded.account(exchange_id).map(|a| a.kind), Some(AccountKind::Exchange));
    let again = serde_json::to_value(&loaded).unwrap();
    assert_eq!(again["accounts"].as_array().unwrap().iter().find(|a| a["id"] == newer), Some(&written));
    // And a build that doesn't know `exchange` would read the same account as one it doesn't: the
    // kind is spelled the way a later reader can skip.
    assert_eq!(serde_json::to_value(AccountKind::Exchange).unwrap(), "exchange");
    let unknown: AccountKind = serde_json::from_value(json!("someFutureKind")).unwrap();
    assert_eq!(unknown, AccountKind::Unknown);
}

/// Once the server's folders are here, the ids are one server's and one mailbox's: the address, the
/// server and the name that signs in are fixed, and the rest (a trusted certificate, a display name)
/// is not. The state the server gave is no setting, and an edit leaves it.
#[test]
fn an_exchange_accounts_server_address_and_user_name_are_fixed_once_its_mail_is_here() {
    let mut s = session(true);
    let id = s.run("account.add", &exchange("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    s.mailbox.account_mut(id).unwrap().ews.as_mut().unwrap().hierarchy = "HIER".into();
    let edits = [
        json!({"id": id, "ews": {"url": "https://other.example.org/EWS/Exchange.asmx"}}),
        json!({"id": id, "ews": {"username": "CORP\\bo"}}),
        json!({"id": id, "email": "bo@example.org"}),
    ];
    // Before any folder is the server's, they can all be changed (and changed back).
    for edit in &edits {
        assert!(s.run("account.edit", edit).is_ok(), "{edit}");
        assert!(s.run("account.edit", &json!({"id": id, "email": "ada@example.org", "ews": {"url": URL, "username": "CORP\\ada"}})).is_ok());
    }
    let inbox = s.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "F1".into(), ..Default::default() });
    for edit in &edits {
        let why = said(s.run("account.edit", edit));
        assert!(why.contains("can't be changed once its mail is here"), "{edit}: {why}");
    }
    // The same user name, said another way (the e-mail address when none is given), is the same.
    assert!(s.run("account.edit", &json!({"id": id, "ews": {"username": "CORP\\ADA "}})).is_ok(), "case and spaces make no other user");
    assert!(s.run("account.edit", &json!({"id": id, "ews": {"trusted": "cd".repeat(32)}, "displayName": "Ada"})).is_ok());
    let ews = s.mailbox.account(id).unwrap().ews.clone().unwrap();
    assert_eq!((ews.url.as_str(), ews.trusted.len(), ews.hierarchy.as_str()), (URL, 64, "HIER"));
}

/// Nothing is queued in an Exchange server account's Outbox until its sending is built, with the
/// switch on as with it off.
#[test]
fn nothing_is_queued_for_an_exchange_account_with_the_switch_on() {
    let mut s = session(true);
    let id = s.run("account.add", &exchange("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    s.run("mail.new", &json!({"account": id, "to": "bob@example.org", "subject": "Hello"})).unwrap();
    let refused = said(s.run("compose.send", &json!({})));
    assert!(refused.contains(crate::cmd::ews_mail::SENDING_NOT_BUILT) && refused.contains("send this message from another account"), "{refused}");
    assert!(s.compose.is_some());
    let outbox = s.mailbox.folder_of_kind(id, FolderKind::Outbox).unwrap();
    assert_eq!(s.mailbox.messages_in(outbox).count(), 0);
}

/// A result is held to the caps before any of it is used: a result with too much in it is refused
/// by `sync.applyEws` with the reason, and not with "not built".
#[test]
fn a_result_with_too_much_in_it_is_refused_before_anything_else_is_asked() {
    let mut s = session(true);
    let id = s.run("account.add", &exchange("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    let states: Vec<Value> = (0..=mailcraft_model::sync::MAX_EWS_STATES).map(|i| json!({"folder": format!("F{i}"), "state": "S"})).collect();
    let too_many = json!({"result": {"account": id, "email": "ada@example.org", "states": states}});
    assert!(said(s.run("sync.applyEws", &too_many)).contains("more in it than is taken in"));
    let long = "x".repeat(mailcraft_model::sync::MAX_EWS_STATE + 1);
    let long_state = json!({"result": {"account": id, "email": "ada@example.org", "hierarchy": long}});
    assert!(said(s.run("sync.applyEws", &long_state)).contains("folder state that can't be one"));
    let sound = json!({"result": {"account": id, "email": "ada@example.org", "hierarchy": "H", "states": [{"folder": "F1", "state": "S"}]}});
    assert_eq!(said(s.run("sync.applyEws", &sound)), NOT_BUILT);
}
