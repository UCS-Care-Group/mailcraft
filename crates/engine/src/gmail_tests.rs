// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's inbox categories (#911): what a synchronisation brings, the five views of the Inbox,
//! the search, what agents see and the alerts.

use mailcraft_model::sync::{FolderDelta, GmailSorted, SyncResult};
use mailcraft_model::{Address, FolderKind, GmailCategory, Id, Message};
use serde_json::json;

use crate::Session;
use crate::arrivals::{arrivals, mark};

/// A session with a Gmail account (by its server's name); its id and its Inbox's.
fn gmail_session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let servers = json!({"email": "ada@mail.example", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}});
    let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    (s, account, inbox)
}

fn message(uid: u32, subject: &str, category: Option<GmailCategory>) -> Message {
    Message {
        uid,
        from: Address::new("Rowan", "rowan@sender.example"),
        subject: subject.into(),
        message_id: format!("<{uid}@sender.example>"),
        date: 1_700_000_000 + i64::from(uid),
        gmail_category: category,
        gmail_id: category.map_or(0, |_| 9_000 + u64::from(uid)),
        ..Default::default()
    }
}

fn sorted(uid: u32, category: GmailCategory) -> GmailSorted {
    GmailSorted { uid, category, id: 9_000 + u64::from(uid) }
}

fn inbox_delta(inbox: Id, new: Vec<Message>, gmail: Vec<GmailSorted>) -> FolderDelta {
    let uid_seen = new.iter().map(|m| m.uid).max().unwrap_or(0);
    FolderDelta { id: Some(inbox), remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen, new, gmail, ..Default::default() }
}

fn apply(s: &mut Session, account: Id, folders: Vec<FolderDelta>) {
    s.run("sync.apply", &json!({"result": SyncResult { account, folders, ..Default::default() }})).unwrap();
}

fn by_uid(s: &Session, account: Id, uid: u32) -> &Message {
    s.mailbox.messages.values().find(|m| m.account == account && m.uid == uid).unwrap()
}

/// The subjects the list in view shows, row by row.
fn listed(s: &Session) -> Vec<String> {
    crate::list::rows(s).iter().filter_map(|r| s.mailbox.message(r.id)).map(|m| m.subject.clone()).collect()
}

/// An Inbox of a Gmail account with mail in four of its categories.
fn sorted_inbox() -> (Session, Id, Id) {
    let (mut s, account, inbox) = gmail_session();
    let new = vec![
        message(1, "Lunch", Some(GmailCategory::Primary)),
        message(2, "Party photos", Some(GmailCategory::Social)),
        message(3, "Sale", Some(GmailCategory::Promotions)),
        message(4, "Your order shipped", Some(GmailCategory::Updates)),
    ];
    apply(&mut s, account, vec![inbox_delta(inbox, new, Vec::new())]);
    (s, account, inbox)
}

#[test]
fn a_synchronisation_sorts_the_inbox_as_gmail_says() {
    let (mut s, account, inbox) = sorted_inbox();
    assert_eq!(by_uid(&s, account, 2).gmail_category, Some(GmailCategory::Social));
    // Gmail sorts mail again, and gives an id to a message that had none.
    let mut older = message(5, "Old", None);
    older.gmail_id = 0;
    apply(&mut s, account, vec![inbox_delta(inbox, vec![older], Vec::new())]);
    let gmail = vec![sorted(2, GmailCategory::Forums), sorted(5, GmailCategory::Updates), sorted(77, GmailCategory::Social)];
    apply(&mut s, account, vec![inbox_delta(inbox, Vec::new(), gmail)]);
    assert_eq!(by_uid(&s, account, 2).gmail_category, Some(GmailCategory::Forums));
    assert_eq!((by_uid(&s, account, 5).gmail_category, by_uid(&s, account, 5).gmail_id), (Some(GmailCategory::Updates), 9_005));
    // What isn't said stays as it was.
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Promotions));
    // Only the Inbox is sorted: news of another folder's is passed over.
    let archive = s.mailbox.add_folder(account, None, "Kept").unwrap();
    s.mailbox.folder_mut(archive).unwrap().remote = "Kept".into();
    let other = FolderDelta {
        id: Some(archive),
        remote: "Kept".into(),
        kind: FolderKind::Custom,
        uid_validity: 9,
        uid_seen: 1,
        new: vec![message(1, "Filed", None)],
        gmail: vec![sorted(1, GmailCategory::Social)],
        ..Default::default()
    };
    apply(&mut s, account, vec![other]);
    let filed = s.mailbox.messages.values().find(|m| m.folder == archive).unwrap();
    assert_eq!(filed.gmail_category, None);
}

#[test]
fn each_category_lists_its_part_of_the_inbox() {
    let (mut s, _, inbox) = sorted_inbox();
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    assert_eq!(listed(&s).len(), 4);
    for (code, subject) in [("primary", "Lunch"), ("social", "Party photos"), ("promotions", "Sale"), ("updates", "Your order shipped")] {
        let answer = s.run("folder.select", &json!({"id": inbox, "gmail": code})).unwrap();
        assert_eq!((answer["gmail"].as_str(), answer["gmailMessages"].as_u64(), answer["gmailUnread"].as_u64()), (Some(code), Some(1), Some(1)));
        assert_eq!(listed(&s), [subject], "{code}");
    }
    s.run("folder.select", &json!({"id": inbox, "gmail": "forums"})).unwrap();
    assert!(listed(&s).is_empty());
    // Going to the Inbox itself lists all of it again.
    s.run("folder.select", &json!({"kind": "inbox"})).unwrap();
    assert_eq!((s.view.gmail, listed(&s).len()), (None, 4));
    // Counted for the folder pane, unread and all.
    let counts = s.folder_counts();
    assert_eq!((counts.gmail(inbox, GmailCategory::Social).total, counts.gmail(inbox, GmailCategory::Forums).total), (1, 0));
}

#[test]
fn a_read_mark_is_counted_in_its_category() {
    let (mut s, account, inbox) = sorted_inbox();
    let social = by_uid(&s, account, 2).id;
    assert_eq!(s.folder_counts().gmail(inbox, GmailCategory::Social).unread, 1);
    s.run("mail.markRead", &json!({"ids": [social], "value": true})).unwrap();
    assert_eq!(s.folder_counts().gmail(inbox, GmailCategory::Social).unread, 0);
    assert_eq!(s.folder_counts(), std::sync::Arc::new(crate::counts::FolderCounts::of(&s.mailbox)));
}

#[test]
fn a_conversation_is_listed_in_a_category_when_one_of_its_messages_is() {
    let (mut s, account, inbox) = sorted_inbox();
    let mut reply = message(6, "Re: Party photos", Some(GmailCategory::Primary));
    reply.in_reply_to = "<2@sender.example>".into();
    reply.references = vec!["<2@sender.example>".into()];
    apply(&mut s, account, vec![inbox_delta(inbox, vec![reply], Vec::new())]);
    s.run("view.conversations", &json!({"value": true})).unwrap();
    s.run("folder.select", &json!({"id": inbox, "gmail": "social"})).unwrap();
    let rows = crate::list::rows(&s);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].members.len(), 2, "the whole conversation, the reply in Primary too");
    // And in Primary, where the reply is.
    s.run("folder.select", &json!({"id": inbox, "gmail": "primary"})).unwrap();
    let rows = crate::list::rows(&s);
    assert_eq!(rows.len(), 2, "Lunch, and the conversation");
}

#[test]
fn only_a_gmail_inbox_with_categories_shown_has_them() {
    let (mut s, account, inbox) = sorted_inbox();
    assert!(crate::gmail::listed_under(&mut s, inbox));
    let sent = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    assert!(!crate::gmail::listed_under(&mut s, sent));
    assert!(s.run("folder.select", &json!({"id": sent, "gmail": "social"})).is_err());
    assert!(s.run("folder.select", &json!({"id": inbox, "gmail": "spam"})).is_err());
    assert!(s.run("folder.select", &json!({"id": inbox, "gmail": 3})).is_err());
    // Turned off: not listed, and a category chosen before lists the whole Inbox.
    s.run("folder.select", &json!({"id": inbox, "gmail": "social"})).unwrap();
    s.run("account.edit", &json!({"id": account, "showGmailCategories": false})).unwrap();
    assert!(!crate::gmail::listed_under(&mut s, inbox));
    assert_eq!(listed(&s).len(), 4);
    assert!(s.run("folder.select", &json!({"id": inbox, "gmail": "social"})).is_err());
    let shown = s.run("account.list", &json!({})).unwrap();
    assert_eq!(shown["accounts"][0]["showGmailCategories"], json!(false));
    s.run("account.edit", &json!({"id": account, "showGmailCategories": true})).unwrap();
    assert!(crate::gmail::listed_under(&mut s, inbox));
    assert!(s.run("account.edit", &json!({"id": account, "showGmailCategories": "yes"})).is_err());

    // An Inbox where everything is in Primary has nothing to sort: none are listed.
    let (mut s, account, inbox) = gmail_session();
    apply(&mut s, account, vec![inbox_delta(inbox, vec![message(1, "Lunch", Some(GmailCategory::Primary))], Vec::new())]);
    assert!(!crate::gmail::listed_under(&mut s, inbox));
    // Nor has another server's.
    let mut s = Session::empty();
    let servers = json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    apply(&mut s, account, vec![inbox_delta(inbox, vec![message(1, "Sale", Some(GmailCategory::Promotions))], Vec::new())]);
    assert!(!crate::gmail::listed_under(&mut s, inbox));
}

#[test]
fn agents_see_the_category_of_gmail_mail_alone() {
    let (s, account, _) = sorted_inbox();
    let social = by_uid(&s, account, 2);
    assert_eq!(crate::cmd::message_summary(&s, social)["gmailCategory"], json!("social"));
    let plain = Message::default();
    assert!(crate::cmd::message_summary(&s, &plain).get("gmailCategory").is_none());
}

#[test]
fn the_search_finds_a_gmail_category() {
    let (mut s, _, inbox) = sorted_inbox();
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    s.run("view.search", &json!({"query": "gmail:promotions"})).unwrap();
    assert_eq!(listed(&s), ["Sale"]);
    s.run("view.search", &json!({"query": "category:promotions"})).unwrap();
    assert!(listed(&s).is_empty(), "the owner's own categories are another thing");
}

#[test]
fn alerts_for_primary_alone_when_the_owner_asks() {
    let (mut s, account, inbox) = sorted_inbox();
    let shown = s.run("account.list", &json!({})).unwrap();
    assert_eq!((shown["accounts"][0]["gmail"].clone(), shown["accounts"][0]["alertPrimaryOnly"].clone()), (json!(true), json!(false)));
    let arriving = || {
        vec![
            message(10, "Dinner?", Some(GmailCategory::Primary)),
            message(11, "Flash sale", Some(GmailCategory::Promotions)),
            message(12, "Not sorted yet", None),
        ]
    };
    let before = mark(&s.mailbox);
    apply(&mut s, account, vec![inbox_delta(inbox, arriving(), Vec::new())]);
    assert_eq!(arrivals(&s.mailbox, &before).len(), 3, "all of it, as for any account");

    let (mut s, account, inbox) = sorted_inbox();
    s.run("account.edit", &json!({"id": account, "alertPrimaryOnly": true})).unwrap();
    let before = mark(&s.mailbox);
    apply(&mut s, account, vec![inbox_delta(inbox, arriving(), Vec::new())]);
    let told: Vec<String> = arrivals(&s.mailbox, &before).into_iter().map(|a| a.subject).collect();
    assert_eq!(told, ["Dinner?", "Not sorted yet"]);
    assert!(s.run("account.edit", &json!({"id": account, "alertPrimaryOnly": 1})).is_err());
}

#[test]
fn mail_that_leaves_the_inbox_leaves_its_category() {
    let (mut s, account, _) = sorted_inbox();
    let social = by_uid(&s, account, 2).id;
    let kept = s.mailbox.add_folder(account, None, "Kept").unwrap();
    s.run("mail.copy", &json!({"ids": [social], "folder": kept})).unwrap();
    s.run("mail.move", &json!({"ids": [social], "folder": kept})).unwrap();
    let filed: Vec<&Message> = s.mailbox.messages.values().filter(|m| m.folder == kept).map(|m| m.as_ref()).collect();
    assert_eq!(filed.len(), 2);
    for m in filed {
        assert_eq!((m.gmail_category, m.gmail_id), (None, 0), "{}", m.subject);
    }
    s.run("view.search", &json!({"query": "gmail:social", "scope": "mailbox"})).unwrap();
    assert!(listed(&s).is_empty());
    assert_eq!(s.folder_counts(), std::sync::Arc::new(crate::counts::FolderCounts::of(&s.mailbox)));
}

#[test]
fn a_category_no_longer_listed_shows_the_whole_inbox() {
    let (mut s, account, inbox) = sorted_inbox();
    s.run("folder.select", &json!({"id": inbox, "gmail": "social"})).unwrap();
    // Gmail puts everything in Primary: the five rows go, and the list is the whole Inbox.
    let gmail = (1..=4).map(|uid| sorted(uid, GmailCategory::Primary)).collect();
    apply(&mut s, account, vec![inbox_delta(inbox, Vec::new(), gmail)]);
    assert!(!crate::gmail::listed_under(&mut s, inbox));
    assert_eq!((crate::gmail::in_view(&s), listed(&s).len()), (None, 4));
    // And none can be chosen while they aren't listed.
    assert!(s.run("folder.select", &json!({"id": inbox, "gmail": "primary"})).is_err());
}

#[test]
fn a_message_opened_from_elsewhere_shows_its_whole_folder() {
    let (mut s, account, inbox) = sorted_inbox();
    let primary = by_uid(&s, account, 1).id;
    s.run("folder.select", &json!({"id": inbox, "gmail": "social"})).unwrap();
    s.run("mail.select", &json!({"id": primary, "elsewhere": true})).unwrap();
    assert_eq!((s.view.folder, s.view.gmail), (inbox, None));
    assert!(listed(&s).contains(&"Lunch".to_string()));
}

#[test]
fn the_category_in_view_is_not_kept_between_sessions() {
    let (mut s, _, inbox) = sorted_inbox();
    s.run("folder.select", &json!({"id": inbox, "gmail": "social"})).unwrap();
    let saved = serde_json::to_value(&s.view).unwrap();
    assert!(saved.get("gmail").is_none(), "{saved}");
    // The list the window keeps follows the category all the same.
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    assert_eq!(s.rows().len(), 4);
    s.run("folder.select", &json!({"id": inbox, "gmail": "social"})).unwrap();
    assert_eq!(s.rows().len(), 1);
}

/// View › Gmail Categories (#911): folders until the owner chooses tabs, kept with the rest of the
/// view's settings, and offered only while a Gmail account shows its categories.
#[test]
fn gmail_categories_are_folders_until_tabs_are_chosen() {
    let mut empty = Session::empty();
    assert!(!empty.view.gmail_tabs);
    assert!(crate::gmail::offered(&empty).is_some());
    assert!(!empty.enabled("view.gmailCategories"));
    assert!(empty.run("view.gmailCategories", &json!({"value": "tabs"})).is_err());

    let (mut s, account, inbox) = sorted_inbox();
    assert!(crate::gmail::offered(&s).is_none() && s.enabled("view.gmailCategories"));
    assert!(!crate::gmail::tabs_shown(&mut s), "folders by default");
    assert!(s.run("view.gmailCategories", &json!({"value": "sideways"})).is_err());
    assert!(s.run("view.gmailCategories", &json!({})).is_err());
    assert_eq!(s.run("view.gmailCategories", &json!({"value": "tabs"})).unwrap()["value"], "tabs");
    assert!(s.view.gmail_tabs);
    // Tabs only over that Inbox's list, while Mail is in front.
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    assert!(crate::gmail::tabs_shown(&mut s));
    s.run("view.calendar", &json!({})).unwrap();
    assert!(!crate::gmail::tabs_shown(&mut s));
    let sent = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    s.run("folder.select", &json!({"id": sent})).unwrap();
    assert!(!crate::gmail::tabs_shown(&mut s));
    // A tab selects as the folder pane's row does, through the same command.
    s.run("folder.select", &json!({"id": inbox, "gmail": "social"})).unwrap();
    assert_eq!(crate::gmail::in_view(&s), Some(GmailCategory::Social));
    assert_eq!(listed(&s), ["Party photos"]);
    // Kept with the view's settings, as the reading pane is.
    let saved: crate::ViewState = serde_json::from_value(serde_json::to_value(&s.view).unwrap()).unwrap();
    assert!(saved.gmail_tabs && saved.gmail.is_none());
    // An account whose owner hid the categories has no tabs either, and nothing offers the choice.
    s.run("account.edit", &json!({"id": account, "showGmailCategories": false})).unwrap();
    assert!(!crate::gmail::tabs_shown(&mut s));
    assert!(crate::gmail::offered(&s).is_some());
    s.run("account.edit", &json!({"id": account, "showGmailCategories": true})).unwrap();
    assert_eq!(s.run("view.gmailCategories", &json!({"value": "folders"})).unwrap()["value"], "folders");
    assert!(!crate::gmail::tabs_shown(&mut s));
}
