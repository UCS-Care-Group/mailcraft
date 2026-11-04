// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The order the owner puts their accounts in (#702): kept apart from the accounts, followed by
//! the folder pane's list, and never the default account's say.

use serde_json::json;

use crate::*;

/// Four accounts of the owner's own, A to D in the order they were added, and a shared mailbox
/// opened through B (a Microsoft work account); their ids, the shared mailbox last.
fn four() -> (Mailbox, [Id; 5]) {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "a@example.org");
    let a = mb.accounts[0].id;
    let oauth = OAuth { provider: OAuthProvider::Microsoft, directory: Directory::Organisation, ..Default::default() };
    let b = mb.add_account(Account { email: "b@example.org".into(), kind: AccountKind::Microsoft, oauth: Some(oauth), ..Default::default() });
    let c = mb.add_account(Account { email: "c@example.org".into(), ..Default::default() });
    let d = mb.add_account(Account { email: "d@example.org".into(), ..Default::default() });
    let shared = mb.add_shared_mailbox(b, "team@example.org").unwrap();
    (mb, [a, b, c, d, shared])
}

fn order(mb: &Mailbox) -> Vec<Id> {
    mb.accounts_in_order().iter().map(|a| a.id).collect()
}

/// Until the owner moves one, the accounts are listed as they were added, and nothing of an order
/// is written to the file.
#[test]
fn accounts_are_listed_as_added_until_one_is_moved() {
    let (mb, [a, b, c, d, shared]) = four();
    assert_eq!(order(&mb), [a, b, shared, c, d]);
    assert!(mb.account_order.is_empty());
    assert!(!serde_json::to_string(&mb).unwrap().contains("accountOrder"));
}

/// The account added last goes second: the folder pane lists it there, a shared mailbox goes with
/// its account, the default account stays the first added, and the accounts themselves stay in
/// the order they were added.
#[test]
fn an_account_moved_stays_where_it_was_put() {
    let (mut mb, [a, b, c, d, shared]) = four();
    assert_eq!(mb.move_account(d, Some(b)), Ok(true));
    assert_eq!(order(&mb), [a, d, b, shared, c]);
    assert_eq!(mb.default_account().map(|x| x.id), Some(a));
    assert_eq!(mb.accounts.iter().map(|x| x.id).collect::<Vec<_>>(), [a, b, c, d, shared]);
    // To the top, and last (no account named).
    assert_eq!(mb.move_account(c, Some(a)), Ok(true));
    assert_eq!(order(&mb), [c, a, d, b, shared]);
    assert_eq!(mb.move_account(b, None), Ok(false), "b is last already");
    assert_eq!(order(&mb), [c, a, d, b, shared]);
    assert_eq!(mb.move_account(a, None), Ok(true));
    assert_eq!(order(&mb), [c, d, b, shared, a]);
    assert_eq!(mb.default_account().map(|x| x.id), Some(a));
    // Put where it already is: nothing changes.
    let was = mb.account_order.clone();
    assert_eq!(mb.move_account(d, Some(b)), Ok(false));
    assert_eq!(mb.account_order, was);
}

/// The order goes into the file and comes back from it.
#[test]
fn the_order_is_kept_in_the_file() {
    let (mut mb, [a, b, c, d, shared]) = four();
    mb.move_account(d, Some(a)).unwrap();
    let json = serde_json::to_string(&mb).unwrap();
    assert!(json.contains(&format!("\"accountOrder\":[{d},{a},{b},{c}]")), "{json}");
    let back: Mailbox = serde_json::from_str(&json).unwrap();
    assert_eq!(order(&back), [d, a, b, shared, c]);
    assert_eq!(back.default_account().map(|x| x.id), Some(a));
}

/// An account added after the owner put the others in order comes last; one removed leaves the
/// order, and the shared mailboxes it took with it never were in it.
#[test]
fn an_account_added_later_comes_last_and_one_removed_leaves() {
    let (mut mb, [a, b, c, d, shared]) = four();
    mb.move_account(c, Some(a)).unwrap();
    let e = mb.add_account(Account { email: "e@example.org".into(), ..Default::default() });
    assert_eq!(order(&mb), [c, a, b, shared, d, e]);
    mb.remove_account(b).unwrap();
    assert_eq!(mb.account_order, [c, a, d]);
    assert_eq!(order(&mb), [c, a, d, e]);
    // The account it named (e) comes in its place once it is moved too.
    mb.move_account(e, Some(a)).unwrap();
    assert_eq!(mb.account_order, [c, e, a, d]);
}

/// A shared mailbox is never moved by itself nor named as the place: it stays under its account.
/// Nor is an account put before itself, or an account that isn't here moved.
#[test]
fn what_cant_be_moved_is_refused() {
    let (mut mb, [a, b, _, d, shared]) = four();
    let refused = |mb: &mut Mailbox, id: Id, before: Option<Id>| mb.move_account(id, before).is_err();
    assert!(refused(&mut mb, shared, Some(a)));
    assert!(refused(&mut mb, d, Some(shared)));
    assert!(refused(&mut mb, d, Some(d)));
    assert!(refused(&mut mb, 999_999, None));
    assert!(refused(&mut mb, d, Some(999_999)));
    assert!(mb.account_order.is_empty(), "a refusal changes nothing");
    assert_eq!(order(&mb), [a, b, shared, four().1[2], d]);
}

/// Whatever a file says the order is, the accounts are all listed once: an id twice counts where
/// it first stands, and ids of no account, of a shared mailbox, or in no list form are passed
/// over; repairing the file keeps only the owner's accounts, each once.
#[test]
fn a_hostile_order_lists_every_account_once() {
    let (mut mb, [a, b, c, d, shared]) = four();
    mb.account_order = vec![d, 424_242, shared, d, b, 0];
    assert_eq!(order(&mb), [d, b, shared, a, c]);
    assert!(mb.repair().iter().any(|line| line.contains("order")));
    assert_eq!(mb.account_order, [d, b]);
    assert!(mb.repair().is_empty(), "nothing more to tidy");
    let json = serde_json::to_string(&mb).unwrap();
    for odd in [r#""x""#, r#"{"a": 1}"#, r#"[1, "two", -3, null, 1.5]"#, "7"] {
        let file = json.replacen(&format!("\"accountOrder\":[{d},{b}]"), &format!("\"accountOrder\":{odd}"), 1);
        assert!(file.contains(odd), "{file}");
        let read: Mailbox = serde_json::from_str(&file).unwrap_or_else(|e| panic!("{odd}: {e}"));
        assert_eq!(read.accounts_in_order().len(), 5, "{odd}");
    }
}

/// Every entry of the order counts against what reading a file may cost, ids or not, so a file
/// with a very long order fails as one with too many messages does, having held no more; one
/// within the budget opens.
#[test]
fn every_entry_of_the_order_in_a_file_counts() {
    let (mb, [_, b, _, d, _]) = four();
    let room = 200;
    assert!(crate::from_json(&serde_json::to_vec(&mb).unwrap(), room).is_ok());
    let mut value = serde_json::to_value(&mb).unwrap();
    let entries: Vec<serde_json::Value> = (0..room).map(|i| if i % 2 == 0 { json!(d) } else { json!({"x": [i, "y"]}) }).collect();
    value["accountOrder"] = serde_json::Value::Array(entries);
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(crate::from_json(&bytes, room).is_err());
    let read = crate::from_json(&bytes, 3 * room).unwrap();
    assert_eq!(read.account_order.iter().filter(|id| **id == d).count(), room / 2, "the ids are kept, the rest passed over");
    // Not a list: costs what it holds, and the file opens without an order.
    value["accountOrder"] = json!({"a": b, "b": [1, 2]});
    let read = crate::from_json(&serde_json::to_vec(&value).unwrap(), room).unwrap();
    assert!(read.account_order.is_empty());
}
