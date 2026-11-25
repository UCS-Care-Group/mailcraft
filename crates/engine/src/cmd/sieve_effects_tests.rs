// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a Sieve text can do, as an agent's approval asks it (#9999): one table classes each
//! command, read from the parsed text; what sends, hands mail to a program, isn't known or can't
//! be read asks as a send; what changes messages asks as settings and says so.

use mailcraft_model::{Id, Keep, Rule};
use serde_json::{Value, json};

use super::rules_sieve::sends;
use super::sieve_own::adds_sending;
use crate::Session;
use crate::agent::{Access, Class, classify, describe};
use crate::sieve::digest;
use crate::sieve_check::{Effect, effects};

/// What the server says its Sieve has: Dovecot's vendor extensions too.
const OFFERS: [&str; 10] = [
    "fileinto",
    "copy",
    "editheader",
    "replace",
    "enclose",
    "variables",
    "vnd.dovecot.report",
    "vnd.dovecot.pipe",
    "vnd.dovecot.filter",
    "vnd.dovecot.execute",
];

const THEIRS: &str = "require \"fileinto\";\r\nif header :contains \"subject\" \"[list]\" {\r\n  fileinto \"Lists\";\r\n}\r\n";

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// An IMAP account whose server keeps Sieve scripts with [`OFFERS`] and runs the owner's script
/// [`THEIRS`].
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let account = run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    let state = json!({"extensions": OFFERS, "scripts": [{"name": "mine", "active": true}], "script": null, "activeScript": {"name": "mine", "text": THEIRS}});
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state}));
    (s, account)
}

fn add_sieve(account: Id, sieve: &str) -> Value {
    json!({"account": account, "name": "R", "sieve": sieve})
}

#[test]
fn the_table_classes_each_command_and_what_it_doesnt_know_sends() {
    let effect = |text: &str, name: &str| effects(text).unwrap().into_iter().find(|u| u.name == name).map(|u| (u.effect, u.known));
    assert_eq!(effect("keep;", "keep"), Some((Effect::Local, true)));
    assert_eq!(effect("redirect \"a@example.org\";", "redirect"), Some((Effect::Sends, true)));
    assert_eq!(effect("report \"abuse\" \"spam\" \"abuse@example.org\";", "report"), Some((Effect::Sends, true)));
    assert_eq!(effect("pipe \"prog\";", "pipe"), Some((Effect::HandsOn, true)));
    assert_eq!(effect("if execute :pipe \"prog\" { keep; }", "execute"), Some((Effect::HandsOn, true)));
    assert_eq!(effect("filter \"prog\" [\"-a\"];", "filter"), Some((Effect::HandsOn, true)));
    assert_eq!(effect("addheader \"X-A\" \"b\";", "addheader"), Some((Effect::Rewrites, true)));
    assert_eq!(effect("REDIRECT \"a@example.org\";", "redirect"), Some((Effect::Sends, true)), "names are case-insensitive");
    assert_eq!(effect("vnd_example_foo;", "vnd_example_foo"), Some((Effect::Sends, false)));
    assert_eq!(effect("if vnd_example_test { keep; }", "vnd_example_test"), Some((Effect::Sends, false)));
    // Counted per name, nested commands and tests too.
    let uses = effects("if true { redirect \"a@example.org\"; } redirect \"b@example.org\";").unwrap();
    assert_eq!(uses.iter().find(|u| u.name == "redirect").map(|u| u.count), Some(2));
    assert!(effects("\"open").is_none());
}

#[test]
fn the_local_check_knows_the_vendor_commands_it_classes() {
    use crate::sieve_check::check;
    let ok = [
        "require \"vnd.dovecot.report\";\r\nreport :headers_only \"abuse\" \"spam\" \"abuse@example.org\";\r\n",
        "require [\"vnd.dovecot.pipe\", \"copy\"];\r\npipe :try :copy \"prog\" [\"a\"];\r\npipe \"prog\";\r\n",
        "require \"vnd.dovecot.filter\";\r\nif filter \"prog\" { keep; }\r\nfilter \"prog\";\r\n",
        "require [\"vnd.dovecot.execute\", \"variables\"];\r\nif execute :input \"x\" :output \"out\" \"prog\" [\"a\"] { keep; }\r\nexecute :pipe \"prog\";\r\n",
    ];
    for text in ok {
        let c = check(text, None).unwrap_or_else(|p| panic!("{text}: {p:?}"));
        assert!(c.warnings.is_empty(), "{text}: {:?}", c.warnings);
    }
    assert!(check("pipe \"prog\";", None).is_err(), "it needs its require");
    assert!(check("require \"vnd.dovecot.execute\";\r\nexecute :input \"x\" :pipe \"prog\";", None).is_err(), ":input and :pipe exclude each other");
    assert!(check("require \"vnd.dovecot.pipe\";\r\nif pipe \"prog\" { keep; }", None).is_err(), "pipe is no test");
}

#[test]
fn an_agents_sieve_rule_that_reports_or_hands_mail_to_a_program_asks_as_a_send() {
    let (s, account) = session();
    for sieve in [
        "report \"abuse\" \"spam\" \"abuse@example.org\";",
        "pipe \"prog\";",
        "if execute :pipe \"prog\" { keep; }",
        "execute \"prog\";",
        "filter \"prog\";",
    ] {
        let params = add_sieve(account, sieve);
        assert_eq!(classify(&s, "rules.addSieve", &params), Access::Owner(Class::Send), "{sieve}");
    }
    assert!(describe(&s, "rules.addSieve", &add_sieve(account, "pipe \"prog\";")).contains("; it can hand mail to programs on the server"));
    assert!(
        describe(&s, "rules.addSieve", &add_sieve(account, "report \"abuse\" \"spam\" \"abuse@example.org\";"))
            .contains("; it can send mail or answer senders")
    );
}

#[test]
fn a_command_not_known_or_a_text_not_read_counts_as_sending() {
    // `vnd.example.foo` isn't even a Sieve identifier: the text can't be read.
    assert!(sends("vnd.example.foo;"));
    assert!(sends("vnd_example_foo;"));
    assert!(sends("if true {"));
    // A rule the local check refuses is made by nothing, so it asks nothing more than settings;
    // one kept already (from a later version, or another program) that is switched on asks as a send.
    let (mut s, account) = session();
    assert_eq!(classify(&s, "rules.addSieve", &add_sieve(account, "vnd_example_foo;")), Access::Owner(Class::Settings));
    let id = s.mailbox.alloc_id();
    let rule =
        Rule { id, name: "Later".into(), enabled: false, account, keep: Keep::Server, sieve: "vnd_example_foo \"x\";".into(), ..Default::default() };
    s.mailbox.rules.push(rule);
    assert_eq!(classify(&s, "rules.enable", &json!({"id": id, "value": true})), Access::Owner(Class::Send));
    let unread = Rule { id: s.mailbox.alloc_id(), sieve: "if true {".into(), ..s.mailbox.rules[0].clone() };
    let unread_id = unread.id;
    s.mailbox.rules.push(unread);
    assert_eq!(classify(&s, "rules.enable", &json!({"id": unread_id, "value": true})), Access::Owner(Class::Send));
}

#[test]
fn a_sending_word_in_a_string_or_a_comment_is_no_command() {
    let (s, account) = session();
    for sieve in ["fileinto \"redirect\";", "# redirect \"bob@example.org\";\r\nkeep;", "/* pipe \"prog\"; */ keep;"] {
        assert!(!sends(sieve), "{sieve}");
        assert_eq!(classify(&s, "rules.addSieve", &add_sieve(account, sieve)), Access::Owner(Class::Settings), "{sieve}");
    }
    assert!(!adds_sending("keep;", "fileinto \"pipe\"; # execute"));
}

#[test]
fn a_rule_that_changes_messages_asks_as_settings_and_says_so() {
    let (s, account) = session();
    let params = add_sieve(account, "addheader \"X-Seen-By\" \"ada\";\r\ndeleteheader \"X-Spam\";");
    assert_eq!(classify(&s, "rules.addSieve", &params), Access::Owner(Class::Settings));
    let said = describe(&s, "rules.addSieve", &params);
    assert!(said.contains("; it can change messages (addheader, deleteheader). Its Sieve"), "{said}");
    assert!(!said.contains("send mail"), "{said}");
    let quiet = describe(&s, "rules.addSieve", &add_sieve(account, "keep;"));
    assert!(!quiet.contains("change messages"), "{quiet}");
}

#[test]
fn the_owners_script_that_adds_a_pipe_asks_as_a_send() {
    let (s, account) = session();
    let d = digest(THEIRS);
    let piped = format!("require [\"fileinto\", \"vnd.dovecot.pipe\"];{}pipe \"prog\";\r\n", THEIRS.trim_start_matches("require \"fileinto\";"));
    let params = json!({"account": account, "text": piped, "digest": d});
    assert_eq!(classify(&s, "sieve.putOwnScript", &params), Access::Owner(Class::Send));
    assert!(describe(&s, "sieve.putOwnScript", &params).contains("; it adds a way to hand mail to programs on the server"));
    assert!(adds_sending(THEIRS, &piped));
    // Another use of what was there counts; the same uses, moved about, don't.
    assert!(adds_sending("pipe \"a\";", "pipe \"a\"; pipe \"b\";"));
    assert!(!adds_sending("pipe \"a\"; redirect \"x@example.org\";", "redirect \"x@example.org\"; pipe \"b\";"));
    // One that only changes messages asks as settings and says so.
    let edited = format!("require [\"fileinto\", \"editheader\"];{}addheader \"X-A\" \"b\";\r\n", THEIRS.trim_start_matches("require \"fileinto\";"));
    let params = json!({"account": account, "text": edited, "digest": d});
    assert_eq!(classify(&s, "sieve.putOwnScript", &params), Access::Owner(Class::Settings));
    assert!(describe(&s, "sieve.putOwnScript", &params).contains("; it adds a way to change messages (addheader)"));
}
