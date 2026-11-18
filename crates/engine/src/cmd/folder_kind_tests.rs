// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1200: a standard folder is named by its kind (`inbox`, `sent`…) in `mail.list` and
//! `folder.select`, whatever the mailbox's language calls it; a path still works, and digits
//! that name no folder are an id.

use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use crate::{Session, sample};

const NOW: i64 = 1_791_993_600;

/// The sample mailbox with its standard folders named as a German Microsoft 365 mailbox names
/// them, and the account's Inbox and Sent Items.
fn german() -> (Session, Id, Id) {
    let mut s = Session::at(sample::mailbox(NOW, 0), NOW, 0);
    let account = s.mailbox.default_account().unwrap().id;
    let names = [(FolderKind::Inbox, "Posteingang"), (FolderKind::Sent, "Gesendete Elemente"), (FolderKind::Deleted, "Gelöschte Elemente")];
    for (kind, name) in names {
        let id = s.mailbox.folder_of_kind(account, kind).unwrap();
        s.mailbox.folder_mut(id).unwrap().name = name.into();
    }
    let (inbox, sent) = (s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap());
    (s, inbox, sent)
}

/// The folder `mail.list` read for these params: its messages' folder.
fn listed_folder(s: &mut Session, params: Value) -> Id {
    let listed = s.run("mail.list", &params).unwrap_or_else(|e| panic!("{params}: {e}"));
    let ids: Vec<Id> = listed["messages"].as_array().unwrap().iter().map(|m| m["id"].as_u64().unwrap()).collect();
    let folders: std::collections::BTreeSet<Id> = ids.iter().map(|id| s.mailbox.message(*id).unwrap().folder).collect();
    assert_eq!(folders.len(), 1, "{params}: one folder's list, {folders:?}");
    folders.into_iter().next().unwrap()
}

/// The issue's case: English names on a German mailbox reach its folders by their kind, in any
/// case, and the window's folder stays where it was.
#[test]
fn a_kind_reaches_the_folder_whatever_it_is_called() {
    let (mut s, inbox, sent) = german();
    let in_view = s.view.folder;
    assert_eq!(listed_folder(&mut s, json!({"folder": "Inbox"})), inbox);
    assert_eq!(listed_folder(&mut s, json!({"folder": "sent"})), sent);
    assert_eq!(listed_folder(&mut s, json!({"folder": " SENT "})), sent);
    assert_eq!(s.view.folder, in_view, "mail.list doesn't move the window");
    // The names it has still work, as paths.
    assert_eq!(listed_folder(&mut s, json!({"folder": "Gesendete Elemente"})), sent);
    // And folder.select takes the same.
    s.run("folder.select", &json!({"folder": "sent"})).unwrap();
    assert_eq!(s.view.folder, sent);
    s.run("folder.select", &json!({"folder": inbox})).unwrap();
    assert_eq!(s.view.folder, inbox);
}

/// A folder called 2024 is that folder, not the folder whose id is 2024; digits that name no
/// folder are an id, so `--folder 13` from a command line reaches folder 13.
#[test]
fn a_path_comes_before_digits_and_digits_are_an_id() {
    let (mut s, _, sent) = german();
    let account = s.mailbox.default_account().unwrap().id;
    let year = s.mailbox.add_folder(account, None, "2024").unwrap();
    s.mailbox.add_message(mailcraft_model::Message { folder: year, account, subject: "Old".into(), ..Default::default() }).unwrap();
    assert_eq!(listed_folder(&mut s, json!({"folder": "2024"})), year);
    assert_eq!(listed_folder(&mut s, json!({"folder": sent.to_string()})), sent);
    assert_eq!(listed_folder(&mut s, json!({"folder": sent})), sent);
    // A folder whose name is another folder's id is that folder by its name; a number is the id.
    let named = s.mailbox.add_folder(account, None, &sent.to_string()).unwrap();
    s.mailbox.add_message(mailcraft_model::Message { folder: named, account, subject: "Named".into(), ..Default::default() }).unwrap();
    assert_eq!(listed_folder(&mut s, json!({"folder": sent.to_string()})), named);
    assert_eq!(listed_folder(&mut s, json!({"folder": sent})), sent);
}

/// A kind is of the account `account` names, else of the account in view.
#[test]
fn a_kind_is_of_the_account_named_or_in_view() {
    let (mut s, inbox, _) = german();
    let other = s.run("account.add", &json!({"email": "noor@example.org"})).unwrap()["id"].as_u64().unwrap();
    let other_inbox = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    s.mailbox.add_message(mailcraft_model::Message { folder: other_inbox, account: other, subject: "Hi".into(), ..Default::default() }).unwrap();
    assert_eq!(listed_folder(&mut s, json!({"folder": "inbox"})), inbox, "the account in view");
    assert_eq!(listed_folder(&mut s, json!({"folder": "inbox", "account": other})), other_inbox);
    s.run("folder.select", &json!({"folder": "inbox", "account": other})).unwrap();
    assert_eq!(s.view.folder, other_inbox);
    assert_eq!(listed_folder(&mut s, json!({"folder": "inbox"})), other_inbox, "now the account in view");
}

/// `account` is read strictly: what isn't an account's id is refused, never dropped for the
/// account in view. Asked for another account's Inbox and given this one's, with nothing in the
/// answer to say which it is, an agent would work on the wrong mailbox.
#[test]
fn an_account_that_is_not_an_id_is_refused() {
    let (mut s, inbox, _) = german();
    let in_view = s.view.folder;
    for account in [json!("7"), json!(""), json!(0), json!(-1), json!(1.5), json!(true), json!([7]), json!({}), Value::Null] {
        for params in [json!({"folder": "inbox", "account": account}), json!({"account": account}), json!({"query": "is:unread", "account": account})]
        {
            assert!(s.run("mail.list", &params).is_err(), "{params}");
            assert_eq!(s.view.folder, in_view, "{params}: nothing moved");
        }
        let params = json!({"folder": "inbox", "account": account});
        assert!(s.run("folder.select", &params).is_err(), "{params}");
        assert_eq!(s.view.folder, in_view, "{params}: nothing moved");
    }
    // An account that is an id but isn't there is said so, not taken for the account in view and
    // not reported as a folder the account hasn't got. The words carry the finding here: without
    // the check the call still fails, with "no Inbox folder", which reads as a different fault.
    let gone = s.mailbox.accounts.iter().map(|a| a.id).max().unwrap_or(0) + 99;
    let says_no_account = |e: crate::CmdError| {
        let text = e.to_string();
        assert!(text.contains("no account with id"), "{text}");
    };
    for params in [json!({"folder": "inbox", "account": gone}), json!({"folder": 1, "account": gone}), json!({"kind": "inbox", "account": gone})] {
        if params.get("folder").is_some() {
            says_no_account(s.run("mail.list", &params).expect_err(&format!("{params}")));
        }
        says_no_account(s.run("folder.select", &params).expect_err(&format!("{params}")));
    }
    assert_eq!(s.view.folder, in_view);

    // `account` says whose folder `folder` means, so on its own it is refused rather than read
    // and then left unused: the list would come from the account in view with nothing to say so.
    let real = s.mailbox.accounts.first().map(|a| a.id).unwrap();
    for params in [json!({"account": real}), json!({"account": real, "query": "is:unread"}), json!({"folder": Value::Null, "account": real})] {
        let text = s.run("mail.list", &params).expect_err(&format!("{params}")).to_string();
        assert!(text.contains("`account` says whose folder"), "{params}: {text}");
    }
    // No `account` key at all is the account in view, as before.
    assert_eq!(listed_folder(&mut s, json!({"folder": "inbox"})), inbox);
}

/// What names no folder is refused, and changes nothing.
#[test]
fn what_names_no_folder_is_refused() {
    let (mut s, _, _) = german();
    let in_view = s.view.folder;
    for folder in [json!(""), json!("   "), json!("Nowhere"), json!("999999"), json!(999_999), json!(true), json!(["inbox"])] {
        assert!(s.run("mail.list", &json!({"folder": folder})).is_err(), "{folder}");
        assert!(s.run("folder.select", &json!({"folder": folder})).is_err(), "{folder}");
    }
    // `custom` is a kind of folder, but names none.
    assert!(s.run("mail.list", &json!({"folder": "custom"})).is_err());
    assert_eq!(s.view.folder, in_view);
}

/// A kind comes before a folder of that name; where the account has no folder of that kind, the
/// folder of that name is taken instead, so nothing becomes unreachable by name.
#[test]
fn a_kind_shadows_a_folder_of_that_name_unless_the_account_has_none() {
    let (mut s, _, sent) = german();
    let account = s.mailbox.default_account().unwrap().id;
    let mine = s.mailbox.add_folder(account, None, "Sent").unwrap();
    s.mailbox.add_message(mailcraft_model::Message { folder: mine, account, subject: "Mine".into(), ..Default::default() }).unwrap();
    assert_eq!(listed_folder(&mut s, json!({"folder": "sent"})), sent, "the account's Sent Items wins");
    assert_eq!(listed_folder(&mut s, json!({"folder": mine})), mine, "the owner's own, by its id");
    // An account without a folder of that kind, as an IMAP server without an Archive leaves it:
    // a folder of the owner's own called that is taken.
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    s.mailbox.folder_mut(archive).unwrap().kind = FolderKind::Custom;
    s.mailbox.folder_mut(archive).unwrap().name = "Alt".into();
    assert_eq!(s.mailbox.folder_of_kind(account, FolderKind::Archive), None);
    let mine = s.mailbox.add_folder(account, None, "Archive").unwrap();
    s.mailbox.add_message(mailcraft_model::Message { folder: mine, account, subject: "Kept".into(), ..Default::default() }).unwrap();
    assert_eq!(listed_folder(&mut s, json!({"folder": "archive"})), mine);
    // That one is its account's own: another account's folder of that name is never reached by a
    // kind, so a word never lists somebody else's mail.
    s.mailbox.folder_mut(mine).unwrap().name = "Later".into();
    let other = s.run("account.add", &json!({"email": "noor@example.org"})).unwrap()["id"].as_u64().unwrap();
    let theirs = s.mailbox.folder_of_kind(other, FolderKind::Archive).unwrap();
    s.mailbox.add_message(mailcraft_model::Message { folder: theirs, account: other, subject: "Theirs".into(), ..Default::default() }).unwrap();
    let refused = s.run("mail.list", &json!({"folder": "archive"})).unwrap_err().to_string();
    assert!(refused.contains("Archive"), "{refused}");
    // Named, that account's own is found.
    assert_eq!(listed_folder(&mut s, json!({"folder": "archive", "account": other})), theirs);
}

/// Digits that name no folder are an id of the account named, not of any account (#1200): asked
/// for folder 12 of account 2, an agent must not be handed account 1's folder 12.
#[test]
fn digits_are_an_id_of_the_account_named() {
    let (mut s, inbox, _) = german();
    let second = s.run("account.add", &json!({"email": "noor@example.org"})).unwrap()["id"].as_u64().unwrap();
    let theirs = s.mailbox.folder_of_kind(second, FolderKind::Inbox).unwrap();
    let in_view = s.view.folder;

    // The Inbox of the first account, by its id, is not the second account's to give.
    let digits = inbox.to_string();
    let as_digits = s.run("mail.list", &json!({"folder": digits.clone(), "account": second})).expect_err("not that account's").to_string();
    assert!(s.run("folder.select", &json!({"folder": digits.clone(), "account": second})).is_err());
    assert_eq!(s.view.folder, in_view, "and nothing moved");

    // The same id as a number, not as digits in a string: one rule for both, and one wording, so
    // that an agent isn't told the folder doesn't exist when it is another account's.
    let as_number = s.run("mail.list", &json!({"folder": inbox, "account": second})).expect_err("a number is an id too").to_string();
    assert!(as_number.contains("is not that account's"), "{as_number}");
    assert_eq!(as_number, as_digits, "both spellings of the id say the same");
    assert!(s.run("folder.select", &json!({"folder": inbox, "account": second})).is_err());
    assert_eq!(s.view.folder, in_view, "and nothing moved");

    // Named with its own account, or with none, it is still that folder.
    assert_eq!(listed_folder(&mut s, json!({"folder": digits.clone()})), inbox);
    s.run("folder.select", &json!({"folder": theirs.to_string(), "account": second})).unwrap();
    assert_eq!(s.view.folder, theirs, "its own account's id reaches it");
}

/// A path without `account` is looked for in the account in view before any other (#1200): two
/// accounts can both have a folder called "Shared Notes", and the one in front of the owner is
/// the one they mean.
#[test]
fn a_path_takes_the_account_in_view_before_another() {
    let (mut s, inbox, _) = german();
    let first = s.mailbox.folder(inbox).unwrap().account;
    let second = s.run("account.add", &json!({"email": "noor@example.org"})).unwrap()["id"].as_u64().unwrap();
    // At the top of each account, so that the two paths read the same: the first account's
    // Inbox is called "Posteingang" here, the second's "Inbox". The first account's folder is
    // added first, so it is the one the old walk over every account would have found: a test
    // that starts with the first account in view would pass either way.
    let mine = s.mailbox.add_folder(first, None, "Shared Notes").unwrap();
    let theirs_inbox = s.mailbox.folder_of_kind(second, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.add_folder(second, None, "Shared Notes").unwrap();
    s.mailbox_changed();

    let path_of = |s: &Session, id: Id| s.mailbox.folder_path(id);
    let (mine_path, theirs_path) = (path_of(&s, mine), path_of(&s, theirs));
    assert_eq!(mine_path, theirs_path, "the two accounts name the folder the same way");

    let reached = |s: &mut Session, params: Value| {
        s.run("folder.select", &params).unwrap_or_else(|e| panic!("{params}: {e}"));
        s.view.folder
    };
    // The second account in view first: this is the one the mailbox order gets wrong.
    s.run("folder.select", &json!({"id": theirs_inbox})).unwrap();
    assert_eq!(reached(&mut s, json!({"folder": theirs_path.clone()})), theirs, "the account in view, not the first in the mailbox");
    // `path` takes it the same way as `folder`: the command line's import uses that form.
    s.run("folder.select", &json!({"id": theirs_inbox})).unwrap();
    assert_eq!(reached(&mut s, json!({"path": theirs_path.clone()})), theirs, "`path` reads it as `folder` does");
    assert_eq!(reached(&mut s, json!({"path": mine_path.clone(), "account": first})), mine, "and takes `account` too");
    assert_eq!(reached(&mut s, json!({"folder": mine_path.clone(), "account": first})), mine, "or the one named");
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    assert_eq!(reached(&mut s, json!({"folder": mine_path})), mine, "now the first account is in view");
}
