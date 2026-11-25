// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent's move to a contacts folder (#1144) waits for the owner, who is told which contacts go
//! where.

use serde_json::json;

use super::{Access, Class, classify, describe};
use crate::Session;

#[test]
fn a_contact_move_is_asked_about_by_name() {
    let mut s = Session::empty();
    s.run("people.new", &json!({"first": "Bo", "last": "Example", "email": "bo@example.org"})).unwrap();
    let id = s.sel.contact.unwrap();
    let params = json!({"ids": [id], "folder": 0});
    assert_eq!(classify(&s, "people.moveToFolder", &params), Access::Owner(Class::Delete));
    let d = describe(&s, "people.moveToFolder", &params);
    assert!(d.contains("Bo Example") && d.contains("On this computer") && d.contains("deleting"), "{d}");
    let junk = describe(&s, "people.moveToFolder", &json!({"ids": "x", "folder": "y"}));
    assert!(junk.contains("no folder"), "{junk}");
}
