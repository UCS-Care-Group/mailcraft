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
    // A contact made here whose answer was lost (the connection dropped once it was sent) comes
    // down: it is taken for the same, not twice.
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Failed, "");
    let back = RemoteContact { notes: "Keep me".into(), last: "Marsh".into(), ..person("c-bo-2", "Bo") };
    let count = s.mailbox.contacts.len();
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![back, person("c-cy", "Cy")])}));
    assert_eq!((s.mailbox.contacts.len(), s.mailbox.contact(bo).unwrap().origin.id.as_str()), (count, "c-bo-2"));
    assert!(s.mailbox.contact_changes.is_empty());
    // Its late answer then changes nothing more: not the contact's fields, nor its key.
    let adopted = s.mailbox.contact(bo).cloned();
    answer(&mut s, account, &op, ContactResult::Done, "c-bo-2");
    assert!(s.mailbox.contact_changes.is_empty());
    assert_eq!(s.mailbox.contact(bo).cloned(), adopted);
    assert_eq!(s.mailbox.contact(bo).unwrap().origin.change_key, "ck-c-bo-2-1");
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

#[test]
fn a_shared_folder_that_refuses_a_change_can_only_be_read_until_a_new_sign_in() {
    let (mut s, account) = session();
    let shared = RemoteContactFolder {
        id: "f-leads".into(),
        name: "Leads".into(),
        owner: FolderOwner::Mailbox("sales@example.org".into()),
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
    answer(&mut s, account, &op, ContactResult::Denied, "");
    // Given up at once (not asked twice), and the folder is now one that can only be read.
    assert!(s.mailbox.contact_changes.is_empty());
    assert!(s.mailbox.contact_folders.iter().any(|f| f.remote == "f-leads" && !f.writable));
    crate::cmd::contacts_remote::retry_contact_changes(&mut s, account);
    assert!(s.mailbox.contact_folders.iter().any(|f| f.remote == "f-leads" && f.writable));
}

fn team(writable: bool, contacts: Vec<RemoteContact>, account: Id) -> ContactsDownload {
    let folder = RemoteContactFolder {
        id: "f-team".into(),
        name: "Team".into(),
        owner: FolderOwner::Mailbox("team@example.org".into()),
        writable,
        ..Default::default()
    };
    ContactsDownload { folder: Some(folder), ..download(account, contacts) }
}

/// Review of #1934: a change waiting when its folder becomes one that may only be read is kept,
/// and held, not lost; it goes when the folder may be written again.
#[test]
fn a_change_waiting_when_its_folder_becomes_read_only_is_held_not_lost() {
    let (mut s, account) = session();
    run(&mut s, "sync.applyContacts", json!({"download": team(true, vec![person("c-di", "Di")], account)}));
    let di = called(&s, "Di");
    run(&mut s, "people.edit", json!({"id": di, "last": "Ray"}));
    for _ in 0..2 {
        run(&mut s, "sync.applyContacts", json!({"download": team(false, vec![person("c-di", "Di")], account)}));
        assert_eq!(s.mailbox.contact(di).unwrap().last, "Ray");
        assert_eq!(notes(&s), [(di, ContactChangeKind::Change, vec![ContactField::Last])]);
        assert!(upload(&mut s, account).ops.is_empty(), "held while it may only be read");
    }
    run(&mut s, "sync.applyContacts", json!({"download": team(true, vec![person("c-di", "Di")], account)}));
    assert_eq!(upload(&mut s, account).ops.len(), 1);
}

/// Review of #1934 and #1937: an undo whose change has no room in the queue isn't undone for
/// that contact (an edit, or a new contact whose deletion couldn't be noted), and says so.
#[test]
fn an_undo_with_no_room_to_tell_the_server_leaves_the_contact_as_it_is() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "");
    let dee = run(&mut s, "people.new", json!({"first": "Dee"}))["id"].as_u64().unwrap();
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "c-dee");
    let filler = mailcraft_model::contact_sync::ContactChange { account, contact: 9_999_999, ..Default::default() };
    s.mailbox.contact_changes = vec![filler; MAX_CONTACT_CHANGES];
    let queue = s.mailbox.contact_changes.clone();
    // Undoing the new contact would need a deletion sent: it stays, on the server and here.
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact(dee).is_some_and(|c| c.origin.id == "c-dee"));
    assert!(s.undo_said().iter().any(|l| l.contains("isn't undone")), "{:?}", s.undo_said());
    // Undoing the edit would need a change sent: it stays.
    run(&mut s, "edit.undo", json!({}));
    let c = s.mailbox.contact(bo).unwrap();
    assert!(c.origin.id == "c-bo" && c.last == "Marsh", "{c:?}");
    assert_eq!(s.mailbox.contact_changes, queue);
}

/// Review of #1934, second pass: undone after its change went up and its folder became one that
/// may only be read, a step can't be told to the server: that contact isn't undone here either
/// (an edit, a deletion, a new contact), and the owner is told.
#[test]
fn an_undo_in_a_folder_that_became_read_only_leaves_the_contact_as_it_is() {
    let (mut s, account) = session();
    run(&mut s, "sync.applyContacts", json!({"download": team(true, vec![person("c-di", "Di"), person("c-eve", "Eve")], account)}));
    let (di, eve) = (called(&s, "Di"), called(&s, "Eve"));
    let folder = s.mailbox.contact(di).unwrap().folder;
    run(&mut s, "people.edit", json!({"id": di, "last": "Ray"}));
    run(&mut s, "people.delete", json!({"id": eve}));
    let fay = run(&mut s, "people.new", json!({"first": "Fay", "email": "fay@example.org", "folder": folder}))["id"].as_u64().unwrap();
    for op in upload(&mut s, account).ops {
        let remote = if op.kind == ContactChangeKind::Create { "c-fay" } else { "" };
        answer(&mut s, account, &op, ContactResult::Done, remote);
    }
    assert!(s.mailbox.contact_changes.is_empty());
    let di_there = RemoteContact { last: "Ray".into(), ..person("c-di", "Di") };
    run(&mut s, "sync.applyContacts", json!({"download": team(false, vec![di_there, person("c-fay", "Fay")], account)}));
    let before = s.mailbox.contacts.clone();
    for _ in 0..3 {
        run(&mut s, "edit.undo", json!({}));
        assert!(s.undo_said().iter().any(|l| l.contains("can only be read")), "{:?}", s.undo_said());
    }
    assert!(s.mailbox.contact_changes.is_empty());
    assert_eq!(s.mailbox.contacts, before);
    assert!(s.mailbox.contact(fay).is_some_and(|c| c.origin.id == "c-fay") && s.mailbox.contact(eve).is_none());
    assert_eq!(s.mailbox.contact(di).unwrap().last, "Ray");
}

/// Review of #1937: a new contact the server took nothing of (too many requests) wasn't sent:
/// it is never taken for one the server lists. One whose answer was lost on the way was.
#[test]
fn only_a_new_contact_that_was_tried_is_taken_for_one_the_server_lists() {
    for (status, taken) in [(429, false), (0, true), (503, true)] {
        let (mut s, account) = session();
        let dee = run(&mut s, "people.new", json!({"first": "Dee", "categories": ["Red category"]}))["id"].as_u64().unwrap();
        let op = upload(&mut s, account).ops.remove(0);
        assert!(!s.mailbox.contact_changes[0].sent, "handing it out doesn't mark it");
        let outcome = ContactOutcome { op: op.clone(), result: ContactResult::Failed, status, ..Default::default() };
        let uploaded = ContactsUploaded { account, email: EMAIL.into(), outcomes: vec![outcome], ..Default::default() };
        run(&mut s, "sync.applyContactChanges", json!({"result": uploaded}));
        assert_eq!(s.mailbox.contact_changes[0].sent, taken, "{status}");
        // The server lists it, without saying its categories.
        let theirs = RemoteContact { id: "c-dee".into(), change_key: "ck-dee".into(), categories: None, ..op.values.clone() };
        run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy"), theirs])}));
        let dees = s.mailbox.contacts.values().filter(|c| c.first == "Dee").count();
        assert_eq!(dees, if taken { 1 } else { 2 }, "{status}");
        if taken {
            // Its categories are those sent: nothing more to send.
            let c = s.mailbox.contact(dee).unwrap();
            assert_eq!((c.origin.id.as_str(), c.categories.clone()), ("c-dee", vec!["Red category".to_string()]));
            assert!(s.mailbox.contact_changes.is_empty(), "{status}: {:?}", notes(&s));
        }
    }
}

/// Review of #1934 (third round): a sent new contact whose contact is gone doesn't stop the
/// search for the next one that matches.
#[test]
fn a_twin_whose_contact_is_gone_is_passed_over() {
    let (mut s, account) = session();
    let first = run(&mut s, "people.new", json!({"first": "Dee"}))["id"].as_u64().unwrap();
    let second = run(&mut s, "people.new", json!({"first": "Dee"}))["id"].as_u64().unwrap();
    let ops = upload(&mut s, account).ops;
    for op in &ops {
        let outcome = ContactOutcome { op: op.clone(), result: ContactResult::Failed, ..Default::default() };
        let uploaded = ContactsUploaded { account, email: EMAIL.into(), outcomes: vec![outcome], ..Default::default() };
        run(&mut s, "sync.applyContactChanges", json!({"result": uploaded}));
    }
    s.mailbox.contacts.remove(&first);
    let theirs = RemoteContact { id: "c-dee".into(), change_key: "ck-dee".into(), ..ops[1].values.clone() };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy"), theirs])}));
    assert_eq!(s.mailbox.contact(second).unwrap().origin.id, "c-dee");
    assert_eq!(s.mailbox.contacts.values().filter(|c| c.first == "Dee").count(), 1);
}

/// Review of #1934: a download that only renames its folder (or makes it) is a change.
#[test]
fn a_download_that_only_changes_its_folder_is_a_change() {
    let (mut s, account) = session();
    let made = super::merge_contacts(&mut s.mailbox, &team(true, vec![], account)).unwrap();
    assert!(made.changed());
    let mut renamed = team(true, vec![], account);
    if let Some(f) = renamed.folder.as_mut() {
        f.name = "Team A".into();
    }
    assert!(super::merge_contacts(&mut s.mailbox, &renamed).unwrap().changed());
    assert!(!super::merge_contacts(&mut s.mailbox, &renamed).unwrap().changed());
    // Made read-only (and its old name again): a change too.
    assert!(super::merge_contacts(&mut s.mailbox, &team(false, vec![], account)).unwrap().changed());
}

/// Review of #1934: a download that leaves a failed change as it was doesn't cut its wait short.
#[test]
fn a_download_that_changes_nothing_keeps_a_failed_changes_wait() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Failed, "");
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy")])}));
    assert_eq!(s.mailbox.contact_changes[0].next_try, NOW + 30);
    assert!(upload(&mut s, account).ops.is_empty());
    // A new copy of the server's makes it due.
    let newer = RemoteContact { company: "Example Ltd".into(), change_key: "ck-c-bo-2".into(), ..person("c-bo", "Bo") };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![newer, person("c-cy", "Cy")])}));
    assert_eq!(upload(&mut s, account).ops.len(), 1);
}

/// Review of #1934: a new contact that hasn't been sent is never taken for one the server lists.
#[test]
fn a_new_contact_not_sent_yet_is_not_taken_for_one_the_server_lists() {
    let (mut s, account) = session();
    let dee = run(&mut s, "people.new", json!({"first": "Dee", "email": "dee@example.org"}))["id"].as_u64().unwrap();
    let theirs = RemoteContact { emails: vec![Labeled::new("", "dee@example.org")], ..person("c-other", "Dee") };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy"), theirs])}));
    assert_eq!(notes(&s), [(dee, ContactChangeKind::Create, vec![])]);
    assert!(s.mailbox.contact(dee).unwrap().origin.id.is_empty());
    assert_eq!(s.mailbox.contacts.values().filter(|c| c.first == "Dee").count(), 2);
}

/// Review of #1934: more contacts than are taken in: where the list was read up to isn't kept.
#[test]
fn a_download_cut_short_keeps_where_its_list_was_read_up_to() {
    let (mut s, account) = session();
    run(&mut s, "sync.applyContacts", json!({"download": ContactsDownload { delta: "d-1".into(), ..team(true, vec![], account) }}));
    let many: Vec<RemoteContact> = (0..=mailcraft_model::remote::MAX_CONTACTS).map(|n| person(&format!("c-{n}"), "P")).collect();
    let cut = ContactsDownload { delta: "d-2".into(), complete: false, ..team(true, many, account) };
    super::merge_contacts(&mut s.mailbox, &cut).unwrap();
    assert!(s.mailbox.contact_folders.iter().any(|f| f.remote == "f-team" && f.delta == "d-1"));
}

/// Review of #1934: made, the server says, but not under what name: not sent again until a
/// download shows it (then it is that one) or a whole list doesn't (then it goes again).
#[test]
fn a_new_contact_made_without_a_name_is_not_made_twice() {
    let (mut s, account) = session();
    let dee = run(&mut s, "people.new", json!({"first": "Dee", "email": "dee@example.org"}))["id"].as_u64().unwrap();
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "");
    assert!(s.mailbox.contact_changes[0].unconfirmed == NOW && upload(&mut s, account).ops.is_empty());
    // A whole list read after the answer, without it: it wasn't made after all.
    let after = ContactsDownload { read_at: NOW + 1, ..download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy")]) };
    run(&mut s, "sync.applyContacts", json!({"download": after}));
    let op = upload(&mut s, account).ops.remove(0);
    assert_eq!(op.contact, dee);
    answer(&mut s, account, &op, ContactResult::Done, "");
    // A list that shows it: that is the one, even renamed here meanwhile.
    run(&mut s, "people.edit", json!({"id": dee, "last": "Park"}));
    let theirs = RemoteContact { id: "c-dee".into(), change_key: "ck-dee".into(), ..op.values.clone() };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy"), theirs])}));
    assert_eq!(s.mailbox.contacts.values().filter(|c| c.first == "Dee").count(), 1);
    assert_eq!(s.mailbox.contact(dee).unwrap().origin.id, "c-dee");
    assert_eq!(notes(&s), [(dee, ContactChangeKind::Change, vec![ContactField::Last])]);
}

/// Review of #1934: a new contact undone while it was on its way, then made by the server: the
/// server is told to delete it.
#[test]
fn a_new_contact_undone_on_its_way_is_deleted_there_once_made() {
    let (mut s, account) = session();
    let dee = run(&mut s, "people.new", json!({"first": "Dee"}))["id"].as_u64().unwrap();
    let op = upload(&mut s, account).ops.remove(0);
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact(dee).is_none() && s.mailbox.contact_changes.is_empty());
    answer(&mut s, account, &op, ContactResult::Done, "c-dee");
    let gone = upload(&mut s, account).ops.remove(0);
    assert_eq!((gone.kind, gone.remote.as_str()), (ContactChangeKind::Delete, "c-dee"));
}

/// Review of #1934: an answer older than a download doesn't put back an older key.
#[test]
fn a_late_answer_keeps_the_newer_key_of_a_download() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
    let op = upload(&mut s, account).ops.remove(0);
    let newer = RemoteContact { last: "Marsh".into(), change_key: "ck-c-bo-3".into(), ..person("c-bo", "Bo") };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![newer, person("c-cy", "Cy")])}));
    // And changed there again since (CodeRabbit on #1934): the newer copy stands, fields and key.
    let lake = RemoteContact { last: "Lake".into(), change_key: "ck-c-bo-4".into(), ..person("c-bo", "Bo") };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![lake, person("c-cy", "Cy")])}));
    answer(&mut s, account, &op, ContactResult::Done, "");
    let c = s.mailbox.contact(bo).unwrap();
    assert_eq!((c.last.as_str(), c.origin.change_key.as_str()), ("Lake", "ck-c-bo-4"));
    assert!(s.mailbox.contact_changes.is_empty());
}

/// Review of #1935/#1936: undone, a step puts back only the fields it changed; what the server
/// changed since in the others stays, and isn't sent back.
#[test]
fn undo_puts_back_only_the_fields_the_step_changed() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "");
    let there = RemoteContact { last: "Marsh".into(), company: "Example Ltd".into(), change_key: "ck-c-bo-5".into(), ..person("c-bo", "Bo") };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![there, person("c-cy", "Cy")])}));
    run(&mut s, "edit.undo", json!({}));
    let c = s.mailbox.contact(bo).unwrap();
    assert_eq!((c.last.as_str(), c.company.as_str(), c.origin.change_key.as_str()), ("", "Example Ltd", "ck-c-bo-5"));
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Change, vec![ContactField::Last])]);
    assert_eq!(upload(&mut s, account).ops[0].values.change_key, "ck-c-bo-5");
}

/// Review of #1935/#1936: a folder's download says nothing of another folder's contacts, even
/// under the same ids.
#[test]
fn a_folders_gone_list_touches_only_its_own_contacts() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(
        &mut s,
        "sync.applyContacts",
        json!({"download": ContactsDownload { gone: vec!["c-bo".into()], complete: false, ..team(true, vec![], account) }}),
    );
    run(&mut s, "sync.applyContacts", json!({"download": team(true, vec![person("c-cy", "Cy again")], account)}));
    assert!(s.mailbox.contact(bo).is_some());
    run(&mut s, "sync.applyContacts", json!({"download": team(true, vec![], account)}));
    assert!(s.mailbox.contacts.values().any(|c| c.first == "Cy") && s.mailbox.contact(bo).is_some());
}

/// Review of #1935/#1936: a folder whose id can't be read is refused, never taken for the main one.
#[test]
fn a_folder_id_that_cant_be_read_is_not_the_main_folder() {
    let (mut s, account) = session();
    let main = s.mailbox.main_contact_folder(account).unwrap();
    let (name, writable) = s.mailbox.contact_folder(main).map(|f| (f.name.clone(), f.writable)).unwrap();
    for id in ["a b", "\u{7}", " "] {
        let mut odd = team(false, vec![person("c-x", "X")], account);
        if let Some(f) = odd.folder.as_mut() {
            (f.id, f.owner) = (id.into(), FolderOwner::Own);
        }
        assert!(s.run("sync.applyContacts", &json!({"download": odd})).is_err(), "{id:?}");
    }
    assert_eq!(s.mailbox.contact_folder(main).map(|f| (f.name.clone(), f.writable)), Some((name, writable)));
    assert!(s.mailbox.contacts.values().all(|c| c.first != "X"));
}

/// Review of #1935/#1936: an answer naming another of the server's contacts than the one this
/// contact is linked to changes nothing.
#[test]
fn an_answer_about_another_server_contact_changes_nothing() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
    let op = upload(&mut s, account).ops.remove(0);
    let snapshot = s.mailbox.clone();
    let elsewhere = ContactOp { remote: "c-someone-else".into(), ..op.clone() };
    for result in [ContactResult::Done, ContactResult::Refused, ContactResult::Gone, ContactResult::Failed] {
        answer(&mut s, account, &elsewhere, result, "");
        answer(&mut s, account, &op, result, "c-other");
    }
    assert!(s.mailbox == snapshot);
}

/// Review of #1934 (second round): only a whole list read after the answer re-arms a new contact
/// the server made without saying its name.
#[test]
fn a_list_read_before_the_answer_doesnt_send_a_new_contact_again() {
    let (mut s, account) = session();
    run(&mut s, "people.new", json!({"first": "Dee"}));
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "");
    let list = |read_at| ContactsDownload { read_at, ..download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy")]) };
    for read_at in [0, NOW - 5, NOW] {
        run(&mut s, "sync.applyContacts", json!({"download": list(read_at)}));
        assert!(upload(&mut s, account).ops.is_empty(), "{read_at}");
    }
    run(&mut s, "sync.applyContacts", json!({"download": list(NOW + 1)}));
    assert_eq!(upload(&mut s, account).ops.len(), 1);
}

/// Review of #1934: a refusal puts back only what was sent, and keeps what was changed since.
#[test]
fn a_refusal_puts_back_only_what_was_sent() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh", "company": "Old Ltd"}));
    let op = upload(&mut s, account).ops.remove(0);
    run(&mut s, "people.edit", json!({"id": bo, "notes": "Met in May", "company": "New Ltd"}));
    answer(&mut s, account, &op, ContactResult::Refused, "");
    let c = s.mailbox.contact(bo).unwrap();
    assert_eq!((c.last.as_str(), c.company.as_str(), c.notes.as_str()), ("", "New Ltd", "Met in May"));
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Change, vec![ContactField::Company, ContactField::Notes])]);
}

/// #1917: Microsoft Graph keeps three e-mail addresses and one mobile number. A contact edited here
/// with four and three keeps them all through a change that went up and a download of what the
/// server kept, and the next round has nothing more to send.
#[test]
fn what_graph_has_no_place_for_survives_a_round_trip() {
    let (mut s, account) = session();
    assert!(s.mailbox.account(account).unwrap().keeps_contacts());
    let bo = called(&s, "Bo");
    let emails: Vec<_> = (1..=4).map(|n| json!({"label": "work", "value": format!("bo{n}@example.org")})).collect();
    let phones: Vec<_> = (1..=3).map(|n| json!({"label": "mobile", "value": format!("+44 7700 90000{n}")})).collect();
    run(&mut s, "people.edit", json!({"id": bo, "emails": emails, "phones": phones}));
    let here = s.mailbox.contact(bo).unwrap().clone();
    let op = upload(&mut s, account).ops.remove(0);
    // What goes up is what fits, in order.
    let places = mailcraft_model::contact_sync::graph_places(&op.values.emails, &op.values.phones, &op.values.addresses, &op.values.websites);
    assert_eq!((places.emails, places.mobile, places.business_phones.len()), (vec![0, 1, 2], Some(0), 0));
    answer(&mut s, account, &op, ContactResult::Done, "");
    // The server's copy: the first three addresses and the first mobile number.
    let kept = RemoteContact {
        change_key: "ck-c-bo-2".into(),
        emails: here.emails.iter().take(3).map(|e| Labeled::new("", e.value.clone())).collect(),
        phones: vec![Labeled::new("mobile", here.phones[0].value.clone())],
        ..person("c-bo", "Bo")
    };
    let kept = || kept.clone();
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![kept(), person("c-cy", "Cy")])}));
    let c = s.mailbox.contact(bo).unwrap();
    let values = |list: &[Labeled]| list.iter().map(|e| e.value.clone()).collect::<Vec<_>>();
    assert_eq!(values(&c.emails), values(&here.emails));
    assert_eq!(values(&c.phones), values(&here.phones));
    assert!(upload(&mut s, account).ops.is_empty(), "nothing more to send: {:?}", s.mailbox.contact_changes);
    // Read again: the overflow isn't doubled.
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![kept(), person("c-cy", "Cy")])}));
    assert_eq!(values(&s.mailbox.contact(bo).unwrap().emails), values(&here.emails));
}

/// #1917: people.get names what the server has no place for, and nothing is refused for it: a
/// fifth address is taken as readily as a first. On this computer nothing is named.
#[test]
fn people_get_names_what_stays_on_this_computer() {
    let (mut s, _) = session();
    let bo = called(&s, "Bo");
    let emails: Vec<_> = (1..=5).map(|n| json!({"label": "work", "value": format!("bo{n}@example.org")})).collect();
    let phones = [json!({"label": "mobile", "value": "+44 7700 900001"}), json!({"label": "mobile", "value": "+44 7700 900002"})];
    run(&mut s, "people.edit", json!({"id": bo, "emails": emails, "phones": phones}));
    let got = run(&mut s, "people.get", json!({"id": bo}));
    assert_eq!(got["emails"].as_array().map(Vec::len), Some(5));
    assert_eq!(got["localOnly"], json!({"emails": [3, 4], "phones": [1], "addresses": [], "websites": []}));
    // What fits names nothing.
    let cy = called(&s, "Cy");
    assert!(run(&mut s, "people.get", json!({"id": cy})).get("localOnly").is_none());
    // On this computer, any number, nothing named.
    let here = run(&mut s, "people.new", json!({"first": "Di", "emails": emails, "folder": 0}))["id"].as_u64().unwrap();
    let got = run(&mut s, "people.get", json!({"id": here}));
    assert_eq!((got["emails"].as_array().map(Vec::len), got.get("localOnly")), (Some(5), None));
}

/// Review of #1937: when the server lets one of its three addresses go, the one kept only here
/// takes its place and goes up, so the next read doesn't take it for one the server deleted.
#[test]
fn a_place_graph_lets_go_is_taken_by_what_was_kept_here_and_it_goes_up() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    let address = |n: &str| Labeled::new("", format!("{n}@example.org"));
    let emails: Vec<_> = ["a", "b", "c", "d"].iter().map(|n| json!({"label": "", "value": format!("{n}@example.org")})).collect();
    run(&mut s, "people.edit", json!({"id": bo, "emails": emails}));
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "");
    let server = |list: &[&str], key: &str| RemoteContact {
        change_key: key.into(),
        emails: list.iter().map(|n| address(n)).collect(),
        ..person("c-bo", "Bo")
    };
    let read = |s: &mut Session, list: &[&str], key: &str| {
        run(s, "sync.applyContacts", json!({"download": download(account, vec![server(list, key), person("c-cy", "Cy")])}));
        s.mailbox.contact(bo).unwrap().emails.iter().map(|e| e.value.clone()).collect::<Vec<_>>()
    };
    assert_eq!(read(&mut s, &["a", "b", "c"], "k2"), ["a@example.org", "b@example.org", "c@example.org", "d@example.org"]);
    assert!(upload(&mut s, account).ops.is_empty());
    // b deleted in Outlook: d has a place now. It stays, b doesn't come back, and d goes up.
    assert_eq!(read(&mut s, &["a", "c"], "k3"), ["a@example.org", "c@example.org", "d@example.org"]);
    assert_eq!(read(&mut s, &["a", "c"], "k3"), ["a@example.org", "c@example.org", "d@example.org"], "read again before it went");
    let op = upload(&mut s, account).ops.remove(0);
    assert_eq!(op.fields.iter().copied().collect::<Vec<_>>(), [ContactField::Emails]);
    answer(&mut s, account, &op, ContactResult::Done, "");
    assert_eq!(read(&mut s, &["a", "c", "d"], "k4"), ["a@example.org", "c@example.org", "d@example.org"]);
    assert!(upload(&mut s, account).ops.is_empty());
}

/// Review of #1937: what the owner changed after a change went up to a shared folder that then
/// refused it stays noted (held while the folder can only be read), and isn't lost to a read.
#[test]
fn a_newer_edit_outlives_a_shared_folders_refusal() {
    let (mut s, account) = session();
    let shared = RemoteContactFolder {
        id: "f-leads".into(),
        name: "Leads".into(),
        owner: FolderOwner::Mailbox("sales@example.org".into()),
        writable: true,
        ..Default::default()
    };
    let leads = |s: &mut Session| {
        run(
            s,
            "sync.applyContacts",
            json!({"download": ContactsDownload { folder: Some(shared.clone()), ..download(account, vec![person("c-di", "Di")]) }}),
        )
    };
    leads(&mut s);
    let di = called(&s, "Di");
    run(&mut s, "people.edit", json!({"id": di, "last": "Ray"}));
    let op = upload(&mut s, account).ops.remove(0);
    run(&mut s, "people.edit", json!({"id": di, "notes": "Met in May"}));
    answer(&mut s, account, &op, ContactResult::Denied, "");
    let c = s.mailbox.contact(di).unwrap();
    assert_eq!((c.last.as_str(), c.notes.as_str()), ("", "Met in May"));
    assert_eq!(notes(&s), [(di, ContactChangeKind::Change, vec![ContactField::Notes])]);
    assert!(upload(&mut s, account).ops.is_empty(), "held while the folder can only be read");
    s.mailbox.contact_folders.iter_mut().filter(|f| f.remote == "f-leads").for_each(|f| f.writable = false);
    leads(&mut s);
    assert_eq!(s.mailbox.contact(di).unwrap().notes, "Met in May");
}

/// Review of #1937: two refusals in one answer from the same shared folder each keep what the
/// owner changed since.
#[test]
fn two_refusals_in_one_answer_keep_both_newer_edits() {
    let (mut s, account) = session();
    let shared = RemoteContactFolder {
        id: "f-leads".into(),
        name: "Leads".into(),
        owner: FolderOwner::Mailbox("sales@example.org".into()),
        writable: true,
        ..Default::default()
    };
    let both = vec![person("c-di", "Di"), person("c-ed", "Ed")];
    run(&mut s, "sync.applyContacts", json!({"download": ContactsDownload { folder: Some(shared), ..download(account, both) }}));
    let (di, ed) = (called(&s, "Di"), called(&s, "Ed"));
    for who in [di, ed] {
        run(&mut s, "people.edit", json!({"id": who, "last": "Ray"}));
    }
    let ops = upload(&mut s, account).ops;
    assert_eq!(ops.len(), 2);
    for who in [di, ed] {
        run(&mut s, "people.edit", json!({"id": who, "notes": "Met in May"}));
    }
    let outcomes = ops.iter().map(|op| ContactOutcome { op: op.clone(), result: ContactResult::Denied, ..Default::default() }).collect();
    run(&mut s, "sync.applyContactChanges", json!({"result": ContactsUploaded { account, email: EMAIL.into(), outcomes, ..Default::default() }}));
    for who in [di, ed] {
        let c = s.mailbox.contact(who).unwrap();
        assert_eq!((c.last.as_str(), c.notes.as_str()), ("", "Met in May"));
    }
    assert_eq!(notes(&s), [(di, ContactChangeKind::Change, vec![ContactField::Notes]), (ed, ContactChangeKind::Change, vec![ContactField::Notes])]);
    assert!(s.mailbox.contact_folders.iter().any(|f| f.remote == "f-leads" && !f.writable));
}

/// Review of #1937: a whole read of a folder the server has nothing in drops the position the
/// server forgot, and what it no longer has goes.
#[test]
fn a_whole_empty_read_drops_a_forgotten_position() {
    let (mut s, account) = session();
    let main = s.mailbox.main_contact_folder(account).unwrap();
    let stale = "https://graph.microsoft.com/v1.0/me/contactFolders/f/contacts/delta?$deltatoken=1";
    s.mailbox.contact_folders.iter_mut().filter(|f| f.id == main).for_each(|f| f.delta = stale.into());
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![])}));
    assert_eq!(s.mailbox.contact_folder(main).unwrap().delta, "");
    assert!(s.mailbox.contacts.values().all(|c| c.folder != main));
    // An incomplete one keeps it.
    s.mailbox.contact_folders.iter_mut().filter(|f| f.id == main).for_each(|f| f.delta = stale.into());
    run(&mut s, "sync.applyContacts", json!({"download": ContactsDownload { complete: false, ..download(account, vec![]) }}));
    assert_eq!(s.mailbox.contact_folder(main).unwrap().delta, stale);
}

/// Review of #1937 (fourth round): a new contact made there without its name heard, then undone
/// here, is deleted there once a download shows it, not taken in again; one a list read after
/// doesn't show is let go.
#[test]
fn a_new_contact_undone_before_its_name_was_heard_is_deleted_there() {
    let (mut s, account) = session();
    let dee = run(&mut s, "people.new", json!({"first": "Dee"}))["id"].as_u64().unwrap();
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact(dee).is_none() && upload(&mut s, account).ops.is_empty());
    let theirs = RemoteContact { id: "c-dee".into(), change_key: "ck-dee".into(), ..op.values.clone() };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy"), theirs])}));
    assert!(s.mailbox.contacts.values().all(|c| c.first != "Dee"));
    let gone = upload(&mut s, account).ops.remove(0);
    assert_eq!((gone.kind, gone.remote.as_str()), (ContactChangeKind::Delete, "c-dee"));
    answer(&mut s, account, &gone, ContactResult::Done, "");
    assert!(s.mailbox.contact_changes.is_empty());
    // Undone again, and not made there after all: a list read after the answer lets it go.
    let (mut s, account) = session();
    run(&mut s, "people.new", json!({"first": "Dee"}));
    let op = upload(&mut s, account).ops.remove(0);
    answer(&mut s, account, &op, ContactResult::Done, "");
    run(&mut s, "edit.undo", json!({}));
    let list = |read_at| ContactsDownload { read_at, ..download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy")]) };
    run(&mut s, "sync.applyContacts", json!({"download": list(NOW - 1)}));
    assert_eq!(s.mailbox.contact_changes.len(), 1, "a list read before says nothing");
    run(&mut s, "sync.applyContacts", json!({"download": list(NOW + 1)}));
    assert!(s.mailbox.contact_changes.is_empty() && upload(&mut s, account).ops.is_empty());
}

/// Review of #1937 (fourth round): a download that came before the answer, not yet showing the
/// change: what the server took, and is still so here, isn't sent again.
#[test]
fn an_answer_after_a_download_retires_what_the_server_took() {
    let (mut s, account) = session();
    let bo = called(&s, "Bo");
    run(&mut s, "people.edit", json!({"id": bo, "last": "Marsh"}));
    let op = upload(&mut s, account).ops.remove(0);
    let older = RemoteContact { company: "Example Ltd".into(), change_key: "ck-c-bo-2".into(), ..person("c-bo", "Bo") };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![older, person("c-cy", "Cy")])}));
    assert_eq!(notes(&s), [(bo, ContactChangeKind::Change, vec![ContactField::Last])]);
    answer(&mut s, account, &op, ContactResult::Done, "");
    assert!(s.mailbox.contact_changes.is_empty(), "{:?}", notes(&s));
    let c = s.mailbox.contact(bo).unwrap();
    assert_eq!((c.last.as_str(), c.company.as_str(), c.origin.change_key.as_str()), ("Marsh", "Example Ltd", "ck-c-bo-2"));
}

/// Review of #1934 (fifth round): a new contact whose try failed, then undone, waits for a list
/// read after that try; and when the server lists it without its categories, the deletion
/// that goes up remembers those sent.
#[test]
fn a_failed_new_contact_undone_waits_for_a_later_list_and_keeps_its_categories() {
    let failed = |s: &mut Session, account| {
        run(s, "people.new", json!({"first": "Dee", "categories": ["Red category"]}));
        let op = upload(s, account).ops.remove(0);
        let outcome = ContactOutcome { op: op.clone(), result: ContactResult::Failed, ..Default::default() };
        let uploaded = ContactsUploaded { account, email: EMAIL.into(), outcomes: vec![outcome], ..Default::default() };
        run(s, "sync.applyContactChanges", json!({"result": uploaded}));
        run(s, "edit.undo", json!({}));
        op
    };
    let (mut s, account) = session();
    failed(&mut s, account);
    let list = |read_at| ContactsDownload { read_at, ..download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy")]) };
    run(&mut s, "sync.applyContacts", json!({"download": list(NOW - 1)}));
    assert_eq!(s.mailbox.contact_changes.len(), 1, "a list read before the try says nothing");
    run(&mut s, "sync.applyContacts", json!({"download": list(NOW + 1)}));
    assert!(s.mailbox.contact_changes.is_empty() && upload(&mut s, account).ops.is_empty());

    let (mut s, account) = session();
    let op = failed(&mut s, account);
    let theirs = RemoteContact { id: "c-dee".into(), change_key: "ck-dee".into(), categories: None, ..op.values };
    run(&mut s, "sync.applyContacts", json!({"download": download(account, vec![person("c-bo", "Bo"), person("c-cy", "Cy"), theirs])}));
    let note = &s.mailbox.contact_changes[0];
    assert_eq!(note.kind, ContactChangeKind::Delete);
    assert_eq!(note.base.as_ref().map(|b| b.categories.clone()), Some(vec!["Red category".to_string()]));
}
