// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #893: each account has a colour. `account.add` gives a new one a colour no other account has,
//! `account.edit` changes it or takes it away, and `account.list` says it.

use mailcraft_model::{Account, Id, PALETTE, Rgb};
use serde_json::{Value, json};

use crate::Session;

fn add(s: &mut Session, params: Value) -> Id {
    s.run("account.add", &params).unwrap()["id"].as_u64().unwrap()
}

fn listed_color(s: &mut Session, id: Id) -> Value {
    let list = s.run("account.list", &json!({})).unwrap();
    list["accounts"].as_array().unwrap().iter().find(|a| a["id"] == json!(id)).unwrap()["color"].clone()
}

/// Each new account is given the next colour of the palette; the placeholder of an empty mailbox,
/// which the first one takes the place of, takes none with it.
#[test]
fn a_new_account_is_given_a_colour_no_other_has() {
    let mut s = Session::empty();
    let a = add(&mut s, json!({"email": "a@example.org"}));
    let b = add(&mut s, json!({"email": "b@example.org"}));
    assert_eq!(s.mailbox.account(a).unwrap().color, Some(PALETTE[0]));
    assert_eq!(s.mailbox.account(b).unwrap().color, Some(PALETTE[1]));
    assert_eq!(listed_color(&mut s, b), json!(PALETTE[1].hex()));
    // One asked for is the one it gets, and null is none.
    let c = add(&mut s, json!({"email": "c@example.org", "color": "#123456"}));
    let d = add(&mut s, json!({"email": "d@example.org", "color": null}));
    assert_eq!(s.mailbox.account(c).unwrap().color, Some(Rgb(0x12, 0x34, 0x56)));
    assert_eq!(s.mailbox.account(d).unwrap().color, None);
    assert_eq!(listed_color(&mut s, d), Value::Null);
}

/// `account.edit` changes the colour or takes it away, and leaves the rest of the account alone;
/// what isn't a colour is refused and changes nothing.
#[test]
fn edit_changes_or_clears_the_colour() {
    let mut s = Session::empty();
    let a = add(&mut s, json!({"email": "a@example.org", "displayName": "Ada"}));
    s.run("account.edit", &json!({"id": a, "color": "2f6fd0"})).unwrap();
    assert_eq!(s.mailbox.account(a).unwrap().color, Some(Rgb(0x2F, 0x6F, 0xD0)));
    assert_eq!(s.mailbox.account(a).unwrap().display_name, "Ada");
    // Saying nothing of it keeps it.
    s.run("account.edit", &json!({"id": a, "displayName": "Ada L."})).unwrap();
    assert_eq!(s.mailbox.account(a).unwrap().color, Some(Rgb(0x2F, 0x6F, 0xD0)));
    for bad in [json!("blue"), json!("#12345"), json!(42), json!([1, 2, 3]), json!(true)] {
        let before = s.mailbox.account(a).cloned();
        assert!(s.run("account.edit", &json!({"id": a, "color": bad})).is_err(), "{bad}");
        assert_eq!(s.mailbox.account(a).cloned(), before);
    }
    s.run("account.edit", &json!({"id": a, "color": null})).unwrap();
    assert_eq!(s.mailbox.account(a).unwrap().color, None);
}

/// The account on this computer that holds imported mail has no address; it can be given a
/// colour all the same, as it can be told whether to ask before deleting a conversation.
#[test]
fn the_account_on_this_computer_takes_a_colour() {
    let mut s = Session::empty();
    add(&mut s, json!({"email": "a@example.org"}));
    let here = s.mailbox.add_account(Account { name: "On this computer".into(), ..Default::default() });
    s.run("account.edit", &json!({"id": here, "color": "#8A5A44"})).unwrap();
    assert_eq!(s.mailbox.account(here).unwrap().color, Some(Rgb(0x8A, 0x5A, 0x44)));
    // (Anything else of it still wants an address.)
    assert!(s.run("account.edit", &json!({"id": here, "displayName": "Here"})).is_err());
}

/// A shared mailbox has a colour of its own, as it has a heading of its own in the folder pane:
/// one no other account has when it is opened, and another when changed.
#[test]
fn a_shared_mailbox_keeps_a_colour_of_its_own() {
    let mut s = Session::empty();
    let work = add(&mut s, json!({"email": "me@work.example", "kind": "microsoft"}));
    let team = s.mailbox.add_shared_mailbox(work, "team@work.example").unwrap();
    assert_eq!(s.mailbox.account(team).unwrap().color, Some(PALETTE[1]));
    assert_eq!(listed_color(&mut s, team), json!(PALETTE[1].hex()));
    s.run("account.edit", &json!({"id": team, "color": "#C24E9A"})).unwrap();
    assert_eq!(s.mailbox.account(team).unwrap().color, Some(Rgb(0xC2, 0x4E, 0x9A)));
    assert_eq!(s.mailbox.account(work).unwrap().color, Some(PALETTE[0]));
}
