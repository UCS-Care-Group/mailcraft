// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a contact remembers of its server, and what a mailbox remembers of contacts deleted here:
//! files from before open, the new fields come back as they were written, a contact says which of
//! its parts were changed here, and a deletion is noted only for an account that writes.

use mailcraft_time::Date;
use serde_json::json;

use crate::remote::{ContactField, RemoteContact};
use crate::{Account, AccountKind, Contact, ContactGone, Labeled, MAX_CONTACT_GONE, Mailbox, Origin, PostalAddress};

fn ada() -> Contact {
    Contact {
        display_name: "Ada Lovelace".into(),
        first: "Ada".into(),
        last: "Lovelace".into(),
        company: "Analytical Engines Ltd".into(),
        emails: vec![Labeled::new("work", "ada@example.org")],
        phones: vec![Labeled::new("mobile", "+44 7700 900000")],
        addresses: vec![PostalAddress { street: "1 Engine Row".into(), city: "London".into(), ..Default::default() }],
        websites: vec!["https://example.org/ada".into()],
        birthday: Date::new(1815, 12, 10),
        notes: "Met at the Royal Society.".into(),
        origin: Origin { account: 7, id: "K1".into(), change_key: "ck-1".into(), ..Default::default() },
        ..Default::default()
    }
}

#[test]
fn only_a_jmap_account_of_ones_own_writes_its_contacts() {
    let jmap = Account { email: "ada@example.org".into(), kind: AccountKind::Jmap, ..Default::default() };
    assert!(jmap.writes_contacts() && jmap.keeps_contacts());
    assert!(!Account { shared_of: Some(7), ..jmap.clone() }.writes_contacts());
    let microsoft = Account {
        oauth: Some(crate::OAuth { provider: crate::OAuthProvider::Microsoft, ..Default::default() }),
        ..Account { email: "bo@example.org".into(), ..Default::default() }
    };
    assert!(microsoft.keeps_contacts() && !microsoft.writes_contacts());
    assert!(!Account::default().writes_contacts());
}

#[test]
fn a_contact_as_its_server_would_have_it_is_the_contact_and_nothing_kept_only_here() {
    let contact = Contact { categories: vec!["Red category".into()], favorite: true, uid: "u-1".into(), ..ada() };
    let card = contact.as_remote();
    assert_eq!((card.id.as_str(), card.change_key.as_str(), card.modified), ("K1", "ck-1", 0));
    assert_eq!((card.display_name.as_str(), card.first.as_str(), card.last.as_str()), ("Ada Lovelace", "Ada", "Lovelace"));
    assert_eq!((card.emails.clone(), card.phones.clone()), (contact.emails.clone(), contact.phones.clone()));
    assert_eq!(
        (card.addresses.clone(), card.websites.clone(), card.birthday),
        (contact.addresses.clone(), contact.websites.clone(), contact.birthday)
    );
    assert_eq!(card.notes.as_str(), "Met at the Royal Society.");
    // Cut to size as everything that goes to a server: an address that is none and an entry with
    // nothing in it are left out.
    let messy = Contact {
        emails: vec![Labeled::new("work", "not an address"), Labeled::new("home", "ada@example.org")],
        phones: vec![Labeled::new("mobile", "")],
        ..ada()
    };
    let card = messy.as_remote();
    assert_eq!((card.emails, card.phones.len()), (vec![Labeled::new("home", "ada@example.org")], 0));
    // Tidying it again changes nothing.
    let mut again = contact.as_remote();
    again.tidy();
    assert_eq!(again, contact.as_remote());
}

#[test]
fn a_contact_says_which_parts_were_changed_here_since_the_server_had_it() {
    let mut contact = ada();
    assert!(contact.changed_fields().is_empty(), "nothing is known of the server: nothing to tell");
    contact.synced = Some(Box::new(contact.as_remote()));
    assert!(contact.changed_fields().is_empty());
    // What is only kept here is no change to tell.
    contact.favorite = true;
    contact.categories = vec!["Red category".into()];
    assert!(contact.changed_fields().is_empty());
    contact.notes = "Met again.".into();
    contact.last = "King".into();
    contact.department = "Engines".into();
    assert_eq!(contact.changed_fields(), [ContactField::Name, ContactField::Organization, ContactField::Notes]);
    // Taken back, it is no change.
    let server = contact.synced.clone().unwrap();
    for field in ContactField::ALL {
        contact.set_part(field, &server);
    }
    assert!(contact.changed_fields().is_empty());
    assert_eq!(Contact { synced: None, favorite: false, categories: Vec::new(), ..contact }, ada());
    // A difference that is cut away on the way to a server is none.
    let mut spaced = ada();
    spaced.synced = Some(Box::new(spaced.as_remote()));
    spaced.phones.push(Labeled::new("home", ""));
    assert!(spaced.changed_fields().is_empty());
}

#[test]
fn every_part_is_put_by_itself() {
    let there = RemoteContact {
        display_name: "A. King".into(),
        first: "Augusta".into(),
        middle: "Ada".into(),
        last: "King".into(),
        nickname: "AAL".into(),
        company: "Difference Engines".into(),
        department: "Notes".into(),
        title: "Countess".into(),
        emails: vec![Labeled::new("home", "augusta@example.org")],
        phones: vec![Labeled::new("work", "+44 20 7946 0000")],
        addresses: vec![PostalAddress { city: "Ockham".into(), ..Default::default() }],
        websites: vec!["https://example.org/king".into()],
        birthday: Date::new(1815, 12, 11),
        notes: "Other notes.".into(),
        ..Default::default()
    };
    for field in ContactField::ALL {
        let mut contact = ada();
        contact.synced = Some(Box::new(contact.as_remote()));
        contact.set_part(field, &there);
        assert_eq!(contact.changed_fields(), [field], "{field:?}");
        assert!(field.same(&contact.as_remote(), &there), "{field:?}");
    }
}

#[test]
fn a_mailbox_from_before_opens_and_the_new_fields_come_back_as_written() {
    // A contact and a mailbox as a version from before wrote them: nothing of the new fields.
    let old: Contact = serde_json::from_value(json!({"id": 4, "first": "Ada", "origin": {"account": 7, "id": "K1"}})).unwrap();
    assert!(old.synced.is_none() && old.origin.id == "K1");
    let mut mailbox = Mailbox::with_local_account("Me", "me@example.org");
    let mut written = serde_json::to_value(&mailbox).unwrap();
    assert!(written.get("contactGone").is_none(), "nothing waiting: nothing written");
    written.as_object_mut().unwrap().remove("contactGone");
    let opened: Mailbox = serde_json::from_value(written).unwrap();
    assert!(opened.contact_gone.is_empty());
    // A contact without the memory writes none: a version from before reads what it always read.
    let plain = serde_json::to_value(ada()).unwrap();
    assert!(plain.get("synced").is_none());
    // With them: written, and read back the same.
    let mut contact = ada();
    contact.synced = Some(Box::new(contact.as_remote()));
    contact.notes = "Changed here.".into();
    let id = mailbox.add_contact(contact.clone());
    mailbox.contact_gone.push(ContactGone { account: 7, id: "K2".into() });
    let text = serde_json::to_string(&mailbox).unwrap();
    let back: Mailbox = serde_json::from_str(&text).unwrap();
    assert_eq!(back.contact_gone, [ContactGone { account: 7, id: "K2".into() }]);
    assert_eq!(back.contact(id), mailbox.contact(id));
    assert_eq!(back.contact(id).unwrap().changed_fields(), [ContactField::Notes]);
    // What a version from before knew of the contact is all still there under the names it knew.
    let written = serde_json::to_value(mailbox.contact(id).unwrap()).unwrap();
    assert_eq!((&written["notes"], &written["origin"]["id"], &written["first"]), (&json!("Changed here."), &json!("K1"), &json!("Ada")));
    // A list of deletions that is no list is read as none, as the other lists of waiting changes.
    let mut odd = serde_json::to_value(&mailbox).unwrap();
    odd["contactGone"] = json!([{"account": 7, "id": "K3"}, "nonsense", 5]);
    let read: Mailbox = serde_json::from_value(odd).unwrap();
    assert!(read.contact_gone.iter().all(|gone| gone.account == 7));
}

#[test]
fn a_deletion_is_noted_for_an_account_that_writes_and_for_no_other() {
    let mut mailbox = Mailbox::with_local_account("Me", "me@example.org");
    let jmap = mailbox.add_account(Account { email: "ada@example.org".into(), kind: AccountKind::Jmap, ..Default::default() });
    let reads = mailbox.add_account(Account {
        email: "bo@example.org".into(),
        oauth: Some(crate::OAuth { provider: crate::OAuthProvider::Microsoft, ..Default::default() }),
        ..Default::default()
    });
    let of = |account, id: &str| Contact {
        first: "Somebody".into(),
        origin: Origin { account, id: id.into(), ..Default::default() },
        ..Default::default()
    };
    let (theirs, waiting, read, mine, quiet) = (
        mailbox.add_contact(of(jmap, "K1")),
        mailbox.add_contact(of(jmap, "")),
        mailbox.add_contact(of(reads, "M1")),
        mailbox.add_contact(of(0, "")),
        mailbox.add_contact(of(jmap, "K2")),
    );
    for id in [theirs, waiting, read, mine] {
        mailbox.remove_contact(id).unwrap();
    }
    mailbox.remove_contact_quietly(quiet).unwrap();
    assert_eq!(mailbox.contact_gone, [ContactGone { account: jmap, id: "K1".into() }]);
    assert!(mailbox.remove_contact(theirs).is_err() && mailbox.remove_contact_quietly(quiet).is_err());
    // Never more than the most that wait, and none twice.
    mailbox.contact_gone = (0..MAX_CONTACT_GONE).map(|n| ContactGone { account: jmap, id: format!("G{n}") }).collect();
    let (one_more, again) = (mailbox.add_contact(of(jmap, "K9")), mailbox.add_contact(of(jmap, "G0")));
    mailbox.remove_contact(one_more).unwrap();
    assert_eq!(mailbox.contact_gone.len(), MAX_CONTACT_GONE);
    mailbox.contact_gone.truncate(3);
    mailbox.remove_contact(again).unwrap();
    assert_eq!(mailbox.contact_gone.len(), 3);
    // The account goes: what waited for its server goes, and a contact it was yet to be given stays.
    let made = mailbox.add_contact(of(jmap, ""));
    mailbox.remove_account(jmap).unwrap();
    assert!(mailbox.contact_gone.is_empty());
    assert!(mailbox.contact(made).is_some_and(|c| c.origin.is_local() && c.synced.is_none()));
}
