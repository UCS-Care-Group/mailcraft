// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules of an IMAP account kept in MailCraft's Sieve script (#2057): what each part is in Sieve,
//! what keeps a rule on this computer, that no value becomes Sieve, and what a server's answer
//! comes to.

use mailcraft_model::{Action, Condition, FolderKind, Id, Keep, Rule};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::rules::{NotOnServer, kept, server_cannot};
use super::sieve_replies::{SIEVE_ID, script_due};
use super::sieve_rules::block;
use crate::Session;
use crate::sieve::{SCRIPT_NAME, Tok, digest, read_vacation, tokens};

const ALL: [&str; 7] = ["vacation", "date", "relational", "fileinto", "copy", "imap4flags", "body"];

/// An IMAP account whose server's scripts may use `offers`, with a Deleted Items and a folder
/// `Lists/Ünïcode` on the server; the account, and that folder.
fn session(offers: &[&str]) -> (Session, Id, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap();
    let account = added["id"].as_u64().unwrap();
    s.mailbox.account_mut(account).unwrap().sieve_offers = offers.iter().map(|e| e.to_string()).collect();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let lists = s.run("folder.new", &json!({"name": "Ünïcode", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.folder_mut(lists).unwrap().remote = "INBOX/Lists/Ünïcode".into();
    if let Some(deleted) = s.mailbox.folder_of_kind(account, FolderKind::Deleted) {
        s.mailbox.folder_mut(deleted).unwrap().remote = "Trash".into();
    }
    (s, account, lists)
}

fn rule(account: Id, conditions: Vec<Condition>, actions: Vec<Action>) -> Rule {
    Rule { id: 77, name: "R".into(), enabled: true, conditions, actions, account, keep: Keep::Server, ..Default::default() }
}

fn words(w: &[&str]) -> Vec<String> {
    w.iter().map(|w| w.to_string()).collect()
}

#[test]
fn each_condition_is_the_sieve_that_does_what_it_does_here() {
    let (s, account, lists) = session(&ALL);
    let mv = vec![Action::MoveTo(lists)];
    for (condition, sieve) in [
        (Condition::FromContains(words(&["digest"])), "address :all :contains \"from\" [\"digest\"]"),
        (Condition::ToContains(words(&["team"])), "address :all :contains [\"to\", \"cc\"] [\"team\"]"),
        (Condition::SubjectContains(words(&["a", "b"])), "header :contains \"subject\" [\"a\", \"b\"]"),
        (Condition::BodyContains(words(&["x"])), "body :text :contains [\"x\"]"),
        (Condition::SubjectOrBodyContains(words(&["x"])), "anyof (header :contains \"subject\" [\"x\"], body :text :contains [\"x\"])"),
        (Condition::From(words(&["bo@example.org"])), "address :all :is \"from\" [\"bo@example.org\"]"),
        (Condition::SentTo(words(&["bo@example.org"])), "address :all :is [\"to\", \"cc\"] [\"bo@example.org\"]"),
        (Condition::ToMe, "address :all :is \"to\" [\"ada@example.org\"]"),
        (Condition::CcMe, "address :all :is \"cc\" [\"ada@example.org\"]"),
        (Condition::ToOrCcMe, "address :all :is [\"to\", \"cc\"] [\"ada@example.org\"]"),
        (Condition::NotToMe, "not address :all :is \"to\" [\"ada@example.org\"]"),
        (Condition::SizeBetween { min: 10, max: 20 }, "allof (not size :under 10240, size :under 21504)"),
    ] {
        let b = block(&s.mailbox, &rule(account, vec![condition.clone()], mv.clone()), account).unwrap();
        assert_eq!(b.text, format!("if {sieve} {{\r\n    fileinto \"INBOX/Lists/Ünïcode\";\r\n}}\r\n"), "{condition:?}");
    }
}

/// A folder's name goes to `fileinto` whole, its spaces kept: on the server "Lists " is another
/// folder than "Lists" (#2058).
#[test]
fn a_folder_name_with_spaces_at_its_ends_is_kept_whole() {
    let (mut s, account, lists) = session(&ALL);
    s.mailbox.folder_mut(lists).unwrap().remote = " Lists ".into();
    if let Some(deleted) = s.mailbox.folder_of_kind(account, FolderKind::Deleted) {
        s.mailbox.folder_mut(deleted).unwrap().remote = "Trash ".into();
    }
    let b = block(&s.mailbox, &rule(account, vec![Condition::ToMe], vec![Action::MoveTo(lists)]), account).unwrap();
    assert!(b.text.contains("fileinto \" Lists \";"), "{}", b.text);
    let b = block(&s.mailbox, &rule(account, vec![Condition::ToMe], vec![Action::Delete]), account).unwrap();
    assert!(b.text.contains("fileinto \"Trash \";"), "{}", b.text);
    // A name of only spaces is no folder on the server.
    s.mailbox.folder_mut(lists).unwrap().remote = "  ".into();
    assert_eq!(block(&s.mailbox, &rule(account, vec![Condition::ToMe], vec![Action::MoveTo(lists)]), account).err(), Some(NotOnServer::FolderNotThere));
}

#[test]
fn each_action_is_the_sieve_that_does_what_it_does_here_flags_first() {
    let (s, account, lists) = session(&ALL);
    let r = Rule {
        exceptions: vec![Condition::SubjectContains(words(&["urgent"])), Condition::From(words(&["boss@example.org"]))],
        stop: true,
        ..rule(
            account,
            vec![Condition::FromContains(words(&["news"])), Condition::ToMe],
            vec![Action::CopyTo(lists), Action::Delete, Action::MarkRead, Action::Flag, Action::ClearFlag],
        )
    };
    let b = block(&s.mailbox, &r, account).unwrap();
    assert_eq!(
        b.text,
        "if allof (address :all :contains \"from\" [\"news\"], address :all :is \"to\" [\"ada@example.org\"], not anyof (header :contains \"subject\" [\"urgent\"], address :all :is \"from\" [\"boss@example.org\"])) {\r\n    addflag \"\\\\Seen\";\r\n    addflag \"\\\\Flagged\";\r\n    removeflag \"\\\\Flagged\";\r\n    fileinto :copy \"INBOX/Lists/Ünïcode\";\r\n    fileinto \"Trash\";\r\n    stop;\r\n}\r\n"
    );
    assert_eq!(b.requires, vec!["copy", "fileinto", "imap4flags"]);
}

#[test]
fn what_sieve_cant_say_keeps_the_rule_here_and_says_which_part() {
    let (mut s, account, lists) = session(&ALL);
    let mv = vec![Action::MoveTo(lists)];
    let from = vec![Condition::FromContains(words(&["x"]))];
    for (r, why) in [
        (rule(account, vec![Condition::HasAttachment], mv.clone()), NotOnServer::Attachments),
        (rule(account, vec![Condition::Importance(mailcraft_model::Importance::High)], mv.clone()), NotOnServer::Importance),
        (rule(account, vec![Condition::Category(words(&["Red"]))], mv.clone()), NotOnServer::Categories),
        (rule(account, vec![Condition::MeetingRequest], mv.clone()), NotOnServer::MeetingRequest),
        (rule(account, vec![Condition::AutomaticReply], mv.clone()), NotOnServer::AutomaticReply),
        (rule(account, vec![Condition::SentOnlyToMe], mv.clone()), NotOnServer::OnlyToMe),
        (rule(account, vec![Condition::Flagged], mv.clone()), NotOnServer::Flagged),
        (rule(account, vec![Condition::ReceivedBetween { from: None, to: None }], mv.clone()), NotOnServer::Days),
        (rule(account, vec![Condition::SizeBetween { min: u64::MAX, max: u64::MAX }], mv.clone()), NotOnServer::Size),
        (rule(account, Vec::new(), mv.clone()), NotOnServer::NoConditions),
        (rule(account, vec![Condition::FromContains(words(&["Ada Lovelace"]))], mv.clone()), NotOnServer::Names),
        (rule(account, vec![Condition::ToContains(words(&["team\""]))], mv.clone()), NotOnServer::Names),
        (rule(account, from.clone(), vec![Action::Categorize("Red".into())]), NotOnServer::Categorize),
        (rule(account, from.clone(), vec![Action::SetImportance(mailcraft_model::Importance::Low)]), NotOnServer::SetImportance),
        (rule(account, from.clone(), vec![Action::FlagFor(mailcraft_model::FlagDue::Today)]), NotOnServer::FlagDue),
        (rule(account, from.clone(), vec![Action::ClearCategories]), NotOnServer::ClearCategories),
        (rule(account, from.clone(), vec![Action::MoveTo(lists), Action::Delete]), NotOnServer::TwoMoves),
        (rule(account, from.clone(), vec![Action::CopyTo(lists), Action::CopyTo(lists)]), NotOnServer::TwoCopies),
        (rule(account, from.clone(), vec![Action::MoveTo(999_999)]), NotOnServer::FolderNotThere),
        (Rule { alert: true, ..rule(account, from.clone(), mv.clone()) }, NotOnServer::Alert),
    ] {
        assert_eq!(server_cannot(&s.mailbox, &r, account), Some(why), "{r:?}");
    }
    // A folder that isn't on the server (made here, or gone there).
    s.mailbox.folder_mut(lists).unwrap().remote.clear();
    assert_eq!(server_cannot(&s.mailbox, &rule(account, from.clone(), mv.clone()), account), Some(NotOnServer::FolderNotThere));
    // An extension the server's scripts don't have.
    let (s, account, lists) = session(&["fileinto"]);
    assert_eq!(
        server_cannot(&s.mailbox, &rule(account, from.clone(), vec![Action::MarkRead, Action::MoveTo(lists)]), account),
        Some(NotOnServer::Extension)
    );
    assert_eq!(
        server_cannot(&s.mailbox, &rule(account, vec![Condition::BodyContains(words(&["x"]))], vec![Action::MoveTo(lists)]), account),
        Some(NotOnServer::Extension)
    );
    assert_eq!(server_cannot(&s.mailbox, &rule(account, from.clone(), vec![Action::MoveTo(lists)]), account), None);
    // A server that never said it keeps Sieve scripts: none of its rules go there.
    let (s, account, lists) = session(&[]);
    assert_eq!(server_cannot(&s.mailbox, &rule(account, from, vec![Action::MoveTo(lists)]), account), Some(NotOnServer::NoServer));
}

/// Every identifier a script MailCraft writes may hold.
const WORDS: [&str; 21] = [
    "require",
    "if",
    "allof",
    "anyof",
    "not",
    "header",
    "address",
    "body",
    "size",
    "fileinto",
    "addflag",
    "removeflag",
    "stop",
    "vacation",
    "currentdate",
    "else",
    "keep",
    "true",
    "false",
    "exists",
    "envelope",
];

/// The script's tokens with every string blanked: its shape.
fn shape(script: &str) -> Vec<Tok> {
    tokens(script).unwrap().into_iter().map(|t| if let Tok::Str(_) = t { Tok::Str(String::new()) } else { t }).collect()
}

/// A rule with every kind of value it can carry to the server set to `v`.
fn every_value(account: Id, lists: Id, v: &str) -> Rule {
    // (Words in an address can't have spaces, quotes or the marks that set a name apart: what is
    // left of `v`.)
    let a: String = v.chars().filter(|c| !c.is_whitespace() && !"\"(),:;<>[]\\".contains(*c)).collect();
    let a = if a.is_empty() { "x".to_string() } else { a };
    Rule {
        exceptions: vec![Condition::SentTo(words(&[v])), Condition::BodyContains(words(&[v]))],
        ..rule(
            account,
            vec![
                Condition::FromContains(words(&[&a])),
                Condition::ToContains(words(&[&a, &a])),
                Condition::SubjectContains(words(&[v])),
                Condition::From(words(&[v])),
                Condition::SubjectOrBodyContains(words(&[v])),
            ],
            vec![Action::MarkRead, Action::CopyTo(lists), Action::MoveTo(lists)],
        )
    }
}

proptest! {
    #[test]
    fn no_rule_value_becomes_sieve(v in ".{0,59}[a-z]", folder in ".{0,39}[a-z]", me in ".{1,40}") {
        // (#2057) Words, addresses, folder names and the owner's address with quotes, backslashes,
        // line breaks, `;`, `}`, `#`, `text:` in them are one string each: the block has the shape
        // it has with plain words, and no command of its own.
        let (mut s, account, lists) = session(&ALL);
        let plain = block(&s.mailbox, &every_value(account, lists, "w"), account).unwrap();
        s.mailbox.folder_mut(lists).unwrap().remote = folder;
        s.mailbox.account_mut(account).unwrap().email = me;
        let nasty = block(&s.mailbox, &every_value(account, lists, &v), account).unwrap();
        prop_assert_eq!(shape(&nasty.text), shape(&plain.text));
        let all = tokens(&nasty.text).unwrap();
        prop_assert!(all.iter().all(|t| !matches!(t, Tok::Ident(w) if !WORDS.contains(&w.as_str()))));
    }
}

#[test]
fn nasty_values_never_make_a_reply_a_redirect_a_reject_or_another_require() {
    let (mut s, account, lists) = session(&ALL);
    let nasty = "x\"; redirect \"evil@example.net\"; reject \"no\"; vacation \"hi\"; require [\"enotify\"]; #\r\n.\r\ntext:\r\nkeep;\r\n.\r\n}";
    s.mailbox.folder_mut(lists).unwrap().remote = nasty.into();
    s.mailbox.rules.push(every_value(account, lists, nasty));
    let vacation = crate::sieve::Vacation {
        status: mailcraft_model::auto_replies::AutoReplyStatus::Disabled,
        start: None,
        end: None,
        internal: String::new(),
        external: String::new(),
        audience: mailcraft_model::auto_replies::ExternalAudience::None,
    };
    let (text, activate) = super::sieve_replies::whole_script(&s.mailbox, account, &vacation);
    assert!(activate);
    let all = tokens(&text).unwrap();
    let idents: Vec<&str> = all.iter().filter_map(|t| if let Tok::Ident(w) = t { Some(w.as_str()) } else { None }).collect();
    for never in ["redirect", "reject", "vacation", "discard", "keep", "notify"] {
        assert!(!idents.contains(&never), "{never} in {text}");
    }
    assert_eq!(idents.iter().filter(|w| **w == "require").count(), 1, "{text}");
    // And the one `require` names only what MailCraft uses.
    assert!(text.contains("require [\"fileinto\", \"copy\", \"imap4flags\", \"body\"];"), "{text}");
}

/// The server's answer to a read, as `sync.applySieve` takes it.
fn state(active: Option<&str>, script: Option<&str>) -> Value {
    let scripts: Vec<Value> = active.iter().map(|name| json!({"name": name, "active": true})).collect();
    json!({"extensions": ALL, "scripts": scripts, "script": script})
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

#[test]
fn a_rule_goes_into_the_script_after_the_replies_and_is_on_the_server_once_it_runs_there() {
    let (mut s, account, lists) = session(&ALL);
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state(None, None)}));
    // A new rule the server can keep is kept there by default, and waits till it is.
    let made = run(
        &mut s,
        "rules.add",
        json!({"name": "Lists", "account": account, "conditions": [{"type": "fromContains", "value": "list"}], "actions": [{"type": "moveTo", "value": lists}]}),
    );
    assert_eq!(made["keep"], "server");
    let id = made["id"].as_u64().unwrap();
    assert_eq!(kept(&s.mailbox, s.mailbox.rules.iter().find(|r| r.id == id).unwrap()).0, "waiting");
    // What is to be written: the replies (off), then the rule.
    let due = script_due(&s, account, false).unwrap();
    let text = due["text"].as_str().unwrap().to_string();
    assert_eq!(due["activate"], true);
    assert!(text.contains(&format!("# rule {id}\r\nif address :all :contains \"from\" [\"list\"]")), "{text}");
    assert!(text.find("Automatic replies are off").unwrap() < text.find("# rule").unwrap());
    // Written and active: the rule is the server's.
    run(
        &mut s,
        "sync.applySieve",
        json!({"account": account, "state": state(Some(SCRIPT_NAME), Some(&text)), "written": {"result": "done", "text": text}}),
    );
    let r = s.mailbox.rules.iter().find(|r| r.id == id).unwrap();
    assert_eq!(r.server.as_ref().map(|s| s.id.clone()), Some(format!("{SIEVE_ID}{id}")));
    assert_eq!(kept(&s.mailbox, r).0, "server");
    assert!(script_due(&s, account, false).is_none());
    // Switched off here: the script is to change, without it.
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    let off = script_due(&s, account, false).unwrap()["text"].as_str().unwrap().to_string();
    assert!(!off.contains("# rule"), "{off}");
    // Replies changed: the rules stay in the script.
    run(&mut s, "rules.enable", json!({"id": id, "value": true}));
    let replies = run(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "internal": "Away."}));
    let both = replies["sieve"]["text"].as_str().unwrap();
    assert!(both.contains("vacation ") && both.contains(&format!("# rule {id}")), "{both}");
    assert!(both.find("vacation ").unwrap() < both.find("# rule").unwrap());
    assert_eq!(read_vacation(both).unwrap().internal, "Away.");
}

#[test]
fn rules_run_here_while_the_server_runs_another_script_or_a_changed_one() {
    let (mut s, account, lists) = session(&ALL);
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state(None, None)}));
    let id = run(
        &mut s,
        "rules.add",
        json!({"name": "Lists", "account": account, "conditions": [{"type": "fromContains", "value": "list"}], "actions": [{"type": "moveTo", "value": lists}]}),
    )["id"]
        .as_u64()
        .unwrap();
    let text = script_due(&s, account, false).unwrap()["text"].as_str().unwrap().to_string();
    // Written beside the owner's own active script: not run there, so run here.
    run(
        &mut s,
        "sync.applySieve",
        json!({"account": account, "state": state(Some("roundcube"), Some(&text)), "written": {"result": "anotherActive", "text": text}}),
    );
    assert!(!s.mailbox.rules.iter().find(|r| r.id == id).unwrap().on_server());
    assert!(script_due(&s, account, false).is_none(), "written already; nothing to write again");
    // Active, then changed elsewhere: back to this computer, and nothing is written over.
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state(Some(SCRIPT_NAME), Some(&text))}));
    assert!(s.mailbox.rules.iter().find(|r| r.id == id).unwrap().on_server());
    let edited = text.replace("\"list\"", "\"other\"");
    assert_ne!(edited, text);
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state(Some(SCRIPT_NAME), Some(&edited))}));
    assert!(!s.mailbox.rules.iter().find(|r| r.id == id).unwrap().on_server());
    assert!(script_due(&s, account, false).is_none());
    assert_ne!(s.mailbox.account(account).unwrap().sieve_written, digest(&edited));
    // Replaced only when the owner asks, over the script as it was read.
    let replace = script_due(&s, account, true).unwrap();
    assert_eq!(replace["lastWritten"], digest(&edited));
    assert!(replace["text"].as_str().unwrap().contains(&format!("# rule {id}")));
    // A rule deleted here leaves the script, and isn't kept for Graph to be told.
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state(Some(SCRIPT_NAME), Some(&text))}));
    run(&mut s, "rules.remove", json!({"id": id}));
    assert!(s.mailbox.rule_gone.is_empty());
    assert!(!script_due(&s, account, false).unwrap()["text"].as_str().unwrap().contains("# rule"));
}

/// A rule kept in MailCraft's script, changed to stay on this computer: while the server still
/// runs the script with it (the write not made yet, or refused), it stays the server's and doesn't
/// run here as well, which would act on the same mail twice; once the server is read with the
/// script without it, it runs here (#2058).
#[test]
fn a_rule_moved_off_the_server_runs_here_only_once_the_script_without_it_is_there() {
    let (mut s, account, lists) = session(&ALL);
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state(None, None)}));
    let rule = json!({"name": "Lists", "account": account, "conditions": [{"type": "fromContains", "value": "list"}], "actions": [{"type": "moveTo", "value": lists}]});
    let id = run(&mut s, "rules.add", rule.clone())["id"].as_u64().unwrap();
    let text = script_due(&s, account, false).unwrap()["text"].as_str().unwrap().to_string();
    run(
        &mut s,
        "sync.applySieve",
        json!({"account": account, "state": state(Some(SCRIPT_NAME), Some(&text)), "written": {"result": "done", "text": text}}),
    );
    let on_server = |s: &Session| s.mailbox.rules.iter().find(|r| r.id == id).unwrap().on_server();
    assert!(on_server(&s));
    let mut here = rule.clone();
    here["id"] = json!(id);
    here["server"] = json!(false);
    here.as_object_mut().unwrap().remove("account");
    assert_eq!(run(&mut s, "rules.edit", here)["keep"], "here");
    let r = s.mailbox.rules.iter().find(|r| r.id == id).unwrap();
    assert_eq!(r.keep, Keep::Here);
    assert!(r.on_server(), "the server's until the script without it is there");
    // Read again before the write, or after one refused: the server still runs it.
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state(Some(SCRIPT_NAME), Some(&text))}));
    assert!(on_server(&s));
    // Written without it: this computer's.
    let without = script_due(&s, account, false).unwrap()["text"].as_str().unwrap().to_string();
    assert!(!without.contains("# rule"), "{without}");
    run(
        &mut s,
        "sync.applySieve",
        json!({"account": account, "state": state(Some(SCRIPT_NAME), Some(&without)), "written": {"result": "done", "text": without}}),
    );
    assert!(!on_server(&s));
    assert_eq!(kept(&s.mailbox, s.mailbox.rules.iter().find(|r| r.id == id).unwrap()).0, "here");
}

#[test]
fn what_the_server_said_its_scripts_may_use_is_kept_with_the_account() {
    let (mut s, account, _) = session(&[]);
    run(&mut s, "sync.applySieve", json!({"account": account, "state": state(None, None)}));
    assert_eq!(s.mailbox.account(account).unwrap().sieve_offers, ALL.map(String::from).to_vec());
    assert!(s.mailbox.keeps_sieve_rules(account));
    // Changing a rule kept there needs the owner when an agent asks, as any rule does.
    assert!(matches!(crate::agent::access("rules.add"), Some(crate::agent::Access::Owner(_))));
    assert_eq!(crate::agent::access("sync.sieveScript"), Some(crate::agent::Access::Never));
}
