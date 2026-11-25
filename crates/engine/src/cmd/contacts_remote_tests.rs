// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts going up to their servers (#1917): the queue, the answers, the merge, undo.

use mailcraft_model::contact_sync::{
    ContactChangeKind, ContactField, ContactOp, ContactOutcome, ContactResult, ContactsUpload, ContactsUploaded, FolderOwner, MAX_CONTACT_CHANGES,
    RemoteContactFolder,
};
use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use mailcraft_model::{Id, Labeled};
use serde_json::{Value, json};

use crate::{CmdError, Session};

const NOW: i64 = 1_790_000_000;
const EMAIL: &str = "ada@example.org";

/// The sample mailbox with a Microsoft account whose server has Bo and Cy.
fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let account =
        run(&mut s, "account.add", json!({"email": EMAIL, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}))["id"].as_u64().unwrap();
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy")])}));
    (s, account)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn person(id: &str, first: &str) -> RemoteContact {
    RemoteContact {
        id: id.into(),
        change_key: format!("ck-{id}-1"),
        first: first.into(),
        emails: vec![Labeled::new("work", format!("{}@example.org", first.to_lowercase()))],
        ..Default::default()
    }
}

fn download(account: Id, contacts: Vec<RemoteContact>) -> ContactsDownload {
    ContactsDownload { account, email: EMAIL.into(), complete: true, contacts, ..Default::default() }
}

fn called(s: &Session, first: &str) -> Id {
    s.mailbox.contacts.values().find(|c| c.first == first).map(|c| c.id).unwrap_or_else(|| panic!("no {first}"))
}

fn upload(s: &mut Session, account: Id) -> ContactsUpload {
    serde_json::from_value(run(s, "sync.contactChanges", json!({"account": account}))).unwrap()
}

/// The server's answer to `op`.
fn answer(s: &mut Session, account: Id, op: &ContactOp, result: ContactResult, remote: &str) -> Value {
    let outcome = ContactOutcome { op: op.clone(), result, remote: remote.into(), change_key: "ck-new".into(), ..Default::default() };
    let uploaded = ContactsUploaded { account, email: EMAIL.into(), outcomes: vec![outcome], ..Default::default() };
    run(s, "sync.applyContactChanges", json!({"result": uploaded}))
}

/// The notes waiting, as (contact, kind, fields).
fn notes(s: &Session) -> Vec<(Id, ContactChangeKind, Vec<ContactField>)> {
    s.mailbox.contact_changes.iter().map(|c| (c.contact, c.kind, c.fields.iter().copied().collect())).collect()
}

#[test]
fn a_new_contact_in_an_accounts_folder_goes_up_whole_and_takes_the_servers_id() {
    let (mut s, account) = session();
    let made = run(&mut s, "people.new", json!({"first": "Dee", "email": "dee@example.org", "categories": ["Red category"]}))["id"].as_u64().unwrap();
    let main = s.mailbox.main_contact_folder(account).unwrap();
    assert_eq!(notes(&s), [(made, ContactChangeKind::Create, vec![])]);
    assert_eq!(run(&mut s, "people.get", json!({"id": made}))["notSent"], true);
    let sent = upload(&mut s, account);
    let [op] = sent.ops.as_slice() else { panic!("{sent:?}") };
    assert_eq!((op.kind, op.contact, op.folder.as_str(), &op.owner, op.fields.len()), (ContactChangeKind::Create, made, "", &FolderOwner::Own, 15));
    assert_eq!((op.values.first.as_str(), op.values.categories.as_deref()), ("Dee", Some(&["Red category".to_string()][..])));
    answer(&mut s, account, op, ContactResult::Done, "c-dee");
    let c = s.mailbox.contact(made).unwrap();
    assert_eq!((c.origin.id.as_str(), c.origin.change_key.as_str(), c.folder), ("c-dee", "ck-new", main));
    assert!(s.mailbox.contact_changes.is_empty());
    assert_eq!(run(&mut s, "people.get", json!({"id": made}))["notSent"], false);
}

#[test]
fn an_edit_sends_only_the_fields_the_owner_changed_and_a_favourite_stays_here() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.favorite", json!({"id": bo, "value": true}));
    assert!(s.mailbox.contact_changes.is_empty(), "a favourite is this computer's");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh", "company": "Example Ltd"}));
    run(&mut s, "people.categorize", json!({"id": bo, "category": "Red category"}));
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Change, vec![ContactField::Last, ContactField::Company, ContactField::Categories])]);
    let op = upload(&mut s, account).ops.remove(0);
    assert_eq!((op.remote.as_str(), op.values.change_key.as_str(), op.values.last.as_str()), ("c-bo", "ck-c-bo-1", "Marsh"));
    // Changed again while on its way: the rest goes next.
    run(&mut s, "people.edit", json!({"id": bo, "notes": "Met in May"}));
    answer(&mut s, account, &op, ContactResult::Done, "");
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Change, vec![ContactField::Notes])]);
    let c = s.mailbox.contact(bo).unwrap();
    assert_eq!((c.origin.change_key.as_str(), c.favorite), ("ck-new", true));
}

#[test]
fn a_delete_goes_up_and_the_deleted_contact_isnt_brought_back_meanwhile() {
    let (mut s, account) = session();
    let cy = called(&s, "Cy");
    run(&mut s, "people.delete", json!({"id": cy}));
    assert_eq!(notes(&s), [(cy, ContactChangeKind::Delete, vec![])]);
    // A download before the server heard: Cy stays deleted.
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy")])}));
    assert!(s.mailbox.contacts.values().all(|c| c.first != "Cy"));
    let op = upload(&mut s, account).ops.remove(0);
    assert_eq!((op.kind, op.remote.as_str(), op.fields.is_empty()), (ContactChangeKind::Delete, "c-cy", true));
    answer(&mut s, account, &op, ContactResult::Done, "");
    assert!(s.mailbox.contact_changes.is_empty());
}

#[test]
fn every_op_meets_every_answer() {
    use ContactChangeKind::{Change, Create, Delete};
    use ContactResult::{Denied, Done, Failed, Gone, Refused};
    for kind in [Create, Change, Delete] {
        for result in [Done, Gone, Refused, Denied, Failed] {
            let (mut s, account) = session();
            let bo = called(&s, "Bo");
            let id = match kind {
                Create => run(&mut s, "people.new", json!({"first": "Dee"}))["id"].as_u64().unwrap(),
                Change => {
                    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
                    bo
                }
                Delete => {
                    run(&mut s, "people.delete", json!({"id": bo}));
                    bo
                }
            };
            let op = upload(&mut s, account).ops.remove(0);
            let said = answer(&mut s, account, &op, result, if kind == Create { "c-dee" } else { "" });
            let what = format!("{kind:?} {result:?}");
            let c = s.mailbox.contact(id).cloned();
            let waiting = notes(&s).into_iter().map(|(_, kind, _)| kind).collect::<Vec<_>>();
            match (kind, result) {
                (_, Done) | (Delete, Gone) => assert!(waiting.is_empty(), "{what}"),
                (Create, Gone | Refused) => {
                    // Kept on this computer, and said.
                    assert!(waiting.is_empty() && c.as_ref().is_some_and(|c| c.folder == 0 && c.origin.is_local()), "{what}");
                    assert_eq!(said["messages"].as_array().unwrap().len(), 1, "{what}");
                }
                (Change, Gone) => {
                    // Deleted there, changed here: made again.
                    assert_eq!(waiting, [Create], "{what}");
                    assert!(c.as_ref().is_some_and(|c| c.origin.id.is_empty() && c.last == "Marsh"), "{what}");
                }
                (Change, Refused) => {
                    assert!(waiting.is_empty() && c.as_ref().is_some_and(|c| c.last.is_empty() && c.origin.id == "c-bo"), "{what}");
                    assert_eq!(said["refused"], 1, "{what}");
                }
                (Delete, Refused) => {
                    assert!(waiting.is_empty() && s.mailbox.contacts.values().any(|c| c.first == "Bo" && c.origin.id == "c-bo"), "{what}");
                }
                (_, Failed | Denied) => {
                    let note = s.mailbox.contact_changes.first().unwrap();
                    assert_eq!((note.attempts, note.next_try, note.denied), (1, NOW + 30, result == Denied), "{what}");
                    assert_eq!((said["failed"].as_u64(), said["denied"].as_bool()), (Some(1), Some(result == Denied)), "{what}");
                    // Not due yet; and a second refusal of the permission gives up.
                    assert!(upload(&mut s, account).ops.is_empty(), "{what}");
                    if result == Denied {
                        answer(&mut s, account, &op, Denied, "");
                        assert!(s.mailbox.contact_changes.is_empty(), "{what}");
                    }
                }
            }
            assert!(s.mailbox.clone().repair().is_empty(), "{what}");
        }
    }
}

#[test]
fn failures_back_off_and_a_new_sign_in_tries_again_at_once() {
    let (mut s, account) = session();
    let who = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": who, "last": "Marsh"}));
    let op = upload(&mut s, account).ops.remove(0);
    for (n, wait) in [(1, 30), (2, 60), (3, 120)] {
        answer(&mut s, account, &op, ContactResult::Failed, "");
        let note = &s.mailbox.contact_changes[0];
        assert_eq!((note.attempts, note.next_try), (n, NOW + wait));
    }
    super::retry_contact_changes(&mut s, account);
    assert_eq!(upload(&mut s, account).ops.len(), 1);
}

#[test]
fn a_download_while_a_change_waits_merges_field_by_field() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
    // Meanwhile on the server: Bo's company, and his surname too.
    let there = RemoteContact { company: "Example Ltd".into(), last: "Moor".into(), change_key: "ck-c-bo-2".into(), ..person("c-bo", "Bo") };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![there, person("c-cy", "Cy")])}));
    let c = s.mailbox.contact(bo).unwrap();
    assert_eq!((c.last.as_str(), c.company.as_str(), c.origin.change_key.as_str()), ("Marsh", "Example Ltd", "ck-c-bo-2"));
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Change, vec![ContactField::Last])]);
    assert_eq!(s.mailbox.contact_changes[0].base.as_ref().unwrap().company, "Example Ltd");
    // The server came round to the owner's surname: nothing left to say.
    let same = RemoteContact { last: "Marsh".into(), change_key: "ck-c-bo-3".into(), ..person("c-bo", "Bo") };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![same, person("c-cy", "Cy")])}));
    assert!(s.mailbox.contact_changes.is_empty());
    // Deleted there, changed here: made again.
    run(&mut s, "people.edit", json!({"id": bo, "notes": "Keep me"}));
    run(
        &mut s,
        "sync.applyContacts",
        json!({"download": ContactsDownload { gone: vec!["c-bo".into()], complete: false, ..download(account, vec![]) }}),
    );
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Create, vec![])]);
    assert!(s.mailbox.contact(bo).is_some_and(|c| c.origin.id.is_empty() && c.notes == "Keep me"));
    // A contact made here whose answer was lost comes down: it is taken for the same, not twice.
    let op = upload(&mut s, account).ops.remove(0);
    let back = RemoteContact { notes: "Keep me".into(), last: "Marsh".into(), ..person("c-bo-2", "Bo") };
    let count = s.mailbox.contacts.len();
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![back, person("c-cy", "Cy")])}));
    assert_eq!((s.mailbox.contacts.len(), s.mailbox.contact(bo).unwrap().origin.id.as_str()), (count, "c-bo-2"));
    assert!(s.mailbox.contact_changes.is_empty());
    // Its late answer then changes nothing more.
    answer(&mut s, account, &op, ContactResult::Done, "c-bo-2");
    assert!(s.mailbox.contact_changes.is_empty());
}

#[test]
fn a_shared_mailboxs_read_only_folder_refuses_changes_with_a_reason() {
    let (mut s, account) = session();
    let shared = RemoteContactFolder {
        id: "f-sales".into(),
        name: "Sales".into(),
        owner: FolderOwner::Mailbox("sales@example.org".into()),
        writable: false,
        ..Default::default()
    };
    let theirs = ContactsDownload { folder: Some(shared), delta: "d-1".into(), ..download(account, vec![person("c-di", "Di")]) };
    run(&mut s, "sync.applyContacts", json!({"download": theirs}));
    let di = called(&s, "Di");
    let folder = s.mailbox.contact_folder(s.mailbox.contact(di).unwrap().folder).unwrap().clone();
    assert_eq!(
        (folder.name.as_str(), folder.writable, folder.delta.as_str(), folder.owner.address()),
        ("Sales", false, "d-1", Some("sales@example.org"))
    );
    let snapshot = s.mailbox.clone();
    for (command, params) in [
        ("people.edit", json!({"id": di, "last": "X"})),
        ("people.delete", json!({"id": di})),
        ("people.categorize", json!({"id": di, "category": "Red category"})),
        ("people.new", json!({"first": "Eli", "folder": folder.id})),
    ] {
        let why = s.run(command, &params).unwrap_err().to_string();
        assert!(why.contains("\"Sales\" can only be read"), "{command}: {why}");
    }
    assert!(s.mailbox == snapshot);
    assert_eq!(run(&mut s, "people.get", json!({"id": di}))["readOnly"], true);
    // The main folder's contacts are not touched by that folder's download.
    assert!(s.mailbox.contacts.values().any(|c| c.first == "Bo"));
}

#[test]
fn a_writable_shared_folder_is_addressed_through_its_mailbox() {
    let (mut s, account) = session();
    let shared = RemoteContactFolder {
        id: "f-team".into(),
        name: "Team".into(),
        owner: FolderOwner::Person("bo@example.org".into()),
        writable: true,
        ..Default::default()
    };
    run(
        &mut s,
        "sync.applyContacts",
        json!({"download": ContactsDownload { folder: Some(shared), ..download(account, vec![person("c-di", "Di")]) }}),
    );
    let who = called(&s, "Di");
    run(&mut s, "people.edit", json!({"id": who, "last": "Ray"}));
    let op = upload(&mut s, account).ops.remove(0);
    assert_eq!((op.folder.as_str(), op.owner.address()), ("f-team", Some("bo@example.org")));
}

#[test]
fn undo_before_the_change_went_up_takes_the_note_back() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact_changes.is_empty() && s.mailbox.contact(bo).unwrap().last.is_empty());
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Change, vec![ContactField::Last])]);
    let made = run(&mut s, "people.new", json!({"first": "Dee"}))["id"].as_u64().unwrap();
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact(made).is_none() && notes(&s).len() == 1);
    assert!(upload(&mut s, account).ops.iter().all(|op| op.contact == bo));
}

#[test]
fn undo_after_a_change_went_up_sends_the_opposite() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    // An edit, gone up: undone, the server is told the old surname.
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Change, vec![ContactField::Last])]);
    let back = upload(&mut s, account).ops.remove(0);
    assert_eq!((back.values.last.as_str(), back.values.change_key.as_str()), ("", "ck-new"));
    answer(&mut s, account, &back, ContactResult::Done, "");

    // A new contact, gone up: undone, it is deleted there.
    let made = run(&mut s, "people.new", json!({"first": "Dee"}))["id"].as_u64().unwrap();
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "c-dee");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact(made).is_none());
    let gone = upload(&mut s, account).ops.remove(0);
    assert_eq!((gone.kind, gone.remote.as_str()), (ContactChangeKind::Delete, "c-dee"));
    answer(&mut s, account, &gone, ContactResult::Done, "");

    // A delete, gone up: undone, the contact is made again there.
    let cy = called(&s, "Cy");
    run(&mut s, "people.delete", json!({"id": cy}));
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact(cy).is_some_and(|c| c.origin.id.is_empty()));
    assert_eq!(notes(&s), [(cy, ContactChangeKind::Create, vec![])]);
    // And an answer to a delete that was undone while on its way: made again too.
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.delete", json!({"id": bo}));
    let op = upload(&mut s, account).ops.remove(0);
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact_changes.is_empty());
    answer(&mut s, account, &op, ContactResult::Done, "");
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Create, vec![])]);
}

#[test]
fn a_full_queue_refuses_the_change_and_changes_nothing() {
    let (mut s, account) = session();
    let filler = mailcraft_model::contact_sync::ContactChange { account, contact: 9_999_999, ..Default::default() };
    s.mailbox.contact_changes = vec![filler; MAX_CONTACT_CHANGES];
    let snapshot = s.mailbox.clone();
    match s.run("people.edit", &json!({"id": called(&s, "Bo"), "last": "Marsh"})) {
        Err(CmdError::Failed(why)) => assert!(why.contains("Too many contact changes"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(s.run("people.new", &json!({"first": "Dee"})).is_err());
    assert!(s.mailbox == snapshot);
    // A contact on this computer is still made.
    assert!(s.run("people.new", &json!({"first": "Dee", "folder": 0})).is_ok());
}

#[test]
fn the_queue_outlives_a_restart() {
    let (mut s, account) = session();
    let who = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": who, "last": "Marsh"}));
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Failed, "");
    let text = serde_json::to_string(&s.mailbox).unwrap();
    let mut back: mailcraft_model::Mailbox = serde_json::from_str(&text).unwrap();
    assert!(back.repair().is_empty());
    assert_eq!((&back.contact_changes, &back.contact_folders), (&s.mailbox.contact_changes, &s.mailbox.contact_folders));
}

#[test]
fn forgetting_an_account_keeps_what_was_made_here_on_this_computer() {
    let (mut s, account) = session();
    let made = run(&mut s, "people.new", json!({"first": "Dee"}))["id"].as_u64().unwrap();
    run(&mut s, "sync.forgetRemote", json!({"account": account}));
    let c = s.mailbox.contact(made).unwrap();
    assert!(c.origin.is_local() && c.folder == 0);
    assert!(s.mailbox.contact_folders.is_empty() && s.mailbox.contact_changes.is_empty());
    assert!(s.mailbox.contacts.values().all(|c| c.first != "Bo"));
}

#[test]
fn junk_answers_and_downloads_are_refused_or_change_nothing_wrong() {
    let (mut s, account) = session();
    for junk in [json!({}), json!({"result": 3}), json!({"result": {"account": 424_242, "outcomes": []}})] {
        assert!(s.run("sync.applyContactChanges", &junk).is_err(), "{junk}");
    }
    // An answer about nothing that waits changes nothing.
    let snapshot = s.mailbox.contact_changes.clone();
    let stray = ContactOp { kind: ContactChangeKind::Change, contact: 31_337, ..Default::default() };
    for result in [ContactResult::Failed, ContactResult::Refused, ContactResult::Denied, ContactResult::Gone] {
        answer(&mut s, account, &stray, result, "");
    }
    assert_eq!(s.mailbox.contact_changes, snapshot);
    let bad = RemoteContactFolder { owner: FolderOwner::Mailbox("not an address".into()), ..Default::default() };
    assert!(s.run("sync.applyContacts", &json!({"download": ContactsDownload { folder: Some(bad), ..download(account, vec![]) }})).is_err());
}
