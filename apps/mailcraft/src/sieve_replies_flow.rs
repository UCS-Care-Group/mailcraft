// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The whole way of an IMAP account's Automatic Replies kept in a Sieve script (#2057): the
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
fn a_rule_changed_is_written_again_in_its_place_and_one_sieve_cant_say_leaves_the_script() {
    let fake = Fake::sieve(world());
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    let rule = lists_rule(&mut app, account);
    let lists = app.session.mailbox.rules.iter().find(|r| r.id == rule).and_then(|r| r.actions.first().cloned());
    let Some(mailcraft_model::Action::MoveTo(lists)) = lists else { panic!("the rule moves mail") };
    app.poll_sieve();
    // Change Rule: other words, the same folder; the script on the server has them under its id.
    let changed = json!({"id": rule, "name": "Digests", "account": account, "conditions": [{"type": "subjectContains", "value": "digest"}],
        "actions": [{"type": "moveTo", "value": lists}]});
    assert_eq!(app.execute("rules.edit", changed).unwrap()["keep"], "server");
    app.poll_sieve();
    let text = fake.world().script(SCRIPT_NAME).unwrap().to_string();
    assert!(text.contains(&format!("# rule {rule}\r\nif header :contains \"subject\" [\"digest\"]")), "{text}");
    assert!(!text.contains("\"list\""), "{text}");
    assert!(app.session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().on_server());
    // A part Sieve can't say: it runs here, and leaves the script.
    let here = json!({"id": rule, "name": "Digests", "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "moveTo", "value": lists}]});
    assert_eq!(app.execute("rules.edit", here).unwrap()["keep"], "here");
    app.poll_sieve();
    let after = fake.world().script(SCRIPT_NAME).unwrap().to_string();
    assert!(!after.contains("# rule"), "{after}");
    assert!(!app.session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().on_server());
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

#[test]
fn a_sieve_rule_goes_into_mailcrafts_script_with_its_require_and_a_refusal_leaves_the_server_as_it_was() {
    let fake = Fake::sieve(SieveWorld { refuse_scripts_with: Some("refuse-me".into()), ..world() });
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    let made = app
        .execute("rules.addSieve", json!({"account": account, "name": "Too big", "sieve": "if size :over 9M {\n  reject \"too big\";\n} # end"}))
        .unwrap();
    let rule = made["id"].as_u64().unwrap();
    app.poll_sieve();
    let text = fake.world().script(SCRIPT_NAME).unwrap().to_string();
    assert!(text.contains("require [\"reject\"];\r\n"), "{text}");
    assert!(
        text.ends_with(&format!("# rule {rule}\r\n# Sieve rule: Too big\r\nif size :over 9M {{\r\n  reject \"too big\";\r\n}} # end\r\n")),
        "{text}"
    );
    assert_eq!(fake.world().active.as_deref(), Some(SCRIPT_NAME));
    assert!(app.session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().on_server());
    // The server refuses the next script: what it has stays, and the window says why.
    app.execute("rules.editSieve", json!({"id": rule, "sieve": "if header :contains \"subject\" \"refuse-me\" { discard; }"})).unwrap();
    app.poll_sieve();
    assert_eq!(fake.world().script(SCRIPT_NAME), Some(text.as_str()));
    assert!(app.sieve_line(account).contains("unknown command"), "{}", app.sieve_line(account));
}

#[test]
fn the_editors_check_reaches_the_server_and_brings_its_words_back_unwritten() {
    use mailcraft_ui_egui::sieve_replies::ServerCheck;
    let fake = Fake::sieve(SieveWorld {
        refuse_scripts_with: Some("bogus".into()),
        refusal: "line 4: error: unknown command 'bogus'.\r\nerror: validation failed.".into(),
        ..world()
    });
    let (mut app, account) = window(&fake);
    let ticket = app.sieve_check_start(account, "require \"fileinto\";\r\nkeep;\r\n\r\nbogus;\r\n".into()).unwrap();
    let answer = app.sieve_check_answer(ticket).expect("the host answered at once");
    assert_eq!(answer.checked, Some(ServerCheck::Errors("line 4: error: unknown command 'bogus'.\nerror: validation failed.".into())));
    let ticket = app.sieve_check_start(account, "keep;\r\n".into()).unwrap();
    assert_eq!(app.sieve_check_answer(ticket).unwrap().checked, Some(ServerCheck::Fine));
    // Asked once each, and nothing kept on the server.
    let world = fake.world();
    assert_eq!(world.log.iter().filter(|l| l.starts_with("CHECKSCRIPT")).count(), 2);
    assert!(world.scripts.is_empty() && !world.heard("PUTSCRIPT"));
}

/// The owner's own script, as a webmail wrote it: CRLF, comments, an extension MailCraft doesn't
/// know (which the fake server offers).
const OWN: &str = "# rule:[Lists]\r\nrequire [\"fileinto\", \"x-unknown\"];\r\n/* kept as is */\r\nif header :contains \"subject\" \"[list]\" {\r\n  fileinto \"INBOX/Lists\";\r\n}\r\n";

fn own_world() -> SieveWorld {
    SieveWorld {
        extensions: "fileinto vacation x-unknown".into(),
        scripts: vec![("roundcube".into(), OWN.into())],
        active: Some("roundcube".into()),
        ..world()
    }
}

#[test]
fn the_owners_own_script_is_written_back_as_typed_and_stays_active() {
    let fake = Fake::sieve(own_world());
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    let got = app.execute("sieve.ownScript", json!({"account": account})).unwrap();
    assert_eq!(got["script"]["text"], OWN);
    let edited = OWN.replace("INBOX/Lists", "INBOX/Old lists");
    app.execute("sieve.putOwnScript", json!({"account": account, "text": edited, "digest": got["script"]["digest"]})).unwrap();
    app.poll_sieve();
    {
        let world = fake.world();
        assert_eq!(world.script("roundcube").map(str::as_bytes), Some(edited.as_bytes()));
        assert_eq!(world.active.as_deref(), Some("roundcube"));
        assert_eq!(world.script(SCRIPT_NAME), None);
        assert!(!world.heard("SETACTIVE") && !world.heard("DELETESCRIPT") && !world.heard("RENAMESCRIPT"));
    }
    assert_eq!(app.status_msg.as_ref().map(|(s, _)| s.as_str()), Some("Your Sieve script is saved on the server"));
    // Read again with the write: the rules shown follow the new text; MailCraft's digest is untouched.
    assert_eq!(app.execute("sieve.ownScript", json!({"account": account})).unwrap()["script"]["text"], edited.as_str());
    assert_eq!(app.session.mailbox.account(account).unwrap().sieve_written, "");
}

#[test]
fn the_owners_own_script_changed_on_the_server_meanwhile_is_left_as_it_is() {
    let fake = Fake::sieve(own_world());
    let (mut app, account) = window(&fake);
    read(&mut app, account);
    let digest_read = digest(OWN);
    // Changed in webmail after MailCraft read it.
    let webmail = format!("{OWN}# rule:[Later]\r\nif size :over 1M {{ discard; }}\r\n");
    {
        let mut world = fake.world();
        world.scripts = vec![("roundcube".into(), webmail.clone())];
    }
    app.execute("sieve.putOwnScript", json!({"account": account, "text": "keep;\r\n", "digest": digest_read})).unwrap();
    app.poll_sieve();
    assert_eq!(fake.world().script("roundcube"), Some(webmail.as_str()));
    let said = app.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("changed on the server since you opened it"), "{said}");
    // What the server has now is what is known, and the stale digest is refused at once.
    assert_eq!(app.execute("sieve.ownScript", json!({"account": account})).unwrap()["script"]["text"], webmail.as_str());
    assert!(app.execute("sieve.putOwnScript", json!({"account": account, "text": "keep;\r\n", "digest": digest_read})).is_err());
    assert!(!fake.world().heard("PUTSCRIPT"));
}

/// A filter's settings in a script of a webmail of the RainLoop kind: its comment, base64 in lines
/// of 74, as that webmail writes it (synthetic).
fn filter_header(id: &str, json: &str) -> String {
    let b64 = mailcraft_engine::cmd::base64_encode(json.as_bytes());
    let lines: Vec<String> = b64.as_bytes().chunks(74).map(|c| String::from_utf8(c.to_vec()).unwrap()).collect();
    format!("/*\nBEGIN:FILTER:{id}\nBEGIN:HEADER\n{}\nEND:HEADER\n*/", lines.join("\n"))
}

const SPAM_FILTER: &str = r#"{"ID":"0000017000590660000","Enabled":true,"Name":"Spam","Conditions":[{"Field":"Header","Type":"EqualTo","Value":"YES","ValueSecond":"X-Spam-Flag"}],"ConditionsType":"Any","ActionType":"MoveTo","ActionValue":"Junk","ActionValueSecond":"","ActionValueThird":"","ActionValueFourth":"","Keep":true,"Stop":true,"MarkAsRead":false}"#;
const LIST_FILTER: &str = r#"{"ID":"6orktqi8w2n5v0c3b7x1z9q4m8k2j6hd","Enabled":true,"Name":"ML-list","Conditions":[{"Field":"Recipient","Type":"Contains","Value":"list@lists.example.org","ValueSecond":""}],"ConditionsType":"Any","ActionType":"MoveTo","ActionValue":"Mailing-Lists/list","ActionValueSecond":"","ActionValueThird":"","ActionValueFourth":"","Keep":true,"Stop":true,"MarkAsRead":false}"#;
const WEBMAIL_HEAD: &str =
    "require [\"fileinto\",\"body\"];\n\n# This is SnappyMail sieve script.\n# Please don't change anything here.\n# RAINLOOP:SIEVE\n\n";

fn spam_filter() -> String {
    format!(
        "{}\n\nif header :is [\"X-Spam-Flag\"] \"YES\"\n{{\n    fileinto \"Junk\";\n    stop;\n}}\n\n/* END:FILTER */",
        filter_header("0000017000590660000", SPAM_FILTER)
    )
}

fn list_filter() -> String {
    let sieve = "if header :contains [\"To\", \"CC\"] \"list@lists.example.org\"\n{\n    fileinto \"Mailing-Lists/list\";\n    stop;\n}";
    format!("{}\n\n{sieve}\n\n/* END:FILTER */", filter_header("6orktqi8w2n5v0c3b7x1z9q4m8k2j6hd", LIST_FILTER))
}

fn webmail_script() -> String {
    format!("{WEBMAIL_HEAD}{}\n\n{}\n", spam_filter(), list_filter())
}

/// A window whose account has the folders Junk and Mailing-Lists/list, and whose server runs the
/// webmail's script `snappymail`, read: the window, the account, Junk, and the filters' rule ids.
fn webmail_window(fake: &FakeSieve) -> (MailApp, Id, Id, Vec<Id>) {
    let (mut app, account) = window(fake);
    let inbox = app.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    app.session.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let junk = app.execute("folder.new", json!({"name": "Junk", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    app.session.mailbox.folder_mut(junk).unwrap().remote = "Junk".into();
    let list = app.execute("folder.new", json!({"name": "list", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    app.session.mailbox.folder_mut(list).unwrap().remote = "Mailing-Lists/list".into();
    read(&mut app, account);
    let rules: Vec<Id> = app.session.sieve[&account].elsewhere.as_ref().unwrap().rules.iter().map(|r| r.id).collect();
    (app, account, junk, rules)
}

fn webmail_world() -> SieveWorld {
    SieveWorld {
        extensions: "fileinto body vacation".into(),
        scripts: vec![("snappymail".into(), webmail_script())],
        active: Some("snappymail".into()),
        ..world()
    }
}

#[test]
fn a_webmails_filter_is_changed_in_its_own_shape_and_the_rest_of_the_script_stays() {
    let fake = Fake::sieve(webmail_world());
    let (mut app, account, junk, rules) = webmail_window(&fake);
    assert_eq!(rules.len(), 2);
    let change = json!({"id": rules[1], "name": "Lists", "conditions": [{"type": "toContains", "value": "list@lists.example.org"}],
        "actions": [{"type": "moveTo", "value": junk}], "stop": true});
    app.execute("rules.edit", change).unwrap();
    app.poll_sieve();
    let json = LIST_FILTER.replace("\"ML-list\"", "\"Lists\"").replace("\"Mailing-Lists/list\"", "\"Junk\"");
    let changed = list_filter()
        .replace(&filter_header("6orktqi8w2n5v0c3b7x1z9q4m8k2j6hd", LIST_FILTER), &filter_header("6orktqi8w2n5v0c3b7x1z9q4m8k2j6hd", &json))
        .replace("fileinto \"Mailing-Lists/list\"", "fileinto \"Junk\"");
    let expected = format!("{WEBMAIL_HEAD}{}\n\n{changed}\n", spam_filter());
    {
        let world = fake.world();
        assert_eq!(world.script("snappymail").map(str::as_bytes), Some(expected.as_bytes()));
        assert_eq!(world.active.as_deref(), Some("snappymail"));
        assert_eq!(world.script(SCRIPT_NAME), None);
        assert!(!world.heard("SETACTIVE") && !world.heard("DELETESCRIPT") && !world.heard("RENAMESCRIPT"));
    }
    assert_eq!(app.status_msg.as_ref().map(|(s, _)| s.as_str()), Some("Your Sieve script is saved on the server"));
    // Read again with the write: the rule shown follows; MailCraft's own digest is untouched.
    let shown = app.session.sieve[&account].elsewhere.clone().unwrap();
    assert_eq!(shown.text, expected);
    assert_eq!(shown.rules[1].name, "Lists");
    assert_eq!(app.session.mailbox.account(account).unwrap().sieve_written, "");

    // Switched off, then deleted: each written the same way.
    app.execute("rules.enable", json!({"id": shown.rules[0].id, "value": false})).unwrap();
    app.poll_sieve();
    let off = fake.world().script("snappymail").map(str::to_string).unwrap_or_default();
    // Its settings say so, its Sieve is left out, its comments stay (not seen yet: `off_block`).
    let spam_off = filter_header("0000017000590660000", &SPAM_FILTER.replace("\"Enabled\":true", "\"Enabled\":false"));
    assert_eq!(off, format!("{WEBMAIL_HEAD}{spam_off}\n\n/* END:FILTER */\n\n{changed}\n"));
    let rules: Vec<Id> = app.session.sieve[&account].elsewhere.as_ref().unwrap().rules.iter().map(|r| r.id).collect();
    app.execute("rules.remove", json!({"id": rules[0]})).unwrap();
    app.poll_sieve();
    assert_eq!(fake.world().script("snappymail"), Some(format!("{WEBMAIL_HEAD}{changed}\n").as_str()));
}

#[test]
fn a_webmails_filter_changed_on_the_server_meanwhile_is_left_as_it_is() {
    let fake = Fake::sieve(webmail_world());
    let (mut app, _, junk, rules) = webmail_window(&fake);
    // Changed in webmail after MailCraft read it.
    let webmail = webmail_script().replace("list@lists.example.org", "other@lists.example.org");
    {
        let mut world = fake.world();
        world.scripts = vec![("snappymail".into(), webmail.clone())];
    }
    let change = json!({"id": rules[0], "name": "Junk mail", "conditions": [{"type": "toContains", "value": "x@example.org"}],
        "actions": [{"type": "moveTo", "value": junk}]});
    app.execute("rules.edit", change).unwrap();
    app.poll_sieve();
    assert_eq!(fake.world().script("snappymail"), Some(webmail.as_str()));
    let said = app.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("changed on the server since you opened it"), "{said}");
    assert!(!fake.world().heard("PUTSCRIPT"));
}
