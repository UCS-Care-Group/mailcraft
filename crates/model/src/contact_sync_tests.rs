// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::BTreeSet;

use proptest::prelude::*;

use crate::contact_sync::{
    ContactChange, ContactChangeKind, ContactField, ContactFolder, ContactResult, FolderOwner, MAX_CONTACT_CHANGES, MAX_CONTACT_FOLDERS, wait_after,
};
use crate::{Account, Contact, Id, Labeled, Mailbox, Origin};

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
    let folder = file.contact_folders[0].id;
    file.contact_changes = vec![ContactChange { account, folder, contact: 4_242, sent: true, ..Default::default() }; MAX_CONTACT_CHANGES + 1];
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
    let main = mb.ensure_main_contact_folder(account).unwrap();
    mb.contacts.insert(TOO_BIG + 1, std::sync::Arc::new(Contact { id: TOO_BIG + 1, folder: main, ..ada() }));
    mb.contact_changes.push(ContactChange { account, kind: ContactChangeKind::Create, contact: TOO_BIG + 1, folder: main, ..Default::default() });
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

/// Review of #1937: only folders that can be kept count towards the cap; new keys never land on
/// an item still to be given one; a deletion waiting under a key out of range gets one in range.
#[test]
fn repair_counts_only_kept_folders_and_rehomes_without_losing_anything() {
    const MAX: crate::Id = 1 << 53;
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    mb.contact_folders.push(ContactFolder { id: 0, account, remote: "bad".into(), ..Default::default() });
    mb.contact_folders.push(ContactFolder { id: MAX + 9, account, remote: "big".into(), ..Default::default() });
    for n in 0..MAX_CONTACT_FOLDERS {
        let id = mb.alloc_id();
        mb.contact_folders.push(ContactFolder { id, account, remote: format!("f-{n}"), ..Default::default() });
    }
    let main = mb.contact_folders.last().map(|f| f.id).unwrap();
    // A contact at the top of the range, two above it, and two events above it whose keys the
    // contacts' new ones would reach.
    mb.contacts.insert(MAX, std::sync::Arc::new(Contact { id: MAX, ..ada() }));
    for k in [MAX + 1, MAX + 2] {
        mb.contacts.insert(k, std::sync::Arc::new(Contact { id: k, first: format!("c{k}"), ..Default::default() }));
    }
    for k in [MAX + 1, MAX + 3] {
        mb.events.insert(k, std::sync::Arc::new(crate::Event { id: k, title: format!("e{k}"), ..Default::default() }));
    }
    let base = Some(Box::new(ada()));
    mb.contact_changes.push(ContactChange {
        account,
        kind: ContactChangeKind::Delete,
        contact: MAX + 50,
        folder: main,
        remote: "c-9".into(),
        base,
        ..Default::default()
    });
    mb.repair();
    assert_eq!(mb.contact_folders.len(), MAX_CONTACT_FOLDERS);
    assert_eq!((mb.contacts.len(), mb.events.len()), (3, 2));
    assert_eq!(mb.contact_changes.len(), 1);
    // The deletion's key is the next one after every key given out: no item has it, and the
    // next new item doesn't get it.
    let id = mb.contact_changes[0].contact;
    let top = mb.contacts.keys().chain(mb.events.keys()).copied().max().unwrap();
    assert_eq!(id, top + 1);
    assert!(mb.clone().alloc_id() > id);
}

/// Review of #1934, second pass: what a file keeps of contacts folders and changes waiting.
/// A change waits only when it can still be sent, and the cap counts only those; a deletion's
/// key is never given out again; a folder kept is cut to size like one from a server, and a
/// folder given twice takes no room of its account's.
#[test]
fn repair_drops_before_it_caps_keeps_deletions_keys_and_tidies_folders() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    let main = mb.ensure_main_contact_folder(account).unwrap();
    let here = mb.add_contact(Contact { folder: main, origin: Origin { account, id: "c-1".into(), ..Default::default() }, ..ada() });
    let sent = |contact| ContactChange { account, kind: ContactChangeKind::Create, folder: main, contact, sent: true, ..Default::default() };
    // Of a contact that is gone: a change, and a new contact never sent, go.
    let missing: Id = 1_000_000;
    let mut changes = vec![
        ContactChange {
            account,
            kind: ContactChangeKind::Change,
            contact: missing,
            folder: main,
            remote: "c-2".into(),
            base: Some(Box::new(ada())),
            fields: BTreeSet::from([ContactField::Last]),
            ..Default::default()
        },
        ContactChange { sent: false, ..sent(missing + 1) },
    ];
    // Then as many as may wait that can be sent: none of them is cut for the two above.
    changes.extend((0..MAX_CONTACT_CHANGES as Id).map(|n| sent(missing + 10 + n)));
    // A deletion of a contact whose key is above every other.
    let deleted: Id = 3_000_000;
    changes[2] = ContactChange { kind: ContactChangeKind::Delete, remote: "c-9".into(), base: Some(Box::new(ada())), ..sent(deleted) };
    mb.contact_changes = changes;
    // A folder with a name from outside, one whose owner isn't an address, and the main folder
    // given twice, ahead of a full set.
    let copy = mb.contact_folders[0].clone();
    mb.contact_folders.insert(0, copy);
    for n in 1..MAX_CONTACT_FOLDERS {
        let id = mb.alloc_id();
        let name = if n == 1 { format!("Sales\u{0}{}", "x".repeat(10_000)) } else { format!("f-{n}") };
        mb.contact_folders.push(ContactFolder { id, account, remote: format!("f-{n}"), name, ..Default::default() });
    }
    let odd = mb.alloc_id();
    let nobody = FolderOwner::Person("nobody".into());
    mb.contact_folders.insert(1, ContactFolder { id: odd, account, remote: "odd".into(), owner: nobody, ..Default::default() });
    let mut mb: Mailbox = serde_json::from_str(&serde_json::to_string(&mb).unwrap()).unwrap();
    mb.repair();
    assert_eq!(mb.contact_changes.len(), MAX_CONTACT_CHANGES);
    assert!(mb.contact_changes.iter().all(|c| c.contact != missing && c.contact != missing + 1));
    assert!(mb.contact_changes.iter().any(|c| c.contact == deleted && c.kind == ContactChangeKind::Delete));
    let next = mb.clone().alloc_id();
    assert!(next > deleted && next > missing + 10 + MAX_CONTACT_CHANGES as Id, "{next}");
    let fresh = mb.add_contact(Contact { first: "Bo".into(), ..Default::default() });
    assert!(mb.contact_changes.iter().all(|c| c.contact != fresh));
    // Folders: the twin goes without taking room, the one with no address goes, the rest stay.
    assert_eq!(mb.contact_folders.len(), MAX_CONTACT_FOLDERS);
    assert!(mb.contact_folder(odd).is_none());
    assert_eq!(mb.contact_folders.iter().filter(|f| f.id == main).count(), 1);
    let sales = mb.contact_folders.iter().find(|f| f.remote == "f-1").unwrap();
    assert!(sales.name.starts_with("Sales") && sales.name.chars().count() <= crate::MAX_NAME && !sales.name.contains('\u{0}'), "{}", sales.name);
    assert_eq!(mb.contact(here).map(|c| c.folder), Some(main));
    assert!(mb.repair().is_empty());
}

/// Re-review of #1944: a move's link to the contact it came from follows that one's new key, so
/// the deletion waiting for the move stays held.
#[test]
fn repair_gives_a_moves_link_the_new_key_of_the_contact_it_came_from() {
    const MAX: crate::Id = 1 << 53;
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    let main = mb.ensure_main_contact_folder(account).unwrap();
    let team = mb.add_contact_folder(ContactFolder { account, remote: "f-team".into(), ..Default::default() }).unwrap();
    // Moved from main (its deletion waits, under a key out of range) into team (to be made
    // there), itself under a key out of range.
    mb.contacts.insert(MAX + 7, std::sync::Arc::new(Contact { id: MAX + 7, folder: team, moved_from: MAX + 3, ..ada() }));
    mb.contact_changes.push(ContactChange { account, kind: ContactChangeKind::Create, contact: MAX + 7, folder: team, ..Default::default() });
    let base = Some(Box::new(Contact { folder: main, origin: Origin { account, id: "c-ada".into(), ..Default::default() }, ..ada() }));
    mb.contact_changes.push(ContactChange {
        account,
        kind: ContactChangeKind::Delete,
        contact: MAX + 3,
        folder: main,
        remote: "c-ada".into(),
        base,
        ..Default::default()
    });
    let mut file: Mailbox = serde_json::from_str(&serde_json::to_string(&mb).unwrap()).unwrap();
    file.repair();
    let moved = file.contacts.values().next().unwrap();
    let delete = file.contact_changes.iter().find(|c| c.kind == ContactChangeKind::Delete).unwrap();
    assert!(moved.id <= MAX && delete.contact <= MAX);
    assert_eq!(moved.moved_from, delete.contact);
}

/// Review of #1935/#1936: a contact of an account that is gone is this computer's, and a change
/// that couldn't be sent or put back doesn't wait for ever.
#[test]
fn repair_frees_contacts_of_gone_accounts_and_drops_changes_that_cant_be_sent() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    let main = mb.ensure_main_contact_folder(account).unwrap();
    let stray = mb.add_contact(Contact { origin: Origin { account: 999, id: "x".into(), ..Default::default() }, ..ada() });
    let id = mb.add_contact(Contact { folder: main, origin: Origin { account, id: "c-1".into(), ..Default::default() }, ..ada() });
    let change = |kind, remote: &str, base: bool, fields: &[ContactField]| ContactChange {
        account,
        kind,
        contact: id,
        folder: main,
        remote: remote.into(),
        base: base.then(|| Box::new(ada())),
        fields: fields.iter().copied().collect(),
        ..Default::default()
    };
    mb.contact_changes = vec![
        change(ContactChangeKind::Delete, "c-1", false, &[]),
        change(ContactChangeKind::Delete, "", true, &[]),
        change(ContactChangeKind::Change, "c-1", true, &[]),
        change(ContactChangeKind::Change, "c-1", false, &[ContactField::Last]),
        ContactChange { folder: 4242, ..change(ContactChangeKind::Create, "", false, &[]) },
        ContactChange { account: 999, ..change(ContactChangeKind::Create, "", false, &[]) },
        change(ContactChangeKind::Change, "c-1", true, &[ContactField::Last]),
        ContactChange { contact: 4_243, ..change(ContactChangeKind::Delete, "c-1", true, &[]) },
        change(ContactChangeKind::Create, "", false, &[]),
    ];
    let said = mb.repair();
    assert!(said.iter().any(|l| l.contains("couldn't be sent")), "{said:?}");
    assert_eq!(
        mb.contact_changes.iter().map(|c| c.kind).collect::<Vec<_>>(),
        [ContactChangeKind::Change, ContactChangeKind::Delete, ContactChangeKind::Create]
    );
    assert!(mb.contact(stray).unwrap().origin.is_local());
    assert!(mb.repair().is_empty());
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

/// Review of #1937 and #1934 (fourth round): a change waits only in a folder of its own
/// account, a change needs its contact (a new contact gone here only once sent), and a change
/// moved with its contact keeps it even when the new key is out of range too.
#[test]
fn repair_keeps_only_changes_that_can_go_to_their_own_accounts_folder() {
    const MAX: crate::Id = 1 << 53;
    let mut mb = Mailbox::default();
    let ada_account = mb.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    let bo_account = mb.add_account(Account { email: "bo@example.org".into(), ..Default::default() });
    let adas = mb.ensure_main_contact_folder(ada_account).unwrap();
    let bos = mb.ensure_main_contact_folder(bo_account).unwrap();
    let id = mb.add_contact(Contact { folder: adas, ..ada() });
    let note = |kind, contact, folder, sent| ContactChange { account: ada_account, kind, contact, folder, sent, ..Default::default() };
    // At the top of the range, so the one moved gets a key out of range again.
    mb.contacts.insert(MAX, std::sync::Arc::new(Contact { id: MAX, folder: adas, ..ada() }));
    mb.contacts.insert(MAX + 5, std::sync::Arc::new(Contact { id: MAX + 5, first: "Big".into(), folder: adas, ..Default::default() }));
    mb.contact_changes = vec![
        note(ContactChangeKind::Create, id, bos, false),
        note(ContactChangeKind::Create, 4_242, adas, false),
        ContactChange {
            remote: "c-1".into(),
            base: Some(Box::new(ada())),
            fields: BTreeSet::from([ContactField::Last]),
            ..note(ContactChangeKind::Change, 4_243, adas, true)
        },
        ContactChange {
            remote: "c-2".into(),
            base: Some(Box::new(ada())),
            fields: BTreeSet::from([ContactField::Last]),
            ..note(ContactChangeKind::Change, MAX + 40, adas, true)
        },
        note(ContactChangeKind::Create, 4_244, adas, true),
        note(ContactChangeKind::Create, id, adas, false),
        note(ContactChangeKind::Create, MAX + 5, adas, false),
    ];
    mb.repair();
    let big = mb.contacts.values().find(|c| c.first == "Big").unwrap().id;
    assert_eq!(mb.contact_changes.iter().map(|c| c.contact).collect::<Vec<_>>(), [4_244, id, big]);
}

/// Review of #1934 (fifth round): a note must name its contact as it is. A new or changed
/// contact that is here is in the note's folder, a deleted one isn't, a new one has no server
/// name, and a sent one gone here names a contact.
#[test]
fn repair_drops_notes_that_dont_match_their_contact() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "ada@example.org".into(), ..Default::default() });
    let main = mb.ensure_main_contact_folder(account).unwrap();
    let other = mb.add_contact_folder(ContactFolder { account, name: "Other".into(), writable: true, ..Default::default() }).unwrap();
    let id = mb.add_contact(Contact { folder: main, ..ada() });
    let note = |kind, contact, folder| ContactChange { account, kind, contact, folder, sent: true, ..Default::default() };
    let sent = |kind, folder| ContactChange {
        remote: "c-1".into(),
        base: Some(Box::new(ada())),
        fields: BTreeSet::from([ContactField::Last]),
        ..note(kind, id, folder)
    };
    mb.contact_changes = vec![
        note(ContactChangeKind::Create, id, other),
        sent(ContactChangeKind::Change, other),
        sent(ContactChangeKind::Delete, main),
        ContactChange { remote: "c-1".into(), ..note(ContactChangeKind::Create, id, main) },
        note(ContactChangeKind::Create, 0, main),
        // Kept: each as it should be.
        note(ContactChangeKind::Create, id, main),
        sent(ContactChangeKind::Change, main),
        sent(ContactChangeKind::Delete, other),
        note(ContactChangeKind::Create, 4_244, main),
    ];
    mb.repair();
    let kept: Vec<_> = mb.contact_changes.iter().map(|c| (c.kind, c.contact, c.folder)).collect();
    assert_eq!(
        kept,
        [
            (ContactChangeKind::Create, id, main),
            (ContactChangeKind::Change, id, main),
            (ContactChangeKind::Delete, id, other),
            (ContactChangeKind::Create, 4_244, main)
        ]
    );
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

/// #1917: what a server has no place for is said for a folder of an account whose server limits
/// places (Microsoft's), by position in the contact's own lists; never On this computer, nor for
/// a server that keeps every entry.
#[test]
fn local_only_names_what_the_server_has_no_place_for() {
    use crate::contact_sync::{LocalOnly, local_only};
    use crate::{AccountKind, OAuth, OAuthProvider, PostalAddress};
    let mut mb = Mailbox::default();
    let oauth = OAuth { provider: OAuthProvider::Microsoft, ..Default::default() };
    let ms = mb.add_account(Account { email: "ada@example.org".into(), kind: AccountKind::Microsoft, oauth: Some(oauth), ..Default::default() });
    let imap = mb.add_account(Account { email: "bo@example.org".into(), kind: AccountKind::Imap, ..Default::default() });
    let ms_folder = mb.ensure_main_contact_folder(ms).unwrap();
    let imap_folder = mb.ensure_main_contact_folder(imap).unwrap();
    let address = |label: &str, city: &str| PostalAddress { label: label.into(), city: city.into(), ..Default::default() };
    let many = Contact {
        // An empty one between is neither placed nor marked.
        emails: ["a", "", "b", "c", "d", "e"]
            .iter()
            .map(|n| Labeled::new("work", if n.is_empty() { String::new() } else { format!("{n}@example.org") }))
            .collect(),
        phones: vec![Labeled::new("mobile", "+44 7700 900001"), Labeled::new("mobile", "+44 7700 900002"), Labeled::new("home", "+44 20 7946 0001")],
        addresses: vec![address("home", "Leeds"), address("home", "York"), address("work", "Hull"), address("other", "Bath")],
        websites: vec!["https://a.example".into(), "https://b.example".into()],
        ..ada()
    };
    let at = |folder| Contact { folder, ..many.clone() };
    let expected = LocalOnly { emails: vec![4, 5], phones: vec![1], addresses: vec![3], websites: vec![1] };
    assert_eq!(local_only(&mb, &at(ms_folder)), expected);
    assert!(local_only(&mb, &at(0)).is_empty(), "On this computer: nothing is marked");
    assert!(local_only(&mb, &at(imap_folder)).is_empty(), "a server with no limit: nothing is marked");
    assert!(local_only(&mb, &Contact { folder: ms_folder, ..ada() }).is_empty(), "what fits isn't marked");
}

/// #1917: only a Microsoft (Graph) account's folders are marked: an account of another kind that
/// keeps contacts (an Exchange server's, a JMAP server's) has no such limits.
#[test]
fn an_account_that_keeps_contacts_but_not_through_graph_is_never_marked() {
    use crate::contact_sync::local_only;
    use crate::{AccountKind, OAuth, OAuthProvider};
    let mut mb = Mailbox::default();
    let microsoft = || Some(OAuth { provider: OAuthProvider::Microsoft, ..Default::default() });
    let exchange = mb.add_account(Account { email: "ex@example.org".into(), kind: AccountKind::Exchange, oauth: microsoft(), ..Default::default() });
    let jmap = mb.add_account(Account { email: "jm@example.org".into(), kind: AccountKind::Jmap, oauth: microsoft(), ..Default::default() });
    // One that keeps contacts without Graph: its kind decides, not that it keeps contacts.
    assert!(mb.account(exchange).unwrap().keeps_contacts());
    for account in [exchange, jmap] {
        assert!(!mb.account(account).unwrap().contacts_in_graph());
        let folder = mb.ensure_main_contact_folder(account).unwrap();
        let many = Contact {
            folder,
            emails: (1..=5).map(|n| Labeled::new("work", format!("a{n}@example.org"))).collect(),
            websites: vec!["https://a.example".into(), "https://b.example".into()],
            ..ada()
        };
        assert!(local_only(&mb, &many).is_empty(), "account {account}");
    }
}
