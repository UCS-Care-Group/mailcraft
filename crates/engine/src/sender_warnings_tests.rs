// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sender warnings in a session (#508): what `mail.get` says of a message, and how what they are
//! measured against (contacts, Sent Items, the accounts) keeps up with the mailbox.
//!
//! The people, hosts and addresses are invented.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, Body, FolderKind, Id, Message};
use serde_json::{Value, json};

use crate::Session;

/// A session with Avery's IMAP account at Lumen, whose Inbox has `messages` from its server; the
/// account's id.
fn imap(messages: Vec<Message>) -> (Session, Id) {
    let mut s = Session::empty();
    let a = s
        .run("account.add", &json!({"email": "avery@lumen.example", "displayName": "Avery Quinn", "incoming": {"host": "imap.lumen.example"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let new = messages.into_iter().enumerate().map(|(i, m)| Message { uid: i as u32 + 1, ..m }).collect();
    let result = SyncResult {
        account: a,
        folders: vec![FolderDelta {
            id: None,
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 50,
            new,
            ..Default::default()
        }],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": result})).unwrap();
    (s, a)
}

fn mail(name: &str, email: &str, subject: &str) -> Message {
    Message {
        from: Address::new(name, email),
        subject: subject.into(),
        message_id: format!("{subject}@example.org"),
        date: 100,
        ..Default::default()
    }
}

fn id_of(s: &Session, subject: &str) -> Id {
    s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.id).unwrap()
}

fn kinds(s: &mut Session, subject: &str) -> Vec<String> {
    let got = s.run("mail.get", &json!({"id": id_of(s, subject)})).unwrap();
    got["warnings"].as_array().unwrap().iter().map(|w| w["kind"].as_str().unwrap().to_string()).collect()
}

#[test]
fn mail_get_gives_the_warnings_as_data() {
    let mut failed = mail("Lin Zhao", "lin.zhao@harborworks.example", "Invoice");
    failed.headers = vec![("Authentication-Results".into(), "mx.lumen.example; dmarc=fail header.from=harborworks.example".into())];
    failed.body = Body {
        text: "Pay".into(),
        html: Some("<p>Pay at <a href=\"https://pay.evil.example/x\"><b>https://</b>harborworks.example/pay</a></p>".into()),
    };
    let (mut s, _) = imap(vec![failed, mail("Maya Okafor", "maya@lumen.example", "Lunch")]);
    let got = s.run("mail.get", &json!({"id": id_of(&s, "Invoice")})).unwrap();
    let warnings = got["warnings"].as_array().unwrap();
    assert_eq!(warnings[0], json!({"kind": "failedCheck", "level": "warning", "method": "dmarc"}));
    assert_eq!(warnings[1]["kind"], "linkMismatch");
    assert_eq!(warnings[1]["textHost"], "harborworks.example");
    assert_eq!(warnings[1]["host"], "pay.evil.example");
    assert_eq!(warnings[2], json!({"kind": "external", "level": "tag", "domain": "harborworks.example"}));
    // A colleague's ordinary mail: nothing.
    assert_eq!(kinds(&mut s, "Lunch"), Vec::<String>::new());
}

#[test]
fn a_contact_or_a_new_recipient_makes_a_lookalike_known() {
    let (mut s, _) = imap(vec![
        mail("Lin Zhao", "lin.zhao@harborworks.example", "Old thread"),
        mail("Lin", "lin@harborwork.example", "Lookalike"),
        mail("Robin Hale", "robin@cloudlines.example", "Second"),
    ]);
    // Nobody at harborworks.example is known yet: nothing to look like.
    assert_eq!(kinds(&mut s, "Lookalike"), ["external"]);
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    assert_eq!(kinds(&mut s, "Lookalike"), ["lookalikeSender", "external"]);
    // Then the lookalike itself becomes a contact: it is known.
    s.run("people.new", &json!({"displayName": "Lin at the depot", "email": "lin@harborwork.example"})).unwrap();
    assert_eq!(kinds(&mut s, "Lookalike"), ["external"]);

    // Written to: cloudline.example, from the Outbox on, makes cloudlines.example a lookalike;
    // and writing to cloudlines.example makes it known again.
    assert_eq!(kinds(&mut s, "Second"), ["external"]);
    s.run("mail.new", &json!({})).unwrap();
    s.run("compose.set", &json!({"to": "Billing <billing@cloudline.example>", "subject": "Card"})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    assert_eq!(kinds(&mut s, "Second"), ["lookalikeSender", "external"]);
    s.run("mail.new", &json!({})).unwrap();
    s.run("compose.set", &json!({"to": "robin@cloudlines.example", "subject": "Hello"})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    assert_eq!(kinds(&mut s, "Second"), ["external"]);
}

#[test]
fn a_known_name_on_a_stranger_address() {
    let (mut s, _) = imap(vec![mail("Maya Okafor", "maya.okafor@freemail.example", "Urgent"), mail("Avery Quinn", "ceo@freemail.example", "Mine")]);
    assert_eq!(kinds(&mut s, "Urgent"), ["external"]);
    s.run("people.new", &json!({"first": "Maya", "last": "Okafor", "email": "maya@lumen.example"})).unwrap();
    assert_eq!(kinds(&mut s, "Urgent"), ["nameHidesAddress", "external"]);
    let got = s.run("mail.get", &json!({"id": id_of(&s, "Mine")})).unwrap();
    assert_eq!(got["warnings"][0], json!({"kind": "nameHidesAddress", "level": "warning", "name": "Avery Quinn", "reason": "ownName"}));
}

#[test]
fn a_name_written_in_sent_items_is_not_compared() {
    let (mut s, _) = imap(vec![mail("Robin Hale", "robin.hale@freemail.example", "Hello")]);
    // The owner wrote to Robin by that name: the address is known, the name is not.
    s.run("mail.new", &json!({})).unwrap();
    s.run("compose.set", &json!({"to": "Robin Hale <robin@cloudline.example>", "subject": "Hi"})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    assert_eq!(kinds(&mut s, "Hello"), ["external"]);
    // A contact's name is.
    s.run("people.new", &json!({"displayName": "Robin Hale", "email": "robin@cloudline.example"})).unwrap();
    assert_eq!(kinds(&mut s, "Hello"), ["nameHidesAddress", "external"]);
}

#[test]
fn warnings_are_kept_until_what_they_rest_on_changes() {
    let (mut s, _) = imap(vec![mail("Lin", "lin@harborwork.example", "Lookalike")]);
    let m = s.mailbox.messages.get(&id_of(&s, "Lookalike")).cloned().unwrap();
    let first = s.sender_warnings(&m, false);
    let again = s.sender_warnings(&m, false);
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    // Marking it read changes the mailbox, not what the warnings rest on.
    s.run("mail.markRead", &json!({"ids": [m.id]})).unwrap();
    let m = s.mailbox.messages.get(&m.id).cloned().unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &s.sender_warnings(&m, false)));
    // A contact does.
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &s.sender_warnings(&m, false)));
    // So does the owner's organisation.
    let before = s.sender_warnings(&m, false);
    s.run("account.add", &json!({"email": "avery@harborwork.example", "incoming": {"host": "imap.harborwork.example"}})).unwrap();
    let after = s.sender_warnings(&m, false);
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
    // Now from the owner's own domain, so not External; and that domain is known, so no lookalike.
    assert!(after.is_empty(), "{after:?}");
}

#[test]
fn a_new_body_of_the_same_length_or_another_server_is_worked_out_again() {
    let (mut s, _) = imap(vec![mail("Lin", "lin@harborworks.example", "Body")]);
    let mut m = s.mailbox.messages.get(&id_of(&s, "Body")).cloned().unwrap();
    std::sync::Arc::make_mut(&mut m).body.html = Some(r#"<a href="https://lumen.example/">lumen.example</a>"#.into());
    assert!(s.sender_warnings(&m, true).iter().all(|w| w.kind() != "linkMismatch"));
    // The same length, another place.
    std::sync::Arc::make_mut(&mut m).body.html = Some(r#"<a href="https://lumen.exampl/">lumen.example</a>"#.into());
    assert!(s.sender_warnings(&m, true).iter().any(|w| w.kind() == "linkMismatch"));
    // A row asks without the links, and the pane's answer stands for it.
    let pane = s.sender_warnings(&m, true);
    assert!(std::sync::Arc::ptr_eq(&pane, &s.sender_warnings(&m, false)));
    // The server of the account is part of what the verdict rests on.
    let account = m.account;
    if let Some(a) = s.mailbox.accounts.iter_mut().find(|a| a.id == account)
        && let Some(server) = a.incoming.as_mut()
    {
        server.host = "imap.elsewhere.example".into();
    }
    assert!(!std::sync::Arc::ptr_eq(&pane, &s.sender_warnings(&m, false)));
}

#[test]
fn the_knowledge_follows_another_mailbox() {
    let (mut s, _) = imap(vec![]);
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    assert!(s.knows("lin.zhao@harborworks.example"));
    s.replace_mailbox(mailcraft_model::Mailbox::with_local_account("", ""));
    assert!(!s.knows("lin.zhao@harborworks.example"));
}

#[test]
fn hostile_messages_never_panic_mail_get() {
    let long = "x".repeat(100_000);
    let mut m = mail(&format!("{long}@{long}"), &format!("a@{long}.example"), "Long");
    m.headers = vec![("Authentication-Results".into(), format!("mx.lumen.example; dmarc=fail header.from={long}; {}", ";".repeat(10_000)))];
    let links: String = (0..3_000).map(|i| format!("<a href=\"https://x{i}.example/\">site{i}.example</a> ")).collect();
    m.body = Body { text: String::new(), html: Some(format!("<table><tr><td>{links}</td></tr></table>")) };
    let (mut s, _) = imap(vec![m]);
    let got = s.run("mail.get", &json!({"id": id_of(&s, "Long")})).unwrap();
    let warnings = got["warnings"].as_array().unwrap();
    assert!(warnings.len() <= 25, "{}", warnings.len());
    assert!(warnings.iter().all(|w| w.to_string().len() < 2_000));
    let _: &Value = &got;
}

fn banner_dismissed(s: &mut Session, subject: &str) -> bool {
    let m = s.mailbox.messages.get(&id_of(s, subject)).unwrap().clone();
    let warnings = s.sender_warnings(&m, true);
    mailcraft_model::sender_warnings::dismissed(&warnings, &m.warnings_dismissed)
}

#[test]
fn dismissing_the_banner_is_remembered_with_the_message_and_can_be_undone() {
    let (mut s, _) = imap(vec![mail("Lin", "lin@harborwork.example", "Odd"), mail("Maya Okafor", "maya@lumen.example", "Lunch")]);
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    let id = id_of(&s, "Odd");
    assert!(!banner_dismissed(&mut s, "Odd"));
    assert_eq!(s.run("mail.dismissWarnings", &json!({"ids": [id]})).unwrap()["changed"], 1);
    assert!(banner_dismissed(&mut s, "Odd"));
    // Saved and read back, it stays dismissed.
    let saved = serde_json::to_string(&s.mailbox).unwrap();
    let mut again = Session::new(serde_json::from_str(&saved).unwrap());
    assert!(banner_dismissed(&mut again, "Odd"));
    // Undo shows it again; showing it again by command too.
    assert!(s.undo());
    assert!(!banner_dismissed(&mut s, "Odd"));
    s.run("mail.dismissWarnings", &json!({"ids": [id]})).unwrap();
    s.run("mail.dismissWarnings", &json!({"ids": [id], "value": false})).unwrap();
    assert!(!banner_dismissed(&mut s, "Odd"));
    // A message with nothing to warn of has no banner to dismiss.
    assert!(!banner_dismissed(&mut s, "Lunch"));
}

#[test]
fn a_new_kind_of_warning_brings_the_banner_back() {
    let (mut s, _) = imap(vec![mail("Lin", "lin@harborwork.example", "Odd")]);
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    let id = id_of(&s, "Odd");
    s.run("mail.dismissWarnings", &json!({"ids": [id]})).unwrap();
    assert!(banner_dismissed(&mut s, "Odd"));
    // The server's checks now fail for it: a kind that was not there.
    s.mailbox.message_mut(id).unwrap().headers =
        vec![("Authentication-Results".into(), "mx.lumen.example; dmarc=fail header.from=harborwork.example".into())];
    assert!(kinds(&mut s, "Odd").contains(&"failedCheck".to_string()));
    assert!(!banner_dismissed(&mut s, "Odd"));
}

#[test]
fn an_agent_may_not_hide_a_warning() {
    use crate::agent::{Access, Class, Verdict, access, check};
    assert_eq!(access("mail.dismissWarnings"), Some(Access::Owner(Class::Settings)));
    let (s, _) = imap(vec![mail("Lin", "lin@harborwork.example", "Odd")]);
    let params = json!({"ids": [id_of(&s, "Odd")]});
    assert!(matches!(check(&s, "mail.dismissWarnings", &params, &[]), Verdict::Ask(Class::Settings, _)));
}

/// Whoever mail in Sent Items went to is known while that mail is there: moved in, moved out,
/// undone, deleted for good, among many messages of other chunks.
#[test]
fn the_people_mail_went_to_are_known_while_it_is_in_sent_items() {
    // Enough mail that the messages lie in several chunks of the mailbox's map.
    let many: Vec<Message> = (0..2_500).map(|i| mail(&format!("sender{i}@lumen.example"), "avery@lumen.example", "Hello")).collect();
    let (mut s, account) = imap(many);
    let sent = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let who = "sol.reyes@harborworks.example";
    let first = s.mailbox.messages_in(inbox).map(|m| m.id).next().unwrap();
    // (Written with capitals and a space after it: known by the address as it is compared.)
    s.mailbox.message_mut(first).unwrap().to = vec![Address::new("Sol Reyes", "Sol.Reyes@Harborworks.example ")];
    assert!(!s.knows(who), "in the Inbox: not mail sent to them");
    let id = first;
    s.run("mail.move", &json!({"id": id, "folder": sent})).unwrap();
    assert!(s.knows(who), "in Sent Items");
    s.run("mail.move", &json!({"id": id, "folder": inbox})).unwrap();
    assert!(!s.knows(who), "moved out of it");
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.knows(who), "the move out undone");
    s.run("mail.deletePermanently", &json!({"id": id})).unwrap();
    assert!(!s.knows(who), "deleted for good");
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.knows(who), "and that undone");
}

/// AU3-14: a deceptive link that only a narrow pane, or only a dark window, shows is warned of
/// as one every pane shows; a link shown in no look at all is not; and an honest message with
/// the same style sheet says nothing.
#[test]
fn a_deceptive_link_one_look_alone_shows_is_warned_of() {
    let looks = |condition: &str, hidden_link: &str| {
        format!(
            "<style>.n{{display:none}} @media {condition} {{ .n{{display:block !important}} .w{{display:none !important}} }}</style>\
             <div class=\"w\"><a href=\"https://bank.example/login\">Sign in</a></div>\
             <div class=\"n\">{hidden_link}</div>"
        )
    };
    let deceptive = "<a href=\"https://evil.example/login\">https://bank.example/login</a>";
    let honest = "<a href=\"https://bank.example/login\">https://bank.example/login</a>";
    let message = |subject: &str, html: String| Message {
        body: Body { text: "Sign in".into(), html: Some(html) },
        ..mail("Bank", "notice@bank.example", subject)
    };
    let (mut s, _) = imap(vec![
        message("narrow", looks("(max-width: 600px)", deceptive)),
        message("dark", looks("(prefers-color-scheme: dark)", deceptive)),
        message("narrow and dark", looks("(max-width: 37.5em) and (prefers-color-scheme: dark)", deceptive)),
        message("print", looks("print", deceptive)),
        message("honest", looks("(max-width: 600px)", honest)),
    ]);
    for subject in ["narrow", "dark", "narrow and dark"] {
        assert!(kinds(&mut s, subject).contains(&"linkMismatch".to_string()), "{subject}");
    }
    // Shown in no pane: there is nothing to be deceived by.
    assert!(!kinds(&mut s, "print").contains(&"linkMismatch".to_string()));
    assert!(!kinds(&mut s, "honest").contains(&"linkMismatch".to_string()));
    // Every look's links, each once.
    let html = looks("(max-width: 600px)", deceptive);
    let (links, every) = crate::sender_warnings::links_in_every_look(&html);
    assert_eq!(links.len(), 2, "{links:?}");
    assert!(every);
    assert!(!kinds(&mut s, "narrow").contains(&"linksUnchecked".to_string()));
}

/// AU3-14: a message with more looks than are checked is said to be, so that a deceptive link in
/// a look past the last one checked never passes in silence: here twenty bands of width, the link
/// only between 800 and 850 pixels.
#[test]
fn a_message_with_more_looks_than_are_checked_says_so() {
    let mut css = String::from(".b{display:none}");
    let mut html = String::new();
    for i in 0..20 {
        let (from, to) = (50 * i, 50 * i + 49);
        css.push_str(&format!(" @media (min-width: {from}px) and (max-width: {to}px) {{ .b{i}{{display:block !important}} }}"));
        let link = if from == 800 {
            "<a href=\"https://evil.example/login\">https://bank.example/login</a>".to_string()
        } else {
            format!("<a href=\"https://bank.example/{i}\">Band {i}</a>")
        };
        html.push_str(&format!("<div class=\"b b{i}\">{link}</div>"));
    }
    let html = format!("<style>{css}</style>{html}");
    let (_, every) = crate::sender_warnings::links_in_every_look(&html);
    assert!(!every);
    let m = Message { body: Body { text: "Sign in".into(), html: Some(html) }, ..mail("Bank", "notice@bank.example", "bands") };
    let (mut s, _) = imap(vec![m]);
    let found = kinds(&mut s, "bands");
    assert!(found.contains(&"linksUnchecked".to_string()), "{found:?}");
}

/// R7, link warnings: a verdict the sender wrote, or one MailCraft left behind, takes no warning
/// away from a link (they are judged by the link alone).
#[test]
fn a_forged_verdict_takes_no_link_warning_away() {
    let html = "<p><a href=\"https://pay.evil.example/x\">https://harborworks.example/pay</a></p>";
    let mut forged = mail("Lin Zhao", "lin.zhao@harborworks.example", "Invoice");
    forged.headers = vec![
        ("Authentication-Results".into(), "mx.harborworks.example; dmarc=pass header.from=harborworks.example".into()),
        ("ARC-Authentication-Results".into(), "i=1; mx.lumen.example; dmarc=pass header.from=harborworks.example".into()),
    ];
    forged.body = Body { text: "Pay".into(), html: Some(html.into()) };
    let mut own = mail("Lin Zhao", "lin.zhao@harborworks.example", "Paid");
    own.headers = vec![("Authentication-Results".into(), "mx.lumen.example; dmarc=pass header.from=harborworks.example".into())];
    own.body = Body { text: "Pay".into(), html: Some(html.into()) };
    let (mut s, _) = imap(vec![forged, own]);
    assert!(kinds(&mut s, "Invoice").contains(&"linkMismatch".to_string()));
    assert!(kinds(&mut s, "Paid").contains(&"linkMismatch".to_string()));
}
