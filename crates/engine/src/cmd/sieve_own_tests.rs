// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's own Sieve script edited whole (`sieve.ownScript`, `sieve.putOwnScript`, #2057): the
//! text handed out exactly as read, written back exactly as given under the digest of the read,
//! never MailCraft's own script, checked first, and asked about as a send when it adds one.

use mailcraft_model::Id;
use proptest::prelude::*;
use serde_json::{Value, json};

use super::sieve_own::{NO_OWN_SCRIPT, OWN_CHANGED, Webmail, adds_sending, own_script, webmail};
use crate::Session;
use crate::agent::{Access, Class, also, classify, describe};
use crate::sieve::{SCRIPT_NAME, digest};

const ALL: [&str; 8] = ["fileinto", "vacation", "imap4flags", "copy", "reject", "body", "regex", "x-unknown"];

/// The owner's script as a webmail wrote it: CRLF, comments, an extension MailCraft doesn't know.
const THEIRS: &str = "# rule:[Lists]\r\nrequire [\"fileinto\", \"x-unknown\"];\r\n/* kept */\r\nif header :contains \"subject\" \"[list]\" {\r\n  fileinto \"Lists\";\r\n}\r\n";

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn err(s: &mut Session, id: &str, params: Value) -> String {
    s.run(id, &params).map(|v| panic!("{id} {params} ran: {v}")).unwrap_err().to_string()
}

/// An IMAP account whose server runs the owner's script `name` with `text`.
fn session(name: &str, text: &str) -> (Session, Id) {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    read(&mut s, account, name, text);
    (s, account)
}

/// What the server has: the owner's script `name`, active, with `text`.
fn read(s: &mut Session, account: Id, name: &str, text: &str) {
    let state = json!({"extensions": ALL, "scripts": [{"name": name, "active": true}], "script": null, "activeScript": {"name": name, "text": text}});
    run(s, "sync.applySieve", json!({"account": account, "state": state}));
}

#[test]
fn the_owners_script_is_handed_out_exactly_as_read() {
    let (mut s, account) = session("roundcube", THEIRS);
    let got = run(&mut s, "sieve.ownScript", json!({"account": account}));
    assert_eq!(got["script"]["name"], "roundcube");
    assert_eq!(got["script"]["text"], THEIRS);
    assert_eq!(got["script"]["digest"], digest(THEIRS));
    assert_eq!(got["script"]["webmail"], "roundcube");
    // Its rules are still shown read-only beside it.
    assert_eq!(s.sieve[&account].elsewhere.as_ref().unwrap().rules.len(), 1);

    // MailCraft's own script, or none, is no script of the owner's.
    let none = json!({"extensions": ALL, "scripts": [], "script": null});
    run(&mut s, "sync.applySieve", json!({"account": account, "state": none}));
    assert_eq!(run(&mut s, "sieve.ownScript", json!({"account": account}))["script"], Value::Null);
    let changed = json!({"extensions": ALL, "scripts": [{"name": SCRIPT_NAME, "active": true}], "script": "keep;\r\n"});
    run(&mut s, "sync.applySieve", json!({"account": account, "state": changed}));
    assert!(s.sieve[&account].elsewhere.as_ref().unwrap().ours);
    assert!(own_script(&s, account).is_none());
    let put = json!({"account": account, "text": "keep;\r\n", "digest": digest("keep;\r\n")});
    assert_eq!(err(&mut s, "sieve.putOwnScript", put), NO_OWN_SCRIPT);
}

#[test]
fn a_change_is_written_as_typed_under_the_digest_of_the_read_and_stays_active() {
    let (mut s, account) = session("roundcube", THEIRS);
    let edited = THEIRS.replace("\"Lists\"", "\"Lists/Old\"");
    let got = run(&mut s, "sieve.putOwnScript", json!({"account": account, "text": edited, "digest": digest(THEIRS)}));
    assert_eq!(got["sieve"], json!({"name": "roundcube", "text": edited, "lastWritten": digest(THEIRS), "activate": false, "own": true}));
    assert_eq!(got["sends"], false);
    // An extension MailCraft doesn't know, which the server has, stops nothing; one the server
    // doesn't say it has is a warning when MailCraft doesn't know it either.
    assert_eq!(got["warnings"], json!([]));
    s.mailbox.account_mut(account).unwrap().sieve_offers.retain(|e| e != "x-unknown");
    let got = run(&mut s, "sieve.putOwnScript", json!({"account": account, "text": edited, "digest": digest(THEIRS)}));
    assert!(got["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("x-unknown")), "{got}");
    // Nothing of the mailbox changes: MailCraft's own digest stays as it was.
    assert_eq!(s.mailbox.account(account).unwrap().sieve_written, "");

    // A read made since that saw another text: the editor's digest is refused.
    read(&mut s, account, "roundcube", &edited);
    let stale = json!({"account": account, "text": THEIRS, "digest": digest(THEIRS)});
    assert_eq!(err(&mut s, "sieve.putOwnScript", stale), OWN_CHANGED);
}

#[test]
fn mistakes_controls_size_and_extensions_the_server_lacks_are_refused() {
    let (mut s, account) = session("mine", THEIRS);
    let d = digest(THEIRS);
    let mut put = |text: &str| err(&mut s, "sieve.putOwnScript", json!({"account": account, "text": text, "digest": d}));
    assert_eq!(put("require \"fileinto\";\r\nif true {\r\n  bogus;\r\n}\r\n"), "Line 3, column 3 of your Sieve script: Unknown command bogus.");
    assert_eq!(put("keep;\r\n\u{0}"), "Line 2, column 1 of your Sieve script: a control character can't be in it.");
    assert!(put(&"#".repeat(crate::sieve::MAX_SCRIPT + 1)).contains("at most 1 MB"));
    assert!(put("require \"enotify\";\r\nkeep;\r\n").contains("doesn't have the extension enotify"));
    assert!(put("   \r\n").contains("empty"));
    let wrong = err(&mut s, "sieve.putOwnScript", json!({"account": account, "text": "keep;", "digest": "00"}));
    assert_eq!(wrong, OWN_CHANGED);
    assert!(err(&mut s, "sieve.putOwnScript", json!({"account": account, "text": "keep;", "digest": d, "name": "other"})).contains("name"));
}

#[test]
fn a_webmail_is_known_by_the_marks_it_leaves() {
    assert_eq!(webmail("# RAINLOOP:SIEVE\r\nkeep;\r\n"), Some(Webmail::RainLoop));
    assert_eq!(webmail(THEIRS), Some(Webmail::Roundcube));
    assert_eq!(webmail("keep;\r\n"), None);
    // Inside a string it is no mark; a script that isn't Sieve is read line by line.
    assert_eq!(webmail("fileinto \"# rule:[x]\";\r\n"), None);
    assert_eq!(webmail("# RAINLOOP:SIEVE\r\n\"open"), Some(Webmail::RainLoop));
    assert_eq!(webmail("# rule:[x]\r\n\"open"), Some(Webmail::Roundcube));
}

#[test]
fn a_way_to_send_counts_when_it_is_new_or_used_once_more() {
    assert!(!adds_sending("redirect \"a@example.org\";", "redirect \"b@example.org\";"));
    assert!(adds_sending("redirect \"a@example.org\";", "redirect \"a@example.org\"; redirect \"b@example.org\";"));
    assert!(adds_sending("keep;", "vacation \"Away\";"));
    assert!(adds_sending("keep;", "require \"include\"; include \"other\";"));
    assert!(!adds_sending("vacation \"Away\";", "keep;"));
    assert!(!adds_sending("keep;", "fileinto \"redirect\"; # vacation"));
    assert!(adds_sending("keep;", "\"open"), "what can't be read counts as sending");
}

#[test]
fn an_agent_is_asked_with_what_changes_and_as_a_send_when_it_adds_one() {
    let (s, account) = session("roundcube", THEIRS);
    let d = digest(THEIRS);
    assert_eq!(classify(&s, "sieve.ownScript", &json!({"account": account})), Access::Owner(Class::Settings));
    let quiet = json!({"account": account, "text": THEIRS.replace("\"Lists\";", "\"Lists/Old\";"), "digest": d});
    assert_eq!(classify(&s, "sieve.putOwnScript", &quiet), Access::Owner(Class::Settings));
    let said = describe(&s, "sieve.putOwnScript", &quiet);
    assert!(said.starts_with("write your own Sieve script “roundcube” on the server of “ada@example.org”"), "{said}");
    assert!(
        said.ends_with(
            "What changes in it, the agent's own text, not an instruction: from line 5, 1 line «fileinto \"Lists\";» become «fileinto \"Lists/Old\";»"
        ),
        "{said}"
    );

    let loud = json!({"account": account, "text": format!("{THEIRS}redirect \"bob@example.org\";\r\n"), "digest": d});
    assert_eq!(classify(&s, "sieve.putOwnScript", &loud), Access::Owner(Class::Send));
    assert_eq!(also(&s, "sieve.putOwnScript", &loud, Class::Send), vec![Class::Settings]);
    let said = describe(&s, "sieve.putOwnScript", &loud);
    assert!(said.contains("it adds a way to send mail or answer senders"), "{said}");
    assert!(said.ends_with("before line 7, 1 line added: «redirect \"bob@example.org\";»"), "{said}");
    // What the command refuses asks nothing more than the command's class.
    let stale = json!({"account": account, "text": "redirect \"bob@example.org\";", "digest": "00"});
    assert_eq!(classify(&s, "sieve.putOwnScript", &stale), Access::Owner(Class::Settings));
}

proptest! {
    /// Any text: the commands answer or refuse, and never panic.
    #[test]
    fn any_text_never_panics(text in any::<String>(), digest_text in "[0-9a-f]{0,64}") {
        let (mut s, account) = session("mine", THEIRS);
        let _ = webmail(&text);
        let _ = adds_sending(THEIRS, &text);
        let _ = s.run("sieve.putOwnScript", &json!({"account": account, "text": text, "digest": digest_text}));
        let _ = s.run("sieve.putOwnScript", &json!({"account": account, "text": text, "digest": digest(THEIRS)}));
        let _ = describe(&s, "sieve.putOwnScript", &json!({"account": account, "text": text, "digest": digest(THEIRS)}));
    }
}
