// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #70: a calendar's owner and rights, and the calendars opened by address: old files, round trips
//! and hostile values.

use serde_json::json;

use crate::calendar::MAX_OWNER_ADDRESS;
use crate::*;

fn calendar(value: serde_json::Value) -> Calendar {
    serde_json::from_value(value).unwrap()
}

#[test]
fn an_older_file_reads_read_only_as_rights() {
    assert_eq!(calendar(json!({"id": 3, "name": "Holidays", "read_only": true})).rights, Rights::Read);
    assert_eq!(calendar(json!({"id": 3, "name": "Team", "read_only": false})).rights, Rights::Edit);
    assert_eq!(calendar(json!({"id": 3, "name": "Team"})).rights, Rights::Edit);
    // Rights, when the file has them, win over a stale `read_only`.
    assert_eq!(calendar(json!({"id": 3, "read_only": true, "rights": "edit"})).rights, Rights::Edit);
}

#[test]
fn rights_and_owner_survive_a_save_and_reload() {
    let mut mb = Mailbox::with_local_account("Ada", "ada@example.org");
    for (name, rights) in [("Read", Rights::Read), ("Busy", Rights::FreeBusy), ("Edit", Rights::Edit)] {
        let id = mb.alloc_id();
        let owner = Some(CalendarOwner { address: "bo@example.org".into(), name: "Bo".into() });
        mb.calendars.push(Calendar { id, name: name.into(), rights, owner, ..Default::default() });
    }
    let account = mb.accounts[0].id;
    mb.account_mut(account).unwrap().opened_calendars =
        vec![OpenedCalendar { address: "cy@example.org".into(), name: "Cy".into(), color: Rgb(1, 2, 3), ticked: true }];
    let bytes = serde_json::to_vec(&mb).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(!text.contains("read_only"), "the old key is gone");
    let back = from_json(&bytes, 10_000).unwrap();
    assert_eq!(back.calendars, mb.calendars);
    assert_eq!(back.accounts[0].opened_calendars, mb.accounts[0].opened_calendars);
    // The default (edit, no owner) writes nothing.
    let plain = serde_json::to_value(Calendar { id: 1, ..Default::default() }).unwrap();
    assert!(plain.get("rights").is_none() && plain.get("owner").is_none());
}

#[test]
fn hostile_rights_and_owners_are_clamped() {
    assert_eq!(calendar(json!({"id": 1, "rights": "root"})).rights, Rights::Read);
    assert_eq!(calendar(json!({"id": 1, "rights": 7})).rights, Rights::Read);
    assert_eq!(calendar(json!({"id": 1, "rights": " FreeBusy "})).rights, Rights::FreeBusy);
    let huge = "x".repeat(100_000);
    let c = calendar(json!({"id": 1, "owner": {"address": format!("{huge}@example.org"), "name": format!("A\r\nB{huge}")}}));
    let owner = c.owner.unwrap();
    assert!(owner.address.chars().count() <= MAX_OWNER_ADDRESS && owner.name.chars().count() <= MAX_NAME);
    assert!(!owner.name.contains('\n') && !owner.name.contains('\r'));
    assert_eq!(calendar(json!({"id": 1, "owner": {"address": "  ", "name": ""}})).owner, None);
    assert_eq!(calendar(json!({"id": 1, "owner": "bo"})).owner, None);
}

#[test]
fn calendars_opened_by_address_are_capped_and_cleaned() {
    let mut mb = Mailbox::with_local_account("Ada", "ada@example.org");
    let account = mb.accounts[0].id;
    let mut list: Vec<OpenedCalendar> =
        (0..60).map(|n| OpenedCalendar { address: format!("p{n}@example.org"), name: format!("P{n}\r\n"), ..Default::default() }).collect();
    list.push(OpenedCalendar { address: "P1@EXAMPLE.org".into(), ..Default::default() });
    list.insert(0, OpenedCalendar { address: " ".into(), ..Default::default() });
    mb.account_mut(account).unwrap().opened_calendars = list;
    mb.repair();
    let kept = &mb.accounts[0].opened_calendars;
    assert_eq!(kept.len(), MAX_OPENED_CALENDARS);
    assert!(kept.iter().all(|c| !c.address.is_empty() && !c.name.contains('\n')));
    assert_eq!(kept.iter().filter(|c| c.address.eq_ignore_ascii_case("p1@example.org")).count(), 1);
    // A file whose list is not a list, or has junk in it, still opens.
    let mut value = serde_json::to_value(&mb).unwrap();
    value["accounts"][0]["openedCalendars"] = json!([{"address": "a@example.org", "color": "not a colour"}, 5, "x"]);
    assert!(serde_json::from_value::<Mailbox>(value.clone()).is_ok());
    value["accounts"][0]["openedCalendars"] = json!("junk");
    assert!(serde_json::from_value::<Mailbox>(value).is_ok());
}

/// A Microsoft account with the shared mailbox `ops@example.org` open under it.
fn with_shared_mailbox() -> (Mailbox, Id, Id) {
    let mut mb = Mailbox::with_local_account("Ada", "ada@example.org");
    let owner = mb.alloc_id();
    mb.accounts.push(Account { id: owner, email: "noor@example.org".into(), kind: AccountKind::Microsoft, ..Default::default() });
    let shared = mb.add_shared_mailbox(owner, "ops@example.org").unwrap();
    (mb, owner, shared)
}

#[test]
fn a_calendar_listed_twice_is_remembered_cleaned_and_forgotten_with_its_shared_mailbox() {
    let (mut mb, owner, shared) = with_shared_mailbox();
    // An older file has none, and says nothing of it.
    assert!(mb.account(owner).unwrap().listed_twice.is_empty());
    assert!(serde_json::to_value(mb.account(owner).unwrap()).unwrap().get("listedTwice").is_none());
    mb.account_mut(owner).unwrap().listed_twice = vec![ListedTwice { address: "OPS@example.org".into(), hidden: true }];
    let a = mb.account(owner).unwrap();
    assert_eq!((a.listed_twice_answer("ops@EXAMPLE.org"), a.listed_twice_answer("cy@example.org")), (Some(true), None));
    let ops = CalendarOwner { address: "Ops@Example.org".into(), name: "Operations".into() };
    assert!(a.hides_listed(Some(&ops)) && !a.hides_listed(None));
    assert!(!a.hides_listed(Some(&CalendarOwner { address: "cy@example.org".into(), name: String::new() })));
    // A save and a reload keep it.
    let back = from_json(&serde_json::to_vec(&mb).unwrap(), 10_000).unwrap();
    assert_eq!(back.account(owner).unwrap().listed_twice, mb.account(owner).unwrap().listed_twice);
    // Junk, duplicates and entries for no shared mailbox open under the account are cleaned away.
    let mut value = serde_json::to_value(&mb).unwrap();
    let at = mb.accounts.iter().position(|a| a.id == owner).unwrap();
    value["accounts"][at]["listedTwice"] = json!([{"address": "ops@example.org", "hidden": true}, {"address": "OPS@example.org"}, {"address": "gone@example.org", "hidden": true}, 5, "x", {"address": "  "}]);
    let mut back: Mailbox = serde_json::from_value(value.clone()).unwrap();
    back.repair();
    assert_eq!(back.account(owner).unwrap().listed_twice, [ListedTwice { address: "ops@example.org".into(), hidden: true }]);
    value["accounts"][at]["listedTwice"] = json!("junk");
    assert!(serde_json::from_value::<Mailbox>(value).is_ok());
    // Closing the shared mailbox forgets it: opened again, it is offered again.
    mb.remove_account(shared).unwrap();
    assert!(mb.account(owner).unwrap().listed_twice.is_empty());
}
