// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts kept in step over JMAP (CS10, RFC 9610), against the stand-in and against servers that answer
//! anything: address books and their rights, a whole read and a read of changes, a state the server can't
//! count from, the changes going up and what each `SetError` comes to, the caps. Every person is invented.

use std::collections::BTreeSet;

use mailcraft_model::Labeled;
use mailcraft_model::contact_sync::{ContactChangeKind, ContactField, ContactOp, ContactResult, FolderOwner};
use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use mailcraft_model::sync::{JmapContactPlace, JmapContactsRequest, JmapMailRequest, JmapMailResult};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::changes_tests::{Fake, ME, api_reply, asks, json_reply, last, run_fake, run_stand_in};
use crate::connection::tests::any_json;
use crate::standin::{ACCOUNT, CONTACTS, SESSION, StandIn};
use crate::{CAPABILITY_CORE as CORE, CAPABILITY_MAIL as MAIL, Method};

const ZOE: &str = "zoe@example.org";

/// A card of a person, in the address book `book`.
fn person(full: &str, email: &str, book: &str) -> Value {
    json!({"@type": "Card", "uid": format!("urn:uuid:{full}"), "kind": "individual", "addressBookIds": {book: true},
        "name": {"full": full}, "emails": {"e1": {"address": email}}, "imAddresses": {"i1": {"handle": "kept"}}})
}

/// A stand-in with an Inbox and contacts: the account's own Personal (default) and Archive (read
/// only) address books, one the user may not read, and Zoe's, shared. Their ids, in that order.
fn served() -> (StandIn, [String; 4]) {
    let stand_in = StandIn::new();
    let books = stand_in.with(|d| {
        d.mailbox("Inbox", None, Some("inbox"));
        d.contacts.on = true;
        d.contacts.shared = Some(("A2".into(), ZOE.into()));
        let personal = d.contacts.book(ACCOUNT, "Personal", true);
        let archive = d.contacts.book(ACCOUNT, "Archive", false);
        let hidden = d.contacts.book(ACCOUNT, "Hidden", false);
        let zoes = d.contacts.book("A2", "Zoe's people", true);
        for book in d.contacts.books.iter_mut() {
            book.may_write = book.id != archive;
            book.may_read = book.id != hidden;
        }
        [personal, archive, hidden, zoes]
    });
    (stand_in, books)
}

fn ask(contacts: JmapContactsRequest) -> JmapMailRequest {
    JmapMailRequest { contacts: Some(contacts), ..asks(SESSION) }
}

fn downloads(pieces: &[JmapMailResult]) -> Vec<ContactsDownload> {
    pieces.iter().flat_map(|p| p.contacts.iter().cloned()).collect()
}

fn folder_named<'a>(downloads: &'a [ContactsDownload], name: &str) -> &'a ContactsDownload {
    downloads.iter().find(|d| d.folder.as_ref().is_some_and(|f| f.name == name)).unwrap_or_else(|| panic!("no {name}: {downloads:?}"))
}

fn names(d: &ContactsDownload) -> BTreeSet<String> {
    d.contacts.iter().map(|c| c.display_name.clone()).collect()
}

/// Where each owner's books were read up to, as the engine would say after `downloads`.
fn places_after(downloads: &[ContactsDownload]) -> Vec<JmapContactPlace> {
    let mut places: Vec<JmapContactPlace> = Vec::new();
    for d in downloads {
        let f = d.folder.clone().unwrap();
        match places.iter_mut().find(|p| p.owner == f.owner) {
            Some(p) => p.books.push(f.id),
            None => places.push(JmapContactPlace { owner: f.owner, state: d.delta.clone(), books: vec![f.id] }),
        }
    }
    places
}

fn calls(stand_in: &StandIn) -> Vec<String> {
    stand_in.calls()
}

#[test]
fn address_books_come_with_their_rights_and_owners_and_are_read_whole_the_first_time() {
    let (stand_in, [personal, archive, hidden, zoes]) = served();
    stand_in.with(|d| {
        d.contacts.card(ACCOUNT, person("Bo Park", "bo@example.org", &personal));
        d.contacts.card(ACCOUNT, person("Cy Old", "cy@example.org", &archive));
        d.contacts.card(ACCOUNT, person("Secret", "s@example.org", &hidden));
        d.contacts.card("A2", person("Di Zoe", "di@example.org", &zoes));
        d.contacts.card(ACCOUNT, json!({"uid": "g", "kind": "group", "addressBookIds": {personal.clone(): true}, "name": {"full": "Team"}}));
    });
    let pieces = run_stand_in(&stand_in, &ask(JmapContactsRequest::default()));
    assert!(last(&pieces).errors.is_empty(), "{:?}", last(&pieces).errors);
    let got = downloads(&pieces);
    assert_eq!(got.len(), 3, "the book the user may not read is left out: {got:?}");
    let main = folder_named(&got, "Personal");
    let f = main.folder.as_ref().unwrap();
    // The default book is its owner's main folder.
    assert_eq!((f.id.as_str(), &f.owner, f.writable), ("", &FolderOwner::Own, true));
    assert_eq!(names(main), ["Bo Park".to_string()].into(), "a group is no contact");
    assert!(main.complete && !main.delta.is_empty() && main.read_at > 0 && main.gone.is_empty());
    assert_eq!(main.contacts[0].emails, vec![Labeled::new("", "bo@example.org")]);
    let old = folder_named(&got, "Archive").folder.clone().unwrap();
    assert_eq!((old.id.as_str(), old.writable), (archive.as_str(), false));
    let shared = folder_named(&got, "Zoe's people");
    assert_eq!(shared.folder.as_ref().unwrap().owner, FolderOwner::Person(ZOE.into()));
    assert_eq!(shared.folder.as_ref().unwrap().id, "", "the person's default book is their main folder");
    assert_eq!(names(shared), ["Di Zoe".to_string()].into());
    assert!(calls(&stand_in).contains(&"ContactCard/query".to_string()));
}

#[test]
fn a_server_without_contacts_is_left_alone_and_no_request_leaves_them_alone() {
    let stand_in = StandIn::new();
    stand_in.with(|d| d.mailbox("Inbox", None, Some("inbox")));
    let pieces = run_stand_in(&stand_in, &ask(JmapContactsRequest::default()));
    assert!(downloads(&pieces).is_empty() && last(&pieces).errors.is_empty());
    let (stand_in, _) = served();
    let pieces = run_stand_in(&stand_in, &asks(SESSION));
    assert!(downloads(&pieces).is_empty());
    assert!(!calls(&stand_in).iter().any(|c| c.starts_with("AddressBook")));
}

#[test]
fn from_a_state_only_what_changed_is_read_and_what_left_a_book_is_gone_from_it() {
    let (stand_in, [personal, archive, _, _]) = served();
    let (bo, cy, di) = stand_in.with(|d| {
        (
            d.contacts.card(ACCOUNT, person("Bo Park", "bo@example.org", &personal)),
            d.contacts.card(ACCOUNT, person("Cy Old", "cy@example.org", &personal)),
            d.contacts.card(ACCOUNT, person("Di Gone", "di@example.org", &personal)),
        )
    });
    let first = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest::default())));
    let places = places_after(&first);
    stand_in.with(|d| {
        d.contacts.change(&bo, "name", json!({"full": "Bo Parker"}));
        d.contacts.change(&cy, "addressBookIds", json!({archive.clone(): true}));
        d.contacts.remove(&di);
        d.contacts.card(ACCOUNT, person("Ed New", "ed@example.org", &personal));
    });
    let before = stand_in.calls().len();
    let got = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest { places, ..Default::default() })));
    let asked: Vec<String> = stand_in.calls().into_iter().skip(before).collect();
    assert!(asked.contains(&"ContactCard/changes".to_string()) && !asked.contains(&"ContactCard/query".to_string()), "{asked:?}");
    let main = folder_named(&got, "Personal");
    assert!(!main.complete);
    assert_eq!(names(main), ["Bo Parker".to_string(), "Ed New".to_string()].into());
    for gone in [&cy, &di] {
        assert!(main.gone.contains(gone), "{gone} {:?}", main.gone);
    }
    assert_eq!(names(folder_named(&got, "Archive")), ["Cy Old".to_string()].into());
    assert!(!folder_named(&got, "Archive").gone.contains(&cy));
    // Every book is read up to the same state.
    assert!(got.iter().all(|d| d.delta == got[0].delta && !d.delta.is_empty()), "{got:?}");
}

#[test]
fn a_state_the_server_cant_count_from_or_a_new_book_is_read_whole() {
    let (stand_in, [personal, ..]) = served();
    stand_in.with(|d| d.contacts.card(ACCOUNT, person("Bo Park", "bo@example.org", &personal)));
    let first = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest::default())));
    let places = places_after(&first);
    stand_in.with(|d| d.contacts.forget_before = 1_000);
    let got = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest { places: places.clone(), ..Default::default() })));
    assert!(folder_named(&got, "Personal").complete, "{got:?}");
    assert_eq!(names(folder_named(&got, "Personal")), ["Bo Park".to_string()].into());
    // A book that isn't here yet: whole again.
    stand_in.with(|d| {
        d.contacts.forget_before = 0;
        d.contacts.book(ACCOUNT, "Club", false);
    });
    let places = places_after(&got);
    let before = stand_in.calls().len();
    let got = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest { places, ..Default::default() })));
    assert!(stand_in.calls().into_iter().skip(before).any(|c| c == "ContactCard/query"));
    assert!(folder_named(&got, "Club").complete);
}

#[test]
fn a_whole_read_that_moved_under_it_or_paged_is_told_apart() {
    let (stand_in, [personal, ..]) = served();
    stand_in.with(|d| {
        d.max_get = 7;
        for n in 0..20 {
            d.contacts.card(ACCOUNT, person(&format!("P{n:02}"), &format!("p{n}@example.org"), &personal));
        }
    });
    let got = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest::default())));
    let main = folder_named(&got, "Personal");
    assert_eq!((main.contacts.len(), main.complete), (20, true), "paged, seven at a time");
    // The cards changed while they were read: what came stands, nothing is gone, and no state is kept.
    stand_in.with(|d| d.contacts.query_moves = true);
    let got = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest::default())));
    let main = folder_named(&got, "Personal");
    assert!(!main.complete && main.delta.is_empty() && main.gone.is_empty(), "{main:?}");
}

#[test]
fn many_changes_come_in_rounds_and_books_are_capped() {
    let (stand_in, [personal, ..]) = served();
    let first = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest::default())));
    let places = places_after(&first);
    stand_in.with(|d| {
        for n in 0..1_200 {
            d.contacts.card(ACCOUNT, person(&format!("Q{n}"), &format!("q{n}@example.org"), &personal));
        }
    });
    let got = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest { places, ..Default::default() })));
    assert_eq!(folder_named(&got, "Personal").contacts.len(), 1_200);
    assert!(stand_in.calls().iter().filter(|c| *c == "ContactCard/changes").count() >= 3);
    stand_in.with(|d| {
        for n in 0..300 {
            d.contacts.book(ACCOUNT, &format!("B{n}"), false);
        }
    });
    let got = downloads(&run_stand_in(&stand_in, &ask(JmapContactsRequest::default())));
    assert_eq!(got.len(), mailcraft_model::contact_sync::MAX_CONTACT_FOLDERS);
}

fn op(kind: ContactChangeKind, contact: u64, remote: &str, fields: &[ContactField], values: RemoteContact) -> ContactOp {
    ContactOp { kind, contact, remote: remote.into(), fields: fields.iter().copied().collect(), values, ..Default::default() }
}

#[test]
fn changes_go_up_as_a_new_card_a_patch_of_what_changed_and_a_destroy() {
    let (stand_in, [personal, _, _, zoes]) = served();
    let (bo, cy) = stand_in.with(|d| {
        (
            d.contacts.card(ACCOUNT, person("Bo Park", "bo@example.org", &personal)),
            d.contacts.card(ACCOUNT, person("Cy Old", "cy@example.org", &personal)),
        )
    });
    let new = RemoteContact { display_name: "Ed New".into(), emails: vec![Labeled::new("work", "ed@example.org")], ..Default::default() };
    let bo_now = RemoteContact { display_name: "Bo Parker".into(), emails: vec![Labeled::new("", "bo@example.org")], ..Default::default() };
    let in_zoes = ContactOp { owner: FolderOwner::Person(ZOE.into()), ..op(ContactChangeKind::Create, 4, "", &[], new.clone()) };
    let ops = vec![
        op(ContactChangeKind::Create, 1, "", &[], new.clone()),
        op(ContactChangeKind::Change, 2, &bo, &[ContactField::DisplayName], bo_now),
        op(ContactChangeKind::Delete, 3, &cy, &[], RemoteContact::default()),
        in_zoes,
    ];
    let pieces = run_stand_in(&stand_in, &ask(JmapContactsRequest { ops, ..Default::default() }));
    let sent = pieces.iter().find_map(|p| p.contacts_sent.clone()).unwrap();
    assert_eq!(sent.outcomes.len(), 4);
    assert!(sent.outcomes.iter().all(|o| o.result == ContactResult::Done), "{sent:?}");
    let made = &sent.outcomes[0].remote;
    stand_in.with(|d| {
        let card = d.contacts.get(made).unwrap();
        assert_eq!(card["addressBookIds"], json!({personal.clone(): true}));
        assert!(card["uid"].as_str().unwrap().starts_with("urn:uuid:"));
        // Only the full name went: the e-mail addresses and what MailCraft doesn't show are as they were.
        assert_eq!(d.contacts.patches, vec![(bo.clone(), json!({"name/full": "Bo Parker"}))]);
        assert_eq!(d.contacts.get(&bo).unwrap()["imAddresses"], json!({"i1": {"handle": "kept"}}));
        assert!(d.contacts.get(&cy).is_none());
        let theirs = d.contacts.get(&sent.outcomes[3].remote).unwrap();
        assert_eq!(theirs["addressBookIds"], json!({zoes.clone(): true}));
    });
    // The download after it shows what was made (taken in after the outcomes).
    let got = downloads(&pieces);
    assert!(names(folder_named(&got, "Personal")).contains("Ed New"));
}

#[test]
fn what_the_server_says_no_with_comes_to_gone_denied_refused_or_failed() {
    let (stand_in, [personal, archive, _, _]) = served();
    let ids: Vec<String> =
        stand_in.with(|d| (0..6).map(|n| d.contacts.card(ACCOUNT, person(&format!("R{n}"), &format!("r{n}@example.org"), &personal))).collect());
    stand_in.with(|d| {
        for (id, kind) in ids.iter().zip(["forbidden", "invalidProperties", "overQuota", "tooLarge", "serverFail", "somethingNew"]) {
            d.contacts.refused.insert(id.clone(), (kind.into(), "no".into()));
        }
    });
    let changed = |id: &str| op(ContactChangeKind::Change, 0, id, &[ContactField::Notes], RemoteContact { notes: "x".into(), ..Default::default() });
    let mut ops: Vec<ContactOp> = ids.iter().enumerate().map(|(n, id)| ContactOp { contact: n as u64, ..changed(id) }).collect();
    ops.push(ContactOp { contact: 10, ..changed("NOPE1") });
    ops.push(ContactOp { contact: 11, folder: archive.clone(), ..op(ContactChangeKind::Create, 11, "", &[], RemoteContact::default()) });
    ops.push(ContactOp { contact: 12, ..op(ContactChangeKind::Change, 12, "not an id!", &[], RemoteContact::default()) });
    let pieces = run_stand_in(&stand_in, &ask(JmapContactsRequest { ops, ..Default::default() }));
    let sent = pieces.iter().find_map(|p| p.contacts_sent.clone()).unwrap();
    let of = |contact: u64| sent.outcomes.iter().find(|o| o.op.contact == contact).map(|o| o.result);
    use ContactResult::*;
    assert_eq!((0..6).map(of).collect::<Vec<_>>(), vec![Some(Denied), Some(Refused), Some(Refused), Some(Refused), Some(Failed), Some(Refused)]);
    assert_eq!((of(10), of(11), of(12)), (Some(Gone), Some(Denied), Some(Refused)));
    // Why, in words, and never the server's address.
    assert!(sent.outcomes.iter().filter(|o| o.result != Done).all(|o| !o.error.is_empty() || o.result == Gone));
}

/// A server with contacts whose every call of them is answered as `f` says.
fn hostile(f: impl Fn(&str) -> Option<Value> + 'static) -> Fake {
    Fake::new(move |req, _| {
        if req.method() == Method::Get {
            return json_reply(
                200,
                &json!({
                    "capabilities": {CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": 50, "maxObjectsInSet": 50}, MAIL: {}, CONTACTS: {}},
                    "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {MAIL: {}, CONTACTS: {}}},
                                 "A2": {"name": "not an address", "isPersonal": false, "accountCapabilities": {CONTACTS: {}}}},
                    "primaryAccounts": {MAIL: "A1", CONTACTS: "A1"},
                    "apiUrl": "https://jmap.example.org/jmap/api",
                    "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
                    "uploadUrl": "/jmap/upload/{accountId}/",
                    "state": "1",
                }),
            );
        }
        api_reply(req, |name, args| match f(name) {
            Some(value) => super::changes_tests::Answer::Is(value),
            None => super::changes_tests::reading(name, args),
        })
    })
}

#[test]
fn a_busy_server_ends_the_contacts_and_the_changes_wait_out_its_time() {
    let fake = Fake::new(|req, _| {
        if req.method() == Method::Get {
            return hostile_session();
        }
        let body = String::from_utf8_lossy(req.body()).to_string();
        if body.contains("ContactCard/set") {
            return json_reply(429, &json!({"type": "about:blank", "status": 429}));
        }
        api_reply(req, |name, args| match name {
            "AddressBook/get" => super::changes_tests::Answer::Is(json!({"list": [{"id": "AB1", "name": "Mine", "isDefault": true}]})),
            _ => super::changes_tests::reading(name, args),
        })
    });
    let request = JmapMailRequest {
        session: super::changes_tests::FAKE_SESSION.into(),
        contacts: Some(JmapContactsRequest { ops: vec![op(ContactChangeKind::Create, 1, "", &[], RemoteContact::default())], ..Default::default() }),
        ..asks(super::changes_tests::FAKE_SESSION)
    };
    let pieces = run_fake(&fake, &request);
    let sent = pieces.iter().find_map(|p| p.contacts_sent.clone()).unwrap();
    assert_eq!(sent.outcomes[0].result, ContactResult::Failed);
    assert!(sent.outcomes[0].retry_after.is_some());
    assert!(last(&pieces).busy_for.is_some() && downloads(&pieces).is_empty());
}

#[test]
fn a_refused_sign_in_ends_the_contacts_and_the_rest_waits() {
    let fake = Fake::new(|req, _| {
        if req.method() == Method::Get {
            return hostile_session();
        }
        if String::from_utf8_lossy(req.body()).contains("ContactCard/set") {
            return json_reply(401, &json!({"type": "about:blank", "status": 401}));
        }
        api_reply(req, |name, args| match name {
            "AddressBook/get" => super::changes_tests::Answer::Is(json!({"list": [{"id": "AB1", "name": "Mine", "isDefault": true}]})),
            _ => super::changes_tests::reading(name, args),
        })
    });
    let ops = vec![op(ContactChangeKind::Create, 1, "", &[], RemoteContact::default())];
    let request = JmapMailRequest { contacts: Some(JmapContactsRequest { ops, ..Default::default() }), ..asks(super::changes_tests::FAKE_SESSION) };
    let pieces = run_fake(&fake, &request);
    let sent = pieces.iter().find_map(|p| p.contacts_sent.clone()).unwrap();
    assert!(sent.token_refused && sent.outcomes.is_empty(), "{sent:?}");
    assert!(last(&pieces).auth_refused && downloads(&pieces).is_empty());
}

fn hostile_session() -> super::changes_tests::Reply {
    json_reply(
        200,
        &json!({
            "capabilities": {CORE: {"maxCallsInRequest": 16}, MAIL: {}, CONTACTS: {}},
            "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {MAIL: {}, CONTACTS: {}}}},
            "primaryAccounts": {MAIL: "A1", CONTACTS: "A1"},
            "apiUrl": "https://jmap.example.org/jmap/api",
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
            "uploadUrl": "/jmap/upload/{accountId}/",
            "state": "1",
        }),
    )
}

#[test]
fn an_account_whose_name_is_no_address_is_passed_over() {
    let fake = hostile(|name| match name {
        "AddressBook/get" => Some(json!({"list": [{"id": "AB1", "name": "Mine", "isDefault": true}]})),
        "ContactCard/query" => Some(json!({"queryState": "q", "ids": [], "total": 0})),
        "ContactCard/get" => Some(json!({"state": "s1", "list": [], "notFound": []})),
        _ => None,
    });
    let request = JmapMailRequest { contacts: Some(JmapContactsRequest::default()), ..asks(super::changes_tests::FAKE_SESSION) };
    let got = downloads(&run_fake(&fake, &request));
    assert_eq!(got.len(), 1);
    assert!(got[0].complete && got[0].contacts.is_empty());
    let asked: Vec<String> = fake.sent.borrow().iter().map(|r| String::from_utf8_lossy(r.body()).to_string()).collect();
    assert!(!asked.iter().any(|b| b.contains("\"A2\"")));
}

#[test]
fn a_full_listing_names_every_book_and_a_book_that_cant_be_read_is_not_taken_for_gone() {
    let (stand_in, [_, archive, hidden, _]) = served();
    let gone_book = ContactOp { folder: hidden.clone(), ..op(ContactChangeKind::Delete, 1, "CC77", &[], RemoteContact::default()) };
    let pieces = run_stand_in(&stand_in, &ask(JmapContactsRequest { ops: vec![gone_book], ..Default::default() }));
    let listed: Vec<JmapContactPlace> = pieces.iter().flat_map(|p| p.contact_listed.clone()).collect();
    let own = listed.iter().find(|p| p.owner == FolderOwner::Own).unwrap();
    assert_eq!(own.books.iter().cloned().collect::<BTreeSet<_>>(), [String::new(), archive, hidden].into());
    assert!(listed.iter().any(|p| p.owner == FolderOwner::Person(ZOE.into())));
    // The deletion in the book that can't be read now waits: neither done nor sent.
    let sent = pieces.iter().find_map(|p| p.contacts_sent.clone()).unwrap();
    assert!(sent.outcomes.is_empty(), "{sent:?}");
}

#[test]
fn a_lost_answer_leaves_a_new_card_unconfirmed_and_what_was_done_already_done() {
    let fake = Fake::new(|req, _| {
        if req.method() == Method::Get {
            return hostile_session();
        }
        if String::from_utf8_lossy(req.body()).contains("ContactCard/set") {
            return json_reply(500, &json!({"type": "about:blank", "status": 500}));
        }
        api_reply(req, |name, args| match name {
            "AddressBook/get" => super::changes_tests::Answer::Is(json!({"list": [{"id": "AB1", "name": "Mine", "isDefault": true}]})),
            "ContactCard/get" if args.get("ids").and_then(Value::as_array).is_some_and(|ids| !ids.is_empty()) => super::changes_tests::Answer::Is(
                json!({"state": "s1", "list": [{"id": "CC1", "addressBookIds": {"AB1": true}, "notes": {"n1": {"note": "same"}}}], "notFound": []}),
            ),
            _ => super::changes_tests::reading(name, args),
        })
    });
    let ops = vec![
        op(ContactChangeKind::Change, 1, "CC1", &[ContactField::Notes], RemoteContact { notes: "same".into(), ..Default::default() }),
        op(ContactChangeKind::Create, 2, "", &[], RemoteContact { display_name: "New".into(), ..Default::default() }),
    ];
    let request = JmapMailRequest { contacts: Some(JmapContactsRequest { ops, ..Default::default() }), ..asks(super::changes_tests::FAKE_SESSION) };
    let sent = run_fake(&fake, &request).iter().find_map(|p| p.contacts_sent.clone()).unwrap();
    assert_eq!(sent.outcomes.len(), 2, "{sent:?}");
    let of = |contact: u64| sent.outcomes.iter().find(|o| o.op.contact == contact).unwrap();
    // The change the server had already: done, not failed by the call after it.
    assert_eq!((of(1).result, of(1).remote.as_str()), (ContactResult::Done, "CC1"));
    // The new card may have been made: made under a name not heard, so that it isn't made twice.
    assert_eq!((of(2).result, of(2).remote.as_str()), (ContactResult::Done, ""));
}

#[test]
fn a_deletion_is_never_a_destroy_on_a_guess_and_reads_follow_the_get_limit() {
    // A card whose books the server doesn't say.
    let fake = hostile(|name| match name {
        "AddressBook/get" => Some(json!({"list": [{"id": "AB1", "name": "Mine", "isDefault": true}]})),
        "ContactCard/get" => Some(json!({"state": "s1", "list": [{"id": "CC2"}], "notFound": []})),
        _ => None,
    });
    let ops = vec![op(ContactChangeKind::Delete, 3, "CC2", &[], RemoteContact::default())];
    let request = JmapMailRequest { contacts: Some(JmapContactsRequest { ops, ..Default::default() }), ..asks(super::changes_tests::FAKE_SESSION) };
    let pieces = run_fake(&fake, &request);
    let sent = pieces.iter().find_map(|p| p.contacts_sent.clone()).unwrap();
    assert_eq!(sent.outcomes.first().map(|o| o.result), Some(ContactResult::Failed), "{sent:?}");
    let bodies: Vec<String> = fake.sent.borrow().iter().map(|r| String::from_utf8_lossy(r.body()).to_string()).collect();
    assert!(!bodies.iter().any(|b| b.contains("ContactCard/set")));

    // Two changes, a server that gives one card a `get`: two gets, then the set.
    let (stand_in, [personal, ..]) = served();
    let (a, b) = stand_in.with(|d| {
        d.max_get = 1;
        (d.contacts.card(ACCOUNT, person("A A", "a@example.org", &personal)), d.contacts.card(ACCOUNT, person("B B", "b@example.org", &personal)))
    });
    let noted = RemoteContact { notes: "x".into(), ..Default::default() };
    let ops = vec![
        op(ContactChangeKind::Change, 1, &a, &[ContactField::Notes], noted.clone()),
        op(ContactChangeKind::Change, 2, &b, &[ContactField::Notes], noted),
    ];
    let pieces = run_stand_in(&stand_in, &ask(JmapContactsRequest { ops, ..Default::default() }));
    let sent = pieces.iter().find_map(|p| p.contacts_sent.clone()).unwrap();
    for contact in [1, 2] {
        let mine: Vec<ContactResult> = sent.outcomes.iter().filter(|o| o.op.contact == contact).map(|o| o.result).collect();
        assert_eq!(mine, vec![ContactResult::Done], "{sent:?}");
    }
    // Both patched, in one set after a get of each.
    let patched: BTreeSet<String> = stand_in.with(|d| d.contacts.patches.iter().map(|(id, _)| id.clone()).collect());
    assert_eq!(patched, [a, b].into());
    let asked = calls(&stand_in);
    let set_at = asked.iter().position(|m| m == "ContactCard/set").unwrap();
    assert!(asked.get(..set_at).unwrap().iter().filter(|m| *m == "ContactCard/get").count() >= 2, "{asked:?}");
}

#[test]
fn a_card_listed_twice_across_pages_or_without_its_books_is_no_complete_read() {
    let twice = hostile(|name| match name {
        "AddressBook/get" => Some(json!({"list": [{"id": "AB1", "name": "Mine", "isDefault": true}]})),
        // Every page lists the same card, and the total says there are two.
        "ContactCard/query" => Some(json!({"queryState": "q", "ids": ["CA"], "total": 2})),
        "ContactCard/get" => {
            Some(json!({"state": "s1", "list": [{"id": "CA", "addressBookIds": {"AB1": true}, "name": {"full": "A"}}], "notFound": []}))
        }
        _ => None,
    });
    let request = JmapMailRequest { contacts: Some(JmapContactsRequest::default()), ..asks(super::changes_tests::FAKE_SESSION) };
    let got = downloads(&run_fake(&twice, &request));
    assert!(got.len() == 1 && !got[0].complete && got[0].delta.is_empty(), "{got:?}");

    // Read from changes: a card that doesn't say its books stops the read where it was.
    let unsaid = hostile(|name| match name {
        "AddressBook/get" => Some(json!({"list": [{"id": "AB1", "name": "Mine", "isDefault": true}]})),
        "ContactCard/changes" => {
            Some(json!({"oldState": "s1", "newState": "s2", "hasMoreChanges": false, "created": ["CB"], "updated": [], "destroyed": []}))
        }
        "ContactCard/get" => Some(json!({"state": "s2", "list": [{"id": "CB", "name": {"full": "B"}}], "notFound": []})),
        _ => None,
    });
    let places = vec![JmapContactPlace { owner: FolderOwner::Own, state: "s1".into(), books: vec![String::new()] }];
    let request =
        JmapMailRequest { contacts: Some(JmapContactsRequest { places, ..Default::default() }), ..asks(super::changes_tests::FAKE_SESSION) };
    let pieces = run_fake(&unsaid, &request);
    assert!(downloads(&pieces).is_empty(), "nothing taken, the state not moved on");
    assert!(!last(&pieces).errors.is_empty());
}

#[test]
fn a_get_that_lists_a_card_twice_and_leaves_one_out_is_no_complete_read() {
    let fake = hostile(|name| match name {
        "AddressBook/get" => Some(json!({"list": [{"id": "AB1", "name": "Mine", "isDefault": true}]})),
        "ContactCard/query" => Some(json!({"queryState": "q", "ids": ["CA", "CB"], "total": 2})),
        "ContactCard/get" => Some(json!({"state": "s1", "list": [
            {"id": "CA", "addressBookIds": {"AB1": true}, "name": {"full": "A"}},
            {"id": "CA", "addressBookIds": {"AB1": true}, "name": {"full": "A"}}], "notFound": []})),
        _ => None,
    });
    let request = JmapMailRequest { contacts: Some(JmapContactsRequest::default()), ..asks(super::changes_tests::FAKE_SESSION) };
    let got = downloads(&run_fake(&fake, &request));
    assert_eq!(got.len(), 1);
    assert!(!got[0].complete && got[0].delta.is_empty(), "{got:?}");
    assert_eq!(got[0].contacts.len(), 1);
}

#[test]
fn a_whole_read_whose_last_page_crosses_the_cap_is_not_complete() {
    let total = mailcraft_model::remote::MAX_CONTACTS + 3;
    let fake = Fake::new(move |req, _| {
        if req.method() == Method::Get {
            return json_reply(
                200,
                &json!({
                    "capabilities": {CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": 7, "maxObjectsInSet": 50}, MAIL: {}, CONTACTS: {}},
                    "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {MAIL: {}, CONTACTS: {}}}},
                    "primaryAccounts": {MAIL: "A1", CONTACTS: "A1"},
                    "apiUrl": "https://jmap.example.org/jmap/api",
                    "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
                    "uploadUrl": "/jmap/upload/{accountId}/",
                    "state": "1",
                }),
            );
        }
        api_reply(req, move |name, args| match name {
            "AddressBook/get" => super::changes_tests::Answer::Is(json!({"list": [{"id": "AB1", "name": "Mine", "isDefault": true}]})),
            "ContactCard/query" => {
                let at = args.get("position").and_then(Value::as_u64).unwrap_or(0) as usize;
                let ids: Vec<String> = (at..(at + 7).min(total)).map(|n| format!("C{n}")).collect();
                super::changes_tests::Answer::Is(json!({"queryState": "q", "ids": ids, "total": total}))
            }
            "ContactCard/get" => {
                let ids: Vec<Value> = args.get("ids").and_then(Value::as_array).cloned().unwrap_or_default();
                let list: Vec<Value> = ids.iter().map(|id| json!({"id": id, "addressBookIds": {"AB1": true}, "name": {"full": id}})).collect();
                super::changes_tests::Answer::Is(json!({"state": "s1", "list": list, "notFound": []}))
            }
            _ => super::changes_tests::reading(name, args),
        })
    });
    let request = JmapMailRequest { contacts: Some(JmapContactsRequest::default()), ..asks(super::changes_tests::FAKE_SESSION) };
    let got = downloads(&run_fake(&fake, &request));
    assert_eq!(got.len(), 1);
    assert!(!got[0].complete && got[0].delta.is_empty(), "complete {} delta {:?}", got[0].complete, got[0].delta);
}

#[test]
fn two_shared_accounts_under_one_name_are_one_place() {
    let fake = Fake::new(|req, _| {
        if req.method() == Method::Get {
            return json_reply(
                200,
                &json!({
                    "capabilities": {CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": 50, "maxObjectsInSet": 50}, MAIL: {}, CONTACTS: {}},
                    "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {MAIL: {}, CONTACTS: {}}},
                                 "A2": {"name": ZOE, "isPersonal": false, "accountCapabilities": {CONTACTS: {}}},
                                 "A3": {"name": ZOE, "isPersonal": false, "accountCapabilities": {CONTACTS: {}}}},
                    "primaryAccounts": {MAIL: "A1", CONTACTS: "A1"},
                    "apiUrl": "https://jmap.example.org/jmap/api",
                    "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
                    "uploadUrl": "/jmap/upload/{accountId}/",
                    "state": "1",
                }),
            );
        }
        api_reply(req, |name, args| match name {
            "AddressBook/get" => super::changes_tests::Answer::Is(json!({"list": [{"id": "AB1", "name": "Book", "isDefault": true}]})),
            "ContactCard/query" => super::changes_tests::Answer::Is(json!({"queryState": "q", "ids": [], "total": 0})),
            "ContactCard/get" => super::changes_tests::Answer::Is(json!({"state": "s1", "list": [], "notFound": []})),
            _ => super::changes_tests::reading(name, args),
        })
    });
    let request = JmapMailRequest { contacts: Some(JmapContactsRequest::default()), ..asks(super::changes_tests::FAKE_SESSION) };
    let got = downloads(&run_fake(&fake, &request));
    let zoes = got.iter().filter(|d| d.folder.as_ref().is_some_and(|f| f.owner == FolderOwner::Person(ZOE.into()))).count();
    assert_eq!(zoes, 1, "{got:?}");
    let asked: Vec<String> = fake.sent.borrow().iter().map(|r| String::from_utf8_lossy(r.body()).to_string()).collect();
    let asked_of = |account: &str| asked.iter().any(|b| b.contains(&format!("\"{account}\"")));
    assert!(asked_of("A2") != asked_of("A3"), "one of the two, never both");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]
    #[test]
    fn whatever_a_server_answers_for_contacts_nothing_panics(books in any_json(), cards in any_json(), changes in any_json(), set in any_json()) {
        let fake = hostile(move |name| match name {
            "AddressBook/get" => Some(books.clone()),
            "ContactCard/get" | "ContactCard/query" => Some(cards.clone()),
            "ContactCard/changes" => Some(changes.clone()),
            "ContactCard/set" => Some(set.clone()),
            _ => None,
        });
        let ops = vec![
            op(ContactChangeKind::Create, 1, "", &[], RemoteContact { display_name: "A".into(), ..Default::default() }),
            op(ContactChangeKind::Change, 2, "CC1", &[ContactField::Notes], RemoteContact::default()),
            op(ContactChangeKind::Delete, 3, "CC2", &[], RemoteContact::default()),
        ];
        let places = vec![JmapContactPlace { owner: FolderOwner::Own, state: "s1".into(), books: vec![String::new()] }];
        let request = JmapMailRequest { contacts: Some(JmapContactsRequest { ops, places }), ..asks(super::changes_tests::FAKE_SESSION) };
        let pieces = super::changes_tests::run_unchecked(fake, super::changes_tests::FAKE_TOKEN, &request, &std::sync::atomic::AtomicBool::new(false));
        prop_assert!(!pieces.is_empty());
    }
}
