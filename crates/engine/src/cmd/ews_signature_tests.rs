// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The signature an Exchange server keeps for its owner, taken in (#484): what is made of it, what
//! the owner's own choices keep, and the whole way from a run's result to the mailbox.

use mailcraft_model::sync::{EwsMailResult, EwsSignature, MAX_SIGNATURE_HTML};
use serde_json::json;

use crate::Session;
use crate::cmd::ews_mail::{apply_ews_mail, ews_inbox_request, ews_mail_request};

const ME: &str = "ada@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";
const NAME: &str = "ada@example.org (Exchange)";

fn session() -> (Session, u64) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": URL}})).unwrap()["id"].as_u64().unwrap();
    (s, account)
}

fn server_signature() -> EwsSignature {
    EwsSignature {
        html: "<div style=\"font-family:Calibri\">Ada Lovelace<br><b>Analytical Engines</b></div>".into(),
        text: "Ada Lovelace\nAnalytical Engines".into(),
        on_new: true,
        on_reply: false,
    }
}

fn result(account: u64, signature: Option<EwsSignature>) -> EwsMailResult {
    EwsMailResult { account, email: ME.into(), url: URL.into(), signature, ..Default::default() }
}

fn seen(s: &Session, account: u64) -> u64 {
    s.mailbox.account(account).and_then(|a| a.ews.as_ref()).map(|e| e.signature_seen).unwrap()
}

#[test]
fn the_servers_signature_becomes_a_signature_of_the_mailbox_and_the_accounts_choice_the_first_time() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    let signature = s.mailbox.signature(NAME).expect("a signature named for the account");
    assert!(signature.plain().contains("Ada Lovelace") && signature.plain().contains("Analytical Engines"), "{:?}", signature.plain());
    assert!(signature.html().is_some_and(|h| h.contains("<b>Analytical Engines</b>")), "the owner's own HTML is kept");
    let a = s.mailbox.account(account).unwrap();
    // On for new mail, off for replies, as the server's options say.
    assert_eq!((a.signature.as_str(), a.reply_signature()), (NAME, ""));
    assert_ne!(seen(&s, account), 0);
}

#[test]
fn the_same_signature_again_changes_nothing_and_what_the_owner_changed_here_stays() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    s.run("signature.set", &json!({"name": NAME, "text": "Ada, edited here"})).unwrap();
    let before = s.mailbox.signature(NAME).unwrap().clone();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    assert_eq!(s.mailbox.signature(NAME), Some(&before), "the server said nothing new");
    assert_eq!(s.mailbox.signatures.len(), 1);
}

#[test]
fn a_signature_the_server_changed_comes_in_again_but_the_owners_choice_of_defaults_stays() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    // The owner picks other defaults for the account.
    s.run("signature.set", &json!({"name": "Mine", "text": "Mine"})).unwrap();
    s.run("signature.defaults", &json!({"account": account, "new": "Mine", "reply": "Mine"})).unwrap();
    let changed = EwsSignature { html: "<p>Ada L.</p>".into(), text: "Ada L.".into(), on_new: false, on_reply: true };
    apply_ews_mail(&mut s, result(account, Some(changed))).unwrap();
    assert_eq!(s.mailbox.signature(NAME).unwrap().plain().trim(), "Ada L.");
    let a = s.mailbox.account(account).unwrap();
    assert_eq!((a.signature.as_str(), a.reply_signature()), ("Mine", "Mine"), "the first import's defaults only");
}

#[test]
fn an_account_that_already_has_a_signature_chosen_keeps_it() {
    let (mut s, account) = session();
    s.run("signature.set", &json!({"name": "Mine", "text": "Mine"})).unwrap();
    s.run("signature.defaults", &json!({"account": account, "new": "Mine"})).unwrap();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    assert!(s.mailbox.signature(NAME).is_some());
    assert_eq!(s.mailbox.account(account).unwrap().signature, "Mine");
}

#[test]
fn a_server_without_one_adds_nothing_and_removes_nothing() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, None)).unwrap();
    assert!(s.mailbox.signatures.is_empty() && seen(&s, account) == 0);
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    apply_ews_mail(&mut s, result(account, None)).unwrap();
    apply_ews_mail(&mut s, result(account, Some(EwsSignature::default()))).unwrap();
    assert!(s.mailbox.signature(NAME).is_some(), "one the server no longer has is not taken away");
}

#[test]
fn a_signature_past_the_sizes_is_refused_with_the_whole_result() {
    let (mut s, account) = session();
    let big = EwsSignature { html: "x".repeat(MAX_SIGNATURE_HTML + 1), ..server_signature() };
    assert!(apply_ews_mail(&mut s, result(account, Some(big))).is_err());
    assert!(s.mailbox.signatures.is_empty());
}

#[test]
fn html_that_is_a_picture_alone_shows_the_text_the_server_gave_with_it() {
    let (mut s, account) = session();
    let logo = EwsSignature { html: "<img src=\"cid:logo\" alt=\"\">".into(), text: "Ada Lovelace".into(), on_new: true, on_reply: true };
    apply_ews_mail(&mut s, result(account, Some(logo))).unwrap();
    assert_eq!(s.mailbox.signature(NAME).unwrap().plain().trim(), "Ada Lovelace");
}

#[test]
fn hostile_html_is_read_as_any_pasted_signature_is() {
    let (mut s, account) = session();
    let hostile = EwsSignature {
        html: "<script>alert(1)</script><p onclick=\"x()\">Ada</p><img src=\"http://tracker.example/p.png\">".into(),
        text: String::new(),
        on_new: true,
        on_reply: true,
    };
    apply_ews_mail(&mut s, result(account, Some(hostile))).unwrap();
    let plain = s.mailbox.signature(NAME).unwrap().plain();
    assert!(plain.contains("Ada") && !plain.contains("alert"), "{plain:?}");
}

#[test]
fn another_accounts_signature_of_the_same_name_is_not_touched() {
    let (mut s, account) = session();
    s.run("signature.set", &json!({"name": "other@example.org (Exchange)", "text": "Other"})).unwrap();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    assert_eq!(s.mailbox.signature("other@example.org (Exchange)").unwrap().plain().trim(), "Other");
    assert_eq!(s.mailbox.signatures.len(), 2);
}

#[test]
fn only_a_run_of_every_folder_asks_for_it() {
    let (s, account) = session();
    assert!(ews_mail_request(&s, account, false).unwrap().signature);
    assert!(!ews_mail_request(&s, account, true).unwrap().signature);
    assert!(ews_inbox_request(&s, account).is_none_or(|request| !request.signature));
}

#[test]
fn what_was_seen_survives_a_save_and_a_reopen_so_an_unchanged_signature_is_not_taken_in_twice() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    let json = serde_json::to_string(&s.mailbox.account(account).unwrap().ews).unwrap();
    let back: Option<mailcraft_model::EwsServer> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.map(|e| e.signature_seen), Some(seen(&s, account)));
    // A server written by an older version (no such field) opens as nothing seen.
    let old: mailcraft_model::EwsServer = serde_json::from_str(r#"{"url":"https://exchange.example.org/EWS/Exchange.asmx"}"#).unwrap();
    assert_eq!(old.signature_seen, 0);
}
