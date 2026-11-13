// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contacts of an account kept in step over JMAP, both ways: what is to be told worked out,
//! what the server made of it taken in, and downloads merged without losing what was changed
//! here. The server's side is written out by hand, as a host brings it.

use mailcraft_model::remote::{
    ContactField, ContactOp, ContactOpKind, ContactOutcome, ContactResult, ContactsDownload, ContactsUpload, ContactsUploaded, MAX_CONTACT_OPS,
    RemoteContact,
};
use mailcraft_model::{AccountChoice, Contact, ContactGone, Id, Labeled, MAX_CONTACT_GONE, Origin};
use serde_json::{Value, json};

use super::contacts_remote::{
    MAX_CONTACT_CREATES, apply_contact_changes, apply_contact_outcomes, carries_delete, contact_upload, contacts_waiting, mark_sent,
};
use super::remote::{apply_contacts, merge_contacts};
use crate::{CmdError, Session};

const NOW: i64 = 1_790_000_000;

/// The sample mailbox with an account kept in step over JMAP at `ada@example.org`.
fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let jmap = json!({"email": "ada@example.org", "kind": "jmap", "jmap": {"session": "https://jmap.example.org/jmap/session"}});
    let added = s.run("account.add", &jmap).unwrap();
    (s, added["id"].as_u64().unwrap())
}

/// A Microsoft account beside it, whose contacts are read and never written.
fn microsoft(s: &mut Session) -> Id {
    let added = s.run("account.add", &json!({"email": "bo@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    added["id"].as_u64().unwrap()
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn refused(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Err(e @ (CmdError::Failed(_) | CmdError::Said(_))) => e.to_string(),
        other => panic!("{id} {params}: expected a refusal, got {other:?}"),
    }
}

fn person(id: &str, first: &str, email: &str) -> RemoteContact {
    RemoteContact {
        id: id.into(),
        change_key: format!("ck-{id}"),
        first: first.into(),
        emails: vec![Labeled::new("work", email)],
        ..Default::default()
    }
}

fn everybody(account: Id, contacts: Vec<RemoteContact>) -> ContactsDownload {
    ContactsDownload { account, email: "ada@example.org".into(), complete: true, contacts }
}

/// Ada, Bo and Cy as the server has them, taken in.
fn three(s: &mut Session, account: Id) -> Vec<RemoteContact> {
    let all = vec![person("K1", "Ada", "ada.l@example.org"), person("K2", "Bo", "bo@example.org"), person("K3", "Cy", "cy@example.org")];
    assert_eq!(merge_contacts(&mut s.mailbox, &everybody(account, all.clone())).unwrap().added, 3);
    all
}

fn called(s: &Session, account: Id, remote: &str) -> Contact {
    let found = s.mailbox.contacts.values().find(|c| c.origin.account == account && c.origin.id == remote);
    found.map(|c| c.as_ref().clone()).unwrap_or_else(|| panic!("no contact {remote}"))
}

fn has(s: &Session, account: Id, remote: &str) -> bool {
    s.mailbox.contacts.values().any(|c| c.origin.account == account && c.origin.id == remote)
}

fn upload(s: &Session, account: Id) -> ContactsUpload {
    contact_upload(&s.mailbox, account).unwrap()
}

fn outcome(op: &ContactOp, result: ContactResult) -> ContactOutcome {
    let changed = if result == ContactResult::Done { op.changed.clone() } else { Vec::new() };
    ContactOutcome { kind: op.kind, contact: op.contact, id: op.id.clone(), result, changed, error: String::new() }
}

fn told(account: Id, outcomes: Vec<ContactOutcome>) -> ContactsUploaded {
    ContactsUploaded { account, email: "ada@example.org".into(), outcomes }
}

/// Every op of `sent` answered with `result`; a creation that is done gets the id `made`.
fn all(sent: &ContactsUpload, result: ContactResult, made: &str) -> ContactsUploaded {
    let answer = |op: &ContactOp| {
        let id = if op.kind == ContactOpKind::Create && result == ContactResult::Done { made.to_string() } else { op.id.clone() };
        ContactOutcome { id, ..outcome(op, result) }
    };
    told(sent.account, sent.ops.iter().map(answer).collect())
}

#[test]
fn every_change_made_here_is_worked_out_for_the_server() {
    let (mut s, a) = session();
    three(&mut s, a);
    assert_eq!((contacts_waiting(&s.mailbox, a), upload(&s, a).ops.len()), (0, 0));
    let (ada, bo) = (called(&s, a, "K1"), called(&s, a, "K2"));
    assert_eq!(ada.synced.as_deref(), Some(&ada.as_remote()), "what the server said is remembered");
    // A change, a deletion and a new contact.
    run(&mut s, "people.edit", json!({"id": ada.id, "notes": "Met in May.", "phone": "+44 7700 900001"}));
    run(&mut s, "people.delete", json!({"id": bo.id}));
    let made = run(&mut s, "people.new", json!({"first": "Dee", "email": "dee@example.org", "account": a}));
    assert_eq!((&made["account"], &made["keptAt"]), (&json!(a), &json!("ada@example.org")));
    let made = made["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.contact(made).unwrap().origin, Origin { account: a, ..Default::default() });
    assert_eq!(s.mailbox.contact_gone, [ContactGone { account: a, id: "K2".into() }]);

    let sent = upload(&s, a);
    assert_eq!((sent.account, sent.email.as_str(), contacts_waiting(&s.mailbox, a)), (a, "ada@example.org", 3));
    let kinds: Vec<ContactOpKind> = sent.ops.iter().map(|op| op.kind).collect();
    assert_eq!(kinds, [ContactOpKind::Delete, ContactOpKind::Update, ContactOpKind::Create]);
    let (delete, update, create) = (&sent.ops[0], &sent.ops[1], &sent.ops[2]);
    assert_eq!((delete.contact, delete.id.as_str(), &delete.card, delete.changed.len()), (0, "K2", &RemoteContact::default(), 0));
    assert_eq!((update.contact, update.id.as_str(), &update.changed), (ada.id, "K1", &vec![ContactField::Phones, ContactField::Notes]));
    assert_eq!((update.card.id.as_str(), update.card.notes.as_str(), update.card.first.as_str()), ("K1", "Met in May.", "Ada"));
    assert_eq!((create.contact, create.id.as_str(), create.card.first.as_str(), create.card.id.as_str()), (made, "", "Dee", ""));
    assert!(create.changed.is_empty() && carries_delete(&sent));
    // The command says the same, and changes nothing.
    let rev = s.rev();
    assert_eq!(run(&mut s, "sync.contactChanges", json!({"account": a})), serde_json::to_value(&sent).unwrap());
    assert_eq!(s.rev(), rev);
    // A change that is taken back is none.
    run(&mut s, "people.edit", json!({"id": ada.id, "notes": "", "phones": []}));
    assert_eq!(upload(&s, a).ops.len(), 2);
    // What is only kept here is nothing to tell.
    let cy = called(&s, a, "K3").id;
    run(&mut s, "people.favorite", json!({"id": cy, "value": true}));
    run(&mut s, "people.categorize", json!({"id": cy, "category": "Red category"}));
    assert_eq!(upload(&s, a).ops.len(), 2);
}

#[test]
fn an_upload_is_bounded_and_the_rest_goes_with_the_next() {
    let (mut s, a) = session();
    let many: Vec<RemoteContact> = (0..MAX_CONTACT_OPS + 20).map(|n| person(&format!("K{n}"), &format!("P{n}"), "p@example.org")).collect();
    merge_contacts(&mut s.mailbox, &everybody(a, many)).unwrap();
    let ids: Vec<Id> = s.mailbox.contacts.values().filter(|c| c.origin.account == a).map(|c| c.id).collect();
    for id in &ids {
        s.mailbox.contact_mut(*id).unwrap().notes = "Changed here.".into();
    }
    assert_eq!((upload(&s, a).ops.len(), contacts_waiting(&s.mailbox, a)), (MAX_CONTACT_OPS, MAX_CONTACT_OPS + 20));
    // New contacts: no more at once than to-dos.
    let (mut s, a) = session();
    for n in 0..MAX_CONTACT_CREATES + 5 {
        run(&mut s, "people.new", json!({"first": format!("N{n}"), "account": a}));
    }
    assert_eq!(upload(&s, a).ops.len(), MAX_CONTACT_CREATES);
}

#[test]
fn what_the_server_took_is_taken_in_and_nothing_is_left_to_tell() {
    let (mut s, a) = session();
    three(&mut s, a);
    let (ada, bo) = (called(&s, a, "K1").id, called(&s, a, "K2").id);
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Met in May."}));
    run(&mut s, "people.delete", json!({"id": bo}));
    let made = run(&mut s, "people.new", json!({"first": "Dee", "email": "dee@example.org", "account": a}))["id"].as_u64().unwrap();
    let sent = upload(&s, a);
    assert!(s.can_undo());
    let merged = apply_contact_changes(&mut s, &all(&sent, ContactResult::Done, "K9"), Some(&sent)).unwrap();
    assert_eq!((merged.added, merged.updated, merged.removed, merged.skipped, merged.messages.len()), (1, 1, 1, 0, 0));
    // The new contact has the server's id, and what was sent is what the server has.
    let dee = s.mailbox.contact(made).unwrap().clone();
    assert_eq!((dee.origin.account, dee.origin.id.as_str()), (a, "K9"));
    assert_eq!(dee.synced.as_deref(), Some(&dee.as_remote()));
    assert_eq!(called(&s, a, "K1").synced.unwrap().notes, "Met in May.");
    assert!(s.mailbox.contact_gone.is_empty());
    assert_eq!((contacts_waiting(&s.mailbox, a), upload(&s, a).ops.len()), (0, 0));
    // What a server was told can't be undone there: the history is forgotten.
    assert!(!s.can_undo());
    // The download read after it shows the same: nothing changes but the server's own marks.
    let ada_there = RemoteContact { notes: "Met in May.".into(), ..person("K1", "Ada", "ada.l@example.org") };
    let after = everybody(a, vec![ada_there, person("K3", "Cy", "cy@example.org"), person("K9", "Dee", "dee@example.org")]);
    let merged = merge_contacts(&mut s.mailbox, &after).unwrap();
    assert_eq!((merged.added, merged.removed), (0, 0));
    assert_eq!((s.mailbox.contact(made).unwrap().origin.change_key.as_str(), contacts_waiting(&s.mailbox, a)), ("ck-K9", 0));
    assert_eq!(merge_contacts(&mut s.mailbox, &after).unwrap().unchanged, 3, "and again: nothing");
}

#[test]
fn a_part_changed_again_while_the_first_change_was_on_its_way_stays_to_be_told() {
    let (mut s, a) = session();
    three(&mut s, a);
    let ada = called(&s, a, "K1").id;
    run(&mut s, "people.edit", json!({"id": ada, "notes": "One.", "title": "Analyst"}));
    let sent = upload(&s, a);
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Two."}));
    apply_contact_changes(&mut s, &all(&sent, ContactResult::Done, ""), Some(&sent)).unwrap();
    let now = called(&s, a, "K1");
    assert_eq!(
        (now.notes.as_str(), now.synced.as_ref().unwrap().notes.as_str(), now.synced.as_ref().unwrap().title.as_str()),
        ("Two.", "One.", "Analyst")
    );
    let next = upload(&s, a);
    assert_eq!((next.ops.len(), &next.ops[0].changed, next.ops[0].card.notes.as_str()), (1, &vec![ContactField::Notes], "Two."));
    // Only the parts the server says it wrote are taken as written.
    let (mut s, a) = session();
    three(&mut s, a);
    let ada = called(&s, a, "K1").id;
    run(&mut s, "people.edit", json!({"id": ada, "notes": "One.", "title": "Analyst"}));
    let sent = upload(&s, a);
    let partly = ContactOutcome { changed: vec![ContactField::Notes], ..outcome(&sent.ops[0], ContactResult::Done) };
    apply_contact_changes(&mut s, &told(a, vec![partly]), Some(&sent)).unwrap();
    assert_eq!(upload(&s, a).ops[0].changed, [ContactField::Title]);
}

#[test]
fn without_what_was_sent_a_change_that_is_done_is_settled_by_the_download_after_it() {
    let (mut s, a) = session();
    three(&mut s, a);
    let ada = called(&s, a, "K1").id;
    run(&mut s, "people.edit", json!({"id": ada, "notes": "One."}));
    let made = run(&mut s, "people.new", json!({"first": "Dee", "email": "dee@example.org", "account": a}))["id"].as_u64().unwrap();
    let sent = upload(&s, a);
    // Changed again while it was on its way; the host has only the answer.
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Two."}));
    let said = run(&mut s, "sync.applyContactChanges", json!({"result": all(&sent, ContactResult::Done, "K9")}));
    assert_eq!((&said["done"], &said["failed"]), (&json!(2), &json!(0)));
    assert_eq!(s.mailbox.contact(made).unwrap().origin.id, "K9");
    // Nothing is taken for told that may not have been: both are still noted.
    assert_eq!(contacts_waiting(&s.mailbox, a), 2);
    // The download shows what the server has: the new contact is settled, the second change stays.
    let ada_there = RemoteContact { notes: "One.".into(), ..person("K1", "Ada", "ada.l@example.org") };
    let after = everybody(
        a,
        vec![ada_there, person("K2", "Bo", "bo@example.org"), person("K3", "Cy", "cy@example.org"), person("K9", "Dee", "dee@example.org")],
    );
    apply_contacts(&mut s, &after).unwrap();
    let next = upload(&s, a);
    assert_eq!((next.ops.len(), next.ops[0].kind, next.ops[0].card.notes.as_str()), (1, ContactOpKind::Update, "Two."));
    // With what was sent, the command is as exact as the window.
    let said = run(&mut s, "sync.applyContactChanges", json!({"result": all(&next, ContactResult::Done, ""), "sent": next}));
    assert_eq!(said["done"], 1);
    assert_eq!(contacts_waiting(&s.mailbox, a), 0);
}

#[test]
fn a_download_is_merged_part_by_part_with_what_was_changed_here() {
    let (mut s, a) = session();
    let server = three(&mut s, a);
    let ada = called(&s, a, "K1").id;
    // A change on each side, in different parts: both stand.
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Changed here."}));
    let there = RemoteContact { title: "Analyst".into(), change_key: "ck-2".into(), ..server[0].clone() };
    let download = everybody(a, vec![there.clone(), server[1].clone(), server[2].clone()]);
    let merged = merge_contacts(&mut s.mailbox, &download).unwrap();
    assert_eq!((merged.updated, merged.unchanged), (1, 2));
    let now = called(&s, a, "K1");
    assert_eq!((now.notes.as_str(), now.title.as_str(), now.origin.change_key.as_str()), ("Changed here.", "Analyst", "ck-2"));
    assert_eq!(now.synced.as_deref(), Some(&there), "what the server has is what it said");
    assert_eq!(now.changed_fields(), [ContactField::Notes]);
    // Twice: nothing the second time.
    let snapshot = s.mailbox.clone();
    assert_eq!(merge_contacts(&mut s.mailbox, &download).unwrap().unchanged, 3);
    assert!(s.mailbox == snapshot);
    // Both sides changed one part: what was changed here stays, and goes up.
    run(&mut s, "people.edit", json!({"id": ada, "company": "Here Ltd"}));
    let there = RemoteContact { company: "There Ltd".into(), department: "Engines".into(), ..there };
    merge_contacts(&mut s.mailbox, &everybody(a, vec![there, server[1].clone(), server[2].clone()])).unwrap();
    let now = called(&s, a, "K1");
    assert_eq!((now.company.as_str(), now.department.as_str(), now.synced.as_ref().unwrap().company.as_str()), ("Here Ltd", "", "There Ltd"));
    assert_eq!(now.changed_fields(), [ContactField::Organization, ContactField::Notes]);
    // What is only kept here stays through it all.
    run(&mut s, "people.favorite", json!({"id": ada, "value": true}));
    merge_contacts(&mut s.mailbox, &everybody(a, server.clone())).unwrap();
    let now = called(&s, a, "K1");
    assert!(now.favorite && now.id == ada && now.title.is_empty() && now.notes == "Changed here.");
}

#[test]
fn a_deletion_here_and_a_deletion_there_against_a_download() {
    let (mut s, a) = session();
    let server = three(&mut s, a);
    let (ada, bo, cy) = (called(&s, a, "K1").id, called(&s, a, "K2").id, called(&s, a, "K3").id);
    // Deleted here: a download that still shows it does not bring it back.
    run(&mut s, "people.delete", json!({"id": bo}));
    let merged = merge_contacts(&mut s.mailbox, &everybody(a, server.clone())).unwrap();
    assert_eq!((merged.added, merged.skipped), (0, 1));
    assert!(!has(&s, a, "K2") && s.mailbox.contact_gone.len() == 1);
    // Deleted there, untouched here: it goes, and nobody is told to delete it.
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Changed here."}));
    let merged = merge_contacts(&mut s.mailbox, &everybody(a, Vec::new())).unwrap();
    assert_eq!((merged.removed, merged.updated), (1, 1));
    assert!(s.mailbox.contact(cy).is_none());
    assert_eq!(s.mailbox.contact_gone, [ContactGone { account: a, id: "K2".into() }]);
    // Deleted there, changed here since: made again rather than lost.
    let again = s.mailbox.contact(ada).unwrap();
    assert_eq!((&again.origin, again.synced.is_none(), again.notes.as_str()), (&Origin { account: a, ..Default::default() }, true, "Changed here."));
    let sent = upload(&s, a);
    assert_eq!(sent.ops.iter().map(|op| op.kind).collect::<Vec<_>>(), [ContactOpKind::Delete, ContactOpKind::Create]);
    // A list that isn't all there is removes nobody.
    let (mut s, a) = session();
    three(&mut s, a);
    let partial = ContactsDownload { complete: false, ..everybody(a, Vec::new()) };
    assert_eq!(merge_contacts(&mut s.mailbox, &partial).unwrap().removed, 0);
    assert!(has(&s, a, "K1"));
}

#[test]
fn a_contact_the_server_is_yet_to_be_given_is_never_removed_and_its_twin_is_adopted() {
    let (mut s, a) = session();
    let server = three(&mut s, a);
    let made = run(&mut s, "people.new", json!({"first": "Dee", "last": "Park", "email": "Dee@example.org", "phone": "+1 555 0100", "account": a}));
    let made = made["id"].as_u64().unwrap();
    let count = s.mailbox.contacts.len();
    // A complete download without it leaves it waiting.
    merge_contacts(&mut s.mailbox, &everybody(a, server.clone())).unwrap();
    assert_eq!((s.mailbox.contacts.len(), upload(&s, a).ops.len()), (count, 1));
    // The answer to making it was lost, and the download shows it (the server spells a label its
    // own way): it is that contact, not a second one.
    let there = RemoteContact {
        last: "Park".into(),
        emails: vec![Labeled::new("Work", "dee@example.org")],
        phones: vec![Labeled::new("cell", "+1 555 0100")],
        ..person("K9", "Dee", "")
    };
    let mut with = server.clone();
    with.push(there.clone());
    let merged = merge_contacts(&mut s.mailbox, &everybody(a, with.clone())).unwrap();
    assert_eq!((merged.added, merged.updated, s.mailbox.contacts.len()), (0, 1, count));
    let dee = s.mailbox.contact(made).unwrap().clone();
    assert_eq!((dee.origin.id.as_str(), dee.synced.as_deref()), ("K9", Some(&there)));
    assert!(upload(&s, a).ops.iter().all(|op| op.kind == ContactOpKind::Update), "nothing is made twice");
    // Somebody else of the same first name is somebody else.
    let other = run(&mut s, "people.new", json!({"first": "Dee", "email": "dee.other@example.org", "account": a}))["id"].as_u64().unwrap();
    merge_contacts(&mut s.mailbox, &everybody(a, with)).unwrap();
    assert!(s.mailbox.contact(other).unwrap().origin.id.is_empty());
}

#[test]
fn a_new_contact_deleted_while_it_was_being_made_is_deleted_there_too() {
    let (mut s, a) = session();
    let made = run(&mut s, "people.new", json!({"first": "Dee", "account": a}))["id"].as_u64().unwrap();
    let sent = upload(&s, a);
    run(&mut s, "people.delete", json!({"id": made}));
    assert!(s.mailbox.contact_gone.is_empty(), "the server never had it, as far as is known");
    apply_contact_changes(&mut s, &all(&sent, ContactResult::Done, "K9"), Some(&sent)).unwrap();
    assert_eq!(s.mailbox.contact_gone, [ContactGone { account: a, id: "K9".into() }]);
    let next = upload(&s, a);
    assert_eq!((next.ops.len(), next.ops[0].kind, next.ops[0].id.as_str()), (1, ContactOpKind::Delete, "K9"));
    // And the download that shows it meanwhile does not bring it back.
    merge_contacts(&mut s.mailbox, &everybody(a, vec![person("K9", "Dee", "")])).unwrap();
    assert!(!has(&s, a, "K9"));
    apply_contact_changes(&mut s, &all(&next, ContactResult::Gone, ""), Some(&next)).unwrap();
    assert!(s.mailbox.contact_gone.is_empty() && upload(&s, a).ops.is_empty());
}

#[test]
fn a_change_to_a_contact_the_server_no_longer_has_makes_it_again() {
    let (mut s, a) = session();
    three(&mut s, a);
    let ada = called(&s, a, "K1").id;
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Changed here."}));
    let sent = upload(&s, a);
    let merged = apply_contact_changes(&mut s, &all(&sent, ContactResult::Gone, ""), Some(&sent)).unwrap();
    assert_eq!(merged.updated, 1);
    let again = s.mailbox.contact(ada).unwrap();
    assert!(again.origin.id.is_empty() && again.origin.account == a && again.synced.is_none());
    let next = upload(&s, a);
    assert_eq!((next.ops[0].kind, next.ops[0].contact, next.ops[0].card.notes.as_str()), (ContactOpKind::Create, ada, "Changed here."));
}

#[test]
fn what_the_server_refuses_is_put_back_told_once_and_not_sent_again() {
    let (mut s, a) = session();
    three(&mut s, a);
    let (ada, bo) = (called(&s, a, "K1").id, called(&s, a, "K2").id);
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Too long for it.", "title": "Analyst"}));
    run(&mut s, "people.delete", json!({"id": bo}));
    let made = run(&mut s, "people.new", json!({"first": "Dee", "account": a}))["id"].as_u64().unwrap();
    let sent = upload(&s, a);
    let mut refusals = all(&sent, ContactResult::Refused, "");
    refusals.outcomes[1].error = "invalidProperties: notes\u{0}".into();
    let merged = apply_contact_changes(&mut s, &refusals, Some(&sent)).unwrap();
    assert_eq!((merged.put_back, merged.skipped), (3, 0));
    assert_eq!(
        merged.messages,
        [
            "The server of ada@example.org didn't delete a contact that was deleted here; it comes back with the next Send/Receive",
            "The server of ada@example.org didn't take a change to the contact \"Ada\"; what it refused is as the server has it again: invalidProperties: notes",
            "The server of ada@example.org didn't take the new contact \"Dee\"; it stays on this computer",
        ]
    );
    // The change is as the server has it again; the new contact is this computer's; the deletion
    // is forgotten, and the next download brings the contact back.
    let now = called(&s, a, "K1");
    assert!(now.notes.is_empty() && now.title.is_empty() && now.changed_fields().is_empty());
    let dee = s.mailbox.contact(made).unwrap();
    assert!(dee.origin.is_local() && dee.synced.is_none());
    assert!(s.mailbox.contact_gone.is_empty());
    // Nothing of it is sent again as it is.
    assert_eq!((contacts_waiting(&s.mailbox, a), upload(&s, a).ops.len()), (0, 0));
    // A refusal is no reason to forget the history: nothing went to the server.
    assert!(s.can_undo());
    // The same answer once more (a host that says it twice) says nothing more of what is settled.
    let again = apply_contact_changes(&mut s, &refusals, Some(&sent)).unwrap();
    assert_eq!((again.put_back, again.messages.len(), again.skipped), (1, 1, 2), "only the change's line, whose contact is still there");
    merge_contacts(&mut s.mailbox, &everybody(a, vec![person("K1", "Ada", "ada.l@example.org"), person("K2", "Bo", "bo@example.org")])).unwrap();
    assert!(has(&s, a, "K2"));
    // A part changed once more since it was sent is not what was refused: it stays, to be tried.
    run(&mut s, "people.edit", json!({"id": ada, "notes": "One.", "title": "Analyst"}));
    let sent = upload(&s, a);
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Two."}));
    apply_contact_changes(&mut s, &all(&sent, ContactResult::Refused, ""), Some(&sent)).unwrap();
    let now = called(&s, a, "K1");
    assert_eq!((now.notes.as_str(), now.title.as_str(), now.changed_fields()), ("Two.", "", vec![ContactField::Notes]));
    // Without what was sent, what the answer names is put back; when it names nothing, all of it.
    let sent = upload(&s, a);
    apply_contact_outcomes(&mut s.mailbox, &all(&sent, ContactResult::Refused, "")).unwrap();
    assert!(called(&s, a, "K1").notes.is_empty() && upload(&s, a).ops.is_empty());
}

#[test]
fn what_could_not_be_asked_changes_nothing_and_goes_again() {
    let (mut s, a) = session();
    three(&mut s, a);
    let (ada, bo) = (called(&s, a, "K1").id, called(&s, a, "K2").id);
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Changed here."}));
    run(&mut s, "people.delete", json!({"id": bo}));
    run(&mut s, "people.new", json!({"first": "Dee", "account": a}));
    let sent = upload(&s, a);
    let (snapshot, rev) = (s.mailbox.clone(), s.rev());
    let merged = apply_contact_changes(&mut s, &all(&sent, ContactResult::Failed, ""), Some(&sent)).unwrap();
    assert_eq!((merged.skipped, merged.messages.len()), (3, 0));
    assert!(s.mailbox == snapshot && s.rev() == rev && s.can_undo());
    assert_eq!(upload(&s, a), sent);
    // A change with no answer at all is worked out again too.
    apply_contact_changes(&mut s, &told(a, Vec::new()), Some(&sent)).unwrap();
    assert_eq!(upload(&s, a), sent);
}

#[test]
fn an_answer_from_outside_is_hostile() {
    let (mut s, a) = session();
    let other = microsoft(&mut s);
    three(&mut s, a);
    let ada = called(&s, a, "K1").id;
    let local = s.mailbox.contacts.values().find(|c| c.origin.is_local()).unwrap().id;
    let snapshot = s.mailbox.clone();
    let of = |kind, contact: Id, id: &str, result| ContactOutcome { kind, contact, id: id.into(), result, ..Default::default() };
    let junk = vec![
        // A creation that names a contact of this computer, one that has an id, nobody, no id.
        of(ContactOpKind::Create, local, "X1", ContactResult::Done),
        of(ContactOpKind::Create, ada, "X2", ContactResult::Done),
        of(ContactOpKind::Create, 0, "", ContactResult::Done),
        of(ContactOpKind::Create, local, "", ContactResult::Refused),
        // A change that names a contact by another id, or one that is not this account's.
        of(ContactOpKind::Update, ada, "K2", ContactResult::Gone),
        of(ContactOpKind::Update, local, "", ContactResult::Gone),
        of(ContactOpKind::Update, local, "K1", ContactResult::Refused),
        of(ContactOpKind::Update, 424_242, "K1", ContactResult::Done),
        // A deletion nobody asked for, and an id that is none.
        of(ContactOpKind::Delete, 0, "K3", ContactResult::Done),
        of(ContactOpKind::Delete, 0, &"x".repeat(5_000), ContactResult::Gone),
        of(ContactOpKind::Create, ada, &"y".repeat(5_000), ContactResult::Done),
    ];
    let merged = apply_contact_outcomes(&mut s.mailbox, &told(a, junk.clone())).unwrap();
    assert_eq!((merged.skipped, merged.messages.len(), merged.changed()), (junk.len(), 0, false));
    assert!(s.mailbox == snapshot);
    // A creation that was never asked for, of a contact that isn't here: nothing is noted for the
    // server to delete.
    let (stray, asked) = (told(a, vec![of(ContactOpKind::Create, 424_242, "X9", ContactResult::Done)]), upload(&s, a));
    assert_eq!(apply_contact_changes(&mut s, &stray, Some(&asked)).unwrap().skipped, 1);
    assert!(s.mailbox == snapshot);
    // For another account, for one that doesn't write, for nobody: refused whole.
    let stranger = ContactsUploaded { email: "eve@example.org".into(), ..told(a, junk.clone()) };
    assert!(apply_contact_outcomes(&mut s.mailbox, &stranger).is_err());
    assert!(apply_contact_outcomes(&mut s.mailbox, &ContactsUploaded { account: other, email: String::new(), outcomes: junk.clone() }).is_err());
    assert!(apply_contact_outcomes(&mut s.mailbox, &ContactsUploaded { account: 424_242, ..Default::default() }).is_err());
    for params in
        [json!({}), json!({"result": 5}), json!({"result": {"account": a, "outcomes": "x"}}), json!({"result": told(a, Vec::new()), "sent": 7})]
    {
        assert!(s.run("sync.applyContactChanges", &params).is_err(), "{params}");
    }
    for params in [json!({}), json!({"account": other}), json!({"account": 424_242}), json!({"account": "x"})] {
        assert!(s.run("sync.contactChanges", &params).is_err(), "{params}");
    }
    assert!(s.mailbox == snapshot);
    // What a mailbox file says is waiting: an id that is none is never sent, a deletion of a
    // contact that is here is not carried out, and none is sent twice.
    s.mailbox.contact_gone = vec![
        ContactGone { account: a, id: String::new() },
        ContactGone { account: a, id: "bad\u{0}id".into() },
        ContactGone { account: a, id: "K1".into() },
        ContactGone { account: a, id: "K7".into() },
        ContactGone { account: a, id: "K7".into() },
        ContactGone { account: other, id: "M1".into() },
    ];
    let sent = upload(&s, a);
    assert_eq!(sent.ops.iter().map(|op| op.id.as_str()).collect::<Vec<_>>(), ["K7"]);
    assert_eq!(contacts_waiting(&s.mailbox, a), 1);
    // A merge lets go of what can't be one.
    let merged = merge_contacts(&mut s.mailbox, &ContactsDownload { complete: false, ..everybody(a, Vec::new()) }).unwrap();
    assert!(merged.changed());
    assert!(s.mailbox.contact_gone.iter().filter(|gone| gone.account == a).all(|gone| gone.id.starts_with('K')));
    // More deletions than may wait: refused, not forgotten without a word.
    s.mailbox.contact_gone = (0..MAX_CONTACT_GONE).map(|n| ContactGone { account: a, id: format!("G{n}") }).collect();
    assert!(refused(&mut s, "people.delete", json!({"id": ada})).starts_with("Too many deleted contacts are waiting"));
    assert!(s.mailbox.contact(ada).is_some());
    run(&mut s, "people.delete", json!({"id": local}));
}

#[test]
fn a_microsoft_accounts_contacts_are_read_and_never_written_as_before() {
    let (mut s, _) = session();
    let m = microsoft(&mut s);
    let download =
        ContactsDownload { account: m, email: "bo@example.org".into(), complete: true, contacts: vec![person("M1", "Mo", "mo@example.org")] };
    merge_contacts(&mut s.mailbox, &download).unwrap();
    let mo = called(&s, m, "M1");
    assert!(mo.synced.is_none(), "nothing is remembered of a server that is only read");
    let why =
        "This contact is kept by the server of bo@example.org. Changes made here are not sent back yet, so it can't be changed or deleted here.";
    assert_eq!(refused(&mut s, "people.edit", json!({"id": mo.id, "notes": "x"})), why);
    assert_eq!(refused(&mut s, "people.delete", json!({"id": mo.id})), why);
    assert!(s.run("people.new", &json!({"first": "Dee", "account": m})).is_err());
    assert!(contact_upload(&s.mailbox, m).is_err());
    assert_eq!(contacts_waiting(&s.mailbox, m), 0);
    // Its contact can't be merged either, with the words there were.
    let mine = run(&mut s, "people.new", json!({"first": "Mo", "email": "mo@example.org", "account": 0}))["id"].as_u64().unwrap();
    assert!(refused(&mut s, "people.merge", json!({"keep": mine, "others": [mo.id]})).contains("so it can't be merged here"));
    // Gone from a complete download: removed, and nobody is told anything.
    merge_contacts(&mut s.mailbox, &ContactsDownload { contacts: Vec::new(), ..download }).unwrap();
    assert!(!has(&s, m, "M1") && s.mailbox.contact_gone.is_empty());
}

#[test]
fn a_new_contact_goes_where_the_owner_says_or_where_people_shows() {
    let (mut s, a) = session();
    let m = microsoft(&mut s);
    let place = |s: &Session, id: Id| s.mailbox.contact(id).unwrap().origin.clone();
    let new = |s: &mut Session, params: Value| run(s, "people.new", params)["id"].as_u64().unwrap();
    // With every account shown, and with a reading account shown: on this computer.
    let one = new(&mut s, json!({"first": "One"}));
    assert!(place(&s, one).is_local());
    run(&mut s, "view.peopleAccount", json!({"value": m}));
    let here = new(&mut s, json!({"first": "Two"}));
    assert!(place(&s, here).is_local());
    assert_eq!(super::people::shown_account(&s), AccountChoice::Here);
    // People shows the writing account: there, and the list stays on it.
    run(&mut s, "view.peopleAccount", json!({"value": a}));
    assert_eq!(super::people::new_contact_place(&s), a);
    let there = new(&mut s, json!({"first": "Three"}));
    assert_eq!(place(&s, there), Origin { account: a, ..Default::default() });
    assert_eq!((super::people::shown_account(&s), s.sel.contact), (AccountChoice::Account(a), Some(there)));
    // Said outright: this computer, or the account, whatever is shown.
    let four = new(&mut s, json!({"first": "Four", "account": 0}));
    assert!(place(&s, four).is_local());
    run(&mut s, "view.peopleAccount", json!({"value": "all"}));
    let five = new(&mut s, json!({"first": "Five", "account": a}));
    assert_eq!(place(&s, five).account, a);
    for account in [json!(m), json!(424_242), json!("x"), json!(-1), json!(null)] {
        assert!(s.run("people.new", &json!({"first": "Six", "account": account})).is_err(), "{account}");
    }
    // A contact stays in its account.
    assert!(s.run("people.edit", &json!({"id": there, "account": 0})).is_err());
    run(&mut s, "people.edit", json!({"id": there, "account": a, "last": "Example"}));
    // Add to Contacts follows the same.
    let message = s.mailbox.messages.values().find(|m| m.from.is_valid() && s.mailbox.contact_by_email(&m.from.email).is_none()).unwrap().id;
    let made = run(&mut s, "people.fromMessage", json!({"id": message, "account": a}))["id"].as_u64().unwrap();
    assert_eq!(place(&s, made).account, a);
    // It is a new contact for the server, and its undo is an undo like any other.
    assert_eq!(upload(&s, a).ops.iter().filter(|op| op.kind == ContactOpKind::Create).count(), 3);
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact(made).is_none());
    assert_eq!(upload(&s, a).ops.len(), 2);
}

#[test]
fn undo_before_the_server_hears_takes_the_change_back_and_after_there_is_none() {
    let (mut s, a) = session();
    three(&mut s, a);
    let (ada, bo) = (called(&s, a, "K1").id, called(&s, a, "K2").id);
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Changed here."}));
    run(&mut s, "people.delete", json!({"id": bo}));
    assert_eq!(upload(&s, a).ops.len(), 2);
    // Each is undone like any other change, and what was noted for the server goes with it.
    run(&mut s, "edit.undo", json!({}));
    assert!(has(&s, a, "K2") && s.mailbox.contact_gone.is_empty());
    run(&mut s, "edit.undo", json!({}));
    assert!(upload(&s, a).ops.is_empty() && called(&s, a, "K1").notes.is_empty());
    run(&mut s, "edit.redo", json!({}));
    run(&mut s, "edit.redo", json!({}));
    let sent = upload(&s, a);
    assert_eq!(sent.ops.len(), 2);
    // A job that carries a deletion forgets the history as it starts; one that doesn't, not.
    mark_sent(&mut s, false);
    assert!(s.can_undo());
    mark_sent(&mut s, carries_delete(&sent));
    assert!(!s.can_undo());
    // A change undone while it was on its way: the server is told the opposite.
    let (mut s, a) = session();
    three(&mut s, a);
    let ada = called(&s, a, "K1").id;
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Changed here."}));
    let sent = upload(&s, a);
    run(&mut s, "edit.undo", json!({}));
    apply_contact_changes(&mut s, &all(&sent, ContactResult::Done, ""), Some(&sent)).unwrap();
    let back = upload(&s, a);
    assert_eq!((back.ops.len(), &back.ops[0].changed, back.ops[0].card.notes.as_str()), (1, &vec![ContactField::Notes], ""));
}

#[test]
fn a_contact_read_before_changes_were_written_is_changed_and_told_all_the_same() {
    let (mut s, a) = session();
    three(&mut s, a);
    // As a version from before left it: the server's copy, and nothing remembered of the server.
    let ada = called(&s, a, "K1").id;
    s.mailbox.contact_mut(ada).unwrap().synced = None;
    run(&mut s, "people.edit", json!({"id": ada, "notes": "Changed here."}));
    let sent = upload(&s, a);
    assert_eq!((sent.ops.len(), &sent.ops[0].changed), (1, &vec![ContactField::Notes]));
    // And a download before any change makes it remember.
    let bo = called(&s, a, "K2").id;
    s.mailbox.contact_mut(bo).unwrap().synced = None;
    merge_contacts(&mut s.mailbox, &everybody(a, vec![person("K2", "Bo", "bo@example.org")])).unwrap();
    assert!(s.mailbox.contact(bo).unwrap().synced.is_some());
}

#[test]
fn a_merge_of_an_accounts_contacts_is_told_to_its_server() {
    let (mut s, a) = session();
    let twice = vec![
        person("K1", "Ada", "ada.l@example.org"),
        RemoteContact { phones: vec![Labeled::new("mobile", "+44 7700 900000")], ..person("K2", "Ada", "ada.l@example.org") },
    ];
    merge_contacts(&mut s.mailbox, &everybody(a, twice)).unwrap();
    let (keep, other) = (called(&s, a, "K1").id, called(&s, a, "K2").id);
    let found = run(&mut s, "people.findDuplicates", json!({"ids": [keep, other]}));
    assert_eq!((&found["groups"][0]["mergeable"], found["groups"][0]["locked"].as_array().unwrap().len()), (&json!(true), 0));
    run(&mut s, "people.merge", json!({"keep": keep, "others": [other]}));
    let sent = upload(&s, a);
    assert_eq!(
        sent.ops.iter().map(|op| (op.kind, op.id.as_str())).collect::<Vec<_>>(),
        [(ContactOpKind::Delete, "K2"), (ContactOpKind::Update, "K1")]
    );
    assert_eq!(sent.ops[1].changed, [ContactField::Phones]);
    // One undo takes the whole merge back, and with it what was noted for the server.
    run(&mut s, "edit.undo", json!({}));
    assert!(upload(&s, a).ops.is_empty() && has(&s, a, "K2"));
}

#[test]
fn an_account_that_goes_takes_what_waited_for_its_server_and_leaves_what_was_made_here() {
    let (mut s, a) = session();
    three(&mut s, a);
    let bo = called(&s, a, "K2").id;
    run(&mut s, "people.delete", json!({"id": bo}));
    let made = run(&mut s, "people.new", json!({"first": "Dee", "account": a}))["id"].as_u64().unwrap();
    run(&mut s, "view.peopleAccount", json!({"value": a}));
    run(&mut s, "sync.forgetRemote", json!({"account": a}));
    assert!(s.mailbox.contact_gone.is_empty());
    assert!(s.mailbox.contacts.values().all(|c| c.origin.account != a));
    assert!(s.mailbox.contact(made).is_some_and(|c| c.origin.is_local() && c.synced.is_none()));
    assert!(upload(&s, a).ops.is_empty());
    // Removing the account does the same.
    let again = run(&mut s, "people.new", json!({"first": "Eve", "account": a}))["id"].as_u64().unwrap();
    s.mailbox.contact_gone.push(ContactGone { account: a, id: "K5".into() });
    run(&mut s, "account.remove", json!({"id": a}));
    assert!(s.mailbox.contact_gone.is_empty() && s.mailbox.contact(again).is_some_and(|c| c.origin.is_local()));
    assert!(s.mailbox.clone().repair().is_empty());
}
