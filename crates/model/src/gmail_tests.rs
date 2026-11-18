// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's inbox categories (#911).

use crate::search::Query;
use crate::sync::{FolderDelta, GmailSorted};
use crate::*;

#[test]
fn codes_read_back_and_only_four_are_searched() {
    for c in GmailCategory::ALL {
        assert_eq!(GmailCategory::parse(c.code()), Some(c));
        assert_eq!(GmailCategory::parse(&format!(" {} ", c.code().to_uppercase())), Some(c));
        assert_eq!(serde_json::to_value(c).ok(), Some(serde_json::json!(c.code())));
    }
    assert_eq!(GmailCategory::parse("personal"), None);
    assert_eq!(GmailCategory::parse(""), None);
    assert_eq!(GmailCategory::Primary.query(), None);
    let searched: Vec<&str> = GmailCategory::SEARCHED.iter().filter_map(|c| c.query()).collect();
    assert_eq!(searched, ["category:social", "category:promotions", "category:updates", "category:forums"]);
}

#[test]
fn gmail_is_known_by_its_server() {
    for host in ["imap.gmail.com", "IMAP.GMAIL.COM.", " imap.googlemail.com ", "gmail.com"] {
        assert!(is_gmail_host(host), "{host}");
    }
    for host in ["", "imap.gmail.com.example.org", "notgmail.com", "imap.example.org", "gmail.co", "outlook.office365.com"] {
        assert!(!is_gmail_host(host), "{host}");
    }
    let mut account = Account { incoming: Some(Server { host: "imap.gmail.com".into(), ..Default::default() }), ..Default::default() };
    assert!(account.is_gmail() && account.shows_gmail_categories());
    account.hide_gmail_categories = true;
    assert!(account.is_gmail() && !account.shows_gmail_categories());
    let other = Account { incoming: Some(Server { host: "imap.example.org".into(), ..Default::default() }), ..Default::default() };
    assert!(!other.is_gmail() && !other.shows_gmail_categories());
    assert!(!Account::default().is_gmail());
}

#[test]
fn a_file_from_before_or_with_something_else_still_opens() {
    // From before: nothing said, so no category, and the account shows them and alerts for all.
    let m: Message = serde_json::from_str(r#"{"id": 1, "subject": "Hi"}"#).unwrap();
    assert_eq!((m.gmail_category, m.gmail_id), (None, 0));
    let a: Account = serde_json::from_str(r#"{"id": 1, "email": "ada@gmail.example"}"#).unwrap();
    assert!(!a.hide_gmail_categories && !a.alert_primary_only);
    // Something this version doesn't know is nothing, and the rest of the message is read.
    let m: Message = serde_json::from_str(r#"{"id": 1, "subject": "Hi", "gmailCategory": "spam", "gmailId": "x"}"#).unwrap();
    assert_eq!((m.gmail_category, m.gmail_id, m.subject.as_str()), (None, 0, "Hi"));
    let a: Account = serde_json::from_str(r#"{"id": 1, "hideGmailCategories": 3, "alertPrimaryOnly": "yes"}"#).unwrap();
    assert!(!a.hide_gmail_categories && !a.alert_primary_only);
    // And what is said is kept, Gmail's 64-bit ids whole.
    let m = Message { gmail_category: Some(GmailCategory::Promotions), gmail_id: u64::MAX, ..Default::default() };
    let back: Message = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
    assert_eq!((back.gmail_category, back.gmail_id), (Some(GmailCategory::Promotions), u64::MAX));
    // A message of any other account writes nothing of it.
    let plain = serde_json::to_string(&Message::default()).unwrap();
    assert!(!plain.contains("gmail"), "{plain}");
}

#[test]
fn a_delta_says_categories_only_when_it_has_them() {
    let delta = FolderDelta { remote: "INBOX".into(), ..Default::default() };
    assert!(!serde_json::to_string(&delta).unwrap().contains("gmail"));
    let delta = FolderDelta { gmail: vec![GmailSorted { uid: 4, category: GmailCategory::Social, id: 77 }], ..delta };
    let back: FolderDelta = serde_json::from_str(&serde_json::to_string(&delta).unwrap()).unwrap();
    assert_eq!(back.gmail, delta.gmail);
}

#[test]
fn the_search_box_finds_a_gmail_category_and_not_the_owners() {
    let social = Message { gmail_category: Some(GmailCategory::Social), categories: vec!["Primary".into()], ..Default::default() };
    let none = Message { categories: vec!["Social".into()], ..Default::default() };
    let hit = |q: &str, m: &Message| Query::parse(q).matches(m, 0);
    assert!(hit("gmail:social", &social) && !hit("gmail:social", &none));
    assert!(hit("GMAIL:Social", &social));
    assert!(!hit("gmail:primary", &social) && hit("-gmail:primary", &social));
    // `category:` stays the owner's own.
    assert!(hit("category:social", &none) && !hit("category:social", &social));
    // A value that is none of the five is text.
    assert!(!hit("gmail:spam", &social));
    assert_eq!(search::keyword_spans("gmail:updates").len(), 1);
    assert!(search::keyword_spans("gmail:spam").is_empty());
    // What `gmail:` takes is offered.
    let offered: Vec<String> = search::suggestions("gmail:p", 7, &[]).into_iter().map(|s| s.text).collect();
    assert_eq!(offered, ["gmail:primary ", "gmail:promotions "]);
}

#[test]
fn each_category_has_its_label_for_gmails_api() {
    let labels: Vec<&str> = GmailCategory::ALL.iter().map(|c| c.api_label()).collect();
    assert_eq!(labels, ["CATEGORY_PERSONAL", "CATEGORY_SOCIAL", "CATEGORY_PROMOTIONS", "CATEGORY_UPDATES", "CATEGORY_FORUMS"]);
}

#[test]
fn only_a_gmail_account_signed_in_with_google_moves_between_categories() {
    let server = |host: &str| Some(Server { host: host.into(), port: 993, security: Security::Tls });
    let google = Some(OAuth { provider: OAuthProvider::Google, ..Default::default() });
    let gmail = Account { incoming: server("imap.gmail.com"), ..Default::default() };
    assert!(!gmail.moves_gmail_categories(), "an app password");
    assert!(Account { oauth: google.clone(), ..gmail.clone() }.moves_gmail_categories());
    let microsoft = Some(OAuth { provider: OAuthProvider::Microsoft, ..Default::default() });
    assert!(!Account { oauth: microsoft, ..gmail }.moves_gmail_categories());
    assert!(!Account { incoming: server("imap.mail.example"), oauth: google, ..Default::default() }.moves_gmail_categories());
}

#[test]
fn a_move_waits_while_the_category_here_isnt_what_gmail_said() {
    let m = Message { gmail_category: Some(GmailCategory::Social), gmail_synced: Some(GmailCategory::Primary), gmail_id: 7, ..Default::default() };
    assert_eq!(m.gmail_move_waiting(), Some(GmailCategory::Social));
    // Not without Gmail's id, nor once the two agree, nor before Gmail has said anything.
    assert_eq!(Message { gmail_id: 0, ..m.clone() }.gmail_move_waiting(), None);
    assert_eq!(Message { gmail_synced: Some(GmailCategory::Social), ..m.clone() }.gmail_move_waiting(), None);
    assert_eq!(Message { gmail_synced: None, ..m.clone() }.gmail_move_waiting(), None);
    // Leaving the Inbox forgets it.
    let mut left = m;
    left.left_gmail_inbox();
    assert_eq!((left.gmail_category, left.gmail_synced, left.gmail_id), (None, None, 0));
}
