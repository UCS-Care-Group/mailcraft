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

#[test]
fn a_change_of_only_when_the_server_adds_it_keeps_what_the_owner_changed_here() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    s.run("signature.set", &json!({"name": NAME, "text": "Ada, edited here"})).unwrap();
    let before = s.mailbox.signature(NAME).unwrap().clone();
    // The owner ticks "add to replies" in Outlook on the web, and nothing else.
    let flags = EwsSignature { on_new: false, on_reply: true, ..server_signature() };
    apply_ews_mail(&mut s, result(account, Some(flags))).unwrap();
    assert_eq!(s.mailbox.signature(NAME), Some(&before), "the same words: the owner's edit stays");
    let a = s.mailbox.account(account).unwrap();
    assert_eq!((a.signature.as_str(), a.reply_signature()), (NAME, ""), "the defaults were the first import's");
}

#[test]
fn an_open_copy_with_no_changes_shows_what_came_in_and_saving_it_keeps_the_servers() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    s.run("signature.open", &json!({"name": NAME})).unwrap();
    let changed = EwsSignature { html: "<p>Ada L.</p>".into(), text: "Ada L.".into(), ..server_signature() };
    apply_ews_mail(&mut s, result(account, Some(changed))).unwrap();
    let open = s.signature_edit.as_ref().expect("still open");
    assert_eq!(open.body.plain().trim(), "Ada L.", "the editor shows the server's new one");
    s.run("signature.save", &json!({})).unwrap();
    assert_eq!(s.mailbox.signature(NAME).unwrap().plain().trim(), "Ada L.", "saving puts nothing old back");
}

#[test]
fn an_open_copy_with_changes_is_the_owners_to_save() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    s.run("signature.open", &json!({"name": NAME})).unwrap();
    s.run("compose.type", &json!({"text": " (mine)"})).unwrap();
    let typed = s.signature_edit.as_ref().unwrap().body.plain();
    let changed = EwsSignature { html: "<p>Ada L.</p>".into(), text: "Ada L.".into(), ..server_signature() };
    apply_ews_mail(&mut s, result(account, Some(changed))).unwrap();
    let open = s.signature_edit.as_ref().unwrap();
    assert!(open.dirty && open.body.plain() == typed, "what the owner is typing is not taken away");
    assert_eq!(s.mailbox.signature(NAME).unwrap().plain().trim(), "Ada L.", "the kept one is the server's until the owner saves");
}

#[test]
fn a_signature_of_the_owners_with_that_name_is_never_written_over() {
    let (mut s, account) = session();
    s.run("signature.set", &json!({"name": NAME, "text": "My own"})).unwrap();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    assert_eq!(s.mailbox.signature(NAME).unwrap().plain().trim(), "My own");
    let second = "ada@example.org (Exchange 2)";
    assert!(s.mailbox.signature(second).unwrap().plain().contains("Ada Lovelace"), "the server's is beside it");
    // And a later change of the server's is that one's.
    let changed = EwsSignature { html: "<p>Ada L.</p>".into(), text: "Ada L.".into(), ..server_signature() };
    apply_ews_mail(&mut s, result(account, Some(changed))).unwrap();
    assert_eq!(s.mailbox.signature(NAME).unwrap().plain().trim(), "My own");
    assert_eq!(s.mailbox.signature(second).unwrap().plain().trim(), "Ada L.");
    assert_eq!(s.mailbox.signatures.len(), 2);
}

#[test]
fn a_renamed_signature_is_followed_and_a_removed_one_comes_back_only_with_a_change_of_the_servers() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    s.run("signature.rename", &json!({"name": NAME, "to": "Work"})).unwrap();
    let changed = EwsSignature { html: "<p>Ada L.</p>".into(), text: "Ada L.".into(), ..server_signature() };
    apply_ews_mail(&mut s, result(account, Some(changed.clone()))).unwrap();
    assert_eq!(s.mailbox.signature("Work").unwrap().plain().trim(), "Ada L.", "renamed, it is still the server's");
    assert!(s.mailbox.signature(NAME).is_none() && s.mailbox.signatures.len() == 1);
    s.run("signature.remove", &json!({"name": "Work"})).unwrap();
    apply_ews_mail(&mut s, result(account, Some(changed))).unwrap();
    assert!(s.mailbox.signatures.is_empty(), "removed, and the server has nothing new");
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    assert!(s.mailbox.signature(NAME).is_some_and(|x| x.plain().contains("Ada Lovelace")), "the server's change is a new one");
}

#[test]
fn html_with_a_picture_the_server_keeps_apart_is_kept_as_rich_text_not_sent_as_written() {
    let (mut s, account) = session();
    let logo = EwsSignature {
        html: "<p><b>Ada Lovelace</b></p><p><img src=\"cid:image001.png@01DB0000.00000000\" alt=\"Logo\"></p>".into(),
        text: "Ada Lovelace".into(),
        on_new: true,
        on_reply: true,
    };
    apply_ews_mail(&mut s, result(account, Some(logo))).unwrap();
    let signature = s.mailbox.signature(NAME).unwrap();
    assert!(signature.html().is_none(), "no HTML that would go out with a broken picture");
    assert_eq!(signature.plain().trim(), "Ada Lovelace");
    assert!(signature.body().paragraphs().iter().flat_map(|p| &p.runs).any(|r| r.style.bold), "its look is kept");
    // A picture from the web is the HTML's own, as for a pasted signature.
    let web = EwsSignature { html: "<p>Ada <img src=\"https://example.org/logo.png\" alt=\"Logo\"></p>".into(), ..server_signature() };
    apply_ews_mail(&mut s, result(account, Some(web))).unwrap();
    assert!(s.mailbox.signature(NAME).unwrap().html().is_some_and(|h| h.contains("https://example.org/logo.png")));
}

#[test]
fn another_mailbox_starts_again_and_the_old_ones_signature_stays_the_owners() {
    let (mut s, account) = session();
    apply_ews_mail(&mut s, result(account, Some(server_signature()))).unwrap();
    // Another server is another mailbox: what the old one's server gave is forgotten with its states.
    let other = "https://other.example.org/EWS/Exchange.asmx";
    s.run("account.edit", &json!({"id": account, "ews": {"url": other}})).unwrap();
    let server = s.mailbox.account(account).unwrap().ews.clone().unwrap();
    assert_eq!((server.signature_seen, server.signature_name.as_str()), (0, ""));
    // So the same signature on the other server is taken in, beside the old one, which is not written.
    let mut again = result(account, Some(EwsSignature { html: "<p>Ada at the other</p>".into(), text: String::new(), ..server_signature() }));
    again.url = other.into();
    apply_ews_mail(&mut s, again).unwrap();
    assert!(s.mailbox.signature(NAME).unwrap().plain().contains("Analytical Engines"), "the old mailbox's stays as it was");
    assert_eq!(s.mailbox.signature("ada@example.org (Exchange 2)").unwrap().plain().trim(), "Ada at the other");
    assert_ne!(seen(&s, account), 0);
}

#[test]
fn a_run_signed_in_as_the_name_used_before_brings_in_no_signature() {
    let (mut s, account) = session();
    // A run is asked for with the address alone, and the user name is set while it is under way
    // (its mail isn't here yet, so it may be).
    s.run("account.edit", &json!({"id": account, "ews": {"url": URL, "username": "CORP\\ada"}})).unwrap();
    let late = EwsMailResult { username: ME.into(), ..result(account, Some(server_signature())) };
    let why = apply_ews_mail(&mut s, late).unwrap_err();
    assert!(why.to_string().contains("another account"), "{why}");
    assert!(s.mailbox.signature(NAME).is_none(), "the other name's signature isn't taken in");
    assert_eq!(seen(&s, account), 0);
    // One signed in as the name it has now is taken in (the case of a name is not another name).
    apply_ews_mail(&mut s, EwsMailResult { username: "corp\\ADA".into(), ..result(account, Some(server_signature())) }).unwrap();
    assert!(s.mailbox.signature(NAME).is_some());
}

#[test]
fn html_that_shows_nothing_is_not_sent_as_written_and_the_text_goes_instead() {
    let (mut s, account) = session();
    let blank = EwsSignature { html: "<div><p>&nbsp;</p><p></p></div>".into(), text: "Ada Lovelace".into(), on_new: true, on_reply: true };
    apply_ews_mail(&mut s, result(account, Some(blank))).unwrap();
    let signature = s.mailbox.signature(NAME).unwrap();
    assert!(signature.html().is_none(), "no blank HTML for recipients who read HTML");
    assert_eq!(signature.plain().trim(), "Ada Lovelace");
}
