// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::BTreeSet;

use proptest::prelude::*;

use crate::contact_sync::{
    ContactChange, ContactChangeKind, ContactField, ContactFolder, ContactResult, FolderOwner, MAX_CONTACT_CHANGES, MAX_CONTACT_FOLDERS, wait_after,
};
use crate::{Account, Contact, Labeled, Mailbox, Origin};

fn ada() -> Contact {
    Contact { first: "Ada".into(), last: "Lovelace".into(), emails: vec![Labeled::new("work", "ada@example.org")], ..Default::default() }
}

#[test]
fn a_queue_and_its_folders_go_through_a_file_and_back() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    let main = mb.ensure_main_contact_folder(account).unwrap();
    let shared = mb
        .add_contact_folder(ContactFolder {
            account,
            remote: "f-1".into(),
            name: "Sales".into(),
            owner: FolderOwner::Mailbox("sales@example.org".into()),
            delta: "https://graph.example.org/delta?token=x".into(),
            ..Default::default()
        })
        .unwrap();
    let id = mb.add_contact(Contact { folder: main, origin: Origin { account, id: "c-1".into(), ..Default::default() }, ..ada() });
    mb.contact_changes.push(ContactChange {
        account,
        kind: ContactChangeKind::Change,
        contact: id,
        folder: main,
        remote: "c-1".into(),
        fields: BTreeSet::from([ContactField::Last, ContactField::Categories]),
        base: Some(Box::new(ada())),
        attempts: 2,
        next_try: 1_800_000_000,
        ..Default::default()
    });
    mb.contact_place = Some(shared);
    let back: Mailbox = serde_json::from_str(&serde_json::to_string(&mb).unwrap()).unwrap();
    assert_eq!((&back.contact_folders, &back.contact_changes, back.contact_place), (&mb.contact_folders, &mb.contact_changes, mb.contact_place));
    assert_eq!(back.contact(id).unwrap().folder, main);
    assert!(back.clone().repair().is_empty());
}

#[test]
fn an_old_file_puts_its_accounts_contacts_in_their_main_folders() {
    // A file from before folders: a server's contact (origin) and one of this computer's.
    let mut old = Mailbox::default();
    let account = old.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    let theirs = old.add_contact(Contact { origin: Origin { account, id: "c-1".into(), ..Default::default() }, ..ada() });
    let mine = old.add_contact(Contact { first: "Bo".into(), ..Default::default() });
    // An account that is gone: its contact is this computer's.
    let stray =
        old.add_contact(Contact { first: "Cy".into(), origin: Origin { account: 999, id: "x".into(), ..Default::default() }, ..Default::default() });
    let text = serde_json::to_string(&old).unwrap();
    assert!(!text.contains("contactFolders") && !text.contains("\"folder\""));
    let mut mb: Mailbox = serde_json::from_str(&text).unwrap();
    assert!(!mb.repair().is_empty());
    let main = mb.main_contact_folder(account).unwrap();
    assert_eq!(mb.contact_folder(main).map(|f| (f.writable, f.name.as_str(), f.is_main())), Some((true, "Contacts", true)));
    assert_eq!((mb.contact(theirs).unwrap().folder, mb.contact(mine).unwrap().folder, mb.contact(stray).unwrap().folder), (main, 0, 0));
    // Once is enough.
    assert!(mb.repair().is_empty());
}

#[test]
fn folders_and_changes_are_capped() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    for n in 0..MAX_CONTACT_FOLDERS {
        assert!(mb.add_contact_folder(ContactFolder { account, remote: format!("f-{n}"), ..Default::default() }).is_some());
    }
    assert!(mb.add_contact_folder(ContactFolder { account, remote: "one too many".into(), ..Default::default() }).is_none());
    assert!(mb.add_contact_folder(ContactFolder { account: 4242, ..Default::default() }).is_none(), "no such account");
    // A file with more than that, and more changes than may wait: cut, and said.
    let mut file = mb.clone();
    let copy = file.contact_folders[0].clone();
    file.contact_folders.push(ContactFolder { id: 1_000_000, ..copy });
    file.contact_changes = vec![ContactChange { account, ..Default::default() }; MAX_CONTACT_CHANGES + 1];
    let said = file.repair();
    assert!(said.iter().any(|l| l.contains("folders")) && said.iter().any(|l| l.contains("cut")), "{said:?}");
    assert_eq!((file.contact_folders.len(), file.contact_changes.len()), (MAX_CONTACT_FOLDERS, MAX_CONTACT_CHANGES));
}

/// Review of #1934: a folder whose id is out of range is dropped (its id could be given out
/// again), and a contact given a new key keeps the change waiting for it.
#[test]
fn repair_drops_folders_out_of_range_and_keeps_a_rehomed_contacts_change() {
    const TOO_BIG: crate::Id = (1 << 53) + 1;
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    mb.contact_folders.push(ContactFolder { id: TOO_BIG, account, remote: "f-big".into(), ..Default::default() });
    mb.contacts.insert(TOO_BIG + 1, std::sync::Arc::new(Contact { id: TOO_BIG + 1, ..ada() }));
    mb.contact_changes.push(ContactChange { account, kind: ContactChangeKind::Create, contact: TOO_BIG + 1, ..Default::default() });
    let mut text: Mailbox = serde_json::from_str(&serde_json::to_string(&mb).unwrap()).unwrap();
    text.repair();
    assert!(text.contact_folders.iter().all(|f| f.id <= 1 << 53));
    let moved = text.contacts.values().find(|c| c.first == "Ada").unwrap().id;
    assert!(moved <= 1 << 53);
    assert_eq!(text.contact_changes[0].contact, moved);
    // Two new folders get ids of their own.
    let a = text.add_contact_folder(ContactFolder { account, remote: "a".into(), ..Default::default() }).unwrap();
    let b = text.add_contact_folder(ContactFolder { account, remote: "b".into(), ..Default::default() }).unwrap();
    assert!(a != b && text.contact_folders.iter().filter(|f| f.id == a || f.id == b).count() == 2);
}

#[test]
fn an_owner_from_outside_is_an_address_or_no_folder() {
    let bad = ContactFolder { owner: FolderOwner::Mailbox("not an address".into()), ..Default::default() };
    assert!(bad.tidied().is_none());
    let good =
        ContactFolder { name: " ".into(), delta: "has space".into(), owner: FolderOwner::Person(" bo@example.org ".into()), ..Default::default() };
    let good = good.tidied().unwrap();
    assert_eq!((good.name.as_str(), good.delta.as_str(), good.owner.address()), ("Contacts", "", Some("bo@example.org")));
}

#[test]
fn statuses_map_as_for_events() {
    use ContactChangeKind::{Change, Create, Delete};
    use ContactResult::{Denied, Done, Failed, Gone, Refused};
    for (kind, status, result) in [
        (Create, 201, Some(Done)),
        (Change, 200, Some(Done)),
        (Delete, 204, Some(Done)),
        (Change, 404, Some(Gone)),
        (Delete, 410, Some(Gone)),
        (Change, 403, Some(Denied)),
        (Change, 401, None),
        (Create, 400, Some(Refused)),
        (Change, 409, Some(Refused)),
        (Create, 409, Some(Failed)),
        (Change, 412, Some(Refused)),
        (Create, 413, Some(Refused)),
        (Change, 429, Some(Failed)),
        (Change, 503, Some(Failed)),
        (Change, 0, Some(Failed)),
    ] {
        assert_eq!(ContactResult::of_status(kind, status), result, "{kind:?} {status}");
    }
}

#[test]
fn back_off_doubles_to_a_quarter_hour() {
    assert_eq!([1, 2, 3, 5, 6, 40].map(|n| wait_after(n, None)), [30, 60, 120, 480, 900, 900]);
    assert_eq!((wait_after(1, Some(600)), wait_after(1, Some(u32::MAX))), (600, 86_400));
}

fn person() -> impl Strategy<Value = Contact> {
    let word = prop::sample::select(vec!["", "a", "b"]);
    (
        word.clone(),
        word.clone(),
        word.clone(),
        word.clone(),
        word.clone(),
        prop::option::of(Just(mailcraft_time::Date { year: 2000, month: 1, day: 1 })),
        word,
    )
        .prop_map(|(first, last, company, notes, email, birthday, category)| Contact {
            first: first.into(),
            last: last.into(),
            company: company.into(),
            notes: notes.into(),
            emails: if email.is_empty() { Vec::new() } else { vec![Labeled::new("work", email)] },
            birthday,
            categories: if category.is_empty() { Vec::new() } else { vec![category.to_string()] },
            ..Default::default()
        })
}

proptest! {
    /// Changed in both places: what the owner changed is theirs, every other field the server's.
    #[test]
    fn a_merge_keeps_the_owners_fields_and_the_servers_rest(base in person(), mine in person(), theirs in person()) {
        let changed = ContactField::changed(&base, &mine);
        let merged = ContactField::merged(&mine, &theirs, &changed);
        for field in ContactField::ALL {
            let from = if changed.contains(&field) { &mine } else { &theirs };
            prop_assert!(!field.differs(&merged, from), "{field:?}");
        }
        // Nothing changed here: the server's version, whole.
        prop_assert_eq!(ContactField::merged(&base, &theirs, &ContactField::changed(&base, &base)), theirs.clone());
    }
}
