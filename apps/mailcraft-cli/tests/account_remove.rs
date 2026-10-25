// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `mailcraft-cli run --cmd account.remove`, with no window to clean up after it (#325): the
//! saved mailbox has nothing left that came from the removed account's server, neither its
//! events, contacts and calendars nor Outlook's list of categories, and the other account's stay.
//! (The working hours are kept with the window's preferences, which the CLI doesn't write.)
//! Every address is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::process::Command;

use mailcraft_engine::Session;
use mailcraft_model::{Calendar, Contact, Event, Id, Mailbox, Origin};
use serde_json::json;

/// A folder of this test's own in the system's temporary folder, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let dir = std::env::temp_dir().join(format!("mailcraft-cli-account-remove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What `account`'s server gave, as the window's download leaves it in the file: a calendar of
/// its own with an event in it, and a contact.
fn from_server(s: &mut Session, account: Id, who: &str) {
    let calendar = s.mailbox.alloc_id();
    let origin = |id: &str| Origin { account, id: format!("{who}-{id}"), ..Default::default() };
    s.mailbox.calendars.push(Calendar {
        id: calendar,
        name: format!("{who}'s calendar"),
        visible: true,
        origin: origin("calendar"),
        ..Default::default()
    });
    s.mailbox.add_event(Event { calendar, title: format!("{who}'s meeting"), origin: origin("meeting"), ..Default::default() }).unwrap();
    s.mailbox.add_contact(Contact { display_name: format!("{who}'s colleague"), origin: origin("colleague"), ..Default::default() });
}

#[test]
fn the_cli_removes_what_came_from_the_accounts_server() {
    let tmp = Scratch::new();
    let (before, after) = (tmp.0.join("before.mailcraft"), tmp.0.join("after.mailcraft"));
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let keep = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let going = s.run("account.add", &json!({"email": "grace@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    from_server(&mut s, keep, "Ada");
    from_server(&mut s, going, "Grace");
    s.mailbox.add_contact(Contact { display_name: "Neighbour".into(), ..Default::default() });
    s.mailbox.outlook_categories = vec!["Red category".into()];
    s.run("file.saveAs", &json!({"path": before})).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_mailcraft-cli"))
        .arg("run")
        .arg("--file")
        .arg(&before)
        .arg("--cmd")
        .arg(format!("account.remove={}", json!({"id": going})))
        .arg("--save")
        .arg(&after)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let m = mailcraft_engine::io::open_path(&after).unwrap();
    assert!(m.account(going).is_none() && m.account(keep).is_some());
    let mut titles: Vec<&str> = m.events.values().map(|e| e.title.as_str()).collect();
    titles.sort();
    assert_eq!(titles, ["Ada's meeting"]);
    let mut people: Vec<&str> = m.contacts.values().map(|c| c.display_name.as_str()).collect();
    people.sort();
    assert_eq!(people, ["Ada's colleague", "Neighbour"]);
    assert!(!m.calendars.iter().any(|c| c.origin.account == going), "{:?}", m.calendars);
    assert!(m.calendars.iter().any(|c| c.origin.account == keep));
    assert!(m.outlook_categories.is_empty());
}
