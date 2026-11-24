// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The whole way of an IMAP account's Automatic Replies kept in a Sieve script (#9999): the
//! window asks, this host's job speaks ManageSieve through `mailcraft-net` to the fake server of
//! its tests (`crates/net/src/fake_sieve.rs`, on loopback), and the engine takes in what came
//! back. Nothing here leaves this machine, and every address and word is invented.
//!
//! The host's job runs on the test's own thread, so that each step is done when the next begins.

use std::sync::mpsc;

use mailcraft_engine::Session;
use mailcraft_engine::sieve::{SCRIPT_NAME, digest, read_vacation};
use mailcraft_model::auto_replies::AutoReplyStatus;
use mailcraft_model::{Account, AccountKind, Id, Mailbox, Security, Server};
use mailcraft_ui_egui::sieve_replies::SieveJob;
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::json;

use crate::fake::Fake;
use crate::fake_sieve::{FakeSieve, Opening, SieveWorld};

/// Monday 2026-11-23, 09:00 UTC.
const NOW: i64 = 1_795_424_400;
const PASSWORD: &str = "an invented password";

/// A window with one IMAP account whose ManageSieve server is `fake`, and a host that runs the
/// job at once.
fn window(fake: &FakeSieve) -> (MailApp, Id) {
    let mut mailbox = Mailbox::with_local_account("", "");
    let account = Account {
        id: 0,
        name: "Ada".into(),
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: 1, security: Security::None }),
        username: "ada".into(),
        sieve_port: fake.port,
        ..Default::default()
    };
    let id = mailbox.add_account(account);
    let services = Services {
        sieve: Some(Box::new(|job: SieveJob| {
            let (tx, rx) = mpsc::channel();
            tx.send(crate::sieve_job::sieve_job(job)).unwrap();
            rx
        })),
        ..Default::default()
    };
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), services);
    app.autosave = false;
    app.stand_in_password(id, PASSWORD);
    (app, id)
}

fn world() -> SieveWorld {
    SieveWorld { user: "ada".into(), password: PASSWORD.into(), ..Default::default() }
}

/// Ask for the replies, as File › Info and the dialog do, and take in the answer.
fn read(app: &mut MailApp, account: Id) {
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    app.poll_sieve();
}

/// Try Again, as File › Info's link and the dialog's button do, and take in the answer.
fn try_again(app: &mut MailApp, account: Id) {
    app.sieve_try_again(account);
    app.poll_sieve();
}

#[test]
fn replies_are_read_written_and_read_back_through_the_fake_server() {
    let fake = Fake::sieve(world());
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    assert_eq!(app.sieve_line(account), "Automatic replies: off");
    assert_eq!(fake.world().sign_ins, 1);

    app.execute(
        "account.setAutomaticReplies",
        json!({"account": account, "status": "alwaysEnabled", "internal": "Back on Monday.", "external": "Away\"; discard;", "audience": "all"}),
    )
    .unwrap();
    app.poll_sieve();
    // On the server, active, and known as MailCraft's.
    let text = fake.world().script(SCRIPT_NAME).unwrap().to_string();
    assert_eq!(fake.world().active.as_deref(), Some(SCRIPT_NAME));
    assert_eq!(read_vacation(&text).unwrap().external, "Away\"; discard;");
    assert_eq!(app.session.mailbox.account(account).unwrap().sieve_written, digest(&text));
    assert_eq!(app.status_msg.as_ref().map(|(s, _)| s.as_str()), Some("Automatic replies are on"));
    assert_eq!(app.sieve_line(account), "Automatic replies: on, until you turn them off");

    // Off again: the script stays MailCraft's, without the replies.
    app.execute("account.setAutomaticReplies", json!({"account": account, "status": "disabled"})).unwrap();
    app.poll_sieve();
    let off = fake.world().script(SCRIPT_NAME).unwrap().to_string();
    assert_eq!(read_vacation(&off).unwrap().status, AutoReplyStatus::Disabled);
    assert_eq!(app.status_msg.as_ref().map(|(s, _)| s.as_str()), Some("Automatic replies are off"));
}

#[test]
fn a_script_changed_elsewhere_is_left_until_the_owner_replaces_it() {
    let theirs = "require \"fileinto\";\r\nfileinto \"Lists\";\r\n";
    let fake = Fake::sieve(SieveWorld { scripts: vec![(SCRIPT_NAME.into(), theirs.into())], active: Some(SCRIPT_NAME.into()), ..world() });
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    assert!(app.sieve_line(account).contains("changed elsewhere"), "{}", app.sieve_line(account));
    assert!(app.execute("account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled"})).is_err());
    assert_eq!(fake.world().script(SCRIPT_NAME), Some(theirs));

    // The owner replaces it.
    app.execute("account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "internal": "Away.", "replace": true})).unwrap();
    app.poll_sieve();
    let now = fake.world().script(SCRIPT_NAME).unwrap().to_string();
    assert_eq!(read_vacation(&now).unwrap().internal, "Away.");
    assert_eq!(app.sieve_line(account), "Automatic replies: on, until you turn them off");
}

#[test]
fn the_owners_own_script_stays_active_and_is_named() {
    let fake = Fake::sieve(SieveWorld { scripts: vec![("roundcube".into(), "keep;".into())], active: Some("roundcube".into()), ..world() });
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    app.execute("account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "internal": "Away."})).unwrap();
    app.poll_sieve();
    assert_eq!(fake.world().active.as_deref(), Some("roundcube"));
    assert!(fake.world().script(SCRIPT_NAME).is_some());
    let said = app.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("another Sieve script"), "{said}");
    assert!(app.sieve_line(account).contains("\"roundcube\" runs instead"), "{}", app.sieve_line(account));
}

#[test]
fn a_server_without_managesieve_or_vacation_is_said_and_not_asked_again() {
    // Nothing listens on the port: not available, and not asked again in the session.
    // (A port nothing listens on: a listener's, closed at once.)
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let fake = Fake::sieve(world());
    let (mut app, account) = window(&fake);
    app.session.mailbox.account_mut(account).unwrap().sieve_port = port;
    read(&mut app, account);
    assert_eq!(
        app.sieve_line(account),
        format!("Automatic replies: not available (nothing answers at 127.0.0.1:{port} (the server refused the connection))")
    );
    read(&mut app, account);
    assert_eq!(fake.world().connections, 0);

    // Something else answers there (an IMAP server): said, with where; asked again at Try Again.
    let fake = Fake::sieve(SieveWorld { opening: Opening::Says(b"* OK [CAPABILITY IMAP4rev1] ready\r\n".to_vec()), ..world() });
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    let at = format!("127.0.0.1:{}", fake.port);
    assert_eq!(
        app.sieve_line(account),
        format!("Automatic replies: not available (something answers at {at}, but it isn't a ManageSieve server: it looks like an IMAP server)")
    );
    read(&mut app, account);
    assert_eq!(fake.world().connections, 1);
    fake.world().opening = Opening::Greets;
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    app.poll_sieve();
    assert_eq!(fake.world().connections, 1);
    // (What File › Info's Try again does.)
    try_again(&mut app, account);
    let seen = fake.world();
    assert_eq!((seen.connections, seen.sign_ins), (2, 1));
    drop(seen);
    assert_eq!(app.sieve_line(account), "Automatic replies: off");

    // A ManageSieve server without STARTTLS, where it is asked for: refused before the sign-in.
    let fake = Fake::sieve(world());
    let (mut app, account) = window(&fake);
    app.session.mailbox.account_mut(account).unwrap().incoming = Some(Server { host: "127.0.0.1".into(), port: 1, security: Security::StartTls });
    read(&mut app, account);
    assert_eq!(
        app.sieve_line(account),
        format!("Automatic replies: not available (the ManageSieve server at 127.0.0.1:{} doesn't offer STARTTLS)", fake.port)
    );
    assert_eq!(fake.world().sign_ins, 0);

    // Sieve without vacation.
    let fake = Fake::sieve(SieveWorld { extensions: "fileinto".into(), ..world() });
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    assert!(app.sieve_line(account).contains("can't send them"), "{}", app.sieve_line(account));
    assert!(app.execute("account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled"})).is_err());
}

#[test]
fn a_wrong_password_is_said_and_nothing_is_written() {
    let fake = Fake::sieve(SieveWorld { password: "another password".into(), ..world() });
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    assert!(app.sieve_line(account).contains("didn't take your sign-in"), "{}", app.sieve_line(account));
    assert!(fake.world().scripts.is_empty());
}

/// A folder `Lists` under the Inbox that the server has, and a rule moving mail from lists there.
fn lists_rule(app: &mut MailApp, account: Id) -> Id {
    let inbox = app.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    app.session.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let lists = app.execute("folder.new", json!({"name": "Lists", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    app.session.mailbox.folder_mut(lists).unwrap().remote = "INBOX/Lists".into();
    let made = app
        .execute(
            "rules.add",
            json!({"name": "Lists", "account": account, "conditions": [{"type": "fromContains", "value": "list"}, {"type": "subjectContains", "value": "[list\"; discard;"}], "actions": [{"type": "moveTo", "value": lists}]}),
        )
        .unwrap();
    made["id"].as_u64().unwrap()
}

#[test]
fn a_rule_kept_on_the_server_goes_into_mailcrafts_script_after_the_replies() {
    let fake = Fake::sieve(world());
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    let rule = lists_rule(&mut app, account);
    app.poll_sieve();
    let text = fake.world().script(SCRIPT_NAME).unwrap().to_string();
    assert!(text.contains("fileinto \"INBOX/Lists\""), "{text}");
    assert!(text.contains("address :all :contains \"from\" [\"list\"]"), "{text}");
    assert!(text.contains("header :contains \"subject\" [\"[list\\\"; discard;\"]"), "{text}");
    assert_eq!(fake.world().active.as_deref(), Some(SCRIPT_NAME));
    assert!(app.session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().on_server());
    assert_eq!(app.status_msg.as_ref().map(|(s, _)| s.as_str()), Some("Your rules are kept on the server"));

    // The replies changed: the rule stays in the script, after them.
    app.execute("account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "internal": "Away."})).unwrap();
    app.poll_sieve();
    let both = fake.world().script(SCRIPT_NAME).unwrap().to_string();
    assert!(both.find("vacation ").unwrap() < both.find(&format!("# rule {rule}")).unwrap(), "{both}");
    assert!(app.session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().on_server());

    // Deleted here: it leaves the script, and the replies stay.
    app.execute("rules.remove", json!({"id": rule})).unwrap();
    app.poll_sieve();
    let after = fake.world().script(SCRIPT_NAME).unwrap().to_string();
    assert!(!after.contains("# rule") && after.contains("vacation "), "{after}");
}

#[test]
fn rules_run_here_while_the_owners_own_script_runs_on_the_server() {
    let fake = Fake::sieve(SieveWorld { scripts: vec![("roundcube".into(), "keep;".into())], active: Some("roundcube".into()), ..world() });
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    let rule = lists_rule(&mut app, account);
    app.poll_sieve();
    assert_eq!(fake.world().active.as_deref(), Some("roundcube"));
    assert!(fake.world().script(SCRIPT_NAME).is_some());
    assert!(!app.session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().on_server());
    let said = app.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("they run on this computer"), "{said}");
}

#[test]
fn the_owners_own_scripts_rules_are_shown_and_never_written() {
    let theirs = "require [\"fileinto\", \"vacation\"];\r\nif address :contains \"from\" \"shop.example\" { fileinto \"INBOX/Shop\"; stop; }\r\nif true { vacation \"Away.\"; }\r\n";
    let fake = Fake::sieve(SieveWorld { scripts: vec![("roundcube".into(), theirs.into())], active: Some("roundcube".into()), ..world() });
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    let listed = app.execute("rules.list", json!({"account": account})).unwrap();
    assert_eq!(listed["elsewhere"][0]["script"], "roundcube");
    assert_eq!(listed["elsewhere"][0]["rules"].as_array().map(Vec::len), Some(1));
    let id = listed["elsewhere"][0]["rules"][0]["id"].as_u64().unwrap();
    assert!(app.execute("rules.remove", json!({"id": id})).is_err());
    assert!(app.sieve_line(account).contains("set by your Sieve script \"roundcube\""), "{}", app.sieve_line(account));
    let world = fake.world();
    assert_eq!(world.script("roundcube"), Some(theirs));
    assert!(!world.heard("PUTSCRIPT") && !world.heard("SETACTIVE"));
}
